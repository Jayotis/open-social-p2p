//! The media and direct endpoints (ADR 0014, build step 2), with a real relay
//! on loopback: bindings are checked, and a node the direct endpoint refuses
//! learns no address from it.
//!
//! What matters can't be seen from the node's side, so this also watches what
//! iroh's client side is told: `told_of` counts the addresses a node registered
//! for an endpoint. An allowed node is told, so the count is known to work.

use std::{
    io::Write,
    net::Ipv4Addr,
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr,
    endpoint::{ConnectOptions, Connection, ConnectionError, VarInt, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use osp_node::{Config, DirectRule, Node, RelayChoice};
use osp_proto::{
    BlobHash, BoundEndpoint, CLIENT_ALPN, EndpointBinding, EndpointKind, ObjectId, PairingTicket,
    SOCIAL_ALPN,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse},
    social::{SocialRequest, SocialResponse},
};

/// Lines of iroh's log saying a node was told of an address.
static TOLD: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Clone)]
struct Capture;

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let line = String::from_utf8_lossy(buf);
        if line.contains("AddressAdded") {
            TOLD.lock().unwrap().push(line.into_owned());
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
            .with_env_filter("iroh::socket::remote_map=trace")
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

/// Long enough for addresses sent at once to have arrived, on loopback.
const SETTLE: Duration = Duration::from_millis(500);

/// The code a node refused after the handshake is told.
const NOT_ALLOWED: VarInt = VarInt::from_u32(403);

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

fn config(name: &str, relay: &RelayUrl, rule: DirectRule) -> Config {
    let mut config = Config::local_test(name);
    config.network.relay = RelayChoice::Custom(vec![relay.to_string()]);
    config.endpoints.enabled = true;
    config.endpoints.direct = rule;
    config
}

async fn start(root: &Path, config: Config) -> Node {
    let home = root.join(config.name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn(&home).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

/// An endpoint of a node on nobody's list, with IP sockets and the relay, as
/// someone trying to find a node's address would have.
async fn stranger(relay: &RelayUrl) -> Endpoint {
    endpoint_with(relay, SecretKey::generate()).await
}

async fn endpoint_with(relay: &RelayUrl, key: SecretKey) -> Endpoint {
    Endpoint::builder(presets::N0)
        .clear_address_lookup()
        .relay_mode(RelayMode::custom([relay.clone()]))
        .secret_key(key)
        .bind()
        .await
        .unwrap()
}

/// `addr` with only its relay, as anyone holding an endpoint's ID could find.
fn through_relay(addr: &EndpointAddr, relay: &RelayUrl) -> EndpointAddr {
    EndpointAddr::from_parts(addr.id, [TransportAddr::Relay(relay.clone())])
}

async fn direct_addr(node: &Node, relay: &RelayUrl) -> EndpointAddr {
    through_relay(&node.direct_addr().await.unwrap(), relay)
}

async fn assert_refused(from: &Endpoint, addr: EndpointAddr, alpn: &[u8]) {
    let id = addr.id;
    let tried = tokio::time::timeout(Duration::from_secs(10), from.connect(addr, alpn)).await;
    let error = match tried {
        Ok(Ok(_)) => panic!("connected"),
        Ok(Err(e)) => format!("{e:#}"),
        Err(_) => panic!("neither refused nor connected"),
    };
    assert!(error.contains("refused"), "{error}");
    tokio::time::sleep(SETTLE).await;
    assert_eq!(told_of(&id), 0, "told of an address");
}

/// What a node that already has an IP address gets: its key isn't known before
/// the handshake, so its side may connect, and the connection then closes with
/// the code for not being allowed.
async fn assert_closed_on(tried: Result<Connection, impl std::fmt::Debug>) {
    let Ok(conn) = tried else { return };
    let closed = tokio::time::timeout(Duration::from_secs(10), conn.closed())
        .await
        .expect("the connection stayed open");
    let ConnectionError::ApplicationClosed(close) = &closed else {
        panic!("closed by something else: {closed:?}");
    };
    assert_eq!(close.error_code, NOT_ALLOWED);
}

/// Posts a small photo, which anyone may fetch, and returns its hash and size.
async fn post_photo(node: &Node, dir: &Path) -> (BlobHash, u64) {
    let photo = dir.join("photo.png");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc=size=320x240"])
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let req = ClientRequest::Post {
        content: "a photo".into(),
        attachments: vec![photo],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, req).await else {
        panic!("not posted")
    };
    let id: ObjectId = id;
    let ClientResponse::Item { item } = call(node, ClientRequest::Show { id }).await else {
        panic!("not shown")
    };
    let attachment = &item.attachments[0].attachment;
    (attachment.blob, attachment.size)
}

async fn fetch_on(conn: Connection, hash: BlobHash) -> usize {
    let hash = iroh_blobs::Hash::from_bytes(*hash.as_bytes());
    iroh_blobs::get::request::get_blob(conn, hash)
        .bytes()
        .await
        .unwrap()
        .len()
}

/// Asks for a pairing ticket.
async fn pairing(node: &Node) -> PairingTicket {
    let ClientResponse::Pairing { ticket, .. } = call(node, ClientRequest::NewPairing).await else {
        panic!("no pairing ticket");
    };
    ticket.parse().unwrap()
}

/// Makes a request the way a phone does.
async fn phone_request(conn: &Connection, req: &ClientRequest) -> ClientResponse {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, req).await.unwrap();
    send.finish().unwrap();
    read_frame(&mut recv).await.unwrap()
}

/// Connects and pairs the way a phone does, offering the ticket's pass.
async fn pair_phone(phone: &Endpoint, ticket: &PairingTicket, addr: EndpointAddr) -> Connection {
    let options = ConnectOptions::new().with_additional_alpns(vec![ticket.pass()]);
    let conn = phone
        .connect_with_opts(addr, CLIENT_ALPN, options)
        .await
        .unwrap()
        .await
        .unwrap();
    assert_eq!(conn.alpn(), CLIENT_ALPN, "the pass isn't a protocol");
    let req = ClientRequest::Pair {
        secret: data_encoding::HEXLOWER.encode(&ticket.secret),
        name: "Test phone".into(),
    };
    let resp = phone_request(&conn, &req).await;
    assert!(matches!(resp, ClientResponse::Paired { .. }), "{resp:?}");
    conn
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_holding_the_direct_id_is_refused_and_learns_no_address() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), config("Node", &url, DirectRule::Nobody)).await;
    let addr = direct_addr(&node, &url).await;

    let stranger = stranger(&url).await;
    assert_refused(&stranger, addr.clone(), iroh_blobs::ALPN).await;
    assert_refused(&stranger, addr.clone(), CLIENT_ALPN).await;
    let info = stranger.remote_info(addr.id).await;
    let ips = info
        .iter()
        .flat_map(|i| i.addrs())
        .filter(|a| a.addr().is_ip());
    assert_eq!(ips.count(), 0, "it has an IP address for the node");

    node.shutdown().await.unwrap();
}

