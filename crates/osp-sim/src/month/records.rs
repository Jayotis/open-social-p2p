//! Each node's own record of each other node, and ADR 0018's layers
//! ([ADR 0018](../../../docs/decisions/0018-earned-direct-connections.md)).
//!
//! A node credits only what it received and checked itself: when Jimmy
//! serves Alice, the work goes in Alice's record of Jimmy, and what Alice
//! served Jimmy goes in his record of her as "the other direction". Nothing
//! passes between two records except a link offered, which is something the
//! peer did itself. Everything fades by half every 14 days.
//!
//! Reaching a layer and keeping it are counted apart (the maintainer's
//! clarification of ADR 0018, 2026-11-09): a layer is reached on actual
//! days of verified work, counted once each within the last 30 days, and
//! kept while the faded standing stays at half the layer's mark or more.
//! `DayRule::Faded`, the model's first reading, reached a layer on faded
//! days too, and is kept to compare the two.

use std::collections::HashMap;

pub const DAY: f64 = 86_400.0;
const HALF_LIFE: f64 = 14.0 * DAY;

/// ADR 0018's starting values, as the plan sets them.
const DAYS_FOR_REGULAR: f64 = 3.0;
const DAYS_FOR_BOTH_WAYS: f64 = 3.0;
const EARNED_AFTER: f64 = 30.0 * DAY;
pub const EARNED_BYTES: f64 = 100e6;
pub const MAX_EARNED: usize = 32;
const NEW_EARNED_A_WEEK: usize = 2;

/// How a layer's days are counted to reach it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, clap::ValueEnum)]
pub enum DayRule {
    /// Distinct days of verified work in the last 30 days (ADR 0018 as
    /// clarified).
    Window,
    /// The faded count of days, as for keeping a layer (the first reading).
    Faded,
}

/// How layer 4's checked bytes are counted to reach it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, clap::ValueEnum)]
pub enum BytesRule {
    /// Bytes received and checked in the last 30 days, from daily totals
    /// (the maintainer's decision, 2026-11-09).
    Window,
    /// The faded total, as for keeping the layer (the first reading).
    Faded,
}

/// Why a pair isn't directly connected, one direction's record or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
pub enum Block {
    /// One of them said no to earned links.
    Consent,
    /// Fewer than 3 days of work (layer 2).
    WorkDays,
    /// Not served on 3 days, and no offer (layer 3's both ways).
    BothWays,
    /// Too few checked bytes (layer 4).
    Bytes,
    /// Under 30 days since first contact (layer 4).
    Age,
    /// The holder had no room under the caps when all else was met.
    Caps,
}

impl Block {
    pub const ALL: [Block; 6] = [
        Block::Consent,
        Block::WorkDays,
        Block::BothWays,
        Block::Bytes,
        Block::Age,
        Block::Caps,
    ];
}

/// Days of work count toward reaching a layer for this long.
const QUALIFYING_WINDOW: i64 = 30;

/// One node's record of another, faded to `at`.
#[derive(Clone, Debug, Default)]
pub struct Record {
    /// Seconds from the start of the first contact.
    pub first: f64,
    /// Different days the other did checked work for this node, faded.
    pub work_days: f64,
    last_work_day: Option<i64>,
    /// Checked bytes of that work, faded.
    pub bytes: f64,
    /// Different days this node served the other, faded.
    pub served_days: f64,
    last_served_day: Option<i64>,
    /// The days of each, unfaded, in the last 30 days, to reach a layer.
    work_day_list: Vec<i64>,
    served_day_list: Vec<i64>,
    /// Checked bytes received each day, in the last 30 days: totals only.
    byte_days: Vec<(i64, f64)>,
    /// The other offered this node a link: it reached layer 4 with it.
    pub offered: bool,
    pub layer: u8,
    at: f64,
}

impl Record {
    fn fade_to(&mut self, t: f64) {
        if t > self.at {
            let f = 0.5f64.powf((t - self.at) / HALF_LIFE);
            self.work_days *= f;
            self.bytes *= f;
            self.served_days *= f;
            self.at = t;
        }
        let since = day(t) - QUALIFYING_WINDOW;
        self.work_day_list.retain(|&d| d > since);
        self.served_day_list.retain(|&d| d > since);
        self.byte_days.retain(|&(d, _)| d > since);
    }

    /// Different days this node served the other in the last 30 days.
    pub fn served_days_window(&self) -> usize {
        self.served_day_list.len()
    }

    /// Checked bytes received in the last 30 days.
    pub fn window_bytes(&self) -> f64 {
        self.byte_days.iter().map(|&(_, b)| b).sum()
    }
}

