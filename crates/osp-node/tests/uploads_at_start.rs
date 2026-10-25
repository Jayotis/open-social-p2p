//! What a node does at start with the uploads a stopped node left behind
//! (ADR 0008): it publishes those whose files had all arrived, keeps those
//! still to be resumed, and clears away old and stray ones.
//!
//! The leftovers are written straight into a stopped node's home, as a node
//! killed part way would leave them. Requires ffmpeg.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use osp_node::{Config, Node};
use osp_proto::rpc::{ClientRequest, ClientResponse, Upload, UploadId};

async fn make_clip(dir: &Path) -> PathBuf {
    let out = dir.join("clip.mp4");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=320x240:rate=10",
        ])
        .args(["-c:v", "libx264", "-preset", "ultrafast"])
        .arg(&out)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    out
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Leaves an upload in a stopped node's home: its row, and the first
/// `have` bytes of `file`. Returns its folder.
fn leave_upload(
    home: &Path,
    id: u8,
    content: &str,
    file: &Path,
    have: usize,
    updated_ms: i64,
) -> PathBuf {
    let device = iroh::SecretKey::generate().public();
    let id = UploadId::from_bytes([id; 32]);
    let bytes = std::fs::read(file).unwrap();
    let files = vec![Upload {
        name: "clip.mp4".into(),
        size: bytes.len() as u64,
    }];
    let db = rusqlite::Connection::open(home.join("osp.db")).unwrap();
    db.execute(
        "INSERT INTO uploads (device, id, content, files, state, updated_ms)
         VALUES (?1, ?2, ?3, ?4, 'receiving', ?5)",
        rusqlite::params![
            device.to_string(),
            id.to_string(),
            content,
            serde_json::to_string(&files).unwrap(),
            updated_ms
        ],
    )
    .unwrap();
    let folder = home.join("uploads").join(format!("{device}-{id}"));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("0.mp4"), &bytes[..have]).unwrap();
    folder
}

async fn feed_says(node: &Node, content: &str) -> bool {
    let ClientResponse::Feed { items } = node.handle(ClientRequest::Feed { limit: 10 }).await
    else {
        panic!("no feed");
    };
    items.iter().any(|i| i.content == content)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_node_publishes_received_uploads_and_clears_away_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("alice");
    Node::init(&home, &Config::local_test("Alice")).unwrap();
    let clip = make_clip(dir.path()).await;
    let size = std::fs::metadata(&clip).unwrap().len() as usize;

    let complete = leave_upload(&home, 1, "all here", &clip, size, now_ms());
    let partial = leave_upload(&home, 2, "half here", &clip, size / 2, now_ms());
    let day_ago = now_ms() - 25 * 3600 * 1000;
    let expired = leave_upload(&home, 3, "abandoned", &clip, size / 2, day_ago);
    let stray = home.join("uploads").join("stray");
    std::fs::create_dir_all(&stray).unwrap();
    let scratch = home.join("work").join("transcode-left-over");
    std::fs::create_dir_all(&scratch).unwrap();

    let node = Node::spawn(&home).await.unwrap();
    // Marked before any device could ask, so a resume can't prepare it twice.
    let db = rusqlite::Connection::open(home.join("osp.db")).unwrap();
    let state: String = db
        .query_row(
            "SELECT state FROM uploads WHERE content = 'all here'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(state, "receiving");

    // The upload whose files had all arrived is published.
    let published = tokio::time::timeout(Duration::from_secs(60), async {
        while !feed_says(&node, "all here").await {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(published.is_ok(), "the received upload wasn't published");
    assert!(!complete.exists(), "its files are deleted once published");

    // The half-received one waits to be resumed; the rest are cleared away.
    assert!(partial.join("0.mp4").exists());
    assert!(!feed_says(&node, "half here").await);
    assert!(!expired.exists());
    assert!(!stray.exists());
    assert!(!scratch.exists());

    node.shutdown().await.unwrap();
}
