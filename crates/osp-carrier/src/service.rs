//! The carrier: what every node runs on its carrier endpoint, so circuits can
//! go through it.
//!
//! One task holds every circuit, and each link (a QUIC connection with a
//! neighbour) has a task reading its frames, sent as QUIC datagrams. A lost
//! datagram is resent by the ends' own QUIC when it was theirs; a lost setup
//! fails that build, and the viewer builds another.
//!
//! A carrier dials only the carrier endpoint a signed descriptor names,
//! through a relay its owner's policy allows ([`Dial`]), and delivers to a
//! host only through a host slot it gave that host's own connection. Nothing
//! asks it to reach anything else.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow};
use bytes::Bytes;
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayUrl, SecretKey,
    endpoint::{Connection, RecvStream, SendStream},
};
use n0_future::boxed::BoxFuture;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
    time::Instant,
};
use tracing::{debug, warn};

use crate::{
    CarrierInfo, CarrierKeys, CircuitId, Descriptor, DestroyReason, Direction, Frame, Layer, Relay,
    SLOT_LEN, Slot, VERSION,
};

/// The ALPN of a carrier endpoint's links.
pub const ALPN: &[u8] = b"osp/carrier/1";
/// How long a host slot lasts. The host asks for a new one well before.
pub const SLOT_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// How long a hop waits for the next to answer `CREATE`, the dial included.
pub const SETUP_TIMEOUT: Duration = Duration::from_secs(20);
/// A circuit that carried a packet this recently counts as a job.
const ACTIVE: Duration = Duration::from_secs(10);
/// A link this carrier opened, with no circuit on it this long, is closed.
const LINK_IDLE: Duration = Duration::from_secs(5 * 60);
/// How often limits, slots, setups and keys are looked at.
const TICK: Duration = Duration::from_secs(1);
/// Packets held for the upload share, at most this long's worth of it...
const HOLD_FOR: Duration = Duration::from_millis(250);
/// ...and never less than this many bytes, so a slow share still smooths a
/// burst.
const HOLD_AT_LEAST: usize = 64 * 1024;
/// Circuit IDs from the side that opened a link are below this; from the
/// side that accepted it, at or above.
const ACCEPTOR_IDS: CircuitId = 1 << 31;
/// A slot request on a link's stream, and the answer's sizes.
const SLOT_REQUEST: [u8; 2] = [VERSION, b'S'];
const SLOT_ANSWER: usize = 2 + SLOT_LEN + 4;

/// The largest frame a link carries: a 1200-byte QUIC packet, the smallest
/// QUIC allows, under three hops' layers.
pub const LINK_DATAGRAM: usize = 1200 + crate::overhead(crate::MAX_HOPS);