/// Each of a record's conditions for the layers, met or not, judged apart.
struct Conditions {
    layer1: bool,
    work_days: bool,
    both_ways: bool,
    bytes: bool,
    age: bool,
    room: bool,
}

/// Reaching a layer counts actual days (or, under `Faded`, faded ones) and,
/// for layer 4, bytes; keeping one it holds needs half the mark, faded. An
/// introduction the holder counts halves layer 4's days and bytes (the plan,
/// as ADR 0018 does for a reference).
fn conditions(
    r: &Record,
    rule: DayRule,
    bytes_rule: BytesRule,
    t: f64,
    room_now: bool,
    introduced: bool,
) -> Conditions {
    let cut = if introduced { 0.5 } else { 1.0 };
    let was = r.layer;
    let (work_q, served_q) = match rule {
        DayRule::Window => (r.work_day_list.len() as f64, r.served_day_list.len() as f64),
        DayRule::Faded => (r.work_days, r.served_days),
    };
    let bytes_q = match bytes_rule {
        BytesRule::Window => r.window_bytes(),
        BytesRule::Faded => r.bytes,
    };
    // Qualifies for `layer` by `q`, or keeps it by the faded `kept`.
    let ok =
        |layer: u8, q: f64, kept: f64, mark: f64| q >= mark || (was >= layer && kept >= 0.5 * mark);
    Conditions {
        layer1: ok(1, work_q, r.work_days, 1.0),
        work_days: ok(2, work_q, r.work_days, DAYS_FOR_REGULAR),
        both_ways: r.offered || ok(3, served_q, r.served_days, DAYS_FOR_BOTH_WAYS),
        bytes: ok(4, bytes_q, r.bytes, cut * EARNED_BYTES),
        age: t - r.first >= cut * EARNED_AFTER,
        room: was >= 4 || room_now,
    }
}

fn day(t: f64) -> i64 {
    (t / DAY).floor() as i64
}

#[derive(Debug)]
pub struct Records {
    /// (holder, about): `holder`'s record of `about`.
    rows: HashMap<(usize, usize), Record>,
    /// Who says yes to earned links.
    pub yes: Vec<bool>,
    pub rule: DayRule,
    pub bytes_rule: BytesRule,
    /// When each node granted layer 4 to each peer it holds there now.
    earned: Vec<Vec<(usize, f64)>>,
    /// When each node granted layer 4, for the weekly cap.
    granted_at: Vec<Vec<f64>>,
    /// Peers each node approved by hand: direct from the start, outside the
    /// caps (ADR 0018's fifth rule).
    approved: Vec<Vec<usize>>,
    /// (holder, about): an introduction of `about` that `holder` counts,
    /// until then.
    introduced: HashMap<(usize, usize), f64>,
}

impl Records {
    pub fn new(yes: Vec<bool>, rule: DayRule, bytes_rule: BytesRule) -> Self {
        let n = yes.len();
        Records {
            rows: HashMap::new(),
            yes,
            rule,
            bytes_rule,
            earned: vec![Vec::new(); n],
            granted_at: vec![Vec::new(); n],
            approved: vec![Vec::new(); n],
            introduced: HashMap::new(),
        }
    }

    /// `a` and `b` approved each other by hand.
    pub fn approve(&mut self, a: usize, b: usize) {
        self.approved[a].push(b);
        self.approved[b].push(a);
    }

    /// `holder` counts an introduction of `about` until `until`.
    pub fn introduce(&mut self, holder: usize, about: usize, until: f64) {
        let u = self.introduced.entry((holder, about)).or_insert(until);
        *u = u.max(until);
    }

    fn is_introduced(&self, holder: usize, about: usize, t: f64) -> bool {
        self.introduced
            .get(&(holder, about))
            .is_some_and(|&u| t < u)
    }

    fn row(&mut self, holder: usize, about: usize, t: f64) -> &mut Record {
        let r = self.rows.entry((holder, about)).or_insert_with(|| Record {
            first: t,
            at: t,
            ..Default::default()
        });
        r.fade_to(t);
        r
    }

    pub fn get(&self, holder: usize, about: usize) -> Option<&Record> {
        self.rows.get(&(holder, about))
    }

    /// `holder` received and checked `bytes` from `about` (a download's
    /// part, a spot check, or, once shown, a job its machine carried).
    pub fn work(&mut self, holder: usize, about: usize, bytes: f64, t: f64) {
        if bytes <= 0.0 || holder == about {
            return;
        }
        let r = self.row(holder, about, t);
        r.bytes += bytes;
        match r.byte_days.last_mut() {
            Some((d, b)) if *d == day(t) => *b += bytes,
            _ => r.byte_days.push((day(t), bytes)),
        }
        if r.last_work_day != Some(day(t)) {
            r.last_work_day = Some(day(t));
            r.work_days += 1.0;
            r.work_day_list.push(day(t));
        }
        self.update(holder, about, t);
    }

