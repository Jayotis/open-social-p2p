//! Circuits through carriers on loopback: Jimmy's node (V) fetching from
//! Alice's (H) through P, a newcomer M and Alice's hop Q.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Result, anyhow};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, endpoint::presets};
use osp_carrier::{
    ALPN, Carrier, CarrierInfo, Config, Descriptor, Dial, HostEvent, HostLink, Limits,
    ViewerCircuit, inbox_drops,
};
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(10);

async fn endpoint(alpns: Vec<Vec<u8>>) -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Disabled)
        .alpns(alpns)
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap()
}

/// Where each carrier endpoint is on loopback, in place of a relay.
type Book = Arc<Mutex<HashMap<EndpointId, EndpointAddr>>>;

fn book_dial(endpoint: Endpoint, book: Book) -> Dial {
    Arc::new(move |info: CarrierInfo| {
        let endpoint = endpoint.clone();
        let addr = book.lock().unwrap().get(&info.endpoint).cloned();
        Box::pin(async move {
            let addr = addr.ok_or_else(|| anyhow!("not in the book"))?;
            Ok(endpoint.connect(addr, ALPN).await?)
        })
    })
}

struct Node {
    endpoint: Endpoint,
    carrier: Carrier,
}

impl Node {
    async fn descriptor(&self) -> Descriptor {
        self.carrier.descriptor().await.unwrap()
    }
}

async fn carrier(book: &Book, limits: Limits) -> Node {
    let endpoint = endpoint(vec![ALPN.to_vec()]).await;
    book.lock().unwrap().insert(endpoint.id(), endpoint.addr());
    let carrier = Carrier::spawn(
        endpoint.clone(),
        Config {
            machine: SecretKey::generate(),
            relay: "https://relay.example/".into(),
            offers: vec![],
            limits,
            dial: book_dial(endpoint.clone(), book.clone()),
            others: None,
        },
    )
    .unwrap();
    Node { endpoint, carrier }
}

/// Alice's node: a link to her hop Q, holding a slot.
async fn host(q: &Node) -> (Endpoint, HostLink) {
    let endpoint = endpoint(vec![]).await;
    let conn = endpoint.connect(q.endpoint.addr(), ALPN).await.unwrap();
    (endpoint, HostLink::open(conn).await.unwrap())
}

/// Jimmy's node: a fresh endpoint for this circuit, linked to its first hop.
async fn viewer(path: &[&Node], slot: osp_carrier::Slot) -> Result<(Endpoint, ViewerCircuit)> {
    let endpoint = endpoint(vec![]).await;
    let conn = endpoint.connect(path[0].endpoint.addr(), ALPN).await?;
    let mut hops = Vec::new();
    for n in path {
        hops.push(n.descriptor().await);
    }
    let circuit = ViewerCircuit::build(conn, &hops, slot).await?;
    Ok((endpoint, circuit))
}

/// A packet each way, and checks Alice's node is told the circuit came.
async fn round_trip(circuit: &mut ViewerCircuit, host: &mut HostLink) {
    let opened = match timeout(WAIT, host.recv()).await.unwrap().unwrap() {
        HostEvent::Opened(id) => id,
        other => panic!("expected the circuit to open, got {other:?}"),
    };
    for i in 0..20u8 {
        let packet = vec![i; 1200];
        circuit.send(packet.clone()).unwrap();
        match timeout(WAIT, host.recv()).await.unwrap().unwrap() {
            HostEvent::Packet(id, got) => {
                assert_eq!(id, opened);
                assert_eq!(got, packet);
            }
            other => panic!("expected a packet, got {other:?}"),
        }
        let reply = vec![i ^ 0xff; 1200];
        host.send(opened, reply.clone()).unwrap();
        assert_eq!(timeout(WAIT, circuit.recv()).await.unwrap().unwrap(), reply);
    }
}

#[tokio::test]
async fn a_packet_goes_through_two_hops_and_through_three() {
    let book = Book::default();
    let p = carrier(&book, Limits::home()).await;
    let m = carrier(&book, Limits::home()).await;
    let q = carrier(&book, Limits::seed()).await;
    let (_h, mut host) = host(&q).await;

    // V → P → Q → H
    let (_v, mut circuit) = viewer(&[&p, &q], host.slot()).await.unwrap();
    round_trip(&mut circuit, &mut host).await;

    // V → P → M → Q → H, on a newcomer's turn.
    let (_v2, mut circuit) = viewer(&[&p, &m, &q], host.slot()).await.unwrap();
    round_trip(&mut circuit, &mut host).await;

    // One hop, when both ends picked the same machine.
    let (_v3, mut circuit) = viewer(&[&q], host.slot()).await.unwrap();
    round_trip(&mut circuit, &mut host).await;

    assert!(m.carrier.stats().await.unwrap().bytes >= 2 * 20 * 1200);
}

