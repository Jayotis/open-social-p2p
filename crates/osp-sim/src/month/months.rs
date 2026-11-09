//! Three months of Kim's posts ([plan]): who's in each start, when each
//! post goes out, and what each post cost and took.
//!
//! Built a step at a time. So far: ways 1 (relays alone) and 2 (earned
//! links) under the rules as accepted.
//!
//! [plan]: ../../../docs/testing/hops-model.md

use serde::Serialize;

use super::calibrate::{ASK_DELAY, DOWN, SPREAD_SECS, TELL_DELAY, UP, time_for};
use super::records::{Block, BytesRule, DayRule, Records};
use super::rng::Rng;
use super::spread::{
    Fetch, Leg, Load, Part, Peer, Rules, SWARM_SAMPLE, Sources, Spread, Swarm, VIDEO_BYTES,
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
    fn sources(&mut self, to: usize, swarm: &Swarm, rng: &mut Rng) -> Sources {
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

    fn path(&mut self, source: usize, to: usize, _load: &Load, _rng: &mut Rng) -> Vec<Leg> {
        let relayed = match self.way {
            Way::Relays => true,
            Way::Earned => !self.records.direct(source, to),
        };
        vec![Leg {
            from: source,
            relayed,
            carried: false,
        }]
    }

    fn part_done(&mut self, part: &Part, now: f64) {
        let t = self.post_at + now;
        if part.bytes > 0.0 {
            self.records.work(part.to, part.source, part.bytes, t);
            self.records.served(part.source, part.to, t);
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
}

#[derive(Debug, Serialize)]
pub struct Months {
    pub start: Start,
    pub way: Way,
    pub rule_set: RuleSet,
    pub days: DayRule,
    pub bytes_rule: BytesRule,
    pub seed: u64,
    pub posts: Vec<Post>,
    pub newcomers: Vec<Newcomer>,
    /// Why follower pairs aren't direct peers, by day 60 and at the end.
    pub pairs_day60: Option<PairReport>,
    pub pairs_end: PairReport,
    /// Kim's spot checks, apart: they can't reach layer 4's bytes.
    pub spot_checks: SpotChecks,
}

/// Follower pairs, Kim left out, that have dealt with each other, and why
/// those not directly connected aren't.
#[derive(Debug, Default, Serialize)]
pub struct PairReport {
    pub day: f64,
    pub all: Blocked,
    /// Of those, pairs where one is the other's partner for Kim's posts.
    pub partners: Blocked,
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
            "pair {a}, {b} meets everything but isn't direct"
        );
        let operator = rules.operator[a] || rules.operator[b];
        rep.all.add(direct, consent, operator, &ab, &ba);
        if rules.partners[a].has(b) || rules.partners[b].has(a) {
            rep.partners.add(direct, consent, operator, &ab, &ba);
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
}

/// Kim's spot checks a day, each of a 1 KiB piece (a starting value).
const SPOT_CHECKS_A_DAY: usize = 100;
const SPOT_CHECK_BYTES: f64 = 1024.0;
/// Honest nodes saying yes to earned links (a starting value).
const YES_TO_EARNED: f64 = 0.75;

pub fn run(
    start: Start,
    way: Way,
    rule_set: RuleSet,
    days: DayRule,
    bytes_rule: BytesRule,
    seed: u64,
) -> Months {
    // What's the same in every way: who's there, who says yes, when each
    // post goes out and when each follower wakes for it.
    let mut plan = Rng::new(seed);
    let (members, mut peers) = population(start);
    let creator = 0;
    let yes: Vec<bool> = members
        .iter()
        .map(|m| m.kind == Kind::Operator || plan.chance(YES_TO_EARNED))
        .collect();
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
    let mut rules = Media {
        way,
        rule_set,
        creator,
        partners: vec![Partners::default(); peers.len()],
        from_partners: 0.0,
        from_operator: 0.0,
        operator: members.iter().map(|m| m.kind == Kind::Operator).collect(),
        post_at: 0.0,
        records: Records::new(yes.clone(), days, bytes_rule),
    };
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
        })
        .collect();
    let mut checked_to = 0.0;
    let mut posts = Vec::new();
    let mut pairs_day60 = None;
    for (index, (at, fetches)) in schedule.into_iter().enumerate() {
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
        if pairs_day60.is_none() && at > 60.0 * DAY {
            rules.records.sweep(60.0 * DAY);
            pairs_day60 = Some(pair_report(&rules, creator, 60.0 * DAY));
        }
        rules.records.sweep(at);
        let standing = rules.records.standing(peers.len(), creator);
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
        }
        rules.post_at = at;
        rules.from_partners = 0.0;
        rules.from_operator = 0.0;
        let present: Vec<usize> = fetches.iter().map(|f| f.node).collect();
        // Kim's node notes each copy anew for each post; who has followed
        // longest is the order they followed. It names first those it
        // reaches directly.
        let mut swarm = Swarm::new(peers.len());
        for &i in &present {
            swarm.direct[i] = rules.records.direct(creator, i);
        }
        let direct_pairs = (0..peers.len())
            .map(|i| {
                rules
                    .records
                    .direct_peers(i)
                    .into_iter()
                    .filter(|&p| p > i)
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
        for &i in &present {
            hosts[i] |= out.held[i].is_some();
        }
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
        });
    }
    let end = posts.last().map_or(0.0, |p| p.at) + POST_UNTIL;
    rules.records.sweep(end);
    let pairs_end = pair_report(&rules, creator, end);
    let spot_checks = spot_checks(&rules, creator, peers.len());
    Months {
        start,
        way,
        rule_set,
        days,
        bytes_rule,
        seed,
        posts,
        newcomers,
        pairs_day60,
        pairs_end,
        spot_checks,
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
