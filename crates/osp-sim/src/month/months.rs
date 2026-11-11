//! Three months of Kim's posts ([plan]): who's in each start, when each
//! post goes out, and what each post cost and took.
//!
//! All three ways, under both sets of rules, with meeting, lists and
//! introductions earned (`meeting`).
//!
//! [plan]: ../../../docs/testing/hops-model.md

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use super::calibrate::{ASK_DELAY, DOWN, SPREAD_SECS, TELL_DELAY, UP, time_for};
use super::meeting::{Asking, MeetCounts, Meeting, Standing, Who};
use super::records::{Block, BytesRule, DayRule, Records};
use super::rng::Rng;
use super::spread::{
    Fetch, Leg, Load, OVERHEAD, Part, Peer, Rules, SWARM_SAMPLE, Sources, Spread, Swarm,
    VIDEO_BYTES, transfer_places,
};

use super::records::DAY;
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
    /// 2: directly between two nodes that each earned it (ADR 0018),
    /// otherwise through the relay.
    Earned,
    /// 3: as 2; otherwise through hops where an end has one (ADR 0017),
    /// any leg without one through the relay.
    Hops,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
pub enum RuleSet {
    /// Followers fetch from those Kim's node names at random.
    Accepted,
    /// ADR 0023: partners first, and Kim's node names first those whose
    /// copies passed her spot checks on the most days.
    Partners,
}

/// Experiments A to C (docs/testing/hops-experiments.md), each a candidate
/// revision measured against the baseline, the model's second run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
pub enum Experiment {
    Baseline,
    /// One-way service toward a direct link, and the machine-to-node binding.
    A,
    /// A share of relayed transfers given to machines with no standing.
    B5,
    B10,
    B20,
    /// B10, with 1% or 5% of B's jobs failing part way.
    #[value(name = "b10-f1")]
    #[serde(rename = "b10-f1")]
    B10F1,
    #[value(name = "b10-f5")]
    #[serde(rename = "b10-f5")]
    B10F5,
    /// Kim's node gives each post first to up to 4 followers.
    C,
    Abc5,
    Abc10,
    Abc20,
}

/// What an experiment changes.
#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    one_way: bool,
    /// B: the share of eligible transfers given to a machine with no
    /// standing, and the share of those jobs that fail.
    b_share: f64,
    b_fail: f64,
    kim_first: bool,
}

impl Experiment {
    fn settings(self) -> Settings {
        use Experiment::*;
        let b = |b_share: f64| Settings {
            b_share,
            ..Default::default()
        };
        let abc = |b_share: f64| Settings {
            one_way: true,
            b_share,
            b_fail: 0.0,
            kim_first: true,
        };
        match self {
            Baseline => Settings::default(),
            A => Settings {
                one_way: true,
                ..Default::default()
            },
            B5 => b(0.05),
            B10 => b(0.10),
            B20 => b(0.20),
            B10F1 => Settings {
                b_fail: 0.01,
                ..b(0.10)
            },
            B10F5 => Settings {
                b_fail: 0.05,
                ..b(0.10)
            },
            C => Settings {
                kim_first: true,
                ..Default::default()
            },
            Abc5 => abc(0.05),
            Abc10 => abc(0.10),
            Abc20 => abc(0.20),
        }
    }
}

/// B: jobs one machine takes a day (a starting value).
const B_JOBS_A_DAY: usize = 2;
/// C: followers Kim's node gives each post to first.
const KIM_FIRST: usize = 4;
/// Days followed for the daily figures.
const RUN_DAYS: usize = 100;

/// ADR 0023's starting values.
const MAX_PARTNERS: usize = 4;
const PARTNERS_AT_ONCE: usize = 2;
/// A partner whose faded work falls below this drops out.
const PARTNER_FLOOR: f64 = 1e6;
const NAMED_BY_CHECKS: usize = 3;
const HALF_LIFE: f64 = 14.0 * DAY;

/// One node's partners for Kim's posts: the peer, the bytes swapped with
/// it either way, faded, and when that was last brought up to date.
#[derive(Clone, Debug, Default)]
struct Partners(Vec<(usize, f64, f64)>);

impl Partners {
    fn fade_to(&mut self, t: f64) {
        for (_, score, at) in &mut self.0 {
            if t > *at {
                *score *= 0.5f64.powf((t - *at) / HALF_LIFE);
                *at = t;
            }
        }
        self.0.retain(|&(_, score, _)| score >= PARTNER_FLOOR);
        self.0.sort_by(|a, b| b.1.total_cmp(&a.1));
    }

    fn swapped(&mut self, peer: usize, bytes: f64, t: f64) {
        self.fade_to(t);
        if let Some(p) = self.0.iter_mut().find(|p| p.0 == peer) {
            p.1 += bytes;
        } else if self.0.len() < MAX_PARTNERS {
            self.0.push((peer, bytes, t));
        } else if self.0.last().is_some_and(|l| l.1 < bytes) {
            // Keeping those with the most work done.
            *self.0.last_mut().unwrap() = (peer, bytes, t);
        }
        self.0.sort_by(|a, b| b.1.total_cmp(&a.1));
    }

    fn has(&self, peer: usize) -> bool {
        self.0.iter().any(|p| p.0 == peer)
    }
}

/// What decides each transfer's path, and what the nodes write down.
struct Media {
    way: Way,
    rule_set: RuleSet,
    creator: usize,
    /// Seconds from the start of the post spreading now.
    post_at: f64,
    records: Records,
    partners: Vec<Partners>,
    /// Bytes delivered by a partner, and by an operator's node, this post.
    from_partners: f64,
    from_operator: f64,
    operator: Vec<bool>,
    /// Way 3: who says yes to carrying, and is there now.
    carries: Vec<bool>,
    present: Vec<bool>,
    /// Jobs each machine finished for each viewer as a hop, on which days:
    /// layer 2 here is jobs on 3 days in the last 30, ADR 0017's bar for a
    /// viewer's hop.
    carried: Records,
    /// The hop next to each viewer's record of the jobs it carried for that
    /// viewer: what the hop itself knows of whom it worked for.
    near_viewer: Records,
    hop_rng: Rng,
    /// When each machine first got a middle turn.
    first_middle: Vec<Option<f64>>,
    hops: HopCounts,
    /// Machines meeting machines: each viewer picks its middle machine from
    /// its own table.
    meeting: Meeting,
    /// When each node first served a copy.
    first_served: Vec<Option<f64>>,
    /// (a, b) with a < b: bytes a served b, and b served a, over the run.
    swapped: HashMap<(usize, usize), [f64; 2]>,
    /// Bytes of the file the operator's nodes received.
    operator_down: f64,
    /// The experiment's settings, and what it follows.
    exp: Settings,
    newcomer: Vec<bool>,
    /// B: picks, and which jobs fail, apart from every other draw.
    b_rng: Rng,
    fail_rng: Rng,
    /// The path just picked is one of B's jobs.
    last_b: bool,
    /// B's jobs each machine took each day.
    b_day_jobs: HashMap<(usize, usize), usize>,
    /// Days each machine turned a job away under the daily limit.
    b_limit_days: HashMap<usize, BTreeSet<usize>>,
    /// A machine's own record of the viewers it did B's jobs for: the viewer
    /// gave it the job, through the relay, so it knows the viewer's machine,
    /// not its address.
    b_known: Records,
    /// Days each machine finished jobs for each viewer, any jobs: (m, v).
    job_days: HashMap<(usize, usize), BTreeSet<usize>>,
    /// The first job each machine got: a middle turn or one of B's.
    first_job: Vec<Option<f64>>,
    b_jobs_of: Vec<usize>,
    /// Followers Kim's node gives this post to first (C).
    first_copies: Vec<bool>,
    /// Addresses learned: by any other machine, by the operator's.
    exposed_any: Vec<bool>,
    exposed_operator: Vec<bool>,
    extra: Extra,
    /// Downloads whose transfer had a failed job this post.
    failed_to: Vec<usize>,
}

