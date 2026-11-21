//! Every node carries (ADR 0043, stage B part 4a): its machine record names
//! its carrier in a signed descriptor and says when its machine key started,
//! and a circuit goes through the node's carrier. A host keeps a link to a
//! seed carrier and names it to viewers (part 4b).
//!
//! Kim's node is the carrier here. Machine names run on the simulated
//! network, which has no Tor; the carrier endpoint uses a real relay on
//! loopback, as it would on the internet.

use std::{net::Ipv4Addr, path::Path, time::Duration};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr,
    endpoint::presets,
};
use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use osp_carrier::{ALPN, CarrierInfo, Descriptor, HostEvent, HostLink, ViewerCircuit};
use osp_node::{
    Config, Node, RelayChoice, SeedCarrier, SimSettings,
    simnet::{self, Link, SimNet},
};
use osp_proto::{
    BlobHash, MACHINE_ALPN, ObjectId,
    framing::{read_frame, write_frame},
    machine::{MachineInfo, MachineRecord, MachineRequest, MachineResponse},
    rpc::{ClientRequest, ClientResponse},
    social::{CircuitOffer, MEDIA_HOSTS_ALPN, MediaHostsRequest, MediaHostsResponse},
};
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(30);

/// A plain-http relay on loopback. Keep the server; dropping it stops it.
async fn local_relay() -> (Server, RelayUrl) {
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new((Ipv4Addr::LOCALHOST, 0)));
    let server = Server::spawn(config).await.unwrap();
    let url = format!("http://{}", server.http_addr().unwrap())
        .parse()
        .unwrap();
    (server, url)
}

fn sim(net: &SimNet) -> SimSettings {
    SimSettings {
        network: Some(net.link(Link::default())),
        machine_names: true,
        ..SimSettings::default()
    }
}

async fn start(home: &Path, net: &SimNet) -> Node {
    Node::spawn_sim(home, sim(net)).await.unwrap()
}

fn config(name: &str, relay: &RelayUrl) -> Config {
    let mut config = Config::local_test(name);
    config.network.relay = RelayChoice::Custom(vec![relay.to_string()]);
    config
}

/// Someone on the simulated network, to greet machines.
async fn stranger(net: &SimNet) -> (Endpoint, SecretKey) {
    let key = SecretKey::generate();
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .add_custom_transport(net.link(Link::default()).transport(key.public()))
        .secret_key(key.clone())
        .bind()
        .await
        .unwrap();
    (endpoint, key)
}

