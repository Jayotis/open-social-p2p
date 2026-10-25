//! A simulated network for the harness, `osp-sim`, and tests: nodes in one
//! process send their packets through it instead of over loopback. Each
//! node's connection to it is like a home connection: a speed each way, a
//! delay, a chance of losing each packet, and a router queue that fills up.
//! A connection can be cut off and restored. Real QUIC runs over it, so
//! congestion control, retransmission and timeouts behave as they would on
//! such a connection.
//!
//! It plugs into iroh as a custom transport. That API is behind iroh's
//! `unstable-custom-transports` feature, outside its semver guarantees, so
//! an iroh update may need changes here. Only the `sim` feature turns it on;
//! a real node is never built with it.

use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap, VecDeque},
    io,
    sync::{
        Arc, Mutex, MutexGuard, RwLock,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    task::{Context, Poll},
    time::Duration,
};

use iroh::{
    EndpointAddr, EndpointId, TransportAddr,
    address_lookup::{AddressLookup, EndpointData, EndpointInfo, Error, Item},
    endpoint::transports::{CustomEndpoint, CustomSender, CustomTransport, RecvInfo, Transmit},
};
use iroh_base::CustomAddr;
use n0_future::boxed::BoxStream;
use serde::Serialize;
use tokio::{
    sync::{Notify, mpsc},
    task::JoinHandle,
    time::Instant,
};

/// Marks addresses on the simulated network among custom transports'.
pub const TRANSPORT_ID: u64 = 0x6f73_7073_696d; // "ospsim"
/// Longest a packet waits in a router queue. A packet that would wait
/// longer is dropped, as a home router's full queue drops it.
const QUEUE: Duration = Duration::from_millis(100);
/// Packets delivered to a node but not read yet. More are dropped, as a
/// full socket buffer drops them.
const RECEIVE_BUFFER: usize = 4096;

/// A node's connection to the simulated network.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Link {
    /// Upload and download speeds, in bits per second. 0 is no limit.
    pub up_bps: u64,
    pub down_bps: u64,
    /// How long a packet takes between the node and the middle of the
    /// network, each way. Between two nodes it takes both of theirs, so
    /// their round trip is twice the sum.
    pub delay: Duration,
    /// The chance, from 0 to 1, that a packet is lost on its way out, and
    /// the same on its way in.
    pub loss: f64,
}

/// One network that nodes join. Cloning it gives another handle to the
/// same network.
#[derive(Debug, Clone)]
pub struct SimNet {
    net: Arc<Net>,
}

/// A node's place on a network: what `SimSettings::network` takes.
#[derive(Debug, Clone)]
pub struct SimLink {
    pub net: SimNet,
    pub link: Link,
}

/// What the network did with the packets sent into it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct NetStats {
    pub packets: u64,
    pub bytes: u64,
    pub delivered: u64,
    /// Lost by chance, on the way out or in (`Link::loss`).
    pub lost: u64,
    /// Dropped by a full router queue or receive buffer.
    pub queue_full: u64,
    /// Dropped because the sender or receiver was cut off, or the receiver
    /// wasn't on the network.
    pub cut_off: u64,
}

#[derive(Debug)]
struct Net {
    ports: RwLock<HashMap<EndpointId, Arc<Port>>>,
    seed: u64,
    /// Orders packets that reach a node at the same moment.
    next_seq: AtomicU64,
    stats: Counters,
}

#[derive(Debug, Default)]
struct Counters {
    packets: AtomicU64,
    bytes: AtomicU64,
    delivered: AtomicU64,
    lost: AtomicU64,
    queue_full: AtomicU64,
    cut_off: AtomicU64,
}

/// A node on the network.
#[derive(Debug)]
struct Port {
    id: EndpointId,
    state: Mutex<PortState>,
    /// Packets on their way in, earliest first, by when they reach this
    /// node's connection.
    arriving: Mutex<BinaryHeap<Arriving>>,
    /// Told when a packet arrives that is due before all the others.
    wake: Notify,
}

#[derive(Debug)]
struct PortState {
    link: Link,
    online: bool,
    up: Queue,
    down: Queue,
    rng: Rng,
}

