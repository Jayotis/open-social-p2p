//! The transport: iroh's QUIC packets carried on TCP streams through Tor,
//! between onion services.
//!
//! Unlike `iroh-tor-transport`, which this started from, one stream carries
//! a pair of nodes' packets both ways, and the node that dialled proves its
//! key on it first:
//!
//! 1. The node that accepted the stream sends [`MAGIC`] and 32 random bytes.
//! 2. The node that dialled sends its ID and its signature over
//!    [`CONTEXT`], the other node's ID and those bytes.
//! 3. Then packets go both ways, each as a 2-byte length and the packet.
//!
//! The node that dialled knows whom it reached, as Tor checks the onion
//! address, and that is the other node's key. Once the other node knows
//! too, it sends back on the same stream. So a pair of nodes needs one
//! circuit, not one each way; a node never dials anyone because a packet
//! named them; and a node that only dials needs no onion service.

use std::{
    collections::HashMap,
    fmt, io,
    net::Ipv4Addr,
    num::NonZeroUsize,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
    },
    task::{Context, Poll},
    time::Duration,
};

use iroh::{
    EndpointAddr, EndpointId, SecretKey, Signature, TransportAddr,
    address_lookup::{AddressLookup, EndpointData, EndpointInfo, Error, Item},
    endpoint::{
        Builder, QuicTransportConfig, QuicTransportConfigBuilder,
        presets::{Minimal, Preset},
        transports::{CustomEndpoint, CustomSender, CustomTransport, RecvInfo, Transmit},
    },
};
use iroh_base::CustomAddr;
use n0_future::boxed::{BoxFuture, BoxStream};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter},
    net::{TcpListener, TcpStream},
    sync::{Notify, mpsc},
    task::JoinSet,
    time::Instant,
};
use tracing::{debug, warn};

use crate::{
    onion, socks,
    tor::{OnionService, Phase, Tor},
};

/// Marks addresses on this transport among custom transports'.
pub const TRANSPORT_ID: u64 = 0x6f73_7074_6f72; // "osptor"
/// The port streams go to at a node's onion address. Any would do: it's
/// never published.
pub const ONION_PORT: u16 = 677;
/// Starts every stream, from the node that accepted it. It names this
/// wire format, which isn't upstream's.
const MAGIC: &[u8; 8] = b"osp-tor1";
/// What the node that dialled signs, before the other node's ID and the
/// random bytes, so the signature can't be used for anything else.
const CONTEXT: &[u8] = b"osp-tor1: the dialling node's key";
/// Longest a dial may go on, the proof included, trying again after each
/// failure while packets wait for it. A first one took up to 24 s in M4's
/// test, and a minute at times.
const DIAL_TIMEOUT: Duration = Duration::from_secs(90);
/// Longest a QUIC connection over Tor goes without hearing from the other
/// node before it's given up, its handshake included: a first connection
/// gets a minute (ADR 0014), not iroh's 30 s. Both sides must set it, as
/// the shorter wins; [`TorTransport::preset`] does.
pub const MAX_IDLE: Duration = Duration::from_secs(60);
/// Longest the node that accepted a stream waits for the proof: a round
/// trip or two over Tor.
const PROOF_TIMEOUT: Duration = Duration::from_secs(60);
/// A stream that brings nothing for this long is closed, and its circuit
/// with it. Until then, a new connection between the pair reuses it.
const IDLE: Duration = Duration::from_secs(300);
/// Packets waiting to go out on one stream, or for it while it's dialled.
/// More are lost, as a full socket buffer loses them; QUIC sends them again.
const STREAM_QUEUE: usize = 256;
/// Packets received, waiting for iroh to read them.
const RECEIVE_BUFFER: usize = 4096;
/// Streams open at once, dials at once, and streams accepted that haven't
/// proved their key yet. Packets to a node that would need a stream beyond
/// these are lost until there's room.
const MAX_STREAMS: usize = 1024;
const MAX_DIALS: usize = 64;
const MAX_UNPROVEN: usize = 128;
/// After a try at dialling fails, the next waits: the first of these,
/// doubling each time, up to the second. So does a new dial to a node after
/// one gave up on it.
const RETRY: (Duration, Duration) = (Duration::from_secs(1), Duration::from_secs(30));