/// Waits until `conn` carries datagrams of [`LINK_DATAGRAM`] bytes. A new
/// link starts at QUIC's 1200-byte packets, about 1160 bytes of datagram,
/// and path MTU discovery raises it within a few round trips if the path
/// allows: 1414 on loopback within 3 s. A link that never gets there isn't
/// used (the design note's "Packet size"), and nothing relies on IP
/// fragmentation.
pub async fn wait_for_room(conn: &Connection, within: Duration) -> Result<()> {
    let deadline = Instant::now() + within;
    loop {
        if conn.max_datagram_size().is_some_and(|m| m >= LINK_DATAGRAM) {
            return Ok(());
        }
        anyhow::ensure!(
            conn.close_reason().is_none(),
            "the link closed before it carried full-size packets"
        );
        anyhow::ensure!(
            Instant::now() < deadline,
            "a link that carries at most {:?} bytes, under {LINK_DATAGRAM}",
            conn.max_datagram_size()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// What bounds a carrier's work. The sizing stage set the per-neighbour and
/// per-minute values ([carrier-sizing.md](../../docs/testing/carrier-sizing.md)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Circuits open at once.
    pub circuits: usize,
    /// Circuits carrying at once: a new one is refused with `BUSY` while
    /// this many carried a packet in the last 10 s.
    pub jobs: usize,
    /// Circuits open at once from one neighbour.
    pub per_neighbour: usize,
    /// New circuits taken in any minute.
    pub new_per_minute: usize,
    /// Bytes a second carried, all circuits together: the owner's upload
    /// share. Packets beyond it wait their turn (`Pacer`), so the ends'
    /// QUIC sees a slower path rather than losses. Only a full queue drops.
    pub bytes_per_sec: Option<u64>,
    /// A circuit's longest life and most bytes, after which it's ended.
    pub circuit_life: Duration,
    pub circuit_bytes: u64,
    /// Extends waiting for the next carrier at once.
    pub pending: usize,
    /// Host slots one host's connection holds at once.
    pub slots_per_host: usize,
}

impl Limits {
    /// A home node's, from the sizing stage: 2 jobs at once, 4 circuits
    /// from one neighbour, 14 new a minute.
    pub fn home() -> Self {
        Self {
            circuits: 16,
            jobs: 2,
            per_neighbour: 4,
            new_per_minute: 14,
            bytes_per_sec: None,
            circuit_life: Duration::from_secs(60 * 60),
            circuit_bytes: 8 << 30,
            pending: 8,
            slots_per_host: 4,
        }
    }

    /// A seed carrier's: a server with 40 jobs at once, 36 circuits from one
    /// neighbour, 160 new a minute.
    pub fn seed() -> Self {
        Self {
            circuits: 400,
            jobs: 40,
            per_neighbour: 36,
            new_per_minute: 160,
            pending: 64,
            ..Self::home()
        }
    }
}

/// Opens a link to the carrier a checked descriptor names.
pub type Dial = Arc<dyn Fn(CarrierInfo) -> BoxFuture<Result<Connection>> + Send + Sync>;

/// The way a node dials: the carrier endpoint by its ID, through the relay
/// its descriptor names, and only if `allow` takes that relay. With no
/// address of its own published, the hole punch then shows each the other's
/// address, as ADR 0017 says a carrier shows the machines next to it.
pub fn relay_dial(
    endpoint: Endpoint,
    allow: impl Fn(&RelayUrl) -> bool + Send + Sync + 'static,
) -> Dial {
    let allow = Arc::new(allow);
    Arc::new(move |info: CarrierInfo| {
        let endpoint = endpoint.clone();
        let allow = allow.clone();
        Box::pin(async move {
            let relay: RelayUrl = info
                .relay
                .parse()
                .map_err(|e| anyhow!("not a relay URL: {e}"))?;
            anyhow::ensure!(allow(&relay), "a relay this node doesn't use: {relay}");
            let addr = EndpointAddr::new(info.endpoint).with_relay_url(relay);
            Ok(endpoint.connect(addr, ALPN).await?)
        })
    })
}

/// How a carrier is set up.
pub struct Config {
    /// Its machine key, which signs its descriptor.
    pub machine: SecretKey,
    /// The relay its carrier endpoint is reached through, for the descriptor.
    pub relay: String,
    pub offers: Vec<String>,
    pub limits: Limits,
    pub dial: Dial,
    /// Takes connections on the endpoint's other ALPNs, if it has any: the
    /// node's own small requests to its carrier. Without it they're closed.
    pub others: Option<Others>,
}

/// Where a carrier endpoint's connections on other ALPNs go.
pub type Others = Arc<dyn Fn(Connection) + Send + Sync>;

/// What a carrier has done, for status and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    /// Whether it's paused: it takes no new circuits or host slots.
    pub paused: bool,
    pub circuits: usize,
    pub links: usize,
    pub busy: u64,
    pub refused: u64,
    pub dropped: u64,
    pub bytes: u64,
    /// Packets that waited for the upload share before going on: the share,
    /// not the path, set how fast they went.
    pub held: u64,
    /// Packets sent on to a link whose datagram buffer was backed up: the
    /// path, or the owner's connection, couldn't take them as fast.
    pub link_full: u64,
}

/// A running carrier. Dropping it stops it.
pub struct Carrier {
    tx: mpsc::Sender<Event>,
    _tasks: JoinSet<()>,
}

impl Carrier {
    /// Starts carrying on `endpoint`, which must take [`ALPN`] and be
    /// published nowhere.
    pub fn spawn(endpoint: Endpoint, config: Config) -> Result<Self> {
        let (tx, rx) = mpsc::channel(4096);
        let mut tasks = JoinSet::new();
        let others = config.others.clone();
        let state = State::new(&endpoint, config, tx.clone())?;
        tasks.spawn(state.run(rx));
        let accept_tx = tx.clone();
        tasks.spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let tx = accept_tx.clone();
                let others = others.clone();
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(conn) if conn.alpn() == ALPN => {
                            let _ = tx.send(Event::Link(conn, false)).await;
                        }
                        Ok(conn) => match &others {
                            Some(others) => others(conn),
                            None => conn.close(0u32.into(), b"not a carrier link"),
                        },
                        Err(e) => debug!("a carrier link failed to open: {e:#}"),
                    }
                });
            }
        });
        Ok(Self { tx, _tasks: tasks })
    }

    /// Its descriptor now, signed, with the current circuit key.
    pub async fn descriptor(&self) -> Result<Descriptor> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Event::Descriptor(tx))
            .await
            .map_err(|_| anyhow!("the carrier stopped"))?;
        rx.await.map_err(|_| anyhow!("the carrier stopped"))
    }

    /// Names another relay in its descriptors from now on: the carrier
    /// endpoint's home relay moved, say to the fallback.
    pub async fn set_relay(&self, relay: String) -> Result<()> {
        self.tx
            .send(Event::Relay(relay))
            .await
            .map_err(|_| anyhow!("the carrier stopped"))
    }

    /// Takes new limits from now on: the owner changed a control. Circuits
    /// already open stay, unless they're past the new limits' life or bytes.
    pub async fn set_limits(&self, limits: Limits) -> Result<()> {
        self.tx
            .send(Event::Limits(limits))
            .await
            .map_err(|_| anyhow!("the carrier stopped"))
    }

    /// Paused, it ends the circuits it carries and answers new ones `BUSY`,
    /// so their ends go another way: for a metered connection, or for the
    /// owner's troubleshooting.
    pub async fn set_paused(&self, paused: bool) -> Result<()> {
        self.tx
            .send(Event::Paused(paused))
            .await
            .map_err(|_| anyhow!("the carrier stopped"))
    }

    pub async fn stats(&self) -> Result<Stats> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Event::Stats(tx))
            .await
            .map_err(|_| anyhow!("the carrier stopped"))?;
        rx.await.map_err(|_| anyhow!("the carrier stopped"))
    }
}

type LinkId = u64;

enum Event {
    /// A link opened: by this carrier, or by the neighbour.
    Link(Connection, bool),
    Frame(LinkId, Frame),
    LinkClosed(LinkId),
    Dialed(EndpointId, Result<Connection>),
    Slot(LinkId, oneshot::Sender<Option<Slot>>),
    Descriptor(oneshot::Sender<Descriptor>),
    Relay(String),
    Limits(Limits),
    Paused(bool),
    Stats(oneshot::Sender<Stats>),
}