/// So the test above isn't passing for want of anything to tell: a node let
/// through is told the addresses, however it's let through.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_let_through_is_told_the_addresses() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), config("Node", &url, DirectRule::Anyone)).await;
    let addr = direct_addr(&node, &url).await;

    let anyone = stranger(&url).await;
    let conn = anyone
        .connect(addr.clone(), iroh_blobs::ALPN)
        .await
        .unwrap();
    let started = Instant::now();
    while told_of(&addr.id) == 0 {
        assert!(started.elapsed() < Duration::from_secs(10), "told nothing");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(conn);
    node.shutdown().await.unwrap();
}

/// Someone who found an IP address elsewhere comes straight to it. Its key
/// isn't known before the handshake, so by then it may have been told the
/// node's other addresses (ADR 0014). It gets nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_who_has_an_ip_address_gets_nothing_from_it() {
    capture_logs();
    let dir = tempfile::tempdir().unwrap();
    // Without relays, the IP address is all a node hands out.
    let mut config = Config::local_test("Node");
    config.endpoints.enabled = true;
    let node = start(dir.path(), config).await;
    let addr = node.direct_addr().await.unwrap();
    assert!(addr.ip_addrs().count() > 0);
    let stranger = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();

    let tried = stranger.connect(addr.clone(), iroh_blobs::ALPN).await;
    assert_closed_on(tried).await;
    let tried = stranger.connect(addr.clone(), CLIENT_ALPN).await;
    assert_closed_on(tried).await;

    // While a phone might be pairing at home, it may try that, and only that.
    pairing(&node).await;
    let conn = stranger.connect(addr.clone(), CLIENT_ALPN).await.unwrap();
    let resp = phone_request(&conn, &ClientRequest::Status).await;
    assert!(matches!(resp, ClientResponse::Error { .. }), "{resp:?}");
    conn.close(0u32.into(), b"done");
    let tried = stranger.connect(addr, iroh_blobs::ALPN).await;
    assert_closed_on(tried).await;

    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_pairs_through_a_relay_with_its_tickets_pass_and_then_needs_none() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), config("Node", &url, DirectRule::Nobody)).await;
    let ticket = pairing(&node).await;
    let direct = node.direct_addr().await.unwrap();
    assert_eq!(
        ticket.node.id, direct.id,
        "the ticket names the direct endpoint"
    );
    let addr = through_relay(&direct, &url);

    // While a ticket waits, nobody gets in without it.
    assert_refused(&stranger(&url).await, addr.clone(), CLIENT_ALPN).await;
    let forged = PairingTicket {
        node: addr.clone(),
        secret: [9; 16],
    };
    let options = ConnectOptions::new().with_additional_alpns(vec![forged.pass()]);
    let forger = stranger(&url).await;
    let tried = forger
        .connect_with_opts(addr.clone(), CLIENT_ALPN, options)
        .await
        .unwrap()
        .await;
    assert!(tried.is_err(), "a made-up pass got in");
    assert_eq!(told_of(&addr.id), 0);

    let key = SecretKey::generate();
    let phone = endpoint_with(&url, key.clone()).await;
    let conn = pair_phone(&phone, &ticket, addr.clone()).await;
    let resp = phone_request(&conn, &ClientRequest::Status).await;
    assert!(matches!(resp, ClientResponse::Status(_)), "{resp:?}");
    conn.close(0u32.into(), b"done");

    // Paired, it's known by its key, with no pass.
    let conn = phone.connect(addr.clone(), CLIENT_ALPN).await.unwrap();
    let resp = phone_request(&conn, &ClientRequest::Status).await;
    assert!(matches!(resp, ClientResponse::Status(_)), "{resp:?}");

    // The ticket is used now, so its pass opens nothing.
    let late = stranger(&url).await;
    let options = ConnectOptions::new().with_additional_alpns(vec![ticket.pass()]);
    let tried = late
        .connect_with_opts(addr.clone(), CLIENT_ALPN, options)
        .await
        .unwrap()
        .await;
    assert!(tried.is_err(), "a used pass got in");

    // Unpaired, it's a stranger again.
    let req = ClientRequest::Unpair {
        device: key.public(),
    };
    call(&node, req).await;
    drop(conn);
    let phone = endpoint_with(&url, key).await;
    let tried = phone.connect(addr, CLIENT_ALPN).await;
    assert!(tried.is_err(), "an unpaired phone got in");

    node.shutdown().await.unwrap();
}

