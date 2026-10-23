//! Wiring: one iroh endpoint, the protocols on it, and the background tasks.

use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, PublicKey, RelayMode, SecretKey,
    address_lookup::{DnsAddressLookup, MemoryLookup, PkarrPublisher, PkarrResolver},
    endpoint::{Connection, QuicTransportConfig, presets},
    protocol::Router,
};
use osp_proto::{
    BlobHash, Payload, Person, SignedObject,
    rpc::{BackupInfo, ClientRequest, ClientResponse},
};
use tokio::{
    sync::{Notify, Semaphore, watch},
    task::JoinHandle,
};
use tracing::{info, warn};

/// Most streams a peer may have open on one connection at a time. A phone
/// loading a feed full of images uses the most; extra streams wait their turn.
const MAX_STREAMS_PER_CONN: u32 = 32;

/// Most relays and IP addresses accepted in one node address (see `check_addr`).
const MAX_ADDRS: usize = 16;

use crate::{
    backup,
    client::{ClientProtocol, PendingPairing},
    config::{Config, RelayChoice},
    control::{self, Origin},
    db::{BlobClass, Db},
    home::Home,
    keys::{self, Signer},
    media,
    network::{self, ConnTracker},
    relays::{self, Checked, RelayPolicy},
    social::SocialProtocol,
    store::{self, ContentStore},
    sync,
};

/// A running home node.
pub struct Node {
    core: Arc<Core>,
    router: Router,
    tasks: Vec<JoinHandle<()>>,
    serving_socket: bool,
    /// Locked for as long as the node runs; the lock goes when the file closes.
    _lock: std::fs::File,
}

/// State shared by the protocol handlers and background tasks.
pub(crate) struct Core {
    pub home: Home,
    pub config: Config,
    /// What our objects are signed with (ADR 0010).
    pub signer: Signer,
    pub endpoint: Endpoint,
    /// Addresses we learned out of band (tickets, subscriptions, WhoHosts),
    /// so peers can be dialled by ID without a lookup service.
    pub lookup: MemoryLookup,
    /// Which relays in other people's addresses we may use.
    pub relays: RelayPolicy,
    pub db: Arc<Db>,
    pub store: ContentStore,
    /// Our signed profile, served to anyone who asks.
    pub profile: SignedObject,
    /// Open `osp/social/1` connections, reused across requests.
    pub conns: Mutex<HashMap<EndpointId, Connection>>,
    /// Every live connection in either direction, for `status`.
    pub tracker: ConnTracker,
    /// Only one sync runs at a time.
    pub sync_lock: tokio::sync::Mutex<()>,
    /// Wakes the sync loop early (on a push notification or a new follow).
    pub sync_now: Notify,
    /// Number of prefetch jobs still downloading, so `Sync` can wait for them.
    pub prefetch_jobs: watch::Sender<usize>,
    /// Pairing secrets handed out and not used yet. Kept in memory only.
    pub pairings: Mutex<Vec<PendingPairing>>,
    /// When we last asked each followed identity's node to notify us.
    pub subscribed: Mutex<HashMap<PublicKey, Instant>>,
    /// Followers are told we moved by one task at a time.
    pub tell_moved_lock: tokio::sync::Mutex<()>,
    pub started: Instant,
    /// Whether ffmpeg ran at startup; without it, posts with media fail.
    pub media_tools: bool,
    /// Places for posts with media in progress (`control::MAX_MEDIA_POSTS`).
    pub media_posts: Semaphore,
    /// Only one transcode runs at a time, so many posts at once can't start
    /// many ffmpegs. Posts waiting for it take their turn in order.
    pub transcode_lock: tokio::sync::Mutex<()>,
}

