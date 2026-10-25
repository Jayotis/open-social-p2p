//! Uploads from paired devices that survive a broken connection (ADR 0008).
//!
//! A device names each upload with a random `UploadId`. The node keeps the
//! files it has received in `uploads/<device>-<id>/`, and a row in the
//! `uploads` table says how far the upload got. After a broken connection,
//! or a restart of the node, the device carries on with `ResumeUpload` from
//! the last byte the node has, instead of starting over. Once all the files
//! are here, the node prepares the post (transcodes and publishes it)
//! whether or not the device is still connected, and records the outcome.
//! A device that asks again gets the same post ID, so each upload is
//! published at most once.

use std::{
    collections::HashSet,
    io::SeekFrom,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{
    EndpointId,
    endpoint::{RecvStream, SendStream},
};
use osp_proto::{
    ObjectId,
    framing::write_frame,
    rpc::{ClientResponse, Upload, UploadId},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{MutexGuard, watch},
};
use tracing::{debug, info, warn};

use crate::{
    client::IDLE_TIMEOUT,
    control,
    db::{UploadRow, UploadState, now_ms},
    node::Core,
    store::Reservation,
};

/// Largest file a device may upload. Phone videos up to the length limit fit.
const MAX_UPLOAD_BYTES: u64 = 4 << 30;
/// An unfinished upload nobody has touched for this long is dropped, with
/// its files.
const KEEP_UNFINISHED: Duration = Duration::from_secs(24 * 3600);
/// How long a finished upload's outcome is kept, for a device that asks late.
const KEEP_OUTCOME: Duration = Duration::from_secs(30 * 24 * 3600);
/// Unfinished uploads kept per device. Starting another drops the oldest,
/// so abandoned ones can't pile up.
const MAX_UNFINISHED_PER_DEVICE: usize = 4;
/// How often old uploads are cleared away.
const SWEEP_EVERY: Duration = Duration::from_secs(3600);
/// How often a stream waiting for a post being prepared checks on it.
const OUTCOME_CHECK_EVERY: Duration = Duration::from_millis(200);

pub(crate) type UploadKey = (EndpointId, UploadId);

/// The post ID, or the reason the post failed.
type Outcome = Result<ObjectId, String>;

/// What the node keeps in memory about an upload while streams use it.
pub(crate) struct LiveUpload {
    /// Bumped by each stream that takes the upload over. A stream still
    /// receiving under an older number stops.
    turn: watch::Sender<u64>,
    /// Held while receiving, so a stream that takes over starts only once
    /// the one before it has stopped and its bytes are on the disk.
    receiving: tokio::sync::Mutex<()>,
}

impl LiveUpload {
    fn new() -> Self {
        Self {
            turn: watch::Sender::new(0),
            receiving: tokio::sync::Mutex::new(()),
        }
    }

    /// Takes the upload over from any stream receiving it now, and returns
    /// this stream's number.
    fn take_turn(&self) -> u64 {
        let mut mine = 0;
        self.turn.send_modify(|turn| {
            *turn += 1;
            mine = *turn;
        });
        mine
    }
}

impl Core {
    /// Answers a device's `Post` with uploads: receives the files, then
    /// publishes the post. A post the node can't take on is refused before
    /// any file bytes are read: too big, the node busy with other posts, or
    /// the disk too full. Without an `upload` ID the device can't resume,
    /// so a broken upload is dropped at once, as before resuming existed.
    pub(crate) async fn receive_post(
        self: &Arc<Self>,
        device: EndpointId,
        upload: Option<UploadId>,
        content: String,
        files: &[Upload],
        recv: &mut RecvStream,
    ) -> Result<ObjectId> {
        check_upload(&content, files)?;
        let _slot = self.media_post_slot()?;
        let id = match upload {
            Some(id) => id,
            None => random_id()?,
        };
        // Checked before taking the upload over from a stream receiving it.
        ensure!(
            self.db.upload(&device, &id)?.is_none(),
            "this upload has started already; resume it instead"
        );
        let key = (device, id);
        let live = self.live_upload(&key);
        let mine = live.take_turn();
        let guard = live.receiving.lock().await;
        // Room for every file before any arrives, so two uploads at once
        // can't both count on the same free space.
        let room = self.store.reserve_upload(total_size(files)).await?;
        self.start_upload(&key, &content, files).await?;
        let posted = self
            .finish_upload(&key, content, files, 0, recv, &live, mine, guard, room)
            .await;
        if posted.is_err() && upload.is_none() {
            // Nobody can resume it, so its files are no use.
            let receiving = self.db.upload(&key.0, &key.1)?;
            if receiving.is_some_and(|row| row.state == UploadState::Receiving) {
                self.drop_upload(&key)?;
            }
        }
        posted
    }

    /// Answers `ResumeUpload`: carries on with an upload from the last byte
    /// this node has, or starts it if this node doesn't know it. Writes the
    /// `Resume` reply itself, then returns the final reply.
    pub(crate) async fn resume_upload(
        self: &Arc<Self>,
        device: EndpointId,
        id: UploadId,
        content: String,
        files: &[Upload],
        send: &mut SendStream,
        recv: &mut RecvStream,
    ) -> Result<ClientResponse> {
        check_upload(&content, files)?;
        let key = (device, id);
        let live = self.live_upload(&key);
        // Finished, or being prepared: nothing to receive.
        if let Some(row) = self.db.upload(&device, &id)? {
            check_same_files(&row, files)?;
            if row.state != UploadState::Receiving {
                return self.upload_outcome(&row, send).await;
            }
        }
        let _slot = self.media_post_slot()?;
        let mine = live.take_turn();
        let guard = live.receiving.lock().await;
        // Read again, now that the stream before has stopped.
        let known = match self.db.upload(&device, &id)? {
            None => None,
            Some(row) if row.state == UploadState::Receiving => {
                check_same_files(&row, files)?;
                Some(row)
            }
            Some(row) => {
                drop(guard);
                return self.upload_outcome(&row, send).await;
            }
        };
        let total = total_size(files);
        let from = match known {
            Some(_) => received_bytes(&self.upload_dir(&key), files),
            None => 0,
        };
        let room = self.store.reserve_upload(total - from).await?;
        let content = match known {
            None => {
                self.start_upload(&key, &content, files).await?;
                content
            }
            Some(row) => {
                // When it last moved, for `KEEP_UNFINISHED`.
                self.db
                    .set_upload_state(&device, &id, &UploadState::Receiving, now_ms())?;
                info!(
                    device = %device.fmt_short(),
                    upload = %id.fmt_short(),
                    "resuming an upload at {from} of {total} bytes"
                );
                row.content
            }
        };
        write_frame(send, &ClientResponse::Resume { received: from }).await?;
        let id = self
            .finish_upload(&key, content, files, from, recv, &live, mine, guard, room)
            .await?;
        Ok(ClientResponse::Posted { id })
    }

    /// Answers `CancelUpload`: stops the stream receiving the upload, if
    /// any, and drops the upload with its files. Cancelling one this node
    /// doesn't know is fine; one being prepared or published is refused.
    pub(crate) async fn cancel_upload(&self, device: EndpointId, id: &UploadId) -> Result<()> {
        let key = (device, *id);
        let live = self.live_upload(&key);
        live.take_turn();
        let _guard = live.receiving.lock().await;
        match self.db.upload(&device, id)?.map(|row| row.state) {
            Some(UploadState::Preparing) => {
                bail!("your home node is already preparing this post")
            }
            Some(UploadState::Posted(_)) => bail!("this post was published already"),
            _ => self.drop_upload(&key),
        }
    }

    /// At start, before devices can connect: clears away old uploads and
    /// stray files, then prepares the uploads whose files had all arrived
    /// when the node stopped.
    pub(crate) fn start_uploads(self: &Arc<Self>) -> Result<()> {
        std::fs::create_dir_all(self.home.uploads())?;
        self.expire_uploads(KEEP_UNFINISHED)?;
        self.clear_stray_upload_folders()?;
        for row in self.db.uploads()? {
            let key = (row.device, row.id);
            let complete = match row.state {
                UploadState::Preparing => true,
                UploadState::Receiving => {
                    received_bytes(&self.upload_dir(&key), &row.files) == total_size(&row.files)
                }
                _ => false,
            };
            if !complete {
                continue;
            }
            // Marked before any device can ask, so a resume waits for this
            // instead of preparing the post a second time.
            self.db
                .set_upload_state(&key.0, &key.1, &UploadState::Preparing, now_ms())?;
            let core = self.clone();
            tokio::spawn(async move {
                // Waits for a place, rather than refusing as for a device.
                let _slot = core.media_posts.acquire().await;
                let outcome = core.prepare_upload(&key, row.content, &row.files).await;
                debug!(upload = %key.1.fmt_short(), "prepared an upload after a restart: {outcome:?}");
            });
        }
        Ok(())
    }

    /// Clears away old uploads every `SWEEP_EVERY`.
    pub(crate) async fn sweep_uploads_loop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(SWEEP_EVERY).await;
            if let Err(e) = self.expire_uploads(KEEP_UNFINISHED) {
                warn!("clearing away old uploads: {e:#}");
            }
        }
    }

    /// Drops unfinished uploads nobody has touched for `keep` (normally
    /// `KEEP_UNFINISHED`), unless a stream is receiving one now, and forgets
    /// outcomes older than `KEEP_OUTCOME`. Also lets go of uploads no stream
    /// is using.
    pub(crate) fn expire_uploads(&self, keep: Duration) -> Result<()> {
        let now = now_ms();
        let mut unfinished = HashSet::new();
        for row in self.db.uploads()? {
            let key = (row.device, row.id);
            if row.state.is_finished() {
                continue;
            }
            let old = now - row.updated_ms >= keep.as_millis() as i64;
            if old && row.state == UploadState::Receiving {
                let live = self.live_upload(&key);
                if let Ok(_guard) = live.receiving.try_lock() {
                    info!(upload = %row.id.fmt_short(), "dropping an upload nobody finished");
                    self.drop_upload(&key)?;
                    continue;
                }
            }
            unfinished.insert(key);
        }
        self.db
            .forget_finished_uploads(now - KEEP_OUTCOME.as_millis() as i64)?;
        // Streams take their copy while holding this lock, so one the map
        // holds alone is unused.
        self.lock_uploads()
            .retain(|key, live| Arc::strong_count(live) > 1 || unfinished.contains(key));
        Ok(())
    }

    /// Deletes upload folders that belong to no unfinished upload, left by
    /// a node that stopped part way. Only safe before devices can connect.
    fn clear_stray_upload_folders(&self) -> Result<()> {
        let keep: HashSet<PathBuf> = self
            .db
            .uploads()?
            .iter()
            .filter(|row| !row.state.is_finished())
            .map(|row| self.upload_dir(&(row.device, row.id)))
            .collect();
        for entry in std::fs::read_dir(self.home.uploads())? {
            let path = entry?.path();
            if !keep.contains(&path) {
                debug!("deleting stray upload files {}", path.display());
                let _ = std::fs::remove_dir_all(&path);
            }
        }
        Ok(())
    }

    /// Records a new upload and makes its folder, after dropping the
    /// device's oldest unfinished uploads beyond `MAX_UNFINISHED_PER_DEVICE`.
    async fn start_upload(&self, key: &UploadKey, content: &str, files: &[Upload]) -> Result<()> {
        let (device, id) = key;
        let unfinished: Vec<UploadRow> = self
            .db
            .uploads()?
            .into_iter()
            .filter(|row| row.device == *device && row.state == UploadState::Receiving)
            .collect();
        let excess = (unfinished.len() + 1).saturating_sub(MAX_UNFINISHED_PER_DEVICE);
        for old in &unfinished[..excess] {
            debug!(upload = %old.id.fmt_short(), "dropping a device's oldest unfinished upload");
            if let Err(e) = self.cancel_upload(old.device, &old.id).await {
                debug!("couldn't drop an old upload: {e:#}");
            }
        }
        ensure!(
            self.db.add_upload(device, id, content, files, now_ms())?,
            "this upload has started already; resume it instead"
        );
        let dir = self.upload_dir(key);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(())
    }

    /// Receives the files from byte `from` on, then prepares the post. The
    /// caller holds `live.receiving` (as `guard`), a media slot, and `room`
    /// for the bytes still to come.
    #[allow(clippy::too_many_arguments)]
    async fn finish_upload(
        self: &Arc<Self>,
        key: &UploadKey,
        content: String,
        files: &[Upload],
        from: u64,
        recv: &mut RecvStream,
        live: &LiveUpload,
        mine: u64,
        guard: MutexGuard<'_, ()>,
        room: Reservation<'_>,
    ) -> Result<ObjectId> {
        let dir = self.upload_dir(key);
        let received = receive_files(&dir, files, from, recv, live.turn.subscribe(), mine).await;
        // What arrived is on the disk now, so the free space counts it.
        drop(room);
        if let Err(e) = received {
            // When it last moved, for `KEEP_UNFINISHED`.
            let _ = self
                .db
                .set_upload_state(&key.0, &key.1, &UploadState::Receiving, now_ms());
            return Err(e);
        }
        self.db
            .set_upload_state(&key.0, &key.1, &UploadState::Preparing, now_ms())?;
        drop(guard);
        self.prepare_upload(key, content, files)
            .await
            .map_err(anyhow::Error::msg)
    }

    /// Transcodes and publishes an upload whose files are all here, records
    /// how that went, and deletes the files.
    async fn prepare_upload(
        self: &Arc<Self>,
        key: &UploadKey,
        content: String,
        files: &[Upload],
    ) -> Outcome {
        let dir = self.upload_dir(key);
        let paths: Vec<PathBuf> = files
            .iter()
            .enumerate()
            .map(|(i, file)| dir.join(file_name(i, &file.name)))
            .collect();
        let outcome = self
            .publish_post(content, &paths)
            .await
            .map_err(|e| format!("{e:#}"));
        let state = match &outcome {
            Ok(id) => UploadState::Posted(*id),
            Err(reason) => UploadState::Failed(reason.clone()),
        };
        if let Err(e) = self.db.set_upload_state(&key.0, &key.1, &state, now_ms()) {
            warn!("couldn't record how an upload went: {e:#}");
        }
        let _ = std::fs::remove_dir_all(&dir);
        outcome
    }

    /// The final reply for an upload that is finished or being prepared,
    /// waiting for the outcome unless the device stops waiting.
    async fn upload_outcome(
        &self,
        row: &UploadRow,
        send: &mut SendStream,
    ) -> Result<ClientResponse> {
        let mut state = row.state.clone();
        loop {
            match state {
                UploadState::Posted(id) => return Ok(ClientResponse::Posted { id }),
                UploadState::Failed(reason) => bail!(reason),
                UploadState::Preparing | UploadState::Receiving => {}
            }
            tokio::select! {
                _ = tokio::time::sleep(OUTCOME_CHECK_EVERY) => {}
                _ = send.stopped() => bail!("the device stopped waiting"),
            }
            state = self
                .db
                .upload(&row.device, &row.id)?
                .context("the upload was dropped")?
                .state;
        }
    }

    /// Forgets an upload and deletes its files.
    fn drop_upload(&self, key: &UploadKey) -> Result<()> {
        self.db.remove_upload(&key.0, &key.1)?;
        let _ = std::fs::remove_dir_all(self.upload_dir(key));
        Ok(())
    }

    fn live_upload(&self, key: &UploadKey) -> Arc<LiveUpload> {
        self.lock_uploads()
            .entry(*key)
            .or_insert_with(|| Arc::new(LiveUpload::new()))
            .clone()
    }

    fn upload_dir(&self, (device, id): &UploadKey) -> PathBuf {
        self.home.uploads().join(format!("{device}-{id}"))
    }

    fn lock_uploads(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<UploadKey, Arc<LiveUpload>>> {
        self.uploads.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Refuses an upload the node would never take: too many files, text too
/// long, or a file empty or too big.
fn check_upload(content: &str, files: &[Upload]) -> Result<()> {
    control::check_post(content, files.len())?;
    ensure!(!files.is_empty(), "an upload needs at least one file");
    for file in files {
        ensure!(file.size > 0, "{} is empty", file.name);
        ensure!(
            file.size <= MAX_UPLOAD_BYTES,
            "{} is larger than the {} GB limit",
            file.name,
            MAX_UPLOAD_BYTES >> 30
        );
    }
    Ok(())
}

/// A resumed upload must send the files it started with, or its bytes
/// wouldn't line up.
fn check_same_files(row: &UploadRow, files: &[Upload]) -> Result<()> {
    ensure!(
        row.files == files,
        "these aren't the files this upload started with; post them again"
    );
    Ok(())
}

fn total_size(files: &[Upload]) -> u64 {
    files.iter().map(|f| f.size).sum()
}

/// An ID for an upload whose device didn't name it, so it can't be resumed.
fn random_id() -> Result<UploadId> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|e| anyhow::anyhow!("no secure random numbers available: {e}"))?;
    Ok(UploadId::from_bytes(bytes))
}

