//! The feed: a new post from someone you follow is held back until its
//! large media is on your node, so you never tap a video that isn't there
//! yet; then it's shown on top, above posts that came meanwhile. It's shown
//! anyway once the media won't come, after a time limit, or on a restart.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.
//! Requires ffmpeg.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use osp_node::{BlobClass, Config, Home, Node, SimSettings};
use osp_proto::{
    Attachment, ObjectId,
    rpc::{ClientRequest, ClientResponse},
};

/// Long enough for anything on loopback.
const PROMPTLY: Duration = Duration::from_secs(30);
/// A spread so long that a node in the swarm never gets to the large file
/// during a test, so its post stays held.
const NEVER: Duration = Duration::from_secs(3600);

/// How a test node runs.
#[derive(Clone)]
struct Setup {
    share_prefetched: bool,
    spread: Duration,
    max_hold: Option<Duration>,
    cache_quota_mb: Option<u64>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            share_prefetched: false,
            spread: Duration::ZERO,
            max_hold: None,
            cache_quota_mb: None,
        }
    }
}

impl Setup {
    /// A node in the swarm that never gets to the large files it's spreading.
    fn never_fetching() -> Self {
        Self {
            share_prefetched: true,
            spread: NEVER,
            ..Self::default()
        }
    }

    fn sim(&self) -> SimSettings {
        let mut sim = SimSettings {
            prefetch_spread: self.spread,
            ..SimSettings::default()
        };
        if let Some(max_hold) = self.max_hold {
            sim.max_hold = max_hold;
        }
        sim
    }
}

async fn start(root: &Path, name: &str, setup: &Setup) -> Node {
    let home = root.join(name.to_lowercase());
    let mut config = Config::local_test(name);
    config.swarm.share_prefetched = setup.share_prefetched;
    if let Some(mb) = setup.cache_quota_mb {
        config.storage.cache_quota_mb = mb;
    }
    Node::init(&home, &config).unwrap();
    Node::spawn_sim(&home, setup.sim()).await.unwrap()
}

async fn restart(node: Node, root: &Path, name: &str, setup: &Setup) -> Node {
    node.shutdown().await.unwrap();
    let home = Home::new(root.join(name.to_lowercase()));
    Node::spawn_sim(home.root(), setup.sim()).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn follow(node: &Node, author: &Node) {
    let ClientResponse::Ticket { ticket } = call(author, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(node, ClientRequest::Follow { ticket }).await;
    call(node, ClientRequest::Sync).await;
}

async fn sync(node: &Node) {
    call(node, ClientRequest::Sync).await;
}

/// The node's feed, newest first.
async fn feed(node: &Node) -> Vec<ObjectId> {
    match call(node, ClientRequest::Feed { limit: 50 }).await {
        ClientResponse::Feed { items } => items.into_iter().map(|i| i.id).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

/// Waits until `id` is in the node's feed.
async fn shown(node: &Node, id: ObjectId) -> Vec<ObjectId> {
    let started = Instant::now();
    loop {
        let feed = feed(node).await;
        if feed.contains(&id) {
            return feed;
        }
        assert!(started.elapsed() < PROMPTLY, "never shown");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn post_text(node: &Node, text: &str) -> ObjectId {
    let post = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
    };
    match call(node, post).await {
        ClientResponse::Posted { id } => id,
        other => panic!("unexpected {other:?}"),
    }
}

/// Posts a photo. A `small` one is under 1 MiB; otherwise it's noise, which
/// compresses badly, about 3 MB. Returns the post and its attachment.
async fn post_photo(node: &Node, dir: &Path, small: bool) -> (ObjectId, Attachment) {
    let photo = dir.join("photo.png");
    let source = if small {
        "testsrc=size=640x360"
    } else {
        "nullsrc=s=2048x2048,geq=random(1)*255:random(1)*255:random(1)*255"
    };
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", source])
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let post = ClientRequest::Post {
        content: "a photo".into(),
        attachments: vec![photo],
        uploads: vec![],
    };
    let ClientResponse::Posted { id } = call(node, post).await else {
        panic!("not published");
    };
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    let photo = items[0].attachments[0].attachment.clone();
    assert_eq!(photo.size <= 1 << 20, small, "{} bytes", photo.size);
    (id, photo)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_waits_for_its_large_media_and_a_restart_shows_it() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice", &Setup::default()).await;
    let setup = Setup::never_fetching();
    let bob = start(dir.path(), "Bob", &setup).await;
    // Posts from before Bob followed are shown at once, media or not:
    // held, they'd all land on top as their media came in.
    let (before, _) = post_photo(&alice, dir.path(), false).await;
    follow(&bob, &alice).await;
    assert!(feed(&bob).await.contains(&before));

    // A post with only a small photo is shown at once.
    let (small, _) = post_photo(&alice, dir.path(), true).await;
    sync(&bob).await;
    assert!(feed(&bob).await.contains(&small));

    // One with a large photo waits for it.
    let (large, photo) = post_photo(&alice, dir.path(), false).await;
    sync(&bob).await;
    assert!(!feed(&bob).await.contains(&large));
    assert_eq!(bob.blob_class(&photo.blob).unwrap(), None);
    // Bob's own posts never wait.
    let own = post_text(&bob, "mine").await;
    assert_eq!(feed(&bob).await[0], own);

    // What it waited for was planned by the run before, so a restart shows it.
    let bob = restart(bob, dir.path(), "Bob", &setup).await;
    assert_eq!(feed(&bob).await[0], large);

    for node in [alice, bob] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_post_is_shown_after_a_while_above_posts_that_came_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice", &Setup::default()).await;
    let carol = start(dir.path(), "Carol", &Setup::default()).await;
    let setup = Setup {
        max_hold: Some(Duration::from_secs(5)),
        ..Setup::never_fetching()
    };
    let bob = start(dir.path(), "Bob", &setup).await;
    follow(&bob, &alice).await;
    follow(&bob, &carol).await;

    let (large, photo) = post_photo(&alice, dir.path(), false).await;
    sync(&bob).await;
    let later = post_text(&carol, "posted later").await;
    sync(&bob).await;
    let feed = feed(&bob).await;
    assert_eq!(feed[0], later);
    assert!(!feed.contains(&large));

    // Shown when its time is up, though the photo isn't here, on top.
    let feed = shown(&bob, large).await;
    assert_eq!(feed[..2], [large, later]);
    assert_eq!(bob.blob_class(&photo.blob).unwrap(), None);

    for node in [alice, bob, carol] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_post_is_shown_once_its_media_is_here_or_wont_be() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice", &Setup::default()).await;
    let bob = start(dir.path(), "Bob", &Setup::default()).await;
    // Dave's media cache is too small for the photo, so he won't fetch it.
    let small_cache = Setup {
        cache_quota_mb: Some(1),
        ..Setup::default()
    };
    let dave = start(dir.path(), "Dave", &small_cache).await;
    for node in [&bob, &dave] {
        follow(node, &alice).await;
    }

    // `Sync` waits for the prefetch, which fetches the photo or gives up.
    let (large, photo) = post_photo(&alice, dir.path(), false).await;
    for node in [&bob, &dave] {
        sync(node).await;
        assert_eq!(feed(node).await[0], large);
    }
    assert_eq!(
        bob.blob_class(&photo.blob).unwrap(),
        Some(BlobClass::Prefetch)
    );
    assert_eq!(dave.blob_class(&photo.blob).unwrap(), None);

    for node in [alice, bob, dave] {
        node.shutdown().await.unwrap();
    }
}