/// Opens a stream to a node: through Tor to its onion service or, in
/// tests, anywhere.
pub type Dial = Arc<dyn Fn(EndpointId) -> BoxFuture<io::Result<TcpStream>> + Send + Sync>;

/// A packet received, and the node it came from.
type Received = (EndpointId, Vec<u8>);

/// A node's address on this transport.
pub fn addr(node: EndpointId) -> CustomAddr {
    CustomAddr::from_parts(TRANSPORT_ID, node.as_bytes())
}

fn node_of(addr: &CustomAddr) -> Option<EndpointId> {
    if addr.id() != TRANSPORT_ID {
        return None;
    }
    EndpointId::from_bytes(addr.data().try_into().ok()?).ok()
}

/// The streams a transport has now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Streams {
    /// Open, both ways, with a node whose key is proved.
    pub open: usize,
    pub dialling: usize,
    /// Accepted, waiting for the other node's proof.
    pub unproven: usize,
}

/// The transport for one node, to put on its endpoint with [`preset`] or
/// with `Builder::add_custom_transport` and [`lookup`].
///
/// [`preset`]: TorTransport::preset
/// [`lookup`]: TorTransport::lookup
#[derive(Clone)]
pub struct TorTransport {
    shared: Arc<Shared>,
}

impl fmt::Debug for TorTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TorTransport")
            .field("node", &self.shared.id().fmt_short().to_string())
            .field("streams", &self.streams())
            .finish()
    }
}

struct Shared {
    key: SecretKey,
    dial: Dial,
    /// Where streams to the node's onion service arrive, until the
    /// endpoint takes it.
    listener: Mutex<Option<TcpListener>>,
    received_tx: mpsc::Sender<Received>,
    received: Mutex<Option<mpsc::Receiver<Received>>>,
    peers: Mutex<HashMap<EndpointId, Peer>>,
    next_stream: AtomicU64,
    streams: AtomicUsize,
    dialling: AtomicUsize,
    unproven: AtomicUsize,
    tasks: Mutex<JoinSet<()>>,
    /// Told when Tor becomes ready, so dials waiting to try again go now.
    tor_ready: Notify,
    /// Kept up by the node's Tor while the transport lasts.
    _service: Option<OnionService>,
}

#[derive(Default)]
struct Peer {
    /// Its streams, newest last, the one being dialled among them.
    streams: Vec<StreamTx>,
    /// After a failed dial, none before this.
    retry_at: Option<Instant>,
    failures: u32,
}

struct StreamTx {
    id: u64,
    tx: mpsc::Sender<Vec<u8>>,
    /// Open and proved, not still being dialled.
    open: bool,
}

