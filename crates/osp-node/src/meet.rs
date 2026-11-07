//! Machines meeting machines (ADR 0015's step 3, with ADR 0016's three rules):
//! the table of machines a node's machine name knows, its local record of
//! each, and the background work of meeting them.
//!
//! A machine is heard of when a signed record of it arrives: in a greeting, in
//! another machine's `Known` reply, or on a directory's map. It's met once it
//! answers when greeted, and known after answering for a month with records
//! that checked out. Being known grants nothing: a known machine is checked in
//! with first, and kept when the table is full.
//!
//! Everything here comes from what the machine itself saw. Nothing reads the
//! owner's follows, feed, media, searches or peers, so nothing they do changes
//! what a machine says, and the record never leaves the node. Machines' IDs
//! aren't logged, as a log also holds the public ID.

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use iroh::EndpointId;
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_proto::machine::{MAX_KNOWN, MachineRecord};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::{
    db::{Db, now_ms},
    node::Core,
    sync::{jitter, random_fraction},
};

/// Most machines kept (ADR 0015).
pub(crate) const MAX_MACHINES: usize = 256;
/// Places held for the machines known longest, so a crowd of new names can't
/// push them out.
pub(crate) const KEPT_LONGEST: usize = MAX_MACHINES / 2;
/// Most of the other places that records from one source can fill.
pub(crate) const PER_SOURCE: usize = (MAX_MACHINES - KEPT_LONGEST) / 4;
/// Places beyond those limits that machines greeting this one take in turn,
/// so a newcomer still gets a place when those who greet have filled their
/// share (ADR 0016).
pub(crate) const GREETER_TURNS: usize = 8;
/// New machines dialled an hour, at most.
pub(crate) const DIALS_PER_HOUR: usize = 6;
/// One dial in this many goes to a record from the source that supplied the
/// fewest of the table's machines (ADR 0016), so that a source heard from less
/// still gets a turn: 2 of the 6.
pub(crate) const LESS_HEARD_EVERY: usize = 3;
/// New machines dialled in one round, at most, so an hour's dials spread out.
const DIALS_PER_ROUND: usize = 2;
/// Machines checked in with in one round, at most.
const CHECK_INS_PER_ROUND: usize = 4;
/// Conversations at once.
const AT_ONCE: usize = 4;
/// A machine heard of is dialled this many times before it's forgotten, and
/// its source is taken to have passed on a record that didn't check out.
pub(crate) const MAX_TRIES: u32 = 3;
/// A record dated longer ago than this isn't current: it isn't kept, and
/// whoever passed it on passed on something stale. Machines date their record
/// afresh whenever they greet, and pass on those of machines met lately.
const RECORD_MAX_AGE_MS: i64 = 30 * DAY_MS;
/// Or dated this far ahead of our clock.
const RECORD_MAX_AHEAD_MS: i64 = 5 * 60 * 1000;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// How long things take among machines. A real node always has the defaults;
/// the simulator and tests run them faster (`SimSettings::meeting`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeetingTimes {
    /// The hour in "new machines dialled an hour".
    pub hour: Duration,
    /// About how often a node looks for machines to dial or check in with.
    pub round: Duration,
    /// How long before a machine heard of that didn't answer is tried again.
    pub try_again_after: Duration,
    /// About how often each machine met is checked in with.
    pub check_in_every: Duration,
    /// A machine that answered within this long counts as met lately, and
    /// may be named in a `Known` reply.
    pub met_lately: Duration,
    /// A known machine has answered within this long.
    pub still_answering: Duration,
    /// A machine met that hasn't answered for this long is forgotten.
    pub forget_silent: Duration,
    /// Answering for this long, with records that checked out, makes a
    /// machine known.
    pub known_after: Duration,
    /// The record's counts halve in this long, as in ADR 0011.
    pub half_life: Duration,
}

impl Default for MeetingTimes {
    fn default() -> Self {
        const HOUR: Duration = Duration::from_secs(60 * 60);
        const DAY: Duration = Duration::from_secs(24 * 60 * 60);
        Self {
            hour: HOUR,
            round: Duration::from_secs(10 * 60),
            try_again_after: HOUR * 6,
            check_in_every: DAY,
            met_lately: DAY * 7,
            still_answering: DAY * 3,
            forget_silent: DAY * 30,
            known_after: DAY * 30,
            half_life: DAY * 14,
        }
    }
}

impl MeetingTimes {
    /// All of them `times` as fast.
    pub fn faster(self, times: f64) -> Self {
        let f = |d: Duration| d.div_f64(times);
        Self {
            hour: f(self.hour),
            round: f(self.round),
            try_again_after: f(self.try_again_after),
            check_in_every: f(self.check_in_every),
            met_lately: f(self.met_lately),
            still_answering: f(self.still_answering),
            forget_silent: f(self.forget_silent),
            known_after: f(self.known_after),
            half_life: f(self.half_life),
        }
    }
}

fn ms(d: Duration) -> i64 {
    d.as_millis().min(i64::MAX as u128) as i64
}

/// Where a machine's record first came from. It stays counted against that
/// source after the machine is met (ADR 0016), so meeting a machine doesn't
/// free a place in its source's share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    /// It greeted this machine unasked. All of these count as one source, as
    /// anyone can greet under as many names as they like.
    Greeted,
    /// Its record came from this machine: in a `Known` reply, or on the map
    /// of a directory.
    Machine(EndpointId),
}

impl Source {
    pub fn to_db(self) -> String {
        match self {
            Source::Greeted => "greeted".to_string(),
            Source::Machine(machine) => machine.to_string(),
        }
    }

    pub fn from_db(s: &str) -> Result<Self> {
        Ok(match s {
            "greeted" => Source::Greeted,
            machine => Source::Machine(machine.parse().context("corrupt machine source")?),
        })
    }
}

/// How far a machine has got (ADR 0015's stages; "introduced" comes later).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// A signed record of it arrived. It may be dialled.
    Heard,
    /// It answered.
    Met,
    /// It has answered for a month, and the records it passed on checked out.
    Known,
}

/// What a machine did, faded as in ADR 0011 so old behaviour stops counting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Counts {
    /// It answered when greeted, or didn't.
    #[serde(default)]
    pub answers: f64,
    #[serde(default)]
    pub misses: f64,
    /// Records it was the first to pass on whose machine answered when
    /// dialled, and those that were invalid or stale, or whose machine never
    /// answered.
    #[serde(default)]
    pub good: f64,
    #[serde(default)]
    pub bad: f64,
}

/// One machine in the table.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub machine: EndpointId,
    /// Its latest record, as it signed it.
    pub record: MachineRecord,
    /// That record's date.
    pub published_ms: i64,
    pub source: Source,
    pub heard_ms: i64,
    /// When it first answered.
    pub met_ms: Option<i64>,
    /// When it was last dialled, or 0.
    pub tried_ms: i64,
    /// Dials in a row it hasn't answered.
    pub tries: u32,
    pub answered_ms: Option<i64>,
    pub counts: Counts,
    /// When the counts were last faded.
    pub faded_ms: i64,
}

