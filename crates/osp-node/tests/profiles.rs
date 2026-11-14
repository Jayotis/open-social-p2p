//! Profiles people want to follow (ADR 0032): a picture and links set on
//! the node, kept over a restart, carried to a follower, and a person's
//! posts paged through on their profile.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.
//! Requires ffmpeg.

use std::path::{Path, PathBuf};

use osp_node::{BlobClass, Config, Node};
use osp_proto::EarlierKeys;
use osp_proto::{
    ObjectId, ProfileLink, PublicKey,
    rpc::{ClientRequest, ClientResponse, IconChange, ProfileInfo, ProfileItemKind, Upload},
    types::MAX_ICON_BYTES,
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

async fn refused(node: &Node, req: ClientRequest) -> String {
    match node.handle(req).await {
        ClientResponse::Error { message } => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn identity(node: &Node) -> PublicKey {
    match call(node, ClientRequest::Status).await {
        ClientResponse::Status(status) => status.identity,
        other => panic!("unexpected {other:?}"),
    }
}

async fn profile(node: &Node, actor: PublicKey) -> ProfileInfo {
    match call(node, ClientRequest::Profile { actor }).await {
        ClientResponse::Profile { profile } => *profile,
        other => panic!("unexpected {other:?}"),
    }
}

async fn follow(follower: &Node, followed: &Node) {
    let ClientResponse::Ticket { ticket } = call(followed, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    call(follower, ClientRequest::Follow { ticket }).await;
}

async fn post(node: &Node, text: &str, attachments: Vec<PathBuf>) -> ObjectId {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments,
        uploads: vec![],
        upload: None,
    };
    match call(node, req).await {
        ClientResponse::Posted { id } => id,
        other => panic!("unexpected {other:?}"),
    }
}

/// A photo of noise, which compresses worst of all, so the picture has to be
/// made smaller to fit.
async fn noisy_photo(dir: &Path) -> PathBuf {
    let photo = dir.join("photo.png");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i"])
        .arg("nullsrc=s=1600x900,geq=random(1)*255:random(1)*255:random(1)*255")
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    photo
}

fn link(name: &str, value: &str) -> ProfileLink {
    ProfileLink {
        name: name.into(),
        value: value.into(),
    }
}

fn set_profile(name: &str, links: Vec<ProfileLink>, icon: IconChange) -> ClientRequest {
    ClientRequest::SetProfile {
        name: name.into(),
        summary: Some("Takes photos of birds".into()),
        links,
        icon,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_picture_and_links_are_kept_over_a_restart_and_reach_a_follower() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let jimmy = start(dir.path(), "Jimmy").await;
    let kim_id = identity(&kim).await;

    let photo = noisy_photo(dir.path()).await;
    let links = vec![link("Site", "https://example.org/kim")];
    let req = set_profile("Kim K", links.clone(), IconChange::File { path: photo });
    let ClientResponse::Profile { profile: set } = call(&kim, req).await else {
        panic!("no profile")
    };
    let icon = set.person.icon.clone().expect("a picture");
    assert_eq!(icon.media_type, "image/webp");
    assert!(icon.size <= MAX_ICON_BYTES, "{} bytes", icon.size);
    assert!(set.icon_local);
    assert_eq!(set.person.name, "Kim K");
    assert_eq!(set.person.attachment, links);
    assert_eq!((set.followers, set.following), (Some(0), Some(0)));
    let ClientResponse::Status(status) = call(&kim, ClientRequest::Status).await else {
        panic!("no status")
    };
    assert_eq!(status.name, "Kim K");

    // The edit is in config.toml, so a restart signs nothing new.
    let signed = kim.profile();
    kim.shutdown().await.unwrap();
    let kim = Node::spawn(&dir.path().join("kim")).await.unwrap();
    assert_eq!(kim.profile().id(), signed.id());

    follow(&jimmy, &kim).await;
    call(&jimmy, ClientRequest::Sync).await;
    let seen = profile(&jimmy, kim_id).await;
    assert!(seen.followed);
    assert!(!seen.known);
    assert_eq!(seen.person.icon, Some(icon.clone()));
    // The picture came with it, fetched ahead as a followed post's media.
    assert!(seen.icon_local);
    assert_eq!(
        jimmy.blob_class(&icon.blob).unwrap(),
        Some(BlobClass::Prefetch)
    );
    assert_eq!(seen.person.attachment, links);
    assert_eq!(
        seen.person.summary.as_deref(),
        Some("Takes photos of birds")
    );
    // Nobody else's followers are known.
    assert_eq!((seen.followers, seen.following), (None, None));

    // Taking the picture off reaches Jimmy at his next sync.
    let req = set_profile("Kim K", vec![], IconChange::Remove);
    call(&kim, req).await;
    call(&jimmy, ClientRequest::Sync).await;
    let seen = profile(&jimmy, kim_id).await;
    assert_eq!(seen.person.icon, None);
    assert!(seen.person.attachment.is_empty());
    let own = profile(&kim, kim_id).await;
    assert_eq!(own.followers, Some(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_edits_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let before = kim.profile();

    let plain_http = vec![link("Site", "http://example.org")];
    let why = refused(&kim, set_profile("Kim", plain_http, IconChange::Keep)).await;
    assert!(why.contains("https"), "{why}");
    let five = vec![link("Site", "https://example.org"); 5];
    refused(&kim, set_profile("Kim", five, IconChange::Keep)).await;
    refused(&kim, set_profile("  ", vec![], IconChange::Keep)).await;
    // A photo's bytes come only from a device, after its request.
    let upload = IconChange::Upload {
        file: Upload {
            name: "me.jpg".into(),
            size: 1000,
        },
    };
    refused(&kim, set_profile("Kim", vec![], upload)).await;
    let text = dir.path().join("not-a-photo.txt");
    std::fs::write(&text, "hello").unwrap();
    let not_photo = IconChange::File { path: text };
    refused(&kim, set_profile("Kim", vec![], not_photo)).await;

    assert_eq!(kim.profile().id(), before.id());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_counts_what_the_node_holds_and_lists_it_a_page_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let jimmy = start(dir.path(), "Jimmy").await;
    let kim_id = identity(&kim).await;

    let mut posts = Vec::new();
    for n in 0..5 {
        posts.push(post(&kim, &format!("post {n}"), vec![]).await);
        // Two in the same millisecond are ordered by ID, not as posted.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let photo = noisy_photo(dir.path()).await;
    let with_photo = post(&kim, "a photo", vec![photo]).await;
    posts.push(with_photo);
    let reshare = match call(
        &kim,
        ClientRequest::Reshare {
            id: posts[0],
            host: false,
        },
    )
    .await
    {
        ClientResponse::Posted { id } => id,
        other => panic!("unexpected {other:?}"),
    };

    follow(&jimmy, &kim).await;
    call(&jimmy, ClientRequest::Sync).await;
    let seen = profile(&jimmy, kim_id).await;
    assert_eq!((seen.posts, seen.media, seen.reshares), (6, 1, 1));

    let page = |kind, before| ClientRequest::ProfileItems {
        actor: kim_id,
        kind,
        before,
        limit: 4,
    };
    let ClientResponse::ProfileItems { items, next } =
        call(&jimmy, page(ProfileItemKind::Posts, None)).await
    else {
        panic!("no page")
    };
    let first: Vec<ObjectId> = items.iter().map(|i| i.id).collect();
    let newest_first: Vec<ObjectId> = posts.iter().rev().copied().collect();
    assert_eq!(first, newest_first[..4]);
    let next = next.expect("a second page");
    let ClientResponse::ProfileItems { items, next } =
        call(&jimmy, page(ProfileItemKind::Posts, Some(next))).await
    else {
        panic!("no page")
    };
    let second: Vec<ObjectId> = items.iter().map(|i| i.id).collect();
    assert_eq!(second, newest_first[4..]);
    assert_eq!(next, None);

    let ClientResponse::ProfileItems { items, .. } =
        call(&jimmy, page(ProfileItemKind::Media, None)).await
    else {
        panic!("no page")
    };
    assert_eq!(items.iter().map(|i| i.id).collect::<Vec<_>>(), [with_photo]);
    let ClientResponse::ProfileItems { items, .. } =
        call(&jimmy, page(ProfileItemKind::Reshares, None)).await
    else {
        panic!("no page")
    };
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].reshared_by.as_ref().map(|r| r.announce),
        Some(reshare)
    );

    // Never more than a page, however many are asked for.
    let ClientResponse::ProfileItems { items, .. } = call(
        &jimmy,
        ClientRequest::ProfileItems {
            actor: kim_id,
            kind: ProfileItemKind::Posts,
            before: None,
            limit: 10_000,
        },
    )
    .await
    else {
        panic!("no page")
    };
    assert!(items.len() <= osp_proto::rpc::MAX_PROFILE_PAGE as usize);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restore_keeps_the_picture_and_links() {
    let dir = tempfile::tempdir().unwrap();
    let (kim, words) = start_with_words(dir.path(), "Kim").await;
    let photo = noisy_photo(dir.path()).await;
    let links = vec![link("Site", "https://example.org/kim")];
    let req = set_profile("Kim K", links.clone(), IconChange::File { path: photo });
    let ClientResponse::Profile { profile: set } = call(&kim, req).await else {
        panic!("no profile")
    };
    let file = dir.path().join("kim.ospbackup");
    let req = ClientRequest::ExportBackup {
        path: file.clone(),
        media: true,
    };
    call(&kim, req).await;
    kim.shutdown().await.unwrap();

    let home = dir.path().join("kim-new");
    let mut config = Config::local_test("");
    config.name = String::new();
    Node::restore(&home, &config, &words, Some(&file), EarlierKeys::Retired).unwrap();
    let kim = Node::spawn(&home).await.unwrap();
    let restored = profile(&kim, set.person.actor).await;
    assert_eq!(restored.person.name, "Kim K");
    assert_eq!(restored.person.attachment, links);
    assert_eq!(restored.person.icon, set.person.icon);
    assert!(restored.icon_local);
}
