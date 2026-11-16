//! Opened posts (ADR 0039 §4). Jimmy sends Kim a code for Alice's post, and
//! Kim doesn't follow Alice. Kim's node fetches the post and Alice's profile
//! and keeps them where posts and profiles always are, as a found post's
//! are, so liking, replying and media work as for any post. This marks it:
//! for 7 days from when Kim last opened it, liked it or replied, within a
//! count and a byte cap. No list shows an opened post, nor someone whose
//! profile is held only because of one (`opened_people`).
//!
//! An opened post becomes an ordinary one, and its author someone held as
//! usual, as soon as it has another reason to be here: Kim follows Alice,
//! reshares it, or it arrives some other way.

use anyhow::{Context, Result, bail};
use iroh::PublicKey;
use osp_proto::{BlobHash, Note, ObjectId, Payload, Person, SignedObject};
use rusqlite::{Connection, OptionalExtension, params};

use super::{
    Db, declare_sizes, index_for_search, insert_object, profile_is_newer, profile_text, profiles,
    remove_from_search, upsert_profile,
};

/// How long an opened post is kept after it was last opened, liked or
/// replied to.
pub(crate) const OPENED_FOR_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// SQL that is true of an `objects` row that isn't an opened post.
pub(super) const NOT_OPENED: &str = "id NOT IN (SELECT id FROM opened_posts)";

/// What `keep_opened` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kept {
    /// It's kept as opened, newly or again.
    Opened,
    /// The node holds it already as an ordinary post: it opens as one.
    Held,
}

impl Db {
    /// Keeps `post`, `note`'s signed form, as opened until `now` plus
    /// `OPENED_FOR_MS`, and `profile` if it's newer than the one held. Its
    /// files are to be fetched as a found post's are (ADR 0024).
    pub(crate) fn keep_opened(
        &self,
        post: &SignedObject,
        note: &Note,
        profile: Option<(&Person, &SignedObject)>,
        now: i64,
    ) -> Result<Kept> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let id = post.id().to_string();
        let actor = note.actor.to_string();
        let held: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM objects WHERE id = ?1)",
            params![id],
            |row| row.get(0),
        )?;
        if held && !is_opened(&tx, &id)? {
            return Ok(Kept::Held);
        }
        if !held {
            if !insert_object(&tx, post, &Payload::Note(note.clone()), now, None)? {
                bail!("its author deleted it");
            }
            remove_from_search(&tx, &id)?;
            for (blob, size) in note.sized_blobs() {
                tx.execute(
                    "INSERT OR IGNORE INTO found_blobs (blob, post, size) VALUES (?1, ?2, ?3)",
                    params![blob.to_string(), id, size as i64],
                )?;
            }
        }
        let bytes = serde_json::to_vec(post)?.len() as i64
            + note.sized_blobs().map(|(_, size)| size as i64).sum::<i64>();
        tx.execute(
            "INSERT INTO opened_posts (id, actor, opened_ms, until_ms, bytes)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET until_ms = excluded.until_ms",
            params![id, actor, now, now + OPENED_FOR_MS, bytes],
        )?;
        // Their profile is held only for this if none was, or only for
        // opened posts.
        let profile_held: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM profiles WHERE actor = ?1)",
            params![actor],
            |row| row.get(0),
        )?;
        let only_opened = !profile_held || is_opened_person(&tx, &actor)?;
        if let Some((person, signed)) = profile
            && person.actor == note.actor
            && profile_is_newer(&tx, person)?
        {
            upsert_profile(&tx, person, signed)?;
        }
        if only_opened && profile_held_now(&tx, &actor)? {
            tx.execute(
                "INSERT OR IGNORE INTO opened_people (actor) VALUES (?1)",
                params![actor],
            )?;
            remove_from_search(&tx, &actor)?;
        }
        tx.commit()?;
        Ok(Kept::Opened)
    }

    /// Until when `id` is kept as opened, if it is.
    pub(crate) fn opened_until(&self, id: &ObjectId) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT until_ms FROM opened_posts WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Starts `id`'s 7 days over, if it's an opened post: Kim opened it
    /// again, liked it or replied. Returns whether it is one.
    pub(crate) fn touch_opened(&self, id: &ObjectId, now: i64) -> Result<bool> {
        Ok(self.conn().execute(
            "UPDATE opened_posts SET until_ms = ?2 WHERE id = ?1",
            params![id.to_string(), now + OPENED_FOR_MS],
        )? > 0)
    }

    /// The opened posts, the most lately opened, liked or replied to first.
    pub(crate) fn opened_posts(&self) -> Result<Vec<ObjectId>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id FROM opened_posts ORDER BY until_ms DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?.parse()?);
        }
        Ok(ids)
    }

    /// Drops the opened post `id` and what came only with it: its files
    /// that nothing else here names, and its author's profile if it was
    /// held only for opened posts. Returns the files to let go of, or None
    /// if `id` isn't an opened post.
    pub(crate) fn drop_opened(&self, id: &ObjectId) -> Result<Option<Vec<BlobHash>>> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let gone = drop_opened(&tx, &id.to_string())?;
        tx.commit()?;
        Ok(gone)
    }

    /// The opened posts to drop at `now`: those past their 7 days, then the
    /// least lately opened until at most `max_posts` and `max_bytes` are left.
    pub(crate) fn opened_to_drop(
        &self,
        now: i64,
        max_posts: usize,
        max_bytes: u64,
    ) -> Result<Vec<ObjectId>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id, until_ms, bytes FROM opened_posts ORDER BY until_ms DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let (mut kept, mut kept_bytes, mut drop) = (0usize, 0u64, Vec::new());
        for row in rows {
            let (id, until, bytes) = row?;
            let bytes = bytes.max(0) as u64;
            if until <= now || kept >= max_posts || kept_bytes + bytes > max_bytes {
                drop.push(id.parse()?);
            } else {
                kept += 1;
                kept_bytes += bytes;
            }
        }
        Ok(drop)
    }
}