impl Entry {
    fn fade(&mut self, now: i64, half_life: i64) {
        if now > self.faded_ms && half_life > 0 {
            let factor = 0.5f64.powf((now - self.faded_ms) as f64 / half_life as f64);
            let c = &mut self.counts;
            for count in [&mut c.answers, &mut c.misses, &mut c.good, &mut c.bad] {
                *count *= factor;
            }
        }
        self.faded_ms = self.faded_ms.max(now);
    }
}

/// What became of a record heard of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Heard {
    Added,
    /// The table has it already; a newer record replaces the one it had.
    Already,
    /// This machine, or a directory in `config.toml`, which are kept apart.
    Apart,
    /// Its source has filled its share.
    SourceFull,
    /// The table is full, and nothing gives up its place for it.
    Full,
    /// It greeted this machine, and took a turn (`GREETER_TURNS`).
    Turn,
}

/// Rows to write after a change.
#[derive(Debug, Default)]
pub(crate) struct Changes {
    pub put: Vec<Entry>,
    pub removed: Vec<EndpointId>,
    /// The hour's dials, when they changed.
    pub dials: Option<Vec<Dial>>,
}

/// A new machine dialled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Dial {
    pub at_ms: i64,
    /// Whether it went to the source heard from less (ADR 0016's rule 2).
    pub less_heard: bool,
    /// Whether it went to a machine that greeted this one unasked.
    pub greeted_back: bool,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.put.is_empty() && self.removed.is_empty() && self.dials.is_none()
    }
}

/// What a round of meeting will do.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Plan {
    /// Machines heard of, to greet for the first time or again.
    pub dials: Vec<EndpointId>,
    /// Machines met, to check in with.
    pub check_ins: Vec<EndpointId>,
}

/// The machines a node knows, and its record of each. The limits hold
/// whatever arrives, and after a restart, as they're applied to what's
/// stored.
pub(crate) struct Table {
    entries: HashMap<EndpointId, Entry>,
    /// The last hour's new machines dialled.
    dials: Vec<Dial>,
    /// This machine, and the directories in `config.toml`: never in the
    /// table, so a full one can't push the directories out (ADR 0016).
    apart: HashSet<EndpointId>,
    times: MeetingTimes,
    changed: HashSet<EndpointId>,
    removed: HashSet<EndpointId>,
    dials_changed: bool,
}

impl Table {
    pub fn new(apart: HashSet<EndpointId>, times: MeetingTimes) -> Self {
        Self {
            entries: HashMap::new(),
            dials: Vec::new(),
            apart,
            times,
            changed: HashSet::new(),
            removed: HashSet::new(),
            dials_changed: false,
        }
    }

