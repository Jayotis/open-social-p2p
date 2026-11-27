//! The project's hide list (ADR 0042 §6), given by hand as the daily fetch
//! would give it. Alice applies a list holding Kim's post and Kim's photo:
//! both leave her view, with Bob's reshare of the post, and the photo's copy
//! goes. Kim, the author, keeps his own, but won't serve the photo. A list
//! signed by another key is refused, an older one too; turning the list off,
//! or retracting the entries, brings the posts back. Excepting Kim brings
//! his posts and photo back for Alice alone. Until the owner answers whether
//! to use the list, it's off.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.

use std::path::Path;

use osp_node::{Config, Node};
use osp_proto::{
    BlobHash, ObjectId, PublicKey, SecretKey,
    lists::{HideKind, HideList, SignedList, file_entry, post_entry},
    rpc::{ClientRequest, ClientResponse, HideListInfo},
};

async fn start(root: &Path, name: &str, key: Option<&SecretKey>) -> Node {
    let home = root.join(name.to_lowercase());
    let mut config = Config::local_test(name);
    config.lists.hide_key = key.map(|k| k.public());
    config.lists.hide = Some(true);
    Node::init(&home, &config).unwrap();
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

async fn post(node: &Node, text: &str, attachments: Vec<std::path::PathBuf>) -> ObjectId {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments,
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, req).await else {
        panic!("not posted")
    };
    id
}

/// A photo made with ffmpeg; the same each time.
async fn photo(dir: &Path) -> std::path::PathBuf {
    let photo = dir.join("photo.png");
    if photo.exists() {
        return photo;
    }
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
    photo
}

async fn blob_of(node: &Node, id: ObjectId) -> BlobHash {
    let ClientResponse::Item { item } = call(node, ClientRequest::Show { id }).await else {
        panic!("not shown")
    };
    item.attachments[0].attachment.blob
}

