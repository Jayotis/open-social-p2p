//! One post spreading through the follower swarm, as a fluid model: no
//! packets, but each transfer gets a rate each second from the uploads,
//! downloads, relay limits and carrying limits along its path, shared out
//! about as QUIC shares a link (near enough max-min fair).
//!
//! What a node does is copied from `osp-node`. It waits a random moment in
//! the 10 minutes after a post, asks the creator's node who holds a copy,
//! and fetches in parts (`fetch_in_parts`): the first 3 named each send an
//! equal part; one that finishes takes the next part not taken, or the end
//! of the part that will take longest, if that ends it at least 2 s sooner
//! (`split_slowest`), and what the slower one had already sent past the new
//! end, up to QUIC's stream window, is sent for nothing. One more node is
//! asked only when none of the first is fetching; the creator's node last.
//! A node serves one transfer for each 1.5 Mbit/s it has measured itself
//! sending over 5 s, 2 until it has, and at most 8; a request waits up to
//! 10 s for a place before it's refused as busy, and the node tries again
//! later, after a minute, then two, and so on. Which sources and which path
//! are the rules' to say (`Rules`); this only moves the bytes.

use super::rng::Rng;

/// B8's video, once encoded (`--video-mb`'s default in `osp-sim`).
pub const VIDEO_BYTES: f64 = 52.3e6;
/// Bytes a second, from megabits a second.
pub fn mbps(m: f64) -> f64 {
    m * 1e6 / 8.0
}

/// Copied from `osp-node` (store.rs, sync.rs).
const MBPS_PER_TRANSFER: f64 = 1.5;
const MIN_TRANSFERS: usize = 2;
const MAX_TRANSFERS: usize = 8;
const AT_ONCE: usize = 3;
const BUSY_WAIT: f64 = 10.0;
const FIRST_RETRY_WAIT: f64 = 60.0;
const MAX_RETRY_WAIT: f64 = 15.0 * 60.0;
const MAX_RETRIES: u32 = 6;
const SPLIT_GAIN: f64 = 2.0;
const MEASURE_OVER: usize = 5;
/// QUIC's stream window: what a node sending a part may have sent ahead of
/// what's been read (`SEND_AHEAD`), and so the most it sends for nothing
/// when another node takes over the end of its part.
const SEND_AHEAD: f64 = 1.25e6;
/// Bytes on the wire for each byte of media: with packets of 1,200 to 1,450
/// bytes, QUIC's header, encryption tag and frame header add about 3%, and
/// an acknowledgement every two packets about 2%. The simulated relay
/// counts whole packets, so the model counts them too.
pub const OVERHEAD: f64 = 1.05;

/// n0's public relays, as measured ([media through relays]): a burst, then a
/// steady rate, for what each endpoint sends through one.
///
/// [media through relays]: ../../../docs/testing/network-conditions.md#media-through-relays
pub const RELAY_BURST: f64 = 2.7e6;
pub const RELAY_RATE: f64 = 1.0e6;

/// Transfers a node serves at once, having measured `mbps`.
pub fn transfer_places(mbps: f64) -> usize {
    ((mbps / MBPS_PER_TRANSFER) as usize).clamp(MIN_TRANSFERS, MAX_TRANSFERS)
}

#[derive(Clone, Debug)]
pub struct Peer {
    /// Bytes a second.
    pub up: f64,
    pub down: f64,
    /// The fastest it has measured itself sending over 5 s, in Mbit/s.
    pub measured: Option<f64>,
    /// Transfers carried at once as a hop; 0 for a node that doesn't carry.
    pub carry_places: usize,
    /// The share of its upload it carries within, behind its own serving.
    pub carry_share: f64,
}

impl Peer {
    pub fn new(up_mbps: f64, down_mbps: f64) -> Self {
        Peer {
            up: mbps(up_mbps),
            down: mbps(down_mbps),
            measured: None,
            carry_places: 0,
            carry_share: 0.0,
        }
    }

    /// Transfers served at once.
    pub fn places(&self) -> usize {
        self.measured.map_or(MIN_TRANSFERS, transfer_places)
    }
}