impl Node {
    /// Creates a node home for a new identity: keys, config and database.
    /// Returns the identity's recovery words, the only copy of its key
    /// (ADR 0010).
    pub fn init(home: &Path, config: &Config) -> Result<Vec<&'static str>> {
        let words = backup::new_identity_words();
        Self::init_with_words(home, config, &words.join(" "))?;
        Ok(words)
    }

    /// Creates a node home for the new identity that `words` stand for
    /// (`backup::new_identity_words`), once its owner has written them down.
    pub fn init_with_words(home: &Path, config: &Config, words: &str) -> Result<()> {
        create_home(
            &Home::new(home),
            config,
            &backup::identity_from_words(words)?,
        )
    }

    /// Creates a node home for an existing identity, given its recovery
    /// words and optionally a backup file (ADR 0009). The node gets a new
    /// node key, so the network sees a new node speaking for the same
    /// identity; when it starts, it tells the followers in the backup.
    ///
    /// An empty `config.name` takes the name from the backed-up profile.
    /// If restoring the file fails, what was created is removed again.
    pub fn restore(
        home: &Path,
        config: &Config,
        words: &str,
        backup_file: Option<&Path>,
    ) -> Result<Restored> {
        let identity = backup::identity_from_words(words)?;
        let home = Home::new(home);
        // Check the file before creating anything.
        let file = backup_file
            .map(|path| backup::BackupFile::open(path, &identity))
            .transpose()?;
        let mut config = config.clone();
        if config.name.is_empty() {
            config.name = file
                .as_ref()
                .and_then(|f| f.name())
                .unwrap_or_else(|| Config::default().name);
        }
        let existed = home.root().exists();
        create_home(&home, &config, &identity)?;
        let from_file = match file.map(|f| f.restore(&home, &identity)).transpose() {
            Ok(info) => info,
            Err(e) => {
                remove_new_home(&home, existed);
                return Err(e);
            }
        };
        Ok(Restored {
            identity: identity.public(),
            from_file,
        })
    }

    /// Takes the identity key off a home made before ADR 0010: makes a
    /// signing key and its certificate, then deletes `identity.key`.
    /// `confirm` gets the recovery words first, to show them and check they
    /// were written down; if it fails, nothing changes. The node must not be
    /// running, and can't start until this is done. When it next starts, it
    /// signs its profile with the new key.
    pub fn protect_keys(
        home: &Path,
        confirm: impl FnOnce(&[&'static str]) -> Result<()>,
    ) -> Result<()> {
        let home = Home::new(home);
        let _lock = lock_home(&home)?;
        ensure!(
            home.identity_key().exists(),
            "this node doesn't keep its identity key; there's nothing to protect"
        );
        confirm(&backup::recovery_words(home.root())?)?;
        keys::protect(&home)
    }

    /// Starts the node stored in `home`.
    pub async fn spawn(home: &Path) -> Result<Self> {
        let home = Home::new(home);
        if !home.is_initialized() {
            bail!("no node in {}; run `osp init` first", home.root().display());
        }
        let lock = lock_home(&home)?;
        let config = Config::load(&home.config())?;
        check_network_config(&config)?;
        let signer = Signer::load(&home)?;
        let node_key = keys::load(&home.node_key())?;
        let db = Arc::new(Db::open(&home.db())?);
        std::fs::create_dir_all(home.work())?;

        let lookup = MemoryLookup::new();
        let relays = RelayPolicy::from_config(&config.network)?;
        let tracker = ConnTracker::new(relays.clone());
        let endpoint = bind_endpoint(&config, node_key, &lookup, &relays, tracker.clone()).await?;
        for follow in db.follows()? {
            remember(&lookup, &follow.addr);
        }
        for addr in db.followers()? {
            remember(&lookup, &addr);
        }

        let store = ContentStore::open(
            &home.blobs(),
            &endpoint,
            db.clone(),
            &config.storage,
            store::GC_INTERVAL,
        )
        .await?;
        import_restored_media(&home, &store).await?;
        let profile = own_profile(&signer, &config.name, endpoint.id(), &db)?;
        let media_tools = media::tools_available(&config.media).await;
        if matches!(signer, Signer::Identity(_)) {
            warn!("this node keeps your identity key; run `osp keys protect` (ADR 0010)");
        }
        info!(
            identity = %signer.identity().fmt_short(),
            node = %endpoint.id().fmt_short(),
            "node started"
        );

        let core = Arc::new(Core {
            home,
            config,
            signer,
            endpoint: endpoint.clone(),
            lookup,
            relays,
            db,
            store,
            profile,
            conns: Mutex::new(HashMap::new()),
            tracker,
            sync_lock: tokio::sync::Mutex::new(()),
            sync_now: Notify::new(),
            prefetch_jobs: watch::Sender::new(0),
            pairings: Mutex::new(Vec::new()),
            subscribed: Mutex::new(HashMap::new()),
            tell_moved_lock: tokio::sync::Mutex::new(()),
            started: Instant::now(),
            media_tools,
            media_posts: Semaphore::new(control::MAX_MEDIA_POSTS),
            transcode_lock: tokio::sync::Mutex::new(()),
        });

        let (blobs, serving_rules) = core.store.protocol(&core.config.serving);
        let router = Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, blobs)
            .accept(osp_proto::SOCIAL_ALPN, SocialProtocol::new(core.clone()))
            .accept(osp_proto::CLIENT_ALPN, ClientProtocol::new(core.clone()))
            .spawn();
        let sync_loop = tokio::spawn(sync::run_loop(core.clone()));
        let tell_moved = tokio::spawn(control::tell_moved_loop(core.clone()));
        let replicas = {
            let core = core.clone();
            tokio::spawn(async move { core.refetch_replicas().await })
        };
        let mut tasks = vec![serving_rules, sync_loop, tell_moved, replicas];
        if core.config.network.relay != RelayChoice::Disabled {
            tasks.push(tokio::spawn(network::log_nat_changes(
                core.endpoint.clone(),
            )));
        }

        Ok(Self {
            core,
            router,
            tasks,
            serving_socket: false,
            _lock: lock,
        })
    }

    /// Accepts commands from the local `osp` tool on the home's Unix socket.
    pub async fn serve_local_socket(&mut self) -> Result<()> {
        let task = control::serve_unix(self.core.clone(), self.core.home.socket()).await?;
        self.tasks.push(task);
        self.serving_socket = true;
        Ok(())
    }

    /// Runs a command as if it came from the local socket.
    pub async fn handle(&self, req: ClientRequest) -> ClientResponse {
        self.core.handle_client(req, Origin::Local).await
    }

    pub fn identity(&self) -> PublicKey {
        self.core.signer.identity()
    }

    pub fn node_id(&self) -> EndpointId {
        self.core.endpoint.id()
    }

    /// The address other nodes should use to reach this one.
    pub async fn addr(&self) -> EndpointAddr {
        self.core.shareable_addr().await
    }

    pub fn blob_class(&self, hash: &BlobHash) -> Result<Option<BlobClass>> {
        self.core.db.blob_class(hash)
    }

    /// Downloads a blob of at most `size` bytes from specific nodes into
    /// the cache. For tests and diagnostics.
    pub async fn download_blob_from(
        &self,
        hash: &BlobHash,
        size: u64,
        providers: &[EndpointAddr],
    ) -> Result<u64> {
        for addr in providers {
            remember(&self.core.lookup, addr);
        }
        let ids = providers.iter().map(|a| a.id).collect();
        self.core
            .store
            .download(hash, ids, BlobClass::Cache, size)
            .await
    }

    /// Promises `bytes` of disk space, as an upload in progress does, until
    /// the returned guard is dropped. For tests: it fills the disk as far
    /// as the node can tell, without writing anything.
    pub async fn reserve_disk_space(&self, bytes: u64) -> Result<store::Reservation<'_>> {
        self.core.store.reserve_upload(bytes).await
    }

    /// Stops background work, closes connections and flushes the blob store.
    pub async fn shutdown(self) -> Result<()> {
        for task in &self.tasks {
            task.abort();
        }
        self.router
            .shutdown()
            .await
            .context("shutting down protocols")?;
        if self.serving_socket {
            let _ = std::fs::remove_file(self.core.home.socket());
        }
        Ok(())
    }
}