impl TorTransport {
    /// A transport through `tor` for the node with `key`, which other nodes
    /// reach at the onion address its ID gives.
    pub async fn new(tor: &Tor, key: SecretKey) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let service = tor.serve(key.clone(), ONION_PORT, listener.local_addr()?);
        let transport = Self::build(key, tor_dial(tor), Some(listener), Some(service));
        transport.retry_once_ready(tor);
        Ok(transport)
    }

    /// A transport through `tor` that only dials. Nobody can reach it, but
    /// the nodes it reaches answer on the same stream.
    pub fn dial_only(tor: &Tor, key: SecretKey) -> Self {
        let transport = Self::build(key, tor_dial(tor), None, None);
        transport.retry_once_ready(tor);
        transport
    }

    /// A transport whose streams go wherever `dial` opens them, not through
    /// Tor, and which accepts streams on `listener`, if given. For tests.
    pub fn with_dial(key: SecretKey, dial: Dial, listener: Option<TcpListener>) -> Self {
        Self::build(key, dial, listener, None)
    }

    fn build(
        key: SecretKey,
        dial: Dial,
        listener: Option<TcpListener>,
        service: Option<OnionService>,
    ) -> Self {
        let (received_tx, received) = mpsc::channel(RECEIVE_BUFFER);
        Self {
            shared: Arc::new(Shared {
                key,
                dial,
                listener: Mutex::new(listener),
                received_tx,
                received: Mutex::new(Some(received)),
                peers: Mutex::default(),
                next_stream: AtomicU64::new(0),
                streams: AtomicUsize::new(0),
                dialling: AtomicUsize::new(0),
                unproven: AtomicUsize::new(0),
                tasks: Mutex::default(),
                tor_ready: Notify::new(),
                _service: service,
            }),
        }
    }

    /// Dials again at once, instead of waiting out failed dials' pauses,
    /// whenever Tor becomes ready: the dials likely failed because it
    /// wasn't.
    fn retry_once_ready(&self, tor: &Tor) {
        let mut state = tor.state();
        let shared = Arc::downgrade(&self.shared);
        self.shared.spawn(async move {
            let mut ready = state.borrow_and_update().phase == Phase::Ready;
            while state.changed().await.is_ok() {
                let now = state.borrow_and_update().phase == Phase::Ready;
                if now && !ready {
                    let Some(shared) = Weak::upgrade(&shared) else {
                        return;
                    };
                    shared.retry_now();
                }
                ready = now;
            }
        });
    }

    /// Sets up an endpoint to use this transport and nothing else: no IP
    /// sockets, no relays, and no address lookup but this transport's.
    pub fn preset(&self) -> impl Preset + use<> {
        TorPreset {
            transport: self.clone(),
        }
    }

    /// Finds any node by its ID alone: its address on this transport
    /// follows from it.
    pub fn lookup(&self) -> impl AddressLookup + use<> {
        Lookup
    }

    pub fn streams(&self) -> Streams {
        let s = &self.shared;
        let dialling = s.dialling.load(Relaxed);
        Streams {
            open: s.streams.load(Relaxed).saturating_sub(dialling),
            dialling,
            unproven: s.unproven.load(Relaxed),
        }
    }
}

