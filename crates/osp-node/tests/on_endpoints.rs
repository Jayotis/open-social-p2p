//! Messages, media and the swarm with the media and direct endpoints on (ADR
//! 0014, build step 3), with a real relay on loopback. Messages stay on each
//! node's main endpoint here, as only real Tor can put them on Tor: an ignored
//! test in `tor.rs` does that.
//!
//! As in `endpoints.rs`, `told_of` counts the addresses of an endpoint that
//! iroh's client side was told, as the node's side can't show it.
//! Requires ffmpeg.

use std::{
    io::Write,
    net::Ipv4Addr,
    path::Path,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr,
    address_lookup::MemoryLookup,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_blobs::{BlobsProtocol, store::mem::MemStore};
use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use osp_node::{
    BlobClass, Config, DirectRule, Home, Node, RelayChoice, SimSettings, simnet::TorDelay,
};
use osp_proto::{
    Attachment, BlobHash, BoundEndpoint, CLIENT_ALPN, EndpointBinding, EndpointKind, FollowTicket,
    SOCIAL_ALPN,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, PeerRecord},
    social::{SocialRequest, SocialResponse},
};

/// Long enough for anything on loopback.
const PROMPTLY: Duration = Duration::from_secs(30);

/// Lines of iroh's log saying a node was told of an address.
static TOLD: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Times a store was shut down when it already had been.
static SHUT_DOWN_AGAIN: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
struct Capture;

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let line = String::from_utf8_lossy(buf);
        if line.contains("AddressAdded") {
            TOLD.lock().unwrap().push(line.into_owned());
        } else if line.contains("error shutting down store") {
            SHUT_DOWN_AGAIN.fetch_add(1, Ordering::Relaxed);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_logs() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        tracing_subscriber::fmt()
            .with_env_filter("iroh::socket::remote_map=trace,iroh_blobs::net_protocol=error")
            .with_ansi(false)
            .with_writer(|| Capture)
            .init();
    });
}

/// How many addresses of `endpoint` a node has been told. Each client's log
/// names the endpoint it's connected to.
fn told_of(endpoint: &EndpointId) -> usize {
    let remote = format!("remote={}", endpoint.fmt_short());
    let told = TOLD.lock().unwrap();
    told.iter().filter(|line| line.contains(&remote)).count()
}

/// A plain-http relay on loopback. Keep the server; dropping it stops it.
async fn local_relay() -> (Server, RelayUrl) {
    capture_logs();
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new((Ipv4Addr::LOCALHOST, 0)));
    let server = Server::spawn(config).await.unwrap();
    let url = format!("http://{}", server.http_addr().unwrap())
        .parse()
        .unwrap();
    (server, url)
}

fn config(name: &str, relay: &RelayUrl, rule: DirectRule, share: bool) -> Config {
    let mut config = Config::local_test(name);
    config.network.relay = RelayChoice::Custom(vec![relay.to_string()]);
    config.endpoints.enabled = true;
    config.endpoints.direct = rule;
    config.swarm.share_prefetched = share;
    config
}

/// A node that fetches large files at once, not spread out over minutes.
async fn start(root: &Path, config: Config) -> Node {
    start_sim(root, config, SimSettings::default()).await
}