impl Core {
    /// Our address as we hand it out. With relays we share only the node ID
    /// and relay URL, so tickets don't reveal home IP addresses; without
    /// relays, direct addresses are all there is.
    pub async fn shareable_addr(&self) -> EndpointAddr {
        let addr = self.full_addr().await;
        if self.config.network.relay == RelayChoice::Disabled {
            return addr;
        }
        EndpointAddr::from_parts(addr.id, addr.addrs.into_iter().filter(|a| a.is_relay()))
    }

    /// Every address we have, relay and direct. Only for our own devices.
    pub async fn full_addr(&self) -> EndpointAddr {
        if self.config.network.relay != RelayChoice::Disabled {
            // Wait briefly for a home relay; without a network we hand out what we have.
            let _ = tokio::time::timeout(Duration::from_secs(10), self.endpoint.online()).await;
        }
        self.endpoint.addr()
    }
}

/// What `Node::restore` brought back.
#[derive(Debug, Clone)]
pub struct Restored {
    pub identity: PublicKey,
    /// What came from the backup file, if one was given.
    pub from_file: Option<BackupInfo>,
}

/// Undoes a restore that failed part way, so it can simply be run again.
/// Removes only what the restore created.
fn remove_new_home(home: &Home, existed: bool) {
    let db = home.db();
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    let shm = PathBuf::from(format!("{}-shm", db.display()));
    for file in [
        home.signing_key(),
        home.signing_cert(),
        home.backup_key(),
        home.node_key(),
        home.config(),
        db,
        wal,
        shm,
    ] {
        let _ = std::fs::remove_file(file);
    }
    let _ = std::fs::remove_dir_all(home.restored_media());
    if !existed {
        let _ = std::fs::remove_dir(home.root());
    }
}