/// The name file `i` of an upload is kept under.
fn file_name(i: usize, name: &str) -> String {
    format!("{i}.{}", extension(name))
}

/// The upload's extension if it looks like one. ffmpeg goes by the content,
/// so this only helps when reading logs.
fn extension(name: &str) -> String {
    match Path::new(name).extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) => {
            ext.to_ascii_lowercase()
        }
        _ => "bin".to_string(),
    }
}

/// How many bytes of the files are here, counting them back to back. A
/// file counts only once the ones before it are complete.
fn received_bytes(dir: &Path, files: &[Upload]) -> u64 {
    let mut received = 0;
    for (i, file) in files.iter().enumerate() {
        let len = std::fs::metadata(dir.join(file_name(i, &file.name)))
            .map(|m| m.len())
            .unwrap_or(0);
        received += len.min(file.size);
        if len < file.size {
            break;
        }
    }
    received
}

/// Receives the files from byte `from` on, counting them back to back,
/// into `dir`. Stops early, keeping what arrived, when the stream ends,
/// stalls for `IDLE_TIMEOUT`, or `turn` moves past `mine` because another
/// stream took the upload over.
async fn receive_files<R: AsyncRead + Unpin>(
    dir: &Path,
    files: &[Upload],
    from: u64,
    recv: &mut R,
    mut turn: watch::Receiver<u64>,
    mine: u64,
) -> Result<()> {
    let mut start = 0;
    for (i, file) in files.iter().enumerate() {
        let end = start + file.size;
        if from < end {
            let have = from.saturating_sub(start);
            let path = dir.join(file_name(i, &file.name));
            receive_file(&path, have, file.size, recv, &mut turn, mine)
                .await
                .with_context(|| format!("receiving {}", file.name))?;
        }
        start = end;
    }
    Ok(())
}