fn tor_dial(tor: &Tor) -> Dial {
    let tor = tor.clone();
    Arc::new(move |node| {
        let socks = tor.socks();
        Box::pin(async move {
            let socks = socks.ok_or_else(|| io::Error::other("Tor isn't running"))?;
            socks::connect(socks, &onion::host(&node), ONION_PORT).await
        })
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn id(&self) -> EndpointId {
        self.key.public()
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = lock(&self.tasks);
        while tasks.try_join_next().is_some() {}
        tasks.spawn(task);
    }

    /// Stops every stream: the endpoint has closed.
    fn close(&self) {
        lock(&self.tasks).abort_all();
    }

    fn retry_now(&self) {
        for peer in lock(&self.peers).values_mut() {
            peer.retry_at = None;
            peer.failures = 0;
        }
        self.tor_ready.notify_waiters();
    }

    fn has_open(&self, node: &EndpointId) -> bool {
        lock(&self.peers)
            .get(node)
            .is_some_and(|peer| peer.streams.iter().any(|s| s.open))
    }

    /// Sends packets to `to` on its newest open stream, or on the one
    /// being dialled, or dials one.
    fn send<'a>(self: &Arc<Self>, to: EndpointId, packets: impl Iterator<Item = &'a [u8]>) {
        let mut peers = lock(&self.peers);
        let peer = peers.entry(to).or_default();
        let stream = peer
            .streams
            .iter()
            .rev()
            .find(|s| s.open)
            .or(peer.streams.last());
        if let Some(stream) = stream {
            for packet in packets {
                // Full: lost, as a full socket buffer would lose it.
                let _ = stream.tx.try_send(packet.to_vec());
            }
            return;
        }
        if peer.retry_at.is_some_and(|at| Instant::now() < at)
            || self.dialling.load(Relaxed) >= MAX_DIALS
            || self.streams.load(Relaxed) >= MAX_STREAMS
        {
            return;
        }
        let (tx, rx) = mpsc::channel(STREAM_QUEUE);
        for packet in packets {
            let _ = tx.try_send(packet.to_vec());
        }
        let id = self.next_stream.fetch_add(1, Relaxed);
        peer.streams.push(StreamTx {
            id,
            tx,
            open: false,
        });
        drop(peers);
        let guard = StreamGuard::new(self, to, id);
        self.dialling.fetch_add(1, Relaxed);
        self.spawn(self.clone().dial(to, guard, rx));
    }

    async fn dial(
        self: Arc<Self>,
        to: EndpointId,
        guard: StreamGuard,
        rx: mpsc::Receiver<Vec<u8>>,
    ) {
        // Tries again after a failure, rather than waiting for QUIC's next
        // packet: QUIC waits longer each time it sends again.
        let give_up = Instant::now() + DIAL_TIMEOUT;
        let mut pause = RETRY.0;
        let dialled = loop {
            let tried = tokio::time::timeout_at(give_up, async {
                let mut stream = (self.dial)(to).await?;
                prove(&mut stream, &self.key, &to).await?;
                Ok::<_, io::Error>(stream)
            })
            .await;
            match tried {
                Ok(Ok(stream)) => break Ok(stream),
                Ok(Err(e)) if Instant::now() + pause < give_up => {
                    debug!(node = %to.fmt_short(), "couldn't reach the node over Tor: {e}; trying again in {}s", pause.as_secs());
                    tokio::select! {
                        _ = tokio::time::sleep(pause) => {}
                        _ = self.tor_ready.notified() => {}
                    }
                    pause = (pause * 2).min(RETRY.1);
                    if self.has_open(&to) {
                        // The node dialled us meanwhile: its stream will do.
                        break Err(None);
                    }
                }
                Ok(Err(e)) => break Err(Some(e.to_string())),
                Err(_) => break Err(Some("timed out".to_string())),
            }
        };
        self.dialling.fetch_sub(1, Relaxed);
        let stream = match dialled {
            Ok(stream) => stream,
            Err(None) => return,
            Err(Some(why)) => {
                let mut peers = lock(&self.peers);
                if let Some(peer) = peers.get_mut(&to) {
                    peer.failures += 1;
                    let wait = RETRY
                        .0
                        .saturating_mul(1 << peer.failures.min(16).saturating_sub(1))
                        .min(RETRY.1);
                    peer.retry_at = Some(Instant::now() + wait);
                    debug!(node = %to.fmt_short(), "gave up reaching the node over Tor: {why}; no new dial for {}s", wait.as_secs());
                }
                drop(peers);
                drop(guard);
                return;
            }
        };
        if let Some(peer) = lock(&self.peers).get_mut(&to) {
            if let Some(s) = peer.streams.iter_mut().find(|s| s.id == guard.id) {
                s.open = true;
            }
            peer.retry_at = None;
            peer.failures = 0;
        }
        debug!(node = %to.fmt_short(), "stream open (dialled)");
        self.run(to, stream, rx).await;
        drop(guard);
    }

    async fn accept_loop(self: Arc<Self>, listener: TcpListener) {
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    // Out of file descriptors, say: wait, and go on.
                    warn!("couldn't accept a stream from Tor: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            if self.unproven.load(Relaxed) >= MAX_UNPROVEN
                || self.streams.load(Relaxed) >= MAX_STREAMS
            {
                continue;
            }
            self.unproven.fetch_add(1, Relaxed);
            self.spawn(self.clone().accept(stream));
        }
    }

    async fn accept(self: Arc<Self>, mut stream: TcpStream) {
        let proved = tokio::time::timeout(PROOF_TIMEOUT, challenge(&mut stream, &self.id())).await;
        self.unproven.fetch_sub(1, Relaxed);
        let from = match proved {
            Ok(Ok(from)) => from,
            Ok(Err(e)) => {
                debug!("refused a stream: {e}");
                return;
            }
            Err(_) => {
                debug!("refused a stream: no proof of its key in time");
                return;
            }
        };
        let (tx, rx) = mpsc::channel(STREAM_QUEUE);
        let id = self.next_stream.fetch_add(1, Relaxed);
        {
            let mut peers = lock(&self.peers);
            let peer = peers.entry(from).or_default();
            peer.streams.push(StreamTx { id, tx, open: true });
            peer.retry_at = None;
            peer.failures = 0;
        }
        let guard = StreamGuard::new(&self, from, id);
        debug!(node = %from.fmt_short(), "stream open (accepted)");
        self.run(from, stream, rx).await;
        drop(guard);
    }

    /// Carries packets both ways until the stream ends or brings nothing
    /// for a while.
    async fn run(&self, peer: EndpointId, stream: TcpStream, mut rx: mpsc::Receiver<Vec<u8>>) {
        let (read, write) = stream.into_split();
        let reading = async {
            let mut read = BufReader::new(read);
            loop {
                match tokio::time::timeout(IDLE, read_packet(&mut read)).await {
                    Ok(Ok(Some(packet))) if packet.is_empty() => {}
                    Ok(Ok(Some(packet))) => {
                        if self.received_tx.send((peer, packet)).await.is_err() {
                            return "the endpoint closed".to_string();
                        }
                    }
                    Ok(Ok(None)) => return "closed".to_string(),
                    Ok(Err(e)) => return e.to_string(),
                    Err(_) => return "idle".to_string(),
                }
            }
        };
        let writing = async {
            let mut write = BufWriter::new(write);
            while let Some(packet) = rx.recv().await {
                write_packet(&mut write, &packet).await?;
                // Those waiting go out together: Tor sends in cells of
                // about 500 bytes, so fuller writes waste less.
                while let Ok(packet) = rx.try_recv() {
                    write_packet(&mut write, &packet).await?;
                }
                write.flush().await?;
            }
            Ok::<_, io::Error>(())
        };
        let why = tokio::select! {
            why = reading => why,
            wrote = writing => match wrote {
                Ok(()) => "closed".to_string(),
                Err(e) => e.to_string(),
            },
        };
        debug!(node = %peer.fmt_short(), "stream closed: {why}");
    }
}

/// Counts a stream while it lasts, and takes it off its node's list when
/// it ends, however it ends.
struct StreamGuard {
    shared: Arc<Shared>,
    peer: EndpointId,
    id: u64,
}

impl StreamGuard {
    fn new(shared: &Arc<Shared>, peer: EndpointId, id: u64) -> Self {
        shared.streams.fetch_add(1, Relaxed);
        Self {
            shared: shared.clone(),
            peer,
            id,
        }
    }
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.shared.streams.fetch_sub(1, Relaxed);
        let mut peers = lock(&self.shared.peers);
        if let Some(peer) = peers.get_mut(&self.peer) {
            peer.streams.retain(|s| s.id != self.id);
            if peer.streams.is_empty() && peer.retry_at.is_none() {
                peers.remove(&self.peer);
            }
        }
    }
}

