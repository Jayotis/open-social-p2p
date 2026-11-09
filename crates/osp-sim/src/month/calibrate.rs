//! Mark 0 of the [plan]: the model, with no hops and no earned links, run as
//! the [relay runs] measured on real nodes were: one post, 300 followers at
//! 5, 10 and 20 Mbit/s up in turn, the creator at 20, 10 latecomers once they
//! all hold it, and a share of the followers behind the relay, the others
//! allowing anyone to connect directly.
//!
//! [plan]: ../../../docs/testing/hops-model.md#pass-marks
//! [relay runs]: ../../../docs/testing/network-conditions.md#media-through-relays

use serde::Serialize;

use super::rng::Rng;
use super::spread::{Fetch, Leg, Load, Part, Peer, Rules, Sources, Spread, Swarm, VIDEO_BYTES};

pub const FOLLOWERS: usize = 300;
pub const LATECOMERS: usize = 10;
const UP: [f64; 3] = [5.0, 10.0, 20.0];
const DOWN: [f64; 3] = [50.0, 100.0, 300.0];
/// Followers spread their fetches over this long (`PREFETCH_SPREAD`).
pub const SPREAD_SECS: f64 = 600.0;
/// `WhoHosts` over a connection already open: one round trip over Tor
/// (`--tor-delay`'s 0.7 to 2.5 s), and a copy being noted: a round trip and
/// the creator's check of a piece.
pub const ASK_DELAY: (f64, f64) = (0.7, 2.5);
pub const TELL_DELAY: f64 = 2.5;

/// The measured runs, and mark 0's tolerances.
pub struct Measured {
    pub percent: usize,
    pub p95: f64,
    pub relay_copies: f64,
}

pub const MEASURED: [Measured; 3] = [
    Measured {
        percent: 0,
        p95: 704.0,
        relay_copies: 0.0,
    },
    Measured {
        percent: 50,
        p95: 655.0,
        relay_copies: 167.3,
    },
    Measured {
        percent: 100,
        // Measured again with the prefetch fix (b43f2d8): the first run's
        // 1,535 s and 369.7 copies fetched some bytes twice.
        p95: 1376.0,
        relay_copies: 325.3,
    },
];
const P95_WITHIN: f64 = 0.2;
const RELAY_WITHIN: f64 = 0.1;
const CREATOR_COPIES: (f64, f64) = (15.0, 30.0);

/// Whether the `i`th follower or latecomer is one of the `percent` in a
/// hundred behind the relay, spread evenly (`relay_only` in `osp-sim`).
fn relay_only(i: usize, percent: usize) -> bool {
    (i * percent) % 100 < percent
}

struct Plain {
    behind_relay: Vec<bool>,
    creator: usize,
}

impl Rules for Plain {
    fn sources(&mut self, to: usize, swarm: &Swarm, rng: &mut Rng) -> Sources {
        Sources {
            at_once: swarm.named(to, rng),
            in_turn: vec![self.creator],
        }
    }

    fn path(&mut self, source: usize, to: usize, _load: &Load, _rng: &mut Rng) -> Vec<Leg> {
        vec![Leg {
            from: source,
            relayed: self.behind_relay[source] || self.behind_relay[to],
            carried: false,
        }]
    }

    fn part_done(&mut self, _part: &Part, _now: f64) {}
    fn holds(&mut self, _node: usize, _now: f64) {}
}

#[derive(Debug, Serialize)]
pub struct Run {
    pub percent: usize,
    /// Seconds after the post for 50%, 95% and all of the followers to hold it.
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub all: Option<f64>,
    pub creator_copies: f64,
    pub relay_copies: f64,
    /// What followers received over one copy each, sent twice as parts were
    /// taken over: 6, 0.3, 3 and 14% in the measured runs.
    pub followers_extra: f64,
    pub busy: usize,
    pub failed: usize,
    /// Seconds each latecomer took, in order.
    pub latecomers: Vec<f64>,
    pub mark: Option<bool>,
}

