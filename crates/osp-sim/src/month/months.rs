//! Three months of Kim's posts ([plan]): who's in each start, when each
//! post goes out, and what each post cost and took.
//!
//! Built a step at a time. So far: way 1 (relays alone) under the rules as
//! accepted, which needs no records, as nothing earns anything there.
//!
//! [plan]: ../../../docs/testing/hops-model.md

use serde::Serialize;

use super::calibrate::{ASK_DELAY, DOWN, SPREAD_SECS, TELL_DELAY, UP};
use super::rng::Rng;
use super::spread::{Fetch, Leg, Load, Part, Peer, Rules, Sources, Spread, Swarm, VIDEO_BYTES};

const DAY: f64 = 86_400.0;
/// 45 posts, one every 2 days.
pub const POSTS: usize = 45;
const POST_EVERY_DAYS: f64 = 2.0;
/// A post that hasn't spread this long after it is given up on.
const POST_UNTIL: f64 = 2.0 * 3600.0;

/// B8's marks for each post (docs/testing/network-conditions.md).
const P95_WITHIN: f64 = 15.0 * 60.0;
const ALL_WITHIN: f64 = 30.0 * 60.0;
const CREATOR_SHARE: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
pub enum Start {
    /// From zero standing: 300 followers, two groups of 50 newcomers, and
    /// the operator's 30 nodes.
    Z,
    /// From 10 approved by hand, and 100 followers arriving over 4 weeks.
    S,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
pub enum Way {
    /// 1: media through the relay, as today.
    Relays,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Kind {
    Creator,
    Follower,
    /// Approved by hand to one another (S); leaves on day 45.
    Approved,
    Operator,
    /// Told only of a directory (Z, days 35 to 49).
    NewcomerDirectory,
    /// Told of one established machine, the directory off (Z, days 57 to 63).
    NewcomerGiven,
}

#[derive(Clone, Debug)]
pub struct Member {
    pub kind: Kind,
    /// Seconds from the start: follows Kim from then, and leaves then.
    pub arrives: f64,
    pub leaves: Option<f64>,
}

impl Member {
    fn present(&self, t: f64) -> bool {
        self.arrives <= t && self.leaves.is_none_or(|l| t < l)
    }
}

/// Kim is node 0; the others in the order they follow her.
pub fn population(start: Start) -> (Vec<Member>, Vec<Peer>) {
    let mut members = vec![Member {
        kind: Kind::Creator,
        arrives: 0.0,
        leaves: None,
    }];
    let mut peers = vec![Peer::new(20.0, 100.0)];
    let mut add = |kind: Kind, arrives_day: f64, leaves_day: Option<f64>| {
        let i = members.len() - 1;
        let (up, down) = if kind == Kind::Operator {
            (100.0, 1000.0)
        } else {
            (UP[i % 3], DOWN[i % 3])
        };
        members.push(Member {
            kind,
            arrives: arrives_day * DAY,
            leaves: leaves_day.map(|d| d * DAY),
        });
        peers.push(Peer::new(up, down));
    };
    // `n` arriving evenly over the days from `first` to `last`.
    let evenly =
        |k: usize, n: usize, first: f64, last: f64| first + (last - first) * k as f64 / n as f64;
    match start {
        Start::Z => {
            for _ in 0..300 {
                add(Kind::Follower, 0.0, None);
            }
            for _ in 0..30 {
                add(Kind::Operator, 0.0, None);
            }
            for k in 0..50 {
                add(Kind::NewcomerDirectory, evenly(k, 50, 35.0, 49.0), None);
            }
            for k in 0..50 {
                add(Kind::NewcomerGiven, evenly(k, 50, 57.0, 63.0), None);
            }
        }
        Start::S => {
            for _ in 0..10 {
                add(Kind::Approved, 0.0, Some(45.0));
            }
            for k in 0..100 {
                add(Kind::Follower, evenly(k, 100, 0.0, 28.0), None);
            }
        }
    }
    (members, peers)
}

/// Way 1 under the rules as accepted: sources as Kim's node names them,
/// every leg through the relay.
struct RelaysAlone {
    creator: usize,
}

impl Rules for RelaysAlone {
    fn sources(&mut self, to: usize, swarm: &Swarm, rng: &mut Rng) -> Sources {
        Sources {
            at_once: swarm.named(to, rng),
            in_turn: vec![self.creator],
        }
    }

