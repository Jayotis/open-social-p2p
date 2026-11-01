//! The simulated network (`osp_node::simnet`): real QUIC between endpoints
//! in this process, over connections with a delay, a speed each way and
//! packet loss, which can be cut off; two nodes following each other over
//! it; and a follower whose copy an author's node was too busy to hear of
//! telling it again.
//!
//! A packet is never delivered early, so the lower bounds on times here are
//! exact; the upper bounds leave room for a busy machine.

use std::time::{Duration, Instant};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr,
    endpoint::{Connection, RecvStream, SendStream, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_blobs::{
    Hash,
    protocol::{ChunkRanges, ChunkRangesExt, GetRequest},
    store::mem::MemStore,
};
use osp_node::{
    BlobClass, Config, Node, SimSettings,
    simnet::{self, Link, SimNet},
};
use osp_proto::{
    BlobHash, SOCIAL_ALPN,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, StatusInfo},
    social::{SocialRequest, SocialResponse},
};

const ALPN: &[u8] = b"osp-test/simnet";
const MBPS: u64 = 1_000_000;

/// Answers each request, an 8-byte length, with that many bytes.
#[derive(Debug, Clone)]
struct Source;

impl ProtocolHandler for Source {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let mut len = [0u8; 8];
            if recv.read_exact(&mut len).await.is_err() {
                break;
            }
            let data = vec![7u8; u64::from_le_bytes(len) as usize];
            if send.write_all(&data).await.is_err() || send.finish().is_err() {
                break;
            }
        }
        Ok(())
    }
}

/// An endpoint on `net` with `link`, and no other way to send.
async fn endpoint(net: &SimNet, link: Link) -> Endpoint {
    let key = SecretKey::generate();
    let transport = net.link(link).transport(key.public());
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .add_custom_transport(transport)
        .secret_key(key)
        .bind()
        .await
        .unwrap()
}

fn sim_addr(id: EndpointId) -> EndpointAddr {
    EndpointAddr::from_parts(id, [TransportAddr::Custom(simnet::addr(id))])
}

/// A client with `client` connected to a server with `server`.
async fn connected(net: &SimNet, client: Link, server: Link) -> (Endpoint, Router, Connection) {
    let server = Router::builder(endpoint(net, server).await)
        .accept(ALPN, Source)
        .spawn();
    let client = endpoint(net, client).await;
    let conn = client
        .connect(sim_addr(server.endpoint().id()), ALPN)
        .await
        .unwrap();
    (client, server, conn)
}

async fn fetch(conn: &Connection, bytes: usize) -> Vec<u8> {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(&(bytes as u64).to_le_bytes()).await.unwrap();
    send.finish().unwrap();
    recv.read_to_end(bytes + 1).await.unwrap()
}

fn delay(ms: u64) -> Link {
    Link {
        delay: Duration::from_millis(ms),
        ..Link::default()
    }
}

#[tokio::test]
async fn a_round_trip_takes_both_delays_twice() {
    let net = SimNet::new(1);
    // 30 ms and 20 ms each way: 100 ms there and back.
    let (_client, _server, conn) = connected(&net, delay(30), delay(20)).await;
    let start = Instant::now();
    assert_eq!(fetch(&conn, 10).await.len(), 10);
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(100) && took < Duration::from_secs(1),
        "{took:?}"
    );
    let paths = conn.paths();
    let rtt = paths.iter().find(|p| p.is_selected()).unwrap().rtt();
    assert!(
        rtt >= Duration::from_millis(95) && rtt < Duration::from_millis(250),
        "{rtt:?}"
    );
    let stats = net.stats();
    assert!(stats.delivered > 0);
    // Some may still be on their way.
    assert!(stats.delivered <= stats.packets, "{stats:?}");
    assert_eq!((stats.lost, stats.queue_full, stats.cut_off), (0, 0, 0));
}

#[tokio::test]
async fn transfers_go_at_the_slower_ends_speed() {
    let net = SimNet::new(1);
    // 1 MB is 8 Mbit: at least a second at 8 Mbit/s, whichever end is slow.
    let slow_upload = Link {
        up_bps: 8 * MBPS,
        ..delay(5)
    };
    let slow_download = Link {
        down_bps: 8 * MBPS,
        ..delay(5)
    };
    for (client, server) in [(delay(5), slow_upload), (slow_download, delay(5))] {
        let (_client, _server, conn) = connected(&net, client, server).await;
        let start = Instant::now();
        assert_eq!(fetch(&conn, 1_000_000).await.len(), 1_000_000);
        let took = start.elapsed();
        assert!(
            took >= Duration::from_secs(1) && took < Duration::from_secs(5),
            "{took:?}"
        );
    }
    // Neither end limited: much quicker.
    let (_client, _server, conn) = connected(&net, delay(5), delay(5)).await;
    let start = Instant::now();
    fetch(&conn, 1_000_000).await;
    assert!(start.elapsed() < Duration::from_millis(900));
}