/// Receives bytes `have..size` of one file into `path`.
async fn receive_file<R: AsyncRead + Unpin>(
    path: &Path,
    have: u64,
    size: u64,
    recv: &mut R,
    turn: &mut watch::Receiver<u64>,
    mine: u64,
) -> Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .await
        .with_context(|| format!("creating {}", path.display()))?;
    file.seek(SeekFrom::Start(have)).await?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut left = size - have;
    let received = async {
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            let n = tokio::select! {
                n = tokio::time::timeout(IDLE_TIMEOUT, recv.read(&mut buf[..want])) => {
                    n.context("upload stalled")??
                }
                _ = turn.wait_for(|turn| *turn != mine) => {
                    bail!("another connection took this upload over")
                }
            };
            ensure!(n > 0, "upload ended early");
            file.write_all(&buf[..n]).await?;
            left -= n as u64;
        }
        Ok(())
    }
    .await;
    // On the disk before the next stream counts what's here.
    file.flush().await?;
    received
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(name: &str, size: u64) -> Upload {
        Upload {
            name: name.into(),
            size,
        }
    }

    #[test]
    fn upload_extensions_are_sanitised() {
        assert_eq!(extension("IMG_0001.JPG"), "jpg");
        assert_eq!(extension("clip.mp4"), "mp4");
        assert_eq!(extension("../../etc/passwd"), "bin");
        assert_eq!(extension("x.a/b"), "bin");
        assert_eq!(extension("noext"), "bin");
        assert_eq!(extension("weird.sh;rm"), "bin");
    }

    #[tokio::test]
    async fn files_are_received_back_to_back_and_resumed_where_they_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let files = [upload("a.jpg", 4), upload("b.mp4", 6)];
        let (_tx, turn) = watch::channel(1);

        // The stream ends part way into the second file.
        let mut data: &[u8] = b"AAAABB";
        let err = receive_files(dir.path(), &files, 0, &mut data, turn.clone(), 1)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("ended early"), "{err:#}");
        assert_eq!(received_bytes(dir.path(), &files), 6);

        // The next stream sends the rest.
        let mut data: &[u8] = b"BBBB";
        receive_files(dir.path(), &files, 6, &mut data, turn, 1)
            .await
            .unwrap();
        assert_eq!(received_bytes(dir.path(), &files), 10);
        assert_eq!(std::fs::read(dir.path().join("0.jpg")).unwrap(), b"AAAA");
        assert_eq!(std::fs::read(dir.path().join("1.mp4")).unwrap(), b"BBBBBB");
    }

    #[test]
    fn a_file_counts_only_once_the_ones_before_it_are_complete() {
        let dir = tempfile::tempdir().unwrap();
        let files = [upload("a", 4), upload("b", 4)];
        std::fs::write(dir.path().join("0.bin"), b"AA").unwrap();
        std::fs::write(dir.path().join("1.bin"), b"BBBB").unwrap();
        assert_eq!(received_bytes(dir.path(), &files), 2);
        // Bytes past a file's size don't count.
        std::fs::write(dir.path().join("0.bin"), b"AAAAAA").unwrap();
        assert_eq!(received_bytes(dir.path(), &files), 8);
    }

    #[tokio::test]
    async fn a_stream_stops_when_another_takes_the_upload_over() {
        let dir = tempfile::tempdir().unwrap();
        let files = [upload("a", 1 << 20)];
        let (tx, turn) = watch::channel(1);
        // A stream that never sends anything, taken over at once.
        let (_writer, mut reader) = tokio::io::duplex(64);
        tx.send_replace(2);
        let err = receive_files(dir.path(), &files, 0, &mut reader, turn, 1)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("took this upload over"),
            "{err:#}"
        );
    }
}
