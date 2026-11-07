//! Deleting a post (ADR 0019): the author's `Delete` reaches followers, and
//! through reshares the followers of resharers, who drop the post, its
//! reshares and its media, and keep a tombstone so it can't come back.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.

use std::{path::Path, time::Duration};

use osp_node::{BlobClass, Config, Node};
use osp_proto::{
    BlobHash, ObjectId,
    rpc::{ClientRequest, ClientResponse},
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

async fn follow(follower: &Node, followed: &Node) {
    let ClientResponse::Ticket { ticket } = call(followed, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    call(follower, ClientRequest::Follow { ticket }).await;
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

/// Posts a photo made with ffmpeg, captioned `text`. Returns the post and
/// the photo.
async fn post_photo(node: &Node, dir: &Path, text: &str) -> (ObjectId, BlobHash) {
    let photo = dir.join(format!("{}.png", node.identity()));
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240",
            "-frames:v",
            "1",
        ])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![photo],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, req).await else {
        panic!("not posted")
    };
    let ClientResponse::Item { item } = call(node, ClientRequest::Show { id }).await else {
        panic!("not shown")
    };
    (id, item.attachments[0].attachment.blob)
}

async fn reshare(node: &Node, id: ObjectId, host: bool) -> ObjectId {
    let ClientResponse::Posted { id } = call(node, ClientRequest::Reshare { id, host }).await
    else {
        panic!("not reshared")
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

fn delete(id: ObjectId) -> ClientRequest {
    ClientRequest::Delete { id }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_post_goes_from_followers_and_through_reshares() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    let carol = start(dir.path(), "Carol").await;

    // Bob follows Alice and reshares her photo, hosting it. Carol follows
    // only Bob, so she has the photo through his reshare.
    let (photo_post, photo) = post_photo(&alice, dir.path(), "the wrong photo").await;
    post(&alice, "the right words").await;
    follow(&bob, &alice).await;
    assert_eq!(feed_texts(&bob).await.len(), 2);
    let bobs = reshare(&bob, photo_post, true).await;
    assert_eq!(bob.blob_class(&photo).unwrap(), Some(BlobClass::Replica));
    follow(&carol, &bob).await;
    assert_eq!(feed_texts(&carol).await, ["the wrong photo"]);
    assert!(carol.blob_class(&photo).unwrap().is_some());

    call(&alice, delete(photo_post)).await;
    assert_eq!(alice.blob_class(&photo).unwrap(), None);
    assert!(alice.knows_deleted(&photo_post).unwrap());
    let ClientResponse::Error { message } = alice.handle(delete(photo_post)).await else {
        panic!("deleted twice")
    };
    assert!(message.contains("isn't one of your posts"), "{message}");

    // Bob drops the post, his reshare and the photo, and passes it on.
    assert_eq!(feed_texts(&bob).await, ["the right words"]);
    assert_eq!(bob.blob_class(&photo).unwrap(), None);
    assert!(bob.knows_deleted(&photo_post).unwrap());
    let ClientResponse::Error { .. } = bob.handle(ClientRequest::Show { id: bobs }).await else {
        panic!("Bob's reshare is still there")
    };
    // Carol, who doesn't follow Alice, hears through Bob's forward.
    assert!(feed_texts(&carol).await.is_empty());
    assert_eq!(carol.blob_class(&photo).unwrap(), None);
    assert!(carol.knows_deleted(&photo_post).unwrap());

    // Someone else's post can't be deleted.
    let ClientResponse::Error { .. } = carol.handle(delete(bobs)).await else {
        panic!("deleted someone else's reshare")
    };

    for node in [alice, bob, carol] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resharer_who_no_longer_follows_the_author_asks_once_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    let carol = start(dir.path(), "Carol").await;

    let words = post(&alice, "words to take back").await;
    follow(&bob, &alice).await;
    feed_texts(&bob).await;
    reshare(&bob, words, false).await;
    call(
        &bob,
        ClientRequest::Unfollow {
            actor: alice.identity(),
        },
    )
    .await;
    follow(&carol, &bob).await;
    assert_eq!(feed_texts(&carol).await, ["words to take back"]);

    call(&alice, delete(words)).await;
    assert_eq!(bob.check_reshares_now().await.unwrap(), 1);
    assert!(bob.knows_deleted(&words).unwrap());
    // Bob's forward reaches Carol.
    assert!(feed_texts(&carol).await.is_empty());
    assert!(carol.knows_deleted(&words).unwrap());
    // Nothing is left to ask about.
    assert_eq!(bob.check_reshares_now().await.unwrap(), 0);

    for node in [alice, bob, carol] {
        node.shutdown().await.unwrap();
    }
}

/// Followers who were offline hear at their next sync, however much was
/// posted since.
#[tokio::test(flavor = "multi_thread")]
async fn a_follower_that_was_away_hears_at_its_next_sync() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    let early = post(&alice, "early").await;
    follow(&bob, &alice).await;
    assert_eq!(feed_texts(&bob).await, ["early"]);

    let home = dir.path().join("bob");
    bob.shutdown().await.unwrap();
    call(&alice, delete(early)).await;
    for i in 0..5 {
        post(&alice, &format!("later {i}")).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let bob = Node::spawn(&home).await.unwrap();
    let texts = feed_texts(&bob).await;
    assert_eq!(texts.len(), 5, "{texts:?}");
    assert!(!texts.contains(&"early".to_string()));

    for node in [alice, bob] {
        node.shutdown().await.unwrap();
    }
}
