//! Machines meeting machines in the month model, with lists and
//! introductions earned ([ADR 0022]), as the [plan] sets it: a directory's
//! map, greetings within ADR 0015's 6 dials an hour, and tables within ADR
//! 0015's and 0016's limits (256 places, the 40 places of those who greet,
//! a quarter for one source). Tor's timing isn't modelled, so a dial is
//! answered at once.
//!
//! What counts as standing here is ADR 0022's work first: jobs a machine
//! carried for the node, on 3 different days, or, once its node has shown
//! the machine (its node holds the other's at layer 2), downloads on 3
//! days. `Standing` is worked out by the caller from the records, once a
//! day.
//!
//! [ADR 0022]: ../../../docs/decisions/0022-lists-and-introductions-are-earned.md
//! [plan]: ../../../docs/testing/hops-model.md

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::Serialize;

use super::records::DAY;
use super::rng::Rng;

const HOUR: f64 = 3600.0;
pub const TABLE: usize = 256;
pub const GREETERS: usize = 40;
pub const ONE_SOURCE: usize = TABLE / 4;
const DIALS_AN_HOUR: usize = 6;
/// Of the 6, one goes to a machine that greeted and hasn't been dialled
/// (ADR 0016's turns).
const GREETER_TURNS: usize = 1;
/// `Known`: up to 20 records, once a day to one machine.
const LIST_SIZE: usize = 20;
/// ADR 0022's starting values for introductions.
const INTRODUCTIONS_EACH: usize = 3;
const INTRODUCTIONS_A_DAY: usize = 12;
const COUNTED_FROM_ONE: usize = 8;
const INTRODUCTION_LASTS: f64 = 14.0 * DAY;
/// The plan: the directory goes off on day 56.
pub const DIRECTORY_OFF: f64 = 56.0 * DAY;

/// Where a record in a table came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Src {
    /// The directory's map: one source.
    Map,
    /// Given by a friend, for a newcomer started from one machine.
    Given,
    /// It greeted unasked.
    Greeted,
    /// Passed on by a machine first met through its greeting: counts against
    /// those who greet (the crowd limit).
    PassedOn,
    /// Introduced: counts among those who greet (ADR 0022).
    Introduced,
    /// Passed on in a list by this machine.
    From(usize),
    /// Did work this node checked: out of the places of those who greet.
    Worked,
}

impl Src {
    fn greeter(self) -> bool {
        matches!(self, Src::Greeted | Src::PassedOn | Src::Introduced)
    }
}

#[derive(Clone, Debug)]
struct Entry {
    src: Src,
    since: f64,
}

/// Who's in the run, as meeting needs it.
pub struct Who {
    pub arrives: Vec<f64>,
    pub leaves: Vec<Option<f64>>,
    pub operator: Vec<bool>,
    /// A newcomer started from one machine: that machine.
    pub given: Vec<Option<usize>>,
}

impl Who {
    fn present(&self, m: usize, t: f64) -> bool {
        self.arrives[m] <= t && self.leaves[m].is_none_or(|l| t < l)
    }
}

/// (holder, about): `holder` holds `about` at standing (work on 3 days).
/// Also used for (asker, asked): the asker's own record gives it a reason to
/// think it has standing with the machine asked.
pub type Standing = HashSet<(usize, usize)>;

/// When an honest machine asks another for its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
pub enum Asking {
    /// Every machine in its table, every day: the aggressive baseline of
    /// the first run (2026-11-09).
    EveryDay,
    /// Only machines its own record gives it a reason to (it served them on
    /// 3 days in the last 30, or was the hop next to them on 3 days), at
    /// most once a day each, and only while its table has room: the
    /// once-a-day limit is a maximum, not a schedule (the maintainer,
    /// 2026-11-09).
    WhenReason,
}

/// What meeting did, for the report.
#[derive(Clone, Debug, Default, Serialize)]
pub struct MeetCounts {
    pub greetings: usize,
    pub lists_asked: usize,
    pub lists_refused: usize,
    pub lists_given: usize,
    pub introductions_sent: usize,
    /// Of those, counted by the machine they went to (at most 8 from one
    /// signer at a time).
    pub introductions_counted: usize,
    /// Lists and introductions that went to the operator's machines, and
    /// lists they asked for.
    pub operator_lists: usize,
    pub operator_lists_asked: usize,
    pub operator_introduced: usize,
}

