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
#[derive(Debug, Clone)]
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
    /// share. Packets beyond it are dropped, and the ends' QUIC slows down.
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
}

/// What a carrier has done, for status and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub circuits: usize,
    pub links: usize,
    pub busy: u64,
    pub refused: u64,
    pub dropped: u64,
    pub bytes: u64,
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
        let state = State::new(&endpoint, config, tx.clone())?;
        tasks.spawn(state.run(rx));
        let accept_tx = tx.clone();
        tasks.spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let tx = accept_tx.clone();
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(conn) if conn.alpn() == ALPN => {
                            let _ = tx.send(Event::Link(conn, false)).await;
                        }
                        Ok(conn) => conn.close(0u32.into(), b"not a carrier link"),
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
    Stats(oneshot::Sender<Stats>),
}

struct Link {
    conn: Connection,
    peer: EndpointId,
    /// Whether this carrier opened it, which sets the circuit IDs it picks.
    opened: bool,
    idle_since: Instant,
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
    bucket: Option<(f64, Instant)>,
    stats: Stats,
    tasks: JoinSet<()>,
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
            bucket: None,
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
            tokio::select! {
                ev = rx.recv() => match ev {
                    Some(ev) => self.handle(ev),
                    None => return,
                },
                _ = tick.tick() => self.tick(Instant::now()),
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
        let Some(l) = self.links.get(&link) else {
            return;
        };
        if let Err(e) = l.conn.send_datagram(Bytes::from(frame.encode())) {
            self.stats.dropped += 1;
            debug!("couldn't send a carrier frame: {e}");
        }
    }

    /// Takes `bytes` from the owner's upload share: false if there's none.
    fn spend(&mut self, bytes: usize, now: Instant) -> bool {
        let Some(rate) = self.limits.bytes_per_sec else {
            return true;
        };
        let rate = rate as f64;
        let (tokens, at) = self.bucket.get_or_insert((rate, now));
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * rate).min(rate);
        *at = now;
        if *tokens < bytes as f64 {
            return false;
        }
        *tokens -= bytes as f64;
        true
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
        if self.circuits.len() >= self.limits.circuits
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
                if self.spend(payload.len(), now) {
                    self.stats.bytes += payload.len() as u64;
                    self.send(
                        on,
                        &Frame::Data {
                            circuit: id,
                            direction: Direction::ToHost,
                            pn,
                            payload,
                        },
                    );
                } else {
                    self.stats.dropped += 1;
                }
            }
            (Next::Host, Some((on, id))) => match Relay::decode(payload) {
                Ok(Relay::Data(packet)) => {
                    c.bytes += packet.len() as u64;
                    c.last_active = Some(now);
                    if self.spend(packet.len(), now) {
                        self.stats.bytes += packet.len() as u64;
                        self.send(
                            on,
                            &Frame::Data {
                                circuit: id,
                                direction: Direction::ToHost,
                                pn: 0,
                                payload: packet,
                            },
                        );
                    } else {
                        self.stats.dropped += 1;
                    }
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
            Ok((pn, payload)) if self.spend(len, now) => {
                self.stats.bytes += len as u64;
                self.send(
                    key.0,
                    &Frame::Data {
                        circuit: key.1,
                        direction: Direction::ToViewer,
                        pn,
                        payload,
                    },
                );
            }
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
