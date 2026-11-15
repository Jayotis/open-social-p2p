//! What the node did or saw lately (ADR 0036 §1): a short list of events
//! for its owner's devices to draw. Each comes from something that
//! happened. Busy ones are summed, so one thing happening often is drawn
//! once. The list is the node's own: it goes in no backup, and nothing is
//! sent anywhere because of it.

use anyhow::{Context, Result};
use osp_proto::universe::{EVENT_HOURS, Event, EventWhat, MAX_EVENTS, SUM_MINUTES};
use rusqlite::{Connection, OptionalExtension, params};

const HOUR_MS: i64 = 60 * 60 * 1000;
const MINUTE_MS: i64 = 60 * 1000;

/// What an event is about, so that events about the same thing are summed.
fn subject(what: &EventWhat) -> String {
    let or_none = |v: Option<String>| v.unwrap_or_default();
    match what {
        EventWhat::Found { directory, .. } => format!("found:{directory}"),
        EventWhat::Reshared { from, author, .. } => format!("reshared:{from}:{author}"),
        EventWhat::Fetched { author, post, .. } => format!(
            "fetched:{}:{}",
            or_none(author.map(|a| a.to_string())),
            or_none(post.map(|p| p.to_string()))
        ),
        EventWhat::Served { author, post, .. } => format!(
            "served:{}:{}",
            or_none(author.map(|a| a.to_string())),
            or_none(post.map(|p| p.to_string()))
        ),
        EventWhat::Machine { words, met } => format!("machine:{words}:{met}"),
    }
}

/// `earlier` and `then` summed: counts added, the newest of the rest.
fn sum(earlier: EventWhat, then: EventWhat) -> EventWhat {
    match (earlier, then) {
        (EventWhat::Found { people: a, .. }, EventWhat::Found { directory, people }) => {
            EventWhat::Found {
                directory,
                people: a.saturating_add(people),
            }
        }
        (
            EventWhat::Fetched {
                bytes: a,
                host: earlier_host,
                kept_by_copy: earlier_kept,
                from_author: earlier_from_author,
                ..
            },
            EventWhat::Fetched {
                author,
                post,
                bytes,
                host,
                kept_by_copy,
                from_author,
            },
        ) => EventWhat::Fetched {
            author,
            post,
            bytes: a.saturating_add(bytes),
            host: host.or(earlier_host),
            kept_by_copy: kept_by_copy || earlier_kept,
            from_author: from_author || earlier_from_author,
        },
        (
            EventWhat::Served { bytes: a, .. },
            EventWhat::Served {
                author,
                post,
                bytes,
            },
        ) => EventWhat::Served {
            author,
            post,
            bytes: a.saturating_add(bytes),
        },
        // Reshares keep the newest post; a machine's turn is the same turn.
        (_, then) => then,
    }
}