#[derive(Debug)]
struct Packet {
    from: EndpointId,
    data: Box<[u8]>,
}

#[derive(Debug)]
struct Arriving {
    at: Instant,
    seq: u64,
    packet: Packet,
}

// The earliest first in a `BinaryHeap`, which pops the greatest.
impl Ord for Arriving {
    fn cmp(&self, other: &Self) -> Ordering {
        (other.at, other.seq).cmp(&(self.at, self.seq))
    }
}

impl PartialOrd for Arriving {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Arriving {
    fn eq(&self, other: &Self) -> bool {
        (self.at, self.seq) == (other.at, other.seq)
    }
}

impl Eq for Arriving {}

/// One direction of a connection: packets leave one after another, at its
/// speed, after those ahead of them.
#[derive(Debug, Default)]
struct Queue {
    /// When the last packet let in will have left.
    free_at: Option<Instant>,
}

impl Queue {
    /// When a packet of `bytes` that reaches the queue at `now` will have
    /// left it, at `bps`; `None` if the queue is full and drops it.
    fn admit(&mut self, now: Instant, bytes: usize, bps: u64) -> Option<Instant> {
        if bps == 0 {
            return Some(now);
        }
        let start = self.free_at.filter(|t| *t > now).unwrap_or(now);
        if start - now > QUEUE {
            return None;
        }
        let sending = Duration::from_nanos(bytes as u64 * 8 * 1_000_000_000 / bps);
        let left = start + sending;
        self.free_at = Some(left);
        Some(left)
    }
}

/// A small random number generator (SplitMix64), so each node's losses
/// follow from the network's seed.
#[derive(Debug)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// True with the chance `p`.
    fn chance(&mut self, p: f64) -> bool {
        // 53 random bits make a number from 0 up to 1.
        p > 0.0 && ((self.next() >> 11) as f64 / (1u64 << 53) as f64) < p
    }
}

impl SimNet {
    /// A new, empty network. Each node's chances of losing packets follow
    /// from `seed` and the node's ID.
    pub fn new(seed: u64) -> Self {
        Self {
            net: Arc::new(Net {
                ports: RwLock::default(),
                seed,
                next_seq: AtomicU64::new(0),
                stats: Counters::default(),
            }),
        }
    }

    /// A place on this network with `link`, for one node.
    pub fn link(&self, link: Link) -> SimLink {
        SimLink {
            net: self.clone(),
            link,
        }
    }

    /// Cuts a node off, or connects it again. While it's cut off, what it
    /// sends and what reaches it are lost. Returns whether the node is on
    /// this network.
    pub fn set_online(&self, node: EndpointId, online: bool) -> bool {
        match self.net.port(&node) {
            Some(port) => {
                lock(&port.state).online = online;
                true
            }
            None => false,
        }
    }

    /// What the network has done with packets so far.
    pub fn stats(&self) -> NetStats {
        let c = &self.net.stats;
        NetStats {
            packets: c.packets.load(Relaxed),
            bytes: c.bytes.load(Relaxed),
            delivered: c.delivered.load(Relaxed),
            lost: c.lost.load(Relaxed),
            queue_full: c.queue_full.load(Relaxed),
            cut_off: c.cut_off.load(Relaxed),
        }
    }
}

impl SimLink {
    /// The transport for node `id` to put on its endpoint
    /// (`Builder::add_custom_transport`). The node joins the network when
    /// its endpoint binds, and leaves when the endpoint closes.
    pub fn transport(&self, id: EndpointId) -> Arc<dyn CustomTransport> {
        Arc::new(Transport {
            net: self.net.net.clone(),
            id,
            link: self.link,
        })
    }

    /// Finds the nodes on the network by their IDs
    /// (`Builder::address_lookup`). An endpoint's own address
    /// (`Endpoint::addr`) lists only relays and IP addresses, so what a node
    /// hands out has just its ID, and this finds the rest.
    pub fn lookup(&self) -> impl AddressLookup + use<> {
        Lookup {
            net: self.net.net.clone(),
        }
    }
}

#[derive(Debug)]
struct Lookup {
    net: Arc<Net>,
}

impl AddressLookup for Lookup {
    fn publish(&self, _data: &EndpointData) {}