async fn start_sim(root: &Path, config: Config, sim: SimSettings) -> Node {
    let home = root.join(config.name.to_lowercase());
    Node::init(&home, &config).unwrap();
    let sim = SimSettings {
        prefetch_spread: Duration::ZERO,
        ..sim
    };
    Node::spawn_sim(&home, sim).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn follow(node: &Node, author: &Node) {
    let ClientResponse::Ticket { ticket } = call(author, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(node, ClientRequest::Follow { ticket }).await;
    call(node, ClientRequest::Sync).await;
}

/// Posts a photo. A `small` one is under 1 MiB; otherwise it's noise, which
/// compresses badly, about 3 MB. Returns its attachment.
async fn post_photo(node: &Node, dir: &Path, small: bool) -> Attachment {
    let photo = dir.join("photo.png");
    let source = if small {
        "testsrc=size=640x360"
    } else {
        "nullsrc=s=2048x2048,geq=random(1)*255:random(1)*255:random(1)*255"
    };
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", source])
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let post = ClientRequest::Post {
        content: "a photo".into(),
        attachments: vec![photo],
        uploads: vec![],
        upload: None,
    };
    call(node, post).await;
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    items[0].attachments[0].attachment.clone()
}

/// What `node` measured about `other` (ADR 0011).
async fn peer(node: &Node, other: EndpointId) -> Option<PeerRecord> {
    match call(node, ClientRequest::Peers).await {
        ClientResponse::Peers { peers } => peers.into_iter().find(|p| p.node == other),
        other => panic!("unexpected {other:?}"),
    }
}

/// Waits until `check` holds.
async fn until(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let started = Instant::now();
    while !check().await {
        assert!(started.elapsed() < PROMPTLY, "{what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Waits until `node` holds `blob` as a prefetched copy. One that shares
/// fetches large files after its sync, from the queue of those put off.
async fn prefetched(node: &Node, blob: &BlobHash) {
    until("not prefetched", async || {
        node.blob_class(blob).unwrap() == Some(BlobClass::Prefetch)
    })
    .await;
}

/// A peer node made by hand, reaching a node's main endpoint through the relay.
/// Holds its endpoint, since dropping that closes the connection.
struct Peer {
    key: SecretKey,
    endpoint: Endpoint,
    conn: Connection,
}

impl Peer {
    async fn new(node: &EndpointAddr, relay: &RelayUrl) -> Self {
        let key = SecretKey::generate();
        let lookup = MemoryLookup::new();
        lookup.add_endpoint_info(node.clone());
        let endpoint = Endpoint::builder(presets::N0)
            .clear_address_lookup()
            .address_lookup(lookup)
            .relay_mode(RelayMode::custom([relay.clone()]))
            .secret_key(key.clone())
            .bind()
            .await
            .unwrap();
        let conn = endpoint.connect(node.clone(), SOCIAL_ALPN).await.unwrap();
        Self {
            key,
            endpoint,
            conn,
        }
    }

    /// A peer that follows the node and shares what it fetches.
    async fn follower(node: &EndpointAddr, relay: &RelayUrl, media: Option<&SecretKey>) -> Self {
        let peer = Self::new(node, relay).await;
        let media = media.map(|key| Box::new(peer.bound(key, EndpointKind::Media, relay)));
        let reply = peer.subscribe(media, None).await;
        assert!(matches!(reply, SocialResponse::Ok), "{reply:?}");
        peer
    }

    async fn subscribe(
        &self,
        media: Option<Box<BoundEndpoint>>,
        direct: Option<Box<BoundEndpoint>>,
    ) -> SocialResponse {
        let req = SocialRequest::Subscribe {
            addr: EndpointAddr::new(self.endpoint.id()),
            shares_prefetched: true,
            media,
            direct,
        };
        self.ask(&req).await
    }

    /// This peer's endpoint with `key`, reached through the relay.
    fn bound(&self, key: &SecretKey, kind: EndpointKind, relay: &RelayUrl) -> BoundEndpoint {
        BoundEndpoint {
            binding: EndpointBinding::new(&self.key, key, kind, 1),
            addr: EndpointAddr::from_parts(key.public(), [TransportAddr::Relay(relay.clone())]),
        }
    }

    async fn ask(&self, req: &SocialRequest) -> SocialResponse {
        let (mut send, mut recv) = self.conn.open_bi().await.unwrap();
        write_frame(&mut send, req).await.unwrap();
        send.finish().unwrap();
        read_frame(&mut recv).await.unwrap()
    }

    /// The hosts a node names, by their endpoints.
    async fn who_hosts(&self, blob: BlobHash) -> Vec<BoundEndpoint> {
        match self.ask(&SocialRequest::WhoHosts { blob }).await {
            SocialResponse::BoundHosts { hosts } => hosts,
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Waits until the node names `node` as a host of `blob`, and returns the
    /// hosts it named. Slowly enough to stay within the samples a follower
    /// gets an hour.
    async fn hosts_naming(&self, blob: BlobHash, node: EndpointId) -> Vec<BoundEndpoint> {
        let started = Instant::now();
        loop {
            let hosts = self.who_hosts(blob).await;
            if hosts.iter().any(|h| h.binding.public == node) {
                return hosts;
            }
            assert!(started.elapsed() < PROMPTLY, "{node} was never named");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

/// The main endpoint carries only messages. Media goes through the others, and
/// what's measured of it is counted against each node's public ID.
#[tokio::test(flavor = "multi_thread")]
async fn media_goes_through_the_other_endpoints_and_counts_against_the_public_id() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let ada = start(dir.path(), config("Ada", &url, DirectRule::Nobody, false)).await;
    let bea = start(dir.path(), config("Bea", &url, DirectRule::Nobody, false)).await;
    let photo = post_photo(&ada, dir.path(), false).await;

    // A ticket and everything else about the public endpoint names only a
    // relay, here, as this node is off Tor.
    follow(&bea, &ada).await;
    assert_eq!(
        bea.blob_class(&photo.blob).unwrap(),
        Some(BlobClass::Prefetch)
    );

    let from_ada = peer(&bea, ada.node_id()).await.expect("a record of Ada");
    assert!(from_ada.followed);
    assert!(from_ada.counts.received_bytes > 0.0, "{from_ada:?}");
    assert!(from_ada.counts.completed > 0.0, "{from_ada:?}");
    let ada_media = ada.media_addr().await.unwrap().id;
    assert!(peer(&bea, ada_media).await.is_none(), "counted by endpoint");

    // Ada's side too: Bea's media endpoint is Bea's, since Bea subscribed with
    // it. The serving side records a transfer just after the client has it.
    let bea_media = bea.media_addr().await.unwrap().id;
    until("Ada recorded what she sent", async || {
        peer(&ada, bea.node_id())
            .await
            .is_some_and(|p| p.follower && p.counts.sent_bytes > 0.0)
    })
    .await;
    assert!(peer(&ada, bea_media).await.is_none(), "counted by endpoint");

    // Neither learned an address of the other from the media path.
    for node in [&ada, &bea] {
        let media = node.media_addr().await.unwrap().id;
        let direct = node.direct_addr().await.unwrap().id;
        assert_eq!(told_of(&media), 0);
        assert_eq!(told_of(&direct), 0);
    }

    // The main endpoint serves neither media nor phones any more.
    let stranger = Peer::new(&ada.addr().await, &url).await;
    for alpn in [iroh_blobs::ALPN, CLIENT_ALPN] {
        let tried = stranger.endpoint.connect(ada.addr().await, alpn).await;
        assert!(tried.is_err(), "the main endpoint took {alpn:?}");
    }

    // A restart keeps what it knows of the other node's endpoints.
    let home = dir.path().join("bea");
    bea.shutdown().await.unwrap();
    let bea = Node::spawn_sim(&home, SimSettings::default())
        .await
        .unwrap();
    let second = post_photo(&ada, dir.path(), true).await;
    call(&bea, ClientRequest::Sync).await;
    assert_eq!(
        bea.blob_class(&second.blob).unwrap(),
        Some(BlobClass::Prefetch)
    );
}

/// A follower holding a copy is named by its media endpoint, with the binding
/// that ties it to its public ID, and another follower fetches from it there.
#[tokio::test(flavor = "multi_thread")]
async fn followers_are_named_by_their_media_endpoint_and_fetched_from_there() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let ada = start(dir.path(), config("Ada", &url, DirectRule::Nobody, false)).await;
    let bea = start(dir.path(), config("Bea", &url, DirectRule::Nobody, true)).await;
    let cid = start(dir.path(), config("Cid", &url, DirectRule::Nobody, true)).await;
    let photo = post_photo(&ada, dir.path(), false).await;

    follow(&bea, &ada).await;
    prefetched(&bea, &photo.blob).await;

    let bea_media = bea.media_addr().await.unwrap();
    let watcher = Peer::follower(&ada.addr().await, &url, None).await;
    let hosts = watcher.hosts_naming(photo.blob, bea.node_id()).await;
    let named = hosts
        .iter()
        .find(|h| h.binding.public == bea.node_id())
        .unwrap();
    assert_eq!(named.binding.kind, EndpointKind::Media);
    assert_eq!(named.binding.endpoint, bea_media.id);
    named.check(&bea.node_id(), EndpointKind::Media).unwrap();
    // No host's address gives any more than a relay.
    assert!(hosts.iter().all(|h| h.addr.ip_addrs().next().is_none()));
    // Ada names herself by her media endpoint too.
    let ada_media = ada.media_addr().await.unwrap().id;
    assert!(hosts.iter().any(|h| h.binding.endpoint == ada_media));

    follow(&cid, &ada).await;
    prefetched(&cid, &photo.blob).await;
    let from_bea = peer(&cid, bea.node_id()).await.expect("a record of Bea");
    assert!(from_bea.counts.received_bytes > 0.0, "{from_bea:?}");
    // To Bea, Cid isn't a follower: a stranger, known by its media endpoint.
    let cid_media = cid.media_addr().await.unwrap().id;
    until("Bea recorded what she sent", async || {
        peer(&bea, cid_media)
            .await
            .is_some_and(|p| p.counts.sent_bytes > 0.0)
    })
    .await;

    // Cid told Ada of its copy too, and Ada's check of it went to Cid's media
    // endpoint: it's named now, and the check counted as a hosting copy.
    let hosts = watcher.hosts_naming(photo.blob, cid.node_id()).await;
    assert!(
        hosts.iter().any(|h| h.binding.endpoint == cid_media),
        "{hosts:?}"
    );
}

/// Two endpoints serve from one store, which is shut down once, when the last
/// of them stops, and what's in it is there after a restart.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_shuts_its_store_down_once_and_keeps_its_media() {
    capture_logs();
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let ada = start(dir.path(), config("Ada", &url, DirectRule::Nobody, false)).await;
    let bea = start(dir.path(), config("Bea", &url, DirectRule::Nobody, false)).await;
    let photo = post_photo(&ada, dir.path(), true).await;
    ada.shutdown().await.unwrap();
    assert_eq!(SHUT_DOWN_AGAIN.load(Ordering::Relaxed), 0);

    let ada = Node::spawn_sim(&dir.path().join("ada"), SimSettings::default())
        .await
        .unwrap();
    let size = bea
        .download_blob_from(&photo.blob, photo.size, &[ada.media_addr().await.unwrap()])
        .await
        .unwrap();
    assert_eq!(size, photo.size);
}

/// A direct endpoint given to one node, because its rule allows that node,
/// never reaches anyone else: an address given can't be taken back.
#[tokio::test(flavor = "multi_thread")]
async fn a_direct_endpoint_offered_privately_is_never_named_to_others() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let ada = start(dir.path(), config("Ada", &url, DirectRule::Follows, false)).await;
    let bea = start(dir.path(), config("Bea", &url, DirectRule::Follows, true)).await;
    // They follow each other, so both rules allow it.
    follow(&ada, &bea).await;
    follow(&bea, &ada).await;
    let bea_direct = bea.direct_addr().await.unwrap().id;
    let found = ada.ask_endpoints(&bea.addr().await).await.unwrap();
    assert_eq!(found.direct.map(|a| a.id), Some(bea_direct));

    // Bea fetches Ada's photo, from Bea's direct endpoint to Ada's.
    let photo = post_photo(&ada, dir.path(), false).await;
    call(&bea, ClientRequest::Sync).await;
    prefetched(&bea, &photo.blob).await;
    let ada_direct = ada.direct_addr().await.unwrap().id;
    assert!(told_of(&ada_direct) > 0, "Bea didn't connect directly");

    // Bea holds a copy, and is named: by her media endpoint only.
    let watcher = Peer::follower(&ada.addr().await, &url, None).await;
    let hosts = watcher.hosts_naming(photo.blob, bea.node_id()).await;
    let named = hosts
        .iter()
        .find(|h| h.binding.public == bea.node_id())
        .unwrap();
    assert_eq!(named.binding.kind, EndpointKind::Media);
    assert!(hosts.iter().all(|h| h.binding.endpoint != bea_direct));
    assert!(hosts.iter().all(|h| h.binding.endpoint != ada_direct));
}

/// A follower whose rule allows anyone is named by its direct endpoint. A node
/// that allows nobody still fetches from its media endpoint, so its own
/// address stays hidden even from a node that allows everyone.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_that_allows_nobody_stays_hidden_from_one_that_allows_anyone() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let ada = start(dir.path(), config("Ada", &url, DirectRule::Nobody, false)).await;
    let bea = start(dir.path(), config("Bea", &url, DirectRule::Anyone, true)).await;
    let cid = start(dir.path(), config("Cid", &url, DirectRule::Nobody, true)).await;
    let photo = post_photo(&ada, dir.path(), false).await;
    follow(&bea, &ada).await;
    prefetched(&bea, &photo.blob).await;

    let bea_direct = bea.direct_addr().await.unwrap().id;
    let watcher = Peer::follower(&ada.addr().await, &url, None).await;
    let hosts = watcher.hosts_naming(photo.blob, bea.node_id()).await;
    let named = hosts
        .iter()
        .find(|h| h.binding.public == bea.node_id())
        .unwrap();
    assert_eq!(named.binding.kind, EndpointKind::Direct);
    assert_eq!(named.binding.endpoint, bea_direct);
    assert!(named.addr.ip_addrs().next().is_none(), "{named:?}");

    follow(&cid, &ada).await;
    prefetched(&cid, &photo.blob).await;
    let from_bea = peer(&cid, bea.node_id()).await.expect("a record of Bea");
    assert!(from_bea.counts.received_bytes > 0.0, "{from_bea:?}");

    // It came to Bea from Cid's media endpoint, which has no address to show.
    // To Bea, Cid is a stranger, known by the endpoint it connected from.
    let cid_media = cid.media_addr().await.unwrap().id;
    let cid_direct = cid.direct_addr().await.unwrap().id;
    until("Bea recorded what she sent", async || {
        peer(&bea, cid_media)
            .await
            .is_some_and(|p| p.counts.sent_bytes > 0.0)
    })
    .await;
    assert!(
        peer(&bea, cid_direct).await.is_none(),
        "Cid connected directly"
    );
}

/// A node can stop allowing direct connections without telling anyone. Its
/// direct endpoint then refuses, and its media comes through the media one. A
/// node in the swarm has only those to try: one that isn't asks who else hosts
/// the file, and is told.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_that_stops_allowing_direct_connections_is_still_fetched_from() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let ada = start(dir.path(), config("Ada", &url, DirectRule::Anyone, false)).await;
    let bea = start(dir.path(), config("Bea", &url, DirectRule::Anyone, true)).await;
    follow(&bea, &ada).await;
    let bea_addr = ada.addr().await;
    let found = bea.ask_endpoints(&bea_addr).await.unwrap();
    assert!(found.direct.is_some());

    // Ada's rule changes as she restarts. Her direct endpoint has the same key.
    let home = dir.path().join("ada");
    ada.shutdown().await.unwrap();
    let mut changed = Config::load(&Home::new(&home).config()).unwrap();
    changed.endpoints.direct = DirectRule::Nobody;
    changed.save(&Home::new(&home).config()).unwrap();
    let sim = SimSettings {
        prefetch_spread: Duration::ZERO,
        ..SimSettings::default()
    };
    let ada = Node::spawn_sim(&home, sim).await.unwrap();

    let photo = post_photo(&ada, dir.path(), false).await;
    call(&bea, ClientRequest::Sync).await;
    prefetched(&bea, &photo.blob).await;

    // Told, Bea forgets the direct endpoint.
    let found = bea.ask_endpoints(&ada.addr().await).await.unwrap();
    assert_eq!(found.direct, None);
}

/// Subscribing with an endpoint that isn't the subscriber's is refused, and a
/// follower with no endpoint isn't named or checked.
#[tokio::test(flavor = "multi_thread")]
async fn a_subscriber_is_held_to_its_bindings() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let ada = start(dir.path(), config("Ada", &url, DirectRule::Nobody, false)).await;
    let photo = post_photo(&ada, dir.path(), true).await;

    let addr = ada.addr().await;
    let peer = Peer::new(&addr, &url).await;
    let media_key = SecretKey::generate();
    let mine = peer.bound(&media_key, EndpointKind::Media, &url);
    // Another node's, one of the wrong kind, and one naming another endpoint.
    let bad = [
        BoundEndpoint {
            binding: EndpointBinding::new(
                &SecretKey::generate(),
                &media_key,
                EndpointKind::Media,
                1,
            ),
            ..mine.clone()
        },
        peer.bound(&media_key, EndpointKind::Direct, &url),
        BoundEndpoint {
            addr: EndpointAddr::new(SecretKey::generate().public()),
            ..mine.clone()
        },
    ];
    for bound in bad {
        let reply = peer.subscribe(Some(Box::new(bound)), None).await;
        assert!(matches!(reply, SocialResponse::Error { .. }), "{reply:?}");
    }
    let reply = peer
        .subscribe(Some(Box::new(mine.clone())), Some(Box::new(mine)))
        .await;
    assert!(
        matches!(reply, SocialResponse::Error { .. }),
        "a media endpoint passed off as a direct one: {reply:?}"
    );
    // None of those made it a follower.
    let ClientResponse::Status(status) = call(&ada, ClientRequest::Status).await else {
        panic!("no status");
    };
    assert_eq!(status.followers, 0);

    // Without an endpoint, a follower's claim of a copy can't be checked.
    let without = Peer::follower(&addr, &url, None).await;
    let reply = without
        .ask(&SocialRequest::Hosting { blob: photo.blob })
        .await;
    let SocialResponse::Error { message } = reply else {
        panic!("unexpected {reply:?}");
    };
    assert!(message.contains("doesn't know an endpoint"), "{message}");

    // With one, it's checked there, and one that serves nothing is refused.
    let quiet_key = SecretKey::generate();
    let quiet = Peer::follower(&addr, &url, Some(&quiet_key)).await;
    let reply = quiet
        .ask(&SocialRequest::Hosting { blob: photo.blob })
        .await;
    assert!(matches!(reply, SocialResponse::Error { .. }), "{reply:?}");
    let hosts = without.who_hosts(photo.blob).await;
    let quiet_public = quiet.endpoint.id();
    assert!(hosts.iter().all(|h| h.binding.public != quiet_public));
}

/// Answers `WhoHosts` with the hosts it's given, whoever asks, and refuses
/// anything else, as a followed node gone bad might.
#[derive(Debug, Clone)]
struct NamesHosts(Vec<BoundEndpoint>);

impl ProtocolHandler for NamesHosts {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let reply = match read_frame::<_, SocialRequest>(&mut recv).await {
                Ok(SocialRequest::WhoHosts { .. }) => SocialResponse::BoundHosts {
                    hosts: self.0.clone(),
                },
                _ => SocialResponse::Error {
                    message: "not that".to_string(),
                },
            };
            let _ = write_frame(&mut send, &reply).await;
            let _ = send.finish();
        }
        Ok(())
    }
}

/// A host named with a binding that doesn't check out isn't used, wherever it
/// is in the reply, and one that does is counted against the node it names.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_named_with_a_bad_binding_isnt_used() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let author = start(
        dir.path(),
        config("Author", &url, DirectRule::Nobody, false),
    )
    .await;
    let mut reader_config = config("Reader", &url, DirectRule::Nobody, false);
    reader_config.sync.prefetch = false;
    let reader = start(dir.path(), reader_config).await;
    let photo = post_photo(&author, dir.path(), true).await;
    follow(&reader, &author).await;

    // Two hosts with a copy, each with its media endpoint on the relay.
    let media = MemStore::new();
    let source = dir.path().join("copy");
    call(
        &author,
        ClientRequest::Fetch {
            id: {
                let ClientResponse::Feed { items } =
                    call(&author, ClientRequest::Feed { limit: 1 }).await
                else {
                    panic!("no feed");
                };
                items[0].id
            },
            out_dir: Some(source.clone()),
        },
    )
    .await;
    let copy = std::fs::read(source.join(format!("{}.webp", photo.blob))).unwrap();
    let _kept = media.add_slice(&copy).temp_tag().await.unwrap();
    let serving = async |key: SecretKey| {
        let endpoint = Endpoint::builder(presets::N0)
            .clear_address_lookup()
            .relay_mode(RelayMode::custom([url.clone()]))
            .secret_key(key)
            .bind()
            .await
            .unwrap();
        endpoint.online().await;
        Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, BlobsProtocol::new(&media, None))
            .spawn()
    };
    let (honest, tampered) = (SecretKey::generate(), SecretKey::generate());
    let (honest_public, tampered_public) = (SecretKey::generate(), SecretKey::generate());
    let bound = |public: &SecretKey, key: &SecretKey| BoundEndpoint {
        binding: EndpointBinding::new(public, key, EndpointKind::Media, 1),
        addr: EndpointAddr::from_parts(key.public(), [TransportAddr::Relay(url.clone())]),
    };
    let _honest_host = serving(honest.clone()).await;
    let _tampered_host = serving(tampered.clone()).await;
    // Named first, it would be tried first.
    let mut changed = bound(&tampered_public, &tampered);
    changed.binding.since_ms += 1;
    let hosts = vec![changed, bound(&honest_public, &honest)];

    // A followed node that names them, and the author gone, so only they're
    // left to ask.
    let liar_key = SecretKey::generate();
    let liar = Endpoint::builder(presets::N0)
        .clear_address_lookup()
        .relay_mode(RelayMode::custom([url.clone()]))
        .secret_key(liar_key.clone())
        .bind()
        .await
        .unwrap();
    liar.online().await;
    let _liar_router = Router::builder(liar.clone())
        .accept(SOCIAL_ALPN, NamesHosts(hosts))
        .spawn();
    let ticket = FollowTicket {
        actor: SecretKey::generate().public(),
        node: EndpointAddr::from_parts(liar.id(), [TransportAddr::Relay(url.clone())]),
    };
    let ticket = ticket.to_string();
    call(&reader, ClientRequest::Follow { ticket }).await;
    author.shutdown().await.unwrap();

    let ClientResponse::Feed { items } = call(&reader, ClientRequest::Feed { limit: 1 }).await
    else {
        panic!("no feed");
    };
    let open = ClientRequest::Fetch {
        id: items[0].id,
        out_dir: None,
    };
    call(&reader, open).await;
    assert_eq!(
        reader.blob_class(&photo.blob).unwrap(),
        Some(BlobClass::Cache)
    );
    let used = peer(&reader, honest_public.public())
        .await
        .expect("a record of the host");
    assert!(used.counts.received_bytes > 0.0, "{used:?}");
    assert!(
        peer(&reader, tampered_public.public()).await.is_none(),
        "the tampered host was used"
    );
    assert!(peer(&reader, tampered.public()).await.is_none());
}

