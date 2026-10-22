//! Identity backup and recovery (ADR 0009): an identity restored from its
//! recovery words on a new node, and its followers finding that node.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.

use std::{net::SocketAddr, path::Path, time::Duration};

use iroh::{Endpoint, EndpointAddr, RelayMode, address_lookup::MemoryLookup, endpoint::presets};
use osp_node::{Config, Node, backup};
use osp_proto::{
    Payload, Person, SOCIAL_ALPN, SecretKey, SignedObject,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, FollowInfo},
    social::{SocialRequest, SocialResponse},
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
    match call(node, ClientRequest::Feed { limit: 50 }).await {
        ClientResponse::Feed { items } => items.into_iter().map(|i| i.content).collect(),
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

async fn followers(node: &Node) -> usize {
    match call(node, ClientRequest::Status).await {
        ClientResponse::Status(status) => status.followers,
        other => panic!("unexpected {other:?}"),
    }
}

/// A key file as the node writes it: 32 bytes in hex.
fn read_key(path: &Path) -> SecretKey {
    let hex = std::fs::read_to_string(path).unwrap();
    let bytes: Vec<u8> = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    SecretKey::from_bytes(&bytes.try_into().unwrap())
}

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

/// A node nobody has seen before, connected to `node` over `osp/social/0`.
/// Returns its endpoint too, since dropping that closes the connection.
async fn stranger(node: &EndpointAddr) -> (Endpoint, iroh::endpoint::Connection) {
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

async fn ask(conn: &iroh::endpoint::Connection, req: &SocialRequest) -> SocialResponse {
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
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    let words = backup::recovery_words(&dir.path().join("alice"))
        .unwrap()
        .join(" ");

    post(&alice, "before the move").await;
    follow(&bob, &alice).await;
    assert_eq!(feed_texts(&bob).await, ["before the move"]);

    // Alice's computer is lost. She brings her identity back on a new one.
    let (identity, old_node) = (alice.identity(), alice.node_id());
    alice.shutdown().await.unwrap();
    let home = dir.path().join("alice-new");
    let restored = Node::restore(&home, &Config::local_test("Alice"), &words).unwrap();
    assert_eq!(restored, identity);
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
    let err = Node::restore(&home, &Config::local_test("Alice"), &words).unwrap_err();
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

    // Alice's node is replaced: her database and identity key move to a
    // new home with a new node key, as a restore from a backup file does.
    let old_node = alice.node_id();
    alice.shutdown().await.unwrap();
    let (old, new) = (dir.path().join("alice"), dir.path().join("alice-new"));
    std::fs::create_dir(&new).unwrap();
    for file in [
        "config.toml",
        "identity.key",
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
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    follow(&bob, &alice).await;
    feed_texts(&bob).await;
    let still_followed = |follows: Vec<FollowInfo>| {
        assert!(follows[0].last_sync.is_some(), "Bob's follow was disturbed");
    };
    still_followed(following(&bob).await);

    // Someone with Alice's key (in a real move, her new node) and someone without.
    let key = read_key(&dir.path().join("alice").join("identity.key"));
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
