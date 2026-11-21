//! What this node, as a viewer, writes down about the carriers its circuits
//! went through (ADR 0017's credit, ADR 0043 stage B part 4d): the days each
//! finished a job for it, which it has ever finished one with, and its
//! probationers.
//!
//! Say Jimmy's node fetches Alice's video through Kim's machine in the
//! middle, and the file passes its hash check. Jimmy's node notes today for
//! Kim's machine. Once Kim's has jobs on 3 different days in the last 30, it
//! has standing with Jimmy's node, which may then pick it next to itself.
//! Until then, after its first job, it may sit in one of Jimmy's 2
//! probation places, getting a middle job each day his node fetches.

use anyhow::Result;
use iroh::EndpointId;
use rusqlite::{OptionalExtension, params};

use super::{Db, day_of};

pub(crate) const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Days of jobs that make standing, within `STANDING_WINDOW_DAYS`.
pub(crate) const STANDING_DAYS: i64 = 3;
pub(crate) const STANDING_WINDOW_DAYS: i64 = 30;
/// Machines remembered as having finished a job, at most: the oldest go.
const MAX_WORKED: i64 = 10_000;
/// A machine's first middle job here makes it one to offer on, for this
/// many days after its last finished job here...
pub(crate) const TIP_DAYS: i64 = 30;
/// ...to at most this many who ask, ever: later jobs don't add more.
pub(crate) const TIP_HANDED: i64 = 2;
/// Suggestions this node keeps at once, each for `SUGGESTION_DAYS`.
pub(crate) const SUGGESTIONS_KEPT: i64 = 2;
pub(crate) const SUGGESTION_DAYS: i64 = 30;

/// A probationary carrier: a machine that finished a job for this node and
/// hasn't earned standing yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Probationer {
    pub machine: EndpointId,
    pub since_ms: i64,
    /// The day it last had a middle job from this node.
    pub last_day: i64,
}