/// What `machine` says of itself when greeted.
async fn greet(me: &(Endpoint, SecretKey), machine: EndpointId) -> MachineInfo {
    let addr = EndpointAddr::from_parts(machine, [TransportAddr::Custom(simnet::addr(machine))]);
    let conn = me.0.connect(addr, MACHINE_ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let record = MachineRecord::new(&me.1, vec![], osp_proto::now_rfc3339()).unwrap();
    write_frame(&mut send, &MachineRequest::Hello { record })
        .await
        .unwrap();
    send.finish().unwrap();
    let reply: MachineResponse = read_frame(&mut recv).await.unwrap();
    conn.close(0u32.into(), b"done");
    let MachineResponse::Hello { record } = reply else {
        panic!("unexpected {reply:?}");
    };
    record.verify().unwrap()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The descriptor in `machine`'s record, once its carrier is running.
async fn carrier_of(me: &(Endpoint, SecretKey), machine: EndpointId) -> (Descriptor, CarrierInfo) {
    timeout(WAIT, async {
        loop {
            if let Some(text) = greet(me, machine).await.carrier {
                let bytes = data_encoding::BASE64URL_NOPAD
                    .decode(text.as_bytes())
                    .unwrap();
                let descriptor = Descriptor::from_bytes(&bytes).unwrap();
                let info = descriptor.verify(now()).unwrap();
                return (descriptor, info);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("the record never named a carrier")
}

/// A loopback endpoint on the relay, as a viewer's or a host's would be.
async fn end(relay: &RelayUrl) -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(RelayMap::from_iter([relay.clone()])))
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap()
}

/// Reaches the carrier a descriptor names, by its ID through its relay, as
/// a node does.
async fn link(from: &Endpoint, info: &CarrierInfo) -> iroh::endpoint::Connection {
    let relay: RelayUrl = info.relay.parse().unwrap();
    let addr = EndpointAddr::new(info.endpoint).with_relay_url(relay);
    timeout(WAIT, from.connect(addr, ALPN))
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_carries_under_its_machine_name_and_keeps_its_start() {
    let dir = tempfile::tempdir().unwrap();
    let (_relay, relay) = local_relay().await;
    let net = SimNet::new(1);
    let home = dir.path().join("kim");
    Node::init(&home, &config("Kim", &relay)).unwrap();
    let kim = start(&home, &net).await;
    let machine = kim.machine_id().unwrap();
    let me = stranger(&net).await;

    // Its record names its carrier, signed by its machine key, on its relay,
    // on an endpoint that is none of the node's others.
    let (descriptor, info) = carrier_of(&me, machine).await;
    assert_eq!(info.machine, machine);
    assert_eq!(info.relay.parse::<RelayUrl>().unwrap(), relay);
    assert!(!kim.endpoint_ids().contains(&info.endpoint));
    let started = greet(&me, machine).await.started_at.unwrap();
    assert!(started.abs_diff(now()) < 60, "{started}");

    // A packet each way through it, V → Kim → H.
    let host = end(&relay).await;
    let mut hosted = HostLink::open(link(&host, &info).await).await.unwrap();
    let viewer = end(&relay).await;
    let mut circuit = ViewerCircuit::build(
        link(&viewer, &info).await,
        std::slice::from_ref(&descriptor),
        hosted.slot(),
    )
    .await
    .unwrap();
    let opened = match timeout(WAIT, hosted.recv()).await.unwrap().unwrap() {
        HostEvent::Opened(id) => id,
        other => panic!("expected the circuit to open, got {other:?}"),
    };
    let packet = vec![7; 1200];
    circuit.send(packet.clone()).unwrap();
    match timeout(WAIT, hosted.recv()).await.unwrap().unwrap() {
        HostEvent::Packet(id, got) => assert_eq!((id, got), (opened, packet)),
        other => panic!("expected a packet, got {other:?}"),
    }
    hosted.send(opened, vec![9; 1200]).unwrap();
    assert_eq!(
        timeout(WAIT, circuit.recv()).await.unwrap().unwrap(),
        vec![9; 1200]
    );
    drop((circuit, hosted));
    kim.shutdown().await.unwrap();

    // After a restart: the same start, and the same carrier endpoint.
    let kim = start(&home, &net).await;
    let (_, again) = carrier_of(&me, machine).await;
    assert_eq!(again.endpoint, info.endpoint);
    assert_eq!(greet(&me, machine).await.started_at, Some(started));
    kim.shutdown().await.unwrap();

    // A new machine name starts again: a new start, and a new carrier
    // endpoint, so nothing ties it to the old name.
    std::fs::write(home.join("machine.started"), "1700000000\n").unwrap();
    Node::forget_machine_name(&home).unwrap();
    let kim = start(&home, &net).await;
    let renamed = kim.machine_id().unwrap();
    assert_ne!(renamed, machine);
    let (_, fresh) = carrier_of(&me, renamed).await;
    assert_ne!(fresh.endpoint, info.endpoint);
    let restarted = greet(&me, renamed).await.started_at.unwrap();
    assert!(restarted.abs_diff(now()) < 60, "{restarted}");
    kim.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_without_a_relay_or_with_carrying_off_names_no_carrier() {
    let dir = tempfile::tempdir().unwrap();
    let (_relay, relay) = local_relay().await;
    let net = SimNet::new(2);
    let me = stranger(&net).await;

    let mut off = config("Off", &relay);
    off.carrier.enabled = false;
    for (name, config) in [("none", Config::local_test("None")), ("off", off)] {
        let home = dir.path().join(name);
        Node::init(&home, &config).unwrap();
        let node = start(&home, &net).await;
        let machine = node.machine_id().unwrap();
        // Long enough for a carrier to have named itself.
        tokio::time::sleep(Duration::from_secs(6)).await;
        let info = greet(&me, machine).await;
        assert_eq!(info.carrier, None, "{name}");
        assert!(info.started_at.is_some(), "{name}");
        assert!(!home.join("carrier.key").exists(), "{name}");
        node.shutdown().await.unwrap();
    }
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

/// A small photo, posted.
async fn post_photo(node: &Node, dir: &Path) -> (ObjectId, BlobHash) {
    let photo = dir.join("photo.png");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc=s=64x64", "-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let req = ClientRequest::Post {
        content: "Alice's photo".into(),
        attachments: vec![photo],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, req).await else {
        panic!("not posted")
    };
    let ClientResponse::Item { item } = call(node, ClientRequest::Show { id }).await else {
        panic!("not shown")
    };
    (id, item.attachments[0].attachment.blob)
}

/// What `host`'s media endpoint answers a stranger asking who hosts `blob`.
async fn media_hosts(
    me: &(Endpoint, SecretKey),
    host: &Node,
    (post, blob): (ObjectId, BlobHash),
) -> Option<CircuitOffer> {
    let ClientResponse::Status(status) = call(host, ClientRequest::Status).await else {
        panic!("no status");
    };
    let media = status
        .endpoints
        .and_then(|e| e.media)
        .expect("a media endpoint");
    let addr = EndpointAddr::from_parts(media, [TransportAddr::Custom(simnet::addr(media))]);
    let conn = me.0.connect(addr, MEDIA_HOSTS_ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &MediaHostsRequest { post, blob })
        .await
        .unwrap();
    send.finish().unwrap();
    let reply: MediaHostsResponse = read_frame(&mut recv).await.unwrap();
    conn.close(0u32.into(), b"done");
    let MediaHostsResponse::Hosts { circuit, .. } = reply else {
        panic!("unexpected {reply:?}");
    };
    circuit.map(|c| *c)
}

/// Waits for `host` to name a hop, or to name none.
async fn offer_of(
    me: &(Endpoint, SecretKey),
    host: &Node,
    asked: (ObjectId, BlobHash),
    named: bool,
) -> Option<CircuitOffer> {
    timeout(Duration::from_secs(60), async {
        loop {
            let offer = media_hosts(me, host, asked).await;
            if offer.is_some() == named {
                return offer;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the host never named {}",
            if named { "a hop" } else { "no hop" }
        )
    })
}

/// Fetches `blob` with only what `offer` says: a circuit through the hop it
/// names, to the slot, then the ends' QUIC to the circuit endpoint.
async fn fetch_through(relay: &RelayUrl, offer: &CircuitOffer, blob: BlobHash) -> Vec<u8> {
    use iroh_blobs::store::mem::MemStore;

    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(offer.carrier.as_bytes())
        .unwrap();
    let descriptor = Descriptor::from_bytes(&bytes).unwrap();
    let info = descriptor.verify(now()).unwrap();
    let viewer = end(relay).await;
    let circuit = ViewerCircuit::build(
        link(&viewer, &info).await,
        std::slice::from_ref(&descriptor),
        offer.slot,
    )
    .await
    .unwrap();
    let ends = osp_carrier::viewer_endpoint(circuit, offer.endpoint)
        .await
        .unwrap();
    let conn = timeout(
        WAIT,
        ends.connect(EndpointAddr::new(offer.endpoint), iroh_blobs::ALPN),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(conn.remote_id(), offer.endpoint);
    let mine = MemStore::new();
    let hash = iroh_blobs::Hash::from_bytes(*blob.as_bytes());
    timeout(WAIT, mine.remote().fetch(conn.clone(), hash))
        .await
        .unwrap()
        .unwrap();
    let got = mine.get_bytes(hash).await.unwrap().to_vec();
    conn.close(0u32.into(), b"done");
    ends.close().await;
    viewer.close().await;
    got
}

/// Alice's node links to Kim's, a seed carrier, and names it, the slot and
/// her circuit endpoint to a stranger asking who hosts her photo. The
/// stranger fetches the photo through Kim's node with only that, and the
/// file checks out. With Kim's node off, Alice's names no hop; once it's
/// back, it links again.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_names_its_seed_carrier_and_is_reached_through_it() {
    let dir = tempfile::tempdir().unwrap();
    let (_relay, relay) = local_relay().await;
    let net = SimNet::new(3);
    let kim_home = dir.path().join("kim");
    let mut kim_config = config("Kim", &relay);
    kim_config.carrier.seed = true;
    Node::init(&kim_home, &kim_config).unwrap();
    let kim = start(&kim_home, &net).await;
    let seed = kim.machine_id().unwrap();

    let alice_home = dir.path().join("alice");
    let mut alice_config = config("Alice", &relay);
    alice_config.endpoints.enabled = true;
    alice_config.carrier.known = vec![SeedCarrier { machine: seed }];
    Node::init(&alice_home, &alice_config).unwrap();
    let alice = start(&alice_home, &net).await;
    let asked = post_photo(&alice, dir.path()).await;
    let me = stranger(&net).await;

    let offer = offer_of(&me, &alice, asked, true).await.unwrap();
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(offer.carrier.as_bytes())
        .unwrap();
    let hop = Descriptor::from_bytes(&bytes)
        .unwrap()
        .verify(now())
        .unwrap();
    assert_eq!(hop.machine, seed);
    // Her circuit endpoint is none of the endpoints with an address.
    assert!(!alice.endpoint_ids().contains(&offer.endpoint));

    let photo = fetch_through(&relay, &offer, asked.1).await;
    assert_eq!(iroh_blobs::Hash::new(&photo).as_bytes(), asked.1.as_bytes());

    // Kim's node goes: no hop to name. Back: linked again, through the same
    // circuit endpoint.
    kim.shutdown().await.unwrap();
    assert!(offer_of(&me, &alice, asked, false).await.is_none());
    let kim = start(&kim_home, &net).await;
    let again = offer_of(&me, &alice, asked, true).await.unwrap();
    assert_eq!(again.endpoint, offer.endpoint);
    assert_eq!(fetch_through(&relay, &again, asked.1).await, photo);

    alice.shutdown().await.unwrap();
    kim.shutdown().await.unwrap();
}
