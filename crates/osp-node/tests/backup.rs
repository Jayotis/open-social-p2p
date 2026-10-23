//! Identity backup and recovery (ADR 0009): an identity restored from its
//! recovery words on a new node, and its followers finding that node.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.

use std::{
    net::SocketAddr,
    path::Path,
    time::{Duration, SystemTime},
};

use iroh::{
    Endpoint, EndpointAddr, RelayMode,
    address_lookup::MemoryLookup,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use osp_node::{BlobClass, Config, Node, backup};
use osp_proto::{
    BlobHash, EarlierKeys, ObjectId, Payload, Person, SOCIAL_ALPN, SecretKey, SignedObject,
    framing::{read_frame, write_frame},
    rpc::{BackupInfo, ClientRequest, ClientResponse, FollowInfo, StatusInfo},
    social::{SocialRequest, SocialResponse},
};

async fn start(root: &Path, name: &str) -> Node {
    start_with_words(root, name).await.0
}

/// Starts a new node, and returns its recovery words too: the only copy
/// of its identity key (ADR 0010).
async fn start_with_words(root: &Path, name: &str) -> (Node, String) {
    let home = root.join(name.to_lowercase());
    let words = Node::init(&home, &Config::local_test(name)).unwrap();
    (Node::spawn(&home).await.unwrap(), words.join(" "))
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn post(node: &Node, text: &str) {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
    };
    call(node, req).await;
}

async fn follow(follower: &Node, followed: &Node) {
    let ClientResponse::Ticket { ticket } = call(followed, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    call(follower, ClientRequest::Follow { ticket }).await;
}

/// Syncs, then returns the texts in the feed, newest first.
async fn feed_texts(node: &Node) -> Vec<String> {
    call(node, ClientRequest::Sync).await;
    texts(node).await
}

/// The texts in the feed, newest first, without syncing first.
async fn texts(node: &Node) -> Vec<String> {
    match call(node, ClientRequest::Feed { limit: 50 }).await {
        ClientResponse::Feed { items } => items.into_iter().map(|i| i.content).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

/// Checks `ready` every 100 ms, for up to 20 s.
async fn wait_until(what: &str, mut ready: impl AsyncFnMut() -> bool) {
    for _ in 0..200 {
        if ready().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("gave up waiting for {what}");
}

/// Posts a small photo made with ffmpeg from an ffmpeg test `source`,
/// such as "testsrc" or "smptebars". Returns the post and the photo.
async fn post_photo(node: &Node, dir: &Path, name: &str, source: &str) -> (ObjectId, BlobHash) {
    let photo = dir.join(format!("{name}.png"));
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", &format!("{source}=size=320x240")])
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let req = ClientRequest::Post {
        content: name.into(),
        attachments: vec![photo],
        uploads: vec![],
    };
    let ClientResponse::Posted { id } = call(node, req).await else {
        panic!("not posted")
    };
    let ClientResponse::Item { item } = call(node, ClientRequest::Show { id }).await else {
        panic!("not shown")
    };
    (id, item.attachments[0].attachment.blob)
}

async fn export(node: &Node, path: &Path, media: bool) -> BackupInfo {
    let req = ClientRequest::ExportBackup {
        path: path.into(),
        media,
    };
    match call(node, req).await {
        ClientResponse::BackedUp { info, .. } => info,
        other => panic!("unexpected {other:?}"),
    }
}

/// The IDs in a node's outbox, in order, as another node sees them.
async fn outbox_ids(node: &Node) -> Vec<ObjectId> {
    let (_peer, conn) = stranger(&node.addr().await).await;
    let req = SocialRequest::GetOutbox {
        since: 0,
        limit: 100,
    };
    match ask(&conn, &req).await {
        SocialResponse::Outbox { entries, .. } => entries.iter().map(|e| e.object.id()).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

/// Waits up to 20 s for the node's own syncing to bring the feed to `want`.
async fn wait_for_feed(node: &Node, want: &[&str]) {
    let mut texts = Vec::new();
    for _ in 0..200 {
        texts = match call(node, ClientRequest::Feed { limit: 50 }).await {
            ClientResponse::Feed { items } => items.into_iter().map(|i| i.content).collect(),
            other => panic!("unexpected {other:?}"),
        };
        if texts == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(texts, want);
}

async fn following(node: &Node) -> Vec<FollowInfo> {
    match call(node, ClientRequest::Following).await {
        ClientResponse::Following { follows } => follows,
        other => panic!("unexpected {other:?}"),
    }
}

async fn status(node: &Node) -> StatusInfo {
    match call(node, ClientRequest::Status).await {
        ClientResponse::Status(status) => status,
        other => panic!("unexpected {other:?}"),
    }
}

async fn followers(node: &Node) -> usize {
    status(node).await.followers
}

/// Writes a key file as the node does: 32 bytes in hex.
fn write_key(path: &Path, key: &SecretKey) {
    let hex: String = key.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(path, hex).unwrap();
}

/// A profile for `key`'s identity listing `nodes`, signed at `published`.
fn profile(key: &SecretKey, nodes: Vec<iroh::EndpointId>, published: &str) -> SignedObject {
    let person = Payload::Person(Person {
        actor: key.public(),
        name: "Alice".into(),
        summary: None,
        nodes,
        published: published.into(),
    });
    SignedObject::sign(key, &person).unwrap()
}

/// The time `by` from now, written as a `published` date.
fn from_now(by: Duration) -> String {
    humantime::format_rfc3339_millis(SystemTime::now() + by).to_string()
}

/// A node run by someone who stole an identity's key. It serves `profile`
/// and an empty outbox, and says yes to anything else.
#[derive(Debug, Clone)]
struct Thief {
    profile: SignedObject,
}

impl ProtocolHandler for Thief {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let Ok(req) = read_frame::<_, SocialRequest>(&mut recv).await else {
                continue;
            };
            let resp = match req {
                SocialRequest::GetProfile => SocialResponse::Profile {
                    profile: self.profile.clone(),
                },
                SocialRequest::GetOutbox { .. } => SocialResponse::Outbox {
                    entries: vec![],
                    referenced: vec![],
                    latest: 0,
                },
                _ => SocialResponse::Ok,
            };
            write_frame(&mut send, &resp).await.ok();
            send.finish().ok();
        }
        Ok(())
    }
}

/// A node nobody has seen before, connected to `node` over `osp/social/1`.
/// Returns its endpoint too, since dropping that closes the connection.
async fn stranger(node: &EndpointAddr) -> (Endpoint, Connection) {
    let lookup = MemoryLookup::new();
    lookup.add_endpoint_info(node.clone());
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .address_lookup(lookup)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let conn = endpoint.connect(node.clone(), SOCIAL_ALPN).await.unwrap();
    (endpoint, conn)
}

async fn ask(conn: &Connection, req: &SocialRequest) -> SocialResponse {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, req).await.unwrap();
    send.finish().unwrap();
    tokio::time::timeout(Duration::from_secs(5), read_frame(&mut recv))
        .await
        .expect("a prompt reply")
        .unwrap()
}

/// Asserts the reply is an error containing `expected`.
fn refused(resp: SocialResponse, expected: &str) {
    match resp {
        SocialResponse::Error { message } => {
            assert!(message.contains(expected), "{message:?}, not {expected:?}")
        }
        other => panic!("expected a refusal ({expected}), got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restored_identity_reaches_followers_who_follow_its_new_node() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let (alice, words) = start_with_words(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;

    post(&alice, "before the move").await;
    follow(&bob, &alice).await;
    assert_eq!(feed_texts(&bob).await, ["before the move"]);

    // Alice's computer is lost. She brings her identity back on a new one.
    let (identity, old_node) = (alice.identity(), alice.node_id());
    alice.shutdown().await.unwrap();
    let home = dir.path().join("alice-new");
    let restored = Node::restore(
        &home,
        &Config::local_test("Alice"),
        &words,
        None,
        EarlierKeys::Retired,
    )
    .unwrap();
    assert_eq!(restored.identity, identity);
    assert!(restored.from_file.is_none());
    let alice = Node::spawn(&home).await.unwrap();
    assert_eq!(alice.identity(), identity);
    assert_ne!(alice.node_id(), old_node);

    // Bob follows her again with a code for the new node. Its outbox starts
    // from 1, so Bob's place in the old one must not hide this post.
    post(&alice, "after the move").await;
    follow(&bob, &alice).await;
    assert_eq!(
        feed_texts(&bob).await,
        ["after the move", "before the move"]
    );

    // A node isn't restored over an existing one.
    let err = Node::restore(
        &home,
        &Config::local_test("Alice"),
        &words,
        None,
        EarlierKeys::Retired,
    )
    .unwrap_err();
    assert!(err.to_string().contains("already holds a node"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn followers_move_to_a_node_that_takes_the_identity_over() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    post(&alice, "before the move").await;
    follow(&bob, &alice).await;
    assert_eq!(feed_texts(&bob).await, ["before the move"]);
    assert_eq!(followers(&alice).await, 1);

    // Alice's node is replaced: her database and keys move to a new home
    // with a new node key, as a restore from a backup file does.
    let old_node = alice.node_id();
    alice.shutdown().await.unwrap();
    let (old, new) = (dir.path().join("alice"), dir.path().join("alice-new"));
    std::fs::create_dir(&new).unwrap();
    for file in [
        "config.toml",
        "signing.key",
        "signing.cert",
        "backup.key",
        "osp.db",
        "osp.db-wal",
        "osp.db-shm",
    ] {
        if old.join(file).exists() {
            std::fs::copy(old.join(file), new.join(file)).unwrap();
        }
    }
    write_key(&new.join("node.key"), &SecretKey::generate());
    // A new name, so we can see Bob take the new profile.
    let config = std::fs::read_to_string(new.join("config.toml")).unwrap();
    assert!(config.contains("\"Alice\""));
    let config = config.replace("\"Alice\"", "\"Alice, new computer\"");
    std::fs::write(new.join("config.toml"), config).unwrap();
    let alice = Node::spawn(&new).await.unwrap();
    assert_ne!(alice.node_id(), old_node);

    // The new node tells Bob as it starts, before posting anything.
    let mut name = None;
    for _ in 0..100 {
        name = following(&bob).await[0].name.clone();
        if name.as_deref() == Some("Alice, new computer") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(name.as_deref(), Some("Alice, new computer"));

    // Bob's follow moved there and started over; his node syncs from it by itself.
    post(&alice, "after the move").await;
    wait_for_feed(&bob, &["after the move", "before the move"]).await;
    let [follow] = &following(&bob).await[..] else {
        panic!("one follow expected")
    };
    assert!(follow.last_error.is_none(), "{:?}", follow.last_error);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_needs_a_newer_profile_that_lists_the_sender() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let (alice, words) = start_with_words(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    follow(&bob, &alice).await;
    feed_texts(&bob).await;
    let still_followed = |follows: Vec<FollowInfo>| {
        assert!(follows[0].last_sync.is_some(), "Bob's follow was disturbed");
    };
    still_followed(following(&bob).await);

    // Someone with Alice's key (in a real move, her new node) and someone without.
    let key = backup::identity_from_words(&words).unwrap();
    let (peer, conn) = stranger(&bob.addr().await).await;
    let now = osp_proto::now_rfc3339();
    let moved = |profile, addr| SocialRequest::Moved { profile, addr };

    // The address must be the sender's own.
    let other = SecretKey::generate().public();
    let req = moved(profile(&key, vec![other], &now), EndpointAddr::new(other));
    refused(ask(&conn, &req).await, "own behalf");
    // The profile must list the sender.
    let req = moved(profile(&key, vec![other], &now), peer.addr());
    refused(ask(&conn, &req).await, "doesn't list your node");
    // It must be newer than the profile Bob has, so an old one can't be replayed.
    let req = moved(
        profile(&key, vec![peer.id()], "2020-01-01T00:00:00Z"),
        peer.addr(),
    );
    refused(ask(&conn, &req).await, "older");
    // It can't be dated ahead of Bob's clock, or no later move could replace it.
    let ahead = from_now(Duration::from_secs(60 * 60));
    let req = moved(profile(&key, vec![peer.id()], &ahead), peer.addr());
    refused(ask(&conn, &req).await, "ahead of our clock");
    // Only identities Bob follows can move him.
    let stranger_key = SecretKey::generate();
    let req = moved(profile(&stranger_key, vec![peer.id()], &now), peer.addr());
    refused(ask(&conn, &req).await, "not following");
    // A profile not signed by the identity it names fails verification.
    let mut forged = serde_json::to_value(profile(&key, vec![peer.id()], &now)).unwrap();
    forged["signer"] = stranger_key.public().to_string().into();
    let forged = serde_json::from_value(forged).unwrap();
    refused(ask(&conn, &moved(forged, peer.addr())).await, "");
    still_followed(following(&bob).await);

    // A newer profile, signed by Alice, listing the sender: Bob's follow moves
    // there and starts over.
    let req = moved(profile(&key, vec![peer.id()], &now), peer.addr());
    assert!(matches!(ask(&conn, &req).await, SocialResponse::Ok));
    let follows = following(&bob).await;
    assert_eq!(follows[0].cursor, 0);
    assert!(follows[0].last_sync.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thief_cant_date_a_profile_so_late_that_the_owner_cant_answer() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let (alice, words) = start_with_words(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    follow(&bob, &alice).await;
    feed_texts(&bob).await;

    // Someone steals Alice's key and moves Bob's follow to their own node,
    // which then serves him her profile dated thousands of years ahead.
    let key = backup::identity_from_words(&words).unwrap();
    let (endpoint, conn) = stranger(&bob.addr().await).await;
    let far_ahead = Payload::Person(Person {
        actor: key.public(),
        name: "Not Alice".into(),
        summary: None,
        nodes: vec![endpoint.id()],
        published: "9999-01-01T00:00:00Z".into(),
    });
    let profile_far_ahead = SignedObject::sign(&key, &far_ahead).unwrap();
    let req = SocialRequest::Moved {
        profile: profile(&key, vec![endpoint.id()], &osp_proto::now_rfc3339()),
        addr: endpoint.addr(),
    };
    let _thief = Router::builder(endpoint)
        .accept(
            SOCIAL_ALPN,
            Thief {
                profile: profile_far_ahead,
            },
        )
        .spawn();
    assert!(matches!(ask(&conn, &req).await, SocialResponse::Ok));

    // Bob syncs from the thief's node, but keeps the profile from the move.
    wait_until("Bob to sync from the thief's node", async || {
        call(&bob, ClientRequest::Sync).await;
        following(&bob).await[0].last_sync.is_some()
    })
    .await;
    assert_eq!(following(&bob).await[0].name.as_deref(), Some("Alice"));

    // So Alice, on a new node, can still answer with a newer move. Her
    // clock being two minutes ahead of Bob's doesn't matter.
    let (new_node, conn) = stranger(&bob.addr().await).await;
    let soon = from_now(Duration::from_secs(2 * 60));
    let req = SocialRequest::Moved {
        profile: profile(&key, vec![new_node.id()], &soon),
        addr: new_node.addr(),
    };
    assert!(matches!(ask(&conn, &req).await, SocialResponse::Ok));
    let follows = following(&bob).await;
    assert!(follows[0].last_sync.is_none(), "the follow didn't move");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_brings_back_posts_follows_followers_and_media() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let (alice, words) = start_with_words(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    let carol = start(dir.path(), "Carol").await;

    // Alice follows Bob and reshares his photo, hosting it. Carol follows Alice.
    let (bobs_post, bobs_photo) = post_photo(&bob, dir.path(), "bob", "smptebars").await;
    follow(&alice, &bob).await;
    call(&alice, ClientRequest::Sync).await;
    post(&alice, "hello").await;
    let (_, alices_photo) = post_photo(&alice, dir.path(), "alice", "testsrc").await;
    let reshare = ClientRequest::Reshare {
        id: bobs_post,
        host: true,
    };
    call(&alice, reshare).await;
    assert_eq!(
        alice.blob_class(&bobs_photo).unwrap(),
        Some(BlobClass::Replica)
    );
    follow(&carol, &alice).await;
    feed_texts(&carol).await;
    assert_eq!(followers(&alice).await, 1);

    let file = dir.path().join("alice.ospbackup");
    let info = export(&alice, &file, true).await;
    assert_eq!((info.posts, info.follows, info.followers), (3, 1, 1));
    assert!(info.media_files >= 1);
    assert_eq!(info.media_missing, 0);
    assert_eq!(info.bytes, std::fs::metadata(&file).unwrap().len());
    let ids = outbox_ids(&alice).await;

    // Alice's computer is lost. She restores from her words and the file,
    // keeping the name in the backup.
    alice.shutdown().await.unwrap();
    let home = dir.path().join("alice-new");
    let mut config = Config::local_test("");
    config.name = String::new();
    let restored =
        Node::restore(&home, &config, &words, Some(&file), EarlierKeys::Retired).unwrap();
    let from_file = restored.from_file.unwrap();
    assert_eq!(
        (from_file.posts, from_file.follows, from_file.followers),
        (3, 1, 1)
    );
    assert_eq!(from_file.media_files, info.media_files);
    let alice = Node::spawn(&home).await.unwrap();

    // Her posts in their order with their IDs, her name, her media, her follow.
    assert_eq!(outbox_ids(&alice).await, ids);
    assert_eq!(status(&alice).await.name, "Alice");
    assert_eq!(
        alice.blob_class(&alices_photo).unwrap(),
        Some(BlobClass::Own)
    );
    assert!(!home.join("restored-media").exists());
    let follows = following(&alice).await;
    assert_eq!(follows.len(), 1);
    assert_eq!(follows[0].actor, bob.identity());

    // The photo she hosts for Bob is fetched again.
    wait_until("the hosted photo", async || {
        alice.blob_class(&bobs_photo).unwrap() == Some(BlobClass::Replica)
    })
    .await;
    // Carol is told Alice moved, and gets her new posts.
    post(&alice, "after the move").await;
    wait_until("Carol to get the new post", async || {
        texts(&carol).await.contains(&"after the move".to_string())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_opens_only_with_its_own_words_and_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (alice, words) = start_with_words(dir.path(), "Alice").await;
    post(&alice, "hello").await;
    let file = dir.path().join("alice.ospbackup");
    let info = export(&alice, &file, false).await;
    assert_eq!((info.posts, info.media_files), (1, 0));
    alice.shutdown().await.unwrap();
    let other_words = Node::init(&dir.path().join("other"), &Config::local_test("Other"))
        .unwrap()
        .join(" ");

    // Each failed restore leaves nothing behind, so it can be run again.
    let config = Config::local_test("Alice");
    let home = dir.path().join("restored");
    let refused = |words: &str, file: &Path, expected: &str| {
        let err =
            Node::restore(&home, &config, words, Some(file), EarlierKeys::Retired).unwrap_err();
        assert!(err.to_string().contains(expected), "{err:#}");
        assert!(!home.exists(), "a failed restore left {}", home.display());
    };
    refused(&other_words, &file, "another identity");
    let bytes = std::fs::read(&file).unwrap();
    let mut changed = bytes.clone();
    let middle = changed.len() / 2;
    changed[middle] ^= 1;
    std::fs::write(dir.path().join("changed"), &changed).unwrap();
    refused(&words, &dir.path().join("changed"), "damaged");
    std::fs::write(dir.path().join("short"), &bytes[..bytes.len() - 10]).unwrap();
    refused(&words, &dir.path().join("short"), "damaged");

    let restored =
        Node::restore(&home, &config, &words, Some(&file), EarlierKeys::Retired).unwrap();
    assert_eq!(restored.from_file.unwrap().posts, 1);
    let alice = Node::spawn(&home).await.unwrap();
    assert_eq!(texts(&alice).await, ["hello"]);
}