/// The figures the experiments' plan records, beyond each post's.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Extra {
    /// B, each day: transfers picked for a job, jobs taken, jobs turned
    /// away by the daily limit (the rest found no machine).
    pub b_offered: Vec<usize>,
    pub b_taken: Vec<usize>,
    pub b_turned_away: Vec<usize>,
    /// B's bytes on the wire: the second leg through the relay, which is
    /// also the relay's standby capacity for the fallback, never counted as
    /// media delivered; and the upload of the machines doing the jobs, of
    /// newcomers among them.
    pub b_relay_extra: f64,
    pub b_standby: f64,
    pub b_upload: f64,
    pub b_upload_newcomers: f64,
    /// B's failures: jobs picked to fail, jobs that did (one picked can end
    /// first), those whose download completed anyway, the bytes on the wire
    /// they cost beyond a job that didn't fail. A failed job's machine is
    /// credited nothing (`part_done`).
    pub b_fail_picked: usize,
    pub b_failed: usize,
    pub b_failed_completed: usize,
    pub b_fail_extra: f64,
    /// Among honest followers: the share of bytes served by the 10% that
    /// served most, and the Gini of bytes served.
    pub top10_share: f64,
    pub gini: f64,
    /// Honest nodes (Kim among them) whose address another machine learned,
    /// through a direct link or as the hop next to it, and the operator's.
    pub honest: usize,
    pub exposed_any: usize,
    pub exposed_operator: usize,
    /// The operator's share of the entries in honest machines' tables at the
    /// end, and of Kim's picks (C).
    pub operator_table_share: f64,
    pub kim_picks: usize,
    pub kim_picks_operator: usize,
    /// Earned links (follower pairs, approved ones left out): distinct pairs
    /// ever linked, by whether both sides had work both ways when first
    /// seen; and at the end.
    pub links_both_ways: usize,
    pub links_one_way: usize,
    pub links_end_both_ways: usize,
    pub links_end_one_way: usize,
    /// A's weekly bar: servers active with a node (3 days or more in 30),
    /// and of those, the ones that miss it, on day 60 and at the end.
    pub active_day60: usize,
    pub missing_weekly_day60: usize,
    pub active_end: usize,
    pub missing_weekly_end: usize,
}

/// Transfers through hops, for mark 9: how often the operator's nodes held
/// the hop next to both ends.
#[derive(Clone, Debug, Default, Serialize)]
pub struct HopCounts {
    /// Transfers with a hop at either end, and at both (a single hop that
    /// both ends picked counts as both).
    pub with_hops: usize,
    pub both_ends: usize,
    /// Hops next to an end, and of those, the operator's.
    pub end_hops: usize,
    pub end_hops_operator: usize,
    /// Transfers with a hop at both ends, all of them the operator's.
    pub operator_both_ends: usize,
    pub middle_turns: usize,
    /// Way 3's transfers not over a direct link, by which ends had a hop
    /// they could pick: both, the host's only, the viewer's only, neither.
    /// Also those from Kim's node, apart.
    pub available: [usize; 4],
    pub available_from_creator: [usize; 4],
}

/// ADR 0017's starting value: a middle turn on 1 transfer in 10 through hops.
const MIDDLE_EVERY: f64 = 0.1;
/// ADR 0023: half the middle turns go to machines that finished one before.
const MIDDLE_RETURNING: f64 = 0.5;
/// Honest nodes saying yes to carrying (a starting value).
const YES_TO_CARRYING: f64 = 0.75;
/// A node carries within this share of its upload.
const CARRY_SHARE: f64 = 0.5;

impl Media {
    /// Whether `m` can carry for others now.
    fn can_hop(&self, m: usize, load: &Load) -> bool {
        self.carries[m] && self.present[m] && load.can_carry(m)
    }

    /// The hops the viewer `v` may pick next to it: its direct peers, and
    /// machines that finished jobs for it on 3 days, with room, if it said
    /// yes.
    fn viewer_hops(&self, v: usize, not: usize, load: &Load) -> Vec<usize> {
        if !self.records.yes[v] {
            return Vec::new();
        }
        let mut c = self.records.direct_peers(v);
        c.extend(self.carried.known_to(v, 2));
        if self.exp.one_way {
            // The binding counts a machine's downloads from the first.
            c.extend(self.records.known_to(v, 2));
        }
        c.sort_unstable();
        c.dedup();
        c.retain(|&m| m != not && m != v && self.can_hop(m, load));
        c
    }

    /// The hops the host `h` may name: its direct peers, with room.
    fn host_hops(&self, h: usize, not: usize, load: &Load) -> Vec<usize> {
        if !self.records.yes[h] {
            return Vec::new();
        }
        let mut c = self.records.direct_peers(h);
        c.retain(|&m| m != not && m != h && self.can_hop(m, load));
        c
    }

    /// `m` has standing with `v`: jobs for it on 3 days, or under A, as the
    /// binding counts them from the first, downloads on 3 days.
    fn has_standing(&self, v: usize, m: usize) -> bool {
        self.carried.get(v, m).is_some_and(|r| r.layer >= 2)
            || (self.exp.one_way && self.records.get(v, m).is_some_and(|r| r.layer >= 2))
    }

    /// A machine with no standing with `v`, for the middle, from `v`'s
    /// table. One whose introduction `v` counts goes first (ADR 0022).
    fn middle(&mut self, v: usize, taken: &[usize], load: &Load) -> Option<usize> {
        let mut rng = std::mem::replace(&mut self.hop_rng, Rng::new(0));
        let m = self.no_standing(&mut rng, v, taken, load);
        self.hop_rng = rng;
        m
    }

