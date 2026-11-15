//! ADR 0036's gate, on the simulated network: each line of a node's
//! summary matches what the node did.
//!
//! Alice posts a photo. Bob follows her, and reshares it with hosting. With
//! Alice gone, Carol follows Bob and has the photo from him. Then Bob's
//! summary says he hosts 1 shared post, served about one photo in the last
//! 30 days, follows 1 person and has 1 follower, and uses at least the
//! photo's space; Alice's says she served it once, to Bob.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use osp_node::{
    Config, Node, SimSettings,
    simnet::{Link, SimNet},
};
use osp_proto::{
    BlobHash, ObjectId,
    rpc::{ClientRequest, ClientResponse, StatusInfo},
};

async fn start(root: &Path, net: &SimNet, name: &str) -> Node {
    let config = Config::local_test(name);
    let home = root.join(name.to_lowercase());
    Node::init(&home, &config).unwrap();
    let settings = SimSettings {
        network: Some(net.link(Link::default())),
        ..SimSettings::default()
    };
    Node::spawn_sim(&home, settings).await.unwrap()
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
    call(follower, ClientRequest::Sync).await;
}

/// Posts a photo made with ffmpeg. Returns the post, the photo and its size.
async fn post_photo(node: &Node, dir: &Path) -> (ObjectId, BlobHash, u64) {
    let photo = dir.join(format!("{}.png", node.identity()));
    let made = tokio::process::Command::new("ffmpeg")
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
    assert!(made.success());
    let req = ClientRequest::Post {
        content: "a photo".into(),
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
    let attachment = &item.attachments[0].attachment;
    (id, attachment.blob, attachment.size)
}

async fn status(node: &Node) -> StatusInfo {
    let ClientResponse::Status(status) = call(node, ClientRequest::Status).await else {
        panic!("no status")
    };
    status
}

/// Waits up to a minute for `check` to hold of `node`'s status, and
/// returns that status.
async fn status_when(node: &Node, what: &str, check: impl Fn(&StatusInfo) -> bool) -> StatusInfo {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let now = status(node).await;
        if check(&now) {
            return now;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {now:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn holds(node: &Node, blob: &BlobHash, who: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while node.blob_class(blob).unwrap().is_none() {
        assert!(Instant::now() < deadline, "{who} lacks the photo");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_nodes_summary_matches_what_it_did() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(36);
    let alice = start(dir.path(), &net, "Alice").await;
    let bob = start(dir.path(), &net, "Bob").await;
    let carol = start(dir.path(), &net, "Carol").await;

    // Nothing done, nothing served.
    for node in [&alice, &bob, &carol] {
        let s = status(node).await;
        assert_eq!(s.served_lately_bytes, 0, "{s:?}");
        assert_eq!(s.shared.posts, 0);
    }

    let (post, photo, size) = post_photo(&alice, dir.path()).await;
    assert!(status(&alice).await.space_bytes >= size);
    follow(&bob, &alice).await;
    call(
        &bob,
        ClientRequest::Reshare {
            id: post,
            host: true,
        },
    )
    .await;
    holds(&bob, &photo, "Bob").await;
    let alices = status_when(&alice, "Alice's serving counted", |s| {
        s.served_lately_bytes >= size
    })
    .await;
    // Bob may have asked for parts of it twice, never much more.
    assert!(alices.served_lately_bytes < 2 * size, "{alices:?}");
    assert_eq!(alices.followers, 1);
    alice.shutdown().await.unwrap();

    // Carol can have the photo only from Bob now.
    follow(&carol, &bob).await;
    let ClientResponse::Item { .. } = call(&carol, ClientRequest::Show { id: post }).await else {
        panic!("Carol can't see the post")
    };
    holds(&carol, &photo, "Carol").await;
    let bobs = status_when(&bob, "Bob's serving counted", |s| {
        s.served_lately_bytes >= size
    })
    .await;
    assert!(bobs.served_lately_bytes < 2 * size, "{bobs:?}");
    assert_eq!(bobs.shared.posts, 1);
    assert!(bobs.shared.served_bytes <= bobs.served_lately_bytes);
    assert_eq!(bobs.follows, 1);
    assert_eq!(bobs.followers, 1);
    assert!(bobs.space_bytes >= size);

    for node in [bob, carol] {
        node.shutdown().await.unwrap();
    }
}