/// One leg of a transfer's path: sent by `from` to the next leg's sender, or,
/// for the last leg, to the node fetching.
#[derive(Clone, Debug, PartialEq)]
pub struct Leg {
    pub from: usize,
    /// Through the relay, held to its limit for what `from` sends.
    pub relayed: bool,
    /// `from` is a hop carrying it, not the source.
    pub carried: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Sources {
    /// Tried first, up to 3 at once, each sending a part.
    pub at_once: Vec<usize>,
    /// Then one at a time: the creator's node last.
    pub in_turn: Vec<usize>,
}

/// A transfer that ended: a part sent, and any more it took over.
#[derive(Clone, Debug)]
pub struct Part {
    pub source: usize,
    pub to: usize,
    pub legs: Vec<Leg>,
    /// Bytes of the file it delivered, every one checked.
    pub bytes: f64,
    pub started: f64,
}

/// What the rules see of who's busy when they pick a path.
pub struct Load<'a> {
    pub serving: &'a [usize],
    pub carrying: &'a [usize],
    pub peers: &'a [Peer],
}

impl Load<'_> {
    pub fn can_carry(&self, n: usize) -> bool {
        self.carrying[n] < self.peers[n].carry_places
    }
}

/// The creator's node's record of followers holding a copy (`swarm` in
/// `osp-node`), and how it names them (`pick_hosts`).
pub struct Swarm {
    /// In the order they were noted.
    pub noted: Vec<usize>,
    /// Whether a follower is reached directly, so named first.
    pub direct: Vec<bool>,
    /// Lower is followed longer.
    pub followed_since: Vec<usize>,
}

/// Named for a file at most (`osp-node`): half those that followed longest,
/// half those noted last.
const SWARM_PLACES: usize = 100;
/// Named in one reply.
pub const SWARM_SAMPLE: usize = 6;

impl Swarm {
    pub fn new(n: usize) -> Self {
        Swarm {
            noted: Vec::new(),
            direct: vec![false; n],
            followed_since: (0..n).collect(),
        }
    }

    /// The followers that can be named: all of them, or once there are more
    /// than 100, half that followed longest and half noted last.
    pub fn pool(&self) -> Vec<usize> {
        if self.noted.len() <= SWARM_PLACES {
            return self.noted.clone();
        }
        let mut longest = self.noted.clone();
        longest.sort_by_key(|&n| self.followed_since[n]);
        longest.truncate(SWARM_PLACES / 2);
        let mut pool = longest.clone();
        for &n in self.noted.iter().rev() {
            if pool.len() == SWARM_PLACES {
                break;
            }
            if !longest.contains(&n) {
                pool.push(n);
            }
        }
        pool
    }

    /// Up to 6 named to `to`, at random, those reached directly first.
    pub fn named(&self, to: usize, rng: &mut Rng) -> Vec<usize> {
        let mut pool: Vec<usize> = self.pool().into_iter().filter(|&n| n != to).collect();
        rng.shuffle(&mut pool);
        pool.truncate(SWARM_SAMPLE);
        pool.sort_by_key(|&n| !self.direct[n]);
        pool
    }
}

pub trait Rules {
    /// Where `to` fetches the post from, asked when it wakes.
    fn sources(&mut self, to: usize, swarm: &Swarm, rng: &mut Rng) -> Sources;
    /// The path of a transfer from `source` to `to`, picked when it starts.
    fn path(&mut self, source: usize, to: usize, load: &Load, rng: &mut Rng) -> Vec<Leg>;
    /// A transfer ended.
    fn part_done(&mut self, part: &Part, now: f64);
    /// `node` holds the post now.
    fn holds(&mut self, node: usize, now: f64);
}

/// Who fetches, and when.
pub struct Fetch {
    pub node: usize,
    /// Seconds after the post when it asks who holds a copy.
    pub wakes: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    /// Waiting for an answer to `WhoHosts`.
    Asking(f64),
    /// No source asked had a place: waiting up to `BUSY_WAIT` for one.
    Queued(f64),
    Active,
    /// Refused as busy; tries again then.
    Later(f64),
    Done,
    Failed,
}

