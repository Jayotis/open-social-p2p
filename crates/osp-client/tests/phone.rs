//! M2 acceptance test for the device protocol (ADR 0008): a phone pairs with
//! its home node, posts a video, reads its feed, plays media through the
//! local media server, and is locked out once unpaired.
//!
//! The node and the "phone" run in this process on loopback, with relays and
//! DNS lookup off. Requires ffmpeg.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use osp_client::{Client, ClientOptions, Pairing};
use osp_node::{BlobClass, Config, Node, SimSettings, simnet::TorDelay};
use osp_proto::{
    Attachment, BlobHash,
    rpc::{ClientRequest, ClientResponse, StatusInfo},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn start_node(root: &Path, name: &str) -> Node {
    let home = root.join(name.to_lowercase());
    Node::init(&home, &Config::local_test(name)).unwrap();
    Node::spawn(&home).await.unwrap()
}

async fn new_pairing(node: &Node) -> String {
    match node.handle(ClientRequest::NewPairing).await {
        ClientResponse::Pairing { ticket, .. } => ticket,
        other => panic!("unexpected {other:?}"),
    }
}

async fn status(client: &Client) -> StatusInfo {
    match client.request(&ClientRequest::Status).await.unwrap() {
        ClientResponse::Status(status) => status,
        other => panic!("unexpected {other:?}"),
    }
}

async fn make_clip(dir: &Path) -> PathBuf {
    let out = dir.join("phone-clip.mp4");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=720x1280:rate=30",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&out)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    out
}

/// Three seconds of noise, which compresses badly, so the posted video is
/// as large as the media profile allows: about 700 KB.
async fn make_noise_clip(dir: &Path) -> PathBuf {
    let out = dir.join("noise.mp4");
    let source = "color=c=gray:s=640x360:d=3,noise=alls=60:allf=t";
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", source])
        .args(["-c:v", "libx264", "-preset", "ultrafast"])
        .arg(&out)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    out
}

/// Posts a video from a file on the node, and returns its attachment.
async fn post_video(node: &Node, clip: PathBuf) -> Attachment {
    let post = ClientRequest::Post {
        content: "a video".into(),
        attachments: vec![clip],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { .. } = node.handle(post).await else {
        panic!("not posted");
    };
    let ClientResponse::Feed { items } = node.handle(ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    items[0].attachments[0].attachment.clone()
}

async fn follow(node: &Node, author: &Node) {
    let ClientResponse::Ticket { ticket } = author.handle(ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    let followed = node.handle(ClientRequest::Follow { ticket }).await;
    assert!(matches!(followed, ClientResponse::Ok), "{followed:?}");
    node.handle(ClientRequest::Sync).await;
}

/// A bare HTTP GET, as a video player would send it. Returns the status
/// line, the headers and the body.
async fn http_get(url: &str, range: Option<&str>) -> (String, String, Vec<u8>) {
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_once('/').unwrap();
    let mut stream = tokio::net::TcpStream::connect(host).await.unwrap();
    let range = range.map(|r| format!("Range: {r}\r\n")).unwrap_or_default();
    let req = format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\n{range}\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).await.unwrap();
    let split = resp
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response head");
    let head = String::from_utf8(resp[..split].to_vec()).unwrap();
    let (status, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
    (
        status.to_string(),
        headers.to_lowercase(),
        resp[split + 4..].to_vec(),
    )
}

/// With the node's other endpoints on (ADR 0014), a phone pairs with the
/// direct one, and goes on using it after the app restarts.
#[tokio::test(flavor = "multi_thread")]
async fn a_phone_pairs_with_the_direct_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::local_test("Alice");
    config.endpoints.enabled = true;
    let home = dir.path().join("alice");
    Node::init(&home, &config).unwrap();
    let node = Node::spawn(&home).await.unwrap();

    let phone_dir = dir.path().join("phone");
    let phone = Client::open(&phone_dir, ClientOptions::local_test())
        .await
        .unwrap();
    let pairing = phone
        .pair(&new_pairing(&node).await, "Test phone")
        .await
        .unwrap();
    assert_eq!(pairing.node.id, node.direct_addr().await.unwrap().id);
    assert_ne!(pairing.node.id, node.node_id());
    let seen = status(&phone).await;
    assert_eq!(seen.devices, 1);
    assert_eq!(seen.identity, node.identity());
    phone.shutdown().await;
    drop(phone);

    let phone = Client::open(&phone_dir, ClientOptions::local_test())
        .await
        .unwrap();
    assert_eq!(status(&phone).await.identity, node.identity());
    phone.shutdown().await;
    node.shutdown().await.unwrap();
}

/// Over Tor a sync takes up to minutes, so a phone asking for one isn't kept
/// waiting for it (ADR 0014); what arrives is in the feed when it looks again.
#[tokio::test(flavor = "multi_thread")]
async fn over_tor_a_phone_isnt_kept_waiting_for_a_sync() {
    let dir = tempfile::tempdir().unwrap();
    let author = start_node(dir.path(), "Author").await;
    let home = dir.path().join("alice");
    Node::init(&home, &Config::local_test("Alice")).unwrap();
    let sim = SimSettings {
        tor_delay: Some(TorDelay {
            first_connect: (Duration::from_secs(300), Duration::from_secs(300)),
            ..TorDelay::default()
        }),
        ..SimSettings::default()
    };
    let node = Node::spawn_sim(&home, sim).await.unwrap();
    let ClientResponse::Ticket { ticket } = author.handle(ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    let followed = node.handle(ClientRequest::Follow { ticket }).await;
    assert!(matches!(followed, ClientResponse::Ok), "{followed:?}");

    let phone = Client::open(&dir.path().join("phone"), ClientOptions::local_test())
        .await
        .unwrap();
    phone
        .pair(&new_pairing(&node).await, "Test phone")
        .await
        .unwrap();
    let asked = phone.request(&ClientRequest::Sync);
    let replied = tokio::time::timeout(Duration::from_secs(20), asked)
        .await
        .expect("the phone was kept waiting")
        .unwrap();
    assert!(matches!(replied, ClientResponse::Synced { new_objects: 0 }));
    phone.shutdown().await;
    node.shutdown().await.unwrap();
    author.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn phone_pairs_posts_plays_and_is_revoked() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,osp_client=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let node = start_node(dir.path(), "Alice").await;

    // An unpaired phone can't do anything.
    let phone = Client::open(&dir.path().join("phone"), ClientOptions::local_test())
        .await
        .unwrap();
    assert!(phone.pairing().is_none());
    assert!(phone.request(&ClientRequest::Status).await.is_err());

    // Pairing with a ticket from the node.
    let ticket = new_pairing(&node).await;
    let pairing = phone.pair(&ticket, "Test phone").await.unwrap();
    assert_eq!(pairing.name, "Alice");
    assert_eq!(pairing.identity, node.identity());

    // The ticket works only once.
    let thief = Client::open(&dir.path().join("thief"), ClientOptions::local_test())
        .await
        .unwrap();
    assert!(thief.pair(&ticket, "Thief").await.is_err());

    // A device that knows the node's address but never paired is refused.
    let stranger_dir = dir.path().join("stranger");
    std::fs::create_dir_all(&stranger_dir).unwrap();
    let fake = Pairing {
        node: node.addr().await,
        identity: node.identity(),
        name: "Alice".into(),
        paired: osp_proto::now_rfc3339(),
    };
    std::fs::write(
        stranger_dir.join("pairing.json"),
        serde_json::to_vec(&fake).unwrap(),
    )
    .unwrap();
    let stranger = Client::open(&stranger_dir, ClientOptions::local_test())
        .await
        .unwrap();
    let refused = stranger.request(&ClientRequest::Status).await.unwrap_err();
    assert!(refused.to_string().contains("isn't paired"), "{refused:#}");

    // The node lists the phone, and status reports health.
    let ClientResponse::Devices { devices } = node.handle(ClientRequest::Devices).await else {
        panic!("devices failed")
    };
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id, phone.device_id());
    assert_eq!(devices[0].name.as_deref(), Some("Test phone"));
    assert!(devices[0].connected);
    let s = status(&phone).await;
    assert_eq!(s.devices, 1);
    assert!(!s.version.is_empty());
    assert!(
        s.peers
            .iter()
            .any(|p| p.node == phone.device_id() && p.device),
        "the phone shows up as a device peer"
    );
    let info = phone.connection_info().await.expect("connected");
    assert_eq!(info.path, "direct");

    // Device management and backups stay on the node itself.
    let backup = ClientRequest::ExportBackup {
        path: dir.path().join("backup"),
        media: false,
    };
    for req in [ClientRequest::NewPairing, ClientRequest::Devices, backup] {
        assert!(phone.request(&req).await.is_err());
    }
    assert!(!dir.path().join("backup").exists());
    let paths = ClientRequest::Post {
        content: "sneaky".into(),
        attachments: vec!["/etc/hostname".into()],
        uploads: vec![],
        upload: None,
    };
    assert!(phone.request(&paths).await.is_err());

    // The phone posts a video; the node transcodes and publishes it.
    let clip = make_clip(dir.path()).await;
    let clip_size = std::fs::metadata(&clip).unwrap().len();
    let sent = Arc::new(AtomicU64::new(0));
    let progress = sent.clone();
    let post_id = phone
        .post("from my phone".into(), &[clip], move |done, total| {
            assert_eq!(total, clip_size);
            progress.store(done, Ordering::Relaxed);
        })
        .await
        .unwrap();
    assert_eq!(sent.load(Ordering::Relaxed), clip_size);

    // A file that isn't media is refused, with the node's reason.
    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "not a video").unwrap();
    let err = phone
        .post("bad".into(), &[notes], |_, _| {})
        .await
        .unwrap_err();
    assert!(err.to_string().contains("media"), "{err:#}");

    // The feed shows the post, and a copy is kept for offline use.
    let feed = phone.feed(20).await.unwrap();
    let item = feed.iter().find(|i| i.id == post_id).expect("post in feed");
    assert_eq!(item.content, "from my phone");
    let video = &item.attachments[0].attachment;
    assert_eq!((video.width, video.height), (720, 1280));
    assert!(item.attachments[0].local);
    let cached = phone.cached_feed().expect("feed saved");
    assert!(cached.items.iter().any(|i| i.id == post_id));

    // Playback: the whole video, then a range, then refusals.
    let url = phone
        .media_url(&video.blob, &video.media_type)
        .await
        .unwrap();
    let (status_line, headers, body) = http_get(&url, Some("bytes=0-")).await;
    assert_eq!(status_line, "HTTP/1.1 206 Partial Content");
    assert!(headers.contains("content-type: video/mp4"));
    assert!(headers.contains(&format!(
        "content-range: bytes 0-{}/{}",
        video.size - 1,
        video.size
    )));
    assert_eq!(body.len() as u64, video.size);
    assert_eq!(blake3::hash(&body).as_bytes(), video.blob.as_bytes());

    let (status_line, _, part) = http_get(&url, Some("bytes=100-199")).await;
    assert_eq!(status_line, "HTTP/1.1 206 Partial Content");
    assert_eq!(part, body[100..200]);
    let (status_line, _, whole) = http_get(&url, None).await;
    assert_eq!(status_line, "HTTP/1.1 200 OK");
    assert_eq!(whole.len(), body.len());

    let (base, file) = url.rsplit_once('/').unwrap();
    let (origin, _token) = base.rsplit_once('/').unwrap();
    let wrong_token = format!("{origin}/0123456789abcdef0123456789abcdef/{file}");
    assert!(http_get(&wrong_token, None).await.0.contains("403"));
    let missing = BlobHash::from_bytes([1; 32]);
    let missing_url = phone.media_url(&missing, "video/mp4").await.unwrap();
    assert!(http_get(&missing_url, None).await.0.contains("502"));

    // Unpairing closes the connection and locks the phone out.
    let resp = node
        .handle(ClientRequest::Unpair {
            device: phone.device_id(),
        })
        .await;
    assert!(matches!(resp, ClientResponse::Ok), "{resp:?}");
    // A request already on its way sees the connection close; the next one
    // reconnects and is refused.
    let err = phone.request(&ClientRequest::Status).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("unpaired") || err.to_string().contains("isn't paired"),
        "{err:#}"
    );
    let err = phone.request(&ClientRequest::Status).await.unwrap_err();
    assert!(err.to_string().contains("isn't paired"), "{err:#}");

    // Forgetting clears the pairing and the cached feed on the phone.
    phone.forget().await.unwrap();
    assert!(phone.pairing().is_none());
    assert!(phone.cached_feed().is_none());

    for client in [phone, thief, stranger] {
        client.shutdown().await;
    }
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_video_the_node_is_still_fetching_plays_as_it_arrives() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,osp_client=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let (alice, video, bob, phone) = video_not_on_the_phones_node(dir.path()).await;
    assert!(video.size > 400_000, "{} bytes", video.size);

    // The phone waits only until the node has the start of the video.
    phone.wait_for_media(&video.blob).await.unwrap();
    assert_eq!(bob.blob_class(&video.blob).unwrap(), None, "all of it came");

    // Players read it at once, several requests at a time, as it arrives.
    let url = phone
        .media_url(&video.blob, &video.media_type)
        .await
        .unwrap();
    let (whole, part) = tokio::join!(
        http_get(&url, Some("bytes=0-")),
        http_get(&url, Some("bytes=300000-300999")),
    );
    let (status_line, headers, body) = whole;
    assert_eq!(status_line, "HTTP/1.1 206 Partial Content");
    assert!(headers.contains(&format!(
        "content-range: bytes 0-{}/{}",
        video.size - 1,
        video.size
    )));
    assert_eq!(blake3::hash(&body).as_bytes(), video.blob.as_bytes());
    assert_eq!(part.2, body[300_000..301_000]);
    // The last bytes may reach the phone just before the node records them.
    let started = Instant::now();
    while bob.blob_class(&video.blob).unwrap() != Some(BlobClass::Cache) {
        assert!(started.elapsed() < Duration::from_secs(10), "not kept");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // All of them shared one download: the video came once. Downloads of
    // their own would have run on a while, then come to about twice that.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let ClientResponse::Peers { peers } = bob.handle(ClientRequest::Peers).await else {
        panic!("no peers");
    };
    let from_alice = peers.iter().find(|p| p.node == alice.node_id()).unwrap();
    // Counts fade a little as time passes.
    let received = from_alice.counts.received_bytes.round() as u64;
    assert!(
        received > video.size * 9 / 10 && received < video.size * 3 / 2,
        "{received} of {} bytes",
        video.size
    );

    phone.shutdown().await;
    for node in [alice, bob] {
        node.shutdown().await.unwrap();
    }
}

/// The phone is told soon: the node asks Alice's node who has a copy (3 s),
/// tries to reach it (10 s) and asks again (3 s), but doesn't try it twice.
#[tokio::test(flavor = "multi_thread")]
async fn a_video_nobody_can_send_fails_soon_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let (alice, video, bob, phone) = video_not_on_the_phones_node(dir.path()).await;
    alice.shutdown().await.unwrap();
    let started = Instant::now();
    let err = phone.wait_for_media(&video.blob).await.unwrap_err();
    assert!(
        err.to_string().contains("no provider could send it"),
        "{err:#}"
    );
    // 16 s. Trying Alice's node again would add 10 s, and waiting for its
    // answer as long as a sync does, 17 s more (it took 43 s before).
    assert!(
        started.elapsed() < Duration::from_secs(21),
        "{:?}",
        started.elapsed()
    );
    phone.shutdown().await;
    bob.shutdown().await.unwrap();
}

