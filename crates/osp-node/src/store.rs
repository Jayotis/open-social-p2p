//! Content-addressed blob storage and transfer, within the storage quotas
//! and the serving limits.
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
    fmt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{Endpoint, EndpointId};
use iroh_blobs::{
    BlobsProtocol, Hash,
    api::{blobs::BlobStatus, proto::ExportRangesItem, remote::GetProgressItem},
    get::{
        GetError,
        fsm::{AtBlobHeaderNextError, DecodeError},
    },
    protocol::{ERR_LIMIT, ERR_PERMISSION, GetRequest},
    provider::events::{
        AbortReason, ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage,
        RequestMode, RequestUpdate, ThrottleMode,
    },
    store::{
        GcConfig, ProtectCb, ProtectOutcome,
        fs::{FsStore, options::Options},
    },
    util::connection_pool::{self, ConnectionPool},
};
use n0_future::StreamExt;
use osp_proto::BlobHash;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::JoinHandle,
};
use tracing::{debug, info, warn};

use crate::{
    config::{ServingConfig, StorageConfig, SwarmConfig},
    db::{BlobClass, Db},
    health,
    peers::{self, Event},
};

/// Give up on a download after this long, so a stalled provider can't hang a sync.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// How long to wait for a connection to a node that may have a blob. Long
/// enough for a first connection through a relay, so a node that is merely
/// slow to reach isn't taken for one that can't be reached.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the garbage collector deletes blobs nothing keeps any more:
/// evicted ones, and the remains of failed downloads.
pub(crate) const GC_INTERVAL: Duration = Duration::from_secs(600);

/// Downloads must leave at least this much free on the disk: the level at
/// which `status` warns.
const MIN_FREE_DISK: u64 = health::LOW_DISK_BYTES;

/// Names of the tags that keep our blobs from the garbage collector.
const TAG_PREFIX: &str = "osp/";

/// Most media transfers served to other nodes at once. Half of them, rounded
/// up, may go to strangers: nodes that neither follow us nor are our
/// devices. A crowd of strangers then can't crowd out our followers.
const MAX_TRANSFERS: usize = 8;
/// With an upload cap, one transfer place for each this many megabits a
/// second: the v1-720p video bitrate (ADR 0005). Each transfer then goes
/// fast enough to finish a video in minutes, and early copies become
/// sources for others sooner, rather than every transfer crawling along
/// together until they time out.
const MBPS_PER_TRANSFER: f64 = 1.5;
/// Fewest transfer places, however slow the upload.
const MIN_TRANSFERS: usize = 2;
/// Most at once for any one node.
const MAX_TRANSFERS_PER_NODE: usize = 2;
/// How long a request over one of those limits waits for a place. After
/// that it is refused as busy, and the requester tries another host.
const BUSY_WAIT: Duration = Duration::from_secs(10);
/// Blobs this small, such as thumbnails, have places of their own, so they
/// aren't held up behind videos. Each takes a moment to send.
pub(crate) const SMALL_BLOB: u64 = 1 << 20;
/// Most small transfers at once. They don't count toward `MAX_TRANSFERS`.
const MAX_SMALL_TRANSFERS: usize = 8;

/// Which provider events we want to see. Every data request is intercepted so
/// the serving rules can refuse it, with news of its transfer, which says how
/// much was sent to each node (ADR 0011); pushes (writes into our store) are
/// off. `protocol` turns on throttling when there is an upload cap.
const EVENT_MASK: EventMask = EventMask {
    connected: ConnectMode::Intercept,
    get: RequestMode::InterceptLog,
    get_many: RequestMode::InterceptLog,
    push: RequestMode::Disabled,
    observe: ObserveMode::Intercept,
    throttle: ThrottleMode::None,
};

