//! ADR 0025's step 5 ([plan]): strangers find creators' videos through a
//! directory over weeks, and some share them with hosting. No nodes run.
//! Each video spreads on its own through `spread`, as in the hops model, by
//! the rules as built: Alice takes the hosts the directory lists, the first
//! 3 at once, then those Kim's media endpoint names, and Kim's own last.
//! She shuffles each group first (ADR 0025, amended after this model's
//! first run); with `same_order`, she takes them as given, as before.
//!
//! [plan]: ../../../docs/testing/found-over-weeks.md

use serde::Serialize;

use super::{
    calibrate::{ASK_DELAY, DOWN, TELL_DELAY, UP},
    rng::Rng,
    spread::{Fetch, Leg, Load, Part, Peer, Rules, Sources, Spread, Swarm, VIDEO_BYTES},
};

pub const CREATORS: usize = 10;
pub const NEWCOMERS: usize = 1000;
/// Newcomers arrive evenly over this many days.
pub const DAYS: f64 = 28.0;
/// Videos each newcomer opens, picked by popularity.
pub const OPENS: usize = 3;
/// A video's first viewers, before resharers are judged.
pub const FIRST_VIEWERS: usize = 50;
/// Hosting reshares of a post a directory lists; as built, more are left out.
const LISTED: usize = 16;
/// Hosting resharers a creator's media endpoint names (`RESHARERS_NAMED`).
const NAMED: usize = 6;
const DAY: f64 = 86_400.0;
const HOUR: f64 = 3_600.0;
/// Seconds after a sharer holds its copy until its creator's node names it:
/// told at once, it checks a piece within seconds.
const NAMING_LAG: f64 = 10.0;

/// What each video's viewers got, and from whom.
#[derive(Debug, Serialize)]
pub struct Video {
    /// 1 is the most popular.
    pub rank: usize,
    pub viewers: usize,
    pub sharers: usize,
    /// The share of what viewers received that came from resharers: its
    /// first 50 viewers, and those after them.
    pub from_resharers_first: f64,
    pub from_resharers_after: Option<f64>,
    /// The creator's copies for each viewer, for the same two groups.
    pub creator_per_viewer_first: f64,
    pub creator_per_viewer_after: Option<f64>,
    pub creator_copies: f64,
    /// The most any one resharer served, as a share of all its viewers got,
    /// and what the 3 that served most did together.
    pub top_resharer: f64,
    pub top_three: f64,
    /// Seconds from asking for sources to holding it.
    pub open_median: Option<f64>,
    pub open_slowest: Option<f64>,
    pub failed: usize,
}

#[derive(Debug, Serialize)]
pub struct Run {
    pub share_percent: f64,
    pub same_order: bool,
    pub seed: u64,
    pub videos: Vec<Video>,
    pub marks: Marks,
}

/// The plan's pass marks, for this run (judged only at 10%).
#[derive(Debug, Serialize)]
pub struct Marks {
    /// 1: for every video, at least half of what viewers after its 50th got
    /// came from resharers.
    pub resharers_carry: bool,
    /// 2: for every video, the creator's copies per viewer after its 50th
    /// are at most half those for its first 50.
    pub creator_cost_falls: bool,
    /// 3: every open succeeds.
    pub everyone_watches: bool,
    /// ADR 0025's revisit condition: one resharer served more than half of
    /// a video's bytes.
    pub one_resharer_most: bool,
}

/// The directory and the creator's node, for one video, and what each
/// viewer got from whom.
struct Found {
    creator: usize,
    same_order: bool,
    /// Whether each node shares this video with hosting once it holds it.
    shares: Vec<bool>,
    /// Each node's wait from holding its copy to being listed: it asks
    /// within the hour, and the directory checks it at its next hourly read.
    listing_lag: Vec<f64>,
    /// When each sharer was listed, and named by the creator's node.
    listed: Vec<(f64, usize)>,
    named: Vec<(f64, usize)>,
    /// For the creator's node's daily draw.
    draw: u64,
    from_creator: Vec<f64>,
    from_resharers: Vec<f64>,
    served: Vec<f64>,
}

/// A number drawn from `key`, `day` and `node`, the same for all three: the
/// creator's node sorts those it names by one, as by a hash.
fn daily(key: u64, day: u64, node: usize) -> u64 {
    Rng::new(
        key ^ day.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ (node as u64).wrapping_mul(0x9E37_79B9),
    )
    .next_u64()
}