/// Syncs, then returns the texts in the feed, sorted.
async fn feed_texts(node: &Node) -> Vec<String> {
    call(node, ClientRequest::Sync).await;
    match call(node, ClientRequest::Feed { limit: 50 }).await {
        ClientResponse::Feed { items } => {
            let mut texts: Vec<String> = items.into_iter().map(|i| i.content).collect();
            texts.sort();
            texts
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn signed(list: &HideList, key: &SecretKey) -> String {
    SignedList::sign(list, key).unwrap().to_json()
}

async fn apply(node: &Node, list: String) -> HideListInfo {
    match call(node, ClientRequest::ApplyHideList { list }).await {
        ClientResponse::HideList { info } => info,
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listed_post_and_file_leave_view_and_come_back_when_retracted() {
    let dir = tempfile::tempdir().unwrap();
    let project = SecretKey::from_bytes(&[7; 32]);
    let alice = start(dir.path(), "Alice", Some(&project)).await;
    let kim = start(dir.path(), "Kim", Some(&project)).await;
    let bob = start(dir.path(), "Bob", None).await;
    let carol = start(dir.path(), "Carol", None).await;

    // Kim posts three times; Bob follows him and reshares the words.
    post(&kim, "fine", vec![]).await;
    let words = post(&kim, "listed words", vec![]).await;
    let shot = post(&kim, "listed photo", vec![photo(dir.path()).await]).await;
    let file = blob_of(&kim, shot).await;
    follow(&bob, &kim).await;
    call(&bob, ClientRequest::Sync).await;
    call(
        &bob,
        ClientRequest::Reshare {
            id: words,
            host: false,
        },
    )
    .await;
    follow(&alice, &kim).await;
    follow(&alice, &bob).await;
    // Bob's reshare shows as Kim's post, once.
    let everything = feed_texts(&alice).await;
    assert_eq!(everything.len(), 3, "{everything:?}");
    assert!(alice.blob_class(&file).unwrap().is_some());

    // The project lists the words and the photo.
    let mut list = HideList::new(project.public());
    list.add(HideKind::Post, post_entry(&words), "2026-11-17T00:00:00Z");
    list.add(HideKind::File, file_entry(&file), "2026-11-17T00:00:00Z");
    list.seq = 1;
    let first = signed(&list, &project);
    let info = apply(&alice, first.clone()).await;
    assert_eq!((info.seq, info.entries), (Some(1), Some(2)));

    // Gone from Alice's view, Bob's reshare too, and the photo's copy.
    assert_eq!(feed_texts(&alice).await, ["fine"]);
    assert_eq!(alice.blob_class(&file).unwrap(), None);
    let found = alice.handle(ClientRequest::Show { id: words }).await;
    assert!(matches!(found, ClientResponse::Error { .. }), "{found:?}");

    // Kim keeps his own posts and photo, but won't serve the photo: Carol,
    // who has no list, sees his posts and can't get it.
    apply(&kim, first.clone()).await;
    assert_eq!(feed_texts(&kim).await.len(), 3);
    assert!(kim.blob_class(&file).unwrap().is_some());
    follow(&carol, &kim).await;
    assert_eq!(feed_texts(&carol).await.len(), 3);
    assert_eq!(carol.blob_class(&file).unwrap(), None);

    // The same photo in a new post stays out too.
    post(&kim, "the photo again", vec![photo(dir.path()).await]).await;
    assert_eq!(feed_texts(&alice).await, ["fine"]);

    // A list signed by another key is refused, and changes nothing.
    let other = SecretKey::from_bytes(&[8; 32]);
    let mut forged = HideList::new(other.public());
    forged.seq = 9;
    let refused = alice
        .handle(ClientRequest::ApplyHideList {
            list: signed(&forged, &other),
        })
        .await;
    assert!(
        matches!(refused, ClientResponse::Error { .. }),
        "{refused:?}"
    );
    assert_eq!(feed_texts(&alice).await, ["fine"]);

    // Turned off, everything is back; on again, it's hidden again.
    call(&alice, ClientRequest::SetHideList { on: false }).await;
    assert_eq!(feed_texts(&alice).await, everything_and_again(&everything));
    let ClientResponse::HideList { info } = call(&alice, ClientRequest::HideList).await else {
        panic!("no info")
    };
    assert!(!info.on && info.seq.is_none());
    call(&alice, ClientRequest::SetHideList { on: true }).await;
    apply(&alice, first.clone()).await;
    assert_eq!(feed_texts(&alice).await, ["fine"]);

    // The project retracts both entries; the posts come back, and the older
    // list can't be applied again after.
    list.retract(HideKind::Post, post_entry(&words), "2026-11-18T00:00:00Z");
    list.retract(HideKind::File, file_entry(&file), "2026-11-18T00:00:00Z");
    list.seq = 2;
    let info = apply(&alice, signed(&list, &project)).await;
    assert_eq!((info.seq, info.entries), (Some(2), Some(0)));
    assert_eq!(feed_texts(&alice).await, everything_and_again(&everything));
    // Its photo opens again.
    alice.open_blob(&file).await.unwrap();
    let older = alice
        .handle(ClientRequest::ApplyHideList { list: first })
        .await;
    assert!(matches!(older, ClientResponse::Error { .. }), "{older:?}");

    for node in [alice, kim, bob, carol] {
        node.shutdown().await.unwrap();
    }
}

fn everything_and_again(everything: &[String]) -> Vec<String> {
    let mut texts = everything.to_vec();
    texts.push("the photo again".into());
    texts.sort();
    texts
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_key_set_the_projects_key_is_used() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice", None).await;
    let project = SecretKey::from_bytes(&[7; 32]);
    let list = HideList::new(project.public());
    let refused = alice
        .handle(ClientRequest::ApplyHideList {
            list: signed(&list, &project),
        })
        .await;
    assert!(
        matches!(refused, ClientResponse::Error { .. }),
        "{refused:?}"
    );
    let ClientResponse::HideList { info } = call(&alice, ClientRequest::HideList).await else {
        panic!("no info")
    };
    // The key built in, not the one that signed this list.
    assert!(info.on && info.seq.is_none());
    assert!(info.key.is_some() && info.key != Some(project.public()));
    alice.shutdown().await.unwrap();
}

async fn info(node: &Node) -> HideListInfo {
    match call(node, ClientRequest::HideList).await {
        ClientResponse::HideList { info } => info,
        other => panic!("unexpected {other:?}"),
    }
}

async fn except(node: &Node, actor: PublicKey, except: bool) -> HideListInfo {
    match call(node, ClientRequest::HideException { actor, except }).await {
        ClientResponse::HideList { info } => info,
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_list_is_on_unless_the_owner_turns_it_off() {
    let dir = tempfile::tempdir().unwrap();
    let project = SecretKey::from_bytes(&[7; 32]);
    let home = dir.path().join("alice");
    let mut config = Config::local_test("Alice");
    config.lists.hide_key = Some(project.public());
    Node::init(&home, &config).unwrap();
    let alice = Node::spawn(&home).await.unwrap();

    // Unset: on, and a list is applied, its signed text kept for phones.
    let mut list = HideList::new(project.public());
    list.seq = 1;
    let first = apply(&alice, signed(&list, &project)).await;
    assert!(first.on, "{first:?}");
    assert_eq!(
        first.signed.as_deref(),
        Some(signed(&list, &project).as_str())
    );

    // Off: a list isn't applied, and it stays off through a restart.
    call(&alice, ClientRequest::SetHideList { on: false }).await;
    alice.shutdown().await.unwrap();
    let alice = Node::spawn(&home).await.unwrap();
    assert!(!info(&alice).await.on);
    let refused = alice
        .handle(ClientRequest::ApplyHideList {
            list: signed(&list, &project),
        })
        .await;
    assert!(
        matches!(refused, ClientResponse::Error { .. }),
        "{refused:?}"
    );

    // On again.
    call(&alice, ClientRequest::SetHideList { on: true }).await;
    let again = apply(&alice, signed(&list, &project)).await;
    assert!(again.on, "{again:?}");
    alice.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_excepted_persons_posts_and_files_come_back_for_the_owner_alone() {
    let dir = tempfile::tempdir().unwrap();
    let project = SecretKey::from_bytes(&[7; 32]);
    let alice = start(dir.path(), "Alice", Some(&project)).await;
    let kim = start(dir.path(), "Kim", Some(&project)).await;
    let bob = start(dir.path(), "Bob", Some(&project)).await;

    post(&kim, "fine", vec![]).await;
    let words = post(&kim, "listed words", vec![]).await;
    let shot = post(&kim, "listed photo", vec![photo(dir.path()).await]).await;
    let file = blob_of(&kim, shot).await;
    follow(&bob, &kim).await;
    call(&bob, ClientRequest::Sync).await;
    call(
        &bob,
        ClientRequest::Reshare {
            id: words,
            host: false,
        },
    )
    .await;
    follow(&alice, &kim).await;
    follow(&alice, &bob).await;
    let everything = feed_texts(&alice).await;
    assert_eq!(everything.len(), 3, "{everything:?}");

    let mut list = HideList::new(project.public());
    list.add(HideKind::Post, post_entry(&words), "2026-11-17T00:00:00Z");
    list.add(HideKind::File, file_entry(&file), "2026-11-17T00:00:00Z");
    list.seq = 1;
    apply(&alice, signed(&list, &project)).await;
    assert_eq!(feed_texts(&alice).await, ["fine"]);
    assert_eq!(alice.blob_class(&file).unwrap(), None);
    assert!(alice.open_blob(&file).await.is_err());

    // Alice excepts Kim: his posts are back, Bob's reshare of the words
    // too, and the photo can be fetched for her again.
    let kims = kim.identity();
    let info = except(&alice, kims, true).await;
    assert_eq!(info.exceptions.len(), 1);
    assert_eq!(info.exceptions[0].actor, kims);
    assert_eq!(info.exceptions[0].name.as_deref(), Some("Kim"));
    assert_eq!(feed_texts(&alice).await, everything);
    alice.open_blob(&file).await.unwrap();
    assert!(alice.blob_class(&file).unwrap().is_some());

    // Taking it back hides them again, and the photo's copy goes.
    let info = except(&alice, kims, false).await;
    assert!(info.exceptions.is_empty());
    assert_eq!(feed_texts(&alice).await, ["fine"]);
    assert_eq!(alice.blob_class(&file).unwrap(), None);

    // The owner's own posts are never hidden, so there's nothing to except.
    let me = alice
        .handle(ClientRequest::HideException {
            actor: alice.identity(),
            except: true,
        })
        .await;
    assert!(matches!(me, ClientResponse::Error { .. }), "{me:?}");

    for node in [alice, kim, bob] {
        node.shutdown().await.unwrap();
    }
}