/// Two nodes that allow each other get each other's direct endpoints, and
/// one's is let through to the other's, on which media is served.
#[tokio::test(flavor = "multi_thread")]
async fn nodes_that_allow_each_other_connect_directly() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let ada = start(dir.path(), config("Ada", &url, DirectRule::Approved)).await;
    let bea = start(dir.path(), config("Bea", &url, DirectRule::Approved)).await;
    let (hash, size) = post_photo(&bea, dir.path()).await;
    let bea_addr = bea.addr().await;
    let allow = |node: EndpointId| ClientRequest::AllowDirect { node };
    call(&ada, allow(bea.node_id())).await;
    call(&bea, allow(ada.node_id())).await;

    let found = ada.ask_endpoints(&bea_addr).await.unwrap();
    let direct = found.direct.expect("Bea's rule allows Ada");
    assert_eq!(direct.id, bea.direct_addr().await.unwrap().id);
    assert_eq!(found.media.unwrap().id, bea.media_addr().await.unwrap().id);

    let addr = through_relay(&direct, &url);
    let conn = ada
        .connect_from_direct(addr.clone(), iroh_blobs::ALPN)
        .await
        .unwrap();
    assert_eq!(fetch_on(conn.clone(), hash).await as u64, size);
    tokio::time::sleep(SETTLE).await;
    assert!(told_of(&addr.id) > 0, "told of Bea's addresses");

    // Taken back, the connection closes. Ada knows Bea's address by now, so
    // her next try may go straight to it, and be refused there.
    let req = ClientRequest::RemoveDirect {
        node: ada.node_id(),
    };
    call(&bea, req).await;
    let closed = tokio::time::timeout(Duration::from_secs(10), conn.closed()).await;
    assert!(closed.is_ok(), "the connection stayed open");
    let tried = ada.connect_from_direct(addr, iroh_blobs::ALPN).await;
    assert_closed_on(tried).await;

    ada.shutdown().await.unwrap();
    bea.shutdown().await.unwrap();
}

