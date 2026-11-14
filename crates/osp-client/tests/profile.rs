//! A phone edits its owner's profile and reads anyone's (ADR 0032): the
//! photo goes on the request's stream, the node makes the picture, and the
//! phone reads it back like any other file.
//!
//! The node and the "phone" run in this process on loopback. Requires ffmpeg.

use std::path::{Path, PathBuf};

use osp_client::{Client, ClientOptions, ProfilePicture};
use osp_node::{Config, Node};
use osp_proto::{
    ProfileLink,
    rpc::{ClientRequest, ClientResponse, ProfileItemKind},
};

async fn start_node(root: &Path, name: &str) -> Node {
    let home = root.join(name.to_lowercase());
    Node::init(&home, &Config::local_test(name)).unwrap();
    Node::spawn(&home).await.unwrap()
}

async fn paired_phone(dir: &Path, node: &Node) -> std::sync::Arc<Client> {
    let ClientResponse::Pairing { ticket, .. } = node.handle(ClientRequest::NewPairing).await
    else {
        panic!("no pairing ticket");
    };
    let phone = Client::open(&dir.join("phone"), ClientOptions::local_test())
        .await
        .unwrap();
    phone.pair(&ticket, "Test phone").await.unwrap();
    phone
}

/// A photo as a phone's picker gives it: a JPEG larger than a picture.
async fn make_photo(dir: &Path) -> PathBuf {
    let photo = dir.join("from-the-phone.jpg");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i"])
        .arg("nullsrc=s=1024x768,geq=random(1)*255:128:random(1)*255")
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    photo
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_sets_a_picture_and_links_and_reads_them_back() {
    let dir = tempfile::tempdir().unwrap();
    let node = start_node(dir.path(), "Kim").await;
    let phone = paired_phone(dir.path(), &node).await;
    let photo = make_photo(dir.path()).await;
    let links = vec![ProfileLink {
        name: "Site".into(),
        value: "https://example.org/kim".into(),
    }];

    let set = phone
        .set_profile(
            "Kim K",
            Some("I make tables"),
            links.clone(),
            ProfilePicture::Photo(photo),
        )
        .await
        .unwrap();
    assert_eq!(set.person.name, "Kim K");
    assert_eq!(set.person.summary.as_deref(), Some("I make tables"));
    assert_eq!(set.person.attachment, links);
    let icon = set.person.icon.clone().expect("a picture");
    assert_eq!(icon.media_type, "image/webp");
    assert!(set.icon_local);
    // Only the owner's own profile has these.
    assert_eq!(set.followers, Some(0));
    assert_eq!(set.following, Some(0));

    // The phone reads it as it reads a post's media.
    phone.wait_for_media(&icon.blob).await.unwrap();
    let read = phone.profile(node.identity()).await.unwrap();
    assert_eq!(read.person.icon.as_ref().map(|i| i.blob), Some(icon.blob));

    // Keeping it changes only what's given.
    let kept = phone
        .set_profile("Kim", None, links.clone(), ProfilePicture::Keep)
        .await
        .unwrap();
    assert_eq!(kept.person.icon.map(|i| i.blob), Some(icon.blob));
    assert_eq!(kept.person.summary, None);
    let removed = phone
        .set_profile("Kim", None, vec![], ProfilePicture::Remove)
        .await
        .unwrap();
    assert!(removed.person.icon.is_none());
    assert!(removed.person.attachment.is_empty());

    // What the node refuses, the phone is told why.
    let bad = vec![ProfileLink {
        name: "Site".into(),
        value: "http://example.org".into(),
    }];
    let why = phone
        .set_profile("Kim", None, bad, ProfilePicture::Keep)
        .await
        .unwrap_err();
    assert!(format!("{why:#}").contains("https"), "{why:#}");
    let empty = dir.path().join("empty.jpg");
    std::fs::write(&empty, b"").unwrap();
    let why = phone
        .set_profile("Kim", None, vec![], ProfilePicture::Photo(empty))
        .await
        .unwrap_err();
    assert!(format!("{why:#}").contains("empty"), "{why:#}");

    phone.shutdown().await;
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_pages_through_a_profiles_posts() {
    let dir = tempfile::tempdir().unwrap();
    let node = start_node(dir.path(), "Kim").await;
    for n in 0..3 {
        let req = ClientRequest::Post {
            content: format!("post {n}"),
            attachments: vec![],
            uploads: vec![],
            upload: None,
        };
        assert!(matches!(
            node.handle(req).await,
            ClientResponse::Posted { .. }
        ));
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let phone = paired_phone(dir.path(), &node).await;

    let profile = phone.profile(node.identity()).await.unwrap();
    assert_eq!(profile.posts, 3);
    let (first, next) = phone
        .profile_items(node.identity(), ProfileItemKind::Posts, None, 2)
        .await
        .unwrap();
    let texts: Vec<_> = first.iter().map(|p| p.content.as_str()).collect();
    assert_eq!(texts, ["post 2", "post 1"]);
    let (rest, next) = phone
        .profile_items(node.identity(), ProfileItemKind::Posts, next, 2)
        .await
        .unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].content, "post 0");
    assert!(next.is_none());
    let (media, _) = phone
        .profile_items(node.identity(), ProfileItemKind::Media, None, 50)
        .await
        .unwrap();
    assert!(media.is_empty());

    phone.shutdown().await;
    node.shutdown().await.unwrap();
}
