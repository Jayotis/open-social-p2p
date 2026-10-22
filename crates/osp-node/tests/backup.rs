//! Identity backup and recovery (ADR 0009): an identity restored from its
//! recovery words on a new node, and its followers finding that node.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.

use std::path::Path;

use osp_node::{Config, Node, backup};
use osp_proto::rpc::{ClientRequest, ClientResponse};

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