/// A direct connection takes both sides' rules, whichever side refuses.
#[tokio::test(flavor = "multi_thread")]
async fn one_sides_rule_isnt_enough() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let ada = start(dir.path(), config("Ada", &url, DirectRule::Approved)).await;
    let bea = start(dir.path(), config("Bea", &url, DirectRule::Approved)).await;
    let bea_addr = bea.addr().await;
    let addr = direct_addr(&bea, &url).await;
    let allow = |node: EndpointId| ClientRequest::AllowDirect { node };

    // Only Ada allows: Bea hands over nothing and keeps nothing of Ada's.
    call(&ada, allow(bea.node_id())).await;
    let found = ada.ask_endpoints(&bea_addr).await.unwrap();
    assert_eq!(found.direct, None);
    assert!(found.media.is_some(), "media goes to anyone");
    let tried = ada
        .connect_from_direct(addr.clone(), iroh_blobs::ALPN)
        .await;
    assert!(tried.is_err(), "Bea's rule doesn't allow Ada");

    // Only Bea allows: Ada offers nothing, and ignores what Bea hands over.
    let req = ClientRequest::RemoveDirect {
        node: bea.node_id(),
    };
    call(&ada, req).await;
    call(&bea, allow(ada.node_id())).await;
    let found = ada.ask_endpoints(&bea_addr).await.unwrap();
    assert_eq!(found.direct, None);
    let tried = ada.connect_from_direct(addr, iroh_blobs::ALPN).await;
    assert!(tried.is_err(), "Bea has no direct endpoint of Ada's");

    ada.shutdown().await.unwrap();
    bea.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_rule_says_who_is_handed_the_direct_endpoint() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let asking = start(dir.path(), config("Asking", &url, DirectRule::Anyone)).await;

    // Nobody: not even someone it follows.
    let nobody = start(dir.path(), config("Nobody", &url, DirectRule::Nobody)).await;
    let found = asking.ask_endpoints(&nobody.addr().await).await.unwrap();
    assert_eq!(found.direct, None);
    assert!(found.media.is_some());

    // Anyone: a node it has never heard of.
    let anyone = start(dir.path(), config("Anyone", &url, DirectRule::Anyone)).await;
    let found = asking.ask_endpoints(&anyone.addr().await).await.unwrap();
    assert!(found.direct.is_some());

    // People it follows: being followed isn't enough, following is.
    let follows = start(dir.path(), config("Follows", &url, DirectRule::Follows)).await;
    let follows_addr = follows.addr().await;
    let found = asking.ask_endpoints(&follows_addr).await.unwrap();
    assert_eq!(found.direct, None, "it doesn't follow the asker");
    let ClientResponse::Ticket { ticket } = call(&follows, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(&asking, ClientRequest::Follow { ticket }).await;
    let found = asking.ask_endpoints(&follows_addr).await.unwrap();
    assert_eq!(found.direct, None, "the asker following it changes nothing");
    let ClientResponse::Ticket { ticket } = call(&asking, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(&follows, ClientRequest::Follow { ticket }).await;
    let found = asking.ask_endpoints(&follows_addr).await.unwrap();
    assert!(found.direct.is_some());

    for node in [asking, nobody, anyone, follows] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_media_endpoint_serves_anyone_and_tells_no_address() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), config("Node", &url, DirectRule::Nobody)).await;
    let (hash, size) = post_photo(&node, dir.path()).await;
    let addr = node.media_addr().await.unwrap();
    assert_eq!(addr.ip_addrs().count(), 0);

    let stranger = stranger(&url).await;
    let conn = stranger
        .connect(addr.clone(), iroh_blobs::ALPN)
        .await
        .unwrap();
    assert_eq!(fetch_on(conn, hash).await as u64, size);
    tokio::time::sleep(SETTLE).await;
    assert_eq!(told_of(&addr.id), 0, "told of an address");

    node.shutdown().await.unwrap();
}