struct Download {
    node: usize,
    state: State,
    failures: u32,
    /// Asked for a place now: the first 3 sources, then one at a time.
    group: Vec<usize>,
    /// Asked next, in order, only if none fetching has a place.
    queue: Vec<usize>,
    /// Parts not taken yet, in bytes.
    todo: Vec<f64>,
    planned: bool,
    streams: Vec<usize>,
    /// Sources that sent their part, at this rate, and may take over the
    /// end of another's.
    idle: Vec<(usize, f64)>,
}

struct Stream {
    download: usize,
    source: usize,
    legs: Vec<Leg>,
    resources: Vec<usize>,
    /// Bytes of its part still to send, and bytes it sends for nothing first.
    part_left: f64,
    junk: f64,
    bytes: f64,
    started: f64,
    rate: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// When each node held the post, in seconds after it, if it did.
    pub held: Vec<Option<f64>>,
    /// Bytes on the wire each node sent, as a source and as a hop.
    pub sent: Vec<f64>,
    /// … as the source, and carrying for others.
    pub served: Vec<f64>,
    pub carried: Vec<f64>,
    /// Bytes on the wire through the relay, once for each relayed leg.
    pub relayed: f64,
    /// Bytes of the file delivered, and of those, with no leg through the
    /// relay, and through hops.
    pub delivered: f64,
    pub delivered_off_relay: f64,
    pub delivered_through_hops: f64,
    /// Bytes received and thrown away, as another node took over a part.
    pub wasted: f64,
    /// Requests refused as busy, and downloads that gave up.
    pub busy: usize,
    pub failed: usize,
}

pub struct Spread<'a, R: Rules> {
    pub peers: &'a mut [Peer],
    pub rules: &'a mut R,
    pub rng: &'a mut Rng,
    /// The creator's node, which holds the post at the start.
    pub creator: usize,
    /// How long `WhoHosts` takes to be answered, and telling the creator's
    /// node of a copy to be noted (messages go over Tor).
    pub ask_delay: (f64, f64),
    pub tell_delay: f64,
    pub bytes: f64,
    /// Give up on the post this long after it.
    pub until: f64,
}

/// Resources a stream uses: each node's upload, what it sends through the
/// relay, what it carries, and its download.
#[derive(Clone, Copy)]
enum Res {
    Up = 0,
    Relay = 1,
    Carry = 2,
    Down = 3,
}

/// The run's state, apart from the borrowed settings.
struct Live {
    n: usize,
    holds: Vec<bool>,
    serving: Vec<usize>,
    carrying: Vec<usize>,
    streams: Vec<Option<Stream>>,
    window: Vec<[f64; MEASURE_OVER]>,
    sending_for: Vec<usize>,
}