    fn resolve(&self, node: EndpointId) -> Option<BoxStream<Result<Item, Error>>> {
        self.net.port(&node)?;
        let found = EndpointAddr::from_parts(node, [TransportAddr::Custom(addr(node))]);
        let item = Item::new(EndpointInfo::from(found), "simulated network", None);
        Some(Box::pin(n0_future::stream::once(Ok(item))))
    }
}

/// A node's address on the network.
pub fn addr(node: EndpointId) -> CustomAddr {
    CustomAddr::from_parts(TRANSPORT_ID, node.as_bytes())
}

fn node_of(addr: &CustomAddr) -> Option<EndpointId> {
    if addr.id() != TRANSPORT_ID {
        return None;
    }
    EndpointId::from_bytes(addr.data().try_into().ok()?).ok()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Net {
    fn port(&self, node: &EndpointId) -> Option<Arc<Port>> {
        let ports = self.ports.read().unwrap_or_else(|e| e.into_inner());
        ports.get(node).cloned()
    }

    /// Sends one packet from `from` to `to`, through `from`'s upload queue
    /// and both delays. It goes through `to`'s download queue when it
    /// reaches `to` (`deliver`).
    fn send(&self, from: &Port, to: EndpointId, data: &[u8]) {
        let c = &self.stats;
        c.packets.fetch_add(1, Relaxed);
        c.bytes.fetch_add(data.len() as u64, Relaxed);
        let left = {
            let mut s = lock(&from.state);
            if !s.online {
                c.cut_off.fetch_add(1, Relaxed);
                return;
            }
            let loss = s.link.loss;
            if s.rng.chance(loss) {
                c.lost.fetch_add(1, Relaxed);
                return;
            }
            let up_bps = s.link.up_bps;
            match s.up.admit(Instant::now(), data.len(), up_bps) {
                Some(left) => left + s.link.delay,
                None => {
                    c.queue_full.fetch_add(1, Relaxed);
                    return;
                }
            }
        };
        let Some(port) = self.port(&to) else {
            c.cut_off.fetch_add(1, Relaxed);
            return;
        };
        let at = left + lock(&port.state).link.delay;
        let packet = Packet {
            from: from.id,
            data: data.into(),
        };
        let seq = self.next_seq.fetch_add(1, Relaxed);
        let mut arriving = lock(&port.arriving);
        let earliest = arriving.peek().is_none_or(|a| at < a.at);
        arriving.push(Arriving { at, seq, packet });
        drop(arriving);
        if earliest {
            port.wake.notify_one();
        }
    }

    /// Hands a node the packets that reach it, once they're through its
    /// download queue. Runs until the node's endpoint closes.
    async fn deliver(&self, port: Arc<Port>, tx: mpsc::Sender<Packet>) {
        let c = &self.stats;
        // Through the download queue, in the order they leave it.
        let mut leaving: VecDeque<(Instant, Packet)> = VecDeque::new();
        loop {
            let now = Instant::now();
            let next_arrival = {
                let mut arriving = lock(&port.arriving);
                let mut s = lock(&port.state);
                while arriving.peek().is_some_and(|a| a.at <= now) {
                    let Some(a) = arriving.pop() else { break };
                    if !s.online {
                        c.cut_off.fetch_add(1, Relaxed);
                        continue;
                    }
                    let loss = s.link.loss;
                    if s.rng.chance(loss) {
                        c.lost.fetch_add(1, Relaxed);
                        continue;
                    }
                    // From when it arrived, however late this task woke.
                    let down_bps = s.link.down_bps;
                    match s.down.admit(a.at, a.packet.data.len(), down_bps) {
                        Some(left) => leaving.push_back((left, a.packet)),
                        None => {
                            c.queue_full.fetch_add(1, Relaxed);
                        }
                    }
                }
                arriving.peek().map(|a| a.at)
            };
            while leaving.front().is_some_and(|(left, _)| *left <= now) {
                let Some((_, packet)) = leaving.pop_front() else {
                    break;
                };
                match tx.try_send(packet) {
                    Ok(()) => c.delivered.fetch_add(1, Relaxed),
                    Err(mpsc::error::TrySendError::Full(_)) => c.queue_full.fetch_add(1, Relaxed),
                    Err(mpsc::error::TrySendError::Closed(_)) => return,
                };
            }
            let next = match (next_arrival, leaving.front().map(|(left, _)| *left)) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let sleep = async {
                match next {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = port.wake.notified() => {}
                _ = sleep => {}
                _ = tx.closed() => return,
            }
        }
    }
}

#[derive(Debug)]
struct Transport {
    net: Arc<Net>,
    id: EndpointId,
    link: Link,
}

impl CustomTransport for Transport {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
        Ok(Box::new(self.join()))
    }
}

impl Transport {
    /// Puts the node on the network.
    fn join(&self) -> Endpoint {
        let id_bytes: [u8; 8] = self.id.as_bytes()[..8].try_into().expect("8 bytes");
        let port = Arc::new(Port {
            id: self.id,
            state: Mutex::new(PortState {
                link: self.link,
                online: true,
                up: Queue::default(),
                down: Queue::default(),
                rng: Rng(self.net.seed ^ u64::from_le_bytes(id_bytes)),
            }),
            arriving: Mutex::default(),
            wake: Notify::new(),
        });
        // A node started again takes over from its last endpoint, even if
        // iroh hasn't dropped that one yet.
        let mut ports = self.net.ports.write().unwrap_or_else(|e| e.into_inner());
        ports.insert(self.id, port.clone());
        drop(ports);
        let (tx, rx) = mpsc::channel(RECEIVE_BUFFER);
        let net = self.net.clone();
        let delivering = port.clone();
        let task = tokio::spawn(async move { net.deliver(delivering, tx).await });
        Endpoint {
            net: self.net.clone(),
            port,
            rx,
            addrs: n0_watcher::Watchable::new(vec![addr(self.id)]),
            task,
        }
    }
}

#[derive(Debug)]
struct Endpoint {
    net: Arc<Net>,
    port: Arc<Port>,
    rx: mpsc::Receiver<Packet>,
    addrs: n0_watcher::Watchable<Vec<CustomAddr>>,
    task: JoinHandle<()>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.task.abort();
        let mut ports = self.net.ports.write().unwrap_or_else(|e| e.into_inner());
        if ports
            .get(&self.port.id)
            .is_some_and(|p| Arc::ptr_eq(p, &self.port))
        {
            ports.remove(&self.port.id);
        }
    }
}