/// Fail closed (ADR 0014): if Tor is down, messages wait. They never go on
/// these endpoints, and nor does the phones' protocol on the media one.
#[tokio::test(flavor = "multi_thread")]
async fn messages_between_nodes_go_on_neither_endpoint() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), config("Node", &url, DirectRule::Anyone)).await;
    let anyone = stranger(&url).await;

    let direct = direct_addr(&node, &url).await;
    let tried = anyone.connect(direct, SOCIAL_ALPN).await;
    assert!(tried.is_err(), "the direct endpoint took messages");
    let media = node.media_addr().await.unwrap();
    let tried = anyone.connect(media.clone(), SOCIAL_ALPN).await;
    assert!(tried.is_err(), "the media endpoint took messages");
    let tried = anyone.connect(media, CLIENT_ALPN).await;
    assert!(tried.is_err(), "the media endpoint took phones");

    node.shutdown().await.unwrap();
}

async fn ask(conn: &Connection, req: &SocialRequest) -> SocialResponse {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, req).await.unwrap();
    send.finish().unwrap();
    read_frame(&mut recv).await.unwrap()
}

/// A node offering a direct endpoint has it checked before it's let through.
#[tokio::test(flavor = "multi_thread")]
async fn an_offered_endpoint_is_checked() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), config("Node", &url, DirectRule::Approved)).await;
    let addr = direct_addr(&node, &url).await;

    let public = SecretKey::generate();
    let direct_key = SecretKey::generate();
    let other = SecretKey::generate();
    let peer = endpoint_with(&url, public.clone()).await;
    let req = ClientRequest::AllowDirect {
        node: public.public(),
    };
    call(&node, req).await;
    let conn = peer.connect(node.addr().await, SOCIAL_ALPN).await.unwrap();

    let bound = |binding: EndpointBinding, id: EndpointId| BoundEndpoint {
        binding,
        addr: EndpointAddr::new(id),
    };
    let now = osp_proto::parse_rfc3339_ms(&osp_proto::now_rfc3339()).unwrap() as u64;
    let direct = EndpointKind::Direct;
    let forged = [
        // Another node's, so not this peer's to offer.
        bound(
            EndpointBinding::new(&other, &direct_key, direct, now),
            direct_key.public(),
        ),
        // A media endpoint passed off as a direct one.
        bound(
            EndpointBinding::new(&public, &direct_key, EndpointKind::Media, now),
            direct_key.public(),
        ),
        // Another endpoint's address.
        bound(
            EndpointBinding::new(&public, &direct_key, direct, now),
            other.public(),
        ),
        // Dated so far ahead it could never be replaced.
        bound(
            EndpointBinding::new(&public, &direct_key, direct, now + 3_600_000),
            direct_key.public(),
        ),
    ];
    let from_direct = endpoint_with(&url, direct_key.clone()).await;
    for bound in forged {
        let req = SocialRequest::Endpoints {
            direct: Some(bound.clone()),
        };
        let resp = ask(&conn, &req).await;
        assert!(matches!(resp, SocialResponse::Error { .. }), "{bound:?}");
        assert_refused(&from_direct, addr.clone(), iroh_blobs::ALPN).await;
    }

    // The real thing is let through.
    let req = SocialRequest::Endpoints {
        direct: Some(bound(
            EndpointBinding::new(&public, &direct_key, direct, now),
            direct_key.public(),
        )),
    };
    let SocialResponse::Endpoints { direct: handed, .. } = ask(&conn, &req).await else {
        panic!("refused");
    };
    assert_eq!(handed.unwrap().addr.id, addr.id);
    let conn = from_direct.connect(addr, iroh_blobs::ALPN).await.unwrap();
    conn.close(0u32.into(), b"done");

    node.shutdown().await.unwrap();
}