    /// The table as stored, with the limits applied again, as they may have
    /// changed, or a machine stored may since have been made a directory.
    pub fn load(
        stored: Vec<Entry>,
        dials: Vec<Dial>,
        apart: HashSet<EndpointId>,
        times: MeetingTimes,
        now: i64,
    ) -> Self {
        let mut table = Self::new(apart, times);
        table.dials = dials;
        for entry in stored {
            if table.apart.contains(&entry.machine) {
                table.removed.insert(entry.machine);
            } else {
                table.entries.insert(entry.machine, entry);
            }
        }
        while table.entries.len() > MAX_MACHINES + GREETER_TURNS {
            let protected = table.protected(now);
            let Some(worst) = table
                .entries
                .values()
                .filter(|e| !protected.contains(&e.machine))
                .min_by_key(|e| (e.met_ms.is_some(), e.answered_ms, e.heard_ms))
                .map(|e| e.machine)
            else {
                break;
            };
            table.remove(&worst);
        }
        table
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn get(&self, machine: &EndpointId) -> Option<&Entry> {
        self.entries.get(machine)
    }

    pub fn stage(&self, entry: &Entry, now: i64) -> Stage {
        let Some(met) = entry.met_ms else {
            return Stage::Heard;
        };
        let t = &self.times;
        let c = &entry.counts;
        let answering = entry
            .answered_ms
            .is_some_and(|at| at >= now - ms(t.still_answering));
        // One record that didn't check out is forgiven, as a machine can go
        // for good between being passed on and being dialled.
        let checked_out = c.bad <= c.good + 1.0;
        if met <= now - ms(t.known_after) && answering && c.answers > c.misses && checked_out {
            Stage::Known
        } else {
            Stage::Met
        }
    }

    /// The places held for the machines known longest: known machines, those
    /// met first.
    pub fn protected(&self, now: i64) -> HashSet<EndpointId> {
        let mut known: Vec<&Entry> = self
            .entries
            .values()
            .filter(|e| self.stage(e, now) == Stage::Known)
            .collect();
        known.sort_by_key(|e| (e.met_ms, e.machine));
        known
            .into_iter()
            .take(KEPT_LONGEST)
            .map(|e| e.machine)
            .collect()
    }

    /// How many of the other places each source's records fill.
    fn shares(&self, protected: &HashSet<EndpointId>) -> HashMap<Source, usize> {
        let mut shares = HashMap::new();
        for entry in self.entries.values() {
            if !protected.contains(&entry.machine) {
                *shares.entry(entry.source).or_default() += 1;
            }
        }
        shares
    }

    /// A signed record of a machine arrived from `source`. An invalid or
    /// stale one is an error, which counts against whoever passed it on.
    pub fn hear(&mut self, record: &MachineRecord, source: Source, now: i64) -> Result<Heard> {
        let info = record.verify()?;
        let published =
            osp_proto::parse_rfc3339_ms(&info.published).context("a machine record has no date")?;
        ensure!(
            published <= now + RECORD_MAX_AHEAD_MS,
            "a machine record dated ahead of our clock"
        );
        ensure!(
            published >= now - RECORD_MAX_AGE_MS,
            "a machine record too old to be current"
        );
        let machine = info.machine;
        if self.apart.contains(&machine) {
            return Ok(Heard::Apart);
        }
        if let Some(entry) = self.entries.get_mut(&machine) {
            if published > entry.published_ms {
                entry.record = record.clone();
                entry.published_ms = published;
                self.changed.insert(machine);
            }
            return Ok(Heard::Already);
        }
        // Mostly there's room, and the source is nowhere near its share.
        let from_source = self.entries.values().filter(|e| e.source == source).count();
        if from_source < PER_SOURCE && self.entries.len() < MAX_MACHINES {
            self.insert(record, machine, published, source, now);
            return Ok(Heard::Added);
        }
        let protected = self.protected(now);
        let shares = self.shares(&protected);
        let share = shares.get(&source).copied().unwrap_or(0);
        if share >= PER_SOURCE {
            return Ok(match source {
                Source::Greeted => self.take_turn(record, machine, published, &protected, now),
                Source::Machine(_) => Heard::SourceFull,
            });
        }
        if self.entries.len() >= MAX_MACHINES {
            // Only a machine never met gives up its place, so a crowd of
            // records can't push out a machine that answers. And only one from
            // a source with at least two more places than this one, the
            // oldest heard of from the source with most, so sources even out.
            let given_up = self
                .entries
                .values()
                .filter(|e| e.met_ms.is_none())
                .filter(|e| shares.get(&e.source).copied().unwrap_or(0) > share + 1)
                .max_by_key(|e| (shares.get(&e.source).copied(), Reverse(e.heard_ms)))
                .map(|e| e.machine);
            match given_up {
                Some(other) => self.remove(&other),
                None if source == Source::Greeted => {
                    return Ok(self.take_turn(record, machine, published, &protected, now));
                }
                None => return Ok(Heard::Full),
            }
        }
        self.insert(record, machine, published, source, now);
        Ok(Heard::Added)
    }

    /// A machine that greeted this one, with no place for it otherwise, takes
    /// one of `GREETER_TURNS` places beyond the limits, in place of the greeter
    /// that took one longest ago. Met or not, each gives its place up in turn,
    /// so a crowd greeting under many names can't keep them.
    fn take_turn(
        &mut self,
        record: &MachineRecord,
        machine: EndpointId,
        published: i64,
        protected: &HashSet<EndpointId>,
        now: i64,
    ) -> Heard {
        let mut greeters: Vec<&Entry> = self
            .entries
            .values()
            .filter(|e| e.source == Source::Greeted && !protected.contains(&e.machine))
            .collect();
        greeters.sort_by_key(|e| (e.heard_ms, e.machine));
        // Those heard of last, as many as are over the share or the table's size.
        let turns = (greeters.len().saturating_sub(PER_SOURCE))
            .max(self.entries.len().saturating_sub(MAX_MACHINES))
            .min(greeters.len());
        let gives_up = greeters[greeters.len() - turns..]
            .first()
            .filter(|_| turns >= GREETER_TURNS)
            .map(|e| e.machine);
        if let Some(other) = gives_up {
            self.remove(&other);
        } else if self.entries.len() >= MAX_MACHINES + GREETER_TURNS {
            return Heard::Full;
        }
        self.insert(record, machine, published, Source::Greeted, now);
        Heard::Turn
    }

    fn insert(
        &mut self,
        record: &MachineRecord,
        machine: EndpointId,
        published: i64,
        source: Source,
        now: i64,
    ) {
        self.entries.insert(
            machine,
            Entry {
                machine,
                record: record.clone(),
                published_ms: published,
                source,
                heard_ms: now,
                met_ms: None,
                tried_ms: 0,
                tries: 0,
                answered_ms: None,
                counts: Counts::default(),
                faded_ms: now,
            },
        );
        self.changed.insert(machine);
    }

    /// Picks this round's dials and check-ins, and notes them as tried, so
    /// they're not picked again while they're under way.
    pub fn plan(&mut self, now: i64) -> Plan {
        self.sweep(now);
        let mut plan = Plan::default();
        let hour_ago = now - ms(self.times.hour);
        let before = self.dials.len();
        self.dials.retain(|dial| dial.at_ms > hour_ago);
        self.dials_changed |= self.dials.len() != before;
        while self.dials.len() < DIALS_PER_HOUR && plan.dials.len() < DIALS_PER_ROUND {
            // One an hour goes to a machine that greeted this one, so a
            // newcomer isn't left behind every machine heard of before it.
            let greeter = match self.dials.iter().any(|d| d.greeted_back) {
                false => self.next_greeter(now),
                true => None,
            };
            // While those to the source heard from less are no more than a
            // third of the hour's, the next is one: the 1st and 4th of a fresh
            // hour, and still 2 of 6 as the hour slides on.
            let less_heard_so_far = self.dials.iter().filter(|d| d.less_heard).count();
            let less_heard =
                greeter.is_none() && less_heard_so_far * LESS_HEARD_EVERY <= self.dials.len();
            let Some(machine) = greeter.or_else(|| self.next_dial(now, less_heard)) else {
                break;
            };
            self.tried(&machine, now);
            self.dials.push(Dial {
                at_ms: now,
                less_heard,
                greeted_back: greeter.is_some(),
            });
            self.dials_changed = true;
            plan.dials.push(machine);
        }
        let protected = self.protected(now);
        let check_in = ms(self.times.check_in_every);
        let mut due: Vec<&Entry> = self
            .entries
            .values()
            .filter(|e| e.met_ms.is_some())
            .filter(|e| now - e.tried_ms >= (check_in as f64 * spread(e)) as i64)
            .collect();
        // Known machines are checked in with first.
        due.sort_by_key(|e| (!protected.contains(&e.machine), e.tried_ms));
        plan.check_ins = due
            .into_iter()
            .take(CHECK_INS_PER_ROUND)
            .map(|e| e.machine)
            .collect();
        for machine in plan.check_ins.clone() {
            self.tried(&machine, now);
        }
        plan
    }

    /// The next machine heard of to dial: the one heard of longest ago, or
    /// with `less_heard`, one from the source that supplied the fewest of the
    /// table's machines.
    fn next_dial(&self, now: i64, less_heard: bool) -> Option<EndpointId> {
        let candidates = self.candidates(now);
        let pick = match less_heard {
            false => candidates.min_by_key(|e| (e.heard_ms, e.machine)),
            true => {
                let mut supplied: HashMap<Source, usize> = HashMap::new();
                for entry in self.entries.values() {
                    *supplied.entry(entry.source).or_default() += 1;
                }
                candidates.min_by_key(|e| (supplied.get(&e.source).copied(), e.heard_ms, e.machine))
            }
        };
        pick.map(|e| e.machine)
    }

    /// A machine that greeted this one unasked and hasn't been met, picked at
    /// random, so a crowd greeting under many names gets only its share.
    fn next_greeter(&self, now: i64) -> Option<EndpointId> {
        let greeters: Vec<&Entry> = self
            .candidates(now)
            .filter(|e| e.source == Source::Greeted)
            .collect();
        let pick = (random_fraction() * greeters.len() as f64) as usize;
        greeters
            .get(pick.min(greeters.len().saturating_sub(1)))
            .map(|e| e.machine)
    }

    /// Machines heard of and never met, that may be dialled now.
    fn candidates(&self, now: i64) -> impl Iterator<Item = &Entry> {
        let again = ms(self.times.try_again_after);
        self.entries
            .values()
            .filter(|e| e.met_ms.is_none() && e.tries < MAX_TRIES)
            .filter(move |e| e.tried_ms == 0 || now - e.tried_ms >= again)
    }

    /// Whether a machine heard of is waiting to be dialled.
    pub fn has_candidates(&self) -> bool {
        self.entries
            .values()
            .any(|e| e.met_ms.is_none() && e.tries < MAX_TRIES)
    }

    /// Whether any machine has answered lately.
    pub fn any_answering(&self, now: i64) -> bool {
        let since = now - ms(self.times.still_answering);
        self.entries
            .values()
            .any(|e| e.answered_ms.is_some_and(|at| at >= since))
    }

    fn tried(&mut self, machine: &EndpointId, now: i64) {
        if let Some(entry) = self.entries.get_mut(machine) {
            entry.tried_ms = now;
            self.changed.insert(*machine);
        }
    }

    /// A machine answered, with its record. If it's met for the first time,
    /// whoever passed on its record passed on one that checked out.
    pub fn answered(&mut self, machine: &EndpointId, record: &MachineRecord, now: i64) {
        let half_life = ms(self.times.half_life);
        let Some(entry) = self.entries.get_mut(machine) else {
            return;
        };
        entry.fade(now, half_life);
        entry.counts.answers += 1.0;
        entry.tries = 0;
        entry.answered_ms = Some(now);
        if *record != entry.record
            && let Some(published) = record
                .verify()
                .ok()
                .filter(|info| info.machine == *machine)
                .and_then(|info| osp_proto::parse_rfc3339_ms(&info.published))
                .filter(|published| *published > entry.published_ms)
        {
            entry.record = record.clone();
            entry.published_ms = published;
        }
        let first = entry.met_ms.is_none();
        entry.met_ms.get_or_insert(now);
        let source = entry.source;
        self.changed.insert(*machine);
        if first && let Source::Machine(from) = source {
            self.passed_on(&from, true, now);
        }
    }

    /// A machine didn't answer. One heard of and never met is forgotten after
    /// a few tries, and whoever passed on its record passed on one that
    /// didn't check out.
    pub fn missed(&mut self, machine: &EndpointId, now: i64) {
        let half_life = ms(self.times.half_life);
        let Some(entry) = self.entries.get_mut(machine) else {
            return;
        };
        entry.fade(now, half_life);
        entry.counts.misses += 1.0;
        entry.tries += 1;
        self.changed.insert(*machine);
        if entry.met_ms.is_none() && entry.tries >= MAX_TRIES {
            let source = entry.source;
            self.remove(machine);
            if let Source::Machine(from) = source {
                self.passed_on(&from, false, now);
            }
        }
    }

    /// Notes a record `from` passed on that did or didn't check out.
    pub fn passed_on(&mut self, from: &EndpointId, good: bool, now: i64) {
        let half_life = ms(self.times.half_life);
        if let Some(entry) = self.entries.get_mut(from) {
            entry.fade(now, half_life);
            match good {
                true => entry.counts.good += 1.0,
                false => entry.counts.bad += 1.0,
            }
            self.changed.insert(*from);
        }
    }

    /// Up to `MAX_KNOWN` records of machines met lately, picked at random,
    /// for a `Known` reply to `asker`.
    pub fn known_reply(&self, asker: &EndpointId, now: i64) -> Vec<MachineRecord> {
        let since = now - ms(self.times.met_lately);
        let mut lately: Vec<&Entry> = self
            .entries
            .values()
            .filter(|e| e.machine != *asker && e.met_ms.is_some())
            .filter(|e| e.answered_ms.is_some_and(|at| at >= since))
            .collect();
        // A partial shuffle: the first `MAX_KNOWN` are a random pick.
        for i in 0..lately.len().min(MAX_KNOWN) {
            let j = i
                + ((random_fraction() * (lately.len() - i) as f64) as usize)
                    .min(lately.len() - i - 1);
            lately.swap(i, j);
        }
        lately
            .into_iter()
            .take(MAX_KNOWN)
            .map(|e| e.record.clone())
            .collect()
    }

    /// Forgets machines met that stopped answering long ago, and machines
    /// heard of whose records are no longer current.
    fn sweep(&mut self, now: i64) {
        let silent = now - ms(self.times.forget_silent);
        let stale = now - RECORD_MAX_AGE_MS;
        let gone: Vec<EndpointId> = self
            .entries
            .values()
            .filter(|e| match e.met_ms {
                Some(_) => e.answered_ms.unwrap_or(0) < silent,
                None => e.published_ms < stale,
            })
            .map(|e| e.machine)
            .collect();
        for machine in gone {
            self.remove(&machine);
        }
    }

    fn remove(&mut self, machine: &EndpointId) {
        if self.entries.remove(machine).is_some() {
            self.changed.remove(machine);
            self.removed.insert(*machine);
        }
    }

    /// What changed since this was last asked, to be written.
    pub fn take_changes(&mut self) -> Changes {
        let put = self
            .changed
            .drain()
            .filter_map(|machine| self.entries.get(&machine).cloned())
            .collect();
        let dials = std::mem::take(&mut self.dials_changed).then(|| self.dials.clone());
        Changes {
            put,
            removed: self.removed.drain().collect(),
            dials,
        }
    }

    pub fn acquaintances(&self, now: i64) -> Vec<Acquaintance> {
        let protected = self.protected(now);
        self.entries
            .values()
            .map(|e| Acquaintance {
                machine: e.machine,
                stage: self.stage(e, now),
                source: match e.source {
                    Source::Greeted => None,
                    Source::Machine(from) => Some(from),
                },
                kept_longest: protected.contains(&e.machine),
            })
            .collect()
    }
}

/// Where between three quarters and one and a quarter of the usual wait a
/// machine's next check-in falls, so they don't all come at once, and don't
/// line up with anything else the node does.
fn spread(entry: &Entry) -> f64 {
    let mut hash = blake3::Hasher::new();
    hash.update(entry.machine.as_bytes());
    hash.update(&entry.tried_ms.to_le_bytes());
    let bytes: [u8; 8] = hash.finalize().as_bytes()[..8].try_into().expect("8 bytes");
    0.75 + 0.5 * (u64::from_le_bytes(bytes) as f64 / u64::MAX as f64)
}

/// What a node's machine name knows of another machine. For tests, the
/// simulator and its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acquaintance {
    pub machine: EndpointId,
    pub stage: Stage,
    /// The machine whose record of it came first, or none if it greeted this
    /// one unasked.
    pub source: Option<EndpointId>,
    /// Whether it holds one of the places kept for the machines known longest.
    pub kept_longest: bool,
}