impl<R: Rules> Spread<'_, R> {
    /// Spreads the post to those in `fetches`; `holding` hold it already.
    pub fn run(&mut self, swarm: &mut Swarm, fetches: &[Fetch], holding: &[usize]) -> Outcome {
        let n = self.peers.len();
        let res = |r: Res, node: usize| r as usize * n + node;
        let mut out = Outcome {
            held: vec![None; n],
            sent: vec![0.0; n],
            served: vec![0.0; n],
            carried: vec![0.0; n],
            ..Default::default()
        };
        let mut live = Live {
            n,
            holds: vec![false; n],
            serving: vec![0; n],
            carrying: vec![0; n],
            streams: Vec::new(),
            window: vec![[0.0; MEASURE_OVER]; n],
            sending_for: vec![0; n],
        };
        out.held[self.creator] = Some(0.0);
        live.holds[self.creator] = true;
        for &h in holding {
            live.holds[h] = true;
            out.held[h] = Some(0.0);
        }
        let mut tokens = vec![RELAY_BURST; n];
        let mut downloads: Vec<Download> = fetches
            .iter()
            .map(|f| Download {
                node: f.node,
                state: State::Asking(f.wakes + self.rng.range(self.ask_delay.0, self.ask_delay.1)),
                failures: 0,
                group: Vec::new(),
                queue: Vec::new(),
                todo: Vec::new(),
                planned: false,
                streams: Vec::new(),
                idle: Vec::new(),
            })
            .collect();
        let mut tells: Vec<(f64, usize)> = Vec::new();
        let mut t = 0.0f64;
        let dt = 1.0;
        let mut second = 0usize;
        while t < self.until {
            // Copies noted by the creator's node.
            tells.retain(|&(at, node)| {
                if at <= t {
                    swarm.noted.push(node);
                    false
                } else {
                    true
                }
            });
            for d in 0..downloads.len() {
                match downloads[d].state {
                    State::Asking(at) if at <= t => {
                        // The first 3 to try at once start the parts, and one
                        // more is asked only when none of them is fetching;
                        // the creator's node last.
                        let node = downloads[d].node;
                        let sources = self.rules.sources(node, swarm, self.rng);
                        let mut list: Vec<usize> = Vec::new();
                        for &s in sources.at_once.iter().chain(&sources.in_turn) {
                            if s != node && !list.contains(&s) {
                                list.push(s);
                            }
                        }
                        let at_once = sources.at_once.iter().filter(|&&s| s != node).count();
                        let first = at_once.clamp(1, AT_ONCE).min(list.len());
                        let dl = &mut downloads[d];
                        dl.queue = list.split_off(first);
                        dl.group = list;
                        if !dl.planned {
                            let parts = at_once.clamp(1, AT_ONCE);
                            dl.todo = vec![self.bytes / parts as f64; parts];
                            dl.planned = true;
                        }
                        dl.state = State::Queued(t + BUSY_WAIT);
                        self.admit(d, &mut downloads, &mut live, t, &mut out.busy);
                    }
                    State::Queued(deadline) => {
                        self.admit(d, &mut downloads, &mut live, t, &mut out.busy);
                        let dl = &mut downloads[d];
                        if dl.state == State::Queued(deadline) && t >= deadline {
                            out.busy += dl.group.len();
                            if !dl.queue.is_empty() {
                                dl.group = vec![dl.queue.remove(0)];
                                dl.state = State::Queued(t + BUSY_WAIT);
                            } else if dl.failures >= MAX_RETRIES {
                                dl.state = State::Failed;
                                out.failed += 1;
                            } else {
                                let wait = (FIRST_RETRY_WAIT
                                    * f64::from(1u32 << dl.failures.min(16)))
                                .min(MAX_RETRY_WAIT);
                                dl.failures += 1;
                                dl.state = State::Later(t + self.rng.jitter(wait));
                            }
                        }
                    }
                    State::Later(at) if at <= t => {
                        downloads[d].state =
                            State::Asking(t + self.rng.range(self.ask_delay.0, self.ask_delay.1));
                    }
                    _ => {}
                }
            }
            let ids: Vec<usize> = (0..live.streams.len())
                .filter(|&s| live.streams[s].is_some())
                .collect();
            if ids.is_empty() {
                // Nothing moving: skip to the next thing that happens.
                let next = downloads
                    .iter()
                    .filter_map(|d| match d.state {
                        State::Asking(at) | State::Later(at) => Some(at),
                        State::Queued(at) => Some(at.min(t + dt)),
                        _ => None,
                    })
                    .chain(tells.iter().map(|&(at, _)| at))
                    .fold(f64::INFINITY, f64::min);
                if !next.is_finite() {
                    break;
                }
                tokens.iter_mut().for_each(|tk| *tk = RELAY_BURST);
                for w in live.window.iter_mut() {
                    *w = [0.0; MEASURE_OVER];
                }
                live.sending_for.iter_mut().for_each(|s| *s = 0);
                t = next.max(t + dt).ceil();
                continue;
            }
            // Each resource's capacity this second, in bytes of media.
            let mut cap = vec![0.0f64; 4 * n];
            for (i, p) in self.peers.iter().enumerate() {
                tokens[i] = (tokens[i] + RELAY_RATE * dt).min(RELAY_BURST);
                cap[res(Res::Up, i)] = p.up / OVERHEAD;
                cap[res(Res::Relay, i)] = tokens[i] / dt / OVERHEAD;
                cap[res(Res::Carry, i)] = p.up * p.carry_share / OVERHEAD;
                cap[res(Res::Down, i)] = p.down / OVERHEAD;
            }
            self.share(&mut live.streams, &ids, &mut cap, n);
            // Move the bytes.
            let mut sent_now = vec![0.0f64; n];
            let mut part_ends: Vec<usize> = Vec::new();
            for &s in &ids {
                let st = live.streams[s].as_mut().unwrap();
                let moved = st.rate * dt;
                let junk = moved.min(st.junk);
                st.junk -= junk;
                let useful = (moved - junk).min(st.part_left.max(0.0));
                st.part_left -= useful;
                st.bytes += useful;
                let wire = (junk + useful) * OVERHEAD;
                let mut through_relay = false;
                for leg in &st.legs {
                    out.sent[leg.from] += wire;
                    sent_now[leg.from] += wire;
                    if leg.carried {
                        out.carried[leg.from] += wire;
                    } else {
                        out.served[leg.from] += wire;
                    }
                    if leg.relayed {
                        out.relayed += wire;
                        tokens[leg.from] = (tokens[leg.from] - wire).max(0.0);
                        through_relay = true;
                    }
                }
                out.wasted += junk;
                out.delivered += useful;
                if !through_relay {
                    out.delivered_off_relay += useful;
                }
                if st.legs.iter().any(|l| l.carried) {
                    out.delivered_through_hops += useful;
                }
                if st.part_left <= 1e-6 && st.junk <= 1e-6 {
                    part_ends.push(s);
                }
            }
            t += dt;
            second += 1;
            // What each node measures itself sending, over 5 s.
            for (i, &sent) in sent_now.iter().enumerate() {
                live.window[i][second % MEASURE_OVER] = sent;
                if sent > 0.0 {
                    live.sending_for[i] += 1;
                } else {
                    live.sending_for[i] = 0;
                }
                if live.sending_for[i] >= MEASURE_OVER {
                    let rate = live.window[i].iter().sum::<f64>() / MEASURE_OVER as f64 * 8.0 / 1e6;
                    let p = &mut self.peers[i];
                    p.measured = Some(p.measured.map_or(rate, |m| m.max(rate)));
                }
            }
            // A source that sent its part takes the next, or the end of the
            // slowest, or stops.
            for s in part_ends {
                let Some(st) = live.streams[s].as_ref() else {
                    continue;
                };
                let d = st.download;
                let (own_rate, source) = (st.rate, st.source);
                if let Some(part) = downloads[d].todo.pop() {
                    live.streams[s].as_mut().unwrap().part_left = part;
                    continue;
                }
                let of = downloads[d].streams.clone();
                if let Some(take) = split_slowest(&mut live.streams, &of, s, own_rate) {
                    live.streams[s].as_mut().unwrap().part_left = take;
                    continue;
                }
                downloads[d].idle.push((source, own_rate));
                self.end_stream(s, &mut downloads, &mut live, t);
            }
            // Sources with nothing to do look again for a part worth taking,
            // if they still have a place.
            for d in 0..downloads.len() {
                if downloads[d].state != State::Active || downloads[d].idle.is_empty() {
                    continue;
                }
                let idle = std::mem::take(&mut downloads[d].idle);
                let mut keep = Vec::new();
                for (source, rate) in idle {
                    if live.serving[source] >= self.peers[source].places() {
                        keep.push((source, rate));
                        continue;
                    }
                    let of = downloads[d].streams.clone();
                    match split_slowest(&mut live.streams, &of, usize::MAX, rate) {
                        Some(take) => {
                            self.start_stream(d, source, take, &mut downloads, &mut live, t)
                        }
                        None => keep.push((source, rate)),
                    }
                }
                downloads[d].idle = keep;
            }
            // Downloads finished.
            for d in 0..downloads.len() {
                if downloads[d].state != State::Active {
                    continue;
                }
                let left: f64 = downloads[d].todo.iter().sum::<f64>()
                    + downloads[d]
                        .streams
                        .iter()
                        .map(|&s| {
                            live.streams[s]
                                .as_ref()
                                .map_or(0.0, |st| st.part_left.max(0.0))
                        })
                        .sum::<f64>();
                if left > 1e-6 {
                    if downloads[d].streams.is_empty() && downloads[d].idle.is_empty() {
                        // Its sources stopped with parts left: ask again.
                        downloads[d].state = State::Asking(t);
                    }
                    continue;
                }
                let node = downloads[d].node;
                downloads[d].state = State::Done;
                out.held[node] = Some(t);
                live.holds[node] = true;
                self.rules.holds(node, t);
                tells.push((t + self.tell_delay, node));
                for s in downloads[d].streams.clone() {
                    if let Some(st) = live.streams[s].as_ref() {
                        out.wasted += st.junk;
                    }
                    self.end_stream(s, &mut downloads, &mut live, t);
                }
                downloads[d].idle.clear();
            }
        }
        out
    }

    fn end_stream(&mut self, s: usize, downloads: &mut [Download], live: &mut Live, now: f64) {
        let Some(st) = live.streams[s].take() else {
            return;
        };
        downloads[st.download].streams.retain(|&x| x != s);
        live.serving[st.source] -= 1;
        for leg in &st.legs {
            if leg.carried {
                live.carrying[leg.from] -= 1;
            }
        }
        let to = downloads[st.download].node;
        self.rules.part_done(
            &Part {
                source: st.source,
                to,
                legs: st.legs,
                bytes: st.bytes,
                started: st.started,
            },
            now,
        );
    }

    fn start_stream(
        &mut self,
        d: usize,
        source: usize,
        part: f64,
        downloads: &mut [Download],
        live: &mut Live,
        t: f64,
    ) {
        let n = live.n;
        let node = downloads[d].node;
        let legs = {
            let load = Load {
                serving: &live.serving,
                carrying: &live.carrying,
                peers: self.peers,
            };
            self.rules.path(source, node, &load, self.rng)
        };
        live.serving[source] += 1;
        let mut resources = Vec::new();
        for leg in &legs {
            resources.push(Res::Up as usize * n + leg.from);
            if leg.relayed {
                resources.push(Res::Relay as usize * n + leg.from);
            }
            if leg.carried {
                live.carrying[leg.from] += 1;
                resources.push(Res::Carry as usize * n + leg.from);
            }
        }
        resources.push(Res::Down as usize * n + node);
        let id = live.streams.len();
        live.streams.push(Some(Stream {
            download: d,
            source,
            legs,
            resources,
            part_left: part,
            junk: 0.0,
            bytes: 0.0,
            started: t,
            rate: 0.0,
        }));
        downloads[d].streams.push(id);
    }

    /// Starts parts for download `d` from those asked now that have a place.
    /// Those without one are refused as busy, and their parts wait for the
    /// others.
    fn admit(
        &mut self,
        d: usize,
        downloads: &mut [Download],
        live: &mut Live,
        t: f64,
        busy: &mut usize,
    ) {
        let node = downloads[d].node;
        let picked: Vec<usize> = downloads[d]
            .group
            .iter()
            .copied()
            .filter(|&s| s != node && live.holds[s] && live.serving[s] < self.peers[s].places())
            .collect();
        if picked.is_empty() {
            return;
        }
        *busy += downloads[d].group.len() - picked.len();
        downloads[d].group.clear();
        for source in picked {
            let Some(part) = downloads[d].todo.pop() else {
                break;
            };
            self.start_stream(d, source, part, downloads, live, t);
        }
        downloads[d].state = State::Active;
    }

    /// Gives each live stream a rate, near enough max-min fair: equal shares
    /// first, then each resource's spare, left by streams held back
    /// elsewhere, shared among those it holds back, a few times over; then
    /// cut back wherever that's more than a resource has. A node's own
    /// serving comes before what it carries.
    fn share(&self, streams: &mut [Option<Stream>], ids: &[usize], cap: &mut [f64], n: usize) {
        let mut users = vec![0usize; cap.len()];
        for &s in ids {
            for &r in &streams[s].as_ref().unwrap().resources {
                users[r] += 1;
            }
        }
        let equal = |st: &Stream, cap: &[f64]| {
            st.resources
                .iter()
                .map(|&r| cap[r] / users[r] as f64)
                .fold(f64::INFINITY, f64::min)
        };
        let mut rate: Vec<f64> = ids
            .iter()
            .map(|&s| equal(streams[s].as_ref().unwrap(), cap))
            .collect();
        for _ in 0..4 {
            // What each node's own serving uses, so carrying gets what's left.
            let mut own = vec![0.0f64; n];
            for (k, &s) in ids.iter().enumerate() {
                own[streams[s].as_ref().unwrap().legs[0].from] += rate[k];
            }
            for (i, p) in self.peers.iter().enumerate() {
                let up = p.up / OVERHEAD;
                cap[Res::Carry as usize * n + i] = (up * p.carry_share).min((up - own[i]).max(0.0));
            }
            // Each stream's tightest resource; the spare elsewhere.
            let mut fixed = vec![0.0f64; cap.len()];
            let mut held_here = vec![0usize; cap.len()];
            for (k, &s) in ids.iter().enumerate() {
                let st = streams[s].as_ref().unwrap();
                let mut tightest = st.resources[0];
                let mut least = f64::INFINITY;
                for &r in &st.resources {
                    let share = cap[r] / users[r] as f64;
                    if share < least {
                        least = share;
                        tightest = r;
                    }
                }
                for &r in &st.resources {
                    if r == tightest {
                        held_here[r] += 1;
                    } else {
                        fixed[r] += rate[k];
                    }
                }
            }
            for (k, &s) in ids.iter().enumerate() {
                let st = streams[s].as_ref().unwrap();
                let r = st
                    .resources
                    .iter()
                    .filter(|&&r| held_here[r] > 0)
                    .map(|&r| {
                        ((cap[r] - fixed[r]).max(0.0) / held_here[r] as f64)
                            .max(cap[r] / users[r] as f64)
                    })
                    .fold(f64::INFINITY, f64::min);
                rate[k] = if r.is_finite() { r } else { equal(st, cap) };
            }
        }
        // Never more than a resource has.
        let mut used = vec![0.0f64; cap.len()];
        for (k, &s) in ids.iter().enumerate() {
            for &r in &streams[s].as_ref().unwrap().resources {
                used[r] += rate[k];
            }
        }
        for (k, &s) in ids.iter().enumerate() {
            let st = streams[s].as_mut().unwrap();
            let factor = st
                .resources
                .iter()
                .map(|&r| {
                    if used[r] > cap[r] {
                        cap[r] / used[r]
                    } else {
                        1.0
                    }
                })
                .fold(1.0f64, f64::min);
            st.rate = rate[k] * factor;
        }
    }
}

