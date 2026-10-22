//! Content-addressed blob storage and transfer, within the storage quotas.
//!
//! This is the only module that uses iroh-blobs (ADR 0002). It is still
//! pre-1.0, so keeping it behind this small interface means an API change, or
//! a replacement, touches one file.
//!
//! Every blob we keep has a named tag, and iroh-blobs' garbage collector
//! deletes blobs without one. So evicting a blob means forgetting it in the
//! database and removing its tag; the collector frees the disk space on its
//! next run.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{Endpoint, EndpointId};
use iroh_blobs::{
    BlobsProtocol, Hash,
    api::{
        blobs::BlobStatus,
        downloader::{DownloadProgressItem, Downloader},
        proto::ExportRangesItem,
    },
    provider::events::{
        AbortReason, ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage,
        RequestMode, ThrottleMode,
    },
    store::{
        GcConfig, ProtectCb, ProtectOutcome,
        fs::{FsStore, options::Options},
    },
};
use n0_future::StreamExt;
use osp_proto::BlobHash;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::mpsc,
    task::JoinHandle,
};
use tracing::{debug, info, warn};

use crate::{
    config::StorageConfig,
    db::{BlobClass, Db},
    health,
};

/// Give up on a download after this long, so a stalled provider can't hang a sync.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// How often the garbage collector deletes blobs nothing keeps any more:
/// evicted ones, and the remains of failed downloads.
pub(crate) const GC_INTERVAL: Duration = Duration::from_secs(600);

/// Downloads must leave at least this much free on the disk: the level at
/// which `status` warns.
const MIN_FREE_DISK: u64 = health::LOW_DISK_BYTES;

/// Names of the tags that keep our blobs from the garbage collector.
const TAG_PREFIX: &str = "osp/";

/// Which provider events we want to see. Every data request is intercepted so
/// the serving rules can refuse it; pushes (writes into our store) are off.
const EVENT_MASK: EventMask = EventMask {
    connected: ConnectMode::Intercept,
    get: RequestMode::Intercept,
    get_many: RequestMode::Intercept,
    push: RequestMode::Disabled,
    observe: ObserveMode::Intercept,
    throttle: ThrottleMode::None,
};

pub(crate) struct ContentStore {
    store: FsStore,
    downloader: Downloader,
    db: Arc<Db>,
    /// Where the blobs live, for checking free disk space.
    dir: PathBuf,
    cache_quota: u64,
    replica_quota: u64,
    /// Bytes promised to downloads still running, by class.
    reserved: Mutex<BTreeMap<BlobClass, u64>>,
}

impl ContentStore {
    /// Opens the store, makes its tags match the database, and trims the
    /// cache to its quota. Evicted blobs are deleted every `gc_every`.
    pub async fn open(
        dir: &Path,
        endpoint: &Endpoint,
        db: Arc<Db>,
        quotas: &StorageConfig,
        gc_every: Duration,
    ) -> Result<Self> {
        // The collector waits until the tags have been checked.
        let ready = Arc::new(AtomicBool::new(false));
        let mut options = Options::new(dir);
        options.gc = Some(GcConfig {
            interval: gc_every,
            add_protected: Some(wait_until(ready.clone())),
        });
        let store = FsStore::load_with_opts(dir.join("blobs.db"), options)
            .await
            .with_context(|| format!("opening blob store in {}", dir.display()))?;
        let downloader = Downloader::new(&store, endpoint);
        let this = Self {
            store,
            downloader,
            db,
            dir: dir.to_path_buf(),
            cache_quota: quotas.cache_quota_mb << 20,
            replica_quota: quotas.replica_quota_mb << 20,
            reserved: Mutex::new(BTreeMap::new()),
        };
        this.reconcile().await?;
        let over = this
            .db
            .class_bytes(BlobClass::Cache)?
            .saturating_sub(this.cache_quota);
        if over > 0 {
            let evicted = this.db.evict_cache(over)?;
            info!(blobs = evicted.len(), "trimmed the cache to its quota");
            this.untag(evicted).await;
        }
        ready.store(true, Ordering::Release);
        Ok(this)
    }

