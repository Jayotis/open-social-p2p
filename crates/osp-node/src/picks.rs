//! Which carriers a viewer's circuit goes through (ADR 0017 as amended
//! 2026-11-20, with the rules [carrier sizing](../../../docs/testing/carrier-sizing.md)
//! measured; ADR 0043 stage B part 4d).
//!
//! Say Jimmy's node fetches Alice's video, and her node named its hop Q.
//! Next to Jimmy's node goes P: a machine that finished jobs for it on 3
//! different days in the last 30 (standing), or else a seed carrier, and
//! not Q while another has room. A machine new to it never sits next to it,
//! as it would see Jimmy's address. Sometimes a machine with no standing
//! goes in the middle, H → Q → M → P → V, where it sees neither end:
//!
//! 1. one of Jimmy's 2 probationers, machines that finished a job for his
//!    node, once a day each, until they earn standing or 30 days pass;
//! 2. else, once a day, the machine that came into his table earliest of
//!    those in their own first 7 days by their signed start and new to his
//!    table in the last 7, that his node never finished a job with;
//! 3. else, on 1 transfer in 20, one from his table at random: one his node
//!    never finished a job with if there is one, else any with no standing.
//!
//! A fetch that passes iroh-blobs' hash check credits each hop that isn't a
//! seed carrier, once a day. A middle machine's job may take it into a
//! probation place: the first place takes any, the second only a first job.
//! A probationer whose circuit fails leaves.

use std::{collections::HashSet, sync::Mutex};

use anyhow::{Result, anyhow};
use iroh::EndpointId;
use osp_carrier::Descriptor;
use tracing::debug;

use crate::{
    db::{DAY_MS, Probationer, day_of, now_ms},
    meet::TableCarrier,
    node::Core,
    sync::random_fraction,
    viewing::in_own_order,
};

/// Probation places: the first for continuity, the second for a machine
/// this node never finished a job with.
const PROBATION_PLACES: usize = 2;
const PROBATION_MS: i64 = 30 * DAY_MS;
/// A machine is fresh for this long after it first came into the table,
/// and while its own start is this recent.
const FRESH_MS: i64 = 7 * DAY_MS;
/// The share of transfers through hops that try a machine at random in the
/// middle, when no probationer or fresh machine had the turn.
const NEWCOMER_RATE: f64 = 0.05;
/// Machines at standing tried for this node's own hop before the seeds:
/// each may need asking for its descriptor.
const STANDING_TRIED: usize = 2;

/// The day this node last gave a fresh turn.
#[derive(Default)]
pub(crate) struct FreshTurns(Mutex<Option<i64>>);

/// Why a machine has the middle of a circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MiddleTurn {
    Probation,
    Fresh,
    Drawn,
}

/// A machine picked for the middle.
#[derive(Debug, Clone)]
pub(crate) struct Middle {
    pub machine: EndpointId,
    pub descriptor: Descriptor,
    pub turn: MiddleTurn,
}

/// What this node knows of the machines it might pick, at one moment.
pub(crate) struct Known {
    pub now: i64,
    pub seeds: HashSet<EndpointId>,
    pub standing: HashSet<EndpointId>,
    pub probationers: Vec<Probationer>,
    pub table: Vec<TableCarrier>,
}

impl Known {
    fn may_be_middle(&self, machine: &EndpointId, taken: &[EndpointId]) -> bool {
        !taken.contains(machine)
            && !self.seeds.contains(machine)
            && !self.standing.contains(machine)
    }

    fn in_table(&self, machine: &EndpointId) -> Option<&TableCarrier> {
        self.table.iter().find(|c| c.machine == *machine)
    }
}

