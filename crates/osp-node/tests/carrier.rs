//! Every node carries (ADR 0043, stage B part 4a): its machine record names
//! its carrier in a signed descriptor and says when its machine key started,
//! and a circuit goes through the node's carrier. A host keeps a link to a
//! seed carrier and names it to viewers (part 4b), and a viewer fetches
//! through its own hop and the host's (part 4c), as does a follower.
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
    Config, KnownDirectory, MeetingTimes, Node, RelayChoice, SeedCarrier, SimSettings,
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
    post_made(node, dir, "testsrc=s=64x64").await
}

/// A photo of noise, about 3 MB, over the size followers fetch through the
/// swarm, posted.
async fn post_large_photo(node: &Node, dir: &Path) -> (ObjectId, BlobHash) {
    post_made(
        node,
        dir,
        "nullsrc=s=2048x2048,geq=random(1)*255:random(1)*255:random(1)*255",
    )
    .await
}

async fn post_made(node: &Node, dir: &Path, source: &str) -> (ObjectId, BlobHash) {
    let photo = dir.join("photo.png");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", source, "-frames:v", "1"])
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
/// names, to the slot, then the ends' QUIC to the circuit endpoint. If that
/// fails, asks `host` for a fresh offer and tries again, twice at most: under
/// load the host may have linked to its hop again since the offer, and its
/// slot was the old link's.
async fn fetch_through(
    relay: &RelayUrl,
    offer: &CircuitOffer,
    blob: BlobHash,
    again: (&(Endpoint, SecretKey), &Node, (ObjectId, BlobHash)),
) -> Vec<u8> {
    let mut offer = offer.clone();
    for tried in 1.. {
        match try_fetch_through(relay, &offer, blob).await {
            Ok(got) => return got,
            Err(e) if tried < 3 => {
                eprintln!("fetching through the hop, try {tried}: {e:#}");
                tokio::time::sleep(Duration::from_secs(1)).await;
                offer = offer_of(again.0, again.1, again.2, true).await.unwrap();
            }
            Err(e) => panic!("fetching through the hop: {e:#}"),
        }
    }
    unreachable!()
}

async fn try_fetch_through(
    relay: &RelayUrl,
    offer: &CircuitOffer,
    blob: BlobHash,
) -> anyhow::Result<Vec<u8>> {
    use iroh_blobs::store::mem::MemStore;

    let bytes = data_encoding::BASE64URL_NOPAD.decode(offer.carrier.as_bytes())?;
    let descriptor = Descriptor::from_bytes(&bytes)?;
    let info = descriptor.verify(now())?;
    let viewer = end(relay).await;
    let got = async {
        let circuit = ViewerCircuit::build(
            link(&viewer, &info).await,
            std::slice::from_ref(&descriptor),
            offer.slot,
        )
        .await?;
        let ends = osp_carrier::viewer_endpoint(circuit, offer.endpoint).await?;
        let conn = timeout(
            WAIT,
            ends.connect(EndpointAddr::new(offer.endpoint), iroh_blobs::ALPN),
        )
        .await??;
        assert_eq!(conn.remote_id(), offer.endpoint);
        let mine = MemStore::new();
        let hash = iroh_blobs::Hash::from_bytes(*blob.as_bytes());
        timeout(WAIT, mine.remote().fetch(conn.clone(), hash)).await??;
        let got = mine.get_bytes(hash).await?.to_vec();
        conn.close(0u32.into(), b"done");
        ends.close().await;
        anyhow::Ok(got)
    }
    .await;
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
    alice_config.carrier.hops = Some(true);
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

    let photo = fetch_through(&relay, &offer, asked.1, (&me, &alice, asked)).await;
    assert_eq!(iroh_blobs::Hash::new(&photo).as_bytes(), asked.1.as_bytes());

    // Kim's node goes: no hop to name. Back: linked again, through the same
    // circuit endpoint.
    kim.shutdown().await.unwrap();
    assert!(offer_of(&me, &alice, asked, false).await.is_none());
    let kim = start(&kim_home, &net).await;
    let again = offer_of(&me, &alice, asked, true).await.unwrap();
    assert_eq!(again.endpoint, offer.endpoint);
    assert_eq!(
        fetch_through(&relay, &again, asked.1, (&me, &alice, asked)).await,
        photo
    );

    alice.shutdown().await.unwrap();
    kim.shutdown().await.unwrap();
}

/// The hop next to Alice's node sees its address, so her node links to one
/// only with her yes (ADR 0043 §8). Not asked yet, it names none. Once she
/// says yes it does, at once; and with no, the link goes, and her media is
/// reached through relays again.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_links_to_its_hop_only_with_its_owners_yes() {
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

    let hops = |status: &osp_proto::rpc::StatusInfo| status.carrier.as_ref().unwrap().hops;
    assert_eq!(hops(&status(&alice).await), None);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(media_hosts(&me, &alice, asked).await.is_none());

    alice.handle(ClientRequest::SetHops { yes: true }).await;
    offer_of(&me, &alice, asked, true).await.unwrap();
    assert_eq!(hops(&status(&alice).await), Some(true));

    alice.handle(ClientRequest::SetHops { yes: false }).await;
    offer_of(&me, &alice, asked, false).await;
    // Kept for the next start.
    let saved = Config::load(&alice_home.join("config.toml")).unwrap();
    assert_eq!(saved.carrier.hops, Some(false));

    alice.shutdown().await.unwrap();
    kim.shutdown().await.unwrap();
}