/// Under `anyone`, nothing needs to know whose an endpoint is, and anyone can
/// make new keys, so offers aren't kept: they'd fill the table.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_that_allows_anyone_keeps_no_offers() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("node");
    let settings = config("Node", &url, DirectRule::Anyone);
    Node::init(&home, &settings).unwrap();
    let node = Node::spawn(&home).await.unwrap();

    let public = SecretKey::generate();
    let direct_key = SecretKey::generate();
    let peer = endpoint_with(&url, public.clone()).await;
    let conn = peer.connect(node.addr().await, SOCIAL_ALPN).await.unwrap();
    let now = osp_proto::parse_rfc3339_ms(&osp_proto::now_rfc3339()).unwrap() as u64;
    let req = SocialRequest::Endpoints {
        direct: Some(BoundEndpoint {
            binding: EndpointBinding::new(&public, &direct_key, EndpointKind::Direct, now),
            addr: EndpointAddr::new(direct_key.public()),
        }),
    };
    let resp = ask(&conn, &req).await;
    assert!(matches!(resp, SocialResponse::Endpoints { .. }), "{resp:?}");
    conn.close(0u32.into(), b"done");
    node.shutdown().await.unwrap();

    // Had it kept the offer, this rule would now let that endpoint through.
    let settings = config("Node", &url, DirectRule::Approved);
    settings.save(&home.join("config.toml")).unwrap();
    let node = Node::spawn(&home).await.unwrap();
    let req = ClientRequest::AllowDirect {
        node: public.public(),
    };
    call(&node, req).await;
    let addr = direct_addr(&node, &url).await;
    let from_direct = endpoint_with(&url, direct_key).await;
    assert_refused(&from_direct, addr, iroh_blobs::ALPN).await;

    node.shutdown().await.unwrap();
}

/// Answers `Endpoints` with whatever it's given.
#[derive(Debug, Clone)]
struct Answers(BoundEndpoint);

impl ProtocolHandler for Answers {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            if read_frame::<_, SocialRequest>(&mut recv).await.is_ok() {
                let resp = SocialResponse::Endpoints {
                    media: Some(Box::new(self.0.clone())),
                    direct: None,
                };
                write_frame(&mut send, &resp).await.ok();
                send.finish().ok();
            }
        }
        Ok(())
    }
}

