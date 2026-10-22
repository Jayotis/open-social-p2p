//! Wiring: one iroh endpoint, the protocols on it, and the background tasks.

use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, PublicKey, RelayMode, RelayUrl, SecretKey,
    address_lookup::MemoryLookup,
    endpoint::{Connection, presets},
    protocol::Router,
};
use osp_proto::{
    BlobHash, Payload, Person, SignedObject,
    rpc::{ClientRequest, ClientResponse},
};
use tokio::{
    sync::{Notify, watch},
    task::JoinHandle,
};
use tracing::info;

use crate::{
    config::{Config, RelayChoice},
    control::{self, Origin},
    db::{BlobClass, Db},
    home::Home,
    keys,
    network::{self, ConnTracker},
    social::SocialProtocol,
    store::ContentStore,
    sync,
};

/// A running home node.
pub struct Node {
    core: Arc<Core>,
    router: Router,
    tasks: Vec<JoinHandle<()>>,
    serving_socket: bool,
}

/// State shared by the protocol handlers and background tasks.
pub(crate) struct Core {
    pub home: Home,
    pub config: Config,
    pub identity: SecretKey,
    pub endpoint: Endpoint,
    /// Addresses we learned out of band (tickets, subscriptions, WhoHosts),
    /// so peers can be dialled by ID without a lookup service.
    pub lookup: MemoryLookup,
    pub db: Arc<Db>,
    pub store: ContentStore,
    /// Our signed profile, served to anyone who asks.
    pub profile: SignedObject,
    /// Open `osp/social/0` connections, reused across requests.
    pub conns: Mutex<HashMap<EndpointId, Connection>>,
    /// Every live connection in either direction, for `status`.
    pub tracker: ConnTracker,
    /// Only one sync runs at a time.
    pub sync_lock: tokio::sync::Mutex<()>,
    /// Wakes the sync loop early (on a push notification or a new follow).
    pub sync_now: Notify,
    /// Number of prefetch jobs still downloading, so `Sync` can wait for them.
    pub prefetch_jobs: watch::Sender<usize>,
}

impl Node {
    /// Creates a new node home: keys, config and database.
    pub fn init(home: &Path, config: &Config) -> Result<()> {
        let home = Home::new(home);
        if home.is_initialized() {
            bail!("{} already holds a node", home.root().display());
        }
        std::fs::create_dir_all(home.root())
            .with_context(|| format!("creating {}", home.root().display()))?;
        std::fs::set_permissions(home.root(), std::fs::Permissions::from_mode(0o700))?;
        keys::create(&home.identity_key())?;
        keys::create(&home.node_key())?;
        config.save(&home.config())?;
        Db::open(&home.db())?;
        Ok(())
    }

    /// Starts the node stored in `home`.
    pub async fn spawn(home: &Path) -> Result<Self> {
        let home = Home::new(home);
        if !home.is_initialized() {
            bail!("no node in {}; run `osp init` first", home.root().display());
        }
        let config = Config::load(&home.config())?;
        let identity = keys::load(&home.identity_key())?;
        let node_key = keys::load(&home.node_key())?;
        let db = Arc::new(Db::open(&home.db())?);
        std::fs::create_dir_all(home.work())?;

        let lookup = MemoryLookup::new();
        let tracker = ConnTracker::default();
        let endpoint = bind_endpoint(&config, node_key, lookup.clone(), tracker.clone()).await?;
        for follow in db.follows()? {
            remember(&lookup, &follow.addr);
        }
        for addr in db.followers()? {
            remember(&lookup, &addr);
        }

        let store = ContentStore::open(&home.blobs(), &endpoint, db.clone()).await?;
        let profile = own_profile(&identity, &config.name, endpoint.id(), &db)?;
        info!(
            identity = %identity.public().fmt_short(),
            node = %endpoint.id().fmt_short(),
            "node started"
        );

        let core = Arc::new(Core {
            home,
            config,
            identity,
            endpoint: endpoint.clone(),
            lookup,
            db,
            store,
            profile,
            conns: Mutex::new(HashMap::new()),
            tracker,
            sync_lock: tokio::sync::Mutex::new(()),
            sync_now: Notify::new(),
            prefetch_jobs: watch::Sender::new(0),
        });

        let (blobs, serving_rules) = core.store.protocol();
        let router = Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, blobs)
            .accept(osp_proto::SOCIAL_ALPN, SocialProtocol::new(core.clone()))
            .spawn();
        let sync_loop = tokio::spawn(sync::run_loop(core.clone()));
        let mut tasks = vec![serving_rules, sync_loop];
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
        self.core.identity.public()
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

    /// Downloads a blob from specific nodes into the cache. For tests and diagnostics.
    pub async fn download_blob_from(
        &self,
        hash: &BlobHash,
        providers: &[EndpointAddr],
    ) -> Result<u64> {
        for addr in providers {
            remember(&self.core.lookup, addr);
        }
        let ids = providers.iter().map(|a| a.id).collect();
        self.core.store.download(hash, ids, BlobClass::Cache).await
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
        if self.config.network.relay == RelayChoice::Disabled {
            return self.endpoint.addr();
        }
        // Wait briefly for a home relay; without a network we hand out the ID alone.
        let _ = tokio::time::timeout(Duration::from_secs(10), self.endpoint.online()).await;
        let addr = self.endpoint.addr();
        EndpointAddr::from_parts(addr.id, addr.addrs.into_iter().filter(|a| a.is_relay()))
    }
}

/// Adds an address to the lookup table, unless it carries nothing but an ID.
pub(crate) fn remember(lookup: &MemoryLookup, addr: &EndpointAddr) {
    if !addr.is_empty() {
        lookup.add_endpoint_info(addr.clone());
    }
}

async fn bind_endpoint(
    config: &Config,
    key: SecretKey,
    lookup: MemoryLookup,
    tracker: ConnTracker,
) -> Result<Endpoint> {
    let net = &config.network;
    let mut builder = Endpoint::builder(presets::N0).hooks(tracker);
    if !net.address_lookup {
        builder = builder.clear_address_lookup();
    }
    builder = match &net.relay {
        RelayChoice::Default => builder,
        RelayChoice::Disabled => builder.relay_mode(RelayMode::Disabled),
        RelayChoice::Custom(urls) => {
            let urls = urls
                .iter()
                .map(|u| u.parse::<RelayUrl>())
                .collect::<Result<Vec<_>, _>>()
                .context("invalid relay URL in config")?;
            builder.relay_mode(RelayMode::custom(urls))
        }
    };
    if let Some(addr) = net.bind {
        builder = builder.clear_ip_transports().bind_addr(addr)?;
    }
    builder
        .secret_key(key)
        .address_lookup(lookup)
        .bind()
        .await
        .context("starting network endpoint")
}

/// Reuses the stored profile if nothing changed, so its ID stays stable.
fn own_profile(
    identity: &SecretKey,
    name: &str,
    node: EndpointId,
    db: &Db,
) -> Result<SignedObject> {
    let me = identity.public();
    if let Some(existing) = db.profile(&me)?
        && let Ok(Payload::Person(p)) = existing.verify()
        && p.name == name
        && p.nodes == [node]
    {
        return Ok(existing);
    }
    let person = Payload::Person(Person {
        actor: me,
        name: name.to_string(),
        summary: None,
        nodes: vec![node],
        published: osp_proto::now_rfc3339(),
    });
    let obj = SignedObject::sign(identity, &person)?;
    db.upsert_profile(&me, name, &obj)?;
    Ok(obj)
}