/// Kim pauses carrying, says the connection is metered, and sets the jobs,
/// share and upload: each applies at once, shows in status and is kept.
#[tokio::test(flavor = "multi_thread")]
async fn the_owners_controls_apply_at_once_and_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (_relay, relay) = local_relay().await;
    let net = SimNet::new(3);
    let home = dir.path().join("kim");
    Node::init(&home, &config("Kim", &relay)).unwrap();
    let kim = start(&home, &net).await;
    let carrier = |s: osp_proto::rpc::StatusInfo| *s.carrier.unwrap();
    let now = timeout(Duration::from_secs(30), async {
        loop {
            let c = carrier(status(&kim).await);
            if c.carrying {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("Kim's node never started carrying");
    // A home node's, from the sizing stage, of a slow home's upload.
    assert_eq!((now.jobs, now.upload_share), (2, 50));
    assert_eq!(now.upload_from, "assumed");
    assert_eq!(now.bytes_per_sec, Some(625_000));

    let set = |paused, metered, jobs, upload_share, upload_mbps| ClientRequest::SetCarrying {
        paused,
        metered,
        jobs,
        upload_share,
        upload_mbps,
    };
    kim.handle(set(None, None, Some(3), Some(40), Some(20)))
        .await;
    let now = carrier(status(&kim).await);
    assert_eq!((now.jobs, now.upload_share), (3, 40));
    assert_eq!(now.upload_from, "set");
    assert_eq!(now.bytes_per_sec, Some(1_000_000));
    assert!(now.carrying);

    kim.handle(set(None, Some(true), None, None, None)).await;
    let now = carrier(status(&kim).await);
    assert!(now.metered && !now.carrying);
    kim.handle(set(Some(true), Some(false), None, None, None))
        .await;
    let now = carrier(status(&kim).await);
    assert!(now.paused && !now.carrying);
    // Out of range is refused, and changes nothing.
    match kim.handle(set(None, None, Some(0), None, None)).await {
        ClientResponse::Error { .. } => {}
        other => panic!("{other:?}"),
    }
    kim.handle(set(Some(false), None, None, None, None)).await;
    let now = carrier(status(&kim).await);
    assert!(now.carrying && now.jobs == 3);

    let saved = Config::load(&home.join("config.toml")).unwrap();
    assert_eq!(saved.carrier.jobs, Some(3));
    assert_eq!(saved.carrier.upload_share, Some(40));
    assert_eq!(saved.serving.upload_mbps, 20);
    assert!(!saved.carrier.paused && !saved.carrier.metered);
    kim.shutdown().await.unwrap();
}

/// A directory, on the simulated network only, where a name listed today
/// may host at once.
async fn directory(home: &Path, net: &SimNet) -> Node {
    let mut config = Config::local_test("The directory");
    config.endpoints.enabled = true;
    config.directory.serve = true;
    Node::init(home, &config).unwrap();
    let settings = SimSettings {
        listed_read_every: Duration::from_millis(200),
        new_name_for: Duration::ZERO,
        hosting_after_listed: Duration::ZERO,
        ..sim(net)
    };
    Node::spawn_sim(home, settings).await.unwrap()
}

/// Someone on the relay who knows `directory` and the seed carrier `seed`.
fn knowing(name: &str, relay: &RelayUrl, directory: &Node, seed: Option<EndpointId>) -> Config {
    let mut config = config(name, relay);
    config.endpoints.enabled = true;
    config.directory.known = vec![KnownDirectory {
        machine: directory.machine_id().unwrap(),
        node: directory.node_id(),
    }];
    config.carrier.known = seed
        .map(|machine| SeedCarrier { machine })
        .into_iter()
        .collect();
    config.carrier.hops = Some(true);
    config
}

/// Reads the directory until `node` holds the post `id`.
async fn find(node: &Node, id: ObjectId) {
    timeout(Duration::from_secs(60), async {
        loop {
            assert!(matches!(
                call(node, ClientRequest::ReadDirectories).await,
                ClientResponse::DirectoriesRead { .. }
            ));
            if matches!(
                node.handle(ClientRequest::Show { id }).await,
                ClientResponse::Item { .. }
            ) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("the post was never found");
}

/// Jimmy finds Alice's photo through a directory and opens it. Both know
/// Kim's node, a seed carrier: Alice's node links to it, and Jimmy's picks
/// it as its own hop too, so the photo comes over one hop, and Kim's carrier
/// carried it. Robin knows no seed carrier and gets it through the relay as
/// before. With Kim's node off, Sam, who knows it, still gets the photo:
/// through the relay.
#[tokio::test(flavor = "multi_thread")]
async fn a_viewer_fetches_through_its_hop_and_the_hosts_and_falls_back_to_the_relay() {
    let dir = tempfile::tempdir().unwrap();
    let (_relay, relay) = local_relay().await;
    let net = SimNet::new(4);
    let directory = directory(&dir.path().join("directory"), &net).await;

    let kim_home = dir.path().join("kim");
    let mut kim_config = config("Kim", &relay);
    kim_config.carrier.seed = true;
    Node::init(&kim_home, &kim_config).unwrap();
    let kim = start(&kim_home, &net).await;
    let seed = kim.machine_id().unwrap();

    let alice_home = dir.path().join("alice");
    let mut alice_config = knowing("Alice", &relay, &directory, Some(seed));
    alice_config.directory.list_me = true;
    Node::init(&alice_home, &alice_config).unwrap();
    let alice = start(&alice_home, &net).await;
    let asked = post_photo(&alice, dir.path()).await;
    assert!(alice.ask_to_be_listed().await);
    let me = stranger(&net).await;
    offer_of(&me, &alice, asked, true).await;

    let viewer = async |name: &str, seed: Option<EndpointId>| {
        let home = dir.path().join(name);
        Node::init(&home, &knowing(name, &relay, &directory, seed)).unwrap();
        let node = start(&home, &net).await;
        find(&node, asked.0).await;
        node
    };
    let carried = async || kim.carrier_stats().await.unwrap().bytes;

    let jimmy = viewer("Jimmy", Some(seed)).await;
    let before = carried().await;
    let size = jimmy.open_blob(&asked.1).await.unwrap();
    assert!(
        carried().await >= before + size,
        "Kim's carrier didn't carry it"
    );
    assert_eq!(jimmy.viewing_endpoints_made(), 1);

    let robin = viewer("Robin", None).await;
    let before = carried().await;
    assert_eq!(robin.open_blob(&asked.1).await.unwrap(), size);
    assert!(
        carried().await < before + size,
        "Robin's fetch went through Kim"
    );

    kim.shutdown().await.unwrap();
    let sam = viewer("Sam", Some(seed)).await;
    assert_eq!(sam.open_blob(&asked.1).await.unwrap(), size);

    for node in [jimmy, robin, sam, alice, directory] {
        node.shutdown().await.unwrap();
    }
}

/// Follows `author` with its ticket, and syncs.
async fn follow(node: &Node, author: &Node) {
    let ClientResponse::Ticket { ticket } = call(author, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(node, ClientRequest::Follow { ticket }).await;
    call(node, ClientRequest::Sync).await;
}

/// Jimmy and Sam follow Alice, and all three know Kim's node, a seed
/// carrier. Alice's node names its hop when they ask who hosts her large
/// photo, so the photo goes through Kim's carrier rather than the relay:
/// to Jimmy when he opens it, and to Sam's node fetching it ahead of time,
/// which asks for the offer although it isn't sharing what it prefetches.
#[tokio::test(flavor = "multi_thread")]
async fn followers_fetch_through_the_hosts_hop() {
    let dir = tempfile::tempdir().unwrap();
    let (_relay, relay) = local_relay().await;
    let net = SimNet::new(4);

    let kim_home = dir.path().join("kim");
    let mut kim_config = config("Kim", &relay);
    kim_config.carrier.seed = true;
    Node::init(&kim_home, &kim_config).unwrap();
    let kim = start(&kim_home, &net).await;
    let seed = SeedCarrier {
        machine: kim.machine_id().unwrap(),
    };
    let carried = async || kim.carrier_stats().await.unwrap().bytes;

    let node = async |name: &str, prefetch: bool| {
        let home = dir.path().join(name);
        let mut config = config(name, &relay);
        config.endpoints.enabled = true;
        config.carrier.known = vec![seed.clone()];
        config.carrier.hops = Some(true);
        config.sync.prefetch = prefetch;
        Node::init(&home, &config).unwrap();
        start(&home, &net).await
    };
    let alice = node("Alice", true).await;
    let me = stranger(&net).await;
    let small = post_photo(&alice, dir.path()).await;
    offer_of(&me, &alice, small, true).await;
    let (_, blob) = post_large_photo(&alice, dir.path()).await;

    let jimmy = node("Jimmy", false).await;
    follow(&jimmy, &alice).await;
    let before = carried().await;
    let size = jimmy.open_blob(&blob).await.unwrap();
    assert!(size > 1 << 20);
    assert!(
        carried().await >= before + size,
        "Jimmy's fetch didn't go through Kim"
    );

    let before = carried().await;
    let sam = node("Sam", true).await;
    follow(&sam, &alice).await;
    timeout(Duration::from_secs(60), async {
        while carried().await < before + size {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("Sam's node didn't fetch it through Kim");

    for node in [jimmy, sam, alice, kim] {
        node.shutdown().await.unwrap();
    }
}

/// Robin finds Alice's large photo through a directory and reshares it
/// with hosting, listed. Robin's node knows Kim's, a seed carrier, and
/// offers a way through it on its media endpoint; Alice's knows none. With
/// Alice's node off, Jimmy finds the post and fetches Robin's copy through
/// Kim's carrier, not the relay.
#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_fetches_a_listed_resharers_copy_through_its_hop() {
    let dir = tempfile::tempdir().unwrap();
    let (_relay, relay) = local_relay().await;
    let net = SimNet::new(4);
    let directory = directory(&dir.path().join("directory"), &net).await;

    let kim_home = dir.path().join("kim");
    let mut kim_config = config("Kim", &relay);
    kim_config.carrier.seed = true;
    Node::init(&kim_home, &kim_config).unwrap();
    let kim = start(&kim_home, &net).await;
    let seed = kim.machine_id().unwrap();
    let carried = async || kim.carrier_stats().await.unwrap().bytes;

    let listed = async |name: &str, seed: Option<EndpointId>| {
        let home = dir.path().join(name);
        let mut config = knowing(name, &relay, &directory, seed);
        config.directory.list_me = true;
        Node::init(&home, &config).unwrap();
        start(&home, &net).await
    };
    let alice = listed("Alice", None).await;
    let asked = post_large_photo(&alice, dir.path()).await;
    assert!(alice.ask_to_be_listed().await);

    let robin = listed("Robin", Some(seed)).await;
    find(&robin, asked.0).await;
    let reshare = ClientRequest::Reshare {
        id: asked.0,
        host: true,
    };
    call(&robin, reshare).await;
    assert!(robin.ask_to_be_listed().await);
    let me = stranger(&net).await;
    offer_of(&me, &robin, asked, true).await;
    alice.shutdown().await.unwrap();

    let home = dir.path().join("jimmy");
    Node::init(&home, &knowing("Jimmy", &relay, &directory, Some(seed))).unwrap();
    let jimmy = start(&home, &net).await;
    timeout(Duration::from_secs(60), async {
        loop {
            find(&jimmy, asked.0).await;
            let ClientResponse::Item { item } =
                call(&jimmy, ClientRequest::Show { id: asked.0 }).await
            else {
                panic!("not shown");
            };
            if item.found_reshares.iter().any(|r| r.hosting) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("Robin's hosting reshare was never found");
    let before = carried().await;
    let size = jimmy.open_blob(&asked.1).await.unwrap();
    assert!(size > 1 << 20);
    assert!(
        carried().await >= before + size,
        "Jimmy's fetch didn't go through Kim"
    );

    for node in [jimmy, robin, kim, directory] {
        node.shutdown().await.unwrap();
    }
}

/// Alice's node knows no carrier. Sam's node fetches her large photo ahead
/// of time and shares its copy; it knows Kim's, a seed carrier, and offers
/// a way through it on its media endpoint. When Jimmy follows Alice, her
/// node names Sam's copy, and Jimmy's node fetches it through Kim's carrier
/// rather than the relay, and before trying Alice's own node.
#[tokio::test(flavor = "multi_thread")]
async fn a_follower_fetches_another_followers_copy_through_its_hop() {
    let dir = tempfile::tempdir().unwrap();
    let (_relay, relay) = local_relay().await;
    let net = SimNet::new(4);

    let kim_home = dir.path().join("kim");
    let mut kim_config = config("Kim", &relay);
    kim_config.carrier.seed = true;
    Node::init(&kim_home, &kim_config).unwrap();
    let kim = start(&kim_home, &net).await;
    let seed = SeedCarrier {
        machine: kim.machine_id().unwrap(),
    };
    let carried = async || kim.carrier_stats().await.unwrap().bytes;

    let node = async |name: &str, known: Vec<SeedCarrier>| {
        let home = dir.path().join(name);
        let mut config = config(name, &relay);
        config.endpoints.enabled = true;
        config.carrier.known = known;
        config.carrier.hops = Some(true);
        config.sync.prefetch = true;
        config.swarm.share_prefetched = true;
        Node::init(&home, &config).unwrap();
        let sim = SimSettings {
            prefetch_spread: Duration::ZERO,
            ..sim(&net)
        };
        Node::spawn_sim(&home, sim).await.unwrap()
    };
    let alice = node("Alice", Vec::new()).await;
    let asked = post_large_photo(&alice, dir.path()).await;

    let sam = node("Sam", vec![seed.clone()]).await;
    follow(&sam, &alice).await;
    timeout(Duration::from_secs(60), async {
        while alice.swarm_copies(&sam.node_id()).unwrap() == 0 {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("Sam's node never told Alice's it holds a copy");
    let me = stranger(&net).await;
    offer_of(&me, &sam, asked, true).await;

    let before = carried().await;
    let jimmy = node("Jimmy", vec![seed]).await;
    follow(&jimmy, &alice).await;
    timeout(Duration::from_secs(60), async {
        while carried().await < before + (1 << 20) {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("Jimmy's node didn't fetch Sam's copy through Kim");

    for node in [jimmy, sam, alice, kim] {
        node.shutdown().await.unwrap();
    }
}

/// Meeting machines quickly, for a test.
fn quick() -> MeetingTimes {
    MeetingTimes {
        hour: Duration::from_secs(1),
        round: Duration::from_millis(100),
        try_again_after: Duration::from_millis(300),
        check_in_every: Duration::from_secs(2),
        met_lately: Duration::from_secs(60),
        still_answering: Duration::from_secs(30),
        forget_silent: Duration::from_secs(120),
        known_after: Duration::from_secs(2),
        half_life: Duration::from_secs(60),
        introducer_after: Duration::from_secs(1),
    }
}

/// Kim's and Quinn's nodes are seed carriers. Alice's node knows only
/// Quinn's, and Jimmy's only Kim's. Mo's node is new, an ordinary one, and
/// Jimmy's machine meets it through a directory. When Jimmy opens Alice's
/// large photo, Mo's machine has his node's fresh turn in the middle:
/// Alice → Quinn → Mo → Kim → Jimmy. It carried the photo, and Jimmy's node
/// takes it on probation, without standing yet. Sam's node, which holds
/// Jimmy's machine at standing, asks it for work and is offered Mo's, which
/// then carries in the middle of Sam's fetch too.
#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_machine_carries_in_the_middle_and_is_offered_on() {
    let dir = tempfile::tempdir().unwrap();
    let (_relay, relay) = local_relay().await;
    let net = SimNet::new(4);
    let directory = directory(&dir.path().join("directory"), &net).await;

    let seed = async |name: &str| {
        let home = dir.path().join(name);
        let mut config = config(name, &relay);
        config.carrier.seed = true;
        Node::init(&home, &config).unwrap();
        start(&home, &net).await
    };
    let kim = seed("Kim").await;
    let quinn = seed("Quinn").await;
    let known = |node: &Node| SeedCarrier {
        machine: node.machine_id().unwrap(),
    };

    let meeting = SimSettings {
        meeting: quick(),
        first_read_wait: Duration::from_millis(200),
        read_every: Duration::from_secs(1),
        ..sim(&net)
    };
    let home = dir.path().join("mo");
    Node::init(&home, &knowing("Mo", &relay, &directory, None)).unwrap();
    let mo = Node::spawn_sim(&home, meeting.clone()).await.unwrap();
    let home = dir.path().join("jimmy");
    let mut jimmy_config = knowing("Jimmy", &relay, &directory, None);
    jimmy_config.carrier.known = vec![known(&kim)];
    jimmy_config.carrier.hops = Some(true);
    jimmy_config.sync.prefetch = false;
    Node::init(&home, &jimmy_config).unwrap();
    let jimmy = Node::spawn_sim(&home, meeting).await.unwrap();
    let mo_machine = mo.machine_id().unwrap();
    // Not only met: Mo's carrier starts after its machine does, so its
    // first record may name none.
    timeout(Duration::from_secs(60), async {
        while !jimmy.carriers_in_table().contains(&mo_machine) {
            let _ = jimmy.meet_round_now().await;
            let _ = mo.meet_round_now().await;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("Jimmy's machine never heard of Mo's carrier");

    let home = dir.path().join("alice");
    let mut alice_config = config("Alice", &relay);
    alice_config.endpoints.enabled = true;
    alice_config.carrier.known = vec![known(&quinn)];
    alice_config.carrier.hops = Some(true);
    Node::init(&home, &alice_config).unwrap();
    let alice = start(&home, &net).await;
    let me = stranger(&net).await;
    let asked = post_large_photo(&alice, dir.path()).await;
    offer_of(&me, &alice, asked, true).await;
    follow(&jimmy, &alice).await;

    let carried = async |node: &Node| node.carrier_stats().await.unwrap().bytes;
    let before = [
        carried(&kim).await,
        carried(&mo).await,
        carried(&quinn).await,
    ];
    let size = jimmy.open_blob(&asked.1).await.unwrap();
    assert!(size > 1 << 20);
    let after = [
        carried(&kim).await,
        carried(&mo).await,
        carried(&quinn).await,
    ];
    for (i, name) in ["Kim", "Mo", "Quinn"].iter().enumerate() {
        assert!(
            after[i] >= before[i] + size,
            "{name}'s machine didn't carry it: {before:?} {after:?}"
        );
    }
    let (probation, standing) = jimmy.carrier_picks().unwrap();
    assert_eq!(probation, [mo_machine]);
    assert!(standing.is_empty(), "standing takes 3 days");

    // Work offers (part 4d-2). Sam's node holds Jimmy's machine at standing
    // and has room for a newcomer, so it asks Jimmy's carrier, which names
    // Mo's machine. Sam's node has met nobody else.
    let home = dir.path().join("sam");
    let mut sam_config = config("Sam", &relay);
    sam_config.endpoints.enabled = true;
    sam_config.carrier.known = vec![known(&kim)];
    sam_config.carrier.hops = Some(true);
    sam_config.sync.prefetch = false;
    Node::init(&home, &sam_config).unwrap();
    let sam = start(&home, &net).await;
    let jimmy_machine = jimmy.machine_id().unwrap();
    sam.credit_carrier_on_days(&jimmy_machine, 3).unwrap();
    sam.ask_for_work_now().await.unwrap();
    assert_eq!(sam.work_offers_held().unwrap(), [mo_machine]);

    // Mo's machine has Sam's newcomer turn in the middle, next to Jimmy's,
    // and is taken on probation. The offer had its turn. (Sam's machine may
    // have met Mo's by then, and given it the turn as fresh instead: the
    // picks module's tests say which goes first.)
    follow(&sam, &alice).await;
    let before = carried(&mo).await;
    let size = sam.open_blob(&asked.1).await.unwrap();
    assert!(
        carried(&mo).await >= before + size,
        "Mo's machine didn't carry for Sam's node"
    );
    let (probation, standing) = sam.carrier_picks().unwrap();
    assert_eq!(probation, [mo_machine]);
    assert_eq!(standing, [jimmy_machine], "an offer is never standing");
    let held = sam.work_offers_held().unwrap();
    assert!(held.is_empty(), "{held:?}, mo {mo_machine}");

    for node in [sam, jimmy, mo, alice, kim, quinn, directory] {
        node.shutdown().await.unwrap();
    }
}

async fn status(node: &Node) -> osp_proto::rpc::StatusInfo {
    match node.handle(ClientRequest::Status).await {
        ClientResponse::Status(status) => status,
        other => panic!("{other:?}"),
    }
}
