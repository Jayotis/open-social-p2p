//! The transport on loopback, without Tor: streams go through relays the
//! tests can cut, as a Tor restart on either side cuts them.

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use iroh::{
    Endpoint, EndpointId, SecretKey, TransportAddr,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
};
use osp_tor::{Dial, TorTransport};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::AbortHandle,
    time::timeout,
};

const ALPN: &[u8] = b"osp-tor-test/0";

/// Where each node takes streams, and the relays in between.
#[derive(Default)]
struct Net {
    listeners: Mutex<HashMap<EndpointId, SocketAddr>>,
    relays: Mutex<Vec<AbortHandle>>,
    /// Dials fail while this is set, as they do while Tor is down.
    down: AtomicBool,
    dials: Mutex<HashMap<EndpointId, usize>>,
}

impl Net {
    /// Cuts every stream, as Tor restarting would.
    fn cut(&self) {
        for relay in self.relays.lock().unwrap().drain(..) {
            relay.abort();
        }
    }

    /// How many times `node` has dialled.
    fn dials(&self, node: EndpointId) -> usize {
        self.dials.lock().unwrap().get(&node).copied().unwrap_or(0)
    }

    fn dial(self: &Arc<Self>, from: EndpointId) -> Dial {
        let net = self.clone();
        Arc::new(move |to| {
            let net = net.clone();
            Box::pin(async move {
                *net.dials.lock().unwrap().entry(from).or_default() += 1;
                if net.down.load(Relaxed) {
                    return Err(io::Error::other("Tor isn't running"));
                }
                let target = net.listeners.lock().unwrap().get(&to).copied();
                let target = target.ok_or_else(|| io::Error::other("no onion service"))?;
                // The dialler's end, and a relay to the target that `cut` ends.
                let near = TcpListener::bind("127.0.0.1:0").await?;
                let ours = TcpStream::connect(near.local_addr()?).await?;
                let (mut theirs, _) = near.accept().await?;
                let mut target = TcpStream::connect(target).await?;
                let relay = tokio::spawn(async move {
                    let _ = tokio::io::copy_bidirectional(&mut theirs, &mut target).await;
                });
                net.relays.lock().unwrap().push(relay.abort_handle());
                Ok(ours)
            })
        })
    }

    /// A node on this network: reachable, or one that only dials.
    async fn node(self: &Arc<Self>, reachable: bool) -> Result<(Endpoint, TorTransport)> {
        let key = SecretKey::generate();
        let listener = if reachable {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            self.listeners
                .lock()
                .unwrap()
                .insert(key.public(), listener.local_addr()?);
            Some(listener)
        } else {
            None
        };
        let transport = TorTransport::with_dial(key.clone(), self.dial(key.public()), listener);
        let endpoint = Endpoint::builder(transport.preset())
            .secret_key(key)
            .bind()
            .await?;
        Ok((endpoint, transport))
    }
}

#[derive(Debug, Clone)]
struct Echo;

impl ProtocolHandler for Echo {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            tokio::spawn(async move {
                if let Ok(data) = recv.read_to_end(1 << 20).await {
                    let _ = send.write_all(&data).await;
                    let _ = send.finish();
                }
            });
        }
        Ok(())
    }
}

async fn echo(conn: &Connection, data: &[u8]) -> Result<()> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(data).await?;
    send.finish()?;
    let back = recv.read_to_end(1 << 20).await?;
    anyhow::ensure!(back == data, "echoed something else");
    Ok(())
}

#[tokio::test]
async fn nodes_talk_on_one_stream_both_ways_and_nothing_else() -> Result<()> {
    let net = Arc::new(Net::default());
    let (a, a_transport) = net.node(true).await?;
    let (b, b_transport) = net.node(true).await?;
    let _router = Router::builder(b.clone()).accept(ALPN, Echo).spawn();

    // By ID alone.
    let conn = timeout(Duration::from_secs(10), a.connect(b.id(), ALPN)).await??;
    echo(&conn, b"hello").await?;
    echo(&conn, &vec![7; 200_000]).await?;

    assert_eq!(a_transport.streams().open, 1);
    assert_eq!(b_transport.streams().open, 1);
    assert_eq!(net.dials(b.id()), 0, "b answers on a's stream");
    for ep in [&a, &b] {
        assert!(ep.bound_sockets().is_empty(), "no IP sockets");
        assert!(
            ep.addr()
                .addrs
                .iter()
                .all(|a| matches!(a, TransportAddr::Custom(_))),
            "no addresses but the transport's"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_node_that_only_dials_is_answered() -> Result<()> {
    let net = Arc::new(Net::default());
    let (a, _) = net.node(false).await?;
    let (b, _) = net.node(true).await?;
    let _router = Router::builder(b.clone()).accept(ALPN, Echo).spawn();
    let conn = timeout(Duration::from_secs(10), a.connect(b.id(), ALPN)).await??;
    echo(&conn, b"hello").await
}

#[tokio::test]
async fn talk_goes_on_after_the_streams_are_cut() -> Result<()> {
    let net = Arc::new(Net::default());
    let (a, _) = net.node(true).await?;
    let (b, _) = net.node(true).await?;
    let _router = Router::builder(b.clone()).accept(ALPN, Echo).spawn();
    let conn = timeout(Duration::from_secs(10), a.connect(b.id(), ALPN)).await??;
    echo(&conn, b"before").await?;

    net.cut();
    // The same connection: QUIC sends again, and a stream is dialled again.
    timeout(Duration::from_secs(10), echo(&conn, b"after"))
        .await
        .context("no answer after the cut")??;
    Ok(())
}

#[tokio::test]
async fn failed_dials_pause_and_recover() -> Result<()> {
    let net = Arc::new(Net::default());
    let (a, _) = net.node(true).await?;
    let (b, _) = net.node(true).await?;
    let _router = Router::builder(b.clone()).accept(ALPN, Echo).spawn();

    net.down.store(true, Relaxed);
    let connecting = tokio::spawn({
        let a = a.clone();
        let b = b.id();
        async move { a.connect(b, ALPN).await }
    });
    tokio::time::sleep(Duration::from_millis(3500)).await;
    // At 0 s, then 1 s and 2 s after each failure: not every packet QUIC sent.
    let dials = net.dials(a.id());
    assert!((2..=3).contains(&dials), "{dials} dials");

    net.down.store(false, Relaxed);
    let conn = timeout(Duration::from_secs(10), connecting).await???;
    echo(&conn, b"hello").await
}

#[tokio::test]
async fn streams_that_dont_prove_a_key_are_closed() -> Result<()> {
    let net = Arc::new(Net::default());
    let (b, b_transport) = net.node(true).await?;
    let addr = net.listeners.lock().unwrap()[&b.id()];
    let mut stream = TcpStream::connect(addr).await?;
    let mut hello = [0; 40];
    stream.read_exact(&mut hello).await?;
    assert_eq!(&hello[..8], b"osp-tor1");
    let key = SecretKey::generate();
    stream.write_all(key.public().as_bytes()).await?;
    stream.write_all(&[0; 64]).await?;
    let mut rest = Vec::new();
    timeout(Duration::from_secs(5), stream.read_to_end(&mut rest)).await??;
    assert!(rest.is_empty(), "closed without a word");
    assert_eq!(b_transport.streams().open, 0);
    Ok(())
}
