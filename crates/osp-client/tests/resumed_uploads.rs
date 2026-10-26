//! Uploads from a phone that carry on after a broken connection, a restart
//! of the app or a restart of the node (ADR 0008), and are published once.
//!
//! The node and the "phone" run in this process on loopback, with relays and
//! DNS lookup off. The connection is broken from the progress callback, at
//! a known point. Requires ffmpeg.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use osp_client::{Client, ClientOptions, NoContact, Pairing, PostProgress};
use osp_node::{Config, Node};
use osp_proto::rpc::{ClientRequest, ClientResponse, Upload, UploadId};

async fn start_node(home: &Path, name: &str) -> Node {
    Node::init(home, &Config::local_test(name)).unwrap();
    Node::spawn(home).await.unwrap()
}

async fn paired_phone(node: &Node, dir: &Path) -> Arc<Client> {
    paired_phone_with(node, dir, ClientOptions::local_test()).await
}

async fn paired_phone_with(node: &Node, dir: &Path, options: ClientOptions) -> Arc<Client> {
    let ClientResponse::Pairing { ticket, .. } = node.handle(ClientRequest::NewPairing).await
    else {
        panic!("no pairing ticket");
    };
    let phone = Client::open(dir, options).await.unwrap();
    phone.pair(&ticket, "Test phone").await.unwrap();
    phone
}

/// A second of noise, losslessly encoded: about 11 MB, so there is time to
/// break the connection part way.
async fn make_big_clip(dir: &Path) -> PathBuf {
    let out = dir.join("big.mp4");
    let source = "color=c=gray:s=640x360:d=1,noise=alls=60:allf=t";
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            source,
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-qp",
            "0",
        ],
        &out,
    )
    .await;
    out
}

/// A minute of test pattern: small, but a few seconds' work to transcode.
async fn make_slow_clip(dir: &Path) -> PathBuf {
    let out = dir.join("slow.mp4");
    let source = "testsrc2=duration=60:size=1280x720:rate=30";
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            source,
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "40",
        ],
        &out,
    )
    .await;
    out
}

async fn make_photo(dir: &Path) -> PathBuf {
    let out = dir.join("photo.png");
    ffmpeg(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=640x480",
            "-frames:v",
            "1",
        ],
        &out,
    )
    .await;
    out
}

async fn ffmpeg(args: &[&str], out: &Path) {
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
}

/// How many of the node's posts say `content`.
async fn posts_saying(node: &Node, content: &str) -> usize {
    let ClientResponse::Feed { items } = node.handle(ClientRequest::Feed { limit: 100 }).await
    else {
        panic!("no feed");
    };
    items.iter().filter(|i| i.content == content).count()
}

/// The folders of uploads the node is keeping.
fn upload_folders(home: &Path) -> usize {
    std::fs::read_dir(home.join("uploads")).unwrap().count()
}

/// How many bytes of its first file the node holds for its only upload,
/// or `None` once the upload is gone.
fn uploaded_bytes(home: &Path) -> Option<u64> {
    let folder = std::fs::read_dir(home.join("uploads")).ok()?.next()?.ok()?;
    let file = std::fs::read_dir(folder.path()).ok()?.next()?.ok()?;
    Some(file.metadata().ok()?.len())
}

/// Starts a stopped node again and gives a stopped phone its new address,
/// as the relay and lookup service would. (A stopped node keeps its port
/// until the process ends, so the new one gets another.)
async fn restart(home: &Path, phone_dir: &Path) -> Node {
    let node = Node::spawn(home).await.unwrap();
    let path = phone_dir.join("pairing.json");
    let mut pairing: Pairing = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    pairing.node = node.addr().await;
    std::fs::write(&path, serde_json::to_vec(&pairing).unwrap()).unwrap();
    node
}

/// Drops the node's unfinished uploads, once no stream is receiving them.
async fn expire_uploads(node: &Node, home: &Path) {
    for _ in 0..50 {
        node.expire_uploads(Duration::ZERO).unwrap();
        if upload_folders(home) == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the upload was kept");
}

/// Breaks the phone's connection to its node, from inside a progress callback.
fn break_connection(phone: &Client) {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(phone.disconnect());
    });
}

