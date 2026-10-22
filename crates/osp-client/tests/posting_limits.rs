//! Limits on posting media: posts take turns at ffmpeg, a stuck ffmpeg is
//! stopped, a busy node refuses more posts, and an upload needs room on the
//! node's disk before it is sent.
//!
//! Runs on loopback like `phone.rs`. Requires ffmpeg.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use osp_client::{Client, ClientOptions};
use osp_node::{Config, Node};
use osp_proto::rpc::{ClientRequest, ClientResponse};

/// The node's limit on posts with media in progress at once.
const MAX_MEDIA_POSTS: usize = 4;
/// What the node leaves free on its disk.
const DISK_FLOOR: u64 = 2 << 30;

async fn start_node(root: &Path, config: &Config) -> Node {
    let home = root.join(config.name.to_lowercase());
    Node::init(&home, config).unwrap();
    Node::spawn(&home).await.unwrap()
}

async fn paired_phone(node: &Node, dir: &Path) -> Arc<Client> {
    let ClientResponse::Pairing { ticket, .. } = node.handle(ClientRequest::NewPairing).await
    else {
        panic!("no pairing ticket")
    };
    let phone = Client::open(&dir.join("phone"), ClientOptions::local_test())
        .await
        .unwrap();
    phone.pair(&ticket, "Test phone").await.unwrap();
    phone
}

async fn make_clip(dir: &Path) -> PathBuf {
    let out = dir.join("clip.mp4");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args(["-i", "testsrc=duration=1:size=320x240:rate=30"])
        .args(["-c:v", "libx264", "-preset", "ultrafast"])
        .arg(&out)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    out
}

/// A post of `path` from a file on the node's own machine.
fn local_post(content: &str, path: &Path) -> ClientRequest {
    ClientRequest::Post {
        content: content.into(),
        attachments: vec![path.to_path_buf()],
        uploads: vec![],
    }
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// Whether a process has ended. One its parent hasn't collected yet counts.
async fn stopped(pid: &str) -> bool {
    for _ in 0..50 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return true;
        };
        // The state letter follows the name, which is in brackets.
        let state = stat.rsplit(')').next().unwrap_or("").trim_start();
        if state.starts_with('Z') {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn media_posts_take_turns_and_a_stuck_ffmpeg_is_stopped() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let clip = make_clip(dir.path()).await;

    // Stands in for ffmpeg: passes the startup check, then each run writes
    // down its process ID and never finishes.
    let started = dir.path().join("started");
    let fake = dir.path().join("fake-ffmpeg");
    let script = format!(
        "#!/bin/sh\n[ \"$1\" = -version ] && exit 0\necho $$ >> '{}'\nexec sleep 60\n",
        started.display()
    );
    std::fs::write(&fake, script).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut config = Config::local_test("Alice");
    config.media.ffmpeg = fake.display().to_string();
    config.media.max_transcode_secs = 2;
    let node = Arc::new(start_node(dir.path(), &config).await);
    let phone = paired_phone(&node, dir.path()).await;

    // As many posts with a video as the node takes: one transcodes, the
    // others wait their turn.
    let mut posts = Vec::new();
    for i in 0..MAX_MEDIA_POSTS {
        let node = node.clone();
        let post = local_post(&format!("post {i}"), &clip);
        posts.push(tokio::spawn(async move { node.handle(post).await }));
    }
    for _ in 0..50 {
        if !lines(&started).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(lines(&started).len(), 1, "only one ffmpeg runs at a time");

    // One more is refused at once, from the phone or from the node's machine.
    let err = phone
        .post("one more".into(), std::slice::from_ref(&clip), |_, _| {})
        .await
        .unwrap_err();
    assert!(err.to_string().contains("busy"), "{err:#}");
    let resp = node.handle(local_post("one more", &clip)).await;
    assert!(
        matches!(&resp, ClientResponse::Error { message } if message.contains("busy")),
        "{resp:?}"
    );
    // Text isn't transcoded, so it still goes through.
    let resp = node
        .handle(ClientRequest::Post {
            content: "just text".into(),
            attachments: vec![],
            uploads: vec![],
        })
        .await;
    assert!(matches!(resp, ClientResponse::Posted { .. }), "{resp:?}");

    // Each stuck ffmpeg is stopped at the time limit, then the next starts.
    for post in posts {
        match post.await.unwrap() {
            ClientResponse::Error { message } => {
                assert!(message.contains("max_transcode_secs"), "{message}")
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    let pids = lines(&started);
    assert_eq!(pids.len(), MAX_MEDIA_POSTS);
    for pid in &pids {
        assert!(stopped(pid).await, "ffmpeg {pid} is still running");
    }
    // The slots are free again.
    let resp = node.handle(local_post("after", &clip)).await;
    assert!(
        matches!(&resp, ClientResponse::Error { message } if message.contains("max_transcode_secs")),
        "{resp:?}"
    );

    phone.shutdown().await;
    Arc::into_inner(node).unwrap().shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_needs_room_on_the_disk_before_it_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let node = start_node(dir.path(), &Config::local_test("Alice")).await;
    let phone = paired_phone(&node, dir.path()).await;

    let ClientResponse::Status(status) = node.handle(ClientRequest::Status).await else {
        panic!("status failed")
    };
    let free = status.disk_free_bytes.expect("free space is known");
    let size: u64 = 64 << 20;
    let Some(spare) = free.checked_sub(DISK_FLOOR + size) else {
        eprintln!("skipped: not enough free disk space to run this test");
        return;
    };
    // Sparse, so it takes no space here, but 64 MB are sent.
    let file = dir.path().join("big.mp4");
    std::fs::File::create(&file).unwrap().set_len(size).unwrap();

    // Another upload has been promised nearly all the space above the
    // floor: half this file would fit, the whole file doesn't.
    let held = node.reserve_disk_space(spare + size / 2).await.unwrap();
    let err = phone
        .post("too big".into(), std::slice::from_ref(&file), |_, _| {})
        .await
        .unwrap_err();
    assert!(err.to_string().contains("free disk space"), "{err:#}");
    drop(held);

    // With room, the file arrives (and fails as media). Its room was given
    // back, so the same promise fits again.
    let err = phone
        .post("not media".into(), std::slice::from_ref(&file), |_, _| {})
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not a readable media file"),
        "{err:#}"
    );
    let held = node.reserve_disk_space(spare + size / 2).await.unwrap();
    drop(held);

    phone.shutdown().await;
    node.shutdown().await.unwrap();
}