fn is_opened(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM opened_posts WHERE id = ?1)",
        params![id],
        |row| row.get(0),
    )?)
}

fn is_opened_person(conn: &Connection, actor: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM opened_people WHERE actor = ?1)",
        params![actor],
        |row| row.get(0),
    )?)
}

fn profile_held_now(conn: &Connection, actor: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM profiles WHERE actor = ?1)",
        params![actor],
        |row| row.get(0),
    )?)
}

/// See `Db::drop_opened`.
pub(super) fn drop_opened(conn: &Connection, id: &str) -> Result<Option<Vec<BlobHash>>> {
    // Something else here points at it, a reshare held say: it isn't only
    // opened, so it stays as an ordinary post.
    let needed: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM opened_posts WHERE id = ?1)
             AND EXISTS (SELECT 1 FROM refs WHERE ref = ?1 AND id != ?1)",
        params![id],
        |row| row.get(0),
    )?;
    if needed {
        opened_post_is_kept(conn, id)?;
        return Ok(Some(Vec::new()));
    }
    let actor: Option<String> = conn
        .query_row(
            "DELETE FROM opened_posts WHERE id = ?1 RETURNING actor",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(actor) = actor else {
        return Ok(None);
    };
    let signed: Option<String> = conn
        .query_row(
            "SELECT signed FROM objects WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    let note = match signed {
        Some(json) => {
            let obj: SignedObject = serde_json::from_str(&json).context("corrupt stored object")?;
            match obj.verify() {
                Ok(Payload::Note(note)) => Some(note),
                _ => None,
            }
        }
        None => None,
    };
    conn.execute("DELETE FROM found_blobs WHERE post = ?1", params![id])?;
    conn.execute("DELETE FROM objects WHERE id = ?1", params![id])?;
    remove_from_search(conn, id)?;
    super::replies::forget_replies_to(conn, id)?;
    let mut gone = Vec::new();
    for blob in note.iter().flat_map(|n| n.blobs()) {
        let hex = blob.to_string();
        let named: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM refs WHERE ref = ?1)
                 OR EXISTS (SELECT 1 FROM icons WHERE blob = ?1)
                 OR EXISTS (SELECT 1 FROM found_blobs WHERE blob = ?1)",
            params![hex],
            |row| row.get(0),
        )?;
        if named || gone.contains(&blob) {
            continue;
        }
        for (table, column) in [
            ("blobs", "hash"),
            ("hosts", "blob"),
            ("swarm", "blob"),
            ("declared_sizes", "blob"),
        ] {
            conn.execute(
                &format!("DELETE FROM {table} WHERE {column} = ?1"),
                params![hex],
            )?;
        }
        gone.push(blob);
    }
    settle_opened_person(conn, &actor)?;
    Ok(Some(gone))
}