/// Moves the media a restore left into the blob store, as media of our own
/// posts. Each file is deleted once it's in; one that fails stays for the
/// next start.
async fn import_restored_media(home: &Home, store: &ContentStore) -> Result<()> {
    let dir = home.restored_media();
    if !dir.exists() {
        return Ok(());
    }
    let mut imported = 0;
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        let expected: BlobHash = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.parse().ok())
            .with_context(|| format!("{} isn't restored media", path.display()))?;
        let (hash, _) = store.import(&path, BlobClass::Own).await?;
        ensure!(
            hash == expected,
            "restored media {} doesn't match its name",
            path.display()
        );
        std::fs::remove_file(&path)?;
        imported += 1;
    }
    std::fs::remove_dir(&dir)?;
    info!(files = imported, "imported the media of a restored backup");
    Ok(())
}

/// Writes a new home's keys, config and database. Refuses to touch a home
/// that already holds a node.
fn create_home(home: &Home, config: &Config, identity: &SecretKey) -> Result<()> {
    if home.is_initialized() {
        bail!("{} already holds a node", home.root().display());
    }
    std::fs::create_dir_all(home.root())
        .with_context(|| format!("creating {}", home.root().display()))?;
    std::fs::set_permissions(home.root(), std::fs::Permissions::from_mode(0o700))?;
    keys::create_signing_key(home, identity)?;
    keys::create(&home.node_key())?;
    config.save(&home.config())?;
    Db::open(&home.db())?;
    Ok(())
}

/// Takes the home's lock, so two nodes never share one home (they would
/// otherwise wait on each other's databases without saying why).
fn lock_home(home: &Home) -> Result<std::fs::File> {
    let path = home.lock();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    if rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).is_err() {
        bail!(
            "another node is already running in {} (as a service? see `systemctl --user status osp`)",
            home.root().display()
        );
    }
    Ok(file)
}

/// Adds an address to the lookup table, unless it carries nothing but an ID.
pub(crate) fn remember(lookup: &MemoryLookup, addr: &EndpointAddr) {
    if !addr.is_empty() {
        lookup.add_endpoint_info(addr.clone());
    }
}

/// Refuses a node address that lists more than `MAX_ADDRS` relays and IP
/// addresses. A relay and a few IP addresses is normal; a long list would
/// let a peer make us store, and dial, addresses of its choosing.
pub(crate) fn check_addr(addr: &EndpointAddr) -> Result<()> {
    let n = addr.addrs.len();
    ensure!(
        n <= MAX_ADDRS,
        "node address lists {n} addresses; at most {MAX_ADDRS} are accepted"
    );
    Ok(())
}