/// The table, kept in memory and written through to the database.
pub(crate) struct Acquaintances {
    table: Mutex<Table>,
    db: Arc<Db>,
}

impl Acquaintances {
    pub fn load(db: Arc<Db>, apart: HashSet<EndpointId>, times: MeetingTimes) -> Result<Self> {
        let (stored, dials) = db.machines()?;
        let mut table = Table::load(stored, dials, apart, times, now_ms());
        db.save_machines(&table.take_changes())?;
        Ok(Self {
            table: Mutex::new(table),
            db,
        })
    }

    /// Does `f` to the table, and writes what changed. Failing to write is
    /// only logged: meeting machines must never stop the node.
    fn with<T>(&self, f: impl FnOnce(&mut Table) -> T) -> T {
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let result = f(&mut table);
        let changes = table.take_changes();
        if !changes.is_empty()
            && let Err(e) = self.db.save_machines(&changes)
        {
            warn!("couldn't write the machines this node knows: {e:#}");
        }
        result
    }

    /// A machine greeted this one.
    pub fn greeted(&self, record: &MachineRecord) {
        if let Err(e) = self.with(|t| t.hear(record, Source::Greeted, now_ms())) {
            debug!("not keeping a greeting's record: {e:#}");
        }
    }

    /// A directory's map had this machine.
    pub fn on_map(&self, directory: EndpointId, record: &MachineRecord) {
        if let Err(e) = self.with(|t| t.hear(record, Source::Machine(directory), now_ms())) {
            debug!("not keeping a record from a directory's map: {e:#}");
        }
    }