/// The accepting side: asks the node that dialled to prove its key, and
/// returns its ID.
async fn challenge<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    local: &EndpointId,
) -> io::Result<EndpointId> {
    let mut hello = [0; 40];
    hello[..8].copy_from_slice(MAGIC);
    getrandom::fill(&mut hello[8..])
        .map_err(|e| io::Error::other(format!("no random numbers: {e}")))?;
    stream.write_all(&hello).await?;
    stream.flush().await?;
    let mut proof = [0; 96];
    stream.read_exact(&mut proof).await?;
    let id: [u8; 32] = proof[..32].try_into().expect("32 bytes");
    let from = EndpointId::from_bytes(&id).map_err(|_| invalid("not a node ID"))?;
    let signature = Signature::from_bytes(proof[32..].try_into().expect("64 bytes"));
    from.verify(&signed(local, &hello[8..]), &signature)
        .map_err(|_| invalid("the node that dialled didn't prove its key"))?;
    Ok(from)
}

/// The dialling side: proves its key to `to`, the node it dialled.
async fn prove<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    key: &SecretKey,
    to: &EndpointId,
) -> io::Result<()> {
    let mut hello = [0; 40];
    stream.read_exact(&mut hello).await?;
    if &hello[..8] != MAGIC {
        return Err(invalid(
            "the other side isn't an osp node, or runs another version",
        ));
    }
    let signature = key.sign(&signed(to, &hello[8..]));
    let mut proof = [0; 96];
    proof[..32].copy_from_slice(key.public().as_bytes());
    proof[32..].copy_from_slice(&signature.to_bytes());
    stream.write_all(&proof).await?;
    stream.flush().await
}