/// A node can't name another's endpoint as its own in its reply either.
#[tokio::test(flavor = "multi_thread")]
async fn endpoints_in_a_reply_are_checked() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), config("Node", &url, DirectRule::Nobody)).await;

    let other = SecretKey::generate();
    let media = SecretKey::generate();
    let liar = endpoint_with(&url, SecretKey::generate()).await;
    let reply = BoundEndpoint {
        binding: EndpointBinding::new(&other, &media, EndpointKind::Media, 1_000),
        addr: EndpointAddr::new(media.public()),
    };
    let _router = Router::builder(liar.clone())
        .accept(SOCIAL_ALPN, Answers(reply))
        .spawn();

    let error = node
        .ask_endpoints(&through_relay(&liar.addr(), &url))
        .await
        .expect_err("a binding for another node was taken");
    assert!(format!("{error:#}").contains("another node"), "{error:#}");

    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn endpoints_keep_their_keys_and_are_off_unless_turned_on() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();

    let off = start(dir.path(), Config::local_test("Off")).await;
    assert_eq!(off.direct_addr().await, None);
    assert_eq!(off.media_addr().await, None);
    let ClientResponse::Status(status) = call(&off, ClientRequest::Status).await else {
        panic!("no status");
    };
    assert_eq!(status.endpoints, None);
    off.shutdown().await.unwrap();
    assert!(!dir.path().join("off/direct.key").exists());

    let on = config("On", &url, DirectRule::Follows);
    let home = dir.path().join("on");
    Node::init(&home, &on).unwrap();
    let node = Node::spawn(&home).await.unwrap();
    let direct = node.direct_addr().await.unwrap().id;
    let media = node.media_addr().await.unwrap().id;
    let ClientResponse::Status(status) = call(&node, ClientRequest::Status).await else {
        panic!("no status");
    };
    let own = status.endpoints.unwrap();
    assert_eq!((own.direct, own.media), (direct, Some(media)));
    assert_eq!(own.rule, "people you follow");
    assert_ne!(direct, node.node_id());
    node.shutdown().await.unwrap();

    let node = Node::spawn(&home).await.unwrap();
    assert_eq!(node.direct_addr().await.unwrap().id, direct);
    assert_eq!(node.media_addr().await.unwrap().id, media);
    node.shutdown().await.unwrap();
}

/// The same as the tests above on n0's public relays, as a relay on loopback
/// might not behave as they do. Needs the internet.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses n0's public relays"]
async fn on_public_relays() {
    capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let real = |name: &str, rule| {
        let mut config = Config::local_test(name);
        config.network.relay = RelayChoice::Default;
        config.network.address_lookup = false;
        config.network.bind = None;
        config.endpoints.enabled = true;
        config.endpoints.direct = rule;
        config
    };
    let node = start(dir.path(), real("Node", DirectRule::Nobody)).await;
    let addr = node.direct_addr().await.unwrap();
    assert_eq!(addr.relay_urls().count(), 1, "{addr:?}");

    let online = async |key: SecretKey| {
        let endpoint = Endpoint::builder(presets::N0)
            .clear_address_lookup()
            .secret_key(key)
            .bind()
            .await
            .unwrap();
        endpoint.online().await;
        endpoint
    };
    let started = Instant::now();
    assert_refused(
        &online(SecretKey::generate()).await,
        addr.clone(),
        CLIENT_ALPN,
    )
    .await;
    println!(
        "a stranger was refused, told of nothing, in {:?}",
        started.elapsed()
    );

    // A phone with the ticket's pass gets through, and is told.
    let ticket = pairing(&node).await;
    let phone = online(SecretKey::generate()).await;
    let conn = pair_phone(
        &phone,
        &ticket,
        through_relay(&addr, addr.relay_urls().next().unwrap()),
    )
    .await;
    let resp = phone_request(&conn, &ClientRequest::Status).await;
    assert!(matches!(resp, ClientResponse::Status(_)), "{resp:?}");
    let started = Instant::now();
    while told_of(&addr.id) == 0 {
        assert!(started.elapsed() < Duration::from_secs(30), "told nothing");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    println!("a paired phone was told of {} addresses", told_of(&addr.id));

    // The media endpoint serves a stranger through the relay.
    let (hash, size) = post_photo(&node, dir.path()).await;
    let media = node.media_addr().await.unwrap();
    let stranger = online(SecretKey::generate()).await;
    let conn = stranger
        .connect(media.clone(), iroh_blobs::ALPN)
        .await
        .unwrap();
    assert_eq!(fetch_on(conn, hash).await as u64, size);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(told_of(&media.id), 0, "told of an address");

    node.shutdown().await.unwrap();
}
