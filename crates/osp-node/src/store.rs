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
    collections::{BTreeMap, HashMap, VecDeque},
    fmt,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{Endpoint, EndpointId};
use iroh_blobs::{
    BlobsProtocol, Hash,
    api::{
        blobs::BlobStatus,
        proto::{Bitfield, ExportRangesItem},
        remote::GetProgressItem,
    },
    get::{
        GetError,
        fsm::{AtBlobHeaderNextError, DecodeError},
    },
    protocol::{ChunkRanges, ChunkRangesExt, ERR_LIMIT, ERR_PERMISSION, GetRequest},
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
use n0_future::{FuturesUnordered, StreamExt, boxed::BoxStream};
use osp_proto::BlobHash;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc, watch},
    task::JoinHandle,
};
use tracing::{debug, info, warn};

use crate::{
    config::{ServingConfig, StorageConfig, SwarmConfig},
    db::{BlobClass, Db},
    health,
    peers::{self, Event},
};

/// Give up on a node that has sent nothing for this long, and ask the next:
/// its connection may have dropped out. Longer than a busy node takes to
/// refuse (`BUSY_WAIT`), so a refusal is still heard as one.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Give up on a whole download after this long, however it's going, so a
/// node sending just enough not to stall can't hold it forever. A download
/// that is merely slow, from slow or lossy connections, has time to finish.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How long to wait for a connection to a node that may have a blob. Long
/// enough for a first connection through a relay, so a node that is merely
/// slow to reach isn't taken for one that can't be reached.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Most nodes one blob is fetched from at once, each sending a part of it
/// (doc 13). A `WhoHosts` reply names up to six followers with a copy;
/// three of them give most of the speed, and leave the others free for
/// other nodes fetching it.
pub(crate) const MAX_SOURCES: usize = 3;
/// Parts are at least this big, so only a blob at least twice this size is
/// fetched in parts. A smaller one takes moments from one node, and each
/// node asked costs a connection and one of its transfer places.
const MIN_PART: u64 = 1 << 20;
/// Parts start on a multiple of this, the blob store's block size: a node
/// always sends whole blocks, so two parts never share one.
const BLOCK: u64 = 16 << 10;
/// A node's rate for its part is judged once it has been sending for this
/// long.
const RATE_AFTER: Duration = Duration::from_millis(500);
/// A node that has sent nothing of its part for this long has stalled, as
/// far as handing its part to another node goes.
const SLOW_START: Duration = Duration::from_secs(2);
/// A part is split by its node's and the taker's rates only if that brings
/// its end at least this much sooner: equal nodes with little to go aren't
/// worth a new request.
const SPLIT_GAIN: Duration = Duration::from_secs(2);
/// How often a node with nothing to do looks again for a part to take, as
/// the others' rates become known; and how often a node whose part was
/// all taken, while it sends nothing, is checked on.
const RECHECK: Duration = Duration::from_millis(250);

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
/// A node fetching a blob in parts asks for its next part as soon as one
/// ends. Its places are kept for it this long after a transfer of part of
/// a blob, so that request doesn't wait behind others: to the rest, it's
/// as if the node fetched the whole blob in one transfer.
const KEEP_PLACES: Duration = Duration::from_secs(2);
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
    /// Most nodes one blob is fetched from at once (`MAX_SOURCES`).
    max_sources: usize,
}