impl Rules for Found {
    fn sources(&mut self, to: usize, _swarm: &Swarm, now: f64, rng: &mut Rng) -> Sources {
        // Listed in the order they were, up to the directory's limit.
        let listed: Vec<usize> = self
            .listed
            .iter()
            .filter(|&&(at, _)| at <= now)
            .take(LISTED)
            .map(|&(_, n)| n)
            .filter(|&n| n != to)
            .collect();
        let day = (now / DAY) as u64;
        let mut named: Vec<usize> = self
            .named
            .iter()
            .filter(|&&(at, n)| at <= now && n != to)
            .map(|&(_, n)| n)
            .collect();
        named.sort_by_key(|&n| daily(self.draw, day, n));
        named.truncate(NAMED);
        let mut listed = listed;
        if !self.same_order {
            rng.shuffle(&mut listed);
            rng.shuffle(&mut named);
        }
        if listed.is_empty() {
            Sources {
                at_once: named,
                in_turn: vec![self.creator],
            }
        } else {
            let mut in_turn: Vec<usize> =
                named.into_iter().filter(|n| !listed.contains(n)).collect();
            in_turn.push(self.creator);
            Sources {
                at_once: listed,
                in_turn,
            }
        }
    }

    fn path(&mut self, source: usize, _to: usize, _load: &Load, _rng: &mut Rng) -> Vec<Leg> {
        // Viewing and media endpoints are relay-only.
        vec![Leg {
            from: source,
            relayed: true,
            carried: false,
        }]
    }

    fn part_done(&mut self, part: &Part, _now: f64) {
        if part.source == self.creator {
            self.from_creator[part.to] += part.bytes;
        } else {
            self.from_resharers[part.to] += part.bytes;
        }
        self.served[part.source] += part.bytes;
    }

    fn holds(&mut self, node: usize, now: f64) {
        if self.shares[node] {
            self.listed.push((now + self.listing_lag[node], node));
            self.listed.sort_by(|a, b| a.0.total_cmp(&b.0));
            self.named.push((now + NAMING_LAG, node));
        }
    }
}

/// Picks `OPENS` of the videos, the one ranked k in proportion to 1/k.
fn pick_videos(rng: &mut Rng) -> Vec<usize> {
    let mut left: Vec<usize> = (0..CREATORS).collect();
    let mut picked = Vec::new();
    while picked.len() < OPENS {
        let total: f64 = left.iter().map(|&v| 1.0 / (v + 1) as f64).sum();
        let mut x = rng.f64() * total;
        let mut i = left.len() - 1;
        for (j, &v) in left.iter().enumerate() {
            x -= 1.0 / (v + 1) as f64;
            if x < 0.0 {
                i = j;
                break;
            }
        }
        picked.push(left.remove(i));
    }
    picked
}

fn share(part: f64, whole: f64) -> f64 {
    if whole > 0.0 { part / whole } else { 0.0 }
}