    /// A protocol handler that serves blobs by the rules in ADR 0003, plus the
    /// task that applies those rules. Abort the task when shutting down.
    pub fn protocol(&self) -> (BlobsProtocol, JoinHandle<()>) {
        let (events, rx) = EventSender::channel(64, EVENT_MASK);
        let task = tokio::spawn(enforce_serving_rules(rx, self.db.clone()));
        (BlobsProtocol::new(&self.store, Some(events)), task)
    }

    /// Copies a local file into the store and records it under `class`.
    pub async fn import(&self, path: &Path, class: BlobClass) -> Result<(BlobHash, u64)> {
        let path = path
            .canonicalize()
            .with_context(|| format!("reading {}", path.display()))?;
        let len = std::fs::metadata(&path)?.len();
        if let Some(free) = health::disk_free(&self.dir) {
            let promised: u64 = self.lock_reserved().values().sum();
            ensure!(
                free.saturating_sub(promised) > len,
                "not enough free disk space to store {} ({})",
                path.display(),
                mb(len)
            );
        }
        let temp = self
            .store
            .add_path(&path)
            .temp_tag()
            .await
            .with_context(|| format!("importing {}", path.display()))?;
        let hash = temp.hash();
        let size = self.size(hash).await?.context("imported blob is missing")?;
        self.keep(hash, class, size).await?;
        Ok((to_blob_hash(hash), size))
    }

    pub async fn has(&self, hash: &BlobHash) -> Result<bool> {
        Ok(self.store.has(to_hash(hash)).await?)
    }

    /// The blob's size if it is stored completely.
    pub async fn stored_size(&self, hash: &BlobHash) -> Result<Option<u64>> {
        self.size(to_hash(hash)).await
    }

    /// Downloads a blob, trying `providers` in order, and records it under
    /// `class`. Every chunk is verified against the hash before it is
    /// stored. `declared` is the size the blob's post gives: room is made
    /// for that much, and the download stops if more arrives.
    pub async fn download(
        &self,
        hash: &BlobHash,
        providers: Vec<EndpointId>,
        class: BlobClass,
        declared: u64,
    ) -> Result<u64> {
        let h = to_hash(hash);
        if let Some(size) = self.size(h).await? {
            // Already here. Hosting it for others still has to fit that quota.
            let _room = if class == BlobClass::Replica
                && self.db.blob_class(hash)? < Some(BlobClass::Replica)
            {
                Some(self.reserve(class, size).await?)
            } else {
                None
            };
            self.keep(h, class, size).await?;
            return Ok(size);
        }
        ensure!(!providers.is_empty(), "no known providers for {hash}");
        let _room = self.reserve(class, declared).await?;
        // Keeps the collector off the partial blob until it has its tag.
        let _downloading = self.store.tags().temp_tag(h).await?;
        debug!(%hash, ?providers, "downloading");
        tokio::time::timeout(DOWNLOAD_TIMEOUT, self.fetch(h, providers, declared))
            .await
            .context("download timed out")?
            .map_err(|e| anyhow::anyhow!("download of {} failed: {e:#}", hash.fmt_short()))?;
        let size = self
            .size(h)
            .await?
            .context("downloaded blob is incomplete")?;
        self.keep(h, class, size).await?;
        Ok(size)
    }

    /// Fails if hosting `bytes` more for others would go over the replica
    /// quota. Downloads check this themselves; this is for checking a
    /// whole post before fetching any of it.
    pub fn check_replica_room(&self, bytes: u64) -> Result<()> {
        let reserved = self.lock_reserved();
        self.replica_room(&reserved, bytes)
    }

    /// Writes a stored blob to a file.
    pub async fn export(&self, hash: &BlobHash, target: &Path) -> Result<()> {
        let target: PathBuf = if target.is_absolute() {
            target.to_path_buf()
        } else {
            std::env::current_dir()?.join(target)
        };
        self.store
            .export(to_hash(hash), &target)
            .await
            .with_context(|| format!("exporting {} to {}", hash.fmt_short(), target.display()))?;
        Ok(())
    }

