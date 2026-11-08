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
use osp_proto::machine::{IntroduceRequest, MAX_INTRODUCE_LIFETIME_SECS, MAX_KNOWN, MachineRecord};
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
/// Requests to be introduced a machine passes on in a day for one machine
/// asking, and for all (ADR 0016's starting values).
pub(crate) const PASSED_ON_PER_ASKER: usize = 3;
pub(crate) const PASSED_ON_PER_DAY: usize = 12;
/// Introductions accepted in an hour, at most.
pub(crate) const ACCEPTED_PER_HOUR: usize = 1;
/// A machine asks to be introduced while fewer than this many machines it
/// has met have come back and greeted it lately: while it's new, or few have
/// met it. Only those it met count, so strangers greeting it can't stop it.
pub(crate) const ASK_WHILE_GREETED_BY_FEWER: usize = 32;
/// Requests to be introduced remembered as seen, at most.
const MAX_SEEN: usize = 4096;
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
    /// A machine's introductions are accepted once it was first met this
    /// long ago (ADR 0016).
    pub introducer_after: Duration,
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
            introducer_after: DAY * 3,
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
            introducer_after: f(self.introducer_after),
        }
    }

    /// The day in "requests passed on a day".
    fn day(&self) -> i64 {
        24 * ms(self.hour)
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
    /// Its record came from this machine, which came in by greeting, or
    /// through one that did, and wasn't known yet. It counts against those who
    /// greet (ADR 0016), so a crowd greeting under many names, and passing on
    /// one another, can't make each name a source with places of its own.
    ThroughGreeter(EndpointId),
    /// This machine introduced it (ADR 0016). It counts against those who
    /// greet, as an introduction is another machine's word: it gets the
    /// machine introduced dialled sooner, not places of its own.
    Introduced(EndpointId),
}

impl Source {
    pub fn to_db(self) -> String {
        match self {
            Source::Greeted => "greeted".to_string(),
            Source::Machine(machine) => machine.to_string(),
            Source::ThroughGreeter(machine) => format!("through:{machine}"),
            Source::Introduced(machine) => format!("introduced:{machine}"),
        }
    }

    pub fn from_db(s: &str) -> Result<Self> {
        let machine = |m: &str| m.parse().context("corrupt machine source");
        Ok(match s {
            "greeted" => Source::Greeted,
            s => match (s.strip_prefix("through:"), s.strip_prefix("introduced:")) {
                (Some(m), _) => Source::ThroughGreeter(machine(m)?),
                (_, Some(m)) => Source::Introduced(machine(m)?),
                _ => Source::Machine(machine(s)?),
            },
        })
    }

    /// The share of places it counts against.
    fn share(self) -> Source {
        match self {
            Source::ThroughGreeter(_) | Source::Introduced(_) => Source::Greeted,
            other => other,
        }
    }

    /// The machine that passed its record on, or introduced it, if one did.
    fn passer(self) -> Option<EndpointId> {
        match self {
            Source::Greeted => None,
            Source::Machine(from) | Source::ThroughGreeter(from) | Source::Introduced(from) => {
                Some(from)
            }
        }
    }