    fn path(&mut self, source: usize, _to: usize, _load: &Load, _rng: &mut Rng) -> Vec<Leg> {
        vec![Leg {
            from: source,
            relayed: true,
            carried: false,
        }]
    }

    fn part_done(&mut self, _part: &Part, _now: f64) {}
    fn holds(&mut self, _node: usize, _now: f64) {}
}

#[derive(Debug, Serialize)]
pub struct Post {
    pub index: usize,
    /// Seconds from the start.
    pub at: f64,
    /// Followers present, Kim not counted.
    pub present: usize,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub all: Option<f64>,
    pub creator_copies: f64,
    /// Bytes on the wire through the relay.
    pub relayed: f64,
    pub delivered: f64,
    pub delivered_off_relay: f64,
    pub delivered_through_hops: f64,
    pub busy: usize,
    pub failed: usize,
    /// B8's three marks for this post.
    pub marks: bool,
}

#[derive(Debug, Serialize)]
pub struct Months {
    pub start: Start,
    pub way: Way,
    pub seed: u64,
    pub posts: Vec<Post>,
}

pub fn run(start: Start, way: Way, seed: u64) -> Months {
    let mut rng = Rng::new(seed);
    let (members, mut peers) = population(start);
    let creator = 0;
    let mut rules = match way {
        Way::Relays => RelaysAlone { creator },
    };
    let mut posts = Vec::new();
    for index in 0..POSTS {
        let at = (index as f64 * POST_EVERY_DAYS) * DAY + rng.range(0.0, DAY);
        let present: Vec<usize> = (1..members.len())
            .filter(|&i| members[i].present(at))
            .collect();
        // Kim's node notes each copy anew for each post; who has followed
        // longest is the order they followed. Under way 1 nobody is reached
        // directly.
        let mut swarm = Swarm::new(peers.len());
        let fetches: Vec<Fetch> = present
            .iter()
            .map(|&node| Fetch {
                node,
                wakes: if members[node].kind == Kind::Operator {
                    0.0
                } else {
                    rng.range(0.0, SPREAD_SECS)
                },
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
            until: POST_UNTIL,
        }
        .run(&mut swarm, &fetches, &[]);
        let held: Vec<Option<f64>> = present.iter().map(|&i| out.held[i]).collect();
        let p95 = super::calibrate::time_for(&held, 0.95);
        let all = super::calibrate::time_for(&held, 1.0);
        let creator_copies = out.served[creator] / VIDEO_BYTES;
        let marks = p95.is_some_and(|t| t <= P95_WITHIN)
            && all.is_some_and(|t| t <= ALL_WITHIN)
            && creator_copies <= CREATOR_SHARE * present.len() as f64;
        posts.push(Post {
            index,
            at,
            present: present.len(),
            p50: super::calibrate::time_for(&held, 0.5),
            p95,
            all,
            creator_copies,
            relayed: out.relayed,
            delivered: out.delivered,
            delivered_off_relay: out.delivered_off_relay,
            delivered_through_hops: out.delivered_through_hops,
            busy: out.busy,
            failed: out.failed,
            marks,
        });
    }
    Months {
        start,
        way,
        seed,
        posts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present(start: Start, day: f64) -> usize {
        let (members, _) = population(start);
        members[1..].iter().filter(|m| m.present(day * DAY)).count()
    }

    #[test]
    fn who_is_there_on_which_day() {
        // Z: 300 followers and the operator's 30, then 50 and 50 newcomers.
        assert_eq!(present(Start::Z, 0.0), 330);
        assert_eq!(present(Start::Z, 50.0), 380);
        assert_eq!(present(Start::Z, 64.0), 430);
        // S: the 10 and the first arrival, all 110 by day 28, the 10 gone on 45.
        assert_eq!(present(Start::S, 0.0), 11);
        assert_eq!(present(Start::S, 28.0), 110);
        assert_eq!(present(Start::S, 45.0), 100);
    }
}