/// Alice posts a video, and her node sends slowly, so it takes seconds to
/// reach Bob's. Bob's node follows hers but doesn't fetch ahead of time.
/// Returns Alice's node, the video, Bob's node and his paired phone.
async fn video_not_on_the_phones_node(dir: &Path) -> (Node, Attachment, Node, Arc<Client>) {
    let home = dir.join("alice");
    let mut config = Config::local_test("Alice");
    config.serving.max_upload_mbps = 1;
    Node::init(&home, &config).unwrap();
    let alice = Node::spawn(&home).await.unwrap();
    let video = post_video(&alice, make_noise_clip(dir).await).await;

    let home = dir.join("bob");
    let mut config = Config::local_test("Bob");
    config.sync.prefetch = false;
    Node::init(&home, &config).unwrap();
    let bob = Node::spawn(&home).await.unwrap();
    follow(&bob, &alice).await;
    let phone = Client::open(&dir.join("phone"), ClientOptions::local_test())
        .await
        .unwrap();
    phone
        .pair(&new_pairing(&bob).await, "Bob's phone")
        .await
        .unwrap();
    (alice, video, bob, phone)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_from_secure_storage_is_used_and_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let node = start_node(dir.path(), "Alice").await;
    let phone_dir = dir.path().join("phone");

    // The app keeps the key outside the client's directory.
    let stored = osp_client::new_device_key();
    let key = osp_client::device_key_from_hex(&stored).unwrap();
    let phone = Client::open_with_key(&phone_dir, key.clone(), ClientOptions::local_test())
        .await
        .unwrap();
    assert_eq!(phone.device_id(), key.public());
    phone
        .pair(&new_pairing(&node).await, "Test phone")
        .await
        .unwrap();
    phone.shutdown().await;
    assert!(!phone_dir.join("device.key").exists());

    // Opened again with the same key, it is still the paired phone.
    let key = osp_client::device_key_from_hex(&stored).unwrap();
    let phone = Client::open_with_key(&phone_dir, key, ClientOptions::local_test())
        .await
        .unwrap();
    assert_eq!(status(&phone).await.devices, 1);
    phone.shutdown().await;

    for bad in ["not hex", &"ab".repeat(31)] {
        assert!(osp_client::device_key_from_hex(bad).is_err(), "{bad}");
    }
    node.shutdown().await.unwrap();
}