pub struct Meeting {
    who: Who,
    tables: Vec<BTreeMap<usize, Entry>>,
    /// Machines each has dialled (greeted).
    dialled: Vec<HashSet<usize>>,
    /// Machines that greeted the directory, in that order.
    map: Vec<usize>,
    on_map: Vec<bool>,
    joined: Vec<bool>,
    /// Seconds from the start the model has reached, a whole hour.
    at: f64,
    day: i64,
    rng: Rng,
    /// (Alice, Jimmy): introductions Alice sent of Jimmy, and to whom.
    sent: HashMap<(usize, usize), Vec<usize>>,
    /// (Kim, Jimmy): Kim counts an introduction of Jimmy by Alice until then.
    counted: HashMap<(usize, usize), (usize, f64)>,
    /// (Kim, Alice): when each introduction Kim counts from Alice runs out.
    from_signer: HashMap<(usize, usize), Vec<f64>>,
    /// (Alice, asker): the last day Alice gave the asker her list.
    listed_on: HashMap<(usize, usize), i64>,
    /// Introductions to apply to the records: (Kim, Jimmy, until).
    pub to_count: Vec<(usize, usize, f64)>,
    pub first_list: Vec<Option<f64>>,
    pub first_introduced: Vec<Option<f64>>,
    /// A list or introduction naming a machine not in its table, after the
    /// directory went off (mark 8).
    pub heard_new_late: Vec<bool>,
    pub counts: MeetCounts,
    pub asking: Asking,
    /// Machines each has a reason, from its own record, to ask for a list.
    reasons: Vec<Vec<usize>>,
}

impl Meeting {
    pub fn new(who: Who, seed: u64) -> Self {
        let n = who.arrives.len();
        Meeting {
            who,
            tables: vec![BTreeMap::new(); n],
            dialled: vec![HashSet::new(); n],
            map: Vec::new(),
            on_map: vec![false; n],
            joined: vec![false; n],
            at: 0.0,
            day: -1,
            rng: Rng::new(seed),
            sent: HashMap::new(),
            counted: HashMap::new(),
            from_signer: HashMap::new(),
            listed_on: HashMap::new(),
            to_count: Vec::new(),
            first_list: vec![None; n],
            first_introduced: vec![None; n],
            heard_new_late: vec![false; n],
            counts: MeetCounts::default(),
            asking: Asking::WhenReason,
            reasons: vec![Vec::new(); n],
        }
    }

    /// `m` is in `v`'s table.
    pub fn knows(&self, v: usize, m: usize) -> bool {
        self.tables[v].contains_key(&m)
    }

    /// `v` counts an introduction of `m` now.
    pub fn introduced(&self, v: usize, m: usize, t: f64) -> bool {
        self.counted.get(&(v, m)).is_some_and(|&(_, u)| t < u)
    }

    pub fn table_len(&self, v: usize) -> usize {
        self.tables[v].len()
    }

    fn count(&self, v: usize, f: impl Fn(Src) -> bool) -> usize {
        self.tables[v].values().filter(|e| f(e.src)).count()
    }

    /// Puts `m` in `v`'s table within the limits; whether it went in.
    fn insert(&mut self, v: usize, m: usize, src: Src, t: f64) -> bool {
        if v == m || self.knows(v, m) {
            return false;
        }
        let greeters = self.count(v, Src::greeter);
        let others = self.tables[v].len() - greeters;
        match src {
            Src::Greeted | Src::Introduced => {
                if greeters >= GREETERS {
                    // A greeting first: a record a greeter passed on goes,
                    // then the greeter that took its place longest ago.
                    let oldest = |want: fn(Src) -> bool| {
                        self.tables[v]
                            .iter()
                            .filter(|(_, e)| want(e.src))
                            .min_by(|a, b| a.1.since.total_cmp(&b.1.since))
                            .map(|(&k, _)| k)
                    };
                    let out = oldest(|s| s == Src::PassedOn)
                        .or_else(|| oldest(|s| matches!(s, Src::Greeted | Src::Introduced)));
                    if let Some(out) = out {
                        self.tables[v].remove(&out);
                    }
                }
            }
            Src::PassedOn => {
                if greeters >= GREETERS {
                    return false;
                }
            }
            Src::Worked => {
                if others >= TABLE - GREETERS {
                    return false;
                }
            }
            Src::Map | Src::Given | Src::From(_) => {
                if others >= TABLE - GREETERS || self.count(v, |s| s == src) >= ONE_SOURCE {
                    return false;
                }
            }
        }
        self.tables[v].insert(m, Entry { src, since: t });
        true
    }

    /// `v` greets `m`: `m` holds `v` among those who greet.
    fn greet(&mut self, v: usize, m: usize, t: f64) {
        self.dialled[v].insert(m);
        self.counts.greetings += 1;
        if self.who.present(m, t) {
            self.insert(m, v, Src::Greeted, t);
        }
    }