struct Link {
    conn: Connection,
    peer: EndpointId,
    /// Whether this carrier opened it, which sets the circuit IDs it picks.
    opened: bool,
    idle_since: Instant,
    /// The most datagram buffer space it has had free: its size.
    buffer: usize,
}

/// Where a circuit goes next, towards the host.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Next {
    /// Nowhere yet: this is the circuit's last hop.
    End,
    /// `CREATE` sent to the next carrier, or waiting for the dial.
    Extending {
        since: Instant,
    },
    Carrier,
    Host,
}

struct Circuit {
    layer: Layer,
    next: Next,
    /// The link and ID towards the host, once there is one.
    onward: Option<(LinkId, CircuitId)>,
    started: Instant,
    last_active: Option<Instant>,
    bytes: u64,
}

/// An extend waiting for its dial.
struct Waiting {
    from: (LinkId, CircuitId),
    create: crate::Setup,
    backward_start: u64,
}

struct State {
    keys: CarrierKeys,
    machine: SecretKey,
    endpoint_id: EndpointId,
    relay: String,
    offers: Vec<String>,
    limits: Limits,
    dial: Dial,
    tx: mpsc::Sender<Event>,
    links: HashMap<LinkId, Link>,
    /// Links this carrier opened, by the carrier endpoint at the other end.
    opened: HashMap<EndpointId, LinkId>,
    dialling: HashMap<EndpointId, Vec<Waiting>>,
    next_link: LinkId,
    /// Circuits, by the link and ID towards the viewer.
    circuits: HashMap<(LinkId, CircuitId), Circuit>,
    /// From the link and ID towards the host, back to the circuit.
    back: HashMap<(LinkId, CircuitId), (LinkId, CircuitId)>,
    slots: HashMap<Slot, (LinkId, Instant)>,
    created: VecDeque<Instant>,
    pacer: Pacer,
    stats: Stats,
    tasks: JoinSet<()>,
}

/// The upload share, as a token bucket with a queue in front of it: a
/// packet over the share waits for its turn rather than being dropped, up
/// to `HOLD_FOR` of the share. In order: nothing passes a packet waiting.
#[derive(Default)]
struct Pacer {
    bucket: Option<(f64, Instant)>,
    /// Each with the bytes it carried, which is what the share counts.
    held: VecDeque<(LinkId, Bytes, usize)>,
    held_bytes: usize,
}

/// What the pacer did with a packet.
#[derive(Debug, PartialEq, Eq)]
enum Paced {
    Now,
    Held,
    Full,
}

