//! Stage 1 acceptance test (doc 03): A publishes, B reshares and hosts, A
//! goes offline, and C still gets verified media from B.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.
//! Requires ffmpeg.

use std::path::{Path, PathBuf};

use osp_node::{BlobClass, Config, Node};
use osp_proto::{
    BlobHash, ObjectId,
    rpc::{ClientRequest, ClientResponse, Direction, FeedItem, StatusInfo},
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
    assert_eq!(bob.blob_class(&video).unwrap(), Some(BlobClass::Cache));

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

    // Serving rules: a stranger may download Bob's replica, but not Carol's cache.
    let dave = start(dir.path(), "Dave").await;
    assert!(
        dave.download_blob_from(&video, &[carol.addr().await])
            .await
            .is_err(),
        "Carol must not serve her cache to strangers"
    );
    dave.download_blob_from(&video, &[bob.addr().await])
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
            attachments: vec![]
        })
        .await,
        ClientResponse::Error { .. }
    ));
    node.shutdown().await.unwrap();
}