    /// Picks a machine with no standing with `v` from its table, by the rule
    /// set's rule for middle turns.
    fn no_standing(&self, rng: &mut Rng, v: usize, taken: &[usize], load: &Load) -> Option<usize> {
        let pool: Vec<usize> = self
            .meeting
            .table(v)
            .filter(|&m| {
                !taken.contains(&m)
                    && self.can_hop(m, load)
                    && !self.has_standing(v, m)
                    && !self.records.direct(v, m)
            })
            .collect();
        if pool.is_empty() {
            return None;
        }
        let t = self.post_at;
        let introduced: Vec<usize> = pool
            .iter()
            .copied()
            .filter(|&m| self.meeting.introduced(v, m, t))
            .collect();
        if !introduced.is_empty() {
            return Some(introduced[rng.below(introduced.len())]);
        }
        if self.rule_set == RuleSet::Partners && rng.chance(MIDDLE_RETURNING) {
            // Those that finished one for `v` before, most jobs first.
            let jobs = |m: usize| self.carried.get(v, m).map_or(0.0, |r| r.work_days);
            let mut back: Vec<usize> = pool.iter().copied().filter(|&m| jobs(m) > 0.0).collect();
            if !back.is_empty() {
                rng.shuffle(&mut back);
                back.sort_by(|&a, &b| jobs(b).total_cmp(&jobs(a)));
                return Some(back[0]);
            }
        }
        let fresh: Vec<usize> = match self.rule_set {
            RuleSet::Accepted => pool,
            RuleSet::Partners => {
                let f: Vec<usize> = pool
                    .iter()
                    .copied()
                    .filter(|&m| self.carried.get(v, m).is_none())
                    .collect();
                if f.is_empty() { pool } else { f }
            }
        };
        Some(fresh[rng.below(fresh.len())])
    }

    /// B: a job for a machine with no standing with `v`, on a transfer from
    /// `h` through the relay. The machine turns it away over its daily limit.
    fn b_job(&mut self, h: usize, v: usize, load: &Load) -> Option<usize> {
        let t = self.post_at + load.now;
        let day = ((t / DAY) as usize).min(RUN_DAYS - 1);
        self.extra.b_offered[day] += 1;
        let mut rng = std::mem::replace(&mut self.b_rng, Rng::new(0));
        let m = self.no_standing(&mut rng, v, &[h, v], load);
        self.b_rng = rng;
        let m = m?;
        let jobs = self.b_day_jobs.entry((m, day)).or_default();
        if *jobs >= B_JOBS_A_DAY {
            self.extra.b_turned_away[day] += 1;
            self.b_limit_days.entry(m).or_default().insert(day);
            return None;
        }
        *jobs += 1;
        self.extra.b_taken[day] += 1;
        self.b_jobs_of[m] += 1;
        self.first_job[m].get_or_insert(t);
        Some(m)
    }

    /// `a` and `b` learned each other's address.
    fn expose(&mut self, a: usize, b: usize) {
        self.exposed_any[a] = true;
        self.exposed_any[b] = true;
        self.exposed_operator[b] |= self.operator[a];
        self.exposed_operator[a] |= self.operator[b];
    }

    /// Way 3's path from host `h` to viewer `v`: H → Q → (M) → P → V.
    fn hop_path(&mut self, h: usize, v: usize, load: &Load) -> Vec<Leg> {
        let t = self.post_at;
        let ps = self.viewer_hops(v, h, load);
        let qs = self.host_hops(h, v, load);
        let k = match (qs.is_empty(), ps.is_empty()) {
            (false, false) => 0,
            (false, true) => 1,
            (true, false) => 2,
            (true, true) => 3,
        };
        self.hops.available[k] += 1;
        if h == self.creator {
            self.hops.available_from_creator[k] += 1;
        }
        let pick = |rng: &mut Rng, c: &[usize]| (!c.is_empty()).then(|| c[rng.below(c.len())]);
        let mut q = pick(&mut self.hop_rng, &qs);
        // If V would pick Q itself, the transfer takes that one hop.
        let single = q.is_some_and(|q| ps.contains(&q));
        let p = if single {
            q
        } else {
            pick(&mut self.hop_rng, &ps)
        };
        if single {
            q = None;
        }
        if p.is_none() && q.is_none() {
            return vec![Leg {
                from: h,
                relayed: true,
                carried: false,
            }];
        }
        let mut taken = vec![h, v];
        taken.extend(p);
        taken.extend(q);
        let m = if self.hop_rng.chance(MIDDLE_EVERY) {
            self.middle(v, &taken, load)
        } else {
            None
        };
        if let Some(m) = m {
            self.hops.middle_turns += 1;
            self.first_middle[m].get_or_insert(t);
            self.first_job[m].get_or_insert(t);
        }
        // Counts for mark 9.
        let ends: Vec<usize> = p.iter().chain(q.iter()).copied().collect();
        self.hops.with_hops += 1;
        self.hops.end_hops += ends.len();
        self.hops.end_hops_operator += ends.iter().filter(|&&e| self.operator[e]).count();
        if single || (p.is_some() && q.is_some()) {
            self.hops.both_ends += 1;
            if ends.iter().all(|&e| self.operator[e]) {
                self.hops.operator_both_ends += 1;
            }
        }
        // An end with no hop of its own reaches the next through the relay;
        // hops reach one another, and the ends they serve, directly.
        let mut chain = vec![h];
        chain.extend(q);
        chain.extend(m);
        chain.extend(p);
        let last = chain.len() - 1;
        chain
            .iter()
            .enumerate()
            .map(|(k, &from)| Leg {
                from,
                relayed: (k == 0 && q.is_none() && !single) || (k == last && p.is_none()),
                carried: k > 0,
            })
            .collect()
    }
}

impl Media {
    /// Up to 6 followers holding a copy, as Kim's node names them to `to`.
    fn named(&self, to: usize, swarm: &Swarm, rng: &mut Rng) -> Vec<usize> {
        match self.rule_set {
            RuleSet::Accepted => swarm.named(to, rng),
            RuleSet::Partners => {
                let mut pool: Vec<usize> = swarm.pool().into_iter().filter(|&n| n != to).collect();
                rng.shuffle(&mut pool);
                let days = |n: usize| {
                    self.records
                        .get(self.creator, n)
                        .map_or(0.0, |r| r.work_days)
                };
                pool.sort_by(|&a, &b| days(b).total_cmp(&days(a)));
                let mut named: Vec<usize> = pool.iter().copied().take(NAMED_BY_CHECKS).collect();
                let mut rest: Vec<usize> = pool.into_iter().skip(NAMED_BY_CHECKS).collect();
                rng.shuffle(&mut rest);
                named.extend(
                    rest.into_iter()
                        .take(SWARM_SAMPLE - named.len().min(SWARM_SAMPLE)),
                );
                named.sort_by_key(|&n| !swarm.direct[n]);
                named
            }
        }
    }
}

impl Rules for Media {
    fn sources(&mut self, to: usize, swarm: &Swarm, _now: f64, rng: &mut Rng) -> Sources {
        if self.first_copies[to] {
            // C: told of the post first, it fetches from Kim at once.
            return Sources {
                at_once: Vec::new(),
                in_turn: vec![self.creator],
            };
        }
        let named = self.named(to, swarm, rng);
        let at_once = match self.rule_set {
            RuleSet::Accepted => named,
            RuleSet::Partners => {
                // Partners holding it in 2 of the 3 places, a follower not
                // worked with in the third, then the rest named.
                let t = self.post_at;
                self.partners[to].fade_to(t);
                let mut list: Vec<usize> = self.partners[to]
                    .0
                    .iter()
                    .map(|p| p.0)
                    .filter(|p| swarm.noted.contains(p))
                    .take(PARTNERS_AT_ONCE)
                    .collect();
                if let Some(&fresh) = named.iter().find(|&&n| !self.partners[to].has(n)) {
                    list.push(fresh);
                }
                for n in named {
                    if !list.contains(&n) {
                        list.push(n);
                    }
                }
                list
            }
        };
        Sources {
            at_once,
            in_turn: vec![self.creator],
        }
    }

