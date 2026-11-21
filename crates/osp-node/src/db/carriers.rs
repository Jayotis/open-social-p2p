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
}