impl Pacer {
    /// Tops the bucket up to `now`, a second of the share at most.
    fn refill(&mut self, rate: f64, now: Instant) -> &mut f64 {
        let (tokens, at) = self.bucket.get_or_insert((rate, now));
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * rate).min(rate);
        *at = now;
        tokens
    }

    /// Whether `frame`, of `bytes` carried, goes now, waits, or doesn't fit.
    fn offer(
        &mut self,
        rate: f64,
        link: LinkId,
        frame: Bytes,
        bytes: usize,
        now: Instant,
    ) -> Paced {
        if self.held.is_empty() {
            let tokens = self.refill(rate, now);
            if *tokens >= bytes as f64 {
                *tokens -= bytes as f64;
                return Paced::Now;
            }
        }
        let room = ((rate * HOLD_FOR.as_secs_f64()) as usize).max(HOLD_AT_LEAST);
        if self.held_bytes + bytes > room {
            return Paced::Full;
        }
        self.held_bytes += bytes;
        self.held.push_back((link, frame, bytes));
        Paced::Held
    }

    /// The packets whose turn has come by `now`, in order, with the bytes
    /// each carried.
    fn due(&mut self, rate: Option<u64>, now: Instant) -> Vec<(LinkId, Bytes, usize)> {
        let Some(rate) = rate else {
            // No share any more: everything held goes.
            self.held_bytes = 0;
            return self.held.drain(..).collect();
        };
        let rate = rate as f64;
        let mut out = Vec::new();
        while let Some(&(_, _, bytes)) = self.held.front() {
            let tokens = self.refill(rate, now);
            if *tokens < bytes as f64 {
                break;
            }
            *tokens -= bytes as f64;
            self.held_bytes -= bytes;
            out.extend(self.held.pop_front());
        }
        out
    }

    /// When the first packet held can go, if any is.
    fn next_turn(&self, rate: Option<u64>) -> Option<Instant> {
        let &(_, _, bytes) = self.held.front()?;
        let Some(rate) = rate else {
            return Some(Instant::now());
        };
        let Some((tokens, at)) = self.bucket else {
            return Some(Instant::now());
        };
        let short = (bytes as f64 - tokens).max(0.0);
        Some(at + Duration::from_secs_f64(short / rate as f64))
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_u32() -> u32 {
    let mut b = [0; 4];
    getrandom::fill(&mut b).expect("random numbers");
    u32::from_be_bytes(b)
}

impl State {
    fn new(endpoint: &Endpoint, config: Config, tx: mpsc::Sender<Event>) -> Result<Self> {
        let keys = CarrierKeys::new(config.machine.public(), unix_now());
        Ok(Self {
            keys,
            endpoint_id: endpoint.id(),
            machine: config.machine,
            relay: config.relay,
            offers: config.offers,
            limits: config.limits,
            dial: config.dial,
            tx,
            links: HashMap::new(),
            opened: HashMap::new(),
            dialling: HashMap::new(),
            next_link: 0,
            circuits: HashMap::new(),
            back: HashMap::new(),
            slots: HashMap::new(),
            created: VecDeque::new(),
            pacer: Pacer::default(),
            stats: Stats::default(),
            tasks: JoinSet::new(),
        })
    }

    fn info(&self) -> CarrierInfo {
        let key = self.keys.current();
        CarrierInfo {
            version: VERSION,
            machine: self.machine.public(),
            endpoint: self.endpoint_id,
            relay: self.relay.clone(),
            circuit_key: key.public(),
            valid_from: key.valid_from(),
            valid_until: key.valid_until(),
            offers: self.offers.clone(),
        }
    }

    async fn run(mut self, mut rx: mpsc::Receiver<Event>) {
        let mut tick = tokio::time::interval(TICK);
        loop {
            let turn = self.pacer.next_turn(self.limits.bytes_per_sec);
            tokio::select! {
                ev = rx.recv() => match ev {
                    Some(ev) => self.handle(ev),
                    None => return,
                },
                _ = tick.tick() => self.tick(Instant::now()),
                _ = tokio::time::sleep_until(turn.unwrap_or_else(Instant::now)), if turn.is_some() => {
                    self.release(Instant::now());
                }
            }
        }
    }

    fn handle(&mut self, ev: Event) {
        match ev {
            Event::Link(conn, opened) => {
                self.add_link(conn, opened);
            }
            Event::Frame(link, frame) => self.frame(link, frame),
            Event::LinkClosed(link) => self.link_closed(link),
            Event::Dialed(peer, result) => self.dialed(peer, result),
            Event::Slot(link, reply) => {
                let _ = reply.send(self.new_slot(link));
            }
            Event::Descriptor(reply) => {
                if let Ok(d) = Descriptor::new(&self.machine, &self.info()) {
                    let _ = reply.send(d);
                }
            }
            Event::Relay(relay) => self.relay = relay,
            Event::Limits(limits) => {
                if limits.bytes_per_sec != self.limits.bytes_per_sec {
                    self.pacer.bucket = None;
                }
                self.limits = limits;
                self.release(Instant::now());
            }
            Event::Paused(paused) => {
                self.stats.paused = paused;
                if paused {
                    let all: Vec<_> = self.circuits.keys().copied().collect();
                    for key in all {
                        self.end(key, DestroyReason::Closed);
                    }
                    self.slots.clear();
                }
            }
            Event::Stats(reply) => {
                let mut s = self.stats.clone();
                s.circuits = self.circuits.len();
                s.links = self.links.len();
                let _ = reply.send(s);
            }
        }
    }

    fn add_link(&mut self, conn: Connection, opened: bool) -> LinkId {
        let id = self.next_link;
        self.next_link += 1;
        let peer = conn.remote_id();
        if opened {
            self.opened.insert(peer, id);
        }
        self.links.insert(
            id,
            Link {
                conn: conn.clone(),
                peer,
                opened,
                idle_since: Instant::now(),
                buffer: 0,
            },
        );
        let tx = self.tx.clone();
        let datagrams = conn.clone();
        self.tasks.spawn(async move {
            loop {
                match datagrams.read_datagram().await {
                    Ok(bytes) => match Frame::decode(&bytes) {
                        Ok(frame) => {
                            if tx.send(Event::Frame(id, frame)).await.is_err() {
                                return;
                            }
                        }
                        Err(e) => debug!("a malformed carrier frame: {e:#}"),
                    },
                    Err(_) => {
                        let _ = tx.send(Event::LinkClosed(id)).await;
                        return;
                    }
                }
            }
        });
        if !opened {
            // The side that accepted a link answers slot requests on it.
            let tx = self.tx.clone();
            self.tasks.spawn(async move {
                while let Ok((send, recv)) = conn.accept_bi().await {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = answer_slot(id, send, recv, tx).await {
                            debug!("a slot request failed: {e:#}");
                        }
                    });
                }
            });
        }
        id
    }

    fn send(&mut self, link: LinkId, frame: &Frame) {
        if !self.links.contains_key(&link) {
            return;
        }
        self.send_encoded(link, Bytes::from(frame.encode()));
    }

    fn send_encoded(&mut self, link: LinkId, frame: Bytes) {
        let Some(l) = self.links.get_mut(&link) else {
            return;
        };
        // Its datagram buffer more than half used: the link isn't keeping
        // up with what's sent on it.
        let space = l.conn.datagram_send_buffer_space();
        l.buffer = l.buffer.max(space);
        if space < l.buffer / 2 {
            self.stats.link_full += 1;
        }
        if let Err(e) = l.conn.send_datagram(frame) {
            self.stats.dropped += 1;
            debug!("couldn't send a carrier frame: {e}");
        }
    }

    /// Sends `frame`, carrying `bytes` for a circuit, within the owner's
    /// upload share: now if there's room in it, else when its turn comes.
    /// Dropped only if too much is waiting already.
    fn carry(&mut self, link: LinkId, frame: &Frame, bytes: usize, now: Instant) {
        let Some(rate) = self.limits.bytes_per_sec else {
            self.stats.bytes += bytes as u64;
            return self.send(link, frame);
        };
        let encoded = Bytes::from(frame.encode());
        match self
            .pacer
            .offer(rate as f64, link, encoded.clone(), bytes, now)
        {
            Paced::Now => {
                self.stats.bytes += bytes as u64;
                self.send_encoded(link, encoded);
            }
            Paced::Held => self.stats.held += 1,
            Paced::Full => self.stats.dropped += 1,
        }
    }

    /// Sends what the pacer held whose turn has come.
    fn release(&mut self, now: Instant) {
        for (link, frame, bytes) in self.pacer.due(self.limits.bytes_per_sec, now) {
            self.stats.bytes += bytes as u64;
            self.send_encoded(link, frame);
        }
    }

    fn pick_id(&self, link: LinkId) -> CircuitId {
        let opened = self.links.get(&link).is_some_and(|l| l.opened);
        loop {
            let r = random_u32() % ACCEPTOR_IDS;
            let id = if opened { r } else { r + ACCEPTOR_IDS };
            if !self.circuits.contains_key(&(link, id)) && !self.back.contains_key(&(link, id)) {
                return id;
            }
        }
    }

    fn frame(&mut self, link: LinkId, frame: Frame) {
        let now = Instant::now();
        match frame {
            Frame::Create {
                circuit,
                setup,
                backward_start,
            } => self.create(link, circuit, setup, backward_start, now),
            Frame::Data {
                circuit,
                direction: Direction::ToHost,
                pn,
                payload,
            } => self.towards_host(link, circuit, pn, payload, now),
            Frame::Data {
                circuit,
                direction: Direction::ToViewer,
                pn,
                payload,
            } => self.towards_viewer(link, circuit, pn, payload, now),
            Frame::Created { circuit } => self.answered(link, circuit, true),
            Frame::Busy { circuit } => self.answered(link, circuit, false),
            Frame::Destroy { circuit, .. } => self.destroyed(link, circuit),
        }
    }

    fn create(
        &mut self,
        link: LinkId,
        circuit: CircuitId,
        setup: crate::Setup,
        backward_start: u64,
        now: Instant,
    ) {
        let Some(l) = self.links.get(&link) else {
            return;
        };
        // Only the side that opened a link starts circuits on it, with IDs
        // from its half.
        if l.opened || circuit >= ACCEPTOR_IDS || self.circuits.contains_key(&(link, circuit)) {
            self.stats.refused += 1;
            self.send(
                link,
                &Frame::Destroy {
                    circuit,
                    reason: DestroyReason::Refused,
                },
            );
            return;
        }
        let peer = l.peer;
        let minute = now.checked_sub(Duration::from_secs(60));
        while self
            .created
            .front()
            .is_some_and(|t| minute.is_some_and(|m| *t < m))
        {
            self.created.pop_front();
        }
        let from_peer = self
            .circuits
            .keys()
            .filter(|(l, _)| self.links.get(l).is_some_and(|l| l.peer == peer))
            .count();
        let jobs = self
            .circuits
            .values()
            .filter(|c| {
                c.last_active
                    .is_some_and(|t| now.duration_since(t) < ACTIVE)
            })
            .count();
        if self.stats.paused
            || self.circuits.len() >= self.limits.circuits
            || from_peer >= self.limits.per_neighbour
            || self.created.len() >= self.limits.new_per_minute
            || jobs >= self.limits.jobs
        {
            self.stats.busy += 1;
            self.send(link, &Frame::Busy { circuit });
            return;
        }
        let mut layer = match self.keys.accept(&setup, unix_now()) {
            Ok(layer) => layer,
            Err(e) => {
                debug!("a setup refused: {e:#}");
                self.stats.refused += 1;
                self.send(
                    link,
                    &Frame::Destroy {
                        circuit,
                        reason: DestroyReason::Refused,
                    },
                );
                return;
            }
        };
        layer.start_own_at(backward_start);
        self.created.push_back(now);
        self.circuits.insert(
            (link, circuit),
            Circuit {
                layer,
                next: Next::End,
                onward: None,
                started: now,
                last_active: None,
                bytes: 0,
            },
        );
        self.send(link, &Frame::Created { circuit });
    }

    /// Sends a command of this hop's own back towards the viewer.
    fn originate(&mut self, key: (LinkId, CircuitId), relay: &Relay) {
        let Some(c) = self.circuits.get_mut(&key) else {
            return;
        };
        match c.layer.originate(relay) {
            Ok((pn, payload)) => self.send(
                key.0,
                &Frame::Data {
                    circuit: key.1,
                    direction: Direction::ToViewer,
                    pn,
                    payload,
                },
            ),
            Err(e) => {
                debug!("couldn't answer on a circuit: {e:#}");
                self.end(key, DestroyReason::Protocol);
            }
        }
    }

    fn towards_host(
        &mut self,
        link: LinkId,
        circuit: CircuitId,
        pn: u64,
        mut payload: Vec<u8>,
        now: Instant,
    ) {
        let key = (link, circuit);
        let Some(c) = self.circuits.get_mut(&key) else {
            self.stats.dropped += 1;
            return;
        };
        if c.layer.open(Direction::ToHost, pn, &mut payload).is_err() {
            self.stats.dropped += 1;
            return;
        }
        match (c.next, c.onward) {
            (Next::Carrier, Some((on, id))) => {
                c.bytes += payload.len() as u64;
                c.last_active = Some(now);
                let bytes = payload.len();
                self.carry(
                    on,
                    &Frame::Data {
                        circuit: id,
                        direction: Direction::ToHost,
                        pn,
                        payload,
                    },
                    bytes,
                    now,
                );
            }
            (Next::Host, Some((on, id))) => match Relay::decode(payload) {
                Ok(Relay::Data(packet)) => {
                    c.bytes += packet.len() as u64;
                    c.last_active = Some(now);
                    let bytes = packet.len();
                    self.carry(
                        on,
                        &Frame::Data {
                            circuit: id,
                            direction: Direction::ToHost,
                            pn: 0,
                            payload: packet,
                        },
                        bytes,
                        now,
                    );
                }
                _ => self.stats.dropped += 1,
            },
            (Next::End, _) => match Relay::decode(payload) {
                Ok(Relay::Extend { setup, descriptor }) => self.extend(key, setup, descriptor),
                Ok(Relay::ConnectHost { slot }) => self.connect_host(key, slot, now),
                _ => self.stats.dropped += 1,
            },
            _ => self.stats.dropped += 1,
        }
    }

    fn extend(&mut self, key: (LinkId, CircuitId), setup: crate::Setup, descriptor: Descriptor) {
        let pending = self
            .circuits
            .values()
            .filter(|c| matches!(c.next, Next::Extending { .. }))
            .count();
        if pending >= self.limits.pending {
            self.stats.busy += 1;
            return self.originate(key, &Relay::Busy);
        }
        let info = match descriptor.verify(unix_now()) {
            Ok(info)
                if info.machine != self.machine.public() && info.endpoint != self.endpoint_id =>
            {
                info
            }
            Ok(_) => {
                self.stats.refused += 1;
                return self.end(key, DestroyReason::Refused);
            }
            Err(e) => {
                debug!("an extend to a bad descriptor: {e:#}");
                self.stats.refused += 1;
                return self.end(key, DestroyReason::Refused);
            }
        };
        let Some(c) = self.circuits.get_mut(&key) else {
            return;
        };
        c.next = Next::Extending {
            since: Instant::now(),
        };
        // The next hop numbers its packets towards the viewer after the one
        // this hop will still send: EXTENDED, or BUSY.
        let backward_start = c.layer.next_own() + 1;
        let waiting = Waiting {
            from: key,
            create: setup,
            backward_start,
        };
        if let Some(&link) = self.opened.get(&info.endpoint) {
            return self.send_create(link, waiting);
        }
        let first = !self.dialling.contains_key(&info.endpoint);
        self.dialling
            .entry(info.endpoint)
            .or_default()
            .push(waiting);
        if first {
            let dial = self.dial.clone();
            let tx = self.tx.clone();
            let peer = info.endpoint;
            self.tasks.spawn(async move {
                let result = tokio::time::timeout(SETUP_TIMEOUT, async move {
                    let conn = dial(info).await?;
                    wait_for_room(&conn, SETUP_TIMEOUT).await?;
                    Ok(conn)
                })
                .await
                .unwrap_or_else(|_| Err(anyhow!("timed out")));
                let _ = tx.send(Event::Dialed(peer, result)).await;
            });
        }
    }

    fn send_create(&mut self, link: LinkId, w: Waiting) {
        if !self.circuits.contains_key(&w.from) {
            return;
        }
        let id = self.pick_id(link);
        self.back.insert((link, id), w.from);
        if let Some(c) = self.circuits.get_mut(&w.from) {
            c.onward = Some((link, id));
        }
        self.send(
            link,
            &Frame::Create {
                circuit: id,
                setup: w.create,
                backward_start: w.backward_start,
            },
        );
    }

    fn dialed(&mut self, peer: EndpointId, result: Result<Connection>) {
        let waiting = self.dialling.remove(&peer).unwrap_or_default();
        match result {
            Ok(conn) if conn.remote_id() == peer => {
                let link = self.add_link(conn, true);
                for w in waiting {
                    self.send_create(link, w);
                }
            }
            other => {
                if let Err(e) = other {
                    debug!("couldn't reach the next carrier: {e:#}");
                }
                for w in waiting {
                    self.extend_failed(w.from);
                }
            }
        }
    }

    /// The next carrier didn't take the extend: the circuit stays at this
    /// hop, and the viewer may try another.
    fn extend_failed(&mut self, key: (LinkId, CircuitId)) {
        let Some(c) = self.circuits.get_mut(&key) else {
            return;
        };
        c.next = Next::End;
        if let Some(onward) = c.onward.take() {
            self.back.remove(&onward);
        }
        self.originate(key, &Relay::Busy);
    }

    /// `CREATED` or `BUSY` from the next carrier.
    fn answered(&mut self, link: LinkId, circuit: CircuitId, created: bool) {
        let Some(&key) = self.back.get(&(link, circuit)) else {
            return;
        };
        let Some(c) = self.circuits.get_mut(&key) else {
            return;
        };
        if !matches!(c.next, Next::Extending { .. }) {
            return;
        }
        if created {
            c.next = Next::Carrier;
            self.originate(key, &Relay::Extended);
        } else {
            self.back.remove(&(link, circuit));
            self.extend_failed(key);
        }
    }

    fn new_slot(&mut self, link: LinkId) -> Option<Slot> {
        if self.stats.paused {
            return None;
        }
        self.links.get(&link).filter(|l| !l.opened)?;
        let mut held: Vec<_> = self
            .slots
            .iter()
            .filter(|(_, (l, _))| *l == link)
            .map(|(s, (_, t))| (*t, *s))
            .collect();
        held.sort();
        while held.len() >= self.limits.slots_per_host {
            let (_, oldest) = held.remove(0);
            self.slots.remove(&oldest);
        }
        let mut slot = [0; SLOT_LEN];
        getrandom::fill(&mut slot).ok()?;
        self.slots
            .insert(slot, (link, Instant::now() + SLOT_LIFETIME));
        Some(slot)
    }

    fn connect_host(&mut self, key: (LinkId, CircuitId), slot: Slot, now: Instant) {
        let host = match self.slots.get(&slot) {
            Some((link, expires)) if *expires > now && self.links.contains_key(link) => *link,
            _ => {
                self.stats.refused += 1;
                return self.end(key, DestroyReason::Refused);
            }
        };
        let id = self.pick_id(host);
        let Some(c) = self.circuits.get_mut(&key) else {
            return;
        };
        c.next = Next::Host;
        c.onward = Some((host, id));
        self.back.insert((host, id), key);
        // Tells the host a circuit came, before any packet on it.
        self.send(host, &Frame::Created { circuit: id });
        self.originate(key, &Relay::Connected);
    }

    fn towards_viewer(
        &mut self,
        link: LinkId,
        circuit: CircuitId,
        pn: u64,
        mut payload: Vec<u8>,
        now: Instant,
    ) {
        let Some(&key) = self.back.get(&(link, circuit)) else {
            self.stats.dropped += 1;
            return;
        };
        let Some(c) = self.circuits.get_mut(&key) else {
            return;
        };
        let len = payload.len();
        let sealed = match c.next {
            Next::Carrier => c
                .layer
                .seal(Direction::ToViewer, pn, &mut payload)
                .map(|()| (pn, payload)),
            Next::Host => c.layer.originate(&Relay::Data(payload)),
            _ => {
                self.stats.dropped += 1;
                return;
            }
        };
        c.bytes += len as u64;
        c.last_active = Some(now);
        match sealed {
            Ok((pn, payload)) => self.carry(
                key.0,
                &Frame::Data {
                    circuit: key.1,
                    direction: Direction::ToViewer,
                    pn,
                    payload,
                },
                len,
                now,
            ),
            _ => self.stats.dropped += 1,
        }
    }

    /// `DESTROY` from either side ends the circuit, and goes on to the other.
    fn destroyed(&mut self, link: LinkId, circuit: CircuitId) {
        if self.circuits.contains_key(&(link, circuit)) {
            self.end((link, circuit), DestroyReason::Gone);
        } else if let Some(&key) = self.back.get(&(link, circuit)) {
            self.back.remove(&(link, circuit));
            if let Some(c) = self.circuits.get_mut(&key) {
                c.onward = None;
            }
            self.end(key, DestroyReason::Gone);
        }
    }

    /// Ends a circuit, telling both sides.
    fn end(&mut self, key: (LinkId, CircuitId), reason: DestroyReason) {
        let Some(c) = self.circuits.remove(&key) else {
            return;
        };
        if let Some(onward) = c.onward {
            self.back.remove(&onward);
            self.send(
                onward.0,
                &Frame::Destroy {
                    circuit: onward.1,
                    reason,
                },
            );
            if let Some(l) = self.links.get_mut(&onward.0) {
                l.idle_since = Instant::now();
            }
        }
        self.send(
            key.0,
            &Frame::Destroy {
                circuit: key.1,
                reason,
            },
        );
    }

    fn link_closed(&mut self, link: LinkId) {
        let Some(l) = self.links.remove(&link) else {
            return;
        };
        if l.opened && self.opened.get(&l.peer) == Some(&link) {
            self.opened.remove(&l.peer);
        }
        self.slots.retain(|_, (s, _)| *s != link);
        let from: Vec<_> = self
            .circuits
            .keys()
            .filter(|(l, _)| *l == link)
            .copied()
            .collect();
        for key in from {
            self.end(key, DestroyReason::Gone);
        }
        let onward: Vec<_> = self
            .back
            .iter()
            .filter(|((l, _), _)| *l == link)
            .map(|(k, v)| (*k, *v))
            .collect();
        for (k, key) in onward {
            self.back.remove(&k);
            if let Some(c) = self.circuits.get_mut(&key) {
                c.onward = None;
            }
            self.end(key, DestroyReason::Gone);
        }
    }

    fn tick(&mut self, now: Instant) {
        if self.keys.rotate_if_due(unix_now()) {
            debug!("circuit key replaced: the descriptor changes");
        }
        self.slots.retain(|_, (_, expires)| *expires > now);
        let ending: Vec<_> = self
            .circuits
            .iter()
            .filter_map(|(k, c)| {
                let stuck = matches!(c.next, Next::Extending { since } if now.duration_since(since) > SETUP_TIMEOUT);
                let old = now.duration_since(c.started) > self.limits.circuit_life
                    || c.bytes > self.limits.circuit_bytes;
                (stuck || old).then_some((*k, stuck))
            })
            .collect();
        for (key, stuck) in ending {
            if stuck {
                self.end(key, DestroyReason::Timeout);
            } else {
                self.end(key, DestroyReason::Closed);
            }
        }
        let in_use: std::collections::HashSet<LinkId> = self
            .circuits
            .keys()
            .map(|(l, _)| *l)
            .chain(self.back.keys().map(|(l, _)| *l))
            .collect();
        let idle: Vec<_> = self
            .links
            .iter_mut()
            .filter_map(|(id, l)| {
                if in_use.contains(id) {
                    l.idle_since = now;
                    None
                } else {
                    (l.opened && now.duration_since(l.idle_since) > LINK_IDLE).then_some(*id)
                }
            })
            .collect();
        for id in idle {
            if let Some(l) = self.links.get(&id) {
                l.conn.close(0u32.into(), b"idle");
            }
            self.link_closed(id);
        }
    }
}