    /// `holder` served `about`: the other direction, for "both ways".
    pub fn served(&mut self, holder: usize, about: usize, t: f64) {
        if holder == about {
            return;
        }
        let r = self.row(holder, about, t);
        if r.last_served_day != Some(day(t)) {
            r.last_served_day = Some(day(t));
            r.served_days += 1.0;
            r.served_day_list.push(day(t));
        }
        self.update(holder, about, t);
    }

    /// Recomputes `holder`'s layer for `about`. Each layer is reached
    /// through the one before, at its mark, and kept while the faded
    /// standing is at half the mark or more. A layer it qualifies for now
    /// is never lost in the same step.
    pub fn update(&mut self, holder: usize, about: usize, t: f64) {
        let earned_now = self.earned[holder].len();
        self.granted_at[holder].retain(|&g| t - g < 7.0 * DAY);
        let new_this_week = self.granted_at[holder].len();
        let (rule, bytes_rule) = (self.rule, self.bytes_rule);
        let room_now = earned_now < MAX_EARNED && new_this_week < NEW_EARNED_A_WEEK;
        let introduced = self.is_introduced(holder, about, t);
        let r = self.row(holder, about, t);
        let was = r.layer;
        let c = conditions(r, rule, bytes_rule, t, room_now, introduced);
        let mut layer = 0;
        if c.layer1 {
            layer = 1;
            if c.work_days {
                layer = 2;
                if c.both_ways {
                    layer = 3;
                    if c.age && c.bytes && c.room {
                        layer = 4;
                    }
                }
            }
        }
        r.layer = layer;
        if layer == 4 && was < 4 {
            // Only a node that says yes asks for a link and spends its caps.
            if self.yes[holder] {
                self.earned[holder].push((about, t));
                self.granted_at[holder].push(t);
                // It asks `about` for its endpoints, offering its own: in
                // `about`'s record of it, an offer, which meets "both ways".
                self.row(about, holder, t).offered = true;
                self.update(about, holder, t);
            }
        } else if layer < 4 && was == 4 {
            self.earned[holder].retain(|&(p, _)| p != about);
        }
    }

    /// What keeps `holder`'s record of `about` short of layer 4 at `t`, each
    /// condition judged on its own (empty if it's at layer 4).
    pub fn blocks(&self, holder: usize, about: usize, t: f64) -> Vec<Block> {
        let Some(r) = self.get(holder, about) else {
            return vec![Block::WorkDays, Block::BothWays, Block::Bytes, Block::Age];
        };
        if r.layer == 4 {
            return Vec::new();
        }
        let mut r = r.clone();
        r.fade_to(t);
        let recent = self.granted_at[holder]
            .iter()
            .filter(|&&g| t - g < 7.0 * DAY)
            .count();
        let room_now = self.earned[holder].len() < MAX_EARNED && recent < NEW_EARNED_A_WEEK;
        let introduced = self.is_introduced(holder, about, t);
        let c = conditions(&r, self.rule, self.bytes_rule, t, room_now, introduced);
        let mut out = Vec::new();
        if !c.work_days {
            out.push(Block::WorkDays);
        }
        if !c.both_ways {
            out.push(Block::BothWays);
        }
        if !c.bytes {
            out.push(Block::Bytes);
        }
        if !c.age {
            out.push(Block::Age);
        }
        if out.is_empty() && !c.room {
            out.push(Block::Caps);
        }
        out
    }

    /// The nodes `holder` holds at `layer` or more.
    pub fn known_to(&self, holder: usize, layer: u8) -> Vec<usize> {
        let mut v: Vec<usize> = self
            .rows
            .iter()
            .filter(|&(&(h, _), r)| h == holder && r.layer >= layer)
            .map(|(&(_, a), _)| a)
            .collect();
        v.sort_unstable();
        v
    }