/// What the dialling node signs: for this accepting node, and this
/// challenge only.
fn signed(accepting: &EndpointId, challenge: &[u8]) -> Vec<u8> {
    [CONTEXT, accepting.as_bytes(), challenge].concat()
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

/// One packet, or `None` at the end of the stream.
async fn read_packet<R: AsyncRead + Unpin>(read: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0; 2];
    match read.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let mut packet = vec![0; u16::from_be_bytes(len) as usize];
    read.read_exact(&mut packet).await?;
    Ok(Some(packet))
}

async fn write_packet<W: AsyncWrite + Unpin>(write: &mut W, packet: &[u8]) -> io::Result<()> {
    let len = u16::try_from(packet.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "packet too big"))?;
    write.write_all(&len.to_be_bytes()).await?;
    write.write_all(packet).await
}

impl CustomTransport for TorTransport {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
        let received = lock(&self.shared.received)
            .take()
            .ok_or_else(|| io::Error::other("this transport already has an endpoint"))?;
        if let Some(listener) = lock(&self.shared.listener).take() {
            self.shared.spawn(self.shared.clone().accept_loop(listener));
        }
        Ok(Box::new(Endpoint {
            shared: self.shared.clone(),
            received,
            addrs: n0_watcher::Watchable::new(vec![addr(self.shared.id())]),
        }))
    }
}

struct TorPreset {
    transport: TorTransport,
}

impl Preset for TorPreset {
    fn apply(self, builder: Builder) -> Builder {
        // `Minimal` sets the crypto provider iroh needs; the rest is ours.
        Minimal
            .apply(builder)
            .clear_ip_transports()
            .clear_relay_transports()
            .clear_address_lookup()
            .add_custom_transport(Arc::new(self.transport))
            .address_lookup(Lookup)
            .transport_config(quic_config().build())
    }
}

/// QUIC's settings for an endpoint over Tor, to add to: iroh's, with
/// [`MAX_IDLE`].
pub fn quic_config() -> QuicTransportConfigBuilder {
    QuicTransportConfig::builder()
        .max_idle_timeout(Some(MAX_IDLE.try_into().expect("a valid idle timeout")))
}

#[derive(Debug, Clone)]
struct Lookup;

impl AddressLookup for Lookup {
    fn publish(&self, _data: &EndpointData) {}

    fn resolve(&self, node: EndpointId) -> Option<BoxStream<Result<Item, Error>>> {
        let found = EndpointAddr::from_parts(node, [TransportAddr::Custom(addr(node))]);
        let item = Item::new(EndpointInfo::from(found), "tor", None);
        Some(Box::pin(n0_future::stream::once(Ok(item))))
    }
}

struct Endpoint {
    shared: Arc<Shared>,
    received: mpsc::Receiver<Received>,
    addrs: n0_watcher::Watchable<Vec<CustomAddr>>,
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("node", &self.shared.id().fmt_short().to_string())
            .finish()
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.shared.close();
    }
}

impl CustomEndpoint for Endpoint {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
        self.addrs.watch()
    }

    fn create_sender(&self) -> Arc<dyn CustomSender> {
        Arc::new(Sender {
            shared: self.shared.clone(),
        })
    }

    fn max_transmit_segments(&self) -> NonZeroUsize {
        NonZeroUsize::new(32).expect("not zero")
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        metas: &mut [noq_udp::RecvMeta],
        recv_infos: &mut [RecvInfo],
    ) -> Poll<io::Result<usize>> {
        let local = addr(self.shared.id());
        let slots = bufs.len().min(metas.len()).min(recv_infos.len());
        let mut filled = 0;
        while filled < slots {
            let (from, packet) = match self.received.poll_recv(cx) {
                Poll::Ready(Some(received)) => received,
                Poll::Ready(None) if filled == 0 => {
                    return Poll::Ready(Err(io::Error::other("the Tor transport closed")));
                }
                Poll::Ready(None) | Poll::Pending => break,
            };
            let len = packet.len();
            // Too big for the buffer: lost, as a datagram would be.
            if len > bufs[filled].len() {
                continue;
            }
            bufs[filled][..len].copy_from_slice(&packet);
            metas[filled].len = len;
            metas[filled].stride = len;
            recv_infos[filled] = RecvInfo::new(addr(from), Some(local.clone()));
            filled += 1;
        }
        if filled == 0 && slots > 0 {
            Poll::Pending
        } else {
            Poll::Ready(Ok(filled))
        }
    }
}