    /// Whether the machine itself reached this one, greeting it or by an
    /// introduction, and so may take a turn (`GREETER_TURNS`).
    fn greets(self) -> bool {
        matches!(self, Source::Greeted | Source::Introduced(_))
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
    /// Introductions (ADR 0016), kept in memory only: after a restart a
    /// machine may ask a little longer, and its limits start afresh.
    intro: Introductions,
}

/// What a machine keeps of introductions.
#[derive(Default)]
struct Introductions {
    /// Machines it met that greeted it lately, and when: whether others have
    /// met it, so whether to ask to be introduced.
    greeted_by: HashMap<EndpointId, i64>,
    /// Machines it asked to be introduced to lately, and when, so it asks of
    /// each once.
    asked_about: HashMap<EndpointId, i64>,
    /// Requests it passed on in the last day, and who asked.
    passed: Vec<(i64, EndpointId)>,
    /// Requests to pass on in the next round, and to whom.
    to_pass_on: Vec<(EndpointId, IntroduceRequest)>,
    /// Introductions it accepted in the last hour.
    accepted: Vec<i64>,
    /// Requests seen, by ID, until they expire.
    seen: HashMap<String, i64>,
    counts: IntroductionCounts,
}

/// How a machine's introductions went, since it started. For the simulator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IntroductionCounts {
    /// Asked of others to be introduced, and passed on by them, by their word.
    pub asked: u64,
    pub asked_passed_on: u64,
    /// Others' requests passed on, and those refused as over the limits.
    pub passed_on: u64,
    pub refused_at_limit: u64,
    /// Introductions to this machine accepted, and declined.
    pub accepted: u64,
    pub declined: u64,
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
            intro: Introductions::default(),
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
        let answering = self.answering(entry, now);
        if met <= now - ms(t.known_after) && answering && c.answers > c.misses && checked_out(c) {
            Stage::Known
        } else {
            Stage::Met
        }
    }

    /// Whether a machine has answered lately enough to be known.
    fn answering(&self, entry: &Entry, now: i64) -> bool {
        entry
            .answered_ms
            .is_some_and(|at| at >= now - ms(self.times.still_answering))
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
                *shares.entry(entry.source.share()).or_default() += 1;
            }
        }
        shares
    }

    /// The source records `from` passes on count as: against those who greet,
    /// while `from` came in by greeting, or through one that did, and isn't
    /// known yet.
    fn counted_as(&self, source: Source, now: i64) -> Source {
        match source {
            Source::Machine(from) => match self.entries.get(&from) {
                Some(e)
                    if e.source.share() == Source::Greeted
                        && self.stage(e, now) != Stage::Known =>
                {
                    Source::ThroughGreeter(from)
                }
                _ => source,
            },
            other => other,
        }
    }

    /// Whether the table holds `machine`.
    pub fn holds(&self, machine: &EndpointId) -> bool {
        self.entries.contains_key(machine)
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
        let source = self.counted_as(source, now);
        // Mostly there's room, and the source is nowhere near its share.
        let from_source = self
            .entries
            .values()
            .filter(|e| e.source.share() == source.share())
            .count();
        if from_source < PER_SOURCE && self.entries.len() < MAX_MACHINES {
            self.insert(record, machine, published, source, now);
            return Ok(Heard::Added);
        }
        let protected = self.protected(now);
        let shares = self.shares(&protected);
        let share = shares.get(&source.share()).copied().unwrap_or(0);
        if share >= PER_SOURCE {
            // Only a machine that greeted gets in, not those it passes on.
            return Ok(match source.greets() {
                true => self.greeter_in(record, machine, published, source, &protected, now),
                false => Heard::SourceFull,
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
                .filter(|e| shares.get(&e.source.share()).copied().unwrap_or(0) > share + 1)
                .max_by_key(|e| (shares.get(&e.source.share()).copied(), Reverse(e.heard_ms)))
                .map(|e| e.machine);
            match given_up {
                Some(other) => self.remove(&other),
                None if source.greets() => {
                    return Ok(self.greeter_in(record, machine, published, source, &protected, now));
                }
                None => return Ok(Heard::Full),
            }
        }
        self.insert(record, machine, published, source, now);
        Ok(Heard::Added)
    }

    /// A machine that greeted this one, with no place for it otherwise, takes
    /// the place of a record passed on through those who greet: one never met
    /// first, then the one heard of longest ago. A greeting comes before what
    /// greeters pass on, so a crowd passing on its own names gives way to a
    /// newcomer, which has none to pass on. With none left, it takes a turn.
    fn greeter_in(
        &mut self,
        record: &MachineRecord,
        machine: EndpointId,
        published: i64,
        source: Source,
        protected: &HashSet<EndpointId>,
        now: i64,
    ) -> Heard {
        let passed_on = self
            .entries
            .values()
            .filter(|e| matches!(e.source, Source::ThroughGreeter(_)))
            .filter(|e| !protected.contains(&e.machine))
            .min_by_key(|e| (e.met_ms.is_some(), e.heard_ms, e.machine))
            .map(|e| e.machine);
        let Some(other) = passed_on else {
            return self.take_turn(record, machine, published, source, protected, now);
        };
        self.remove(&other);
        self.insert(record, machine, published, source, now);
        Heard::Added
    }

    /// A machine that greeted this one, with no place for it otherwise and
    /// none passed on through greeters to give one up, takes one of
    /// `GREETER_TURNS` places beyond the limits, in place of the greeter
    /// that took one longest ago. Met or not, each gives its place up in turn,
    /// so a crowd greeting under many names can't keep them. Those passed on
    /// through greeters count in the share, but never take a turn. A machine
    /// introduced keeps its turn until it's dialled, which comes within the
    /// hour, as only one is accepted an hour.
    fn take_turn(
        &mut self,
        record: &MachineRecord,
        machine: EndpointId,
        published: i64,
        source: Source,
        protected: &HashSet<EndpointId>,
        now: i64,
    ) -> Heard {
        let mut greeters: Vec<&Entry> = self
            .entries
            .values()
            .filter(|e| e.source.share() == Source::Greeted && !protected.contains(&e.machine))
            .collect();
        greeters.sort_by_key(|e| (e.heard_ms, e.machine));
        // Those heard of last, as many as are over the share or the table's size.
        let turns = (greeters.len().saturating_sub(PER_SOURCE))
            .max(self.entries.len().saturating_sub(MAX_MACHINES))
            .min(greeters.len());
        let waiting = |e: &Entry| matches!(e.source, Source::Introduced(_)) && e.met_ms.is_none();
        let gives_up = greeters[greeters.len() - turns..]
            .iter()
            .find(|e| !waiting(e))
            .filter(|_| turns >= GREETER_TURNS)
            .map(|e| e.machine);
        if let Some(other) = gives_up {
            self.remove(&other);
        } else if self.entries.len() >= MAX_MACHINES + GREETER_TURNS {
            return Heard::Full;
        }
        self.insert(record, machine, published, source, now);
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
            // newcomer isn't left behind every machine heard of before it: one
            // introduced first, then one that only greeted.
            let greeter = match self.dials.iter().any(|d| d.greeted_back) {
                false => self.next_introduced(now).or_else(|| self.next_greeter(now)),
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
    /// table's machines. Those who greet, and those passed on through them,
    /// are one source here too, so a crowd's many names aren't many sources
    /// heard from less.
    fn next_dial(&self, now: i64, less_heard: bool) -> Option<EndpointId> {
        let candidates = self.candidates(now);
        let pick = match less_heard {
            false => candidates.min_by_key(|e| (e.heard_ms, e.machine)),
            true => {
                let mut supplied: HashMap<Source, usize> = HashMap::new();
                for entry in self.entries.values() {
                    *supplied.entry(entry.source.share()).or_default() += 1;
                }
                candidates.min_by_key(|e| {
                    (
                        supplied.get(&e.source.share()).copied(),
                        e.heard_ms,
                        e.machine,
                    )
                })
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

    /// A machine introduced to this one and not yet met, the one introduced
    /// first.
    fn next_introduced(&self, now: i64) -> Option<EndpointId> {
        self.candidates(now)
            .filter(|e| matches!(e.source, Source::Introduced(_)))
            .min_by_key(|e| (e.heard_ms, e.machine))
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
        if first && let Some(from) = source.passer() {
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
            if let Some(from) = source.passer() {
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

    /// A machine greeted this one. If it's one this machine met, it has met
    /// this one too, which is what says whether to ask to be introduced.
    pub fn greeted_by(&mut self, machine: &EndpointId, now: i64) {
        if self
            .entries
            .get(machine)
            .is_some_and(|e| e.met_ms.is_some())
        {
            self.intro.greeted_by.insert(*machine, now);
        }
    }

    /// Whether to ask to be introduced: while fewer than
    /// `ASK_WHILE_GREETED_BY_FEWER` machines it met have greeted it lately.
    pub fn should_ask(&mut self, now: i64) -> bool {
        let since = now - ms(self.times.met_lately);
        self.intro.greeted_by.retain(|_, at| *at >= since);
        self.intro.greeted_by.len() < ASK_WHILE_GREETED_BY_FEWER
    }

    /// A machine to ask `via` to introduce this one to, from the records `via`
    /// just passed on, picked at random: not `via`, this machine or a
    /// directory, and none asked of lately or that has greeted this one
    /// lately, as that one has met it.
    pub fn target_to_ask(
        &mut self,
        via: &EndpointId,
        records: &[MachineRecord],
        now: i64,
    ) -> Option<EndpointId> {
        let since = now - ms(self.times.met_lately);
        let intro = &mut self.intro;
        intro.asked_about.retain(|_, at| *at >= since);
        let fresh: Vec<EndpointId> = records
            .iter()
            .filter_map(|r| r.verify().ok())
            .map(|info| info.machine)
            .filter(|m| m != via && !self.apart.contains(m))
            .filter(|m| !intro.asked_about.contains_key(m) && !intro.greeted_by.contains_key(m))
            .collect();
        let pick = (random_fraction() * fresh.len() as f64) as usize;
        let target = *fresh.get(pick.min(fresh.len().saturating_sub(1)))?;
        intro.asked_about.insert(target, now);
        Some(target)
    }

    /// Notes a request to be introduced made, and whether it was passed on.
    pub fn asked(&mut self, passed_on: bool) {
        self.intro.counts.asked += 1;
        self.intro.counts.asked_passed_on += passed_on as u64;
    }

    /// `asker` asks this machine, `me`, to pass on its request to be
    /// introduced. It's passed on, in the next round, if it's the asker's own,
    /// names this machine, hasn't expired, and names another machine met
    /// lately, as `Known` would, within the day's limits for the asker and for
    /// all. Returns whether it will be.
    pub fn pass_on(
        &mut self,
        asker: &EndpointId,
        me: &EndpointId,
        request: IntroduceRequest,
        now: i64,
    ) -> bool {
        let Ok(info) = request.verify() else {
            return false;
        };
        let since = now - ms(self.times.met_lately);
        let met_lately = self
            .entries
            .get(&info.target)
            .is_some_and(|e| e.met_ms.is_some() && e.answered_ms.is_some_and(|at| at >= since));
        if info.asker != *asker
            || info.via != *me
            || info.target == *asker
            || current(&info.expires, now).is_none()
            || !met_lately
        {
            return false;
        }
        let day_ago = now - self.times.day();
        let intro = &mut self.intro;
        intro.passed.retain(|(at, _)| *at > day_ago);
        let for_asker = intro.passed.iter().filter(|(_, a)| a == asker).count();
        if for_asker >= PASSED_ON_PER_ASKER || intro.passed.len() >= PASSED_ON_PER_DAY {
            intro.counts.refused_at_limit += 1;
            return false;
        }
        intro.passed.push((now, *asker));
        intro.to_pass_on.push((info.target, request));
        intro.counts.passed_on += 1;
        true
    }

    /// The requests to pass on now, and to whom.
    pub fn take_to_pass_on(&mut self) -> Vec<(EndpointId, IntroduceRequest)> {
        std::mem::take(&mut self.intro.to_pass_on)
    }

    /// `via` passed on a request to be introduced to this machine, `me`.
    /// Accepted, the asker is held among those who greet, introduced by
    /// `via`, and dialled ahead of those that only greeted. Returns whether it
    /// was; `via` isn't told.
    pub fn introduced(
        &mut self,
        via: &EndpointId,
        me: &EndpointId,
        request: &IntroduceRequest,
        now: i64,
    ) -> bool {
        let accepted = self.accept(via, me, request, now);
        match accepted {
            true => self.intro.counts.accepted += 1,
            false => self.intro.counts.declined += 1,
        }
        accepted
    }

    fn accept(
        &mut self,
        via: &EndpointId,
        me: &EndpointId,
        request: &IntroduceRequest,
        now: i64,
    ) -> bool {
        let Ok(info) = request.verify() else {
            return false;
        };
        let Some(expires) = current(&info.expires, now) else {
            return false;
        };
        if info.via != *via || info.target != *me || info.asker == *via {
            return false;
        }
        // Each request once, however many times it's passed on.
        self.intro.seen.retain(|_, until| *until > now);
        if self.intro.seen.len() >= MAX_SEEN || self.intro.seen.contains_key(&info.id) {
            return false;
        }
        self.intro.seen.insert(info.id, expires);
        // An introducer first met a few days ago, still answering, whose
        // records checked out.
        let trusted = self.entries.get(via).is_some_and(|e| {
            e.met_ms
                .is_some_and(|met| met <= now - ms(self.times.introducer_after))
                && self.answering(e, now)
                && checked_out(&e.counts)
        });
        if !trusted {
            return false;
        }
        let hour_ago = now - ms(self.times.hour);
        self.intro.accepted.retain(|at| *at > hour_ago);
        if self.intro.accepted.len() >= ACCEPTED_PER_HOUR {
            return false;
        }
        // A machine is introduced once: one met, or held otherwise, gains
        // nothing, and one waiting after a greeting is dialled sooner.
        match self.entries.get_mut(&info.asker) {
            Some(e) if e.met_ms.is_none() && e.source == Source::Greeted => {
                e.source = Source::Introduced(*via);
                self.changed.insert(info.asker);
            }
            Some(_) => return false,
            None => match self.hear(&request.record, Source::Introduced(*via), now) {
                Ok(Heard::Added | Heard::Turn) => {}
                _ => return false,
            },
        }
        self.intro.accepted.push(now);
        true
    }

    pub fn introduction_counts(&self) -> IntroductionCounts {
        self.intro.counts
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
                source: e.source.passer(),
                kept_longest: protected.contains(&e.machine),
                greeters_share: e.source.share() == Source::Greeted,
                introduced: matches!(e.source, Source::Introduced(_)),
            })
            .collect()
    }
}

/// Whether the records a machine passed on checked out. One that didn't is
/// forgiven, as a machine can go for good between being passed on and being
/// dialled.
fn checked_out(c: &Counts) -> bool {
    c.bad <= c.good + 1.0
}

/// Whether a request's expiry is still to come, and no further off than a
/// request may run.
fn current(expires: &str, now: i64) -> Option<i64> {
    let at = osp_proto::parse_rfc3339_ms(expires)?;
    let longest = (MAX_INTRODUCE_LIFETIME_SECS * 1000) as i64 + RECORD_MAX_AHEAD_MS;
    (at > now && at <= now + longest).then_some(at)
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
    /// The machine whose record of it came first, or that introduced it, or
    /// none if it greeted this one unasked.
    pub source: Option<EndpointId>,
    /// Whether it holds one of the places kept for the machines known longest.
    pub kept_longest: bool,
    /// Whether it counts against those who greet: it greeted, was introduced,
    /// or was passed on through a machine that greeted (ADR 0016).
    pub greeters_share: bool,
    /// Whether a machine introduced it.
    pub introduced: bool,
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
    pub fn greeted(&self, machine: &EndpointId, record: &MachineRecord) {
        let now = now_ms();
        if let Err(e) = self.with(|t| {
            t.greeted_by(machine, now);
            t.hear(record, Source::Greeted, now)
        }) {
            debug!("not keeping a greeting's record: {e:#}");
        }
    }

    /// Whether to ask to be introduced, and to which of the records `via` just
    /// passed on.
    pub fn target_to_ask(&self, via: &EndpointId, records: &[MachineRecord]) -> Option<EndpointId> {
        let now = now_ms();
        self.with(|t| match t.should_ask(now) {
            true => t.target_to_ask(via, records, now),
            false => None,
        })
    }

    pub fn asked(&self, passed_on: bool) {
        self.with(|t| t.asked(passed_on));
    }

    pub fn pass_on(&self, asker: &EndpointId, me: &EndpointId, request: IntroduceRequest) -> bool {
        self.with(|t| t.pass_on(asker, me, request, now_ms()))
    }

    pub fn take_to_pass_on(&self) -> Vec<(EndpointId, IntroduceRequest)> {
        self.with(|t| t.take_to_pass_on())
    }

    pub fn introduced(&self, via: &EndpointId, me: &EndpointId, request: &IntroduceRequest) {
        self.with(|t| t.introduced(via, me, request, now_ms()));
    }

    pub fn introduction_counts(&self) -> IntroductionCounts {
        self.with(|t| t.introduction_counts())
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
            // A machine that gave up its place while it was being dialled, as
            // one taking a turn may, passes on nothing: what it passed on can't
            // be told from a source of its own.
            Ok(_) if !t.holds(&machine) => true,
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
        // Requests to be introduced, passed on as they came. Whether the
        // machine introduced to accepts isn't told.
        let _: Vec<()> = stream::iter(acquaintances.take_to_pass_on())
            .map(|(target, request)| async move {
                if side.pass_on_to(target, request).await.is_err() {
                    debug!("couldn't pass on a request to be introduced");
                }
            })
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
        let protected = t.protected(now);
        assert!(old.iter().all(|m| protected.contains(m)));
        // Passed on by names that came in by greeting, they all count against
        // those who greet, so the crowd holds that share and leaves the rest.
        let crowd = t.entries.keys().filter(|m| !old.contains(m)).count();
        assert!(crowd <= PER_SOURCE + GREETER_TURNS, "{crowd}");
        assert!(shares_of(&t, now).values().all(|n| *n <= PER_SOURCE));
    }

    #[test]
    fn a_crowd_that_came_in_by_greeting_stays_in_its_share_until_known() {
        let now = start();
        let mut t = table();
        let greeter = hear_many(&mut t, Source::Greeted, 1, now)[0];
        meet(&mut t, &greeter, now);
        // What it passes on counts against those who greet: 31 more, not 32
        // of its own.
        let passed = hear_many(&mut t, Source::Machine(greeter), 50, now);
        assert_eq!(passed.len(), PER_SOURCE - 1);
        let (_, record) = machine(now);
        assert_eq!(
            t.hear(&record, Source::Machine(greeter), now).unwrap(),
            Heard::SourceFull
        );
        // So does what those pass on, once met, and they never take a turn.
        meet(&mut t, &passed[0], now);
        let (_, record) = machine(now);
        assert_eq!(
            t.hear(&record, Source::Machine(passed[0]), now).unwrap(),
            Heard::SourceFull
        );
        // Their passer still answers for them.
        meet(&mut t, &passed[1], now);
        assert_eq!(t.get(&greeter).unwrap().counts.good, 2.0);
        // Another source is unaffected.
        assert_eq!(
            hear_many(&mut t, Source::Machine(machine(now).0), 3, now).len(),
            3
        );
        assert_eq!(shares_of(&t, now).get(&Source::Greeted), Some(&PER_SOURCE));

        // Once known, it's a source of its own.
        let later = now + 31 * DAY_MS;
        keep_answering(&mut t, &[greeter], now, later);
        assert_eq!(stage_of(&t, &greeter, later), Some(Stage::Known));
        let (id, record) = machine(later);
        assert_eq!(
            t.hear(&record, Source::Machine(greeter), later).unwrap(),
            Heard::Added
        );
        assert_eq!(t.get(&id).unwrap().source, Source::Machine(greeter));
    }

    #[test]
    fn a_machine_greeting_takes_the_place_of_one_passed_on_through_greeters() {
        let now = start();
        let mut t = table();
        // One name greets and is met, and what it passes on fills the share.
        let greeter = hear_many(&mut t, Source::Greeted, 1, now)[0];
        meet(&mut t, &greeter, now);
        let passed = hear_many(&mut t, Source::Machine(greeter), PER_SOURCE, now + 1);
        assert_eq!(passed.len(), PER_SOURCE - 1);
        meet(&mut t, &passed[0], now + 2);

        // Machines greeting take their places, those never met first.
        let greeting = hear_many(&mut t, Source::Greeted, PER_SOURCE - 2, now + 3);
        assert_eq!(greeting.len(), PER_SOURCE - 2);
        assert_eq!(passed.iter().filter(|m| t.get(m).is_some()).count(), 1);
        assert!(t.get(&passed[0]).is_some());
        // Then the one met; with none left, a machine greeting takes a turn.
        assert_eq!(hear_many(&mut t, Source::Greeted, 1, now + 4).len(), 1);
        assert!(t.get(&passed[0]).is_none());
        let (_, record) = machine(now + 5);
        assert_eq!(
            t.hear(&record, Source::Greeted, now + 5).unwrap(),
            Heard::Turn
        );
        // The name that greeted keeps its place: it greeted too.
        assert!(t.get(&greeter).is_some());
        assert_eq!(
            shares_of(&t, now + 5).get(&Source::Greeted),
            Some(&(PER_SOURCE + 1))
        );
    }

    #[test]
    fn sources_read_back_as_written() {
        let m = machine(start()).0;
        for source in [
            Source::Greeted,
            Source::Machine(m),
            Source::ThroughGreeter(m),
            Source::Introduced(m),
        ] {
            assert_eq!(Source::from_db(&source.to_db()).unwrap(), source);
        }
        assert!(Source::from_db("through:nonsense").is_err());
        assert!(Source::from_db("introduced:nonsense").is_err());
    }

    /// A request from `asker` to be introduced through `via` to `target`,
    /// running an hour from `now`.
    fn request(
        asker: &SecretKey,
        via: EndpointId,
        target: EndpointId,
        now: i64,
    ) -> IntroduceRequest {
        let mut id = [0; 16];
        getrandom::fill(&mut id).unwrap();
        let expires = date(now + HOUR_MS);
        IntroduceRequest::new(asker, id, via, target, expires, record_at(asker, now)).unwrap()
    }

    /// A table that first met a machine four days ago and still hears from
    /// it, and that machine.
    fn with_introducer(now: i64) -> (Table, EndpointId) {
        let then = now - 4 * DAY_MS;
        let mut t = table();
        let met = hear_many(&mut t, Source::Machine(machine(then).0), 1, then);
        keep_answering(&mut t, &met, then, now);
        (t, met[0])
    }

    #[test]
    fn a_machine_passes_on_its_askers_own_requests_for_machines_met_lately_within_limits() {
        let now = start();
        let me = machine(now).0;
        let mut t = table();
        let met = hear_many(&mut t, Source::Machine(machine(now).0), 2, now);
        meet(&mut t, &met[0], now);
        let (target, unmet) = (met[0], met[1]);
        let asker = SecretKey::generate();
        let a = asker.public();

        let ok = request(&asker, me, target, now);
        assert!(t.pass_on(&a, &me, ok.clone(), now));
        assert_eq!(t.take_to_pass_on(), vec![(target, ok.clone())]);
        // Another machine's request, one naming another machine to pass it
        // on, or one naming a machine not met.
        let other = machine(now).0;
        assert!(!t.pass_on(&other, &me, ok, now));
        assert!(!t.pass_on(&a, &me, request(&asker, other, target, now), now));
        assert!(!t.pass_on(&a, &me, request(&asker, me, unmet, now), now));
        // One that has expired, or runs longer than a request may.
        for expires in [date(now - 1000), date(now + 3 * HOUR_MS)] {
            let late =
                IntroduceRequest::new(&asker, [1; 16], me, target, expires, record_at(&asker, now))
                    .unwrap();
            assert!(!t.pass_on(&a, &me, late, now));
        }
        assert!(t.take_to_pass_on().is_empty());

        // Three a day for one asker, twelve for all.
        for _ in 1..PASSED_ON_PER_ASKER {
            assert!(t.pass_on(&a, &me, request(&asker, me, target, now), now));
        }
        assert!(!t.pass_on(&a, &me, request(&asker, me, target, now), now));
        for i in PASSED_ON_PER_ASKER..PASSED_ON_PER_DAY {
            let other = SecretKey::generate();
            let at = now + i as i64;
            assert!(t.pass_on(&other.public(), &me, request(&other, me, target, at), at));
        }
        let late = SecretKey::generate();
        assert!(!t.pass_on(
            &late.public(),
            &me,
            request(&late, me, target, now),
            now + 20
        ));
        assert_eq!(t.introduction_counts().passed_on, PASSED_ON_PER_DAY as u64);
        assert_eq!(t.introduction_counts().refused_at_limit, 2);
        // A day on, there's room again.
        meet(&mut t, &target, now + DAY_MS);
        let then = now + DAY_MS + 1;
        assert!(t.pass_on(&a, &me, request(&asker, me, target, then), then));
    }

    #[test]
    fn a_machine_introduced_counts_against_those_who_greet_and_is_dialled_first() {
        let now = start();
        let me = machine(now).0;
        let (mut t, alice) = with_introducer(now);
        let greeters = hear_many(&mut t, Source::Greeted, 4, now);
        let jimmy = SecretKey::generate();
        assert!(t.introduced(&alice, &me, &request(&jimmy, alice, me, now), now));

        let held = t.get(&jimmy.public()).unwrap();
        assert_eq!(held.source, Source::Introduced(alice));
        assert_eq!(shares_of(&t, now)[&Source::Greeted], greeters.len() + 1);
        let a = t.acquaintances(now);
        let a = a.iter().find(|a| a.machine == jimmy.public()).unwrap();
        assert!(a.introduced && a.greeters_share && a.source == Some(alice));
        // The hour's turn for those who greet goes to it, ahead of the rest.
        assert_eq!(t.plan(now + 1).dials[0], jimmy.public());

        // Met, it credits the introducer; what it passes on counts against
        // those who greet, as for a greeter, until it's known.
        let good = t.get(&alice).unwrap().counts.good;
        meet(&mut t, &jimmy.public(), now + 2);
        assert!(t.get(&alice).unwrap().counts.good > good);
        let source = t.counted_as(Source::Machine(jimmy.public()), now + 3);
        assert_eq!(source, Source::ThroughGreeter(jimmy.public()));
    }

    #[test]
    fn introductions_come_from_machines_met_days_ago_one_an_hour_each_request_once() {
        let now = start();
        let me = machine(now).0;
        let (mut t, alice) = with_introducer(now);
        // Met only lately.
        let lately = hear_many(&mut t, Source::Machine(machine(now).0), 1, now - DAY_MS)[0];
        keep_answering(&mut t, &[lately], now - DAY_MS, now);
        let asker = || SecretKey::generate();
        assert!(!t.introduced(&lately, &me, &request(&asker(), lately, me, now), now));
        // Not passed on by the machine it names, not to this machine, or by
        // the asker itself.
        let j = asker();
        assert!(!t.introduced(&lately, &me, &request(&j, alice, me, now), now));
        assert!(!t.introduced(&alice, &me, &request(&j, alice, lately, now), now));

        let first = request(&asker(), alice, me, now);
        assert!(t.introduced(&alice, &me, &first, now));
        // One an hour.
        let next = request(&asker(), alice, me, now);
        assert!(!t.introduced(&alice, &me, &next, now + 60_000));
        let then = now + HOUR_MS + 1;
        assert!(t.introduced(&alice, &me, &request(&asker(), alice, me, then), then));
        // Seen before: dropped, even an hour on. Declined once, it's dropped
        // too, so trying it again gets nothing.
        let later = then + HOUR_MS;
        assert!(!t.introduced(&alice, &me, &first, later));
        assert!(!t.introduced(&alice, &me, &next, later));
        // A machine is introduced once, however many pass it on.
        let held = SecretKey::generate();
        let once = request(&held, alice, me, later);
        assert!(t.introduced(&alice, &me, &once, later));
        let again = later + HOUR_MS + 1;
        assert!(!t.introduced(&alice, &me, &request(&held, alice, me, again), again));
        // One waiting after its greeting is dialled sooner.
        let greeted = SecretKey::generate();
        t.hear(&record_at(&greeted, again), Source::Greeted, again)
            .unwrap();
        assert!(t.introduced(&alice, &me, &request(&greeted, alice, me, again), again));
        assert_eq!(
            t.get(&greeted.public()).unwrap().source,
            Source::Introduced(alice)
        );

        // An introducer whose records failed is no introducer.
        for _ in 0..3 {
            t.passed_on(&alice, false, again);
        }
        let last = again + HOUR_MS + 1;
        assert!(!t.introduced(&alice, &me, &request(&asker(), alice, me, last), last));
        assert_eq!(t.introduction_counts().accepted, 4);
    }

    #[test]
    fn a_machine_introduced_keeps_its_turn_until_dialled() {
        let now = start();
        let me = machine(now).0;
        let (mut t, alice) = with_introducer(now);
        hear_many(&mut t, Source::Greeted, PER_SOURCE, now);
        // Heard of after those, so it takes a turn.
        let jimmy = SecretKey::generate();
        let then = now + 1;
        assert!(t.introduced(&alice, &me, &request(&jimmy, alice, me, then), then));
        let turns: Vec<EndpointId> = (2..=GREETER_TURNS as i64 + 4)
            .map(|i| {
                let (id, record) = machine(now + i);
                assert_eq!(
                    t.hear(&record, Source::Greeted, now + i).unwrap(),
                    Heard::Turn
                );
                id
            })
            .collect();
        assert!(t.get(&jimmy.public()).is_some());
        assert_eq!(t.len(), 1 + PER_SOURCE + GREETER_TURNS);
        // Met, it gives its turn up like the rest.
        meet(&mut t, &jimmy.public(), now + 20);
        for i in 0..GREETER_TURNS as i64 {
            let (_, record) = machine(now + 30 + i);
            t.hear(&record, Source::Greeted, now + 30 + i).unwrap();
        }
        assert!(t.get(&jimmy.public()).is_none());
        assert!(turns.iter().all(|m| t.get(m).is_none()));
    }

    #[test]
    fn a_machine_asks_while_few_it_met_have_greeted_it() {
        let now = start();
        let mut t = table();
        assert!(t.should_ask(now));
        // Strangers greeting don't count, nor do machines only heard of.
        let heard = hear_many(&mut t, Source::Machine(machine(now).0), 32, now);
        for m in &heard {
            t.greeted_by(m, now);
        }
        assert!(t.should_ask(now));
        let met = hear_many(&mut t, Source::Machine(machine(now).0), 32, now);
        for (i, m) in met.iter().enumerate() {
            meet(&mut t, m, now);
            assert!(t.should_ask(now), "{i}");
            t.greeted_by(m, now);
        }
        assert!(!t.should_ask(now));
        // A week on, without their greetings, it asks again.
        assert!(t.should_ask(now + 8 * DAY_MS));

        // It picks from the records just passed on, each once, never the
        // machine asked, this machine or a directory, nor one that greeted it.
        let me = machine(now);
        let directory = machine(now);
        let mut t = Table::new([me.0, directory.0].into(), times());
        let via = machine(now);
        let (a, b) = (machine(now), machine(now));
        let greeter = hear_many(&mut t, Source::Machine(machine(now).0), 1, now)[0];
        meet(&mut t, &greeter, now);
        t.greeted_by(&greeter, now);
        let greeter_record = t.get(&greeter).unwrap().record.clone();
        let records = [me.1, directory.1, via.1.clone(), a.1, b.1, greeter_record];
        let mut picked: Vec<EndpointId> = (0..3)
            .filter_map(|_| t.target_to_ask(&via.0, &records, now))
            .collect();
        picked.sort();
        let mut both = vec![a.0, b.0];
        both.sort();
        assert_eq!(picked, both);
    }

    #[test]
    fn a_crowds_names_passed_on_are_no_source_heard_from_less_and_no_greeters() {
        let now = start();
        let mut t = table();
        // Four names greet, are met, and pass on five each.
        for greeter in hear_many(&mut t, Source::Greeted, 4, now) {
            meet(&mut t, &greeter, now);
            hear_many(&mut t, Source::Machine(greeter), 5, now);
        }
        // A source with more than any one of those names, heard of later.
        let few = hear_many(&mut t, Source::Machine(machine(now).0), 6, now + 1);

        let mut dialled = Vec::new();
        for minute in 0..20 {
            dialled.extend(t.plan(now + minute * 60_000).dials);
        }
        assert_eq!(dialled.len(), DIALS_PER_HOUR);
        // The crowd's 24 are one source, so the dials for sources heard from
        // less go to the six.
        assert!(few.contains(&dialled[0]) && few.contains(&dialled[3]));
        // And names passed on aren't dialled as machines that greeted.
        assert!(t.dials.iter().all(|d| !d.greeted_back));
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
        let (greeter, record) = machine(now);
        a.greeted(&greeter, &record);
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
        a.greeted(&talker, &record);
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