pub(crate) struct ContentStore {
    store: FsStore,
    /// Connections to nodes we download from, reused while they're in use.
    pool: ConnectionPool,
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
        let pool = ConnectionPool::new(
            endpoint.clone(),
            iroh_blobs::ALPN,
            connection_pool::Options {
                connect_timeout: CONNECT_TIMEOUT,
                ..Default::default()
            },
        );
        let this = Self {
            store,
            pool,
            db,
            dir: dir.to_path_buf(),
            cache_quota: quotas.cache_quota_mb << 20,
            replica_quota: quotas.replica_quota_mb << 20,
            reserved: Mutex::new(BTreeMap::new()),
        };
        this.reconcile().await?;
        let over = this.db.evictable_bytes()?.saturating_sub(this.cache_quota);
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
    pub fn protocol(
        &self,
        serving: &ServingConfig,
        swarm: &SwarmConfig,
    ) -> (BlobsProtocol, JoinHandle<()>) {
        let pacer = (serving.max_upload_mbps > 0).then(|| Pacer::new(serving.max_upload_mbps));
        let mask = EventMask {
            // Then every 16 KiB sent waits for our go-ahead, which costs a
            // little, so only with a cap.
            throttle: if pacer.is_some() {
                ThrottleMode::Intercept
            } else {
                ThrottleMode::None
            },
            ..EVENT_MASK
        };
        let (events, rx) = EventSender::channel(64, mask);
        let task = tokio::spawn(enforce_serving_rules(
            rx,
            self.db.clone(),
            pacer,
            Places::new(serving.max_upload_mbps),
            swarm.share_prefetched,
        ));
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
        self.fetch(h, providers, declared, Instant::now() + DOWNLOAD_TIMEOUT)
            .await
            .with_context(|| format!("download of {} failed", hash.fmt_short()))?;
        let size = self
            .size(h)
            .await?
            .context("downloaded blob is incomplete")?;
        self.keep(h, class, size).await?;
        Ok(size)
    }

    /// Promises disk space to a file a device is uploading, until the
    /// returned guard is dropped. Uploads and downloads count each other's
    /// promises, and leave the same space free on the disk.
    pub async fn reserve_upload(&self, bytes: u64) -> Result<Reservation<'_>> {
        self.reserve(BlobClass::Own, bytes).await
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

    /// Asks `providers` in turn for what is still missing of a blob, until
    /// it is complete or `deadline` passes. Each node is asked only for what
    /// the ones before it didn't send, and how its attempt ended is logged
    /// (ADR 0011). Stops once more than `declared` bytes have arrived: a
    /// provider can't send bytes that don't match the hash, but the post's
    /// author can declare a smaller size than the blob has. Fails with
    /// [`Busy`] if one of them refused as busy.
    async fn fetch(
        &self,
        hash: Hash,
        providers: Vec<EndpointId>,
        declared: u64,
        deadline: Instant,
    ) -> Result<()> {
        let request = Arc::new(GetRequest::blob(hash));
        let remote = self.store.remote();
        let mut busy = false;
        for node in providers {
            let local = remote.local_for_request(request.clone()).await?;
            if local.is_complete() {
                return Ok(());
            }
            let attempt = self
                .attempt(
                    node,
                    local.missing(),
                    local.local_bytes(),
                    declared,
                    deadline,
                )
                .await;
            attempt.log(hash);
            peers::record(
                &self.db,
                &node,
                Event::Download {
                    outcome: attempt.outcome,
                    bytes: attempt.bytes,
                    took: attempt.took,
                    first_byte: attempt.first_byte,
                },
            );
            match attempt.outcome {
                Outcome::Completed => return Ok(()),
                Outcome::Stopped => {
                    bail!("it is bigger than its post says ({declared} bytes), so it was stopped")
                }
                Outcome::Busy => busy = true,
                _ => {}
            }
            if Instant::now() >= deadline {
                bail!("it timed out");
            }
        }
        if busy {
            return Err(Busy.into());
        }
        bail!("no provider could send it")
    }