pub fn run(share_percent: f64, same_order: bool, seed: u64) -> Run {
    let mut rng = Rng::new(seed);
    // Node 0 is the video's creator; newcomer i is node i + 1.
    let n = NEWCOMERS + 1;
    let mut viewers: Vec<Vec<(f64, usize)>> = vec![Vec::new(); CREATORS];
    let mut shares = vec![vec![false; n]; CREATORS];
    for i in 0..NEWCOMERS {
        let arrives = (i as f64 + 0.5) / NEWCOMERS as f64 * DAYS * DAY;
        for v in pick_videos(&mut rng) {
            viewers[v].push((arrives, i + 1));
            shares[v][i + 1] = rng.chance(share_percent / 100.0);
        }
    }
    let videos: Vec<Video> = (0..CREATORS)
        .map(|v| {
            let mut peers = vec![Peer::new(20.0, 100.0)];
            peers.extend((0..NEWCOMERS).map(|i| Peer::new(UP[i % 3], DOWN[i % 3])));
            let mut rules = Found {
                creator: 0,
                same_order,
                shares: std::mem::take(&mut shares[v]),
                listing_lag: (0..n)
                    .map(|_| rng.range(0.0, HOUR) + rng.range(0.0, HOUR))
                    .collect(),
                listed: Vec::new(),
                named: Vec::new(),
                draw: rng.next_u64(),
                from_creator: vec![0.0; n],
                from_resharers: vec![0.0; n],
                served: vec![0.0; n],
            };
            let fetches: Vec<Fetch> = viewers[v]
                .iter()
                .map(|&(wakes, node)| Fetch { node, wakes })
                .collect();
            let mut swarm = Swarm::new(n);
            let mut video_rng = Rng::new(rng.next_u64());
            let out = Spread {
                peers: &mut peers,
                rules: &mut rules,
                rng: &mut video_rng,
                creator: 0,
                ask_delay: ASK_DELAY,
                tell_delay: TELL_DELAY,
                bytes: VIDEO_BYTES,
                until: (DAYS + 1.0) * DAY,
            }
            .run(&mut swarm, &fetches, &[]);
            let order: Vec<usize> = viewers[v].iter().map(|&(_, node)| node).collect();
            let (first, after) = order.split_at(FIRST_VIEWERS.min(order.len()));
            let sum = |group: &[usize], of: &[f64]| group.iter().map(|&n| of[n]).sum::<f64>();
            let from = |group: &[usize]| {
                let r = sum(group, &rules.from_resharers);
                share(r, r + sum(group, &rules.from_creator))
            };
            let per_viewer = |group: &[usize]| {
                sum(group, &rules.from_creator) / VIDEO_BYTES / group.len() as f64
            };
            let all = sum(&order, &rules.from_resharers) + sum(&order, &rules.from_creator);
            let mut served: Vec<f64> = rules.served[1..].to_vec();
            served.sort_by(|a, b| b.total_cmp(a));
            let top = served.first().copied().unwrap_or(0.0);
            let top_three: f64 = served.iter().take(3).sum();
            let mut opens: Vec<f64> = viewers[v]
                .iter()
                .filter_map(|&(wakes, node)| out.held[node].map(|t| t - wakes))
                .collect();
            opens.sort_by(f64::total_cmp);
            Video {
                rank: v + 1,
                viewers: order.len(),
                sharers: order.iter().filter(|&&n| rules.shares[n]).count(),
                from_resharers_first: from(first),
                from_resharers_after: (!after.is_empty()).then(|| from(after)),
                creator_per_viewer_first: per_viewer(first),
                creator_per_viewer_after: (!after.is_empty()).then(|| per_viewer(after)),
                creator_copies: sum(&order, &rules.from_creator) / VIDEO_BYTES,
                top_resharer: share(top, all),
                top_three: share(top_three, all),
                open_median: opens.get(opens.len() / 2).copied(),
                open_slowest: opens.last().copied(),
                failed: order.len() - opens.len(),
            }
        })
        .collect();
    let marks = Marks {
        resharers_carry: videos
            .iter()
            .all(|v| v.from_resharers_after.is_some_and(|s| s >= 0.5)),
        creator_cost_falls: videos.iter().all(|v| {
            v.creator_per_viewer_after
                .is_some_and(|a| a <= 0.5 * v.creator_per_viewer_first)
        }),
        everyone_watches: videos.iter().all(|v| v.failed == 0),
        one_resharer_most: videos.iter().any(|v| v.top_resharer > 0.5),
    };
    Run {
        share_percent,
        same_order,
        seed,
        videos,
        marks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popular_videos_get_more_viewers_and_each_newcomer_opens_three() {
        let mut rng = Rng::new(1);
        let mut counts = [0usize; CREATORS];
        for _ in 0..NEWCOMERS {
            let picked = pick_videos(&mut rng);
            assert_eq!(picked.len(), OPENS);
            let mut unique = picked.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), OPENS);
            for v in picked {
                counts[v] += 1;
            }
        }
        assert!(counts[0] > 4 * counts[CREATORS - 1], "{counts:?}");
        assert!(counts.iter().all(|&c| c > FIRST_VIEWERS), "{counts:?}");
    }

    #[test]
    fn with_nobody_sharing_the_creator_sends_every_copy() {
        let run = run(0.0, false, 1);
        for v in &run.videos {
            assert_eq!(v.sharers, 0);
            assert_eq!(v.failed, 0);
            assert!((v.creator_copies - v.viewers as f64).abs() < 0.01, "{v:?}");
            assert_eq!(v.from_resharers_after, Some(0.0));
        }
    }
}