/// Adds an event that happened at `now`: summed into one about the same
/// thing that started within `SUM_MINUTES`, which then comes again with a
/// new `seq`. Drops those older than `EVENT_HOURS`, and the oldest past
/// `MAX_EVENTS`. In a transaction the caller holds.
pub(crate) fn add(tx: &Connection, what: EventWhat, now: i64) -> Result<()> {
    let subject = subject(&what);
    let earlier: Option<(i64, i64, i64, String)> = tx
        .query_row(
            "SELECT seq, first_ms, count, what FROM events
             WHERE subject = ?1 AND first_ms > ?2 ORDER BY seq DESC LIMIT 1",
            params![subject, now - SUM_MINUTES as i64 * MINUTE_MS],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let (first, count, what) = match earlier {
        Some((seq, first, count, json)) => {
            tx.execute("DELETE FROM events WHERE seq = ?1", params![seq])?;
            let earlier: EventWhat = serde_json::from_str(&json).context("corrupt event")?;
            (first, count + 1, sum(earlier, what))
        }
        None => (now, 1, what),
    };
    tx.execute(
        "INSERT INTO events (subject, first_ms, last_ms, count, what)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![subject, first, now, count, serde_json::to_string(&what)?],
    )?;
    tx.execute(
        "DELETE FROM events WHERE last_ms < ?1",
        params![now - EVENT_HOURS as i64 * HOUR_MS],
    )?;
    tx.execute(
        "DELETE FROM events WHERE seq NOT IN
             (SELECT seq FROM events ORDER BY seq DESC LIMIT ?1)",
        params![MAX_EVENTS as i64],
    )?;
    Ok(())
}

/// The events after `since`, oldest first, from the last `EVENT_HOURS`
/// before `now`. A `since` past the newest `seq` ever given is from before
/// the node started afresh, so it gets them all.
pub(crate) fn since(conn: &Connection, since: Option<u64>, now: i64) -> Result<Vec<Event>> {
    let newest: i64 = conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'events'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let since = since
        .map(|s| s.min(i64::MAX as u64) as i64)
        .filter(|s| *s <= newest)
        .unwrap_or(0);
    let mut stmt = conn.prepare(
        "SELECT seq, first_ms, last_ms, count, what FROM events
         WHERE seq > ?1 AND last_ms >= ?2 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![since, now - EVENT_HOURS as i64 * HOUR_MS], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let minute = |ms: i64| (ms.div_euclid(MINUTE_MS) * 60).max(0) as u64;
    rows.map(|row| {
        let (seq, first, last, count, json) = row?;
        Ok(Event {
            seq: seq as u64,
            first: minute(first),
            last: minute(last),
            count: count.clamp(0, u32::MAX as i64) as u32,
            what: serde_json::from_str(&json).context("corrupt event")?,
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn found(people: u32) -> EventWhat {
        EventWhat::Found {
            directory: "able-baker-charlie".into(),
            people,
        }
    }

    fn served(bytes: u64) -> EventWhat {
        EventWhat::Served {
            author: None,
            post: None,
            bytes,
        }
    }

    #[test]
    fn events_about_one_thing_within_the_hour_are_summed_and_come_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let start = 1_000 * HOUR_MS;
        db.add_event_at(served(100), start).unwrap();
        db.add_event_at(found(2), start + MINUTE_MS).unwrap();
        let first = db.events_at(None, start + MINUTE_MS).unwrap();
        assert_eq!(first.len(), 2);
        let seen = first.last().unwrap().seq;

        // Within the hour: summed, and given again after what was seen.
        db.add_event_at(served(50), start + 30 * MINUTE_MS).unwrap();
        let again = db.events_at(Some(seen), start + 30 * MINUTE_MS).unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].count, 2);
        assert_eq!(again[0].first, (start / 1000) as u64);
        assert_eq!(again[0].what, served(150));
        assert_eq!(db.events_at(None, start + 30 * MINUTE_MS).unwrap().len(), 2);

        // Past the hour from the first: a new one.
        db.add_event_at(served(1), start + 61 * MINUTE_MS).unwrap();
        assert_eq!(db.events_at(None, start + 61 * MINUTE_MS).unwrap().len(), 3);

        // A day on, the old ones are gone, even unasked.
        let later = start + 26 * HOUR_MS;
        assert!(db.events_at(None, later).unwrap().is_empty());
        db.add_event_at(found(1), later).unwrap();
        let left = db.events_at(None, later).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].what, found(1));

        // A `since` from before a fresh start gets everything.
        assert_eq!(db.events_at(Some(10_000), later).unwrap().len(), 1);
        assert!(db.events_at(Some(left[0].seq), later).unwrap().is_empty());
    }

    #[test]
    fn at_most_max_events_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let now = 1_000 * HOUR_MS;
        for n in 0..MAX_EVENTS + 20 {
            let what = EventWhat::Machine {
                words: format!("machine-{n}"),
                met: true,
            };
            db.add_event_at(what, now).unwrap();
        }
        let kept = db.events_at(None, now).unwrap();
        assert_eq!(kept.len(), MAX_EVENTS);
        assert_eq!(
            kept[0].what,
            EventWhat::Machine {
                words: "machine-20".into(),
                met: true
            }
        );
    }
}