    fn path(&mut self, source: usize, to: usize, load: &Load, _rng: &mut Rng) -> Vec<Leg> {
        self.last_b = false;
        let direct = self.records.direct(source, to);
        let mut legs = match self.way {
            Way::Hops if !direct => self.hop_path(source, to, load),
            _ => vec![Leg {
                from: source,
                relayed: self.way == Way::Relays || !direct,
                carried: false,
            }],
        };
        // B, in way 3 where machines carry: a transfer through the relay
        // with no hop at either end, now and then, through a machine with no
        // standing, both legs through the relay.
        if self.way == Way::Hops
            && self.exp.b_share > 0.0
            && legs.len() == 1
            && legs[0].relayed
            && self.b_rng.chance(self.exp.b_share)
            && let Some(m) = self.b_job(source, to, load)
        {
            legs = vec![
                Leg {
                    from: source,
                    relayed: true,
                    carried: false,
                },
                Leg {
                    from: m,
                    relayed: true,
                    carried: true,
                },
            ];
            self.last_b = true;
        }
        // Each leg not through the relay shows its two machines' addresses
        // to each other.
        for k in 0..legs.len() {
            if !legs[k].relayed {
                let next = legs.get(k + 1).map_or(to, |l| l.from);
                self.expose(legs[k].from, next);
            }
        }
        legs
    }

    fn fail_after(&mut self, part: f64) -> Option<f64> {
        if !self.last_b || self.exp.b_fail <= 0.0 {
            return None;
        }
        let fails = self.fail_rng.chance(self.exp.b_fail);
        let at = self.fail_rng.range(0.1, 0.9);
        fails.then(|| {
            self.extra.b_fail_picked += 1;
            part * at
        })
    }

    fn part_done(&mut self, part: &Part, now: f64) {
        let t = self.post_at + now;
        if part.bytes > 0.0 {
            self.records.work(part.to, part.source, part.bytes, t);
            self.records.served(part.source, part.to, t);
            self.first_served[part.source].get_or_insert(t);
            if self.operator[part.to] {
                self.operator_down += part.bytes;
            }
            let (a, b) = (part.source.min(part.to), part.source.max(part.to));
            self.swapped.entry((a, b)).or_default()[(part.source != a) as usize] += part.bytes;
            if self.partners[part.to].has(part.source) {
                self.from_partners += part.bytes;
            }
            if self.operator[part.source] {
                self.from_operator += part.bytes;
            }
            if part.source != self.creator && part.to != self.creator {
                self.partners[part.to].swapped(part.source, part.bytes, t);
                self.partners[part.source].swapped(part.to, part.bytes, t);
            }
            // B's jobs: two legs through the relay, the second carried.
            let b =
                part.legs.len() == 2 && part.legs.iter().all(|l| l.relayed) && part.legs[1].carried;
            if b {
                let m = part.legs[1].from;
                let wire = part.bytes * OVERHEAD;
                self.extra.b_relay_extra += wire;
                self.extra.b_standby += wire;
                self.extra.b_upload += wire;
                if self.newcomer[m] {
                    self.extra.b_upload_newcomers += wire;
                }
            }
            if part.hop_failed {
                // A failed job earns its machine nothing.
                self.extra.b_failed += 1;
                self.failed_to.push(part.to);
            } else {
                // The viewer credits every hop for a job finished. Its node's
                // record counts what the machine carried once the node has
                // shown the machine, which it does at layer 2 with the
                // viewer; under A, with the binding, from the first job.
                let day = (t / DAY) as usize;
                for leg in part.legs.iter().filter(|l| l.carried) {
                    let m = leg.from;
                    self.carried.work(part.to, m, part.bytes, t);
                    self.job_days.entry((m, part.to)).or_default().insert(day);
                    if self.exp.one_way
                        || self.records.get(m, part.to).is_some_and(|r| r.layer >= 2)
                    {
                        self.records.work(part.to, m, part.bytes, t);
                    }
                }
                if b {
                    self.b_known.work(part.legs[1].from, part.to, part.bytes, t);
                }
            }
            // The hop next to the viewer knows which machine it carried for.
            if let Some(last) = part.legs.last().filter(|l| l.carried && !l.relayed) {
                self.near_viewer.work(last.from, part.to, part.bytes, t);
            }
        }
    }

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
    pub delivered_direct: f64,
    pub busy: usize,
    pub failed: usize,
    /// B8's three marks for this post.
    pub marks: bool,
    /// Pairs with a direct link when the post went out.
    pub direct_pairs: usize,
    /// Of the followers there from day 0 that said yes to earned links, the
    /// share with a direct link to someone other than Kim (mark 4).
    pub day0_linked: f64,
    /// Shares of the bytes delivered that came from a partner, and from the
    /// operator's nodes.
    pub from_partners: f64,
    pub from_operator: f64,
    /// Records between nodes other than Kim at layers 0 to 4.
    pub layers: [usize; 5],
    /// The most checked bytes, and work days, in any record not Kim's.
    pub most_bytes: f64,
    pub most_days: f64,
    /// Bytes on the wire sent by honest followers' homes (Kim not counted),
    /// and by Kim's.
    pub home_up: f64,
    pub kim_up: f64,
}

#[derive(Debug, Serialize)]
pub struct Months {
    pub start: Start,
    pub way: Way,
    pub rule_set: RuleSet,
    pub days: DayRule,
    pub bytes_rule: BytesRule,
    pub asking: Asking,
    pub experiment: Experiment,
    pub seed: u64,
    pub posts: Vec<Post>,
    pub newcomers: Vec<Newcomer>,
    /// Why follower pairs aren't direct peers, by day 60 and at the end.
    pub pairs_day60: Option<PairReport>,
    pub pairs_end: PairReport,
    /// Kim's spot checks, apart: they can't reach layer 4's bytes.
    pub spot_checks: SpotChecks,
    /// Way 3's transfers through hops, for mark 9.
    pub hops: HopCounts,
    /// Partners at the end: how evenly each pair swapped, honest pairs and
    /// pairs with an operator's node apart.
    pub reciprocity_honest: Reciprocity,
    pub reciprocity_operator: Reciprocity,
    /// Mark 6: on the last day, followers present (Kim not counted), and
    /// the largest group of them linked by partners.
    pub present_end: usize,
    pub largest_partner_group: usize,
    /// Meeting, lists and introductions.
    pub meet: MeetCounts,
    /// Mark 8: established honest nodes (there from day 0) that carry, and
    /// of those, how many heard of a machine not in their table through a
    /// list or an introduction after the directory went off. Also the
    /// directory newcomers that carry, as context.
    pub established_carrying: usize,
    pub established_heard_new: usize,
    pub directory_newcomers_carrying: usize,
    pub directory_newcomers_heard_new: usize,
    /// The operator's figures (ADR 0022's price): honest peers with a direct
    /// link to one of its nodes at the end; bytes of the file its nodes
    /// received; bytes on the wire they sent, serving and carrying.
    pub operator_direct_honest: usize,
    pub operator_down: f64,
    pub operator_up: f64,
    /// The experiments' figures.
    pub extra: Extra,
}

