//! Stage 1 acceptance test (doc 03): A publishes, B reshares and hosts, A
//! goes offline, and C still gets verified media from B.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.
//! Requires ffmpeg.

use std::path::{Path, PathBuf};

use iroh::{EndpointAddr, EndpointId, SecretKey};
use osp_node::{BlobClass, Config, Home, Node};
use osp_proto::{
    BlobHash, FollowTicket, ObjectId,
    rpc::{ClientRequest, ClientResponse, Direction, FeedItem, PeerRecord, StatusInfo},
};

async fn start(root: &Path, name: &str) -> Node {
    let home = root.join(name.to_lowercase());
    Node::init(&home, &Config::local_test(name)).unwrap();
    Node::spawn(&home).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn invite(node: &Node) -> String {
    match call(node, ClientRequest::Invite).await {
        ClientResponse::Ticket { ticket } => ticket,
        other => panic!("unexpected {other:?}"),
    }
}

async fn follow(node: &Node, ticket: String) {
    call(node, ClientRequest::Follow { ticket }).await;
}

/// Syncs and waits for prefetching. Following also wakes a background sync,
/// so new objects may already have arrived; check the feed, not the count.
async fn sync(node: &Node) {
    match call(node, ClientRequest::Sync).await {
        ClientResponse::Synced { .. } => {}
        other => panic!("unexpected {other:?}"),
    }
}

async fn status(node: &Node) -> StatusInfo {
    match call(node, ClientRequest::Status).await {
        ClientResponse::Status(status) => status,
        other => panic!("unexpected {other:?}"),
    }
}

async fn feed(node: &Node) -> Vec<FeedItem> {
    match call(node, ClientRequest::Feed { limit: 50 }).await {
        ClientResponse::Feed { items } => items,
        other => panic!("unexpected {other:?}"),
    }
}

/// What `node` measured about the node `other` (ADR 0011).
async fn peer(node: &Node, other: EndpointId) -> Option<PeerRecord> {
    match call(node, ClientRequest::Peers).await {
        ClientResponse::Peers { peers } => peers.into_iter().find(|p| p.node == other),
        other => panic!("unexpected {other:?}"),
    }
}

/// What `node` measured about `other` once it has served it something. The
/// serving side records a transfer when it ends there, which can be just
/// after the other side has what it asked for.
async fn served(node: &Node, other: EndpointId) -> PeerRecord {
    for _ in 0..50 {
        if let Some(p) = peer(node, other).await
            && p.counts.transfers > 0.0
        {
            return p;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("no transfer to that node was recorded");
}

async fn make_clip(dir: &Path) -> PathBuf {
    let out = dir.join("clip.mp4");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=640x360:rate=30",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&out)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn reshared_media_survives_origin_going_offline() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    let carol = start(dir.path(), "Carol").await;

    // Alice posts a video.
    let clip = make_clip(dir.path()).await;
    let ClientResponse::Posted { id: post_id } = call(
        &alice,
        ClientRequest::Post {
            content: "first light".into(),
            attachments: vec![clip],
            uploads: vec![],
            upload: None,
        },
    )
    .await
    else {
        panic!("post failed")
    };

    // Bob follows Alice and gathers the post and its media.
    follow(&bob, invite(&alice).await).await;
    sync(&bob).await;
    let bob_feed = feed(&bob).await;
    let item = bob_feed
        .iter()
        .find(|i| i.id == post_id)
        .expect("post in Bob's feed");
    assert_eq!(item.actor_name.as_deref(), Some("Alice"));
    let video: BlobHash = item.attachments[0].attachment.blob;
    assert!(
        item.attachments[0].local,
        "Bob should have prefetched the video"
    );
    assert_eq!(bob.blob_class(&video).unwrap(), Some(BlobClass::Prefetch));

    // Bob reshares and offers to host.
    call(
        &bob,
        ClientRequest::Reshare {
            id: post_id,
            host: true,
        },
    )
    .await;
    assert_eq!(bob.blob_class(&video).unwrap(), Some(BlobClass::Replica));

    // Carol follows Bob only. Then Alice disappears.
    follow(&carol, invite(&bob).await).await;
    alice.shutdown().await.unwrap();

    sync(&carol).await;
    let carol_feed = feed(&carol).await;
    let item = carol_feed
        .iter()
        .find(|i| i.id == post_id)
        .expect("reshared post in Carol's feed");
    assert_eq!(
        item.actor_name.as_deref(),
        Some("Alice"),
        "Bob passed on Alice's signed profile with the reshare"
    );
    let by = item.reshared_by.as_ref().expect("shown as a reshare");
    assert_eq!(by.name.as_deref(), Some("Bob"));
    assert!(by.hosting);
    assert!(item.attachments[0].local, "Carol prefetched from Bob");

    // Both ends list the connection, including the side that didn't open it.
    let peer = |status: &StatusInfo, node| {
        status
            .peers
            .iter()
            .find(|p| p.node == node)
            .map(|p| p.direction)
    };
    assert_eq!(
        peer(&status(&bob).await, carol.node_id()),
        Some(Direction::Incoming)
    );
    assert_eq!(
        peer(&status(&carol).await, bob.node_id()),
        Some(Direction::Outgoing)
    );

    // Export the file and check it is byte-for-byte what Alice signed.
    let out_dir = dir.path().join("carol-out");
    let ClientResponse::Fetched { files } = call(
        &carol,
        ClientRequest::Fetch {
            id: post_id,
            out_dir: Some(out_dir.clone()),
        },
    )
    .await
    else {
        panic!("fetch failed")
    };
    let bytes = std::fs::read(files[0].path.as_ref().unwrap()).unwrap();
    assert_eq!(blake3::hash(&bytes).as_bytes(), video.as_bytes());

    // Serving rules: a stranger may download Bob's replica, but not what
    // Carol prefetched: she doesn't share it.
    let dave = start(dir.path(), "Dave").await;
    let size = files[0].size;
    assert!(
        dave.download_blob_from(&video, size, &[carol.addr().await])
            .await
            .is_err(),
        "Carol must not serve her prefetched copy to strangers"
    );
    dave.download_blob_from(&video, size, &[bob.addr().await])
        .await
        .expect("Bob serves his replica to anyone");

    // Showing the reshare by its own ID resolves to the original post.
    let ClientResponse::Item { item } = call(&carol, ClientRequest::Show { id: by.announce }).await
    else {
        panic!("show failed")
    };
    assert_eq!(item.id, post_id);

    for node in [bob, carol, dave] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_node_records_the_nodes_it_deals_with() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    let clip = make_clip(dir.path()).await;
    call(
        &alice,
        ClientRequest::Post {
            content: "first light".into(),
            attachments: vec![clip],
            uploads: vec![],
            upload: None,
        },
    )
    .await;
    follow(&bob, invite(&alice).await).await;
    sync(&bob).await;
    let video = feed(&bob).await[0].attachments[0].attachment.clone();

    // Bob asked Alice's node for her profile and posts, and downloaded her
    // video from it. Counts fade a little even within a test, so they're rounded.
    let seen = peer(&bob, alice.node_id())
        .await
        .expect("a record of Alice's node");
    assert_eq!(seen.name.as_deref(), Some("Alice"));
    assert_eq!(seen.actor, Some(alice.identity()));
    assert!(
        seen.followed && seen.contacted && !seen.follower,
        "{seen:?}"
    );
    // An author's node hosts the media of their posts.
    assert!(seen.host, "{seen:?}");
    assert!(seen.first_contact_secs_ago < 60 && seen.last_contact_secs_ago < 60);
    let c = &seen.counts;
    assert!(c.requests.round() >= 2.0, "{c:?}");
    assert!((c.answered - c.requests).abs() < 1e-6, "{c:?}");
    assert!(c.answer_ms > 0.0 && c.unreached == 0.0, "{c:?}");
    assert!(c.completed.round() >= 1.0 && c.failed == 0.0, "{c:?}");
    assert!(c.received_bytes.round() >= video.size as f64, "{c:?}");
    assert!(c.receive_ms > 0.0 && c.first_bytes.round() >= 1.0, "{c:?}");

    // Alice's node sent it to Bob's, which follows hers.
    let to_bob = served(&alice, bob.node_id()).await;
    assert!(to_bob.follower && !to_bob.followed, "{to_bob:?}");
    assert!(to_bob.counts.transfers.round() >= 1.0, "{to_bob:?}");
    assert!(
        to_bob.counts.sent_bytes.round() >= video.size as f64,
        "{to_bob:?}"
    );

    // A stranger's download is recorded on both sides too.
    let dave = start(dir.path(), "Dave").await;
    dave.download_blob_from(&video.blob, video.size, &[alice.addr().await])
        .await
        .unwrap();
    let fetched = peer(&dave, alice.node_id())
        .await
        .expect("a record of Alice's node");
    assert!(fetched.contacted && !fetched.followed && fetched.name.is_none());
    assert!(!fetched.host, "{fetched:?}");
    assert!(fetched.counts.completed.round() >= 1.0, "{fetched:?}");
    let stranger = served(&alice, dave.node_id()).await;
    assert!(!stranger.contacted && !stranger.follower, "{stranger:?}");
    assert!(
        stranger.counts.sent_bytes.round() >= video.size as f64,
        "{stranger:?}"
    );

    // A node that can't be reached: its address names no way to reach it.
    let nowhere = EndpointAddr::new(SecretKey::generate().public());
    let ticket = FollowTicket {
        actor: SecretKey::generate().public(),
        node: nowhere.clone(),
    };
    follow(&dave, ticket.to_string()).await;
    sync(&dave).await;
    let missing = peer(&dave, nowhere.id)
        .await
        .expect("a record of that node");
    let c = &missing.counts;
    assert_eq!((c.tries.round(), c.unreached.round()), (1.0, 1.0), "{c:?}");
    assert_eq!((c.requests.round(), c.answered), (1.0, 0.0), "{c:?}");

    for node in [alice, bob, dave] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_node_on_the_same_home_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("frank");
    Node::init(&home, &Config::local_test("Frank")).unwrap();
    let first = Node::spawn(&home).await.unwrap();
    let second = tokio::time::timeout(std::time::Duration::from_secs(5), Node::spawn(&home))
        .await
        .expect("refused promptly, not left waiting");
    let err = second.err().expect("second node must not start");
    assert!(err.to_string().contains("already running"), "{err:#}");
    first.shutdown().await.unwrap();
    let again = Node::spawn(&home).await.expect("home is free once stopped");
    again.shutdown().await.unwrap();

    // A lock that's let go a moment later is waited for, as when a child
    // process that's starting still has a copy of it.
    let held = std::fs::File::open(Home::new(&home).lock()).unwrap();
    rustix::fs::flock(&held, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(held);
    });
    let third = Node::spawn(&home)
        .await
        .expect("a lock let go a moment later is waited for");
    release.join().unwrap();
    third.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn tampered_or_unknown_requests_fail_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), "Erin").await;
    let unknown = ObjectId::of(b"nothing");
    assert!(matches!(
        node.handle(ClientRequest::Show { id: unknown }).await,
        ClientResponse::Error { .. }
    ));
    assert!(matches!(
        node.handle(ClientRequest::Follow {
            ticket: "OSP1NOTATICKET".into()
        })
        .await,
        ClientResponse::Error { .. }
    ));
    assert!(matches!(
        node.handle(ClientRequest::Post {
            content: "  ".into(),
            attachments: vec![],
            uploads: vec![],
            upload: None,
        })
        .await,
        ClientResponse::Error { .. }
    ));
    node.shutdown().await.unwrap();
}