/// The middle machine, if one has a turn: a probationer that hasn't had one
/// today, else the oldest fresh machine if none had a fresh turn today
/// (`fresh_today`), else on `NEWCOMER_RATE` of transfers one at random
/// (`draw` is a fraction of 1). `worked` says whether this node ever
/// finished a job with a machine. None of `taken`, a seed carrier, or a
/// machine at standing.
pub(crate) fn pick_middle(
    known: &Known,
    taken: &[EndpointId],
    fresh_today: bool,
    draw: f64,
    worked: &dyn Fn(&EndpointId) -> bool,
) -> Option<Middle> {
    let today = day_of(known.now);
    let middle = |c: &TableCarrier, turn| Middle {
        machine: c.machine,
        descriptor: c.descriptor.clone(),
        turn,
    };
    for p in &known.probationers {
        if p.last_day != today
            && known.may_be_middle(&p.machine, taken)
            && let Some(c) = known.in_table(&p.machine)
        {
            return Some(middle(c, MiddleTurn::Probation));
        }
    }
    if !fresh_today {
        let mut fresh: Vec<(&TableCarrier, i64)> = known
            .table
            .iter()
            .filter(|c| known.may_be_middle(&c.machine, taken) && !worked(&c.machine))
            .filter_map(|c| {
                let start = c.start?;
                (known.now - start.first_seen_ms <= FRESH_MS
                    && known.now - start.started_ms <= FRESH_MS)
                    .then_some((c, start.first_seen_ms))
            })
            .collect();
        // Those that came in at the same moment in an order of this node's
        // own, not always the same one first.
        in_own_order(&mut fresh);
        fresh.sort_by_key(|(_, first_seen)| *first_seen);
        if let Some((c, _)) = fresh.first() {
            return Some(middle(c, MiddleTurn::Fresh));
        }
    }
    if draw >= NEWCOMER_RATE {
        return None;
    }
    let mut open: Vec<&TableCarrier> = known
        .table
        .iter()
        .filter(|c| known.may_be_middle(&c.machine, taken))
        .collect();
    in_own_order(&mut open);
    let never = open.iter().find(|c| !worked(&c.machine));
    never.or(open.first()).map(|c| middle(c, MiddleTurn::Drawn))
}

/// After a finished job by `machine`, whose first ever it was if `first`:
/// whether it takes a probation place. The first place takes any machine
/// without standing, the second only one on its first job.
pub(crate) fn takes_probation(known: &Known, machine: &EndpointId, first: bool) -> bool {
    let places = known.probationers.len();
    !known.seeds.contains(machine)
        && !known.standing.contains(machine)
        && !known.probationers.iter().any(|p| p.machine == *machine)
        && places < PROBATION_PLACES
        && (places == 0 || first)
}

impl Core {
    /// What this node knows of the machines it might pick now. Probationers
    /// that earned standing or have had their 30 days leave first.
    pub(crate) fn known_carriers(&self) -> Result<Known> {
        let now = now_ms();
        let seeds = self
            .config
            .carrier
            .known
            .iter()
            .map(|s| s.machine)
            .collect();
        let standing: HashSet<EndpointId> =
            self.db.carriers_at_standing(now)?.into_iter().collect();
        let mut probationers = Vec::new();
        for p in self.db.probationers()? {
            if standing.contains(&p.machine) || now - p.since_ms > PROBATION_MS {
                debug!(machine = %p.machine.fmt_short(), standing = standing.contains(&p.machine), "a probationer leaves");
                self.db.end_probation(&p.machine)?;
            } else {
                probationers.push(p);
            }
        }
        let mut table = match self.machine() {
            Ok(machine) => machine.acquaintances.carriers(crate::hops::unix_now()),
            Err(_) => Vec::new(),
        };
        if let Ok(me) = self.machine().map(|m| m.id()) {
            table.retain(|c| c.machine != me);
        }
        Ok(Known {
            now,
            seeds,
            standing,
            probationers,
            table,
        })
    }