/// Bytes each side of a partner pair served the other over the run.
#[derive(Debug, Default, Serialize)]
pub struct Reciprocity {
    pub pairs: usize,
    /// The median share of the pair's bytes that the side serving less
    /// served (0.5 is even).
    pub median_lesser_share: f64,
    /// Pairs where the side serving less served under a tenth, and none.
    pub under_a_tenth: usize,
    pub one_way_only: usize,
}

/// Follower pairs, Kim left out, that have dealt with each other, and why
/// those not directly connected aren't.
#[derive(Debug, Default, Serialize)]
pub struct PairReport {
    pub day: f64,
    pub all: Blocked,
    /// Of those, pairs where one is the other's partner for Kim's posts.
    pub partners: Blocked,
    /// The partner pairs again: both honest, and with an operator's node.
    pub partners_honest: Blocked,
    pub partners_operator: Blocked,
}

#[derive(Debug, Default, Serialize)]
pub struct Blocked {
    pub pairs: usize,
    pub direct: usize,
    /// One said no to earned links, and of those, pairs whose records
    /// met everything else.
    pub consent: usize,
    pub consent_only: usize,
    /// Pairs where both said yes and aren't direct, by the first thing
    /// missing, in `Block::ALL`'s order (consent's place left at 0):
    /// `WorkDays` if neither did work for the other on 3 days in the last
    /// 30, `BothWays` if only one did (the other direction is the same
    /// transfers seen from the other side), then bytes, age and caps. Also
    /// the only thing missing, and anything missing.
    pub both_yes_blocked: usize,
    /// Of the pairs where work went one way only, those with an operator's
    /// node in them (they fetch at once, so they nearly always hold first).
    pub one_way_with_operator: usize,
    pub first: [usize; 6],
    pub only: [usize; 6],
    pub any: [usize; 6],
}

impl Blocked {
    /// `ab` and `ba`: what each side's record of the other is missing.
    fn add(&mut self, direct: bool, consent: bool, operator: bool, ab: &[Block], ba: &[Block]) {
        self.pairs += 1;
        if direct {
            self.direct += 1;
            return;
        }
        if consent {
            self.consent += 1;
            self.consent_only += (ab.is_empty() && ba.is_empty()) as usize;
            return;
        }
        self.both_yes_blocked += 1;
        let idx = |b: &Block| Block::ALL.iter().position(|x| x == b).unwrap();
        let mut seen = [false; 6];
        let (wa, wb) = (ab.contains(&Block::WorkDays), ba.contains(&Block::WorkDays));
        if wa && wb {
            seen[idx(&Block::WorkDays)] = true;
        } else if wa || wb {
            seen[idx(&Block::BothWays)] = true;
            self.one_way_with_operator += operator as usize;
        }
        for b in ab.iter().chain(ba) {
            if !matches!(b, Block::WorkDays | Block::BothWays) {
                seen[idx(b)] = true;
            }
        }
        let hit: Vec<usize> = (0..6).filter(|&i| seen[i]).collect();
        if let Some(&f) = hit.first() {
            self.first[f] += 1;
        }
        if hit.len() == 1 {
            self.only[hit[0]] += 1;
        }
        for i in hit {
            self.any[i] += 1;
        }
    }
}

#[derive(Debug, Default, Serialize)]
pub struct SpotChecks {
    /// Followers Kim's record holds at layer 2 or more, and at 3.
    pub at_layer2: usize,
    pub at_layer3: usize,
    /// The most checked bytes Kim received from one follower in the last 30
    /// days, and faded, against layer 4's 100 MB.
    pub most_window_bytes: f64,
    pub most_faded_bytes: f64,
    /// Kim's direct links.
    pub links: usize,
}

fn pair_report(rules: &Media, creator: usize, t: f64) -> PairReport {
    let mut rep = PairReport {
        day: t / DAY,
        ..Default::default()
    };
    for (a, b) in rules.records.pairs() {
        if a == creator || b == creator {
            continue;
        }
        let direct = rules.records.direct(a, b);
        let consent = !(rules.records.yes[a] && rules.records.yes[b]);
        let ab = rules.records.blocks(a, b, t);
        let ba = rules.records.blocks(b, a, t);
        assert!(
            direct || consent || !(ab.is_empty() && ba.is_empty()),
            "pair {a}, {b} meets everything but isn't direct: allows {} / {}, earned and new this week {:?} / {:?}",
            rules.records.allows(a, b),
            rules.records.allows(b, a),
            rules.records.earned_load(a, t),
            rules.records.earned_load(b, t),
        );
        let operator = rules.operator[a] || rules.operator[b];
        rep.all.add(direct, consent, operator, &ab, &ba);
        if rules.partners[a].has(b) || rules.partners[b].has(a) {
            rep.partners.add(direct, consent, operator, &ab, &ba);
            if operator {
                rep.partners_operator
                    .add(direct, consent, operator, &ab, &ba);
            } else {
                rep.partners_honest.add(direct, consent, operator, &ab, &ba);
            }
        }
    }
    rep
}

fn spot_checks(rules: &Media, creator: usize, n: usize) -> SpotChecks {
    let mut s = SpotChecks {
        links: rules.records.direct_peers(creator).len(),
        ..Default::default()
    };
    for f in 0..n {
        if let Some(r) = rules.records.get(creator, f) {
            s.at_layer2 += (r.layer >= 2) as usize;
            s.at_layer3 += (r.layer >= 3) as usize;
            s.most_window_bytes = s.most_window_bytes.max(r.window_bytes());
            s.most_faded_bytes = s.most_faded_bytes.max(r.bytes);
        }
    }
    s
}

/// Runs meeting up to `t`, each day's standing worked out from the records
/// (ADR 0022's work first), and has each node's records count the
/// introductions it accepted.
fn meet_until(rules: &mut Media, t: f64) {
    let Media {
        meeting,
        records,
        carried,
        near_viewer,
        b_known,
        exp,
        ..
    } = rules;
    let binding = exp.one_way;
    meeting.advance(t, &mut |_| {
        let mut s: Standing = carried.at_least(2).into_iter().collect();
        for (h, a) in records.at_least(2) {
            // Downloads count once the other's node has shown its machine,
            // which it does at layer 2; under A, with the binding, from the
            // first.
            if binding || records.get(a, h).is_some_and(|r| r.layer >= 2) {
                s.insert((h, a));
            }
        }
        // What each machine's own record tells it: it was the hop next to
        // a viewer on 3 days, or did B's jobs for it on 3 days, or its node
        // served another on 3 days in the last 30 and has shown it its
        // machine (under A, given it the binding).
        let mut reasons: Standing = near_viewer.at_least(2).into_iter().collect();
        reasons.extend(b_known.at_least(2));
        if binding {
            reasons.extend(records.served_on_3_days());
        } else {
            for (v, m) in records.at_least(2) {
                if records
                    .get(v, m)
                    .is_some_and(|r| r.served_days_window() >= 3)
                {
                    reasons.insert((v, m));
                }
            }
        }
        (s, reasons)
    });
    for (kim, jimmy, until) in std::mem::take(&mut meeting.to_count) {
        records.introduce(kim, jimmy, until);
    }
}

