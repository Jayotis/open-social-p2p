//! Search on the node itself (ADR 0014): what it has received, found with
//! nothing leaving the node, and rebuilt after a restore or if it's missing.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.

use std::{path::Path, time::Duration};

use osp_node::{Config, Home, Node};
use osp_proto::{
    EarlierKeys, ObjectId,
    rpc::{ClientRequest, ClientResponse, FeedItem, PersonInfo},
};

async fn start(root: &Path, name: &str) -> Node {
    start_with_words(root, name).await.0
}

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

async fn post(node: &Node, text: &str) -> ObjectId {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
        upload: None,
    };
    match call(node, req).await {
        ClientResponse::Posted { id } => id,
        other => panic!("unexpected {other:?}"),
    }
}

async fn follow(follower: &Node, followed: &Node) {
    let ClientResponse::Ticket { ticket } = call(followed, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    call(follower, ClientRequest::Follow { ticket }).await;
}

async fn search(node: &Node, words: &str) -> (Vec<PersonInfo>, Vec<FeedItem>) {
    let req = ClientRequest::Search {
        words: words.into(),
        limit: 50,
    };
    match call(node, req).await {
        ClientResponse::Found { people, posts } => (people, posts),
        other => panic!("unexpected {other:?}"),
    }
}

async fn found_posts(node: &Node, words: &str) -> Vec<String> {
    search(node, words)
        .await
        .1
        .into_iter()
        .map(|item| item.content)
        .collect()
}

async fn found_people(node: &Node, words: &str) -> Vec<String> {
    search(node, words)
        .await
        .0
        .into_iter()
        .map(|person| person.name)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_finds_posts_and_profiles_with_nobody_else_there() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    post(&bob, "A table of green oak").await;
    post(&alice, "My own chair of ash").await;
    follow(&alice, &bob).await;
    call(&alice, ClientRequest::Sync).await;

    // Bob's node is gone, and Alice's is the only one left.
    let bobs_identity = bob.identity();
    bob.shutdown().await.unwrap();

    let (people, posts) = search(&alice, "oak").await;
    assert!(people.is_empty());
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].content, "A table of green oak");
    assert_eq!(posts[0].actor, bobs_identity);
    assert_eq!(posts[0].actor_name.as_deref(), Some("Bob"));

    let (people, posts) = search(&alice, "bob").await;
    assert!(posts.is_empty());
    assert_eq!(people.len(), 1);
    assert_eq!(people[0].name, "Bob");
    assert_eq!(people[0].actor, bobs_identity);
    assert!(people[0].followed);

    assert_eq!(found_posts(&alice, "chair").await, ["My own chair of ash"]);
    let (people, _) = search(&alice, "alice").await;
    assert_eq!(people[0].actor, alice.identity());
    assert!(!people[0].followed);

    // Stopping the follow keeps what was received, and it's still found.
    call(
        &alice,
        ClientRequest::Unfollow {
            actor: bobs_identity,
        },
    )
    .await;
    assert_eq!(found_posts(&alice, "oak").await.len(), 1);
    assert!(!search(&alice, "bob").await.0[0].followed);

    assert!(search(&alice, "nothing-like-this").await.0.is_empty());
    assert!(search(&alice, "").await.1.is_empty());
    assert!(search(&alice, "\" OR (").await.1.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reshared_post_is_found_once_as_itself() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    let carol = start(dir.path(), "Carol").await;

    // Alice follows only Bob, who reshares Carol's post.
    let carols = post(&carol, "A walnut bench").await;
    follow(&bob, &carol).await;
    call(&bob, ClientRequest::Sync).await;
    call(
        &bob,
        ClientRequest::Reshare {
            id: carols,
            host: false,
        },
    )
    .await;
    follow(&alice, &bob).await;
    call(&alice, ClientRequest::Sync).await;

    // The feed shows the reshare; a search shows the post it reshares, once.
    let ClientResponse::Feed { items } = call(&alice, ClientRequest::Feed { limit: 50 }).await
    else {
        panic!("no feed")
    };
    assert!(items.iter().any(|item| item.reshared_by.is_some()));
    let (_, posts) = search(&alice, "walnut").await;
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].id, carols);
    assert_eq!(posts[0].actor, carol.identity());
    assert!(posts[0].reshared_by.is_none());

    // Its author's profile came along, and a stranger's is found like any.
    let (people, _) = search(&alice, "carol").await;
    assert_eq!(people.len(), 1);
    assert!(!people[0].followed);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restored_node_finds_what_its_backup_brought_back() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let (alice, words) = start_with_words(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    let bobs = post(&bob, "A cedar shelf").await;
    follow(&alice, &bob).await;
    call(&alice, ClientRequest::Sync).await;
    post(&alice, "A maple spoon").await;
    call(
        &alice,
        ClientRequest::Reshare {
            id: bobs,
            host: false,
        },
    )
    .await;

    let file = dir.path().join("alice.ospbackup");
    let req = ClientRequest::ExportBackup {
        path: file.clone(),
        media: false,
    };
    call(&alice, req).await;
    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();

    let home = dir.path().join("alice-new");
    let mut config = Config::local_test("");
    config.name = String::new();
    Node::restore(&home, &config, &words, Some(&file), EarlierKeys::Retired).unwrap();
    let alice = Node::spawn(&home).await.unwrap();

    assert_eq!(found_posts(&alice, "maple").await, ["A maple spoon"]);
    assert_eq!(found_posts(&alice, "cedar").await, ["A cedar shelf"]);
    assert_eq!(found_people(&alice, "alice").await, ["Alice"]);
    assert_eq!(found_people(&alice, "bob").await, ["Bob"]);
    alice.shutdown().await.unwrap();

    // The same if the index is lost: it comes back when the node starts.
    let db = rusqlite::Connection::open(Home::new(&home).db()).unwrap();
    db.execute_batch("DROP TABLE search_text; DROP TABLE search_items;")
        .unwrap();
    drop(db);
    let alice = Node::spawn(&home).await.unwrap();
    assert_eq!(found_posts(&alice, "maple").await, ["A maple spoon"]);
    assert_eq!(found_people(&alice, "alice").await, ["Alice"]);
}

/// Waits for what a follower hears in the background.
async fn wait_for_post(node: &Node, words: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while found_posts(node, words).await.is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{words:?} never found"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_search_answers_with_at_most_100_of_each() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    for i in 0..105 {
        post(&alice, &format!("Same words, number {i}")).await;
    }
    let found = |limit| {
        let alice = &alice;
        async move {
            let req = ClientRequest::Search {
                words: "same".into(),
                limit,
            };
            match call(alice, req).await {
                ClientResponse::Found { posts, .. } => posts.len(),
                other => panic!("unexpected {other:?}"),
            }
        }
    };
    assert_eq!(found(3).await, 3);
    assert_eq!(found(u32::MAX).await, 100);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_post_is_found_as_soon_as_it_arrives() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    follow(&alice, &bob).await;
    call(&alice, ClientRequest::Sync).await;
    post(&bob, "A birch stool").await;
    wait_for_post(&alice, "birch").await;
    assert_eq!(found_posts(&alice, "stool").await, ["A birch stool"]);
}