    /// Runs meeting up to `to`, an hour at a time; `standing` is brought up
    /// to date by the caller for each day.
    pub fn advance(&mut self, to: f64, standing: &mut dyn FnMut(f64) -> (Standing, Standing)) {
        while self.at <= to {
            let t = self.at;
            self.arrivals(t);
            let day = (t / DAY).floor() as i64;
            if day != self.day {
                self.day = day;
                let (s, reasons) = standing(t);
                let mut by_asker = vec![Vec::new(); self.tables.len()];
                for (v, m) in reasons {
                    by_asker[v].push(m);
                }
                for r in &mut by_asker {
                    r.sort_unstable();
                }
                self.reasons = by_asker;
                self.daily(t, &s);
            }
            self.dials(t);
            self.at += HOUR;
        }
    }

    fn arrivals(&mut self, t: f64) {
        let n = self.tables.len();
        let new: Vec<usize> = (0..n)
            .filter(|&m| !self.joined[m] && self.who.present(m, t))
            .collect();
        if new.is_empty() {
            return;
        }
        let directory = t < DIRECTORY_OFF;
        for &m in &new {
            self.joined[m] = true;
            if directory && self.who.given[m].is_none() {
                self.on_map[m] = true;
                self.map.push(m);
            }
        }
        for &m in &new {
            if let Some(g) = self.who.given[m] {
                self.insert(m, g, Src::Given, t);
            } else if directory {
                self.read_map(m, t);
            }
        }
    }

    /// `m` takes records from the map, within the directory's quarter.
    fn read_map(&mut self, m: usize, t: f64) {
        let mut map: Vec<usize> = self
            .map
            .iter()
            .copied()
            .filter(|&x| x != m && !self.knows(m, x) && self.who.present(x, t))
            .collect();
        self.rng.shuffle(&mut map);
        for x in map {
            if self.count(m, |s| s == Src::Map) >= ONE_SOURCE {
                break;
            }
            self.insert(m, x, Src::Map, t);
        }
    }

    /// Each machine's 6 dials this hour: one to a machine that greeted and
    /// hasn't been dialled, the rest to those heard of longest ago. The
    /// operator also greets every machine on the map.
    fn dials(&mut self, t: f64) {
        let n = self.tables.len();
        for v in 0..n {
            if !self.who.present(v, t) {
                continue;
            }
            let waiting = |me: &Self, greeters: bool| {
                let mut w: Vec<(f64, usize)> = me.tables[v]
                    .iter()
                    .filter(|&(&m, e)| e.src.greeter() == greeters && !me.dialled[v].contains(&m))
                    .map(|(&m, e)| (e.since, m))
                    .collect();
                w.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                w
            };
            let mut left = DIALS_AN_HOUR;
            let turns = waiting(self, true);
            let mut picks: Vec<usize> = Vec::new();
            if !turns.is_empty() {
                for _ in 0..GREETER_TURNS {
                    picks.push(turns[self.rng.below(turns.len())].1);
                }
            }
            left -= picks.len();
            for (_, m) in waiting(self, false).into_iter().chain(turns) {
                if left == 0 {
                    break;
                }
                if !picks.contains(&m) {
                    picks.push(m);
                    left -= 1;
                }
            }
            if self.who.operator[v] && left > 0 && t < DIRECTORY_OFF {
                let more: Vec<usize> = self
                    .map
                    .iter()
                    .copied()
                    .filter(|&m| m != v && !self.dialled[v].contains(&m) && !picks.contains(&m))
                    .take(left)
                    .collect();
                picks.extend(more);
            }
            for m in picks {
                self.greet(v, m, t);
            }
        }
    }