    /// Writes `len` bytes of a stored blob, starting at `offset`, to `out`.
    pub async fn write_range<W: AsyncWrite + Unpin>(
        &self,
        hash: &BlobHash,
        offset: u64,
        len: u64,
        out: &mut W,
    ) -> Result<()> {
        let end = offset.checked_add(len).context("range out of bounds")?;
        let mut items = self
            .store
            .export_ranges(to_hash(hash), offset..end)
            .stream();
        let mut position = offset;
        while let Some(item) = items.next().await {
            match item {
                ExportRangesItem::Size(_) => {}
                ExportRangesItem::Data(leaf) => {
                    ensure!(
                        leaf.offset == position,
                        "blob store returned bytes out of order"
                    );
                    out.write_all(&leaf.data).await?;
                    position += leaf.data.len() as u64;
                }
                ExportRangesItem::Error(e) => bail!("reading {}: {e}", hash.fmt_short()),
            }
        }
        ensure!(position == end, "blob {} is incomplete", hash.fmt_short());
        Ok(())
    }

    async fn size(&self, hash: Hash) -> Result<Option<u64>> {
        Ok(match self.store.status(hash).await? {
            BlobStatus::Complete { size } => Some(size),
            _ => None,
        })
    }

    /// Runs a download, stopping it once more than `declared` bytes have
    /// arrived. A provider can't send bytes that don't match the hash, but
    /// the post's author can declare a smaller size than the blob has.
    async fn fetch(&self, hash: Hash, providers: Vec<EndpointId>, declared: u64) -> Result<()> {
        let mut progress = self.downloader.download(hash, providers).stream().await?;
        while let Some(item) = progress.next().await {
            match item {
                DownloadProgressItem::Progress(bytes) => ensure!(
                    bytes <= declared,
                    "it is bigger than its post says ({declared} bytes), so it was stopped"
                ),
                DownloadProgressItem::Error(e) => bail!("{e}"),
                DownloadProgressItem::DownloadError => bail!("no provider could send it"),
                _ => {}
            }
        }
        Ok(())
    }