    /// This node's own hop: one of the machines at standing with it, then
    /// the seed carriers, each in an order of its own, the first whose
    /// descriptor can be had. The host's hop `theirs` last, so the two ends
    /// meet in one hop only when nothing else answers.
    pub(crate) async fn own_hop(&self, known: &Known, theirs: EndpointId) -> Result<Descriptor> {
        let mut standing: Vec<EndpointId> = known
            .standing
            .iter()
            .filter(|m| !known.seeds.contains(*m) && **m != theirs)
            .copied()
            .collect();
        in_own_order(&mut standing);
        standing.truncate(STANDING_TRIED);
        let mut seeds = self.seeds_in_own_order();
        seeds.sort_by_key(|s| *s == theirs);
        let me = self.machine().map(|m| m.id()).ok();
        let mut last = anyhow!("no seed carrier is known");
        for machine in standing.into_iter().chain(seeds) {
            if Some(machine) == me {
                continue;
            }
            match self.seed_descriptor(machine, true).await {
                Ok(descriptor) => return Ok(descriptor),
                Err(e) => last = e,
            }
        }
        Err(last.context("no hop of this node's own could be had"))
    }

    /// The middle machine for a circuit between `taken`, if one has a turn,
    /// noting the turn.
    pub(crate) fn middle_hop(&self, known: &Known, taken: &[EndpointId]) -> Option<Middle> {
        let today = day_of(known.now);
        let mut fresh_day = self.fresh_turns.0.lock().unwrap_or_else(|e| e.into_inner());
        let worked = |m: &EndpointId| self.db.carrier_worked(m).unwrap_or(true);
        let middle = pick_middle(
            known,
            taken,
            *fresh_day == Some(today),
            random_fraction(),
            &worked,
        )?;
        match middle.turn {
            MiddleTurn::Probation => {
                if let Err(e) = self.db.probation_turn(&middle.machine, today) {
                    debug!("noting a probationer's turn: {e:#}");
                }
            }
            MiddleTurn::Fresh => *fresh_day = Some(today),
            MiddleTurn::Drawn => {}
        }
        debug!(machine = %middle.machine.fmt_short(), turn = ?middle.turn, "a machine has the middle");
        Some(middle)
    }

    /// Credits the hops of a circuit whose fetch passed the hash check,
    /// those that aren't seed carriers, once a day each, and takes the
    /// machine in the middle, if any, into probation if there's a place. The
    /// host's hop is the host's pick, and this node's own has standing or
    /// is a seed carrier.
    pub(crate) fn credit_hops(
        &self,
        known: &Known,
        hops: &[EndpointId],
        middle: Option<EndpointId>,
    ) -> Result<()> {
        for machine in hops.iter().filter(|m| !known.seeds.contains(*m)) {
            let now = now_ms();
            let first = self.db.credit_carrier(machine, now)?;
            if Some(*machine) == middle && takes_probation(known, machine, first) {
                self.db.add_probationer(machine, now)?;
                debug!(machine = %machine.fmt_short(), "a machine is taken on probation");
            }
        }
        Ok(())
    }