async fn answer_slot(
    link: LinkId,
    mut send: SendStream,
    mut recv: RecvStream,
    tx: mpsc::Sender<Event>,
) -> Result<()> {
    let mut req = [0; 2];
    tokio::time::timeout(Duration::from_secs(10), recv.read_exact(&mut req)).await??;
    anyhow::ensure!(req == SLOT_REQUEST, "not a slot request");
    let (reply, rx) = oneshot::channel();
    tx.send(Event::Slot(link, reply))
        .await
        .map_err(|_| anyhow!("the carrier stopped"))?;
    let mut out = [0; SLOT_ANSWER];
    out[0] = VERSION;
    if let Some(slot) = rx.await? {
        out[1] = 1;
        out[2..2 + SLOT_LEN].copy_from_slice(&slot);
        out[2 + SLOT_LEN..].copy_from_slice(&(SLOT_LIFETIME.as_secs() as u32).to_be_bytes());
    }
    send.write_all(&out).await?;
    send.finish()?;
    Ok(())
}

/// Asks the carrier at the other end of `conn`, the host's hop, for a host
/// slot: the slot and how long it lasts.
pub async fn request_slot(conn: &Connection) -> Result<(Slot, Duration)> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&SLOT_REQUEST).await?;
    send.finish()?;
    let mut out = [0; SLOT_ANSWER];
    recv.read_exact(&mut out).await?;
    anyhow::ensure!(out[0] == VERSION, "a slot answer of another version");
    anyhow::ensure!(out[1] == 1, "the carrier gave no slot");
    let slot: Slot = out[2..2 + SLOT_LEN].try_into().expect("16 bytes");
    let secs = u32::from_be_bytes(out[2 + SLOT_LEN..].try_into().expect("4 bytes"));
    if secs == 0 {
        warn!("a slot that lasts no time");
    }
    Ok((slot, Duration::from_secs(secs.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 100_000.0;

    fn frame(n: u8) -> Bytes {
        Bytes::from(vec![n; 4])
    }

    #[test]
    fn within_the_share_a_packet_goes_at_once() {
        let mut p = Pacer::default();
        let now = Instant::now();
        assert_eq!(p.offer(RATE, 1, frame(1), 50_000, now), Paced::Now);
        assert_eq!(p.offer(RATE, 1, frame(2), 50_000, now), Paced::Now);
        assert!(p.next_turn(Some(RATE as u64)).is_none());
    }

    #[test]
    fn over_the_share_a_packet_waits_its_turn_in_order() {
        let mut p = Pacer::default();
        let now = Instant::now();
        // The bucket starts with a second of the share.
        assert_eq!(p.offer(RATE, 1, frame(1), 100_000, now), Paced::Now);
        assert_eq!(p.offer(RATE, 1, frame(2), 10_000, now), Paced::Held);
        assert_eq!(p.offer(RATE, 2, frame(3), 10_000, now), Paced::Held);
        // A small packet doesn't pass one waiting, though it would fit sooner.
        assert_eq!(p.offer(RATE, 1, frame(4), 1, now), Paced::Held);
        let rate = Some(RATE as u64);
        let turn = p.next_turn(rate).unwrap();
        assert_eq!(turn, now + Duration::from_millis(100));
        assert!(p.due(rate, now + Duration::from_millis(50)).is_empty());
        let went: Vec<u8> = p
            .due(rate, turn)
            .into_iter()
            .map(|(_, f, _)| f[0])
            .collect();
        assert_eq!(went, vec![2]);
        let went: Vec<(LinkId, u8)> = p
            .due(rate, now + Duration::from_millis(300))
            .into_iter()
            .map(|(l, f, _)| (l, f[0]))
            .collect();
        assert_eq!(went, vec![(2, 3), (1, 4)]);
        assert_eq!(p.held_bytes, 0);
        assert!(p.next_turn(rate).is_none());
    }

    #[test]
    fn only_a_full_queue_drops() {
        let mut p = Pacer::default();
        let now = Instant::now();
        assert_eq!(p.offer(RATE, 1, frame(1), 100_000, now), Paced::Now);
        // A quarter second of 100 kB/s is under the 64 KiB floor.
        let mut held = 0;
        while p.offer(RATE, 1, frame(2), 1_000, now) == Paced::Held {
            held += 1;
        }
        assert_eq!(held, HOLD_AT_LEAST / 1_000);
        assert_eq!(p.offer(RATE, 1, frame(3), 1_000, now), Paced::Full);
        // A fast share holds a quarter second of itself.
        let fast = 10_000_000.0;
        let mut p = Pacer::default();
        assert_eq!(p.offer(fast, 1, frame(1), 10_000_000, now), Paced::Now);
        let mut held = 0;
        while p.offer(fast, 1, frame(2), 1_000, now) == Paced::Held {
            held += 1;
        }
        assert_eq!(held, 2_500);
    }

    #[test]
    fn with_the_share_lifted_everything_held_goes() {
        let mut p = Pacer::default();
        let now = Instant::now();
        assert_eq!(p.offer(RATE, 1, frame(1), 100_000, now), Paced::Now);
        assert_eq!(p.offer(RATE, 1, frame(2), 10_000, now), Paced::Held);
        assert_eq!(p.offer(RATE, 1, frame(3), 10_000, now), Paced::Held);
        assert_eq!(p.due(None, now).len(), 2);
        assert_eq!(p.held_bytes, 0);
    }
}