impl Db {
    /// Notes that `machine` finished a job for this node at `now`. Whether
    /// it was its first ever here.
    pub(crate) fn credit_carrier(&self, machine: &EndpointId, now: i64) -> Result<bool> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let id = machine.to_string();
        tx.execute(
            "INSERT OR IGNORE INTO carrier_jobs (machine, day) VALUES (?1, ?2)",
            params![id, day_of(now)],
        )?;
        tx.execute(
            "DELETE FROM carrier_jobs WHERE day <= ?1",
            params![day_of(now) - STANDING_WINDOW_DAYS],
        )?;
        let first = tx.execute(
            "INSERT OR IGNORE INTO carriers_worked (machine, first_ms) VALUES (?1, ?2)",
            params![id, now],
        )? == 1;
        tx.execute(
            "DELETE FROM carriers_worked WHERE machine IN (
                 SELECT machine FROM carriers_worked ORDER BY first_ms DESC LIMIT -1 OFFSET ?1)",
            params![MAX_WORKED],
        )?;
        tx.commit()?;
        Ok(first)
    }

    /// The machines with standing here at `now`: jobs on `STANDING_DAYS`
    /// different days in the last `STANDING_WINDOW_DAYS`.
    pub(crate) fn carriers_at_standing(&self, now: i64) -> Result<Vec<EndpointId>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT machine FROM carrier_jobs WHERE day > ?1
             GROUP BY machine HAVING COUNT(*) >= ?2",
        )?;
        let rows = stmt.query_map(
            params![day_of(now) - STANDING_WINDOW_DAYS, STANDING_DAYS],
            |row| row.get::<_, String>(0),
        )?;
        let mut machines = Vec::new();
        for row in rows {
            if let Ok(machine) = row?.parse() {
                machines.push(machine);
            }
        }
        Ok(machines)
    }

    /// Whether `machine` ever finished a job for this node, as far as it
    /// remembers.
    pub(crate) fn carrier_worked(&self, machine: &EndpointId) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM carriers_worked WHERE machine = ?1",
                params![machine.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The probationers, the longest-held first: that one has the
    /// continuity place.
    pub(crate) fn probationers(&self) -> Result<Vec<Probationer>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT machine, since_ms, last_day FROM carrier_probation ORDER BY since_ms",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut list = Vec::new();
        for row in rows {
            let (machine, since_ms, last_day) = row?;
            if let Ok(machine) = machine.parse() {
                list.push(Probationer {
                    machine,
                    since_ms,
                    last_day,
                });
            }
        }
        Ok(list)
    }

    pub(crate) fn add_probationer(&self, machine: &EndpointId, now: i64) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO carrier_probation (machine, since_ms, last_day)
             VALUES (?1, ?2, ?3)",
            params![machine.to_string(), now, day_of(now)],
        )?;
        Ok(())
    }

    /// Notes that `machine` had a middle job from this node on `day`.
    pub(crate) fn probation_turn(&self, machine: &EndpointId, day: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE carrier_probation SET last_day = ?2 WHERE machine = ?1",
            params![machine.to_string(), day],
        )?;
        Ok(())
    }

    pub(crate) fn end_probation(&self, machine: &EndpointId) -> Result<()> {
        self.conn().execute(
            "DELETE FROM carrier_probation WHERE machine = ?1",
            params![machine.to_string()],
        )?;
        Ok(())
    }

    /// Notes that `machine` finished its first middle job for this node at
    /// `now`, so this node may offer it to those who ask. Only a first job
    /// makes a tip, so one handed out twice, or ended, isn't made again.
    pub(crate) fn add_tip(&self, machine: &EndpointId, now: i64) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO carrier_tips (machine, first_ms, last_ms)
             VALUES (?1, ?2, ?2)",
            params![machine.to_string(), now],
        )?;
        Ok(())
    }

    /// Notes a later finished job by `machine` at `now`: its tip, if any and
    /// not ended, lasts `TIP_DAYS` from now. Its handouts stay as they were.
    pub(crate) fn refresh_tip(&self, machine: &EndpointId, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE carrier_tips SET last_ms = MAX(last_ms, ?2) WHERE machine = ?1 AND ended = 0",
            params![machine.to_string(), now],
        )?;
        Ok(())
    }

    /// A circuit with `machine` in the middle failed: it's offered no more.
    pub(crate) fn end_tip(&self, machine: &EndpointId) -> Result<()> {
        self.conn().execute(
            "UPDATE carrier_tips SET ended = 1 WHERE machine = ?1",
            params![machine.to_string()],
        )?;
        Ok(())
    }

    /// The machine to offer someone who asks at `now`, if any, counted as
    /// handed once more: of those not ended, handed fewer than `TIP_HANDED`
    /// times and whose last job here was in the last `TIP_DAYS`, the one
    /// whose first job here was latest, so a newer machine goes ahead of one
    /// that keeps working, for which `usable` says yes. Those past their
    /// days go.
    pub(crate) fn hand_tip(
        &self,
        now: i64,
        usable: &dyn Fn(&EndpointId) -> bool,
    ) -> Result<Option<EndpointId>> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM carrier_tips WHERE last_ms < ?1",
            params![now - TIP_DAYS * DAY_MS],
        )?;
        let open: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT machine FROM carrier_tips WHERE handed < ?1 AND ended = 0
                 ORDER BY first_ms DESC",
            )?;
            stmt.query_map(params![TIP_HANDED], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let tip = open
            .iter()
            .filter_map(|m| m.parse::<EndpointId>().ok())
            .find(|m| usable(m));
        if let Some(machine) = tip {
            tx.execute(
                "UPDATE carrier_tips SET handed = handed + 1 WHERE machine = ?1",
                params![machine.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(tip)
    }

    /// Keeps `machine`, offered by `from`, as a suggestion until `until`,
    /// unless `SUGGESTIONS_KEPT` are held already. Whether it was kept.
    pub(crate) fn add_suggestion(
        &self,
        machine: &EndpointId,
        descriptor: &str,
        from: &EndpointId,
        until: i64,
    ) -> Result<bool> {
        let conn = self.conn();
        let held: i64 = conn.query_row("SELECT COUNT(*) FROM carrier_suggestions", [], |row| {
            row.get(0)
        })?;
        if held >= SUGGESTIONS_KEPT {
            return Ok(false);
        }
        Ok(conn.execute(
            "INSERT OR IGNORE INTO carrier_suggestions (machine, descriptor, from_machine, until_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![machine.to_string(), descriptor, from.to_string(), until],
        )? == 1)
    }

    /// The suggestions held at `now`, the earliest offered first, with
    /// each one's descriptor as last had. Those run out go.
    pub(crate) fn suggestions(&self, now: i64) -> Result<Vec<(EndpointId, String)>> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM carrier_suggestions WHERE until_ms <= ?1",
            params![now],
        )?;
        let mut stmt =
            conn.prepare("SELECT machine, descriptor FROM carrier_suggestions ORDER BY until_ms")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut list = Vec::new();
        for row in rows {
            let (machine, descriptor) = row?;
            if let Ok(machine) = machine.parse() {
                list.push((machine, descriptor));
            }
        }
        Ok(list)
    }

    /// A newer descriptor for a suggested machine.
    pub(crate) fn renew_suggestion(&self, machine: &EndpointId, descriptor: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE carrier_suggestions SET descriptor = ?2 WHERE machine = ?1",
            params![machine.to_string(), descriptor],
        )?;
        Ok(())
    }

    /// A suggestion had its turn: it goes, whether the job finished or not.
    pub(crate) fn end_suggestion(&self, machine: &EndpointId) -> Result<()> {
        self.conn().execute(
            "DELETE FROM carrier_suggestions WHERE machine = ?1",
            params![machine.to_string()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;

    #[test]
    fn standing_takes_jobs_on_3_days_within_30() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let kim = SecretKey::generate().public();
        let start = 100 * DAY_MS;
        assert!(db.credit_carrier(&kim, start).unwrap(), "a first job");
        assert!(!db.credit_carrier(&kim, start + 1).unwrap());
        // Two jobs on one day count once.
        db.credit_carrier(&kim, start + DAY_MS).unwrap();
        assert!(db.carriers_at_standing(start + DAY_MS).unwrap().is_empty());
        db.credit_carrier(&kim, start + 2 * DAY_MS).unwrap();
        assert_eq!(db.carriers_at_standing(start + 2 * DAY_MS).unwrap(), [kim]);
        // The first day falls out of the 30.
        let later = start + STANDING_WINDOW_DAYS * DAY_MS;
        assert!(db.carriers_at_standing(later).unwrap().is_empty());
        assert!(db.carrier_worked(&kim).unwrap(), "still worked with");
    }

    #[test]
    fn the_longest_held_probationer_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let [kim, sam] = [0, 1].map(|_| SecretKey::generate().public());
        db.add_probationer(&sam, 2 * DAY_MS).unwrap();
        db.add_probationer(&kim, DAY_MS).unwrap();
        db.add_probationer(&kim, 3 * DAY_MS).unwrap();
        db.probation_turn(&kim, 5).unwrap();
        let list = db.probationers().unwrap();
        assert_eq!(
            list.iter().map(|p| p.machine).collect::<Vec<_>>(),
            [kim, sam]
        );
        assert_eq!((list[0].since_ms, list[0].last_day), (DAY_MS, 5));
        db.end_probation(&kim).unwrap();
        assert_eq!(db.probationers().unwrap().len(), 1);
    }

    /// Mo's machine finished its first middle job for Jimmy's node, then
    /// Sam's did. Sam's, the newer, is handed first, to two who ask, then
    /// Mo's. Later work keeps Mo's going past its 30 days, but never adds
    /// handouts, and a tip isn't made again.
    #[test]
    fn a_tip_goes_to_two_askers_the_newest_first_while_its_machine_works() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let [mo, sam] = [0, 1].map(|_| SecretKey::generate().public());
        let start = 100 * DAY_MS;
        let any = |_: &EndpointId| true;
        let handed = |at| db.hand_tip(at, &any).unwrap();
        db.add_tip(&mo, start).unwrap();
        db.add_tip(&sam, start + 1).unwrap();
        assert_eq!(handed(start + 2), Some(sam));
        assert_eq!(handed(start + 3), Some(sam));
        let not_mo = |m: &EndpointId| *m != mo;
        assert_eq!(
            db.hand_tip(start + 4, &not_mo).unwrap(),
            None,
            "not in the table"
        );
        // Mo's machine works again on day 20, so it's offered past day 30.
        db.refresh_tip(&mo, start + 20 * DAY_MS).unwrap();
        assert_eq!(handed(start + 35 * DAY_MS), Some(mo));
        db.refresh_tip(&mo, start + 36 * DAY_MS).unwrap();
        db.add_tip(&mo, start + 36 * DAY_MS).unwrap();
        assert_eq!(handed(start + 37 * DAY_MS), Some(mo));
        assert_eq!(handed(start + 38 * DAY_MS), None, "two handouts, ever");
    }

    /// A tip ends when a circuit with its machine in the middle fails, and
    /// one whose machine stops working goes after 30 days.
    #[test]
    fn a_tip_ends_on_a_failed_job_or_after_30_days_without_one() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let [mo, sam] = [0, 1].map(|_| SecretKey::generate().public());
        let any = |_: &EndpointId| true;
        db.add_tip(&mo, 0).unwrap();
        db.add_tip(&sam, 0).unwrap();
        db.end_tip(&sam).unwrap();
        db.refresh_tip(&sam, DAY_MS).unwrap();
        assert_eq!(db.hand_tip(DAY_MS, &any).unwrap(), Some(mo));
        assert_eq!(db.hand_tip(TIP_DAYS * DAY_MS + 1, &any).unwrap(), None);
    }

    /// Alice's node keeps 2 suggestions at most, for 30 days, the earliest
    /// first, and one goes once it had its turn.
    #[test]
    fn suggestions_are_kept_two_at_most_until_they_run_out() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let [jimmy, mo, sam, kim] = [0, 1, 2, 3].map(|_| SecretKey::generate().public());
        let until = 30 * DAY_MS;
        assert!(db.add_suggestion(&mo, "mo's", &jimmy, until).unwrap());
        assert!(!db.add_suggestion(&mo, "mo's", &jimmy, until).unwrap());
        assert!(db.add_suggestion(&sam, "sam's", &jimmy, until + 1).unwrap());
        assert!(!db.add_suggestion(&kim, "kim's", &jimmy, until).unwrap());
        db.renew_suggestion(&mo, "newer").unwrap();
        assert_eq!(
            db.suggestions(0).unwrap(),
            [(mo, "newer".into()), (sam, "sam's".into())]
        );
        db.end_suggestion(&sam).unwrap();
        assert_eq!(db.suggestions(until).unwrap(), [], "Mo's ran out");
    }
}
