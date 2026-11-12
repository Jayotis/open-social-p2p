//! Removing a follower: the owner lists their followers' nodes, removes one,
//! and its subscriptions are refused until the owner allows it again. The
//! owner's posts stay public, so the removed node can still read them.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.

use std::path::Path;

use osp_node::{Config, Node};
use osp_proto::{
    EndpointId,
    rpc::{ClientRequest, ClientResponse, FollowerInfo},
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

/// Follows with a ticket, which subscribes at the next sync even if the
/// node subscribed lately, then syncs.
async fn follow(follower: &Node, followed: &Node) {
    let ClientResponse::Ticket { ticket } = call(followed, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    call(follower, ClientRequest::Follow { ticket }).await;
    call(follower, ClientRequest::Sync).await;
}

async fn followers(node: &Node) -> (Vec<FollowerInfo>, Vec<EndpointId>) {
    match call(node, ClientRequest::Followers).await {
        ClientResponse::Followers { followers, removed } => (followers, removed),
        other => panic!("unexpected {other:?}"),
    }
}

async fn feed_texts(node: &Node) -> Vec<String> {
    call(node, ClientRequest::Sync).await;
    match call(node, ClientRequest::Feed { limit: 50 }).await {
        ClientResponse::Feed { items } => items.into_iter().map(|i| i.content).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

async fn post(node: &Node, text: &str) {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
        upload: None,
    };
    call(node, req).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_follower_is_refused_until_allowed_and_can_still_read() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let jimmy = start(dir.path(), "Jimmy").await;
    let alice = start(dir.path(), "Alice").await;

    follow(&jimmy, &kim).await;
    follow(&alice, &kim).await;
    // Kim follows Alice back, so Kim's node knows whose node that is.
    follow(&kim, &alice).await;
    let (list, removed) = followers(&kim).await;
    assert!(removed.is_empty());
    let mut nodes: Vec<_> = list.iter().map(|f| f.node).collect();
    nodes.sort();
    let mut expected = vec![jimmy.node_id(), alice.node_id()];
    expected.sort();
    assert_eq!(nodes, expected);
    let alice_row = list.iter().find(|f| f.node == alice.node_id()).unwrap();
    assert_eq!(alice_row.actor, Some(alice.identity()));
    assert_eq!(alice_row.name.as_deref(), Some("Alice"));
    // A subscription doesn't say whose node it is.
    let jimmy_row = list.iter().find(|f| f.node == jimmy.node_id()).unwrap();
    assert_eq!(jimmy_row.actor, None);

    // Kim removes Jimmy's node.
    let resp = call(
        &kim,
        ClientRequest::RemoveFollower {
            node: jimmy.node_id(),
        },
    )
    .await;
    assert!(matches!(
        resp,
        ClientResponse::FollowerRemoved {
            was_following: true
        }
    ));
    let (list, removed) = followers(&kim).await;
    assert_eq!(list.len(), 1);
    assert_eq!(removed, [jimmy.node_id()]);

    // Jimmy's node subscribes again and is refused; the sync itself still
    // reads Kim's public posts.
    post(&kim, "Still public").await;
    follow(&jimmy, &kim).await;
    let (list, _) = followers(&kim).await;
    assert!(list.iter().all(|f| f.node != jimmy.node_id()));
    assert!(
        feed_texts(&jimmy)
            .await
            .contains(&"Still public".to_string())
    );

    // Allowed again, the next subscription is taken.
    let resp = call(
        &kim,
        ClientRequest::AllowFollower {
            node: jimmy.node_id(),
        },
    )
    .await;
    assert!(matches!(
        resp,
        ClientResponse::FollowerAllowed { was_removed: true }
    ));
    follow(&jimmy, &kim).await;
    let (list, removed) = followers(&kim).await;
    assert!(list.iter().any(|f| f.node == jimmy.node_id()));
    assert!(removed.is_empty());

    // Allowing a node that wasn't removed says so.
    let resp = call(
        &kim,
        ClientRequest::AllowFollower {
            node: alice.node_id(),
        },
    )
    .await;
    assert!(matches!(
        resp,
        ClientResponse::FollowerAllowed { was_removed: false }
    ));
}