    /// Asks one node for `request`: what is still missing of a blob, of
    /// which `local` bytes are already here.
    async fn attempt(
        &self,
        node: EndpointId,
        request: GetRequest,
        local: u64,
        declared: u64,
        deadline: Instant,
    ) -> Attempt {
        let started = Instant::now();
        let mut attempt = Attempt {
            node,
            outcome: Outcome::Failed,
            bytes: 0,
            took: Duration::ZERO,
            first_byte: None,
        };
        let left = || deadline.saturating_duration_since(Instant::now());
        attempt.outcome = match tokio::time::timeout(left(), self.pool.get_or_connect(node)).await {
            Err(_) => Outcome::Failed,
            Ok(Err(e)) => {
                debug!(node = %node.fmt_short(), "couldn't connect: {e}");
                Outcome::Unreachable
            }
            Ok(Ok(conn)) => {
                let mut progress = self
                    .store
                    .remote()
                    .execute_get(conn.clone(), request)
                    .stream();
                loop {
                    match tokio::time::timeout(left(), progress.next()).await {
                        // Out of time, or it ended without saying how.
                        Err(_) | Ok(None) => break Outcome::Failed,
                        Ok(Some(GetProgressItem::Progress(bytes))) => {
                            attempt.first_byte.get_or_insert_with(|| started.elapsed());
                            attempt.bytes = bytes;
                            if local + bytes > declared {
                                break Outcome::Stopped;
                            }
                        }
                        Ok(Some(GetProgressItem::Done(_))) => break Outcome::Completed,
                        Ok(Some(GetProgressItem::Error(e))) => {
                            debug!(node = %node.fmt_short(), "download attempt failed: {e:#}");
                            break outcome_of(&e);
                        }
                    }
                }
            }
        };
        attempt.took = started.elapsed();
        attempt
    }

    /// Promises `bytes` of `class` space to a download until the returned
    /// guard is dropped. Cached and prefetched blobs share the cache quota;
    /// if it is full, they make room (`Db::evict_cache`). Replicas are never
    /// evicted, so a full replica quota is an error. Fails too if the
    /// download would leave the disk too full.
    async fn reserve(&self, class: BlobClass, bytes: u64) -> Result<Reservation<'_>> {
        let evicted = {
            let mut reserved = self.lock_reserved();
            if let Some(free) = health::disk_free(&self.dir) {
                let promised: u64 = reserved.values().sum();
                ensure!(
                    free.saturating_sub(promised) >= bytes.saturating_add(MIN_FREE_DISK),
                    "not enough free disk space: storing {} more would leave less than {} free",
                    mb(bytes),
                    mb(MIN_FREE_DISK)
                );
            }
            let evicted = match class {
                BlobClass::Cache | BlobClass::Prefetch => {
                    ensure!(
                        bytes <= self.cache_quota,
                        "{} is more than the whole media cache ({}, storage.cache_quota_mb)",
                        mb(bytes),
                        mb(self.cache_quota)
                    );
                    let downloading: u64 = reserved
                        .iter()
                        .filter(|(c, _)| c.is_evictable())
                        .map(|(_, n)| n)
                        .sum();
                    ensure!(
                        downloading + bytes <= self.cache_quota,
                        "the media cache is full of downloads in progress; try again shortly"
                    );
                    let stored = self.db.evictable_bytes()?;
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

/// Space promised to one download or upload. Given back when dropped, by
/// which time the bytes are on the disk, or abandoned.
pub struct Reservation<'a> {
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

/// One node's attempt at sending us (part of) a blob.
#[derive(Debug)]
struct Attempt {
    node: EndpointId,
    outcome: Outcome,
    /// Verified payload bytes it sent.
    bytes: u64,
    /// From asking to the end of the attempt.
    took: Duration,
    /// From asking to the first bytes, if any came.
    first_byte: Option<Duration>,
}

/// How an attempt ended (ADR 0011).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// It sent everything we asked for.
    Completed,
    /// It refused because it was busy.
    Busy,
    /// It doesn't have the blob, or won't serve it to us.
    NotHosting,
    /// It sent bytes that don't match the hash.
    BadData,
    /// It couldn't be reached.
    Unreachable,
    /// Anything else: a dropped connection, an error, or out of time.
    Failed,
    /// We stopped it: more arrived than the post says the blob has. That
    /// says nothing about the node, which can only send the blob the hash names.
    Stopped,
}

impl Attempt {
    fn log(&self, hash: Hash) {
        let node = self.node.fmt_short();
        let blob = to_blob_hash(hash).fmt_short();
        if self.outcome == Outcome::BadData {
            warn!(%node, %blob, "a node sent data that doesn't match the hash; trying the next");
        } else {
            debug!(
                %node,
                %blob,
                outcome = ?self.outcome,
                bytes = self.bytes,
                took_ms = self.took.as_millis() as u64,
                first_byte_ms = self.first_byte.map(|d| d.as_millis() as u64),
                "download attempt"
            );
        }
    }
}

/// Why a download failed when a node that has the blob refused as busy:
/// asking again later may work.
#[derive(Debug)]
pub(crate) struct Busy;

impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no provider could send it, and one that has it is busy")
    }
}

impl std::error::Error for Busy {}

/// Whether a download failed because a node that has the blob was busy.
pub(crate) fn is_busy(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| cause.is::<Busy>())
}