#[tokio::test]
async fn lost_packets_are_sent_again() {
    let net = SimNet::new(1);
    let lossy = Link {
        loss: 0.05,
        ..delay(5)
    };
    let (_client, _server, conn) = connected(&net, lossy, lossy).await;
    let data = fetch(&conn, 1_000_000).await;
    assert_eq!(data.len(), 1_000_000);
    assert!(data.iter().all(|b| *b == 7));
    let stats = net.stats();
    // Lost at either end: 1 - 0.95², about 9.75%, where one end alone
    // would lose 5%.
    let lost = stats.lost as f64 / stats.packets as f64;
    assert!((0.07..0.13).contains(&lost), "{stats:?}");
    // Nothing else was dropped, though a few may still be on their way.
    assert_eq!((stats.queue_full, stats.cut_off), (0, 0));
    let accounted = stats.delivered + stats.lost;
    assert!(accounted <= stats.packets && accounted + 10 > stats.packets);
}

#[tokio::test]
async fn a_node_cut_off_is_unreachable_until_it_is_back() {
    let net = SimNet::new(1);
    let (client, _server, conn) = connected(&net, delay(5), delay(5)).await;
    fetch(&conn, 1_000).await;
    assert!(net.set_online(client.id(), false));
    let waiting = tokio::spawn({
        let conn = conn.clone();
        async move { fetch(&conn, 1_000).await.len() }
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!waiting.is_finished(), "nothing gets through while cut off");
    assert!(net.stats().cut_off > 0);
    // Back again before QUIC gives up: what was waiting gets through.
    net.set_online(client.id(), true);
    let got = tokio::time::timeout(Duration::from_secs(10), waiting).await;
    assert_eq!(got.unwrap().unwrap(), 1_000);
    // Only nodes on the network can be cut off.
    assert!(!net.set_online(SecretKey::generate().public(), false));
}

#[tokio::test]
async fn a_node_leaves_the_network_when_its_endpoint_closes() {
    let net = SimNet::new(1);
    let ep = endpoint(&net, Link::default()).await;
    let id = ep.id();
    assert!(net.set_online(id, true));
    ep.close().await;
    drop(ep);
    // iroh drops the transport's endpoint once it has shut down.
    let start = Instant::now();
    while net.set_online(id, true) {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "still on the network"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn start(root: &std::path::Path, name: &str, net: &SimNet, link: Link) -> Node {
    let home = root.join(name);
    Node::init(&home, &Config::local_test(name)).unwrap();
    let sim = SimSettings {
        network: Some(net.link(link)),
        ..SimSettings::default()
    };
    Node::spawn_sim(&home, sim).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn nodes_follow_each_other_over_the_simulated_network() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(1);
    let a = start(dir.path(), "a", &net, delay(25)).await;
    let b = start(dir.path(), "b", &net, delay(25)).await;
    // A node has no IP address to hand out, only its ID: the others find
    // it on the network by that.
    let addr = a.addr().await;
    assert_eq!((addr.id, addr.addrs.len()), (a.node_id(), 0));

    let ClientResponse::Ticket { ticket } = call(&a, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(&b, ClientRequest::Follow { ticket }).await;
    call(
        &a,
        ClientRequest::Post {
            content: "over the simulated network".into(),
            attachments: vec![],
            uploads: vec![],
            upload: None,
        },
    )
    .await;
    // A sync is at least a round trip: 100 ms.
    let start = Instant::now();
    call(&b, ClientRequest::Sync).await;
    assert!(start.elapsed() >= Duration::from_millis(100));
    let ClientResponse::Feed { items } = call(&b, ClientRequest::Feed { limit: 10 }).await else {
        panic!("no feed");
    };
    assert!(
        items
            .iter()
            .any(|i| i.content == "over the simulated network"),
        "{items:?}"
    );
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}

/// Posts a 2048×2048 photo of noise, which comes out at about 3 MB, and
/// returns its blob.
async fn post_noise_photo(node: &Node, dir: &std::path::Path) -> Hash {
    let photo = dir.join("noise.png");
    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg("nullsrc=s=2048x2048,geq=random(1)*255:random(1)*255:random(1)*255")
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let post = ClientRequest::Post {
        content: "noise".into(),
        attachments: vec![photo],
        uploads: vec![],
        upload: None,
    };
    call(node, post).await;
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    Hash::from_bytes(*items[0].attachments[0].attachment.blob.as_bytes())
}

async fn status(node: &Node) -> StatusInfo {
    let ClientResponse::Status(status) = call(node, ClientRequest::Status).await else {
        panic!("no status");
    };
    status
}

/// A node with no upload cap that uploads at `mbps`, with a photo posted,
/// and another node on the network to fetch it.
async fn serving_photo(
    dir: &std::path::Path,
    net: &SimNet,
    mbps: u64,
) -> (Node, Hash, Connection, Endpoint) {
    let link = Link {
        up_bps: mbps * MBPS,
        ..delay(10)
    };
    let node = start(dir, "a", net, link).await;
    let photo = post_noise_photo(&node, dir).await;
    // The fewest places until it has measured anything.
    let before = status(&node).await;
    assert_eq!(
        (before.transfer_places, before.upload_measured_mbps),
        (2, None)
    );
    let other = endpoint(net, delay(10)).await;
    let conn = other
        .connect(sim_addr(node.node_id()), iroh_blobs::ALPN)
        .await
        .unwrap();
    (node, photo, conn, other)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_with_no_upload_cap_measures_a_slow_upload() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(1);
    let (node, photo, conn, _other) = serving_photo(dir.path(), &net, 2).await;
    // 12 s or so at 2 Mbit/s.
    MemStore::new().remote().fetch(conn, photo).await.unwrap();

    // Measured at about the line's speed, a little under for the overhead.
    // Counting what QUIC takes at once at the start would read far faster.
    let after = status(&node).await;
    let mbps = after.upload_measured_mbps.expect("measured");
    assert!((1.4..=2.1).contains(&mbps), "{mbps} Mbit/s");
    assert_eq!(after.transfer_places, 2);
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_with_no_upload_cap_measures_ranges_with_gaps_by_what_it_sends() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(1);
    let (node, photo, conn, _other) = serving_photo(dir.path(), &net, 1).await;
    let ClientResponse::Feed { items } = call(&node, ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    let size = items[0].attachments[0].attachment.size;
    // The start and end of the photo, with the bytes between as a gap, as
    // a node asks for what is left of a blob: 18 s or so at 1 Mbit/s.
    let ranges = ChunkRanges::bytes(0..1_750_000) | ChunkRanges::bytes(size - 500_000..size);
    let request = GetRequest::blob_ranges(photo, ranges);
    let got = MemStore::new().remote().execute_get(conn, request);
    got.complete().await.unwrap();

    // About the line's speed. Counting the gap as sent read 2.6 Mbit/s.
    let mbps = status(&node).await.upload_measured_mbps.expect("measured");
    assert!((0.7..=1.1).contains(&mbps), "{mbps} Mbit/s");
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_with_no_upload_cap_takes_on_more_once_it_measures_a_fast_upload() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(1);
    let (node, photo, conn, _other) = serving_photo(dir.path(), &net, 30).await;
    // Each copy takes a second or so at 30 Mbit/s: one after another until
    // a measurement ends.
    let start = Instant::now();
    let measured = loop {
        assert!(start.elapsed() < Duration::from_secs(30), "never measured");
        MemStore::new()
            .remote()
            .fetch(conn.clone(), photo)
            .await
            .unwrap();
        if let Some(mbps) = status(&node).await.upload_measured_mbps {
            break mbps;
        }
    };
    // Below the line's speed, since a copy's start isn't measured.
    assert!((12.0..=31.0).contains(&measured), "{measured} Mbit/s");
    assert_eq!(status(&node).await.transfer_places, 8);
    node.shutdown().await.unwrap();
}

/// A node that shares what it prefetches and fetches large files at once,
/// not spread out. One it couldn't tell it holds a copy, it tells again
/// within seconds, not minutes.
async fn start_sharing(root: &std::path::Path, name: &str, net: &SimNet, link: Link) -> Node {
    let home = root.join(name);
    let mut config = Config::local_test(name);
    config.swarm.share_prefetched = true;
    Node::init(&home, &config).unwrap();
    let sim = SimSettings {
        prefetch_spread: Duration::ZERO,
        tell_again_after: Duration::from_secs(4),
        network: Some(net.link(link)),
        ..SimSettings::default()
    };
    Node::spawn_sim(&home, sim).await.unwrap()
}

async fn feed_len(node: &Node) -> usize {
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 10 }).await else {
        panic!("no feed");
    };
    items.len()
}

async fn ask(conn: &Connection, req: &SocialRequest) -> SocialResponse {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, req).await.unwrap();
    send.finish().unwrap();
    read_frame(&mut recv).await.unwrap()
}

/// Requests begun and never finished, on one connection.
type Unfinished = (Endpoint, Connection, Vec<(SendStream, RecvStream)>);

/// Takes all 32 of `node`'s places for social requests, as other nodes'
/// requests would: 4 on each of 8 connections, each begun and never
/// finished. Closing the endpoints gives them back.
async fn take_social_places(net: &SimNet, node: EndpointId) -> Vec<Unfinished> {
    let mut taken = Vec::new();
    for _ in 0..8 {
        let ep = endpoint(net, delay(10)).await;
        let conn = ep.connect(sim_addr(node), SOCIAL_ALPN).await.unwrap();
        let mut requests = Vec::new();
        for _ in 0..4 {
            let (mut send, recv) = conn.open_bi().await.unwrap();
            // The first byte of a request's length: the node waits for more.
            send.write_all(&[0]).await.unwrap();
            requests.push((send, recv));
        }
        taken.push((ep, conn, requests));
    }
    taken
}

/// Alice, who posted a photo; a follower of hers, to ask whom her node
/// names; and Carol, who follows her and shares, whose node has the photo
/// but couldn't tell Alice's: others' requests took all its places for
/// them, and keep them until `end_others_requests`.
struct Unheard {
    alice: Node,
    carol: Node,
    blob: BlobHash,
    follower: (Endpoint, Connection),
    taken: Vec<Unfinished>,
    _dir: tempfile::TempDir,
}

impl Unheard {
    async fn set_up() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let net = SimNet::new(1);
        let alice = start(dir.path(), "alice", &net, delay(10)).await;
        let follower = endpoint(&net, delay(10)).await;
        let conn = follower
            .connect(sim_addr(alice.node_id()), SOCIAL_ALPN)
            .await
            .unwrap();
        let subscribe = SocialRequest::Subscribe {
            addr: follower.addr(),
            shares_prefetched: false,
            media: None,
            direct: None,
        };
        assert!(matches!(ask(&conn, &subscribe).await, SocialResponse::Ok));

        // Carol follows Alice, then Alice posts a photo. Carol downloads at
        // 2 Mbit/s: 12 s or so for the photo, and her node holds the new
        // post out of her feed until it's here.
        let slow = Link {
            down_bps: 2 * MBPS,
            ..delay(10)
        };
        let carol = start_sharing(dir.path(), "carol", &net, slow).await;
        let ClientResponse::Ticket { ticket } = call(&alice, ClientRequest::Invite).await else {
            panic!("no ticket");
        };
        call(&carol, ClientRequest::Follow { ticket }).await;
        call(&carol, ClientRequest::Sync).await;
        let photo = post_noise_photo(&alice, dir.path()).await;
        let blob = BlobHash::from_bytes(*photo.as_bytes());
        call(&carol, ClientRequest::Sync).await;
        assert_eq!(feed_len(&carol).await, 0);

        // Once Carol's node has asked who else holds a copy, others'
        // requests take all of Alice's node's places for them.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let taken = take_social_places(&net, alice.node_id()).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let resp = ask(&conn, &SocialRequest::WhoHosts { blob }).await;
        assert!(
            matches!(&resp, SocialResponse::Error { message } if message.contains("busy")),
            "{resp:?}"
        );

        // So when Carol's node has the photo, Alice's is too busy to hear
        // it. The post shows all the same.
        let started = Instant::now();
        while feed_len(&carol).await == 0 {
            assert!(started.elapsed() < Duration::from_secs(60), "still held");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(carol.blob_class(&blob).unwrap(), Some(BlobClass::Prefetch));
        Self {
            alice,
            carol,
            blob,
            follower: (follower, conn),
            taken,
            _dir: dir,
        }
    }

    /// Gives Alice's node its places back. Merely dropping an endpoint
    /// may not tell the other end, which then keeps the requests open until
    /// the connection times out.
    async fn end_others_requests(&mut self) {
        for (endpoint, ..) in self.taken.drain(..) {
            endpoint.close().await;
        }
    }

    /// Whether Alice's node names Carol's as holding a copy, or `None` if
    /// it's too busy to say.
    async fn carol_named(&self) -> Option<bool> {
        let req = SocialRequest::WhoHosts { blob: self.blob };
        match ask(&self.follower.1, &req).await {
            SocialResponse::Hosts { nodes } => {
                Some(nodes.iter().any(|a| a.id == self.carol.node_id()))
            }
            _ => None,
        }
    }

    async fn shut_down(self) {
        self.carol.shutdown().await.unwrap();
        self.alice.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_follower_whose_copy_went_unheard_tells_the_authors_node_again() {
    let mut unheard = Unheard::set_up().await;

    // Once the others are done, Carol's node tells Alice's again, within
    // 6 s, which then names her to its followers.
    unheard.end_others_requests().await;
    let started = Instant::now();
    while unheard.carol_named().await != Some(true) {
        assert!(started.elapsed() < Duration::from_secs(30), "never named");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    unheard.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_follower_whose_copy_went_unheard_tells_the_authors_node_again_only_once() {
    let mut unheard = Unheard::set_up().await;

    // Carol's node tells Alice's again within 6 s, while it's still busy,
    // and not after that.
    tokio::time::sleep(Duration::from_secs(9)).await;
    unheard.end_others_requests().await;
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert_eq!(unheard.carol_named().await, Some(false));
    unheard.shut_down().await;
}
