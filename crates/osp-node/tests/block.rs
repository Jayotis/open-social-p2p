//! Blocking (ADR 0042 §5). Alice blocks Kim: her node drops Kim's posts,
//! reshares of them and Kim's profile, stops following Kim, and keeps all of
//! it out, after a restart too. Kim's node is told nothing.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.

use std::path::Path;

use osp_node::{Config, Node};
use osp_proto::{
    ObjectId,
    rpc::{BlockTarget, ClientRequest, ClientResponse},
};

async fn start(root: &Path, name: &str) -> Node {
    let home = root.join(name.to_lowercase());
    if !home.exists() {
        Node::init(&home, &Config::local_test(name)).unwrap();
    }
    Node::spawn(&home).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn ticket(node: &Node) -> String {
    let ClientResponse::Ticket { ticket } = call(node, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    ticket
}

async fn post(node: &Node, text: &str) -> ObjectId {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, req).await else {
        panic!("not posted")
    };
    id
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
async fn a_blocked_person_stays_out_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let kim = start(dir.path(), "Kim").await;
    let bob = start(dir.path(), "Bob").await;

    // Alice follows Kim and Bob; Bob follows Kim and reshares her post.
    let kims = post(&kim, "from Kim").await;
    call(
        &bob,
        ClientRequest::Follow {
            ticket: ticket(&kim).await,
        },
    )
    .await;
    call(&bob, ClientRequest::Sync).await;
    call(
        &bob,
        ClientRequest::Reshare {
            id: kims,
            host: false,
        },
    )
    .await;
    post(&bob, "from Bob").await;
    call(
        &alice,
        ClientRequest::Follow {
            ticket: ticket(&kim).await,
        },
    )
    .await;
    call(
        &alice,
        ClientRequest::Follow {
            ticket: ticket(&bob).await,
        },
    )
    .await;
    let texts = feed_texts(&alice).await;
    assert!(texts.contains(&"from Kim".to_string()), "{texts:?}");

    let kim_id = kim.identity();
    let target = BlockTarget::Account { actor: kim_id };
    let ClientResponse::Blocked { posts, .. } = call(&alice, ClientRequest::Block { target }).await
    else {
        panic!("not blocked")
    };
    // Kim's post and Bob's reshare of it.
    assert_eq!(posts, 2);
    assert_eq!(feed_texts(&alice).await, ["from Bob"]);
    let ClientResponse::Following { follows } = call(&alice, ClientRequest::Following).await else {
        panic!("no follows")
    };
    assert!(follows.iter().all(|f| f.actor != kim_id));

    // A new post of Kim's, reshared by Bob, doesn't come in either.
    let newer = post(&kim, "Kim again").await;
    call(&bob, ClientRequest::Sync).await;
    call(
        &bob,
        ClientRequest::Reshare {
            id: newer,
            host: false,
        },
    )
    .await;
    assert_eq!(feed_texts(&alice).await, ["from Bob"]);
    // Following Kim again is refused while she's blocked.
    let refused = alice
        .handle(ClientRequest::Follow {
            ticket: ticket(&kim).await,
        })
        .await;
    assert!(
        matches!(refused, ClientResponse::Error { .. }),
        "{refused:?}"
    );
    // Kim's node never heard of it: Alice is still among her followers,
    // and nothing told Kim to drop her.
    let ClientResponse::Followers { followers, .. } = call(&kim, ClientRequest::Followers).await
    else {
        panic!("no followers")
    };
    assert!(followers.iter().any(|f| f.node == alice.node_id()));

    // After a restart, still out.
    alice.shutdown().await.unwrap();
    let alice = start(dir.path(), "Alice").await;
    assert_eq!(feed_texts(&alice).await, ["from Bob"]);
    let ClientResponse::Blocks { blocks } = call(&alice, ClientRequest::Blocks).await else {
        panic!("no blocks")
    };
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].target, target);
    assert_eq!(blocks[0].name.as_deref(), Some("Kim"));

    // Unblocked, Kim can be followed again, and her posts come back.
    call(&alice, ClientRequest::Unblock { target }).await;
    call(
        &alice,
        ClientRequest::Follow {
            ticket: ticket(&kim).await,
        },
    )
    .await;
    let texts = feed_texts(&alice).await;
    assert!(texts.contains(&"Kim again".to_string()), "{texts:?}");

    for node in [alice, kim, bob] {
        node.shutdown().await.unwrap();
    }
}