    /// Promises `bytes` of `class` space to a download until the returned
    /// guard is dropped. If the cache is full, the blobs used longest ago
    /// make room; replicas are never evicted, so a full replica quota is
    /// an error. Fails too if the download would leave the disk too full.
    async fn reserve(&self, class: BlobClass, bytes: u64) -> Result<Reservation<'_>> {
        let evicted = {
            let mut reserved = self.lock_reserved();
            if let Some(free) = health::disk_free(&self.dir) {
                let promised: u64 = reserved.values().sum();
                ensure!(
                    free.saturating_sub(promised) >= bytes.saturating_add(MIN_FREE_DISK),
                    "not enough free disk space: fetching {} would leave less than {} free",
                    mb(bytes),
                    mb(MIN_FREE_DISK)
                );
            }
            let evicted = match class {
                BlobClass::Cache => {
                    ensure!(
                        bytes <= self.cache_quota,
                        "{} is more than the whole media cache ({}, storage.cache_quota_mb)",
                        mb(bytes),
                        mb(self.cache_quota)
                    );
                    let downloading = reserved.get(&class).copied().unwrap_or(0);
                    ensure!(
                        downloading + bytes <= self.cache_quota,
                        "the media cache is full of downloads in progress; try again shortly"
                    );
                    let stored = self.db.class_bytes(class)?;
                    let over = (stored + downloading + bytes).saturating_sub(self.cache_quota);
                    self.db.evict_cache(over)?
                }
                BlobClass::Replica => {
                    self.replica_room(&reserved, bytes)?;
                    Vec::new()
                }
                BlobClass::Own => Vec::new(),
            };
            *reserved.entry(class).or_default() += bytes;
            evicted
        };
        if !evicted.is_empty() {
            debug!(blobs = evicted.len(), "evicted cache blobs to make room");
        }
        self.untag(evicted).await;
        Ok(Reservation {
            store: self,
            class,
            bytes,
        })
    }

    fn replica_room(&self, reserved: &BTreeMap<BlobClass, u64>, bytes: u64) -> Result<()> {
        let used = self.db.class_bytes(BlobClass::Replica)?
            + reserved.get(&BlobClass::Replica).copied().unwrap_or(0);
        ensure!(
            used + bytes <= self.replica_quota,
            "hosting this would go over your replica quota: {} of {} is used, and this needs {} more. \
             Reshare without hosting, or raise storage.replica_quota_mb in config.toml",
            mb(used),
            mb(self.replica_quota),
            mb(bytes)
        );
        Ok(())
    }

    fn lock_reserved(&self) -> MutexGuard<'_, BTreeMap<BlobClass, u64>> {
        self.reserved.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Makes the tags match the database: every blob it lists is tagged,
    /// a cached or hosted blob whose data is gone is forgotten (it can be
    /// fetched again), and a tag it doesn't list, left by an eviction that
    /// was cut short, is removed.
    async fn reconcile(&self) -> Result<()> {
        for (hash, class) in self.db.blobs()? {
            let h = to_hash(&hash);
            if self.store.has(h).await? {
                self.tag(h).await?;
            } else if class == BlobClass::Own {
                warn!(hash = %hash.fmt_short(), "media of one of your posts is missing from the blob store");
            } else {
                warn!(hash = %hash.fmt_short(), ?class, "a stored blob is missing; forgetting it");
                self.db.remove_blob(&hash)?;
            }
        }
        let mut stale = Vec::new();
        let mut tags = self.store.tags().list_prefix(TAG_PREFIX).await?;
        while let Some(tag) = tags.next().await {
            let tag = tag?;
            if self.db.blob_class(&to_blob_hash(tag.hash))?.is_none() {
                stale.push(tag.name);
            }
        }
        for name in stale {
            self.store.tags().delete(name).await?;
        }
        Ok(())
    }

    /// Protects the blob with a named tag, so the collector keeps it, and
    /// records its class.
    async fn keep(&self, hash: Hash, class: BlobClass, size: u64) -> Result<()> {
        self.tag(hash).await?;
        self.db.put_blob(&to_blob_hash(hash), class, size)
    }

    async fn tag(&self, hash: Hash) -> Result<()> {
        self.store.tags().set(tag_name(hash), hash).await?;
        Ok(())
    }

    /// Lets the collector delete blobs the database has already forgotten.
    /// One that fails keeps its tag until the next start (see `reconcile`).
    async fn untag(&self, hashes: Vec<BlobHash>) {
        for hash in hashes {
            if let Err(e) = self.store.tags().delete(tag_name(to_hash(&hash))).await {
                warn!(hash = %hash.fmt_short(), "releasing an evicted blob failed: {e:#}");
            }
        }
    }
}

/// Space promised to one download. Given back when dropped, by which time
/// the blob is recorded in the database, or abandoned.
struct Reservation<'a> {
    store: &'a ContentStore,
    class: BlobClass,
    bytes: u64,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if let Some(n) = self.store.lock_reserved().get_mut(&self.class) {
            *n -= self.bytes;
        }
    }
}

/// Holds the collector back until `ready` is set.
fn wait_until(ready: Arc<AtomicBool>) -> ProtectCb {
    Arc::new(move |_| {
        let outcome = if ready.load(Ordering::Acquire) {
            ProtectOutcome::Continue
        } else {
            ProtectOutcome::Abort
        };
        Box::pin(async move { outcome })
    })
}

fn tag_name(hash: Hash) -> String {
    format!("{TAG_PREFIX}{}", hash.to_hex())
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1 << 20) as f64)
}

fn to_hash(hash: &BlobHash) -> Hash {
    Hash::from_bytes(*hash.as_bytes())
}