/// After one of `actor`'s opened posts went: if none is left and their
/// profile was held only for them, it goes too, unless something else
/// here needs it now.
pub(super) fn settle_opened_person(conn: &Connection, actor: &str) -> Result<()> {
    let left: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM opened_posts WHERE actor = ?1)",
        params![actor],
        |row| row.get(0),
    )?;
    if left || !is_opened_person(conn, actor)? {
        return Ok(());
    }
    conn.execute("DELETE FROM opened_people WHERE actor = ?1", params![actor])?;
    let n = conn.execute(
        "DELETE FROM profiles WHERE actor = ?1
             AND NOT EXISTS (SELECT 1 FROM follows WHERE actor = ?1)
             AND NOT EXISTS (SELECT 1 FROM found_people WHERE actor = ?1)
             AND NOT EXISTS (SELECT 1 FROM objects WHERE actor = ?1)",
        params![actor],
    )?;
    if n > 0 {
        profiles::forget_icon(conn, actor)?;
        remove_from_search(conn, actor)?;
    } else {
        index_profile(conn, actor)?;
    }
    Ok(())
}

/// An opened post has another reason to be here now: it's kept as an
/// ordinary post, and its author's profile as any held. Nothing if `id`
/// isn't an opened post.
pub(super) fn opened_post_is_kept(conn: &Connection, id: &str) -> Result<()> {
    let actor: Option<String> = conn
        .query_row(
            "DELETE FROM opened_posts WHERE id = ?1 RETURNING actor",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(actor) = actor else {
        return Ok(());
    };
    conn.execute("DELETE FROM found_blobs WHERE post = ?1", params![id])?;
    let signed: Option<(String, i64)> = conn
        .query_row(
            "SELECT signed, published_ms FROM objects WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((json, published)) = signed {
        let obj: SignedObject = serde_json::from_str(&json).context("corrupt stored object")?;
        if let Payload::Note(note) = obj.verify()? {
            declare_sizes(conn, &note, Some(&obj.id()))?;
            index_for_search(conn, id, &note.content, Some(published))?;
        }
    }
    opened_person_is_kept(conn, &actor)
}

/// `actor`'s profile has another reason to be here: lists may show them.
pub(super) fn opened_person_is_kept(conn: &Connection, actor: &str) -> Result<()> {
    if conn.execute("DELETE FROM opened_people WHERE actor = ?1", params![actor])? > 0 {
        index_profile(conn, actor)?;
    }
    Ok(())
}

/// Kim follows `actor` now: their opened posts are kept as any follow's.
pub(super) fn opened_author_is_followed(conn: &Connection, actor: &PublicKey) -> Result<()> {
    let ids: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM opened_posts WHERE actor = ?1")?;
        let rows = stmt.query_map(params![actor.to_string()], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for id in &ids {
        opened_post_is_kept(conn, id)?;
    }
    opened_person_is_kept(conn, &actor.to_string())
}

fn index_profile(conn: &Connection, actor: &str) -> Result<()> {
    let signed: Option<String> = conn
        .query_row(
            "SELECT signed FROM profiles WHERE actor = ?1",
            params![actor],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(json) = signed {
        let obj: SignedObject = serde_json::from_str(&json).context("corrupt stored profile")?;
        if let Ok(Payload::Person(person)) = obj.verify() {
            index_for_search(conn, actor, &profile_text(&person), None)?;
        }
    }
    Ok(())
}

/// `actor`'s profile stayed only because objects of theirs are here: if
/// those are all opened posts, and nothing else holds them, it's held only
/// for opened posts again.
pub(super) fn profile_only_for_opened(conn: &Connection, actor: &str) -> Result<()> {
    let only: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM opened_posts WHERE actor = ?1)
             AND EXISTS (SELECT 1 FROM profiles WHERE actor = ?1)
             AND NOT EXISTS (SELECT 1 FROM follows WHERE actor = ?1)
             AND NOT EXISTS (SELECT 1 FROM found_people WHERE actor = ?1)
             AND NOT EXISTS (SELECT 1 FROM objects WHERE actor = ?1
                                 AND id NOT IN (SELECT id FROM opened_posts))",
        params![actor],
        |row| row.get(0),
    )?;
    if only {
        conn.execute(
            "INSERT OR IGNORE INTO opened_people (actor) VALUES (?1)",
            params![actor],
        )?;
        remove_from_search(conn, actor)?;
    }
    Ok(())
}