    /// A circuit with `middle` failed: a probationer there leaves.
    pub(crate) fn middle_failed(&self, middle: &Middle) {
        if middle.turn == MiddleTurn::Probation
            && let Err(e) = self.db.end_probation(&middle.machine)
        {
            debug!("ending a probation: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;
    use crate::meet::Start;

    fn carrier(start: Option<(i64, i64)>) -> TableCarrier {
        let key = SecretKey::generate();
        let machine = key.public();
        TableCarrier {
            machine,
            descriptor: test_descriptor(&key),
            start: start.map(|(started_ms, first_seen_ms)| Start {
                machine,
                started_ms,
                first_seen_ms,
            }),
        }
    }

    fn test_descriptor(key: &SecretKey) -> Descriptor {
        let circuit = osp_carrier::CircuitKey::new(crate::hops::unix_now());
        let info = osp_carrier::CarrierInfo {
            version: osp_carrier::VERSION,
            machine: key.public(),
            endpoint: SecretKey::generate().public(),
            relay: "https://relay.example/".into(),
            circuit_key: circuit.public(),
            valid_from: circuit.valid_from(),
            valid_until: circuit.valid_until(),
            offers: vec![],
        };
        Descriptor::new(key, &info).unwrap()
    }

    fn known(table: Vec<TableCarrier>) -> Known {
        Known {
            now: 100 * DAY_MS,
            seeds: HashSet::new(),
            standing: HashSet::new(),
            probationers: Vec::new(),
            table,
        }
    }

    const NEVER: fn(&EndpointId) -> bool = |_| false;

    #[test]
    fn a_probationer_has_the_middle_once_a_day() {
        let kim = carrier(None);
        let mut k = known(vec![kim.clone()]);
        k.probationers.push(Probationer {
            machine: kim.machine,
            since_ms: k.now - DAY_MS,
            last_day: day_of(k.now) - 1,
        });
        let m = pick_middle(&k, &[], true, 1.0, &NEVER).expect("its turn");
        assert_eq!((m.machine, m.turn), (kim.machine, MiddleTurn::Probation));
        k.probationers[0].last_day = day_of(k.now);
        assert!(pick_middle(&k, &[], true, 1.0, &NEVER).is_none());
        // Not when it's on the path already.
        k.probationers[0].last_day = 0;
        assert!(pick_middle(&k, &[kim.machine], true, 1.0, &NEVER).is_none());
    }

    #[test]
    fn the_earliest_in_of_the_fresh_has_a_fresh_turn() {
        let now = 100 * DAY_MS;
        let early = carrier(Some((now - DAY_MS, now - 3 * DAY_MS)));
        let late = carrier(Some((now - DAY_MS, now - DAY_MS)));
        // Says it started long ago, so it isn't new to the network.
        let old = carrier(Some((now - 60 * DAY_MS, now - 5 * DAY_MS)));
        // Started lately, but in the table over 7 days.
        let stale = carrier(Some((now - 2 * DAY_MS, now - 8 * DAY_MS)));
        let k = known(vec![late.clone(), old, stale, early.clone()]);
        let m = pick_middle(&k, &[], false, 1.0, &NEVER).expect("a fresh turn");
        assert_eq!((m.machine, m.turn), (early.machine, MiddleTurn::Fresh));
        // One already worked with isn't fresh.
        let worked = |m: &EndpointId| *m == early.machine;
        let m = pick_middle(&k, &[], false, 1.0, &worked).unwrap();
        assert_eq!(m.machine, late.machine);
        // Once a day.
        assert!(pick_middle(&k, &[], true, 1.0, &NEVER).is_none());
    }

    #[test]
    fn the_draw_prefers_a_machine_never_worked_with_and_skips_seeds_and_standing() {
        let [a, b, seed, standing] = [0, 1, 2, 3].map(|_| carrier(None));
        let mut k = known(vec![a.clone(), b.clone(), seed.clone(), standing.clone()]);
        k.seeds.insert(seed.machine);
        k.standing.insert(standing.machine);
        assert!(pick_middle(&k, &[], true, NEWCOMER_RATE, &NEVER).is_none());
        let worked = |m: &EndpointId| *m == a.machine;
        for _ in 0..20 {
            let m = pick_middle(&k, &[], true, 0.0, &worked).unwrap();
            assert_eq!((m.machine, m.turn), (b.machine, MiddleTurn::Drawn));
        }
        let all = |_: &EndpointId| true;
        for _ in 0..20 {
            let m = pick_middle(&k, &[], true, 0.0, &all).unwrap();
            assert!([a.machine, b.machine].contains(&m.machine));
        }
    }

    #[test]
    fn the_second_probation_place_takes_only_a_first_job() {
        let [kim, sam, seed] = [0, 1, 2].map(|_| SecretKey::generate().public());
        let mut k = known(Vec::new());
        k.seeds.insert(seed);
        assert!(
            takes_probation(&k, &kim, false),
            "the first place takes any"
        );
        assert!(!takes_probation(&k, &seed, true), "never a seed carrier");
        k.probationers.push(Probationer {
            machine: kim,
            since_ms: 0,
            last_day: 0,
        });
        assert!(!takes_probation(&k, &kim, true), "once");
        assert!(!takes_probation(&k, &sam, false));
        assert!(takes_probation(&k, &sam, true));
        k.probationers.push(Probationer {
            machine: sam,
            since_ms: 0,
            last_day: 0,
        });
        let other = SecretKey::generate().public();
        assert!(!takes_probation(&k, &other, true), "2 places");
    }
}