/// One newcomer's progress, in days after it arrived, as seen at each post.
#[derive(Debug, Serialize)]
pub struct Newcomer {
    pub node: usize,
    pub kind: Kind,
    /// Some node other than Kim holds it at layer 2 or more: work on 3 days
    /// for that node.
    pub regular: Option<f64>,
    /// It has a direct link with someone.
    pub linked: Option<f64>,
    /// Way 3, for mark 7: it said yes to carrying; its first middle turn;
    /// some node holds it at jobs on 3 days.
    pub carries: bool,
    pub middle: Option<f64>,
    pub carry_standing: Option<f64>,
    /// Mark 7's last part: a list from a node it worked for, and an
    /// introduction sent of it. Mark 6's: its first copy served.
    pub list: Option<f64>,
    pub introduced: Option<f64>,
    pub first_served: Option<f64>,
    /// Its first job, a middle turn or one of B's; B's jobs it did.
    pub first_job: Option<f64>,
    pub b_jobs: usize,
    /// Jobs it finished, for how many viewers, and the pairs with a viewer
    /// that had jobs on 1, 2, and 3 days or more, over the run.
    pub jobs: usize,
    pub viewers: usize,
    pub pairs_by_days: [usize; 3],
    /// The most days of jobs for any one viewer in the last 30, at any post.
    pub best_same_viewer: usize,
    /// Days it turned a job away under B's daily limit.
    pub limit_days: usize,
}

fn reciprocity(rules: &Media, creator: usize, operator: bool) -> Reciprocity {
    let mut shares = Vec::new();
    let mut r = Reciprocity::default();
    for (&(a, b), &[ab, ba]) in &rules.swapped {
        if a == creator || b == creator || (rules.operator[a] || rules.operator[b]) != operator {
            continue;
        }
        if !(rules.partners[a].has(b) || rules.partners[b].has(a)) {
            continue;
        }
        let lesser = ab.min(ba) / (ab + ba).max(1.0);
        r.pairs += 1;
        r.under_a_tenth += (lesser < 0.1) as usize;
        r.one_way_only += (ab.min(ba) == 0.0) as usize;
        shares.push(lesser);
    }
    shares.sort_by(f64::total_cmp);
    r.median_lesser_share = shares.get(shares.len() / 2).copied().unwrap_or(0.0);
    r
}

/// The Gini coefficient of `xs`: 0 when all are equal, near 1 when one
/// has it all.
fn gini(xs: &[f64]) -> f64 {
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len() as f64;
    let total: f64 = v.iter().sum();
    if n == 0.0 || total <= 0.0 {
        return 0.0;
    }
    let weighted: f64 = v
        .iter()
        .enumerate()
        .map(|(i, x)| (i as f64 + 1.0) * x)
        .sum();
    (2.0 * weighted) / (n * total) - (n + 1.0) / n
}

/// The largest group of `nodes` linked by partners, either way.
fn largest_group(rules: &Media, nodes: &[usize]) -> usize {
    let n = rules.partners.len();
    let mut root: Vec<usize> = (0..n).collect();
    fn find(root: &mut [usize], x: usize) -> usize {
        let mut x = x;
        while root[x] != x {
            root[x] = root[root[x]];
            x = root[x];
        }
        x
    }
    let inside: Vec<bool> = (0..n).map(|i| nodes.contains(&i)).collect();
    for &a in nodes {
        for &(b, _, _) in &rules.partners[a].0 {
            if inside[b] {
                let (ra, rb) = (find(&mut root, a), find(&mut root, b));
                root[ra] = rb;
            }
        }
    }
    let mut size: HashMap<usize, usize> = HashMap::new();
    for &a in nodes {
        *size.entry(find(&mut root, a)).or_default() += 1;
    }
    size.into_values().max().unwrap_or(0)
}

/// Kim's spot checks a day, each of a 1 KiB piece (a starting value).
const SPOT_CHECKS_A_DAY: usize = 100;
const SPOT_CHECK_BYTES: f64 = 1024.0;
/// Honest nodes saying yes to earned links (a starting value).
const YES_TO_EARNED: f64 = 0.75;