fn to_blob_hash(hash: Hash) -> BlobHash {
    BlobHash::from_bytes(*hash.as_bytes())
}

/// Answers the provider's "may I serve this?" questions.
///
/// - `own` and `replica` blobs: anyone.
/// - `cache` blobs: only my paired devices.
/// - anything else, and any request that would walk a blob as a list of
///   child hashes: refused. (Without that last rule, a crafted request could
///   use a public blob whose bytes happen to be hashes to reach cached blobs,
///   revealing what this node has viewed.)
async fn enforce_serving_rules(mut rx: mpsc::Receiver<ProviderMessage>, db: Arc<Db>) {
    let mut peers: HashMap<u64, Option<EndpointId>> = HashMap::new();
    while let Some(msg) = rx.recv().await {
        match msg {
            ProviderMessage::ClientConnected(msg) => {
                peers.insert(msg.inner.connection_id, msg.inner.endpoint_id);
                msg.tx.send(Ok(())).await.ok();
            }
            ProviderMessage::ConnectionClosed(msg) => {
                peers.remove(&msg.inner.connection_id);
            }
            ProviderMessage::GetRequestReceived(msg) => {
                let peer = peers.get(&msg.inner.connection_id).copied().flatten();
                let request = &msg.inner.request;
                let ok = request.ranges.is_blob() && may_serve(&db, request.hash, peer);
                msg.tx.send(verdict(ok)).await.ok();
            }
            ProviderMessage::GetManyRequestReceived(msg) => {
                let peer = peers.get(&msg.inner.connection_id).copied().flatten();
                let ok = msg
                    .inner
                    .request
                    .hashes
                    .iter()
                    .all(|h| may_serve(&db, *h, peer));
                msg.tx.send(verdict(ok)).await.ok();
            }
            ProviderMessage::ObserveRequestReceived(msg) => {
                let peer = peers.get(&msg.inner.connection_id).copied().flatten();
                let ok = may_serve(&db, msg.inner.request.hash, peer);
                msg.tx.send(verdict(ok)).await.ok();
            }
            ProviderMessage::PushRequestReceived(msg) => {
                msg.tx.send(Err(AbortReason::Permission)).await.ok();
            }
            ProviderMessage::Throttle(msg) => {
                msg.tx.send(Ok(())).await.ok();
            }
            // Notification-only variants; nothing to decide.
            _ => {}
        }
    }
}

fn verdict(ok: bool) -> Result<(), AbortReason> {
    if ok {
        Ok(())
    } else {
        Err(AbortReason::Permission)
    }
}