#[tokio::test]
async fn a_made_up_slot_is_refused() {
    let book = Book::default();
    let p = carrier(&book, Limits::home()).await;
    let q = carrier(&book, Limits::home()).await;
    let (_h, _host) = host(&q).await;
    let err = viewer(&[&p, &q], [9; 16]).await.err().unwrap();
    assert!(format!("{err:#}").contains("ended"), "{err:#}");
    assert_eq!(q.carrier.stats().await.unwrap().refused, 1);
}

#[tokio::test]
async fn a_slot_is_only_its_own_hops() {
    // Alice's slot was given by Q. A circuit that ends at another carrier R
    // can't use it to reach her.
    let book = Book::default();
    let p = carrier(&book, Limits::home()).await;
    let q = carrier(&book, Limits::home()).await;
    let r = carrier(&book, Limits::home()).await;
    let (_h, host) = host(&q).await;
    assert!(viewer(&[&p, &r], host.slot()).await.is_err());
}

#[tokio::test]
async fn a_carrier_at_its_limit_says_busy() {
    let book = Book::default();
    let p = carrier(
        &book,
        Limits {
            per_neighbour: 1,
            ..Limits::home()
        },
    )
    .await;
    let q = carrier(&book, Limits::home()).await;
    let (_h, host) = host(&q).await;
    // Two circuits from one viewer endpoint, on two links: the second is one
    // too many from that neighbour.
    let endpoint = endpoint(vec![]).await;
    let hops = [p.descriptor().await, q.descriptor().await];
    let conn = endpoint.connect(p.endpoint.addr(), ALPN).await.unwrap();
    let _first = ViewerCircuit::build(conn, &hops, host.slot())
        .await
        .unwrap();
    let conn = endpoint.connect(p.endpoint.addr(), ALPN).await.unwrap();
    let err = ViewerCircuit::build(conn, &hops, host.slot())
        .await
        .err()
        .unwrap();
    assert!(format!("{err:#}").contains("busy"), "{err:#}");
    assert_eq!(p.carrier.stats().await.unwrap().busy, 1);
}