/// One start, one way, one set of rules and one experiment, for a seed.
#[allow(clippy::too_many_arguments)]
pub fn run(
    start: Start,
    way: Way,
    rule_set: RuleSet,
    days: DayRule,
    bytes_rule: BytesRule,
    asking: Asking,
    experiment: Experiment,
    seed: u64,
) -> Months {
    let exp = experiment.settings();
    // What's the same in every way: who's there, who says yes, when each
    // post goes out and when each follower wakes for it.
    let mut plan = Rng::new(seed);
    let (members, mut peers) = population(start);
    let creator = 0;
    let yes: Vec<bool> = members
        .iter()
        .map(|m| m.kind == Kind::Operator || plan.chance(YES_TO_EARNED))
        .collect();
    // Who says yes to carrying, picked apart from the rest, so ways 1 and 2
    // are unchanged by it. Only way 3 uses hops; Kim's node doesn't carry.
    let mut carry_plan = Rng::new(seed.wrapping_add(3));
    let carries: Vec<bool> = members
        .iter()
        .map(|m| match m.kind {
            Kind::Creator => false,
            Kind::Operator => true,
            _ => carry_plan.chance(YES_TO_CARRYING),
        })
        .collect();
    if way == Way::Hops {
        for (i, p) in peers.iter_mut().enumerate() {
            if carries[i] {
                p.carry_share = CARRY_SHARE;
                p.carry_places = transfer_places(p.up * 8.0 / 1e6 * CARRY_SHARE);
            }
        }
    }
    let schedule: Vec<(f64, Vec<Fetch>)> = (0..POSTS)
        .map(|index| {
            let at = (index as f64 * POST_EVERY_DAYS) * DAY + plan.range(0.0, DAY);
            let fetches = (1..members.len())
                .filter(|&i| members[i].present(at))
                .map(|node| Fetch {
                    node,
                    wakes: if members[node].kind == Kind::Operator {
                        0.0
                    } else {
                        plan.range(0.0, SPREAD_SECS)
                    },
                })
                .collect();
            (at, fetches)
        })
        .collect();
    let mut rng = Rng::new(seed.wrapping_add(1));
    let mut checks = Rng::new(seed.wrapping_add(2));
    // Each newcomer started from one machine is given one honest follower's,
    // picked at random, as a friend would pass it on.
    let mut friends = Rng::new(seed.wrapping_add(5));
    let established: Vec<usize> = (0..members.len())
        .filter(|&i| members[i].kind == Kind::Follower)
        .collect();
    let who = Who {
        arrives: members.iter().map(|m| m.arrives).collect(),
        leaves: members.iter().map(|m| m.leaves).collect(),
        operator: members.iter().map(|m| m.kind == Kind::Operator).collect(),
        given: members
            .iter()
            .map(|m| {
                (m.kind == Kind::NewcomerGiven)
                    .then(|| established[friends.below(established.len())])
            })
            .collect(),
    };
    let mut rules = Media {
        meeting: {
            let mut m = Meeting::new(who, seed.wrapping_add(6));
            m.asking = asking;
            m
        },
        first_served: vec![None; peers.len()],
        swapped: HashMap::new(),
        operator_down: 0.0,
        way,
        rule_set,
        creator,
        partners: vec![Partners::default(); peers.len()],
        from_partners: 0.0,
        from_operator: 0.0,
        operator: members.iter().map(|m| m.kind == Kind::Operator).collect(),
        post_at: 0.0,
        records: Records::new(yes.clone(), days, bytes_rule),
        carries: carries.clone(),
        present: vec![false; peers.len()],
        carried: Records::new(vec![true; peers.len()], days, bytes_rule),
        near_viewer: Records::new(vec![true; peers.len()], days, bytes_rule),
        hop_rng: Rng::new(seed.wrapping_add(4)),
        first_middle: vec![None; peers.len()],
        hops: HopCounts::default(),
        exp,
        newcomer: members
            .iter()
            .map(|m| matches!(m.kind, Kind::NewcomerDirectory | Kind::NewcomerGiven))
            .collect(),
        b_rng: Rng::new(seed.wrapping_add(7)),
        fail_rng: Rng::new(seed.wrapping_add(8)),
        last_b: false,
        b_day_jobs: HashMap::new(),
        b_limit_days: HashMap::new(),
        b_known: Records::new(vec![true; peers.len()], days, bytes_rule),
        job_days: HashMap::new(),
        first_job: vec![None; peers.len()],
        b_jobs_of: vec![0; peers.len()],
        first_copies: vec![false; peers.len()],
        exposed_any: vec![false; peers.len()],
        exposed_operator: vec![false; peers.len()],
        extra: Extra {
            b_offered: vec![0; RUN_DAYS],
            b_taken: vec![0; RUN_DAYS],
            b_turned_away: vec![0; RUN_DAYS],
            ..Default::default()
        },
        failed_to: Vec::new(),
    };
    rules.records.one_way = exp.one_way;
    let mut kim_rng = Rng::new(seed.wrapping_add(9));
    let mut served_total = vec![0.0f64; peers.len()];
    let mut links: HashMap<(usize, usize), bool> = HashMap::new();
    let mut best_same_viewer = vec![0usize; peers.len()];
    // S: the 10 approved by hand to one another are direct peers.
    let approved: Vec<usize> = (0..members.len())
        .filter(|&i| members[i].kind == Kind::Approved)
        .collect();
    for (k, &a) in approved.iter().enumerate() {
        for &b in &approved[k + 1..] {
            rules.records.approve(a, b);
        }
    }
    let mut hosts = vec![false; peers.len()];
    let mut newcomers: Vec<Newcomer> = (0..members.len())
        .filter(|&i| {
            matches!(
                members[i].kind,
                Kind::NewcomerDirectory | Kind::NewcomerGiven
            )
        })
        .map(|node| Newcomer {
            node,
            kind: members[node].kind,
            regular: None,
            linked: None,
            carries: carries[node],
            middle: None,
            carry_standing: None,
            list: None,
            introduced: None,
            first_served: None,
            first_job: None,
            b_jobs: 0,
            jobs: 0,
            viewers: 0,
            pairs_by_days: [0; 3],
            best_same_viewer: 0,
            limit_days: 0,
        })
        .collect();
    let mut checked_to = 0.0;
    let mut operator_up = 0.0;
    let mut posts = Vec::new();
    let mut pairs_day60 = None;
    for (index, (at, mut fetches)) in schedule.into_iter().enumerate() {
        // Kim's spot checks since the last post, of followers holding a copy.
        while checked_to + DAY <= at {
            let candidates: Vec<usize> = (1..peers.len())
                .filter(|&i| hosts[i] && members[i].present(checked_to))
                .collect();
            if !candidates.is_empty() {
                for _ in 0..SPOT_CHECKS_A_DAY {
                    let f = candidates[checks.below(candidates.len())];
                    let t = checked_to + checks.range(0.0, DAY);
                    rules.records.work(creator, f, SPOT_CHECK_BYTES, t);
                }
            }
            checked_to += DAY;
        }
        meet_until(&mut rules, at);
        if pairs_day60.is_none() && at > 60.0 * DAY {
            rules.records.sweep(60.0 * DAY);
            pairs_day60 = Some(pair_report(&rules, creator, 60.0 * DAY));
            let (active, missing) = rules.records.active_missing_weekly(60.0 * DAY, creator);
            rules.extra.active_day60 = active;
            rules.extra.missing_weekly_day60 = missing;
        }
        rules.records.sweep(at);
        let standing = rules.records.standing(peers.len(), creator);
        rules.carried.sweep(at);
        let carry_standing = rules.carried.standing(peers.len(), creator);
        for nc in &mut newcomers {
            let arrived = members[nc.node].arrives;
            if arrived > at {
                continue;
            }
            let since = (at - arrived) / DAY;
            if nc.regular.is_none() && standing[nc.node] >= 2 {
                nc.regular = Some(since);
            }
            if nc.linked.is_none() && !rules.records.direct_peers(nc.node).is_empty() {
                nc.linked = Some(since);
            }
            if nc.carry_standing.is_none() && carry_standing[nc.node] >= 2 {
                nc.carry_standing = Some(since);
            }
            nc.middle = rules.first_middle[nc.node].map(|m| (m - arrived) / DAY);
        }
        rules.post_at = at;
        rules.from_partners = 0.0;
        rules.from_operator = 0.0;
        let present: Vec<usize> = fetches.iter().map(|f| f.node).collect();
        rules.present = vec![false; peers.len()];
        for &i in &present {
            rules.present[i] = true;
        }
        rules.carried.sweep(at);
        for (n, &d) in rules
            .carried
            .best_window_days(peers.len())
            .iter()
            .enumerate()
        {
            best_same_viewer[n] = best_same_viewer[n].max(d);
        }
        // Direct links show each pair's addresses to each other; earned
        // ones are counted by whether they needed work both ways.
        for &i in &present {
            for p in rules.records.direct_peers(i) {
                rules.expose(i, p);
                if i < p && !rules.records.is_approved(i, p) && i != creator {
                    let both = rules.records.both_ways(i, p);
                    links.entry((i, p)).or_insert(both);
                }
            }
        }
        // C: Kim's node tells up to 4 followers first, those that said yes
        // to earned links whose copies passed her checks on the most days.
        rules.first_copies = vec![false; peers.len()];
        rules.failed_to.clear();
        if exp.kim_first {
            let mut pool: Vec<usize> = present.iter().copied().filter(|&i| yes[i]).collect();
            kim_rng.shuffle(&mut pool);
            let days = |n: usize| rules.records.get(creator, n).map_or(0.0, |r| r.work_days);
            pool.sort_by(|&a, &b| days(b).total_cmp(&days(a)));
            for &i in pool.iter().take(KIM_FIRST) {
                rules.first_copies[i] = true;
                rules.extra.kim_picks += 1;
                rules.extra.kim_picks_operator += rules.operator[i] as usize;
            }
            for f in &mut fetches {
                if rules.first_copies[f.node] {
                    f.wakes = 0.0;
                }
            }
        }
        // Kim's node notes each copy anew for each post; who has followed
        // longest is the order they followed. It names first those it
        // reaches directly.
        let mut swarm = Swarm::new(peers.len());
        for &i in &present {
            swarm.direct[i] = rules.records.direct(creator, i);
        }
        let direct_pairs = present
            .iter()
            .map(|&i| {
                rules
                    .records
                    .direct_peers(i)
                    .into_iter()
                    .filter(|&p| p > i && rules.present[p])
                    .count()
            })
            .sum();
        let day0: Vec<usize> = present
            .iter()
            .copied()
            .filter(|&i| members[i].arrives == 0.0 && members[i].kind != Kind::Operator && yes[i])
            .collect();
        let linked = day0
            .iter()
            .filter(|&&i| rules.records.direct_peers(i).iter().any(|&p| p != creator))
            .count();
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
        let mut home_up = 0.0;
        for &i in &present {
            hosts[i] |= out.held[i].is_some();
            if rules.operator[i] {
                operator_up += out.sent[i];
            } else {
                home_up += out.sent[i];
            }
            served_total[i] += out.served[i];
        }
        rules.extra.b_fail_extra += out.hop_failure_extra;
        let completed = rules
            .failed_to
            .iter()
            .filter(|&&v| out.held[v].is_some())
            .count();
        rules.extra.b_failed_completed += completed;
        let held: Vec<Option<f64>> = present.iter().map(|&i| out.held[i]).collect();
        let p95 = time_for(&held, 0.95);
        let all = time_for(&held, 1.0);
        let creator_copies = out.served[creator] / VIDEO_BYTES;
        let marks = p95.is_some_and(|t| t <= P95_WITHIN)
            && all.is_some_and(|t| t <= ALL_WITHIN)
            && creator_copies <= CREATOR_SHARE * present.len() as f64;
        posts.push(Post {
            index,
            at,
            present: present.len(),
            p50: time_for(&held, 0.5),
            p95,
            all,
            creator_copies,
            relayed: out.relayed,
            delivered: out.delivered,
            delivered_off_relay: out.delivered_off_relay,
            delivered_through_hops: out.delivered_through_hops,
            delivered_direct: out.delivered_direct,
            busy: out.busy,
            failed: out.failed,
            marks,
            direct_pairs,
            from_partners: rules.from_partners / out.delivered.max(1.0),
            from_operator: rules.from_operator / out.delivered.max(1.0),
            day0_linked: linked as f64 / day0.len().max(1) as f64,
            layers: rules.records.layers_without(creator),
            most_bytes: rules.records.most_without(creator).0,
            most_days: rules.records.most_without(creator).1,
            home_up,
            kim_up: out.sent[creator],
        });
    }
    let end = posts.last().map_or(0.0, |p| p.at) + POST_UNTIL;
    rules.records.sweep(end);
    // Meeting runs on to day 91, for mark 8.
    meet_until(&mut rules, 91.0 * DAY);
    // Introductions counted since change what the records reach.
    rules.records.sweep(end);
    // The model's own check: no table over ADR 0015's and 0016's limits.
    assert!((0..peers.len()).all(|v| rules.meeting.sources_ok(v)));
    for nc in &mut newcomers {
        let arrived = members[nc.node].arrives;
        let since = |t: Option<f64>| t.map(|t| (t - arrived) / DAY);
        nc.middle = since(rules.first_middle[nc.node]);
        nc.list = since(rules.meeting.first_list[nc.node]);
        nc.introduced = since(rules.meeting.first_introduced[nc.node]);
        nc.first_served = since(rules.first_served[nc.node]);
        nc.first_job = since(rules.first_job[nc.node]);
        nc.b_jobs = rules.b_jobs_of[nc.node];
        nc.best_same_viewer = best_same_viewer[nc.node];
        nc.limit_days = rules.b_limit_days.get(&nc.node).map_or(0, |d| d.len());
        for (&(m, _), d) in &rules.job_days {
            if m == nc.node {
                nc.viewers += 1;
                nc.jobs += d.len();
                nc.pairs_by_days[d.len().min(3) - 1] += 1;
            }
        }
    }
    let (active, missing) = rules.records.active_missing_weekly(end, creator);
    rules.extra.active_end = active;
    rules.extra.missing_weekly_end = missing;
    for (&(a, b), &both) in &links {
        if both {
            rules.extra.links_both_ways += 1;
        } else {
            rules.extra.links_one_way += 1;
        }
        if rules.records.direct(a, b) {
            if rules.records.both_ways(a, b) {
                rules.extra.links_end_both_ways += 1;
            } else {
                rules.extra.links_end_one_way += 1;
            }
        }
    }
    // Fairness among honest followers, newcomers among them.
    let mut served: Vec<f64> = (0..members.len())
        .filter(|&i| !matches!(members[i].kind, Kind::Creator | Kind::Operator))
        .map(|i| served_total[i])
        .collect();
    served.sort_by(|a, b| b.total_cmp(a));
    let total: f64 = served.iter().sum();
    let top = served.len().div_ceil(10);
    rules.extra.top10_share = served[..top].iter().sum::<f64>() / total.max(1.0);
    rules.extra.gini = gini(&served);
    let honest: Vec<usize> = (0..members.len()).filter(|&i| !rules.operator[i]).collect();
    rules.extra.honest = honest.len();
    rules.extra.exposed_any = honest.iter().filter(|&&i| rules.exposed_any[i]).count();
    rules.extra.exposed_operator = honest
        .iter()
        .filter(|&&i| rules.exposed_operator[i])
        .count();
    let (mut entries, mut of_operator) = (0usize, 0usize);
    for &v in honest.iter().filter(|&&v| members[v].present(end)) {
        for m in rules.meeting.table(v) {
            entries += 1;
            of_operator += rules.operator[m] as usize;
        }
    }
    rules.extra.operator_table_share = of_operator as f64 / entries.max(1) as f64;
    let pairs_end = pair_report(&rules, creator, end);
    let spot_checks = spot_checks(&rules, creator, peers.len());
    let at_end: Vec<usize> = (1..members.len())
        .filter(|&i| members[i].present(end))
        .collect();
    for i in 0..peers.len() {
        rules.partners[i].fade_to(end);
    }
    let carrying = |kind: Kind| -> Vec<usize> {
        (0..members.len())
            .filter(|&i| members[i].kind == kind && carries[i])
            .collect()
    };
    let heard = |v: &[usize]| {
        v.iter()
            .filter(|&&i| rules.meeting.heard_new_late[i])
            .count()
    };
    let (est, dirn) = (carrying(Kind::Follower), carrying(Kind::NewcomerDirectory));
    let operator_direct_honest = (0..members.len())
        .filter(|&i| !rules.operator[i])
        .filter(|&i| {
            rules
                .records
                .direct_peers(i)
                .iter()
                .any(|&p| rules.operator[p])
        })
        .count();
    Months {
        reciprocity_honest: reciprocity(&rules, creator, false),
        reciprocity_operator: reciprocity(&rules, creator, true),
        present_end: at_end.len(),
        largest_partner_group: largest_group(&rules, &at_end),
        meet: rules.meeting.counts.clone(),
        established_carrying: est.len(),
        established_heard_new: heard(&est),
        directory_newcomers_carrying: dirn.len(),
        directory_newcomers_heard_new: heard(&dirn),
        operator_direct_honest,
        operator_down: rules.operator_down,
        operator_up,
        extra: rules.extra.clone(),
        experiment,
        start,
        way,
        rule_set,
        days,
        bytes_rule,
        asking,
        seed,
        posts,
        newcomers,
        pairs_day60,
        pairs_end,
        spot_checks,
        hops: rules.hops.clone(),
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