impl CustomEndpoint for Endpoint {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
        self.addrs.watch()
    }

    fn create_sender(&self) -> Arc<dyn CustomSender> {
        Arc::new(Sender {
            net: self.net.clone(),
            port: self.port.clone(),
        })
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        metas: &mut [noq_udp::RecvMeta],
        recv_infos: &mut [RecvInfo],
    ) -> Poll<io::Result<usize>> {
        let slots = bufs.len().min(metas.len()).min(recv_infos.len());
        let mut filled = 0;
        while filled < slots {
            let packet = match self.rx.poll_recv(cx) {
                Poll::Ready(Some(packet)) => packet,
                Poll::Ready(None) if filled == 0 => {
                    return Poll::Ready(Err(io::Error::other("left the simulated network")));
                }
                Poll::Ready(None) | Poll::Pending => break,
            };
            let len = packet.data.len();
            // Too big for the buffer: lost, as a datagram would be.
            if len > bufs[filled].len() {
                continue;
            }
            bufs[filled][..len].copy_from_slice(&packet.data);
            metas[filled].len = len;
            metas[filled].stride = len;
            recv_infos[filled] = RecvInfo::new(addr(packet.from), Some(addr(self.port.id)));
            filled += 1;
        }
        if filled == 0 && slots > 0 {
            Poll::Pending
        } else {
            Poll::Ready(Ok(filled))
        }
    }
}

