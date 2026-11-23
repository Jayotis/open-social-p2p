//! What the carrier tests share: loopback endpoints, carriers that find
//! each other through a book in place of a relay, and the two ends.
#![allow(dead_code)]

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
    ViewerCircuit,
};
use tokio::time::timeout;

pub const WAIT: Duration = Duration::from_secs(10);

pub async fn endpoint(alpns: Vec<Vec<u8>>) -> Endpoint {
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
pub type Book = Arc<Mutex<HashMap<EndpointId, EndpointAddr>>>;

pub fn book_dial(endpoint: Endpoint, book: Book) -> Dial {
    Arc::new(move |info: CarrierInfo| {
        let endpoint = endpoint.clone();
        let addr = book.lock().unwrap().get(&info.endpoint).cloned();
        Box::pin(async move {
            let addr = addr.ok_or_else(|| anyhow!("not in the book"))?;
            Ok(endpoint.connect(addr, ALPN).await?)
        })
    })
}

pub struct Node {
    pub endpoint: Endpoint,
    pub carrier: Carrier,
}

impl Node {
    pub async fn descriptor(&self) -> Descriptor {
        self.carrier.descriptor().await.unwrap()
    }
}

pub async fn carrier(book: &Book, limits: Limits) -> Node {
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
pub async fn host(q: &Node) -> (Endpoint, HostLink) {
    let endpoint = endpoint(vec![]).await;
    let conn = endpoint.connect(q.endpoint.addr(), ALPN).await.unwrap();
    (endpoint, HostLink::open(conn).await.unwrap())
}

/// Jimmy's node: a fresh endpoint for this circuit, linked to its first hop.
pub async fn viewer(path: &[&Node], slot: osp_carrier::Slot) -> Result<(Endpoint, ViewerCircuit)> {
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
pub async fn round_trip(circuit: &mut ViewerCircuit, host: &mut HostLink) {
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