    pub fn known_reply(&self, asker: &EndpointId) -> Vec<MachineRecord> {
        self.with(|t| t.known_reply(asker, now_ms()))
    }

    pub fn plan(&self, now: i64) -> Plan {
        self.with(|t| t.plan(now))
    }

    /// Notes how a conversation went. Returns whether it answered.
    pub fn after(&self, machine: EndpointId, talk: Result<Talk>) -> bool {
        let now = now_ms();
        self.with(|t| match talk {
            Ok(talk) => {
                t.answered(&machine, &talk.record, now);
                if talk.too_many {
                    t.passed_on(&machine, false, now);
                }
                for record in &talk.known {
                    if let Err(e) = t.hear(record, Source::Machine(machine), now) {
                        debug!("a machine passed on a record that didn't check out: {e:#}");
                        t.passed_on(&machine, false, now);
                    }
                }
                true
            }
            // Not why: the error names the machine.
            Err(_) => {
                debug!("a machine didn't answer");
                t.missed(&machine, now);
                false
            }
        })
    }

    /// Whether the rest have failed: nothing answering lately, and nothing
    /// left to try.
    pub fn stuck(&self, now: i64) -> bool {
        self.with(|t| !t.any_answering(now) && !t.has_candidates())
    }

    pub fn acquaintances(&self) -> Vec<Acquaintance> {
        self.with(|t| t.acquaintances(now_ms()))
    }
}

/// What a conversation with a machine brought.
pub(crate) struct Talk {
    /// Its record, checked as the machine's own.
    pub record: MachineRecord,
    /// The records its `Known` reply passed on, unchecked; none from a
    /// machine that doesn't know the request yet.
    pub known: Vec<MachineRecord>,
    /// It passed on more than it may.
    pub too_many: bool,
}

impl Core {
    /// One round: dials machines heard of, within the limits, checks in with
    /// some met, and goes back to the directories if all else fails.
    async fn meet_round(&self) -> Result<()> {
        let Some(side) = &self.machine else {
            bail!("no machine name");
        };
        if let Some(tor) = &self.tor {
            tor.check_ready()?;
        }
        let acquaintances = &side.acquaintances;
        let plan = acquaintances.plan(now_ms());
        let talks = plan.dials.into_iter().chain(plan.check_ins);
        let _: Vec<bool> = stream::iter(talks)
            .map(|machine| async move { acquaintances.after(machine, side.talk(machine).await) })
            .buffered_unordered(AT_ONCE)
            .collect()
            .await;
        if !self.config.directory.known.is_empty() && acquaintances.stuck(now_ms()) {
            self.read_directories_now.notify_one();
        }
        Ok(())
    }
}