/// Over Tor, someone opening media doesn't wait for a first connection to a
/// node that may name other hosts: it's readied for next time, and the media
/// comes meanwhile. Off Tor the same node would be asked.
#[tokio::test(flavor = "multi_thread")]
async fn opening_media_doesnt_wait_for_a_first_connection_over_tor() {
    let dir = tempfile::tempdir().unwrap();
    let author = start(dir.path(), Config::local_test("Author")).await;
    let mut reader_config = Config::local_test("Reader");
    reader_config.sync.prefetch = false;
    let quick = TorDelay {
        first_connect: (Duration::ZERO, Duration::ZERO),
        round_trip: (Duration::ZERO, Duration::ZERO),
        bytes_per_sec: 10_000_000,
    };
    let sim = SimSettings {
        tor_delay: Some(quick),
        ..SimSettings::default()
    };
    let reader = start_sim(dir.path(), reader_config, sim).await;
    let photo = post_photo(&author, dir.path(), true).await;
    follow(&reader, &author).await;
    assert_eq!(reader.blob_class(&photo.blob).unwrap(), None);
    let ClientResponse::Feed { items } = call(&reader, ClientRequest::Feed { limit: 1 }).await
    else {
        panic!("no feed");
    };
    let id = items[0].id;

    // After a restart there's no connection, and a first one takes minutes.
    let home = dir.path().join("reader");
    reader.shutdown().await.unwrap();
    let slow = TorDelay {
        first_connect: (Duration::from_secs(300), Duration::from_secs(300)),
        ..quick
    };
    let sim = SimSettings {
        tor_delay: Some(slow),
        ..SimSettings::default()
    };
    let reader = Node::spawn_sim(&home, sim).await.unwrap();
    // Asking would take the whole of `ASK_FIRST_WAIT`, 5 s, and leave no other
    // trace: it's cut off before there's an answer to record. The photo takes
    // moments on loopback.
    let open = ClientRequest::Fetch { id, out_dir: None };
    tokio::time::timeout(Duration::from_millis(4500), call(&reader, open))
        .await
        .expect("it waited for a first connection");
    assert_eq!(
        reader.blob_class(&photo.blob).unwrap(),
        Some(BlobClass::Cache)
    );
}

/// Off Tor, a node reached the usual way is asked as before.
#[tokio::test(flavor = "multi_thread")]
async fn off_tor_opening_media_asks_the_followed_node_first() {
    let dir = tempfile::tempdir().unwrap();
    let author = start(dir.path(), Config::local_test("Author")).await;
    let mut reader_config = Config::local_test("Reader");
    reader_config.sync.prefetch = false;
    let reader = start(dir.path(), reader_config).await;
    let photo = post_photo(&author, dir.path(), true).await;
    follow(&reader, &author).await;
    let ClientResponse::Feed { items } = call(&reader, ClientRequest::Feed { limit: 1 }).await
    else {
        panic!("no feed");
    };
    let asked = async || {
        peer(&reader, author.node_id())
            .await
            .map_or(0.0, |p| p.counts.requests.round())
    };
    let before = asked().await;
    let open = ClientRequest::Fetch {
        id: items[0].id,
        out_dir: None,
    };
    call(&reader, open).await;
    assert_eq!(
        reader.blob_class(&photo.blob).unwrap(),
        Some(BlobClass::Cache)
    );
    assert!(asked().await > before);
}