impl ContentStore {
    /// Opens the store, makes its tags match the database, and trims the
    /// cache to its quota. Evicted blobs are deleted every `gc_every`. A
    /// blob is fetched from up to `max_sources` nodes at once.
    pub async fn open(
        dir: &Path,
        endpoint: &Endpoint,
        db: Arc<Db>,
        quotas: &StorageConfig,
        gc_every: Duration,
        max_sources: usize,
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
            max_sources: max_sources.max(1),
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

    /// Downloads a blob from `sources`, and records it under `class`.
    /// Every chunk is verified against the hash before it is stored.
    /// `declared` is the size the blob's post gives: room is made for that
    /// much, and the download stops if more arrives.
    pub async fn download(
        &self,
        hash: &BlobHash,
        sources: Sources,
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
        ensure!(!sources.is_empty(), "no known providers for {hash}");
        let _room = self.reserve(class, declared).await?;
        // Keeps the collector off the partial blob until it has its tag.
        let _downloading = self.store.tags().temp_tag(h).await?;
        debug!(%hash, ?sources, "downloading");
        self.fetch(h, &sources, declared, Instant::now() + DOWNLOAD_TIMEOUT)
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

    /// A blob that may still be on its way here, to read as its bytes
    /// arrive. Each byte was checked against the hash before it was stored.
    pub async fn arriving(&self, hash: &BlobHash) -> Result<Arriving> {
        let hash = to_hash(hash);
        let mut news = self.store.observe(hash).stream().await?.boxed();
        let have = news
            .next()
            .await
            .context("the blob store didn't say what it has")?;
        Ok(Arriving {
            store: self.store.clone(),
            hash,
            news,
            have,
        })
    }

    async fn size(&self, hash: Hash) -> Result<Option<u64>> {
        Ok(match self.store.status(hash).await? {
            BlobStatus::Complete { size } => Some(size),
            _ => None,
        })
    }

    /// Fetches what is still missing of a blob from `sources`, until it is
    /// complete or `deadline` passes. A large one comes in parts from
    /// several of `sources.at_once` (`fetch_in_parts`); then the nodes that
    /// didn't fail there, and `sources.in_turn`, are asked one at a time,
    /// each only for what the ones before it didn't send. How each attempt
    /// ended is logged (ADR 0011). Stops once more than `declared` bytes
    /// have arrived: a provider can't send bytes that don't match the
    /// hash, but the post's author can declare a smaller size than the blob
    /// has. Fails with [`Busy`] if one of them refused as busy.
    async fn fetch(
        &self,
        hash: Hash,
        sources: &Sources,
        declared: u64,
        deadline: Instant,
    ) -> Result<()> {
        let request = Arc::new(GetRequest::blob(hash));
        let remote = self.store.remote();
        let in_parts = self.max_sources > 1
            && sources.at_once.len() > 1
            && declared >= 2 * MIN_PART
            && !remote
                .local_for_request(request.clone())
                .await?
                .is_complete();
        let failed = if in_parts {
            self.fetch_in_parts(hash, &sources.at_once, declared, deadline)
                .await?
        } else {
            Vec::new()
        };
        let mut busy = failed.iter().any(|(_, outcome)| *outcome == Outcome::Busy);
        let rest = sources
            .at_once
            .iter()
            .chain(&sources.in_turn)
            .filter(|node| !failed.iter().any(|(f, _)| f == *node));
        for &node in rest {
            let local = remote.local_for_request(request.clone()).await?;
            if local.is_complete() {
                return Ok(());
            }
            let stop = Stop::PastDeclared {
                local: local.local_bytes(),
                declared,
            };
            let attempt = self.attempt(node, local.missing(), stop, deadline).await;
            self.record(hash, &attempt);
            match attempt.outcome {
                Outcome::Completed => return Ok(()),
                Outcome::Stopped => {
                    bail!("it is bigger than its post says ({declared} bytes), so it was stopped")
                }
                Outcome::Busy => busy = true,
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(Unsent("it timed out").into());
            }
        }
        // The parts may have brought all of it, with no node left to ask.
        if remote.local_for_request(request).await?.is_complete() {
            return Ok(());
        }
        if busy {
            return Err(Busy.into());
        }
        Err(Unsent("no provider could send it").into())
    }

    /// Fetches what is missing of a blob's first `declared` bytes in parts,
    /// from up to `max_sources` of `nodes` at once, in their order. Each
    /// node is asked for one stretch of the blob, its part, so it keeps
    /// one transfer place throughout, as for a whole blob. A node that
    /// finishes takes over the end of the part that will take longest
    /// (`split_slowest`), and looks again every `RECHECK` while there's
    /// none worth taking. What a node that fails didn't send goes to a node that has
    /// sent its part, whose place is kept for it (`KEEP_PLACES`); only if
    /// none is left fetching does it go to a node not asked yet, which
    /// may well be as busy. So no more nodes fetch at once than the first
    /// few asked. Returns the nodes that failed, and how: they aren't
    /// asked again. Whatever is still missing at the end, `fetch` asks for
    /// from one node at a time.
    async fn fetch_in_parts(
        &self,
        hash: Hash,
        nodes: &[EndpointId],
        declared: u64,
        deadline: Instant,
    ) -> Result<Vec<(EndpointId, Outcome)>> {
        let have = self.store.observe(hash).await?;
        let sources = nodes.len().min(self.max_sources);
        let mut todo = plan_parts(&missing(&have, declared), sources);
        // Nodes that sent their part, with the rate they sent it at, and
        // nodes not asked yet.
        let mut done: Vec<(EndpointId, Option<f64>)> = Vec::new();
        let mut unasked: VecDeque<EndpointId> = nodes.iter().copied().collect();
        let mut running: Vec<Arc<Part>> = Vec::new();
        let mut attempts = FuturesUnordered::new();
        let mut failed = Vec::new();
        loop {
            while Instant::now() < deadline {
                // The first few nodes not asked yet start the parts; after
                // that, one is asked only when no node is fetching.
                let asked = nodes.len() - unasked.len();
                let next = match done.last() {
                    Some(&(node, rate)) => Some((node, rate)),
                    None if asked < sources || attempts.is_empty() => {
                        unasked.front().map(|node| (*node, None))
                    }
                    None => None,
                };
                let Some((node, rate)) = next else { break };
                let Some(range) = todo.pop_front().or_else(|| split_slowest(&running, rate)) else {
                    break;
                };
                // Off the list it came from.
                if !done.is_empty() {
                    done.pop();
                } else {
                    unasked.pop_front();
                }
                let part = Arc::new(Part::new(range));
                running.push(part.clone());
                attempts.push(self.fetch_part(hash, node, part, deadline));
            }
            let next = if done.is_empty() {
                attempts.next().await
            } else {
                match tokio::time::timeout(RECHECK, attempts.next()).await {
                    Ok(next) => next,
                    // A part may be worth taking now.
                    Err(_) => continue,
                }
            };
            let Some((part, attempt)) = next else {
                break;
            };
            running.retain(|p| !Arc::ptr_eq(p, &part));
            self.record(hash, &attempt);
            if attempt.outcome == Outcome::Completed {
                done.push((attempt.node, attempt.rate()));
            } else {
                failed.push((attempt.node, attempt.outcome));
                let rest = part.reached()..part.end();
                if !rest.is_empty() {
                    todo.push_front(rest);
                }
            }
        }
        Ok(failed)
    }

    /// Asks `node` for `part` of a blob.
    async fn fetch_part(
        &self,
        hash: Hash,
        node: EndpointId,
        part: Arc<Part>,
        deadline: Instant,
    ) -> (Arc<Part>, Attempt) {
        let ranges = ChunkRanges::bytes(part.start..part.end());
        let request = GetRequest::blob_ranges(hash, ranges);
        let attempt = self
            .attempt(node, request, Stop::PastPart(&part), deadline)
            .await;
        (part, attempt)
    }

    /// Logs how an attempt ended, and adds it to the node's record (ADR 0011).
    fn record(&self, hash: Hash, attempt: &Attempt) {
        attempt.log(hash);
        peers::record(
            &self.db,
            &attempt.node,
            Event::Download {
                outcome: attempt.outcome,
                bytes: attempt.bytes,
                took: attempt.took,
                first_byte: attempt.first_byte,
            },
        );
    }

    /// Asks one node for `request`: part or all of what is still missing
    /// of a blob. It goes on until the node has sent all of it, or `stop`
    /// says enough has arrived, or the node sends nothing for
    /// `STALL_TIMEOUT`.
    async fn attempt(
        &self,
        node: EndpointId,
        request: GetRequest,
        stop: Stop<'_>,
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
                let mut stalls_at = Instant::now() + STALL_TIMEOUT;
                loop {
                    let mut wait = left().min(stalls_at.saturating_duration_since(Instant::now()));
                    if let Stop::PastPart(_) = stop {
                        wait = wait.min(RECHECK);
                    }
                    match tokio::time::timeout(wait, progress.next()).await {
                        Err(_) => {
                            // All of the part was taken from a node that
                            // went quiet: it sent its share, if any.
                            if let Stop::PastPart(part) = stop
                                && part.reached() >= part.end()
                            {
                                break if attempt.bytes > 0 {
                                    Outcome::Completed
                                } else {
                                    Outcome::HandedOn
                                };
                            }
                            // Stalled, or out of time.
                            if Instant::now() >= stalls_at || left().is_zero() {
                                break Outcome::Failed;
                            }
                        }
                        // It ended without saying how.
                        Ok(None) => break Outcome::Failed,
                        Ok(Some(GetProgressItem::Progress(bytes))) => {
                            attempt.first_byte.get_or_insert_with(|| started.elapsed());
                            attempt.bytes = bytes;
                            stalls_at = Instant::now() + STALL_TIMEOUT;
                            match stop {
                                Stop::PastDeclared { local, declared } => {
                                    if local + bytes > declared {
                                        break Outcome::Stopped;
                                    }
                                }
                                // The rest of the part is another node's
                                // now: this one has sent its share.
                                Stop::PastPart(part) => {
                                    if part.arrived(bytes) {
                                        break Outcome::Completed;
                                    }
                                }
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

/// The nodes a download may come from (`ContentStore::download`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Sources {
    /// Nodes that may each send part of a large blob at the same time, up
    /// to `MAX_SOURCES` at once, in this order: followers' copies, named
    /// for this download (doc 13).
    pub at_once: Vec<EndpointId>,
    /// Then, one at a time, for whatever is still missing: the nodes that
    /// everyone asks, such as the author's.
    pub in_turn: Vec<EndpointId>,
}

impl Sources {
    /// Nodes asked one at a time, in this order.
    pub fn one_at_a_time(nodes: Vec<EndpointId>) -> Self {
        Self {
            at_once: Vec::new(),
            in_turn: nodes,
        }
    }

    /// Takes `nodes` out of `at_once`, so they're only asked one at a time,
    /// after the others there: they're the ones to spare, such as the
    /// author's, which everyone asks.
    pub fn spare(&mut self, nodes: &[EndpointId]) {
        let (spared, at_once) = self.at_once.drain(..).partition(|id| nodes.contains(id));
        self.at_once = at_once;
        for id in spared {
            if !self.in_turn.contains(&id) {
                self.in_turn.push(id);
            }
        }
    }

    pub fn contains(&self, node: &EndpointId) -> bool {
        self.at_once.contains(node) || self.in_turn.contains(node)
    }

    pub fn is_empty(&self) -> bool {
        self.at_once.is_empty() && self.in_turn.is_empty()
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

/// How a download that devices wait on is going (see `Arriving`).
#[derive(Clone, Debug)]
pub(crate) enum Download {
    Running,
    Done,
    Failed(String),
}

/// A blob that may still be arriving: what the store has of it, kept up to
/// date while waiting for more.
pub(crate) struct Arriving {
    store: FsStore,
    hash: Hash,
    /// The store's news of what it has, whole each time.
    news: BoxStream<Bitfield>,
    have: Bitfield,
}

impl Arriving {
    /// Waits until the byte at `position` is here, then says where the
    /// bytes stored from there end. Fails if `download` is over first:
    /// with its error, or because the blob is shorter than its post says.
    pub async fn stored_from(
        &mut self,
        position: u64,
        download: &mut watch::Receiver<Download>,
    ) -> Result<u64> {
        loop {
            let end = stored_until(&self.have, position);
            if end > position {
                return Ok(end);
            }
            let state = download.borrow_and_update().clone();
            match state {
                Download::Running => tokio::select! {
                    news = self.news.next() => {
                        self.have = news.context("the blob store stopped saying what it has")?;
                    }
                    changed = download.changed() => {
                        changed.context("the download stopped without saying how")?;
                    }
                },
                Download::Done => {
                    // All of it is here, even if the news hasn't caught up.
                    self.have = self.store.observe(self.hash).await?;
                    let end = stored_until(&self.have, position);
                    ensure!(
                        end > position,
                        "{} ends before byte {position}",
                        to_blob_hash(self.hash).fmt_short()
                    );
                    return Ok(end);
                }
                Download::Failed(e) => bail!("{e}"),
            }
        }
    }
}

/// Where the bytes stored from `position` on end: `position` itself if the
/// byte there hasn't arrived.
fn stored_until(have: &Bitfield, position: u64) -> u64 {
    // Start, end, start, end... in 1 KiB chunks, with no last end when the
    // final range runs on past the end of the blob.
    for range in have.ranges.boundaries().chunks(2) {
        let start = range[0].to_bytes();
        let end = range.get(1).map_or(u64::MAX, |end| end.to_bytes());
        if start <= position && position < end {
            return end.min(have.size()).max(position);
        }
    }
    position
}

/// One node's share of a blob fetched in parts: the bytes from `start` to
/// its end. The end moves earlier when another node takes over the rest.
#[derive(Debug)]
struct Part {
    start: u64,
    /// Where the bytes the node has sent so far end.
    reached: AtomicU64,
    end: AtomicU64,
    /// When the node was asked, and when its first bytes came.
    asked: Instant,
    first_bytes: OnceLock<Instant>,
}

impl Part {
    fn new(range: Range<u64>) -> Self {
        Self {
            start: range.start,
            reached: AtomicU64::new(range.start),
            end: AtomicU64::new(range.end),
            asked: Instant::now(),
            first_bytes: OnceLock::new(),
        }
    }

    /// Bytes a second the node has sent the part at since its first bytes,
    /// or 0 if none came within `SLOW_START`. None while it's too soon to
    /// tell.
    fn rate(&self) -> Option<f64> {
        match self.first_bytes.get() {
            Some(first) => {
                let secs = first.elapsed().as_secs_f64();
                let sent = self.reached() - self.start;
                (secs >= RATE_AFTER.as_secs_f64()).then(|| sent as f64 / secs)
            }
            None => (self.asked.elapsed() >= SLOW_START).then_some(0.0),
        }
    }

    fn reached(&self) -> u64 {
        self.reached.load(Ordering::Relaxed)
    }

    fn end(&self) -> u64 {
        self.end.load(Ordering::Relaxed)
    }

    /// Bytes still to come.
    fn left(&self) -> u64 {
        self.end().saturating_sub(self.reached())
    }

    /// Notes that the node has sent `bytes` of the part. True once they
    /// run past its end. Stopping at the end itself could drop the last
    /// block: its bytes are counted before they're stored.
    fn arrived(&self, bytes: u64) -> bool {
        self.first_bytes.get_or_init(Instant::now);
        let reached = self.start + bytes;
        self.reached.store(reached, Ordering::Relaxed);
        reached > self.end()
    }
}

/// What of a blob's first `declared` bytes isn't here, in runs of whole
/// blocks, so the runs and the parts cut from them start on a block.
fn missing(have: &Bitfield, declared: u64) -> Vec<Range<u64>> {
    let gaps = &ChunkRanges::bytes(0..declared) - &have.ranges;
    let mut runs: Vec<Range<u64>> = Vec::new();
    // Start, end, start, end... in 1 KiB chunks, as in `stored_until`.
    for gap in gaps.boundaries().chunks(2) {
        let start = gap[0].to_bytes() / BLOCK * BLOCK;
        let end = gap
            .get(1)
            .map_or(declared, |end| end.to_bytes().div_ceil(BLOCK) * BLOCK)
            .min(declared);
        match runs.last_mut() {
            Some(last) if start <= last.end => last.end = last.end.max(end),
            _ => runs.push(start..end),
        }
    }
    runs
}

/// Cuts what is `missing` into parts for `sources` nodes, in the blob's
/// order: each about the same size, and at least `MIN_PART` unless a gap
/// is smaller.
fn plan_parts(missing: &[Range<u64>], sources: usize) -> VecDeque<Range<u64>> {
    let total: u64 = missing.iter().map(|gap| gap.end - gap.start).sum();
    let size = (total / sources.max(1) as u64).max(MIN_PART);
    let mut parts = VecDeque::new();
    for gap in missing {
        let len = gap.end - gap.start;
        let count = (len / size).max(1);
        let mut start = gap.start;
        for i in 1..count {
            let end = (gap.start + len * i / count) / BLOCK * BLOCK;
            parts.push_back(start..end);
            start = end;
        }
        parts.push_back(start..gap.end);
    }
    parts
}

/// For a node with nothing to do, which sent its last part at `rate` bytes
/// a second: takes over the end of a part from the node fetching it.
///
/// Once rates are known, that is the part that will take longest, split so
/// both nodes finish together: a node sending at a hundredth of the taker's
/// rate keeps a hundredth of the rest, and a stalled one none of it. Only
/// if that ends the part `SPLIT_GAIN` sooner. Until then, it is the back
/// half of the part with the most still to come, if that is at least two
/// `MIN_PART`s.
fn split_slowest(running: &[Arc<Part>], rate: Option<f64>) -> Option<Range<u64>> {
    let timed = running
        .iter()
        .filter(|part| part.left() > 0)
        .filter_map(|part| Some((part, part.rate()?)))
        .max_by(|(a, a_rate), (b, b_rate)| {
            // Seconds to go at its rate, endless for a stalled node.
            let secs = |part: &Part, rate: f64| {
                if rate > 0.0 {
                    part.left() as f64 / rate
                } else {
                    f64::INFINITY
                }
            };
            secs(a, *a_rate).total_cmp(&secs(b, *b_rate))
        });
    let (part, keep) = match (timed, rate) {
        (Some((part, own)), Some(taker)) if taker > 0.0 => {
            let left = part.left() as f64;
            let gain = left / own - left / (own + taker);
            if gain < SPLIT_GAIN.as_secs_f64() {
                return None;
            }
            (part, (left * own / (own + taker)) as u64)
        }
        _ => {
            let part = running.iter().max_by_key(|part| part.left())?;
            if part.left() < 2 * MIN_PART {
                return None;
            }
            (part, part.left() / 2)
        }
    };
    let (reached, end) = (part.reached(), part.end());
    // A node that keeps next to nothing stops at once; the taker may fetch
    // part of a block again.
    let middle = ((reached + keep) / BLOCK * BLOCK).max(part.start);
    if middle >= end {
        return None;
    }
    part.end.store(middle, Ordering::Relaxed);
    Some(middle..end)
}

/// When an attempt stops before the node has sent all it was asked for.
#[derive(Debug, Clone, Copy)]
enum Stop<'a> {
    /// Once more than the post declares has arrived, counting the `local`
    /// bytes that were here before.
    PastDeclared { local: u64, declared: u64 },
    /// Once bytes past the end of the part arrive.
    PastPart(&'a Part),
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
    /// Anything else: a dropped connection, an error, nothing sent for a
    /// while, or out of time.
    Failed,
    /// It hadn't sent anything when all of its part went to a faster node
    /// (`SLOW_START`): most often it was waiting for a place, as a busy
    /// node makes a request wait up to `BUSY_WAIT` before refusing.
    HandedOn,
    /// We stopped it: more arrived than the post says the blob has. That
    /// says nothing about the node, which can only send the blob the hash names.
    Stopped,
}

impl Attempt {
    /// Bytes a second, if any came.
    fn rate(&self) -> Option<f64> {
        let secs = self.took.as_secs_f64();
        (self.bytes > 0 && secs > 0.0).then(|| self.bytes as f64 / secs)
    }

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

/// Why a download failed when none of the nodes asked sent all of the
/// blob, busy or not: they didn't have it, failed, sent nothing for a
/// while, or time ran out. Asking again later may work.
#[derive(Debug)]
pub(crate) struct Unsent(&'static str);

impl fmt::Display for Unsent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Unsent {}

/// Whether a download failed because the nodes asked couldn't send the
/// blob just then (`Busy` or `Unsent`), not for a reason that will last,
/// such as a blob too big for the cache or for its post.
pub(crate) fn may_come_later(e: &anyhow::Error) -> bool {
    e.chain()
        .any(|cause| cause.is::<Busy>() || cause.is::<Unsent>())
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
/// small blob needs only a small place (`SMALL_BLOB`). A node's places are
/// kept for its next request for a blob it is fetching in parts
/// (`KEEP_PLACES`). With an upload cap, `pacer` spaces out what is sent.
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
        // it is small, the large blob it is for and whether it asks for
        // only part of it, and the channels for the answer and for news of
        // the transfer.
        let (peer, allowed, small, large, part, answer, updates) = match msg {
            ProviderMessage::GetRequestReceived(msg) => {
                let peer = peers.get(&msg.inner.connection_id).copied().flatten();
                let request = &msg.inner.request;
                let size = request
                    .ranges
                    .is_blob()
                    .then(|| may_serve(&db, request.hash, peer, share_prefetched))
                    .flatten();
                let small = size.is_some_and(|s| s <= SMALL_BLOB);
                let large = (!small).then_some(request.hash);
                let whole = matches!(request.ranges.as_single(), Some((0, r)) if r.is_all());
                (peer, size.is_some(), small, large, !whole, msg.tx, msg.rx)
            }
            ProviderMessage::GetManyRequestReceived(msg) => {
                let peer = peers.get(&msg.inner.connection_id).copied().flatten();
                let ok = msg
                    .inner
                    .request
                    .hashes
                    .iter()
                    .all(|h| may_serve(&db, *h, peer, share_prefetched).is_some());
                (peer, ok, false, None, false, msg.tx, msg.rx)
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
        let key = peer.zip(large);
        let needed = places.needed(peer, small, &db);
        let keep = places.kept.clone();
        let db = db.clone();
        tokio::spawn(async move {
            let held = match key {
                // Places kept for the node's transfer of another part of
                // the blob come first, if there are any or once there are:
                // that transfer may not have ended yet.
                Some(key) => tokio::select! {
                    biased;
                    held = keep.taken(key) => Some(held),
                    held = take_places(needed) => held,
                },
                None => take_places(needed).await,
            };
            let Some(held) = held else {
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
                // The node may ask for another part of the blob now.
                if part && let Some(key) = key {
                    keep.keep(key, held);
                }
            }
        });
    }
}

/// Places kept for a moment for nodes whose transfer of part of a blob
/// just ended (`KEEP_PLACES`).
#[derive(Clone, Default)]
struct KeptPlaces(Arc<Kept>);

/// Kept places by node and blob, each with a number that tells one keeping
/// from the next.
type KeptMap = HashMap<(EndpointId, Hash), (u64, Vec<OwnedSemaphorePermit>)>;

#[derive(Default)]
struct Kept {
    places: Mutex<KeptMap>,
    /// Woken whenever places are kept.
    new: Notify,
}

impl KeptPlaces {
    /// Keeps `held` for `key` for `KEEP_PLACES`, then gives them back.
    fn keep(&self, key: (EndpointId, Hash), held: Vec<OwnedSemaphorePermit>) {
        static KEPT: AtomicU64 = AtomicU64::new(0);
        let number = KEPT.fetch_add(1, Ordering::Relaxed);
        self.lock().insert(key, (number, held));
        self.0.new.notify_waiters();
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(KEEP_PLACES).await;
            let mut kept = this.lock();
            // Unless they were taken, and maybe kept again since.
            if kept.get(&key).is_some_and(|(n, _)| *n == number) {
                kept.remove(&key);
            }
        });
    }

    fn take(&self, key: &(EndpointId, Hash)) -> Option<Vec<OwnedSemaphorePermit>> {
        self.lock().remove(key).map(|(_, held)| held)
    }

    /// Waits until places are kept for `key`, and takes them.
    async fn taken(&self, key: (EndpointId, Hash)) -> Vec<OwnedSemaphorePermit> {
        loop {
            // Listening before looking, so places kept in between aren't missed.
            let new = self.0.new.notified();
            tokio::pin!(new);
            new.as_mut().enable();
            if let Some(held) = self.take(&key) {
                return held;
            }
            new.await;
        }
    }

    fn lock(&self) -> MutexGuard<'_, KeptMap> {
        self.0.places.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The transfer limits. Each limit is a semaphore with one permit per place.
struct Places {
    all: Arc<Semaphore>,
    strangers: Arc<Semaphore>,
    small: Arc<Semaphore>,
    per_node: HashMap<Option<EndpointId>, Arc<Semaphore>>,
    kept: KeptPlaces,
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
            kept: KeptPlaces::default(),
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
    use osp_proto::rpc::PeerCounts;

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

    #[test]
    fn stored_bytes_run_to_the_first_gap() {
        use iroh_blobs::protocol::{ChunkRanges, ChunkRangesExt};
        // A 10,000-byte blob with its first two and fifth and sixth chunks.
        let have = Bitfield::new(
            ChunkRanges::bytes(0..2 * KB) | ChunkRanges::bytes(4 * KB..6 * KB),
            10_000,
        );
        assert_eq!(stored_until(&have, 0), 2 * KB);
        assert_eq!(stored_until(&have, 2 * KB - 1), 2 * KB);
        assert_eq!(stored_until(&have, 2 * KB), 2 * KB);
        assert_eq!(stored_until(&have, 5000), 6 * KB);
        assert_eq!(stored_until(&have, 6 * KB), 6 * KB);
        // A run with the last chunk ends at the blob's end.
        let have = Bitfield::new(ChunkRanges::bytes(9 * KB..10_000), 10_000);
        assert_eq!(stored_until(&have, 9500), 10_000);
        assert_eq!(stored_until(&have, 10_000), 10_000);
        assert_eq!(stored_until(&Bitfield::complete(10_000), 3), 10_000);
        assert_eq!(stored_until(&Bitfield::empty(), 0), 0);
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
            self.open_with_quotas(1).await
        }

        /// Opens the store with quotas of `mb` each.
        async fn open_with_quotas(&self, mb: u64) -> ContentStore {
            let quotas = StorageConfig {
                replica_quota_mb: mb,
                cache_quota_mb: mb,
            };
            let blobs = self.dir.path().join("blobs");
            ContentStore::open(
                &blobs,
                &self.endpoint,
                self.db.clone(),
                &quotas,
                GC_SOON,
                MAX_SOURCES,
            )
            .await
            .unwrap()
        }

        /// What this node recorded about downloading from `node` (ADR 0011).
        fn record(&self, node: EndpointId) -> PeerCounts {
            let peers = self.db.peers(crate::db::now_ms()).unwrap();
            let row = peers.into_iter().find(|p| p.node == node);
            row.map(|p| p.counts).unwrap_or_default()
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
            self.provider_with(data, events).await
        }

        /// Starts a node that keeps `data` and sends it slowly: each block
        /// waits `delay` before it goes.
        async fn slow_provider(&self, data: &[u8], delay: Duration) -> (Provider, Hash) {
            let mask = EventMask {
                throttle: ThrottleMode::Intercept,
                ..EventMask::DEFAULT
            };
            let (events, mut rx) = EventSender::channel(8, mask);
            tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    if let ProviderMessage::Throttle(msg) = msg {
                        tokio::spawn(async move {
                            tokio::time::sleep(delay).await;
                            msg.tx.send(Ok(())).await.ok();
                        });
                    }
                }
            });
            self.provider_with(data, Some(events)).await
        }

        /// Like a busy node, holds each request for `wait` before it sends
        /// anything.
        async fn waiting_provider(&self, data: &[u8], wait: Duration) -> (Provider, Hash) {
            let mask = EventMask {
                get: RequestMode::Intercept,
                ..EventMask::DEFAULT
            };
            let (events, mut rx) = EventSender::channel(8, mask);
            tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    if let ProviderMessage::GetRequestReceived(msg) = msg {
                        tokio::spawn(async move {
                            tokio::time::sleep(wait).await;
                            msg.tx.send(Ok(())).await.ok();
                        });
                    }
                }
            });
            self.provider_with(data, Some(events)).await
        }

        async fn provider_with(
            &self,
            data: &[u8],
            events: Option<EventSender>,
        ) -> (Provider, Hash) {
            let media = MemStore::new();
            let hash = media.add_bytes(data.to_vec()).await.unwrap().hash;
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
            let stop = Stop::PastDeclared { local: 0, declared };
            store
                .attempt(node, GetRequest::blob(hash), stop, soon)
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
        let in_turn = |nodes| Sources::one_at_a_time(nodes);
        let providers = vec![busy.id(), liar.id(), nowhere, honest.id()];
        let size = store
            .download(&blob, in_turn(providers), BlobClass::Cache, KB)
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
                in_turn(vec![busy.id(), liar.id()]),
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
            .download(
                &other_blob,
                in_turn(vec![liar.id(), nowhere]),
                BlobClass::Cache,
                KB,
            )
            .await
            .unwrap_err();
        assert!(!is_busy(&err), "{err:#}");

        // Bigger than its post says: no other node can send a smaller one.
        let providers = in_turn(vec![other_provider.id(), honest.id()]);
        let err = store
            .fetch(
                other,
                &providers,
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
        let providers = in_turn(vec![late.id(), late.id()]);
        let err = store
            .fetch(unseen, &providers, KB, Instant::now())
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("timed out"), "{err:#}");
    }

    /// `MB` bytes that are different in every block, so a part sent from
    /// the wrong place can't pass for the right one.
    fn noise(mb: u64) -> Vec<u8> {
        (0..mb << 20)
            .map(|i: u64| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect()
    }

    #[test]
    fn parts_are_cut_in_whole_blocks() {
        use iroh_blobs::protocol::ChunkRanges;
        const MB: u64 = 1 << 20;
        // Nothing here: three parts of about a third each, ending at the
        // size the post declares.
        let size = 10 * MB + 500;
        let gaps = missing(&Bitfield::empty(), size);
        assert_eq!((gaps.len(), &gaps[0]), (1, &(0..size)));
        let parts = plan_parts(&gaps, 3);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].start, 0);
        assert_eq!(parts[2].end, size);
        for pair in parts.iter().collect::<Vec<_>>().windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
            assert_eq!(pair[1].start % BLOCK, 0);
        }
        // Parts are at least 1 MiB: 2.5 MB makes only two.
        let parts_of = |len| plan_parts(std::slice::from_ref(&(0..len)), 3).len();
        assert_eq!(parts_of(5 * MB / 2), 2);
        assert_eq!(parts_of(MB), 1);

        // What is here is left out, and the gaps widen to whole blocks.
        let have = Bitfield::new(
            ChunkRanges::bytes(0..3 * MB + 4 * KB) | ChunkRanges::bytes(3 * MB + 20 * KB..8 * MB),
            size,
        );
        assert_eq!(
            missing(&have, size),
            [3 * MB..3 * MB + 32 * KB, 8 * MB..size]
        );
        // A small gap stays one part, before the parts of a large one.
        let parts = plan_parts(&missing(&have, size), 3);
        assert_eq!(parts[0], 3 * MB..3 * MB + 32 * KB);
        assert_eq!(parts.len(), 3);
        assert!(missing(&Bitfield::complete(size), size).is_empty());
    }

    #[test]
    fn before_rates_are_known_a_node_takes_half_of_the_largest_part() {
        const MB: u64 = 1 << 20;
        let slow = Arc::new(Part::new(0..10 * MB));
        let fast = Arc::new(Part::new(10 * MB..12 * MB));
        assert!(!slow.arrived(MB));
        assert_eq!(slow.rate(), None, "too soon to tell");
        let running = [slow.clone(), fast.clone()];
        // 9 MB left of one part: the back half goes.
        let taken = split_slowest(&running, Some(1e9)).unwrap();
        assert_eq!(taken.start % BLOCK, 0);
        assert_eq!(taken.end, 10 * MB);
        assert_eq!(slow.end(), taken.start);
        assert!(taken.start > MB + 4 * MB && taken.start <= MB + 9 * MB / 2);
        // The node stops once its bytes run past the new end.
        assert!(!slow.arrived(taken.start - MB));
        assert!(!slow.arrived(taken.start));
        assert!(slow.arrived(taken.start + BLOCK));
        // Under 2 MiB left anywhere isn't worth splitting.
        assert!(!fast.arrived(MB / 2));
        assert_eq!(split_slowest(&[fast], None), None);
        assert_eq!(split_slowest(&[], None), None);
    }

    /// A part whose node was asked `secs` ago and has sent `sent` bytes of
    /// it, at an even rate from the start.
    fn part_sending(range: Range<u64>, sent: u64, secs: u64) -> Arc<Part> {
        let ago = Instant::now() - Duration::from_secs(secs);
        let part = Part {
            asked: ago,
            ..Part::new(range)
        };
        if sent > 0 {
            part.first_bytes.set(ago).unwrap();
            assert!(!part.arrived(sent));
        }
        Arc::new(part)
    }

    #[test]
    fn a_node_takes_the_slowest_part_by_the_rates() {
        const MB: u64 = 1 << 20;
        // 1 MB in 8 s, 9 MB to go: 72 s more. 2 MB to go at 8 MB/s.
        let slow = part_sending(0..10 * MB, MB, 8);
        let fast = part_sending(10 * MB..20 * MB, 8 * MB, 1);
        let running = [fast.clone(), slow.clone()];
        // A taker at 10 MB/s takes all but about 1/77 of the slow rest.
        let taken = split_slowest(&running, Some(10.0 * MB as f64)).unwrap();
        assert_eq!(taken.end, 10 * MB);
        assert_eq!(slow.end(), taken.start);
        assert_eq!(taken.start % BLOCK, 0);
        assert!(
            taken.start > MB + 96 * 1024 && taken.start <= MB + 128 * 1024,
            "{taken:?}"
        );
        assert_eq!(fast.end(), 20 * MB, "untouched");

        // Equal fast nodes with little to go: not worth a new request.
        let fast = part_sending(0..10 * MB, 8 * MB, 1);
        assert_eq!(split_slowest(&[fast], Some(8.0 * MB as f64)), None);
    }

    #[test]
    fn a_stalled_nodes_part_is_all_taken() {
        const MB: u64 = 1 << 20;
        let stalled = part_sending(4 * MB..8 * MB, 0, 3);
        assert_eq!(stalled.rate(), Some(0.0));
        let taken = split_slowest(std::slice::from_ref(&stalled), Some(1e6)).unwrap();
        assert_eq!(taken, 4 * MB..8 * MB);
        assert!(stalled.reached() >= stalled.end(), "nothing left to it");
        // Emptied, it isn't picked again over a part with something to go.
        let slow = part_sending(0..4 * MB, MB, 8);
        let taken = split_slowest(&[stalled.clone(), slow], Some(1e7)).unwrap();
        assert_eq!(taken.end, 4 * MB);
        // Asked a moment ago, it may just be starting.
        let starting = part_sending(4 * MB..8 * MB, 0, 0);
        assert_eq!(starting.rate(), None);
    }

    #[tokio::test]
    async fn a_large_blob_comes_in_parts_from_several_nodes_at_once() {
        let f = Fixture::new().await;
        let store = f.open_with_quotas(64).await;
        let data = noise(6);
        let size = data.len() as u64;
        let mut nodes = Vec::new();
        let mut providers = Vec::new();
        for _ in 0..4 {
            let (provider, hash) = f.provider(&data, None).await;
            nodes.push(provider.id());
            providers.push((provider, hash));
        }
        let blob = to_blob_hash(providers[0].1);
        let sources = Sources {
            at_once: nodes.clone(),
            in_turn: Vec::new(),
        };
        assert_eq!(
            store
                .download(&blob, sources, BlobClass::Replica, size)
                .await
                .unwrap(),
            size
        );
        // Three at once, each sending a share; the fourth wasn't needed.
        let mut received = 0.0;
        for node in &nodes[..3] {
            let counts = f.record(*node);
            assert!(counts.completed.round() >= 1.0, "{counts:?}");
            assert!(
                counts.received_bytes.round() >= MIN_PART as f64,
                "{counts:?}"
            );
            received += counts.received_bytes;
        }
        assert!(received < (size + 3 * BLOCK) as f64, "{received} bytes");
        assert_eq!(f.record(nodes[3]).tries, 0.0);
        // Every byte is the one the hash names.
        let out = f.dir.path().join("out");
        store.export(&blob, &out).await.unwrap();
        assert!(std::fs::read(&out).unwrap() == data);

        // A blob under two parts comes from one node.
        let small = noise(1);
        let (first, hash) = f.provider(&small, None).await;
        let (second, _) = f.provider(&small, None).await;
        let sources = Sources {
            at_once: vec![first.id(), second.id()],
            in_turn: Vec::new(),
        };
        store
            .download(
                &to_blob_hash(hash),
                sources,
                BlobClass::Cache,
                2 * MIN_PART - 1,
            )
            .await
            .unwrap();
        assert_eq!(f.record(first.id()).completed.round(), 1.0);
        assert_eq!(f.record(second.id()).tries, 0.0);
    }

    #[tokio::test]
    async fn the_part_a_node_cant_send_goes_to_another() {
        let f = Fixture::new().await;
        let store = f.open_with_quotas(64).await;
        let data = noise(4);
        let size = data.len() as u64;
        let (busy, _) = f.provider(&data, Some(AbortReason::RateLimited)).await;
        let (refusing, _) = f.provider(&data, Some(AbortReason::Permission)).await;
        let (honest, hash) = f.provider(&data, None).await;
        let (spare, _) = f.provider(&data, None).await;
        let blob = to_blob_hash(hash);
        let nowhere = SecretKey::generate().public();
        let sources = Sources {
            at_once: vec![busy.id(), nowhere, refusing.id(), honest.id()],
            in_turn: vec![spare.id()],
        };
        store
            .download(&blob, sources, BlobClass::Cache, size)
            .await
            .unwrap();
        // The honest node sent the parts of the three that couldn't, each
        // as soon as it was free.
        let counts = f.record(honest.id());
        assert!(counts.received_bytes.round() >= size as f64, "{counts:?}");
        assert_eq!(counts.completed.round(), 3.0, "{counts:?}");
        assert_eq!(f.record(busy.id()).busy.round(), 1.0);
        assert_eq!(f.record(refusing.id()).not_hosting.round(), 1.0);
        assert_eq!(f.record(nowhere).unreached.round(), 1.0);
        // Each was asked once, and the node to spare not at all.
        assert_eq!(f.record(spare.id()).tries, 0.0);
        assert_eq!(f.record(busy.id()).tries.round(), 1.0);

        // When none of them can, the nodes asked in turn are, after them,
        // each for all that's missing; the busy node isn't asked again.
        let other = noise(3);
        let (spare, hash) = f.provider(&other, None).await;
        let (busy, _) = f.provider(&other, Some(AbortReason::RateLimited)).await;
        let liar = f.serve(Fake::liar()).await;
        let sources = Sources {
            at_once: vec![busy.id(), liar.id()],
            in_turn: vec![spare.id()],
        };
        let blob = to_blob_hash(hash);
        store
            .download(&blob, sources, BlobClass::Cache, other.len() as u64)
            .await
            .unwrap();
        let counts = f.record(spare.id());
        assert!(counts.received_bytes.round() >= other.len() as f64);
        assert_eq!(counts.tries.round(), 1.0, "{counts:?}");
        assert_eq!(f.record(busy.id()).tries.round(), 1.0);
    }

    #[tokio::test]
    async fn a_part_refused_as_busy_waits_for_a_node_that_sent_its_own() {
        let f = Fixture::new().await;
        let store = f.open_with_quotas(64).await;
        let data = noise(6);
        let size = data.len() as u64;
        let (busy, hash) = f.provider(&data, Some(AbortReason::RateLimited)).await;
        // Slow enough that the busy node refuses while these two send.
        let (first, _) = f.slow_provider(&data, Duration::from_millis(2)).await;
        let (second, _) = f.slow_provider(&data, Duration::from_millis(2)).await;
        let (unasked, _) = f.provider(&data, None).await;
        let sources = Sources {
            at_once: vec![busy.id(), first.id(), second.id(), unasked.id()],
            in_turn: Vec::new(),
        };
        store
            .download(&to_blob_hash(hash), sources, BlobClass::Cache, size)
            .await
            .unwrap();
        // The busy node's part went to one of the other two once it had
        // sent its own, not to a node not asked yet, which could be busy too.
        assert_eq!(f.record(busy.id()).busy.round(), 1.0);
        assert_eq!(f.record(unasked.id()).tries, 0.0);
        let completed = f.record(first.id()).completed + f.record(second.id()).completed;
        assert!(completed.round() >= 3.0, "{completed}");
    }

    #[tokio::test]
    async fn a_node_that_finishes_its_part_takes_over_most_of_a_slow_one() {
        let f = Fixture::new().await;
        let store = f.open_with_quotas(64).await;
        let data = noise(12);
        let size = data.len() as u64;
        // 16 KiB every 40 ms: its 4 MiB part would take 10 seconds, half of
        // it 5.
        let (slow, hash) = f.slow_provider(&data, Duration::from_millis(40)).await;
        let (fast, _) = f.provider(&data, None).await;
        let (also_fast, _) = f.provider(&data, None).await;
        let (unasked, _) = f.provider(&data, None).await;
        let blob = to_blob_hash(hash);
        let sources = Sources {
            at_once: vec![slow.id(), fast.id(), also_fast.id(), unasked.id()],
            in_turn: Vec::new(),
        };
        let started = Instant::now();
        store
            .download(&blob, sources, BlobClass::Cache, size)
            .await
            .unwrap();
        let took = started.elapsed();
        // The slow node sent a small share of its part, and stopped once
        // its rate showed how slow it was.
        let counts = f.record(slow.id());
        assert!(counts.completed.round() >= 1.0, "{counts:?}");
        assert!(counts.received_bytes < (1 << 20) as f64, "{counts:?}");
        // Half its part would have taken it over 5 s more.
        assert!(took < Duration::from_secs(4), "{took:?}");
        // The rest went to nodes that had just sent their parts, not to one
        // not asked yet.
        let completed = f.record(fast.id()).completed + f.record(also_fast.id()).completed;
        assert!(completed.round() >= 3.0, "{completed}");
        assert_eq!(f.record(unasked.id()).tries, 0.0);
        let out = f.dir.path().join("out");
        store.export(&blob, &out).await.unwrap();
        assert!(std::fs::read(&out).unwrap() == data);
    }

    #[tokio::test]
    async fn a_node_that_goes_quiet_has_its_whole_part_taken() {
        let f = Fixture::new().await;
        let store = f.open_with_quotas(64).await;
        let data = noise(12);
        let size = data.len() as u64;
        // After its first block, the next would come a minute later.
        let (silent, hash) = f.slow_provider(&data, Duration::from_secs(60)).await;
        let (fast, _) = f.provider(&data, None).await;
        let (also_fast, _) = f.provider(&data, None).await;
        let blob = to_blob_hash(hash);
        let sources = Sources {
            at_once: vec![silent.id(), fast.id(), also_fast.id()],
            in_turn: Vec::new(),
        };
        let started = Instant::now();
        store
            .download(&blob, sources, BlobClass::Cache, size)
            .await
            .unwrap();
        // Once its rate showed, all the rest of its part went to the
        // others, and it wasn't waited for until it stalled (30 s).
        let took = started.elapsed();
        assert!(took < Duration::from_secs(10), "{took:?}");
        let counts = f.record(silent.id());
        assert!(counts.received_bytes <= BLOCK as f64, "{counts:?}");
        assert_eq!(counts.completed.round(), 1.0, "{counts:?}");
        let out = f.dir.path().join("out");
        store.export(&blob, &out).await.unwrap();
        assert!(std::fs::read(&out).unwrap() == data);
    }

    #[tokio::test]
    async fn a_node_still_waiting_to_send_is_handed_on_not_failed() {
        let f = Fixture::new().await;
        let store = f.open_with_quotas(64).await;
        let data = noise(12);
        let size = data.len() as u64;
        let (waiting, hash) = f.waiting_provider(&data, Duration::from_secs(60)).await;
        let (fast, _) = f.provider(&data, None).await;
        let (also_fast, _) = f.provider(&data, None).await;
        let blob = to_blob_hash(hash);
        let sources = Sources {
            at_once: vec![waiting.id(), fast.id(), also_fast.id()],
            in_turn: Vec::new(),
        };
        let started = Instant::now();
        store
            .download(&blob, sources, BlobClass::Cache, size)
            .await
            .unwrap();
        // After `SLOW_START` with nothing sent, the rest of its part went
        // to the others; its record says so, not that it failed.
        let took = started.elapsed();
        assert!(took < Duration::from_secs(10), "{took:?}");
        let counts = f.record(waiting.id());
        assert_eq!(counts.handed_on.round(), 1.0, "{counts:?}");
        assert_eq!(counts.failed, 0.0, "{counts:?}");
        assert_eq!(counts.received_bytes, 0.0, "{counts:?}");
        let out = f.dir.path().join("out");
        store.export(&blob, &out).await.unwrap();
        assert!(std::fs::read(&out).unwrap() == data);
    }

    #[tokio::test]
    async fn parts_stop_at_the_size_the_post_declares() {
        let f = Fixture::new().await;
        let store = f.open_with_quotas(64).await;
        let data = noise(3);
        let size = data.len() as u64;
        let (a, hash) = f.provider(&data, None).await;
        let (b, _) = f.provider(&data, None).await;
        let blob = to_blob_hash(hash);
        let both = || Sources {
            at_once: vec![a.id(), b.id()],
            in_turn: Vec::new(),
        };
        // Bigger than its post says: nothing past the declared size comes.
        let err = store
            .download(&blob, both(), BlobClass::Cache, size - 600 * KB)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("bigger than its post says"),
            "{err:#}"
        );
        let received = f.record(a.id()).received_bytes + f.record(b.id()).received_bytes;
        assert!(received <= (size - 500 * KB) as f64, "{received} bytes");

        // Smaller than it says: what there is comes, and that's all of it.
        let (c, other) = f.provider(&data[..2 << 20], None).await;
        let (d, _) = f.provider(&data[..2 << 20], None).await;
        let sources = Sources {
            at_once: vec![c.id(), d.id()],
            in_turn: Vec::new(),
        };
        let got = store
            .download(&to_blob_hash(other), sources, BlobClass::Cache, size)
            .await
            .unwrap();
        assert_eq!(got, 2 << 20);
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