    fn daily(&mut self, t: f64, standing: &Standing) {
        let n = self.tables.len();
        // Who each holds at standing, and who holds each.
        let mut holds: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut held_by: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut pairs: Vec<&(usize, usize)> = standing.iter().collect();
        pairs.sort_unstable();
        for &&(h, a) in &pairs {
            if self.who.present(h, t) && self.who.present(a, t) {
                holds[h].push(a);
                held_by[a].push(h);
            }
        }
        // Machines that left are forgotten.
        for v in 0..n {
            let gone: Vec<usize> = self.tables[v]
                .keys()
                .copied()
                .filter(|&m| !self.who.present(m, t) && self.joined[m])
                .collect();
            for m in gone {
                self.tables[v].remove(&m);
            }
        }
        // Work done lets a machine out of the places of those who greet,
        // and puts one this node's machine worked with in its table.
        for (v, held) in holds.iter().enumerate() {
            if !self.who.present(v, t) {
                continue;
            }
            for &m in held {
                match self.tables[v].get(&m).map(|e| e.src) {
                    Some(s) if s.greeter() => {
                        let greeters = self.count(v, Src::greeter);
                        if self.tables[v].len() - greeters < TABLE - GREETERS {
                            self.tables[v].get_mut(&m).unwrap().src = Src::Worked;
                        }
                    }
                    Some(_) => {}
                    None => {
                        self.insert(v, m, Src::Worked, t);
                    }
                }
            }
        }
        // With the directory on, machines read the map again for those
        // that greeted it since.
        if t < DIRECTORY_OFF {
            for v in 0..n {
                if self.who.present(v, t) && self.on_map[v] {
                    self.read_map(v, t);
                }
            }
        }
        self.lists(t, &holds);
        self.introductions(t, &holds, &held_by);
    }

    /// Who asks whom for a list (`Asking`); Alice answers only one that
    /// worked for her on 3 days, once a day. The operator's machines ask
    /// every machine in their table every day, as the plan has them ask for
    /// every list they can.
    fn lists(&mut self, t: f64, holds: &[Vec<usize>]) {
        let n = self.tables.len();
        let day = (t / DAY).floor() as i64;
        let mut order: Vec<usize> = (0..n).filter(|&v| self.who.present(v, t)).collect();
        self.rng.shuffle(&mut order);
        for v in order {
            let poll = self.asking == Asking::EveryDay || self.who.operator[v];
            if !poll && self.tables[v].len() >= TABLE {
                // A full table needs no new contacts.
                continue;
            }
            let asked: Vec<usize> = if poll {
                self.tables[v]
                    .keys()
                    .copied()
                    .filter(|&m| self.who.present(m, t))
                    .collect()
            } else {
                self.reasons[v]
                    .iter()
                    .copied()
                    .filter(|&m| self.who.present(m, t))
                    .collect()
            };
            for alice in asked {
                self.counts.lists_asked += 1;
                if self.who.operator[v] {
                    self.counts.operator_lists_asked += 1;
                }
                let earned = holds[alice].binary_search(&v).is_ok();
                if !earned || self.listed_on.get(&(alice, v)) == Some(&day) {
                    self.counts.lists_refused += 1;
                    continue;
                }
                self.listed_on.insert((alice, v), day);
                let mut list: Vec<usize> = self.tables[alice]
                    .keys()
                    .copied()
                    .filter(|&x| x != v && self.who.present(x, t))
                    .collect();
                self.rng.shuffle(&mut list);
                list.truncate(LIST_SIZE);
                if list.is_empty() {
                    continue;
                }
                self.counts.lists_given += 1;
                if self.who.operator[v] {
                    self.counts.operator_lists += 1;
                }
                self.first_list[v].get_or_insert(t);
                let src = match self.tables[v].get(&alice).map(|e| e.src) {
                    Some(s) if s.greeter() => Src::PassedOn,
                    _ => Src::From(alice),
                };
                for x in list {
                    if !self.knows(v, x) && x != v {
                        if t >= DIRECTORY_OFF {
                            self.heard_new_late[v] = true;
                        }
                        self.insert(v, x, src, t);
                    }
                }
            }
        }
    }