#[derive(Debug)]
struct Sender {
    net: Arc<Net>,
    port: Arc<Port>,
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
            return Poll::Ready(Err(io::Error::other(
                "not an address on the simulated network",
            )));
        };
        let contents = transmit.contents;
        let size = transmit.segment_size.unwrap_or(contents.len()).max(1);
        for datagram in contents.chunks(size) {
            self.net.send(&self.port, to, datagram);
        }
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MBPS: u64 = 1_000_000;

    #[test]
    fn packets_leave_one_after_another_at_the_links_speed() {
        let now = Instant::now();
        let mut q = Queue::default();
        // 1,250 bytes is 10,000 bits: 1 ms at 10 Mbit/s.
        let ms = Duration::from_millis(1);
        assert_eq!(q.admit(now, 1_250, 10 * MBPS), Some(now + ms));
        assert_eq!(q.admit(now, 1_250, 10 * MBPS), Some(now + 2 * ms));
        // One that comes once the queue is empty leaves straight away.
        let later = now + Duration::from_secs(1);
        assert_eq!(q.admit(later, 1_250, 10 * MBPS), Some(later + ms));
        // No limit: no wait, and nothing is queued.
        let mut q = Queue::default();
        assert_eq!(q.admit(now, 1_250, 0), Some(now));
        assert_eq!(q.free_at, None);
    }

    #[test]
    fn a_full_queue_drops_packets() {
        let now = Instant::now();
        let mut q = Queue::default();
        // 1 ms each, so 100 fill the queue's 100 ms; the next would wait
        // longer and is dropped, while one that waits exactly that long isn't.
        for _ in 0..100 {
            assert!(q.admit(now, 1_250, 10 * MBPS).is_some());
        }
        assert_eq!(q.free_at, Some(now + QUEUE));
        assert_eq!(
            q.admit(now, 1_250, 10 * MBPS),
            Some(now + QUEUE + Duration::from_millis(1))
        );
        assert_eq!(q.admit(now, 1_250, 10 * MBPS), None);
        // Once some have left, there's room again.
        let later = now + Duration::from_millis(10);
        assert!(q.admit(later, 1_250, 10 * MBPS).is_some());
    }

    #[test]
    fn losses_follow_the_chance_and_the_seed() {
        let mut rng = Rng(1);
        assert!((0..1_000).all(|_| !rng.chance(0.0)));
        assert!((0..1_000).all(|_| rng.chance(1.0)));
        let lost = (0..100_000).filter(|_| rng.chance(0.01)).count();
        assert!((800..1_200).contains(&lost), "{lost}");
        let draws = |seed| {
            let mut rng = Rng(seed);
            (0..64).map(|_| rng.chance(0.5)).collect::<Vec<_>>()
        };
        assert_eq!(draws(7), draws(7));
        assert_ne!(draws(7), draws(8));
    }

    /// A node on `net` with no delay or limits.
    fn join(net: &SimNet) -> Endpoint {
        let id = iroh::SecretKey::generate().public();
        let link = Link::default();
        let net = net.net.clone();
        Transport { net, id, link }.join()
    }

    #[tokio::test]
    async fn a_node_cut_off_neither_sends_nor_receives() {
        let net = SimNet::new(1);
        let mut a = join(&net);
        let mut b = join(&net);
        let (a_id, b_id) = (a.port.id, b.port.id);
        net.net.send(&a.port, b_id, b"hello");
        assert_eq!(&b.rx.recv().await.unwrap().data[..], b"hello");

        assert!(net.set_online(a_id, false));
        net.net.send(&a.port, b_id, b"out");
        net.net.send(&b.port, a_id, b"in");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(b.rx.try_recv().is_err(), "nothing it sends gets out");
        assert!(a.rx.try_recv().is_err(), "nothing sent to it gets in");
        assert_eq!(net.stats().cut_off, 2);

        net.set_online(a_id, true);
        net.net.send(&b.port, a_id, b"back");
        assert_eq!(&a.rx.recv().await.unwrap().data[..], b"back");
        let stats = net.stats();
        assert_eq!((stats.packets, stats.delivered), (4, 2));
    }

    #[test]
    fn addresses_name_nodes() {
        let node = iroh::SecretKey::generate().public();
        assert_eq!(node_of(&addr(node)), Some(node));
        let other = CustomAddr::from_parts(TRANSPORT_ID + 1, node.as_bytes());
        assert_eq!(node_of(&other), None);
        assert_eq!(
            node_of(&CustomAddr::from_parts(TRANSPORT_ID, &[1, 2])),
            None
        );
    }
}