/// Jimmy's node keeps its link to its hop P open between fetches. A circuit
/// ended by its closer frees its place at P and stops reading the link, so
/// the next circuit on the same link is taken, and gets its own packets.
#[tokio::test]
async fn a_closed_circuits_link_carries_the_next() {
    let book = Book::default();
    let p = carrier(
        &book,
        Limits {
            per_neighbour: 1,
            ..Limits::home()
        },
    )
    .await;
    let q = carrier(&book, Limits::home()).await;
    let (_h, mut host) = host(&q).await;
    let endpoint = endpoint(vec![]).await;
    let hops = [p.descriptor().await, q.descriptor().await];
    let conn = endpoint.connect(p.endpoint.addr(), ALPN).await.unwrap();

    let mut first = ViewerCircuit::build(conn.clone(), &hops, host.slot())
        .await
        .unwrap();
    round_trip(&mut first, &mut host).await;
    let closer = first.closer();
    // As the circuit's endpoint would hold it on: only the closer ends it.
    let _kept = first;
    closer.close();
    timeout(WAIT, async {
        while p.carrier.stats().await.unwrap().circuits > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("P let the first circuit go");
    // Alice's node hears the first one end before the second comes.
    assert!(matches!(
        timeout(WAIT, host.recv()).await.unwrap().unwrap(),
        HostEvent::Closed(_)
    ));

    let mut second = ViewerCircuit::build(conn, &hops, host.slot())
        .await
        .unwrap();
    round_trip(&mut second, &mut host).await;
    assert_eq!(p.carrier.stats().await.unwrap().busy, 0);
}

#[tokio::test]
async fn new_circuits_a_minute_are_limited() {
    let book = Book::default();
    let q = carrier(
        &book,
        Limits {
            new_per_minute: 2,
            ..Limits::home()
        },
    )
    .await;
    let (_h, host) = host(&q).await;
    let mut built = Vec::new();
    for _ in 0..2 {
        built.push(viewer(&[&q], host.slot()).await.unwrap());
    }
    let err = viewer(&[&q], host.slot()).await.err().unwrap();
    assert!(format!("{err:#}").contains("busy"), "{err:#}");
}

#[tokio::test]
async fn a_paused_carrier_ends_its_circuits_and_says_busy_until_it_resumes() {
    let book = Book::default();
    let p = carrier(&book, Limits::home()).await;
    let q = carrier(&book, Limits::home()).await;
    let (_h, host) = host(&q).await;
    let (_v, mut circuit) = viewer(&[&p, &q], host.slot()).await.unwrap();
    // Kim pauses carrying on P, say for a metered connection.
    p.carrier.set_paused(true).await.unwrap();
    assert_eq!(timeout(WAIT, circuit.recv()).await.unwrap(), None);
    let err = viewer(&[&p, &q], host.slot()).await.err().unwrap();
    assert!(format!("{err:#}").contains("busy"), "{err:#}");
    let stats = p.carrier.stats().await.unwrap();
    assert!(stats.paused);
    assert_eq!(stats.circuits, 0);
    // A paused host's hop gives out no slots.
    let paused_q = carrier(&book, Limits::home()).await;
    paused_q.carrier.set_paused(true).await.unwrap();
    let conn = endpoint(vec![])
        .await
        .connect(paused_q.endpoint.addr(), ALPN)
        .await
        .unwrap();
    assert!(HostLink::open(conn).await.is_err());
    // Resumed, it carries again.
    p.carrier.set_paused(false).await.unwrap();
    viewer(&[&p, &q], host.slot()).await.unwrap();
}

#[tokio::test]
async fn new_limits_apply_to_the_next_circuit() {
    let book = Book::default();
    let q = carrier(&book, Limits::home()).await;
    let (_h, host) = host(&q).await;
    let _first = viewer(&[&q], host.slot()).await.unwrap();
    q.carrier
        .set_limits(Limits {
            circuits: 1,
            ..Limits::home()
        })
        .await
        .unwrap();
    let err = viewer(&[&q], host.slot()).await.err().unwrap();
    assert!(format!("{err:#}").contains("busy"), "{err:#}");
}

#[tokio::test]
async fn the_end_of_a_circuit_reaches_the_viewer() {
    let book = Book::default();
    let p = carrier(&book, Limits::home()).await;
    let m = carrier(&book, Limits::home()).await;
    let q = carrier(&book, Limits::home()).await;
    let (_h, mut host) = host(&q).await;
    let (_v, mut circuit) = viewer(&[&p, &m, &q], host.slot()).await.unwrap();
    let HostEvent::Opened(id) = timeout(WAIT, host.recv()).await.unwrap().unwrap() else {
        panic!("no circuit");
    };
    host.close(id);
    assert_eq!(timeout(WAIT, circuit.recv()).await.unwrap(), None);
    // And the carriers forgot it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    for n in [&p, &m, &q] {
        assert_eq!(n.carrier.stats().await.unwrap().circuits, 0);
    }
}

#[tokio::test]
async fn a_carrier_that_goes_ends_its_circuits() {
    let book = Book::default();
    let p = carrier(&book, Limits::home()).await;
    let m = carrier(&book, Limits::home()).await;
    let q = carrier(&book, Limits::home()).await;
    let (_h, mut host) = host(&q).await;
    let (_v, mut circuit) = viewer(&[&p, &m, &q], host.slot()).await.unwrap();
    // Kim's node (M) goes away.
    m.endpoint.close().await;
    drop(m);
    assert_eq!(timeout(WAIT, circuit.recv()).await.unwrap(), None);
    match timeout(WAIT, host.recv()).await.unwrap().unwrap() {
        HostEvent::Opened(_) => {
            assert!(matches!(
                timeout(WAIT, host.recv()).await.unwrap().unwrap(),
                HostEvent::Closed(_)
            ));
        }
        HostEvent::Closed(_) => {}
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_extend_to_a_carrier_that_cant_be_reached_says_busy() {
    let book = Book::default();
    let p = carrier(&book, Limits::home()).await;
    let q = carrier(&book, Limits::home()).await;
    let (_h, host) = host(&q).await;
    // Q's address is taken out of the book: P can't reach it.
    book.lock().unwrap().remove(&q.endpoint.id());
    let err = viewer(&[&p, &q], host.slot()).await.err().unwrap();
    assert!(format!("{err:#}").contains("busy"), "{err:#}");
}

#[tokio::test]
async fn a_path_through_one_carrier_twice_in_a_row_isnt_built() {
    let book = Book::default();
    let p = carrier(&book, Limits::home()).await;
    let q = carrier(&book, Limits::home()).await;
    let (_h, host) = host(&q).await;
    let endpoint = endpoint(vec![]).await;
    let conn = endpoint.connect(p.endpoint.addr(), ALPN).await.unwrap();
    let pp = [p.descriptor().await, p.descriptor().await];
    assert!(ViewerCircuit::build(conn, &pp, host.slot()).await.is_err());
}

#[tokio::test]
async fn the_production_dial_refuses_relays_the_node_doesnt_use() {
    let endpoint = endpoint(vec![]).await;
    let dial = osp_carrier::relay_dial(endpoint, |url| {
        url.as_str().starts_with("https://relay.example")
    });
    let mut info = CarrierInfo {
        version: osp_carrier::VERSION,
        machine: SecretKey::generate().public(),
        endpoint: SecretKey::generate().public(),
        relay: "https://evil.example/".into(),
        circuit_key: [0; 32],
        valid_from: 0,
        valid_until: 1,
        offers: vec![],
    };
    let err = dial(info.clone()).await.err().unwrap();
    assert!(format!("{err:#}").contains("doesn't use"), "{err:#}");
    info.relay = "not a url".into();
    assert!(dial(info).await.is_err());
}

/// Bytes that differ in every block, so a part sent wrong can't pass.
fn video(len: usize) -> Vec<u8> {
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// Every path a connection has: only the circuit, never an IP address or
/// a relay.
fn only_the_circuit(conn: &iroh::endpoint::Connection) {
    let paths = conn.paths();
    assert!(!paths.is_empty());
    for path in paths.iter() {
        assert!(
            !path.is_ip() && !path.is_relay(),
            "{:?}",
            path.remote_addr()
        );
        assert!(
            matches!(path.remote_addr(), iroh::TransportAddr::Custom(a) if a.id() == osp_carrier::TRANSPORT_ID),
            "{:?}",
            path.remote_addr()
        );
    }
}

#[tokio::test]
async fn a_video_goes_over_circuits_and_passes_its_hash_check() {
    use iroh_blobs::{BlobsProtocol, store::mem::MemStore};

    let book = Book::default();
    let p = carrier(&book, Limits::home()).await;
    let m = carrier(&book, Limits::home()).await;
    let q = carrier(&book, Limits::seed()).await;

    // Alice's node: her video, and her circuit endpoint on a link to Q.
    let alice = SecretKey::generate();
    let store = MemStore::new();
    let data = video(4 << 20);
    let hash = store.add_bytes(data.clone()).await.unwrap().hash;
    let (_h, link) = host(&q).await;
    let slot = link.slot();
    let circuit_endpoint =
        osp_carrier::host_endpoint(link, alice.clone(), vec![iroh_blobs::ALPN.to_vec()])
            .await
            .unwrap();
    for a in circuit_endpoint.addr().addrs.iter() {
        assert!(matches!(a, iroh::TransportAddr::Custom(_)), "{a:?}");
    }
    let _router = iroh::protocol::Router::builder(circuit_endpoint)
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
        .spawn();

    // Jimmy fetches it through P, M and Q; Robin at the same time through
    // P and Q.
    let mut fetches = Vec::new();
    for path in [vec![&p, &m, &q], vec![&p, &q]] {
        let (link_endpoint, circuit) = viewer(&path, slot).await.unwrap();
        let ends = osp_carrier::viewer_endpoint(circuit, alice.public())
            .await
            .unwrap();
        for a in ends.addr().addrs.iter() {
            assert!(matches!(a, iroh::TransportAddr::Custom(_)), "{a:?}");
        }
        let data = data.clone();
        let alice = alice.public();
        fetches.push(tokio::spawn(async move {
            let conn = ends
                .connect(EndpointAddr::new(alice), iroh_blobs::ALPN)
                .await
                .unwrap();
            assert_eq!(conn.remote_id(), alice);
            let mine = MemStore::new();
            timeout(
                Duration::from_secs(60),
                mine.remote().fetch(conn.clone(), hash),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(mine.get_bytes(hash).await.unwrap(), data);
            only_the_circuit(&conn);
            drop(link_endpoint);
        }));
    }
    for f in fetches {
        f.await.unwrap();
    }
    assert!(m.carrier.stats().await.unwrap().bytes >= 4 << 20);
}

#[tokio::test]
async fn a_hop_over_its_share_slows_the_video_rather_than_losing_it() {
    use iroh_blobs::{BlobsProtocol, store::mem::MemStore};

    // Robin's hop P is a home node with 250 kB/s to share: under what a
    // circuit carries here, so the share sets the pace.
    let book = Book::default();
    let share = 250_000;
    let p = carrier(
        &book,
        Limits {
            bytes_per_sec: Some(share),
            ..Limits::home()
        },
    )
    .await;
    let q = carrier(&book, Limits::seed()).await;

    let alice = SecretKey::generate();
    let store = MemStore::new();
    let data = video(2 << 20);
    let hash = store.add_bytes(data.clone()).await.unwrap().hash;
    let (_h, link) = host(&q).await;
    let slot = link.slot();
    let circuit_endpoint =
        osp_carrier::host_endpoint(link, alice.clone(), vec![iroh_blobs::ALPN.to_vec()])
            .await
            .unwrap();
    let _router = iroh::protocol::Router::builder(circuit_endpoint)
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
        .spawn();

    let (_link_endpoint, circuit) = viewer(&[&p, &q], slot).await.unwrap();
    let ends = osp_carrier::viewer_endpoint(circuit, alice.public())
        .await
        .unwrap();
    let conn = ends
        .connect(EndpointAddr::new(alice.public()), iroh_blobs::ALPN)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let mine = MemStore::new();
    timeout(Duration::from_secs(60), mine.remote().fetch(conn, hash))
        .await
        .unwrap()
        .unwrap();
    let took = started.elapsed();
    assert_eq!(mine.get_bytes(hash).await.unwrap(), data);
    let stats = p.carrier.stats().await.unwrap();
    // A second of the share goes at once; the rest at the share.
    let least = Duration::from_secs_f64(((2 << 20) - share) as f64 / share as f64 * 0.9);
    assert!(took >= least, "{took:?}, {stats:?}");
    // Most packets over the share wait for it. Some still go when the
    // queue is full: the ends' QUIC grows its window until one does, here
    // on loopback most of all, and slows down then. Dropping each packet
    // over the share, as before, lost about 1,170 here and held none.
    assert!(
        stats.held > 0 && stats.dropped * 2 < stats.held,
        "{stats:?}"
    );
    // The full queue's drops are counted apart, and among all drops.
    assert!(stats.share_full <= stats.dropped, "{stats:?}");
}

/// Alice's node sends Jimmy's circuit more packets than its inbox holds,
/// and Jimmy's end reads none of them until the last is sent. Those over
/// the inbox are dropped at his end, and each one is counted: a full inbox
/// never drops a packet unseen again (stage D's sixth run broke on losses
/// nothing counted).
#[tokio::test]
async fn a_full_viewer_inbox_counts_each_packet_it_drops() {
    let book = Book::default();
    let p = carrier(&book, Limits::home()).await;
    let q = carrier(&book, Limits::home()).await;
    let (_h, mut host) = host(&q).await;
    let endpoint = endpoint(vec![]).await;
    let hops = [p.descriptor().await, q.descriptor().await];
    let conn = endpoint.connect(p.endpoint.addr(), ALPN).await.unwrap();
    let mut circuit = ViewerCircuit::build(conn, &hops, host.slot())
        .await
        .unwrap();
    let opened = match timeout(WAIT, host.recv()).await.unwrap().unwrap() {
        HostEvent::Opened(id) => id,
        other => panic!("expected the circuit to open, got {other:?}"),
    };

    let before = inbox_drops().viewer;
    // Sent in small batches, so the links' own queues keep up and the
    // packets reach Jimmy's end: it's his inbox that overflows.
    const SENT: usize = 3000;
    for batch in 0..SENT / 50 {
        for i in 0..50 {
            host.send(opened, vec![(batch * 50 + i) as u8; 1000])
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Every packet sent came to his end: kept in the inbox or counted.
    let mut kept = 0;
    let dropped = timeout(WAIT, async {
        loop {
            while let Ok(Some(_)) = timeout(Duration::from_millis(500), circuit.recv()).await {
                kept += 1;
            }
            let dropped = (inbox_drops().viewer - before) as usize;
            if kept + dropped >= SENT {
                return dropped;
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "{kept} kept and {} counted, of {SENT} sent",
            inbox_drops().viewer - before
        )
    });
    assert!(dropped > 0, "the inbox overflowed: {kept} kept");
    assert_eq!(kept + dropped, SENT);
}