    /// Alice introduces machines that worked for her on 3 days to machines
    /// that accept hers, those she worked for on 3 days: at most 3 for one
    /// machine, 12 a day. Kim counts at most 8 from one signer at a time,
    /// and holds the machine among those who greet.
    fn introductions(&mut self, t: f64, holds: &[Vec<usize>], held_by: &[Vec<usize>]) {
        let n = self.tables.len();
        let mut order: Vec<usize> = (0..n).filter(|&a| self.who.present(a, t)).collect();
        self.rng.shuffle(&mut order);
        for alice in order {
            let accepting = &held_by[alice];
            if accepting.is_empty() {
                continue;
            }
            let mut jimmies: Vec<usize> = holds[alice]
                .iter()
                .copied()
                .filter(|j| {
                    self.sent
                        .get(&(alice, *j))
                        .is_none_or(|s| s.len() < INTRODUCTIONS_EACH)
                })
                .collect();
            self.rng.shuffle(&mut jimmies);
            jimmies.sort_by_key(|j| self.sent.get(&(alice, *j)).map_or(0, |s| s.len()));
            let mut budget = INTRODUCTIONS_A_DAY;
            for jimmy in jimmies {
                if budget == 0 {
                    break;
                }
                let already = self.sent.get(&(alice, jimmy)).cloned().unwrap_or_default();
                let to: Vec<usize> = accepting
                    .iter()
                    .copied()
                    .filter(|&k| k != jimmy && !already.contains(&k))
                    .collect();
                if to.is_empty() {
                    continue;
                }
                let kim = to[self.rng.below(to.len())];
                budget -= 1;
                self.sent.entry((alice, jimmy)).or_default().push(kim);
                self.counts.introductions_sent += 1;
                self.first_introduced[jimmy].get_or_insert(t);
                if self.who.operator[jimmy] {
                    self.counts.operator_introduced += 1;
                }
                let from_alice = self.from_signer.entry((kim, alice)).or_default();
                from_alice.retain(|&u| t < u);
                if from_alice.len() >= COUNTED_FROM_ONE {
                    continue;
                }
                let until = t + INTRODUCTION_LASTS;
                from_alice.push(until);
                self.counts.introductions_counted += 1;
                self.counted.insert((kim, jimmy), (alice, until));
                self.to_count.push((kim, jimmy, until));
                if !self.knows(kim, jimmy) {
                    if t >= DIRECTORY_OFF {
                        self.heard_new_late[kim] = true;
                    }
                    self.insert(kim, jimmy, Src::Introduced, t);
                }
            }
        }
    }

    /// Machines in `v`'s table, in order.
    pub fn table(&self, v: usize) -> impl Iterator<Item = usize> + '_ {
        self.tables[v].keys().copied()
    }

    /// Distinct machines a node's table holds, for checks.
    pub fn sources_ok(&self, v: usize) -> bool {
        let greeters = self.count(v, Src::greeter);
        let others = self.tables[v].len() - greeters;
        let mut per: BTreeMap<String, usize> = BTreeMap::new();
        for e in self.tables[v].values() {
            if matches!(e.src, Src::Map | Src::Given | Src::From(_)) {
                *per.entry(format!("{:?}", e.src)).or_default() += 1;
            }
        }
        greeters <= GREETERS
            && others <= TABLE - GREETERS
            && per.values().all(|&c| c <= ONE_SOURCE)
            && self.tables[v].keys().collect::<BTreeSet<_>>().len() == self.tables[v].len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(n: usize) -> Who {
        Who {
            arrives: vec![0.0; n],
            leaves: vec![None; n],
            operator: vec![false; n],
            given: vec![None; n],
        }
    }

    #[test]
    fn strangers_get_no_list_and_tables_keep_their_limits() {
        let mut m = Meeting::new(who(300), 1);
        m.asking = Asking::EveryDay;
        m.advance(3.0 * DAY, &mut |_| (Standing::new(), Standing::new()));
        assert!(m.first_list.iter().all(|l| l.is_none()));
        assert!(m.counts.lists_asked > 0);
        assert_eq!(m.counts.lists_refused, m.counts.lists_asked);
        for v in 0..300 {
            assert!(m.sources_ok(v), "table {v} over its limits");
            assert!(
                m.table_len(v) > ONE_SOURCE,
                "table {v} only {}",
                m.table_len(v)
            );
        }
    }

    #[test]
    fn work_gets_a_list_and_introductions_only_to_those_accepting() {
        let mut m = Meeting::new(who(10), 1);
        // 1 worked for 0, and 0 for 2: 0 gives 1 its list and introduces 1
        // to 2, the only machine that accepts 0's.
        let s: Standing = [(0, 1), (2, 0)].into_iter().collect();
        // 1's own record gives it a reason to ask 0.
        let r: Standing = [(1, 0)].into_iter().collect();
        m.advance(1.0 * DAY, &mut |_| (s.clone(), r.clone()));
        assert!(m.first_list[1].is_some());
        assert!(m.first_list[3].is_none());
        assert!(m.first_introduced[1].is_some());
        assert!(m.introduced(2, 1, DAY));
        assert!(!m.introduced(3, 1, DAY));
        assert_eq!(m.counts.introductions_sent, 1);
    }

    #[test]
    fn a_machine_asks_only_those_it_has_a_reason_to() {
        let mut m = Meeting::new(who(50), 1);
        let r: Standing = [(1, 0)].into_iter().collect();
        m.advance(2.0 * DAY, &mut |_| (Standing::new(), r.clone()));
        // Days 0, 1 and 2: one ask a day, from 1 to 0, refused.
        assert_eq!(m.counts.lists_asked, 3);
        assert_eq!(m.counts.lists_refused, 3);
    }
}