struct Sender {
    shared: Arc<Shared>,
}

impl fmt::Debug for Sender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sender")
            .field("node", &self.shared.id().fmt_short().to_string())
            .finish()
    }
}

impl CustomSender for Sender {
    fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
        addr.id() == TRANSPORT_ID
    }

    fn poll_send(
        &self,
        _cx: &mut Context,
        dst: &CustomAddr,
        _src: Option<&CustomAddr>,
        transmit: &Transmit<'_>,
    ) -> Poll<io::Result<()>> {
        let Some(to) = node_of(dst) else {
            return Poll::Ready(Err(io::Error::other("not an address on the Tor transport")));
        };
        let contents = transmit.contents;
        let size = transmit.segment_size.unwrap_or(contents.len()).max(1);
        self.shared.send(to, contents.chunks(size));
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_proof_holds_only_for_its_key_node_and_challenge() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let key = SecretKey::generate();
        let accepting = SecretKey::generate().public();
        let dialling = tokio::spawn({
            let key = key.clone();
            async move { prove(&mut b, &key, &accepting).await }
        });
        assert_eq!(challenge(&mut a, &accepting).await.unwrap(), key.public());
        dialling.await.unwrap().unwrap();

        // Proved for another node: refused.
        let (mut a, mut b) = tokio::io::duplex(1024);
        let other = SecretKey::generate().public();
        tokio::spawn(async move { prove(&mut b, &key, &other).await });
        let err = challenge(&mut a, &accepting).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn a_proof_for_another_challenge_is_refused() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let key = SecretKey::generate();
        let accepting = SecretKey::generate().public();
        tokio::spawn(async move {
            let mut hello = [0; 40];
            b.read_exact(&mut hello).await.unwrap();
            // Signs other random bytes than those it was sent.
            let signature = key.sign(&signed(&accepting, &[0; 32]));
            b.write_all(key.public().as_bytes()).await.unwrap();
            b.write_all(&signature.to_bytes()).await.unwrap();
        });
        assert!(challenge(&mut a, &accepting).await.is_err());
    }

    #[tokio::test]
    async fn the_dialling_side_refuses_what_isnt_an_osp_node() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&[0; 40]).await.unwrap();
        let key = SecretKey::generate();
        let err = prove(&mut b, &key, &key.public()).await.unwrap_err();
        assert!(err.to_string().contains("isn't an osp node"), "{err}");
    }

    #[tokio::test]
    async fn packets_keep_their_bounds() {
        let (mut a, mut b) = tokio::io::duplex(1 << 20);
        for packet in [&b"one"[..], &[], &[7; 1500], &[9; 65535]] {
            write_packet(&mut a, packet).await.unwrap();
        }
        assert!(write_packet(&mut a, &[0; 65536]).await.is_err());
        drop(a);
        let mut got = Vec::new();
        while let Some(packet) = read_packet(&mut b).await.unwrap() {
            got.push(packet.len());
        }
        assert_eq!(got, [3, 0, 1500, 65535]);
    }

    #[test]
    fn addresses_name_nodes() {
        let node = SecretKey::generate().public();
        assert_eq!(node_of(&addr(node)), Some(node));
        let other = CustomAddr::from_parts(TRANSPORT_ID + 1, node.as_bytes());
        assert_eq!(node_of(&other), None);
        assert_eq!(
            node_of(&CustomAddr::from_parts(TRANSPORT_ID, &[1, 2])),
            None
        );
    }
}