fn may_serve(db: &Db, hash: Hash, peer: Option<EndpointId>) -> bool {
    let class = match db.blob_class(&to_blob_hash(hash)) {
        Ok(class) => class,
        Err(e) => {
            warn!("serving check failed: {e:#}");
            return false;
        }
    };
    let allowed = match class {
        Some(class) if class.is_public() => true,
        Some(BlobClass::Cache) => peer.is_some_and(|p| db.is_device(&p).unwrap_or(false)),
        _ => false,
    };
    if !allowed {
        debug!(hash = %hash.fmt_short(), ?peer, ?class, "refused blob request");
    }
    allowed
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use iroh::{RelayMode, endpoint::presets};

    use super::*;

    const KB: u64 = 1 << 10;
    /// Short, so a test sees the collector run.
    const GC_SOON: Duration = Duration::from_millis(100);

    struct Fixture {
        dir: tempfile::TempDir,
        endpoint: Endpoint,
        db: Arc<Db>,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = Arc::new(Db::open(&dir.path().join("node.db")).unwrap());
            let endpoint = Endpoint::builder(presets::Minimal)
                .relay_mode(RelayMode::Disabled)
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
                .unwrap()
                .bind()
                .await
                .unwrap();
            Self { dir, endpoint, db }
        }

        /// Opens the store with quotas of 1 MB each.
        async fn open(&self) -> ContentStore {
            let quotas = StorageConfig {
                replica_quota_mb: 1,
                cache_quota_mb: 1,
            };
            let blobs = self.dir.path().join("blobs");
            ContentStore::open(&blobs, &self.endpoint, self.db.clone(), &quotas, GC_SOON)
                .await
                .unwrap()
        }

        /// Stores `size` bytes of `fill` under `class`.
        async fn add(
            &self,
            store: &ContentStore,
            fill: u8,
            size: u64,
            class: BlobClass,
        ) -> BlobHash {
            let path = self.dir.path().join(format!("file-{fill}"));
            std::fs::write(&path, vec![fill; size as usize]).unwrap();
            store.import(&path, class).await.unwrap().0
        }
    }

    /// Waits up to five seconds for the collector to delete `hash`.
    async fn collected(store: &ContentStore, hash: &BlobHash) -> bool {
        for _ in 0..50 {
            if !store.has(hash).await.unwrap() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_full_cache_evicts_what_was_used_longest_ago_and_the_bytes_go() {
        let f = Fixture::new().await;
        let store = f.open().await;
        let own = f.add(&store, 1, 600 * KB, BlobClass::Own).await;
        let stale = f.add(&store, 2, 400 * KB, BlobClass::Cache).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        let recent = f.add(&store, 3, 400 * KB, BlobClass::Cache).await;

        // 800 KB cached; another 400 KB needs the oldest one gone.
        let room = store.reserve(BlobClass::Cache, 400 * KB).await.unwrap();
        drop(room);
        assert_eq!(f.db.blob_class(&stale).unwrap(), None);
        assert!(collected(&store, &stale).await, "the collector deletes it");
        assert!(store.has(&recent).await.unwrap());
        assert!(store.has(&own).await.unwrap(), "own media is never evicted");
        assert_eq!(f.db.blob_class(&own).unwrap(), Some(BlobClass::Own));
    }

    #[tokio::test]
    async fn what_doesnt_fit_is_refused_with_a_reason() {
        let f = Fixture::new().await;
        let store = f.open().await;
        let err = store
            .reserve(BlobClass::Cache, 2 << 20)
            .await
            .err()
            .unwrap();
        assert!(format!("{err:#}").contains("whole media cache"), "{err:#}");

        // Replicas are a promise to others, so they're never evicted.
        let hosted = f.add(&store, 1, 700 * KB, BlobClass::Replica).await;
        let err = store
            .reserve(BlobClass::Replica, 400 * KB)
            .await
            .err()
            .unwrap();
        assert!(format!("{err:#}").contains("replica quota"), "{err:#}");
        assert!(store.check_replica_room(400 * KB).is_err());
        assert!(store.check_replica_room(300 * KB).is_ok());
        assert_eq!(f.db.blob_class(&hosted).unwrap(), Some(BlobClass::Replica));

        // Room promised to a download counts until it finishes.
        let room = store.reserve(BlobClass::Replica, 324 * KB).await.unwrap();
        assert!(store.check_replica_room(KB).is_err());
        drop(room);
        assert!(store.check_replica_room(KB).is_ok());
    }

    #[tokio::test]
    async fn tags_are_made_to_match_the_database_at_startup() {
        let f = Fixture::new().await;
        let store = f.open().await;
        let untagged = f.add(&store, 1, 100 * KB, BlobClass::Cache).await;
        let forgotten = f.add(&store, 2, 100 * KB, BlobClass::Cache).await;
        // As if a crash came between the database and the tags.
        store
            .store
            .tags()
            .delete(tag_name(to_hash(&untagged)))
            .await
            .unwrap();
        f.db.remove_blob(&forgotten).unwrap();
        store.store.shutdown().await.unwrap();
        drop(store);

        let store = f.open().await;
        assert!(collected(&store, &forgotten).await);
        // Several collections later, the blob the database lists is still there.
        tokio::time::sleep(GC_SOON * 5).await;
        assert!(store.has(&untagged).await.unwrap());
    }
}