/// Refuses settings that can't work together.
fn check_network_config(config: &Config) -> Result<()> {
    let net = &config.network;
    if net.relay_only {
        ensure!(
            net.relay != RelayChoice::Disabled,
            "relay_only needs relays: set relay to \"default\" or to your own relays in config.toml"
        );
        ensure!(
            net.bind.is_none(),
            "relay_only and bind can't be used together: a relay-only node has no IP sockets to bind"
        );
    }
    Ok(())
}

async fn bind_endpoint(
    config: &Config,
    key: SecretKey,
    lookup: &MemoryLookup,
    relays: &RelayPolicy,
    tracker: ConnTracker,
) -> Result<Endpoint> {
    let net = &config.network;
    let transport = QuicTransportConfig::builder()
        .max_concurrent_bidi_streams(MAX_STREAMS_PER_CONN.into())
        // None of our protocols use one-way streams. Nobody would read them,
        // so whatever a peer sent on them would sit in memory.
        .max_concurrent_uni_streams(0u32.into())
        .build();
    // The lookups are set up again here, as in the N0 preset, so that what
    // they find passes the relay policy first.
    let mut builder = Endpoint::builder(presets::N0)
        .hooks(tracker)
        .transport_config(transport)
        .clear_address_lookup()
        .address_lookup(Checked::new(lookup.clone(), relays.clone()));
    if net.address_lookup {
        builder = builder
            .address_lookup(PkarrPublisher::n0_dns())
            .address_lookup(Checked::new(PkarrResolver::n0_dns(), relays.clone()))
            .address_lookup(Checked::new(DnsAddressLookup::n0_dns(), relays.clone()));
    }
    builder = match &net.relay {
        RelayChoice::Default => builder,
        RelayChoice::Disabled => builder.relay_mode(RelayMode::Disabled),
        RelayChoice::Custom(_) => {
            builder.relay_mode(RelayMode::custom(relays::own_relays(&net.relay)?))
        }
    };
    if net.relay_only {
        // No IP sockets at all, so there is nothing to connect directly
        // with, and no address to give away. Without a relay, nothing
        // connects: there is no direct path to fall back to.
        builder = builder.clear_ip_transports();
    } else if let Some(addr) = net.bind {
        builder = builder.clear_ip_transports().bind_addr(addr)?;
    }
    builder
        .secret_key(key)
        .bind()
        .await
        .context("starting network endpoint")
}

/// Reuses the stored profile if nothing changed, so its ID stays stable.
/// If the stored one lists another node, this node has taken the identity
/// over (ADR 0009), and every follower is marked to be told. One signed
/// with an earlier key is signed again, so it carries the current key's
/// certificate (ADR 0010).
fn own_profile(signer: &Signer, name: &str, node: EndpointId, db: &Db) -> Result<SignedObject> {
    let me = signer.identity();
    let previous = match db.profile(&me)?.map(|obj| (obj.verify(), obj)) {
        Some((Ok(Payload::Person(p)), obj)) => Some((p, obj)),
        _ => None,
    };
    if let Some((p, existing)) = &previous
        && p.name == name
        && p.nodes == [node]
        && existing.certificate() == signer.certificate()
    {
        return Ok(existing.clone());
    }
    // Followers take the newer profile, so make it later than the one it
    // replaces even if this computer's clock is behind.
    let mut at = SystemTime::now();
    if let Some(ms) = previous
        .as_ref()
        .and_then(|(p, _)| osp_proto::parse_rfc3339_ms(&p.published))
    {
        at = at.max(UNIX_EPOCH + Duration::from_millis(ms as u64 + 1));
    }
    let person = Payload::Person(Person {
        actor: me,
        name: name.to_string(),
        summary: None,
        nodes: vec![node],
        published: humantime::format_rfc3339_millis(at).to_string(),
    });
    let obj = signer.sign(&person)?;
    db.upsert_profile(&me, name, &obj)?;
    if previous.is_some_and(|(p, _)| !p.nodes.contains(&node)) {
        let followers = db.mark_followers_to_tell_moved()?;
        info!(
            followers,
            "this node took over the identity; telling its followers"
        );
    }
    Ok(obj)
}
