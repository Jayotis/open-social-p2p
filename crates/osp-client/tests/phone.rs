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
};

use osp_client::{Client, ClientOptions, Pairing};
use osp_node::{Config, Node};
use osp_proto::{
    BlobHash,
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

    // Device management stays on the node itself.
    for req in [ClientRequest::NewPairing, ClientRequest::Devices] {
        assert!(phone.request(&req).await.is_err());
    }
    let paths = ClientRequest::Post {
        content: "sneaky".into(),
        attachments: vec!["/etc/hostname".into()],
        uploads: vec![],
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