/// For a source with nothing to do, sending at `rate`: takes over the end of
/// the part among `of` that will take longest, split so both finish
/// together, if that ends it at least `SPLIT_GAIN` sooner (`split_slowest`
/// in `osp-node`). What the slower one had sent past the new end, up to
/// QUIC's stream window, it sends for nothing. Returns the bytes taken.
fn split_slowest(
    streams: &mut [Option<Stream>],
    of: &[usize],
    taker: usize,
    rate: f64,
) -> Option<f64> {
    if rate <= 0.0 {
        return None;
    }
    let slowest = of
        .iter()
        .copied()
        .filter(|&s| s != taker)
        .filter_map(|s| streams[s].as_ref().map(|st| (s, st)))
        .filter(|(_, st)| st.part_left > 1e-6)
        .max_by(|(_, a), (_, b)| {
            let secs = |st: &Stream| {
                if st.rate > 0.0 {
                    st.part_left / st.rate
                } else {
                    f64::INFINITY
                }
            };
            secs(a).total_cmp(&secs(b))
        })
        .map(|(s, _)| s)?;
    let st = streams[slowest].as_mut().unwrap();
    let left = st.part_left;
    let own = st.rate;
    let gain = if own > 0.0 {
        left / own - left / (own + rate)
    } else {
        f64::INFINITY
    };
    if gain < SPLIT_GAIN {
        return None;
    }
    let keep = if own > 0.0 {
        left * own / (own + rate)
    } else {
        0.0
    };
    st.part_left = keep;
    st.junk += (left.min(SEND_AHEAD) - keep).max(0.0);
    Some(left - keep)
}
