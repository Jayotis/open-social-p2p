//! Every node carries (ADR 0043, stage B part 4a): its machine record names
//! its carrier in a signed descriptor and says when its machine key started,
//! and a circuit goes through the node's carrier.
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
    Config, Node, RelayChoice, SimSettings,
    simnet::{self, Link, SimNet},
};
use osp_proto::{
    MACHINE_ALPN,
    framing::{read_frame, write_frame},
    machine::{MachineInfo, MachineRecord, MachineRequest, MachineResponse},
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