    /// Every pair with a record either way.
    pub fn pairs(&self) -> Vec<(usize, usize)> {
        let mut v: Vec<(usize, usize)> = self
            .rows
            .keys()
            .map(|&(h, a)| (h.min(a), h.max(a)))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Fades every record to `t`, so links whose work stopped drop back.
    pub fn sweep(&mut self, t: f64) {
        // In a fixed order, so a seed always gives the same run: which
        // record is brought up to date first can decide a cap or an offer.
        let mut keys: Vec<_> = self.rows.keys().copied().collect();
        keys.sort_unstable();
        for (h, a) in keys {
            self.update(h, a, t);
        }
    }

    /// A connection is direct only when both sides allow it: each says yes
    /// to earned links and holds the other at layer 4.
    pub fn direct(&self, a: usize, b: usize) -> bool {
        self.approved[a].contains(&b)
            || (self.yes[a]
                && self.yes[b]
                && self.get(a, b).is_some_and(|r| r.layer == 4)
                && self.get(b, a).is_some_and(|r| r.layer == 4))
    }

    /// The peers `node` has a direct link with now, approved or earned.
    pub fn direct_peers(&self, node: usize) -> Vec<usize> {
        let mut v = self.approved[node].clone();
        v.extend(
            self.earned[node]
                .iter()
                .map(|&(p, _)| p)
                .filter(|&p| self.direct(node, p) && !self.approved[node].contains(&p)),
        );
        v
    }

    /// Every (holder, about) whose record is at `layer` or more.
    pub fn at_least(&self, layer: u8) -> Vec<(usize, usize)> {
        let mut v: Vec<(usize, usize)> = self
            .rows
            .iter()
            .filter(|(_, r)| r.layer >= layer)
            .map(|(&k, _)| k)
            .collect();
        v.sort_unstable();
        v
    }

    /// For each node, the highest layer any node but `without` holds it at.
    pub fn standing(&self, n: usize, without: usize) -> Vec<u8> {
        let mut best = vec![0; n];
        for (&(h, a), r) in &self.rows {
            if h != without {
                best[a] = best[a].max(r.layer);
            }
        }
        best
    }

    /// How many records are at each layer, leaving out those `node` keeps
    /// or is the subject of.
    pub fn layers_without(&self, node: usize) -> [usize; 5] {
        let mut c = [0; 5];
        for (&(h, a), r) in &self.rows {
            if h != node && a != node {
                c[r.layer as usize] += 1;
            }
        }
        c
    }

    /// The most checked bytes and work days any record holds, leaving out
    /// `node`'s, faded to when each was last touched.
    pub fn most_without(&self, node: usize) -> (f64, f64) {
        self.rows
            .iter()
            .filter(|&(&(h, a), _)| h != node && a != node)
            .fold((0.0, 0.0), |(b, d), (_, r)| {
                (b.max(r.bytes), d.max(r.work_days))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_needs_work_both_ways_for_a_month() {
        let mut r = Records::new(vec![true, true], DayRule::Window, BytesRule::Window);
        // 0 and 1 fetch 20 MB from each other every 3 days.
        for k in 0..12 {
            let t = k as f64 * 3.0 * DAY;
            r.work(0, 1, 20e6, t);
            r.served(1, 0, t);
            r.work(1, 0, 20e6, t + 3600.0);
            r.served(0, 1, t + 3600.0);
            if t < EARNED_AFTER {
                assert!(!r.direct(0, 1), "direct before 30 days, at day {}", k * 3);
            }
        }
        assert!(r.direct(0, 1));
        // Once the work stops, the link fades and drops back.
        r.sweep(36.0 * DAY + 30.0 * DAY);
        assert!(!r.direct(0, 1));
    }

    #[test]
    fn three_actual_days_reach_layer_2_though_spread_out() {
        let mut r = Records::new(vec![true, true], DayRule::Window, BytesRule::Window);
        for d in [0.0, 12.0, 24.0] {
            r.work(0, 1, 1e6, d * DAY);
        }
        // Faded, those three days come to well under 3.
        assert!(r.get(0, 1).unwrap().work_days < 2.0);
        assert_eq!(r.get(0, 1).unwrap().layer, 2);
        // Kept while it still qualifies, then while faded standing is at
        // half the mark; lost once neither holds.
        r.sweep(40.0 * DAY);
        assert_eq!(r.get(0, 1).unwrap().layer, 1);

        let mut faded = Records::new(vec![true, true], DayRule::Faded, BytesRule::Faded);
        for d in [0.0, 12.0, 24.0] {
            faded.work(0, 1, 1e6, d * DAY);
        }
        assert_eq!(faded.get(0, 1).unwrap().layer, 1);
    }

    #[test]
    fn days_older_than_30_dont_qualify() {
        let mut r = Records::new(vec![true, true], DayRule::Window, BytesRule::Window);
        for d in [0.0, 20.0, 40.0] {
            r.work(0, 1, 1e6, d * DAY);
        }
        assert_eq!(r.get(0, 1).unwrap().layer, 1);
    }

    #[test]
    fn spot_checks_put_a_follower_on_record_but_earn_nothing_direct() {
        let mut r = Records::new(vec![true, true], DayRule::Window, BytesRule::Window);
        for d in 0..91 {
            let t = d as f64 * DAY;
            for _ in 0..100 {
                r.work(0, 1, 1024.0, t);
            }
            r.served(0, 1, t);
        }
        assert_eq!(r.get(0, 1).unwrap().layer, 3);
        assert!(!r.direct(0, 1));
    }
}