/// A node's machine meeting others: a round at a time of its own, about every
/// `MeetingTimes::round`.
pub(crate) async fn meet_loop(core: Arc<Core>) {
    let round = core.sim.meeting.round;
    tokio::time::sleep(round.mul_f64(random_fraction())).await;
    loop {
        // Messages wait for Tor, and go nowhere else (ADR 0014).
        if let Some(tor) = &core.tor {
            tor.ready().await;
        }
        if let Err(e) = core.meet_round().await {
            debug!("a round of meeting machines failed: {e:#}");
        }
        tokio::time::sleep(jitter(round)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use iroh::SecretKey;

    use super::*;

    const HOUR_MS: i64 = 60 * 60 * 1000;

    fn times() -> MeetingTimes {
        MeetingTimes::default()
    }

    fn date(at: i64) -> String {
        humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_millis(at as u64)).to_string()
    }

    fn record_at(key: &SecretKey, at: i64) -> MachineRecord {
        MachineRecord::new(key, vec![], date(at)).unwrap()
    }

    /// A new machine's record, dated `at`.
    fn machine(at: i64) -> (EndpointId, MachineRecord) {
        let key = SecretKey::generate();
        (key.public(), record_at(&key, at))
    }

    fn start() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    fn table() -> Table {
        Table::new(HashSet::new(), times())
    }

    fn stage_of(t: &Table, machine: &EndpointId, now: i64) -> Option<Stage> {
        t.get(machine).map(|e| t.stage(e, now))
    }

    /// Hears of `count` new machines from `source`, and returns those kept.
    fn hear_many(t: &mut Table, source: Source, count: usize, now: i64) -> Vec<EndpointId> {
        (0..count)
            .filter_map(|_| {
                let (id, record) = machine(now);
                (t.hear(&record, source, now).unwrap() == Heard::Added).then_some(id)
            })
            .collect()
    }

    /// Meets `machine`: it answers a dial.
    fn meet(t: &mut Table, machine: &EndpointId, now: i64) {
        let record = t.get(machine).unwrap().record.clone();
        t.tried(machine, now);
        t.answered(machine, &record, now);
    }

    /// Lets `machines` answer a check-in every day until `until`.
    fn keep_answering(t: &mut Table, machines: &[EndpointId], from: i64, until: i64) {
        let mut now = from;
        while now <= until {
            for m in machines {
                meet(t, m, now);
            }
            now += DAY_MS;
        }
    }

    fn shares_of(t: &Table, now: i64) -> HashMap<Source, usize> {
        t.shares(&t.protected(now))
    }

    #[test]
    fn a_source_fills_a_quarter_of_the_places_not_kept_longest_and_meeting_frees_none() {
        let now = start();
        let mut t = table();
        let source = Source::Machine(machine(now).0);
        let kept = hear_many(&mut t, source, PER_SOURCE + 10, now);
        assert_eq!(kept.len(), PER_SOURCE);
        assert_eq!(PER_SOURCE, 32);

        // Meeting them all still leaves them counted against their source.
        for m in &kept {
            meet(&mut t, m, now);
        }
        assert!(hear_many(&mut t, source, 1, now).is_empty());

        // Another source, and those who greet unasked, have shares of their own.
        assert_eq!(
            hear_many(&mut t, Source::Greeted, 40, now).len(),
            PER_SOURCE
        );
        let other = Source::Machine(machine(now).0);
        assert_eq!(hear_many(&mut t, other, 5, now).len(), 5);
    }

    #[test]
    fn a_duplicate_takes_no_place_and_counts_for_its_first_source_only() {
        let now = start();
        let mut t = table();
        let (first, second) = (machine(now).0, machine(now).0);
        let key = SecretKey::generate();
        let older = record_at(&key, now - HOUR_MS);
        assert_eq!(
            t.hear(&older, Source::Machine(first), now).unwrap(),
            Heard::Added
        );
        for _ in 0..3 {
            let again = t.hear(&older, Source::Machine(second), now).unwrap();
            assert_eq!(again, Heard::Already);
        }
        let shares = shares_of(&t, now);
        assert_eq!(shares.get(&Source::Machine(first)), Some(&1));
        assert_eq!(shares.get(&Source::Machine(second)), None);
        assert_eq!(t.len(), 1);

        // A newer record replaces it, whoever brings it; an older one doesn't.
        let newer = record_at(&key, now);
        t.hear(&newer, Source::Machine(second), now).unwrap();
        assert_eq!(t.get(&key.public()).unwrap().record, newer);
        t.hear(&older, Source::Greeted, now).unwrap();
        assert_eq!(t.get(&key.public()).unwrap().record, newer);
        assert_eq!(t.get(&key.public()).unwrap().source, Source::Machine(first));
    }

    #[test]
    fn records_that_dont_check_out_are_refused() {
        let now = start();
        let mut t = table();
        let key = SecretKey::generate();
        let stale = record_at(&key, now - RECORD_MAX_AGE_MS - HOUR_MS);
        let ahead = record_at(&key, now + HOUR_MS);
        let undated = MachineRecord::new(&key, vec![], "soon".into()).unwrap();
        for record in [stale, ahead, undated] {
            t.hear(&record, Source::Greeted, now).unwrap_err();
        }
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn this_machine_and_the_directories_in_config_stay_apart() {
        let now = start();
        let (me, my_record) = machine(now);
        let (directory, its_record) = machine(now);
        let mut t = Table::new(HashSet::from([me, directory]), times());
        for record in [&my_record, &its_record] {
            assert_eq!(t.hear(record, Source::Greeted, now).unwrap(), Heard::Apart);
        }
        // A full table holds neither, so it can't push them out, and the node
        // can always go back to its directories.
        let source = |_| Source::Machine(machine(now).0);
        for i in 0..MAX_MACHINES {
            hear_many(&mut t, source(i), 1, now);
        }
        assert_eq!(t.len(), MAX_MACHINES);
        assert!(t.get(&directory).is_none() && t.get(&me).is_none());

        // One stored from before it was a directory goes when the table loads.
        let (stored, dials) = (t.entries.values().cloned().collect(), vec![]);
        let some = *t.entries.keys().next().unwrap();
        let t = Table::load(stored, dials, HashSet::from([some]), times(), now);
        assert!(t.get(&some).is_none());
        assert_eq!(t.len(), MAX_MACHINES - 1);
    }

    #[test]
    fn a_machine_is_known_after_a_month_of_answers_with_records_that_checked_out() {
        let then = start() - 40 * DAY_MS;
        let mut t = table();
        let ids = hear_many(&mut t, Source::Greeted, 4, then);
        let [steady, quiet, liar, flaky] = ids[..] else {
            panic!()
        };
        assert_eq!(stage_of(&t, &steady, then), Some(Stage::Heard));
        meet(&mut t, &steady, then);
        assert_eq!(stage_of(&t, &steady, then), Some(Stage::Met));

        let now = then + 31 * DAY_MS;
        keep_answering(&mut t, &[steady, quiet, liar, flaky], then, now);
        assert_eq!(stage_of(&t, &steady, now), Some(Stage::Known));
        // Known needs more answers than misses.
        for i in 0..40 {
            t.missed(&flaky, now - i);
        }
        assert_eq!(stage_of(&t, &flaky, now), Some(Stage::Met));
        // Known needs answers lately.
        assert_eq!(
            stage_of(&t, &quiet, now + 4 * DAY_MS),
            Some(Stage::Met),
            "not answering for days"
        );
        // And records that checked out: one bad one is forgiven, not more.
        t.passed_on(&liar, false, now);
        assert_eq!(stage_of(&t, &liar, now), Some(Stage::Known));
        t.passed_on(&liar, false, now);
        assert_eq!(stage_of(&t, &liar, now), Some(Stage::Met));
        t.passed_on(&liar, true, now);
        assert_eq!(stage_of(&t, &liar, now), Some(Stage::Known));
    }

    /// The table full: `known` machines known longest, met 40 days ago and
    /// answering since, and the rest machines met lately, from many sources.
    fn full_table(known: usize, now: i64) -> (Table, Vec<EndpointId>) {
        let then = now - 40 * DAY_MS;
        let mut t = table();
        let mut old = Vec::new();
        // Each met a moment after the one before.
        for i in 0..known as i64 {
            let met = hear_many(&mut t, Source::Machine(machine(then).0), 1, then + i);
            meet(&mut t, &met[0], then + i);
            old.extend(met);
        }
        keep_answering(&mut t, &old, then + DAY_MS, now);
        while t.len() < MAX_MACHINES {
            let new = hear_many(&mut t, Source::Machine(machine(now).0), 1, now);
            for m in &new {
                meet(&mut t, m, now);
            }
        }
        (t, old)
    }

    #[test]
    fn half_the_places_stay_with_those_known_longest_whatever_arrives() {
        let now = start();
        let (mut t, old) = full_table(KEPT_LONGEST + 20, now);
        let protected = t.protected(now);
        assert_eq!(protected.len(), KEPT_LONGEST);
        assert!(old[..KEPT_LONGEST].iter().all(|m| protected.contains(m)));

        // A crowd of new names, from many sources and none: nothing met goes.
        let before: HashSet<EndpointId> = t.entries.keys().copied().collect();
        for _ in 0..50 {
            hear_many(&mut t, Source::Machine(machine(now).0), 10, now);
        }
        hear_many(&mut t, Source::Greeted, 100, now);
        let after: HashSet<EndpointId> = t.entries.keys().copied().collect();
        assert!(before.is_subset(&after));
        // Those greeting only took turns.
        assert_eq!(t.len(), MAX_MACHINES + GREETER_TURNS);
    }

    #[test]
    fn machines_greeting_a_full_table_take_turns_met_or_not() {
        let now = start();
        let (mut t, _) = full_table(KEPT_LONGEST, now);
        let before: HashSet<EndpointId> = t.entries.keys().copied().collect();
        let greet = |t: &mut Table, at: i64| {
            let (id, record) = machine(at);
            assert_eq!(t.hear(&record, Source::Greeted, at).unwrap(), Heard::Turn);
            id
        };
        let first: Vec<EndpointId> = (0..GREETER_TURNS as i64)
            .map(|i| greet(&mut t, now + i))
            .collect();
        assert_eq!(t.len(), MAX_MACHINES + GREETER_TURNS);
        // Met, the first still gives its place up to the next greeter.
        meet(&mut t, &first[0], now + 10);
        let next = greet(&mut t, now + 20);
        assert!(t.get(&first[0]).is_none() && t.get(&next).is_some());
        assert!(first[1..].iter().all(|m| t.get(m).is_some()));
        assert_eq!(t.len(), MAX_MACHINES + GREETER_TURNS);
        // Nothing else gave up a place.
        assert!(before.iter().all(|m| t.get(m).is_some()));

        // With room in the table, those beyond the greeters' share take turns.
        let mut t = table();
        assert_eq!(
            hear_many(&mut t, Source::Greeted, PER_SOURCE, now).len(),
            PER_SOURCE
        );
        let turns: Vec<EndpointId> = (0..GREETER_TURNS as i64 + 3)
            .map(|i| greet(&mut t, now + 1 + i))
            .collect();
        assert_eq!(t.len(), PER_SOURCE + GREETER_TURNS);
        assert_eq!(
            turns.iter().filter(|m| t.get(m).is_some()).count(),
            GREETER_TURNS
        );
    }

    #[test]
    fn fake_names_referring_one_another_cant_push_out_those_known_longest() {
        let now = start();
        let (mut t, old) = full_table(KEPT_LONGEST, now);
        // Make room: the machines met lately go silent and are forgotten.
        let lately: Vec<EndpointId> = t
            .entries
            .keys()
            .filter(|m| !old.contains(m))
            .copied()
            .collect();
        let later = now + 31 * DAY_MS;
        keep_answering(&mut t, &old, now, later);
        t.plan(later);
        assert!(lately.iter().all(|m| t.get(m).is_none()));

        // One operator's names pass on one another and fresh fakes, and answer
        // every time, from the start.
        let mut names: Vec<EndpointId> = hear_many(&mut t, Source::Greeted, 4, later);
        let mut now = later;
        for _ in 0..3 {
            now += HOUR_MS;
            // Those still in the table answer, and each passes on 20 more.
            names.retain(|name| t.get(name).is_some());
            for name in names.clone().into_iter().take(30) {
                // A name never met can give up its place to another's.
                if t.get(&name).is_none() {
                    continue;
                }
                meet(&mut t, &name, now);
                names.extend(hear_many(&mut t, Source::Machine(name), 20, now));
            }
            for m in &old {
                meet(&mut t, m, now);
            }
        }
        assert!(t.len() == MAX_MACHINES, "the fakes filled the rest");
        let protected = t.protected(now);
        assert!(old.iter().all(|m| protected.contains(m)));
        assert!(t.len() <= MAX_MACHINES);
        // No one source fills more than its share of the rest.
        assert!(shares_of(&t, now).values().all(|n| *n <= PER_SOURCE));
    }

    #[test]
    fn a_newcomer_takes_the_place_of_one_never_met_from_the_source_with_most() {
        let now = start();
        let mut t = table();
        let big = Source::Machine(machine(now).0);
        let big_ones = hear_many(&mut t, big, PER_SOURCE, now);
        // The rest fill up with machines met, one per source.
        while t.len() < MAX_MACHINES {
            for m in hear_many(&mut t, Source::Machine(machine(now).0), 1, now) {
                meet(&mut t, &m, now);
            }
        }
        // A source with one place takes one from the biggest, oldest first.
        let small = Source::Machine(machine(now).0);
        let (id, record) = machine(now);
        assert_eq!(t.hear(&record, small, now + 1).unwrap(), Heard::Added);
        assert!(t.get(&id).is_some());
        assert_eq!(big_ones.iter().filter(|m| t.get(m).is_none()).count(), 1);
        // The source with most can't take a place from itself, or from any
        // smaller one.
        let (_, one_more) = machine(now);
        assert_eq!(t.hear(&one_more, big, now).unwrap(), Heard::Full);
        // Once the biggest source's machines are met, none gives up its place.
        for m in &big_ones {
            if t.get(m).is_some() {
                meet(&mut t, m, now);
            }
        }
        let (_, another) = machine(now);
        let newcomer = Source::Machine(machine(now).0);
        assert_eq!(t.hear(&another, newcomer, now + 2).unwrap(), Heard::Full);
        assert_eq!(big_ones.iter().filter(|m| t.get(m).is_none()).count(), 1);
    }

    #[test]
    fn at_most_six_new_machines_are_dialled_an_hour_two_for_sources_heard_from_less() {
        let now = start();
        let mut t = table();
        let crowd = Source::Machine(machine(now).0);
        hear_many(&mut t, crowd, PER_SOURCE, now);
        let few = hear_many(&mut t, Source::Machine(machine(now).0), 2, now + 1);

        let mut dialled = Vec::new();
        let mut at = now;
        for round in 0..20 {
            let dials = t.plan(at).dials;
            if round == 0 {
                assert_eq!(dials.len(), DIALS_PER_ROUND, "spread over the hour");
            }
            dialled.extend(dials);
            at += 60 * 1000;
        }
        assert_eq!(dialled.len(), DIALS_PER_HOUR);
        // None twice: one dialled waits before it's tried again.
        assert_eq!(dialled.iter().collect::<HashSet<_>>().len(), DIALS_PER_HOUR);
        // The 1st and 4th went to the source that supplied fewest.
        assert!(few.contains(&dialled[0]) && few.contains(&dialled[3]));
        assert!(dialled.iter().filter(|m| few.contains(m)).count() == 2);

        // A new hour, more dials.
        assert!(!t.plan(now + HOUR_MS + 1).dials.is_empty());
    }

    #[test]
    fn the_source_heard_from_less_keeps_two_of_six_as_the_hours_slide_on() {
        let now = start();
        let mut t = table();
        let crowd = Source::Machine(machine(now).0);
        hear_many(&mut t, crowd, PER_SOURCE, now);
        let few = hear_many(&mut t, Source::Machine(machine(now).0), 12, now + 1);

        // Five hours of rounds, as often as a real node's, none answered.
        let round = ms(times().round);
        let mut dialled = Vec::new();
        let mut at = now;
        while at < now + 5 * HOUR_MS {
            dialled.extend(t.plan(at).dials.into_iter().map(|m| (at, m)));
            at += round;
        }
        assert_eq!(dialled.len(), 5 * DIALS_PER_HOUR);
        // In every hour, however it's cut, 2 of the 6 went to the few.
        for &(from, _) in &dialled {
            let hour: Vec<_> = dialled
                .iter()
                .filter(|(at, _)| *at >= from && *at < from + HOUR_MS)
                .collect();
            if hour.len() == DIALS_PER_HOUR {
                let to_few = hour.iter().filter(|(_, m)| few.contains(m)).count();
                assert_eq!(to_few, DIALS_PER_HOUR / LESS_HEARD_EVERY, "from {from}");
            }
        }
    }

    #[test]
    fn one_dial_an_hour_goes_to_a_machine_that_greeted_unasked() {
        let now = start();
        let mut t = table();
        let crowd = Source::Machine(machine(now).0);
        hear_many(&mut t, crowd, PER_SOURCE, now);
        let few = hear_many(&mut t, Source::Machine(machine(now).0), 2, now + 1);
        // Heard of last, so dialled last but for this rule.
        let greeters = hear_many(&mut t, Source::Greeted, 4, now + 2);

        let mut dialled = Vec::new();
        for minute in 0..20 {
            dialled.extend(t.plan(now + minute * 60_000).dials);
        }
        assert_eq!(dialled.len(), DIALS_PER_HOUR);
        assert!(greeters.contains(&dialled[0]));
        assert_eq!(dialled.iter().filter(|m| greeters.contains(m)).count(), 1);
        // The source heard from less still gets its two.
        assert_eq!(dialled.iter().filter(|m| few.contains(m)).count(), 2);

        // The next hour, another greeter.
        let next = t.plan(now + HOUR_MS + 60_000).dials;
        assert!(greeters.contains(&next[0]) && !dialled.contains(&next[0]));
    }

    #[test]
    fn one_never_answering_is_dropped_after_a_few_tries_and_counts_against_its_source() {
        let now = start();
        let mut t = table();
        let referrer = hear_many(&mut t, Source::Greeted, 1, now)[0];
        meet(&mut t, &referrer, now);
        let fake = hear_many(&mut t, Source::Machine(referrer), 1, now)[0];
        for i in 0..MAX_TRIES {
            assert!(t.get(&fake).is_some());
            t.missed(&fake, now + i as i64);
        }
        assert!(t.get(&fake).is_none());
        assert_eq!(t.get(&referrer).unwrap().counts.bad, 1.0);

        // One that answers counts for it.
        let real = hear_many(&mut t, Source::Machine(referrer), 1, now)[0];
        meet(&mut t, &real, now);
        assert_eq!(t.get(&referrer).unwrap().counts.good, 1.0);
        // Only the first time.
        meet(&mut t, &real, now + 1);
        assert_eq!(t.get(&referrer).unwrap().counts.good, 1.0);
    }

    #[test]
    fn counts_fade_by_half_in_a_half_life() {
        let now = start();
        let mut t = table();
        let m = hear_many(&mut t, Source::Greeted, 1, now)[0];
        meet(&mut t, &m, now);
        meet(&mut t, &m, now + ms(times().half_life));
        let answers = t.get(&m).unwrap().counts.answers;
        assert!((answers - 1.5).abs() < 1e-9, "{answers}");
    }

    #[test]
    fn known_names_only_machines_met_lately_at_most_twenty_and_not_the_asker() {
        let now = start();
        let mut t = table();
        let asker = machine(now).0;
        let heard = hear_many(&mut t, Source::Greeted, 5, now);
        let met = hear_many(&mut t, Source::Machine(asker), 30, now);
        for m in &met {
            meet(&mut t, m, now);
        }
        let long_silent = met[0];
        t.entries.get_mut(&long_silent).unwrap().answered_ms = Some(now - 8 * DAY_MS);
        let reply = t.known_reply(&asker, now);
        assert_eq!(reply.len(), MAX_KNOWN);
        let named: HashSet<EndpointId> =
            reply.iter().map(|r| r.verify().unwrap().machine).collect();
        assert_eq!(named.len(), MAX_KNOWN);
        assert!(named.iter().all(|m| met.contains(m) && *m != long_silent));
        assert!(heard.iter().all(|m| !named.contains(m)));
        // Picked at random: a few replies name more than one could.
        let named_in_some: HashSet<EndpointId> = (0..10)
            .flat_map(|_| t.known_reply(&asker, now))
            .map(|r| r.verify().unwrap().machine)
            .collect();
        assert!(named_in_some.len() > MAX_KNOWN, "{}", named_in_some.len());
        // The one asking isn't named to itself, however often it asks: each
        // reply would name it 2 times in 3 if it could.
        for _ in 0..20 {
            let reply = t.known_reply(&met[1], now);
            assert!(reply.iter().all(|r| r.verify().unwrap().machine != met[1]));
        }
    }

    #[test]
    fn check_ins_come_about_daily_known_machines_first() {
        let now = start();
        let (mut t, old) = full_table(10, now);
        // Fresh from being met: nothing due.
        assert!(t.plan(now).check_ins.is_empty());
        let next_day = now + ms(times().check_in_every) * 5 / 4 + 1;
        let first = t.plan(next_day).check_ins;
        assert_eq!(first.len(), CHECK_INS_PER_ROUND);
        assert!(first.iter().all(|m| old.contains(m)), "known ones first");
    }

    #[test]
    fn the_limits_and_the_hours_dials_hold_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(&dir.path().join("osp.db")).unwrap());
        let load = || Acquaintances::load(db.clone(), HashSet::new(), times()).unwrap();
        let now = now_ms();
        let map = machine(now).0;
        let a = load();
        for _ in 0..PER_SOURCE + 8 {
            a.on_map(map, &machine(now).1);
        }
        a.greeted(&machine(now).1);
        let mut dialled = 0;
        for minute in 0..3 {
            dialled += a.plan(now + minute * 60_000).dials.len();
        }
        assert_eq!(dialled, DIALS_PER_HOUR);
        let before: HashSet<EndpointId> = a.acquaintances().iter().map(|a| a.machine).collect();
        let dials_before = a.table.lock().unwrap().dials.clone();
        assert!(dials_before.iter().any(|d| d.less_heard));
        assert!(dials_before.iter().any(|d| d.greeted_back));
        drop(a);

        let a = load();
        let after: HashSet<EndpointId> = a.acquaintances().iter().map(|a| a.machine).collect();
        assert_eq!(after, before);
        // Which dials went to the source heard from less and to a greeter, too.
        assert_eq!(a.table.lock().unwrap().dials, dials_before);
        assert_eq!(after.len(), PER_SOURCE + 1);
        // The source's share is still full, and the hour's dials still used.
        a.on_map(map, &machine(now).1);
        assert_eq!(a.acquaintances().len(), PER_SOURCE + 1);
        assert!(a.plan(now + 4 * 60_000).dials.is_empty());
        assert!(!a.plan(now + HOUR_MS + 1).dials.is_empty());
    }

    #[test]
    fn a_reply_passing_on_too_many_counts_against_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(&dir.path().join("osp.db")).unwrap());
        let a = Acquaintances::load(db, HashSet::new(), times()).unwrap();
        let now = now_ms();
        let (talker, record) = machine(now);
        a.greeted(&record);
        let talk = Talk {
            record,
            known: (0..MAX_KNOWN).map(|_| machine(now).1).collect(),
            too_many: true,
        };
        assert!(a.after(talker, Ok(talk)));
        let table = a.table.lock().unwrap();
        assert_eq!(table.get(&talker).unwrap().counts.bad, 1.0);
        assert_eq!(table.len(), 1 + MAX_KNOWN);
    }

    #[test]
    fn those_silent_long_and_records_gone_stale_are_forgotten() {
        let now = start();
        let mut t = table();
        let met = hear_many(&mut t, Source::Greeted, 1, now)[0];
        meet(&mut t, &met, now);
        let key = SecretKey::generate();
        let older = record_at(&key, now - 25 * DAY_MS);
        t.hear(&older, Source::Greeted, now).unwrap();
        // Six days on, its record is 31 days old.
        t.plan(now + 6 * DAY_MS);
        assert!(t.get(&key.public()).is_none());
        assert!(t.get(&met).is_some(), "met, and silent only six days");
        t.plan(now + ms(times().forget_silent) + 1);
        assert!(t.get(&met).is_none());
    }
}