pub fn time_for(held: &[Option<f64>], share: f64) -> Option<f64> {
    let mut times: Vec<f64> = held.iter().flatten().copied().collect();
    times.sort_by(f64::total_cmp);
    let needed = ((held.len() as f64) * share).ceil() as usize;
    if needed == 0 || times.len() < needed {
        None
    } else {
        Some(times[needed - 1])
    }
}

pub fn run(percent: usize, seed: u64) -> Run {
    let mut rng = Rng::new(seed);
    let creator = 0;
    let mut peers = vec![Peer::new(20.0, 100.0)];
    let mut behind_relay = vec![false];
    for i in 0..FOLLOWERS + LATECOMERS {
        let k = if i < FOLLOWERS { i } else { i - FOLLOWERS };
        peers.push(Peer::new(UP[i % 3], DOWN[i % 3]));
        behind_relay.push(relay_only(k, percent));
    }
    let n = peers.len();
    let mut swarm = Swarm::new(n);
    for (i, &relayed) in behind_relay.iter().enumerate() {
        swarm.direct[i] = !relayed;
    }
    let mut rules = Plain {
        behind_relay,
        creator,
    };
    let fetches: Vec<Fetch> = (1..=FOLLOWERS)
        .map(|node| Fetch {
            node,
            wakes: rng.range(0.0, SPREAD_SECS),
        })
        .collect();
    let out = Spread {
        peers: &mut peers,
        rules: &mut rules,
        rng: &mut rng,
        creator,
        ask_delay: ASK_DELAY,
        tell_delay: TELL_DELAY,
        bytes: VIDEO_BYTES,
        until: 3600.0,
    }
    .run(&mut swarm, &fetches, &[]);
    let followers = &out.held[1..=FOLLOWERS];
    let mut creator_bytes = out.served[creator];
    let mut relayed = out.relayed;
    let (mut busy, mut failed) = (out.busy, out.failed);
    // Latecomers, one after another, each fetching at once.
    let mut holding: Vec<usize> = (1..=FOLLOWERS).filter(|&i| out.held[i].is_some()).collect();
    let mut latecomers = Vec::new();
    for node in FOLLOWERS + 1..n {
        let late = Spread {
            peers: &mut peers,
            rules: &mut rules,
            rng: &mut rng,
            creator,
            ask_delay: ASK_DELAY,
            tell_delay: TELL_DELAY,
            bytes: VIDEO_BYTES,
            until: 3600.0,
        }
        .run(&mut swarm, &[Fetch { node, wakes: 0.0 }], &holding);
        creator_bytes += late.served[creator];
        relayed += late.relayed;
        busy += late.busy;
        failed += late.failed;
        if let Some(at) = late.held[node] {
            latecomers.push(at);
            holding.push(node);
        }
    }
    let mut run = Run {
        percent,
        p50: time_for(followers, 0.5),
        p95: time_for(followers, 0.95),
        all: time_for(followers, 1.0),
        creator_copies: creator_bytes / VIDEO_BYTES,
        relay_copies: relayed / VIDEO_BYTES,
        followers_extra: out.wasted / (out.delivered.max(1.0)),
        busy,
        failed,
        latecomers,
        mark: None,
    };
    if let Some(m) = MEASURED.iter().find(|m| m.percent == percent) {
        let p95_ok = run
            .p95
            .is_some_and(|p| (p - m.p95).abs() <= P95_WITHIN * m.p95);
        let relay_ok =
            (run.relay_copies - m.relay_copies).abs() <= RELAY_WITHIN * m.relay_copies.max(1.0);
        let creator_ok = (CREATOR_COPIES.0..=CREATOR_COPIES.1).contains(&run.creator_copies);
        run.mark = Some(p95_ok && relay_ok && creator_ok);
    }
    run
}