/// Records progress, and breaks the connection the first time `when` is true.
fn progress_breaking_when<W: Fn(u64, u64) -> bool>(
    phone: &Arc<Client>,
    when: W,
) -> (
    Arc<Mutex<Vec<PostProgress>>>,
    impl Fn(PostProgress) + use<W>,
) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (record, phone, broken) = (seen.clone(), phone.clone(), AtomicBool::new(false));
    let progress = move |p: PostProgress| {
        record.lock().unwrap().push(p);
        if let PostProgress::Sending { sent, total } = p
            && when(sent, total)
            && !broken.swap(true, Ordering::Relaxed)
        {
            break_connection(&phone);
        }
    };
    (seen, progress)
}

/// Where sending carried on after the first lost contact, if it did.
fn resumed_at(seen: &[PostProgress]) -> Option<u64> {
    let waiting = seen.iter().position(|p| *p == PostProgress::Waiting)?;
    seen[waiting..].iter().find_map(|p| match p {
        PostProgress::Sending { sent, .. } => Some(*sent),
        PostProgress::Waiting => None,
    })
}

fn init_logs() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,osp_client=debug,warn")
        .with_test_writer()
        .try_init();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_carries_on_where_it_stopped_after_the_connection_drops() {
    init_logs();
    let dir = tempfile::tempdir().unwrap();
    let node = start_node(&dir.path().join("alice"), "Alice").await;
    let phone = paired_phone(&node, &dir.path().join("phone")).await;
    let files = [
        make_photo(dir.path()).await,
        make_big_clip(dir.path()).await,
    ];
    let photo_size = std::fs::metadata(&files[0]).unwrap().len();

    // The connection breaks half way, inside the second file. Before that,
    // a second `Post` with the same ID is refused without disturbing it.
    let (seen, breaking) = progress_breaking_when(&phone, |sent, total| sent > total / 2);
    let upload = Arc::new(Mutex::new(None));
    let (record, reader, repeated) = (upload.clone(), phone.clone(), AtomicBool::new(false));
    let progress = move |p| {
        let pending = reader.pending_post().expect("pending while sending");
        record.lock().unwrap().get_or_insert(pending.upload);
        if let PostProgress::Sending { sent, total } = p
            && sent > total / 4
            && !repeated.swap(true, Ordering::Relaxed)
        {
            let again = ClientRequest::Post {
                content: pending.content.clone(),
                attachments: Vec::new(),
                uploads: pending
                    .files
                    .iter()
                    .map(|f| Upload {
                        name: f.path.file_name().unwrap().to_string_lossy().into(),
                        size: f.size,
                    })
                    .collect(),
                upload: Some(pending.upload),
            };
            let refused = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(reader.request(&again))
            });
            let err = refused.unwrap_err();
            assert!(err.to_string().contains("started already"), "{err:#}");
        }
        breaking(p);
    };
    let id = phone
        .post_with("carried on".into(), &files, progress)
        .await
        .unwrap();

    // It lost contact once, then carried on from where the node had got
    // to, past the first file, rather than from the start.
    let seen = seen.lock().unwrap().clone();
    let waits = seen.iter().filter(|p| **p == PostProgress::Waiting).count();
    assert_eq!(waits, 1, "{seen:?}");
    let PostProgress::Sending { total, .. } = seen[0] else {
        panic!("{seen:?}");
    };
    let from = resumed_at(&seen).expect("lost contact and carried on");
    assert!(
        from > photo_size && from < total,
        "carried on at {from} of {total}"
    );
    assert_eq!(
        seen.last(),
        Some(&PostProgress::Sending { sent: total, total })
    );

    // Published once, with both files, and nothing left over.
    assert_eq!(posts_saying(&node, "carried on").await, 1);
    let ClientResponse::Item { item } = node.handle(ClientRequest::Show { id }).await else {
        panic!("post not found");
    };
    assert_eq!(item.attachments.len(), 2);
    assert!(phone.pending_post().is_none());
    assert_eq!(upload_folders(&dir.path().join("alice")), 0);

    // A published post can't be cancelled.
    let upload = upload.lock().unwrap().expect("an upload ID");
    let cancel = ClientRequest::CancelUpload { upload };
    let err = phone.request(&cancel).await.unwrap_err();
    assert!(err.to_string().contains("published already"), "{err:#}");

    phone.shutdown().await;
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_gives_up_when_the_node_stays_away_and_is_kept() {
    init_logs();
    let dir = tempfile::tempdir().unwrap();
    let node = start_node(&dir.path().join("alice"), "Alice").await;
    let options = ClientOptions {
        post_give_up: Duration::from_secs(2),
        ..ClientOptions::local_test()
    };
    let phone = paired_phone_with(&node, &dir.path().join("phone"), options).await;
    let photo = make_photo(dir.path()).await;

    // The home computer is off.
    node.shutdown().await.unwrap();
    phone.disconnect().await;
    let started = Instant::now();
    let err = phone
        .post_with("while away".into(), &[photo], |_| {})
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no contact"), "{err:#}");
    assert!(err.downcast_ref::<NoContact>().is_some(), "{err:#}");
    assert!(started.elapsed() < Duration::from_secs(60));
    assert!(phone.pending_post().is_some(), "kept, to carry on later");

    // Discarding it doesn't wait long for a node that isn't there.
    let started = Instant::now();
    phone.discard_post().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(phone.pending_post().is_none());

    phone.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn breaks_further_apart_than_the_give_up_time_dont_give_up() {
    init_logs();
    let dir = tempfile::tempdir().unwrap();
    let node = start_node(&dir.path().join("alice"), "Alice").await;
    let options = ClientOptions {
        post_give_up: Duration::from_secs(2),
        ..ClientOptions::local_test()
    };
    let phone = paired_phone_with(&node, &dir.path().join("phone"), options).await;
    let clip = make_big_clip(dir.path()).await;

    // Breaks at a quarter and at three quarters, more than the give-up time
    // apart. Contact in between starts the clock again.
    let reader = phone.clone();
    let steps = Mutex::new(vec![0.75, 0.5, 0.25]);
    let progress = move |p| {
        let PostProgress::Sending { sent, total } = p else {
            return;
        };
        let mut steps = steps.lock().unwrap();
        let Some(&next) = steps.last() else {
            return;
        };
        if sent as f64 > total as f64 * next {
            steps.pop();
            if next == 0.5 {
                std::thread::sleep(Duration::from_millis(2500));
            } else {
                break_connection(&reader);
            }
        }
    };
    phone
        .post_with("broken twice".into(), &[clip], progress)
        .await
        .unwrap();
    assert_eq!(posts_saying(&node, "broken twice").await, 1);

    phone.shutdown().await;
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_drop_after_sending_gets_the_same_post_without_sending_again() {
    init_logs();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("alice");
    let node = start_node(&home, "Alice").await;
    let phone = paired_phone(&node, &dir.path().join("phone")).await;
    let clip = make_big_clip(dir.path()).await;
    let slow = make_slow_clip(dir.path()).await;

    // The connection breaks once the node has every byte, while it
    // prepares the post.
    let (seen, progress) = progress_breaking_when(&phone, move |sent, total| {
        if sent < total {
            return false;
        }
        for _ in 0..1000 {
            match uploaded_bytes(&home) {
                Some(n) if n < total => std::thread::sleep(Duration::from_millis(5)),
                _ => break,
            }
        }
        true
    });
    // Transcodes run one at a time, so a slow one posted on the node first
    // keeps the phone's post waiting to be prepared when the phone asks again.
    let slow_post = node.handle(ClientRequest::Post {
        content: "slow".into(),
        attachments: vec![slow],
        uploads: Vec::new(),
        upload: None,
    });
    let phone_post = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        phone
            .post_with("prepared once".into(), &[clip], progress)
            .await
    };
    let (slow_posted, posted) = tokio::join!(slow_post, phone_post);
    assert!(
        matches!(slow_posted, ClientResponse::Posted { .. }),
        "{slow_posted:?}"
    );
    let id = posted.unwrap();

    // The phone asked again and got the post's ID, without sending more.
    let seen = seen.lock().unwrap().clone();
    assert!(seen.contains(&PostProgress::Waiting), "{seen:?}");
    assert_eq!(resumed_at(&seen), None, "{seen:?}");
    assert_eq!(posts_saying(&node, "prepared once").await, 1);
    let ClientResponse::Item { item } = node.handle(ClientRequest::Show { id }).await else {
        panic!("post not found");
    };
    assert_eq!(item.content, "prepared once");

    phone.shutdown().await;
    node.shutdown().await.unwrap();
}

/// Starts a post and stops it half way, as if the app had been closed.
async fn post_stopped_half_way(phone: &Arc<Client>, content: &str, files: &[PathBuf]) {
    stopped_past(phone, 0.5, |progress| async move {
        phone.post_with(content.into(), files, progress).await
    })
    .await;
}

/// Runs a post (`post_with` or `continue_post`) and stops it once more than
/// `fraction` of its bytes have gone, as if the app had been closed.
async fn stopped_past<F, Fut>(phone: &Arc<Client>, fraction: f64, post: F)
where
    F: FnOnce(Box<dyn Fn(PostProgress)>) -> Fut,
    Fut: Future<Output = anyhow::Result<osp_proto::ObjectId>>,
{
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let stop = Mutex::new(Some(stop));
    let (_, progress) = progress_breaking_when(phone, move |sent, total| {
        let past = sent as f64 > total as f64 * fraction;
        if past && let Some(stop) = stop.lock().unwrap().take() {
            let _ = stop.send(());
        }
        past
    });
    tokio::select! {
        posted = post(Box::new(progress)) => {
            panic!("the post finished before it was stopped: {posted:?}")
        }
        _ = stopped => {}
    }
    assert!(phone.pending_post().is_some());
}

/// Checks that the node's partial copy of its only upload is the start of
/// `source`, byte for byte, once no stream is writing it.
async fn node_holds_the_start_of(home: &Path, source: &Path) -> u64 {
    let source = std::fs::read(source).unwrap();
    let mut last = None;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let len = uploaded_bytes(home).expect("the upload is kept");
        if last == Some(len) {
            break;
        }
        last = Some(len);
    }
    let folder = std::fs::read_dir(home.join("uploads"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    let held = std::fs::read(folder.path().join("0.mp4")).unwrap();
    assert!(!held.is_empty() && held.len() < source.len());
    assert!(
        held == source[..held.len()],
        "the node's copy differs from the file"
    );
    held.len() as u64
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_carries_on_after_the_app_and_the_node_restart() {
    init_logs();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("alice");
    let node = start_node(&home, "Alice").await;
    let phone = paired_phone(&node, &dir.path().join("phone")).await;
    let clip = make_big_clip(dir.path()).await;
    post_stopped_half_way(&phone, "after restarts", std::slice::from_ref(&clip)).await;
    phone.shutdown().await;

    // The node restarts, and so does the app.
    node.shutdown().await.unwrap();
    let node = restart(&home, &dir.path().join("phone")).await;
    let phone = Client::open(&dir.path().join("phone"), ClientOptions::local_test())
        .await
        .unwrap();
    let pending = phone.pending_post().expect("the post was kept");
    assert_eq!(pending.content, "after restarts");
    let half = node_holds_the_start_of(&home, &clip).await;

    // Carrying on puts each byte in its place: stopped again further on,
    // the node's copy is still the start of the file.
    stopped_past(&phone, 0.75, |progress| phone.continue_post(progress)).await;
    assert!(node_holds_the_start_of(&home, &clip).await > half);

    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    phone
        .continue_post(move |p| record.lock().unwrap().push(p))
        .await
        .unwrap();
    let seen = seen.lock().unwrap().clone();
    let PostProgress::Sending { sent: from, total } = seen[0] else {
        panic!("{seen:?}");
    };
    assert!(from > 0 && from < total, "carried on at {from} of {total}");
    assert_eq!(posts_saying(&node, "after restarts").await, 1);
    assert!(phone.pending_post().is_none());
    assert_eq!(upload_folders(&home), 0);

    phone.shutdown().await;
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pending_post_can_be_discarded_expired_or_stopped_by_a_changed_file() {
    init_logs();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("alice");
    let node = start_node(&home, "Alice").await;
    let phone = paired_phone(&node, &dir.path().join("phone")).await;
    let clip = make_big_clip(dir.path()).await;

    // Discarding drops the files on both sides.
    post_stopped_half_way(&phone, "discarded", std::slice::from_ref(&clip)).await;
    assert_eq!(upload_folders(&home), 1);
    phone.discard_post().await.unwrap();
    assert!(phone.pending_post().is_none());
    assert_eq!(upload_folders(&home), 0);

    // An upload nobody touched for long enough is dropped, and the post
    // then starts over, still published once.
    post_stopped_half_way(&phone, "expired", std::slice::from_ref(&clip)).await;
    expire_uploads(&node, &home).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    phone
        .continue_post(move |p| record.lock().unwrap().push(p))
        .await
        .unwrap();
    let first = seen.lock().unwrap()[0];
    assert!(
        matches!(first, PostProgress::Sending { sent: 0, .. }),
        "{first:?}"
    );
    assert_eq!(posts_saying(&node, "expired").await, 1);

    // An upload is never dropped while it's being received, however old.
    let expired = AtomicBool::new(false);
    phone
        .post_with(
            "received meanwhile".into(),
            std::slice::from_ref(&clip),
            |p| {
                if let PostProgress::Sending { sent, total } = p
                    && sent > total / 4
                    && !expired.swap(true, Ordering::Relaxed)
                {
                    node.expire_uploads(Duration::ZERO).unwrap();
                }
            },
        )
        .await
        .unwrap();
    assert_eq!(posts_saying(&node, "received meanwhile").await, 1);

    // A file that changed since the post began stops it.
    post_stopped_half_way(&phone, "changed", std::slice::from_ref(&clip)).await;
    std::fs::write(&clip, b"something else").unwrap();
    let err = phone.continue_post(|_| {}).await.unwrap_err();
    assert!(
        err.to_string().contains("changed since the post began"),
        "{err:#}"
    );
    assert!(
        phone.pending_post().is_some(),
        "kept, for the owner to discard"
    );

    // So does one that's gone.
    std::fs::remove_file(&clip).unwrap();
    let err = phone.continue_post(|_| {}).await.unwrap_err();
    assert!(
        err.to_string().contains("can't be read any more"),
        "{err:#}"
    );
    assert!(phone.pending_post().is_some());

    phone.shutdown().await;
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn uploads_belong_to_their_device_and_keep_their_files() {
    init_logs();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("alice");
    let node = start_node(&home, "Alice").await;
    let phone = paired_phone(&node, &dir.path().join("phone")).await;
    let other = paired_phone(&node, &dir.path().join("other")).await;
    let files = vec![Upload {
        name: "clip.mp4".into(),
        size: 1000,
    }];
    let post = |id: u8| ClientRequest::Post {
        content: "raw".into(),
        attachments: Vec::new(),
        uploads: files.clone(),
        upload: Some(UploadId::from_bytes([id; 32])),
    };

    // Without an ID the upload can't be resumed, so a broken one is
    // dropped at once.
    let unnamed = ClientRequest::Post {
        content: "raw".into(),
        attachments: Vec::new(),
        uploads: files.clone(),
        upload: None,
    };
    let err = phone.request(&unnamed).await.unwrap_err();
    assert!(format!("{err:#}").contains("ended early"), "{err:#}");
    assert_eq!(upload_folders(&home), 0);

    // A `Post` whose bytes never come is kept for resuming; its ID can't
    // start a second upload.
    let err = phone.request(&post(1)).await.unwrap_err();
    assert!(format!("{err:#}").contains("ended early"), "{err:#}");
    let err = phone.request(&post(1)).await.unwrap_err();
    assert!(err.to_string().contains("started already"), "{err:#}");
    assert_eq!(upload_folders(&home), 1);

    // Resuming it with other files is refused.
    let resume = |files: Vec<Upload>| ClientRequest::ResumeUpload {
        upload: UploadId::from_bytes([1; 32]),
        content: "raw".into(),
        uploads: files,
    };
    let mut others = files.clone();
    others[0].size = 999;
    let err = phone.request(&resume(others)).await.unwrap_err();
    assert!(err.to_string().contains("aren't the files"), "{err:#}");

    // Another device using the same ID starts its own upload.
    let resp = other.request(&resume(files.clone())).await.unwrap();
    assert!(
        matches!(resp, ClientResponse::Resume { received: 0 }),
        "{resp:?}"
    );
    assert_eq!(upload_folders(&home), 2);

    // A device keeps at most four unfinished uploads: a fifth drops the oldest.
    for id in 2..=5 {
        phone.request(&post(id)).await.unwrap_err();
    }
    assert_eq!(
        upload_folders(&home),
        5,
        "four of the phone's, one of the other's"
    );
    let resp = phone.request(&resume(files.clone())).await.unwrap();
    assert!(
        matches!(resp, ClientResponse::Resume { received: 0 }),
        "the oldest was dropped, so it starts again: {resp:?}"
    );

    for client in [phone, other] {
        client.shutdown().await;
    }
    node.shutdown().await.unwrap();
}