/// What a failed request says about the node that was asked. A node refuses
/// by closing the stream with an error code; one that ends before sending
/// the whole blob doesn't have it.
fn outcome_of(e: &GetError) -> Outcome {
    match e.iroh_error_code() {
        Some(code) if code == ERR_LIMIT => return Outcome::Busy,
        Some(code) if code == ERR_PERMISSION => return Outcome::NotHosting,
        _ => {}
    }
    match e {
        GetError::Decode {
            source: DecodeError::LeafHashMismatch { .. } | DecodeError::ParentHashMismatch { .. },
            ..
        } => Outcome::BadData,
        GetError::AtBlobHeaderNext {
            source: AtBlobHeaderNextError::NotFound { .. },
            ..
        }
        | GetError::Decode {
            source:
                DecodeError::ChunkNotFound { .. }
                | DecodeError::ParentNotFound { .. }
                | DecodeError::LeafNotFound { .. },
            ..
        } => Outcome::NotHosting,
        _ => Outcome::Failed,
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
/// - `prefetch` blobs: anyone if `share_prefetched`, otherwise only my
///   paired devices.
/// - `cache` blobs: only my paired devices.
/// - anything else, and any request that would walk a blob as a list of
///   child hashes: refused. (Without that last rule, a crafted request could
///   use a public blob whose bytes happen to be hashes to reach cached blobs,
///   revealing what this node has viewed.)
///
/// A request that may be served still waits for a place within the
/// transfer limits, and is refused as busy if none frees up in time. A
/// small blob needs only a small place (`SMALL_BLOB`). With an upload cap,
/// `pacer` spaces out what is sent.
async fn enforce_serving_rules(
    mut rx: mpsc::Receiver<ProviderMessage>,
    db: Arc<Db>,
    mut pacer: Option<Pacer>,
    mut places: Places,
    share_prefetched: bool,
) {
    let mut peers: HashMap<u64, Option<EndpointId>> = HashMap::new();
    while let Some(msg) = rx.recv().await {
        // Requests for data: who asked, whether the rules allow it, whether
        // it is small, and the channels for the answer and for news of the
        // transfer.
        let (peer, allowed, small, answer, updates) = match msg {
            ProviderMessage::GetRequestReceived(msg) => {
                let peer = peers.get(&msg.inner.connection_id).copied().flatten();
                let request = &msg.inner.request;
                let size = request
                    .ranges
                    .is_blob()
                    .then(|| may_serve(&db, request.hash, peer, share_prefetched))
                    .flatten();
                let small = size.is_some_and(|s| s <= SMALL_BLOB);
                (peer, size.is_some(), small, msg.tx, msg.rx)
            }
            ProviderMessage::GetManyRequestReceived(msg) => {
                let peer = peers.get(&msg.inner.connection_id).copied().flatten();
                let ok = msg
                    .inner
                    .request
                    .hashes
                    .iter()
                    .all(|h| may_serve(&db, *h, peer, share_prefetched).is_some());
                (peer, ok, false, msg.tx, msg.rx)
            }
            ProviderMessage::ClientConnected(msg) => {
                peers.insert(msg.inner.connection_id, msg.inner.endpoint_id);
                msg.tx.send(Ok(())).await.ok();
                continue;
            }
            ProviderMessage::ConnectionClosed(msg) => {
                peers.remove(&msg.inner.connection_id);
                continue;
            }
            ProviderMessage::ObserveRequestReceived(msg) => {
                let peer = peers.get(&msg.inner.connection_id).copied().flatten();
                let ok = may_serve(&db, msg.inner.request.hash, peer, share_prefetched).is_some();
                msg.tx.send(verdict(ok)).await.ok();
                continue;
            }
            ProviderMessage::PushRequestReceived(msg) => {
                msg.tx.send(Err(AbortReason::Permission)).await.ok();
                continue;
            }
            ProviderMessage::Throttle(msg) => {
                let wait = match pacer.as_mut() {
                    Some(pacer) => pacer.wait_for(msg.inner.size),
                    None => Duration::ZERO,
                };
                // Waiting here would hold up every other request.
                tokio::spawn(async move {
                    tokio::time::sleep(wait).await;
                    msg.tx.send(Ok(())).await.ok();
                });
                continue;
            }
            // Notification-only variants; nothing to decide.
            _ => continue,
        };
        if !allowed {
            answer.send(verdict(false)).await.ok();
            continue;
        }
        let needed = places.needed(peer, small, &db);
        let db = db.clone();
        tokio::spawn(async move {
            let Some(_held) = take_places(needed).await else {
                debug!(?peer, "busy: refused a blob request");
                answer.send(Err(AbortReason::RateLimited)).await.ok();
                if let Some(peer) = peer {
                    peers::record(&db, &peer, Event::RefusedBusy);
                }
                return;
            };
            if answer.send(Ok(())).await.is_ok() {
                // The channel closes when the transfer ends; the places
                // are given back then. The last update has what was sent.
                let mut updates = updates;
                let mut sent = None;
                while let Ok(Some(update)) = updates.recv().await {
                    match update {
                        RequestUpdate::Completed(done) => {
                            sent = Some(done.stats.payload_bytes_sent)
                        }
                        RequestUpdate::Aborted(cut) => sent = Some(cut.stats.payload_bytes_sent),
                        _ => {}
                    }
                }
                if let (Some(peer), Some(bytes)) = (peer, sent) {
                    peers::record(&db, &peer, Event::Served { bytes });
                }
            }
        });
    }
}

/// The transfer limits. Each limit is a semaphore with one permit per place.
struct Places {
    all: Arc<Semaphore>,
    strangers: Arc<Semaphore>,
    small: Arc<Semaphore>,
    per_node: HashMap<Option<EndpointId>, Arc<Semaphore>>,
}

impl Places {
    /// The limits for an upload cap of `max_upload_mbps`, 0 for none.
    fn new(max_upload_mbps: u64) -> Self {
        let all = transfer_places(max_upload_mbps);
        Self {
            all: Arc::new(Semaphore::new(all)),
            strangers: Arc::new(Semaphore::new(all.div_ceil(2))),
            small: Arc::new(Semaphore::new(MAX_SMALL_TRANSFERS)),
            per_node: HashMap::new(),
        }
    }

    /// The limits a request from `peer` needs a place in, in the order to
    /// take them. A `small` one needs only a small place, besides its node's.
    fn needed(&mut self, peer: Option<EndpointId>, small: bool, db: &Db) -> Vec<Arc<Semaphore>> {
        // Forget nodes with nothing in progress or waiting.
        self.per_node
            .retain(|_, limit| Arc::strong_count(limit) > 1);
        let own = self
            .per_node
            .entry(peer)
            .or_insert_with(|| Arc::new(Semaphore::new(MAX_TRANSFERS_PER_NODE)));
        let mut needed = vec![own.clone()];
        if small {
            needed.push(self.small.clone());
            return needed;
        }
        if !is_follower_or_device(db, peer) {
            needed.push(self.strangers.clone());
        }
        needed.push(self.all.clone());
        needed
    }
}

/// Media transfers served at once with an upload cap of `mbps`, 0 for none.
fn transfer_places(mbps: u64) -> usize {
    if mbps == 0 {
        return MAX_TRANSFERS;
    }
    let places = (mbps as f64 / MBPS_PER_TRANSFER) as usize;
    places.clamp(MIN_TRANSFERS, MAX_TRANSFERS)
}

/// Waits up to `BUSY_WAIT` for a place in each limit. `None` if one stayed
/// full. The places are given back when the permits are dropped.
async fn take_places(needed: Vec<Arc<Semaphore>>) -> Option<Vec<OwnedSemaphorePermit>> {
    let all = async {
        let mut held = Vec::new();
        for limit in needed {
            held.push(limit.acquire_owned().await.ok()?);
        }
        Some(held)
    };
    tokio::time::timeout(BUSY_WAIT, all).await.ok().flatten()
}

fn is_follower_or_device(db: &Db, peer: Option<EndpointId>) -> bool {
    peer.is_some_and(|p| db.is_follower(&p).unwrap_or(false) || db.is_device(&p).unwrap_or(false))
}

/// Spaces out what the node sends so it stays under `max_upload_mbps`.
struct Pacer {
    bytes_per_sec: f64,
    /// When everything let through so far will have gone out at the cap.
    next: Instant,
}

impl Pacer {
    fn new(mbps: u64) -> Self {
        Self {
            bytes_per_sec: mbps as f64 * 1_000_000.0 / 8.0,
            next: Instant::now(),
        }
    }

    /// How long to hold back the next `bytes`.
    fn wait_for(&mut self, bytes: u64) -> Duration {
        let now = Instant::now();
        let start = self.next.max(now);
        self.next = start + Duration::from_secs_f64(bytes as f64 / self.bytes_per_sec);
        start - now
    }
}

fn verdict(ok: bool) -> Result<(), AbortReason> {
    if ok {
        Ok(())
    } else {
        Err(AbortReason::Permission)
    }
}

/// The blob's size, if `peer` may download it.
fn may_serve(db: &Db, hash: Hash, peer: Option<EndpointId>, share_prefetched: bool) -> Option<u64> {
    let (class, size) = match db.blob(&to_blob_hash(hash)) {
        Ok(Some((class, size))) => (Some(class), size),
        Ok(None) => (None, 0),
        Err(e) => {
            warn!("serving check failed: {e:#}");
            return None;
        }
    };
    let allowed = match class {
        Some(class) if class.served_to_anyone(share_prefetched) => true,
        Some(BlobClass::Cache | BlobClass::Prefetch) => {
            peer.is_some_and(|p| db.is_device(&p).unwrap_or(false))
        }
        _ => false,
    };
    if !allowed {
        debug!(hash = %hash.fmt_short(), ?peer, ?class, "refused blob request");
    }
    allowed.then_some(size)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use iroh::{
        RelayMode, SecretKey,
        address_lookup::MemoryLookup,
        endpoint::{Connection, presets},
        protocol::{AcceptError, ProtocolHandler, Router},
    };
    use iroh_blobs::store::mem::MemStore;

    use super::*;

    const KB: u64 = 1 << 10;
    /// Short, so a test sees the collector run.
    const GC_SOON: Duration = Duration::from_millis(100);

    #[test]
    fn a_slow_upload_offers_fewer_transfers_at_once() {
        let places = |mbps| {
            let p = Places::new(mbps);
            (p.all.available_permits(), p.strangers.available_permits())
        };
        // No cap: as many as ever. A cap: one per 1.5 Mbit/s, within limits.
        assert_eq!(places(0), (8, 4));
        assert_eq!(places(1), (2, 1));
        assert_eq!(places(5), (3, 2));
        assert_eq!(places(10), (6, 3));
        assert_eq!(places(12), (8, 4));
        assert_eq!(places(1000), (8, 4));
        // Small files keep their own places.
        assert_eq!(Places::new(1).small.available_permits(), 8);
    }

    struct Fixture {
        dir: tempfile::TempDir,
        endpoint: Endpoint,
        /// Where the endpoint finds the nodes a test downloads from.
        lookup: MemoryLookup,
        db: Arc<Db>,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = Arc::new(Db::open(&dir.path().join("node.db")).unwrap());
            let lookup = MemoryLookup::new();
            let endpoint = local_endpoint(Some(&lookup)).await;
            Self {
                dir,
                endpoint,
                lookup,
                db,
            }
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

        /// Starts a node that keeps `data`, and refuses every request for
        /// `refuse` if it is given. Returns its ID and the data's hash.
        async fn provider(&self, data: &[u8], refuse: Option<AbortReason>) -> (Provider, Hash) {
            let media = MemStore::new();
            let hash = media.add_bytes(data.to_vec()).await.unwrap().hash;
            let events = refuse.map(|reason| {
                let mask = EventMask {
                    get: RequestMode::Intercept,
                    ..EventMask::DEFAULT
                };
                let (events, mut rx) = EventSender::channel(8, mask);
                tokio::spawn(async move {
                    while let Some(msg) = rx.recv().await {
                        if let ProviderMessage::GetRequestReceived(msg) = msg {
                            msg.tx.send(Err(reason)).await.ok();
                        }
                    }
                });
                events
            });
            let blobs = BlobsProtocol::new(&media, events);
            let provider = self.serve(blobs).await;
            (
                Provider {
                    _media: media,
                    ..provider
                },
                hash,
            )
        }

        /// Starts a node answering blob requests with `handler`, and tells
        /// our endpoint where it is.
        async fn serve(&self, handler: impl ProtocolHandler) -> Provider {
            let endpoint = local_endpoint(None).await;
            self.lookup.add_endpoint_info(endpoint.addr());
            let router = Router::builder(endpoint)
                .accept(iroh_blobs::ALPN, handler)
                .spawn();
            Provider {
                router,
                _media: MemStore::new(),
            }
        }
    }

    /// A node serving blobs, until dropped.
    struct Provider {
        router: Router,
        _media: MemStore,
    }

    impl Provider {
        fn id(&self) -> EndpointId {
            self.router.endpoint().id()
        }
    }

    /// Answers every blob request with the same bytes, then ends the stream.
    #[derive(Debug)]
    struct Fake(Vec<u8>);

    impl Fake {
        /// The size of a 1 KB blob, then 1 KB of bytes that don't match its hash.
        fn liar() -> Self {
            let mut reply = KB.to_le_bytes().to_vec();
            reply.extend([0xff; KB as usize]);
            Self(reply)
        }
    }

    impl ProtocolHandler for Fake {
        async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
            while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                recv.read_to_end(4 * KB as usize).await.ok();
                send.write_all(&self.0).await.ok();
                send.finish().ok();
            }
            Ok(())
        }
    }

    async fn local_endpoint(lookup: Option<&MemoryLookup>) -> Endpoint {
        let mut builder = Endpoint::builder(presets::Minimal)
            .secret_key(SecretKey::generate())
            .relay_mode(RelayMode::Disabled)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
            .unwrap();
        if let Some(lookup) = lookup {
            builder = builder.address_lookup(lookup.clone());
        }
        builder.bind().await.unwrap()
    }

    #[tokio::test]
    async fn each_download_attempt_says_how_it_ended() {
        let f = Fixture::new().await;
        let store = f.open().await;
        let data = vec![7; KB as usize];
        let (honest, hash) = f.provider(&data, None).await;
        let (busy, _) = f.provider(&data, Some(AbortReason::RateLimited)).await;
        let (refusing, _) = f.provider(&data, Some(AbortReason::Permission)).await;
        let (without_it, _) = f.provider(b"something else", None).await;
        let liar = f.serve(Fake::liar()).await;
        let silent = f.serve(Fake(Vec::new())).await;
        let just_the_size = f.serve(Fake(KB.to_le_bytes().to_vec())).await;
        let nowhere = SecretKey::generate().public();

        let soon = Instant::now() + Duration::from_secs(30);
        let attempt = async |node: EndpointId, declared: u64| {
            store
                .attempt(node, GetRequest::blob(hash), 0, declared, soon)
                .await
        };
        assert_eq!(attempt(busy.id(), KB).await.outcome, Outcome::Busy);
        assert_eq!(
            attempt(refusing.id(), KB).await.outcome,
            Outcome::NotHosting
        );
        // Our nodes refuse what they don't keep, as above. iroh-blobs' own
        // provider says it failed instead.
        assert_eq!(attempt(without_it.id(), KB).await.outcome, Outcome::Failed);
        assert_eq!(attempt(liar.id(), KB).await.outcome, Outcome::BadData);
        // It ended without sending the blob, or all of it.
        assert_eq!(attempt(silent.id(), KB).await.outcome, Outcome::NotHosting);
        assert_eq!(
            attempt(just_the_size.id(), KB).await.outcome,
            Outcome::NotHosting
        );
        assert_eq!(attempt(nowhere, KB).await.outcome, Outcome::Unreachable);
        assert_eq!(attempt(honest.id(), KB / 2).await.outcome, Outcome::Stopped);

        let done = attempt(honest.id(), KB).await;
        assert_eq!(done.outcome, Outcome::Completed);
        assert_eq!(done.bytes, KB);
        assert!(done.first_byte.is_some_and(|t| t <= done.took), "{done:?}");
    }

    #[tokio::test]
    async fn a_download_moves_on_from_nodes_that_cant_send_it() {
        let f = Fixture::new().await;
        let store = f.open().await;
        let data = vec![7; KB as usize];
        let (honest, hash) = f.provider(&data, None).await;
        let (busy, _) = f.provider(&data, Some(AbortReason::RateLimited)).await;
        let liar = f.serve(Fake::liar()).await;
        let nowhere = SecretKey::generate().public();

        let blob = to_blob_hash(hash);
        let providers = vec![busy.id(), liar.id(), nowhere, honest.id()];
        let size = store
            .download(&blob, providers, BlobClass::Cache, KB)
            .await
            .unwrap();
        assert_eq!(size, KB);
        assert_eq!(f.db.blob_class(&blob).unwrap(), Some(BlobClass::Cache));

        // None of them could send it. One was only busy, so it's worth
        // trying again later.
        let (other_provider, other) = f.provider(&[8; KB as usize], None).await;
        let other_blob = to_blob_hash(other);
        let err = store
            .download(
                &other_blob,
                vec![busy.id(), liar.id()],
                BlobClass::Cache,
                KB,
            )
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("no provider could send it"),
            "{err:#}"
        );
        assert!(is_busy(&err), "{err:#}");
        let err = store
            .download(&other_blob, vec![liar.id(), nowhere], BlobClass::Cache, KB)
            .await
            .unwrap_err();
        assert!(!is_busy(&err), "{err:#}");

        // Bigger than its post says: no other node can send a smaller one.
        let providers = vec![other_provider.id(), honest.id()];
        let err = store
            .fetch(
                other,
                providers.clone(),
                KB / 2,
                Instant::now() + Duration::from_secs(30),
            )
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("bigger than its post says"),
            "{err:#}"
        );

        // Out of time: the next node isn't asked.
        let (late, unseen) = f.provider(&[9; KB as usize], None).await;
        let err = store
            .fetch(unseen, vec![late.id(), late.id()], KB, Instant::now())
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("timed out"), "{err:#}");
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
    async fn prefetched_and_cached_media_share_the_cache_quota() {
        let f = Fixture::new().await;
        let store = f.open().await;
        let prefetched = f.add(&store, 1, 400 * KB, BlobClass::Prefetch).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        let cached = f.add(&store, 2, 400 * KB, BlobClass::Cache).await;

        // Room for another 400 KB: the cached blob goes first, though newer.
        let room = store.reserve(BlobClass::Prefetch, 400 * KB).await.unwrap();
        assert_eq!(f.db.blob_class(&cached).unwrap(), None);
        assert!(collected(&store, &cached).await);
        assert_eq!(
            f.db.blob_class(&prefetched).unwrap(),
            Some(BlobClass::Prefetch)
        );

        // Downloads in progress of either kind count against the quota.
        let err = store
            .reserve(BlobClass::Cache, 700 * KB)
            .await
            .err()
            .unwrap();
        assert!(format!("{err:#}").contains("in progress"), "{err:#}");
        drop(room);
        store.reserve(BlobClass::Cache, 700 * KB).await.unwrap();
        assert_eq!(f.db.blob_class(&prefetched).unwrap(), None);

        // Over the quota at startup, as after lowering it: trimmed to fit.
        let older = f.add(&store, 3, 700 * KB, BlobClass::Prefetch).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        let newer = f.add(&store, 4, 500 * KB, BlobClass::Prefetch).await;
        store.store.shutdown().await.unwrap();
        drop(store);
        f.open().await;
        assert_eq!(f.db.blob_class(&older).unwrap(), None);
        assert_eq!(f.db.blob_class(&newer).unwrap(), Some(BlobClass::Prefetch));
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
    async fn promised_space_counts_toward_the_free_disk_floor() {
        let f = Fixture::new().await;
        let store = f.open().await;
        let free = health::disk_free(&store.dir).expect("free space is known");
        let margin = 64 << 20;
        let Some(spare) = free.checked_sub(MIN_FREE_DISK + margin) else {
            eprintln!("skipped: less than {} free", mb(MIN_FREE_DISK + margin));
            return;
        };
        // A promise is only bookkeeping, so nearly all the free space can be
        // promised without writing anything.
        let upload = store.reserve_upload(spare).await.unwrap();
        let err = store.reserve_upload(2 * margin).await.err().unwrap();
        assert!(format!("{err:#}").contains("free disk space"), "{err:#}");
        drop(upload);
        store.reserve_upload(2 * margin).await.unwrap();
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
