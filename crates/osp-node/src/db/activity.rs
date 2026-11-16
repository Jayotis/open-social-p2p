//! The owner's Activity (ADR 0038 §3): one row for each thing that
//! happened, by what it's about, with whether it's read. What an entry says
//! (names, counts, text) is read from where that's kept when it's shown, so
//! a like taken back or a reply deleted needs nothing here.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use super::Db;

/// Most entries kept. Past this, the oldest change goes.
pub const MAX_ACTIVITY: i64 = 1_000;

/// An Activity entry, as it's kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivityRow {
    pub key: String,
    pub seq: u64,
    pub first_ms: i64,
    pub last_ms: i64,
    pub read: bool,
}

/// Notes something for the owner's Activity under `key`. A key new here
/// gets an entry, read if `read`. One already here changes only if `again`,
/// as grouped entries do ("and 3 others liked"): it moves up and is unread.
pub(crate) fn note(conn: &Connection, key: &str, again: bool, read: bool, now: i64) -> Result<()> {
    let held: Option<i64> = conn
        .query_row(
            "SELECT first_ms FROM activity WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?;
    // A change takes a new `seq`, never one given before (AUTOINCREMENT), so
    // a device's "read through" never covers it.
    let (first, read) = match held {
        Some(_) if !again => return Ok(()),
        Some(first) => {
            conn.execute("DELETE FROM activity WHERE key = ?1", params![key])?;
            (first, false)
        }
        None => (now, read),
    };
    conn.execute(
        "INSERT INTO activity (key, first_ms, last_ms, read) VALUES (?1, ?2, ?3, ?4)",
        params![key, first, now, read],
    )?;
    conn.execute(
        "DELETE FROM activity WHERE seq IN (
             SELECT seq FROM activity ORDER BY seq DESC LIMIT -1 OFFSET ?1)",
        params![MAX_ACTIVITY],
    )?;
    Ok(())
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<ActivityRow> {
    Ok(ActivityRow {
        key: r.get(0)?,
        seq: r.get::<_, i64>(1)?.max(0) as u64,
        first_ms: r.get(2)?,
        last_ms: r.get(3)?,
        read: r.get(4)?,
    })
}

impl Db {
    /// Notes something for the owner's Activity (`note`).
    pub(crate) fn note_activity(&self, key: &str, again: bool, read: bool, now: i64) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        note(&tx, key, again, read, now)?;
        tx.commit()?;
        Ok(())
    }

    /// Entries, the latest change first: at most `limit` of those `before`
    /// a `seq`, or of all.
    pub(crate) fn activity_rows(
        &self,
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<ActivityRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT key, seq, first_ms, last_ms, read FROM activity
             WHERE seq < ?1 ORDER BY seq DESC LIMIT ?2",
        )?;
        let before = before.map_or(i64::MAX, |b| b.min(i64::MAX as u64) as i64);
        let rows = stmt
            .query_map(params![before, limit as i64], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The entries not read, the latest change first.
    pub(crate) fn unread_activity(&self) -> Result<Vec<ActivityRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT key, seq, first_ms, last_ms, read FROM activity
             WHERE read = 0 ORDER BY seq DESC",
        )?;
        let rows = stmt
            .query_map([], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Marks read the entries named, and with `through` every one whose
    /// `seq` is no later.
    pub(crate) fn mark_activity_read(&self, keys: &[String], through: Option<u64>) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        for key in keys {
            tx.execute("UPDATE activity SET read = 1 WHERE key = ?1", params![key])?;
        }
        if let Some(through) = through {
            tx.execute(
                "UPDATE activity SET read = 1 WHERE seq <= ?1",
                params![through.min(i64::MAX as u64) as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Drops an entry whose subject is gone.
    pub(crate) fn forget_activity(&self, key: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM activity WHERE key = ?1", params![key])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        (dir, db)
    }

    fn keys(rows: &[ActivityRow]) -> Vec<&str> {
        rows.iter().map(|r| r.key.as_str()).collect()
    }

    #[test]
    fn a_grouped_entry_moves_up_and_is_unread_again() {
        let (_dir, db) = open();
        db.note_activity("liked:p", true, false, 1).unwrap();
        db.note_activity("reply:r", false, false, 2).unwrap();
        db.mark_activity_read(&[], Some(u64::MAX)).unwrap();
        assert!(db.unread_activity().unwrap().is_empty());

        // A one-off entry noted again stays as it was.
        db.note_activity("reply:r", false, false, 3).unwrap();
        assert!(db.unread_activity().unwrap().is_empty());

        // A grouped one moves to the top, unread, keeping when it began.
        db.note_activity("liked:p", true, false, 4).unwrap();
        let rows = db.activity_rows(None, 10).unwrap();
        assert_eq!(keys(&rows), ["liked:p", "reply:r"]);
        assert_eq!(
            (rows[0].first_ms, rows[0].last_ms, rows[0].read),
            (1, 4, false)
        );
        assert!(rows[1].read);
    }

    #[test]
    fn read_through_a_seq_leaves_later_changes_unread() {
        let (_dir, db) = open();
        db.note_activity("a", false, false, 1).unwrap();
        db.note_activity("b", false, false, 2).unwrap();
        let seen = db.activity_rows(None, 10).unwrap()[0].seq;
        db.note_activity("c", false, false, 3).unwrap();
        db.note_activity("d", false, true, 4).unwrap();
        db.mark_activity_read(&[], Some(seen)).unwrap();
        assert_eq!(keys(&db.unread_activity().unwrap()), ["c"]);
        db.mark_activity_read(&["c".to_string()], None).unwrap();
        assert!(db.unread_activity().unwrap().is_empty());
    }

    #[test]
    fn pages_read_on_and_the_oldest_go_past_the_cap() {
        let (_dir, db) = open();
        for i in 0..MAX_ACTIVITY + 5 {
            db.note_activity(&format!("k{i}"), false, false, i).unwrap();
        }
        let all = db.activity_rows(None, usize::MAX >> 1).unwrap();
        assert_eq!(all.len() as i64, MAX_ACTIVITY);
        assert_eq!(all.last().unwrap().key, "k5");
        let page = db.activity_rows(Some(all[1].seq), 2).unwrap();
        assert_eq!(keys(&page), [all[2].key.as_str(), all[3].key.as_str()]);
        db.forget_activity("k5").unwrap();
        assert_eq!(
            db.activity_rows(None, 2000).unwrap().len() as i64,
            MAX_ACTIVITY - 1
        );
    }
}
