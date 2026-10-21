//! Content-addressed blob storage and transfer.
//!
//! This is the only module that uses iroh-blobs (ADR 0002). It is still
//! pre-1.0, so keeping it behind this small interface means an API change, or
//! a replacement, touches one file.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use iroh::{Endpoint, EndpointId};
use iroh_blobs::{
    BlobsProtocol, Hash,
    api::{blobs::BlobStatus, downloader::Downloader},
    provider::events::{
        AbortReason, ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage,
        RequestMode, ThrottleMode,
    },
    store::fs::FsStore,
};
use osp_proto::BlobHash;
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{debug, warn};

use crate::db::{BlobClass, Db};

/// Give up on a download after this long, so a stalled provider can't hang a sync.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

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
}

impl ContentStore {
    pub async fn open(dir: &Path, endpoint: &Endpoint, db: Arc<Db>) -> Result<Self> {
        let store = FsStore::load(dir)
            .await
            .with_context(|| format!("opening blob store in {}", dir.display()))?;
        let downloader = Downloader::new(&store, endpoint);
        Ok(Self {
            store,
            downloader,
            db,
        })
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

    /// Downloads a blob, trying `providers` in order. Every chunk is verified
    /// against the hash before it is stored.
    pub async fn download(
        &self,
        hash: &BlobHash,
        providers: Vec<EndpointId>,
        class: BlobClass,
    ) -> Result<u64> {
        let h = to_hash(hash);
        if !self.store.has(h).await? {
            anyhow::ensure!(!providers.is_empty(), "no known providers for {hash}");
            debug!(%hash, ?providers, "downloading");
            tokio::time::timeout(DOWNLOAD_TIMEOUT, self.downloader.download(h, providers))
                .await
                .context("download timed out")?
                .map_err(|e| anyhow::anyhow!("download of {} failed: {e}", hash.fmt_short()))?;
        }
        let size = self
            .size(h)
            .await?
            .context("downloaded blob is incomplete")?;
        self.keep(h, class, size).await?;
        Ok(size)
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

    async fn size(&self, hash: Hash) -> Result<Option<u64>> {
        Ok(match self.store.status(hash).await? {
            BlobStatus::Complete { size } => Some(size),
            _ => None,
        })
    }

    /// Protects the blob with a named tag (so a future garbage collector keeps
    /// it) and records its class.
    async fn keep(&self, hash: Hash, class: BlobClass, size: u64) -> Result<()> {
        self.store
            .tags()
            .set(format!("osp/{}", hash.to_hex()), hash)
            .await?;
        self.db.put_blob(&to_blob_hash(hash), class, size)
    }
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
