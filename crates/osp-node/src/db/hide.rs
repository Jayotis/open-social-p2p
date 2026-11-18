//! The project's hide list as this node applies it (ADR 0042 §6). A listed
//! post, a reshare of one, or a post naming a listed file is taken out of
//! every table that shows it and kept aside; a listed file isn't fetched or
//! served, and its copies go. The owner's own posts are never hidden: an
//! author keeps what they posted.
//!
//! When an entry is retracted, the posts it hid come back: those of people
//! the owner follows, and posts they reshared. Others, found through a
//! directory or opened from a code, come back as they're found again.
//!
//! People the owner excepts are treated as the owner is: their posts aren't
//! hidden, and their listed files may be fetched for the owner to see. No
//! listed file is ever served on, though, whoever's it is.

use anyhow::{Result, bail};
use iroh::PublicKey;
use osp_proto::{
    BlobHash, ObjectId, Payload, SignedObject,
    lists::{HideKind, HideList, file_entry, post_entry},
};
use rusqlite::{Connection, OptionalExtension, params};

use super::{Db, declare_sizes, drop_objects, forget_blob, insert_object};

/// What applying a list did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Posts and reshares taken out of view.
    pub hidden: usize,
    /// Posts and reshares back in view after a retraction.
    pub shown: usize,
    /// Files whose copies go: the caller lets them go from the store.
    pub files: Vec<BlobHash>,
}

/// The list as last applied here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HideState {
    pub key: PublicKey,
    pub seq: u64,
    pub published: String,
    /// Entries in force.
    pub entries: u64,
    pub fetched_ms: i64,
}

fn kind_name(kind: HideKind) -> &'static str {
    match kind {
        HideKind::Post => "post",
        HideKind::File => "file",
    }
}

fn hex(h: &[u8; 32]) -> String {
    data_encoding::HEXLOWER.encode(h)
}

fn listed(conn: &Connection, kind: HideKind, h: &[u8; 32]) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM hide_list WHERE kind = ?1 AND h = ?2",
            params![kind_name(kind), hex(h)],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn owner(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT owner FROM hide_state", [], |row| row.get(0))
        .optional()?)
}

fn any_listed(conn: &Connection) -> Result<bool> {
    Ok(
        conn.query_row("SELECT EXISTS (SELECT 1 FROM hide_list)", [], |row| {
            row.get(0)
        })?,
    )
}

/// Whether the list leaves `actor`'s posts alone: the owner, or someone
/// the owner excepted.
fn spared(conn: &Connection, actor: &str) -> Result<bool> {
    if owner(conn)?.is_some_and(|me| me == actor) {
        return Ok(true);
    }
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM hide_exceptions WHERE actor = ?1)",
        params![actor],
        |row| row.get(0),
    )?)
}

/// Whether the list hides `payload`, an object with ID `id`: the post
/// itself, a file it names, or for a reshare the post it points at, listed
/// or hidden here already. Never one of the owner's own, or of someone the
/// owner excepted; nor a reshare of such a post.
pub(super) fn hides_object(conn: &Connection, id: &ObjectId, payload: &Payload) -> Result<bool> {
    if !any_listed(conn)? || spared(conn, &payload.actor().to_string())? {
        return Ok(false);
    }
    if listed(conn, HideKind::Post, &post_entry(id))? {
        return Ok(true);
    }
    match payload {
        Payload::Note(note) => {
            for blob in note.blobs() {
                if listed(conn, HideKind::File, &file_entry(&blob))? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Payload::Announce(a) => {
            let aside = conn
                .query_row(
                    "SELECT 1 FROM hidden_objects WHERE id = ?1",
                    params![a.object.to_string()],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if aside {
                return Ok(true);
            }
            if !listed(conn, HideKind::Post, &post_entry(&a.object))? {
                return Ok(false);
            }
            // Listed, but shown here if its author is spared.
            let author: Option<String> = conn
                .query_row(
                    "SELECT actor FROM objects WHERE id = ?1",
                    params![a.object.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(match author {
                Some(author) => !spared(conn, &author)?,
                None => true,
            })
        }
        _ => Ok(false),
    }
}

/// Whether the list hides a reply with ID `id` by `actor`. Replies carry no
/// files, so only its own entry counts.
pub(super) fn hides_reply(conn: &Connection, id: &ObjectId, actor: &PublicKey) -> Result<bool> {
    Ok(any_listed(conn)?
        && !spared(conn, &actor.to_string())?
        && listed(conn, HideKind::Post, &post_entry(id))?)
}

/// Keeps `obj`, which the list hides, aside for a retraction.
pub(super) fn keep_aside(
    conn: &Connection,
    obj: &SignedObject,
    payload: &Payload,
    received: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO hidden_objects (id, kind, actor, signed, received_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            obj.id().to_string(),
            payload.kind(),
            payload.actor().to_string(),
            serde_json::to_string(obj)?,
            received
        ],
    )?;
    Ok(())
}

/// Whether the list holds `hash`: it isn't served, and isn't fetched
/// unless a post the list spares names it (see [`Db::may_fetch_listed`]).
pub(super) fn hides_file(conn: &Connection, hash: &BlobHash) -> Result<bool> {
    listed(conn, HideKind::File, &file_entry(hash))
}

/// Whether a post by the owner or someone excepted, held here, names `hash`.
fn named_by_spared(conn: &Connection, hash: &str, me: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM refs JOIN objects ON objects.id = refs.id
                        WHERE refs.ref = ?1
                          AND (objects.actor = ?2
                               OR objects.actor IN (SELECT actor FROM hide_exceptions)))",
        params![hash, me],
        |row| row.get(0),
    )?)
}

impl Db {
    /// Applies `list`, as fetched and checked: its entries replace the ones
    /// before, what it hides is taken out of view, and what a retraction
    /// frees comes back. `signed` is the file as served, kept for phones. A
    /// list older than the one held, from the same key, is refused.
    pub fn apply_hide_list(
        &self,
        list: &HideList,
        signed: &str,
        me: &PublicKey,
        fetched_ms: i64,
    ) -> Result<Applied> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let held: Option<(String, i64)> = tx
            .query_row("SELECT key, seq FROM hide_state", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        if let Some((key, seq)) = &held
            && *key == list.key.to_string()
        {
            if (list.seq as i64) < *seq {
                bail!("the list fetched is older than the one held");
            }
            if list.seq as i64 == *seq {
                tx.execute("UPDATE hide_state SET fetched_ms = ?1", params![fetched_ms])?;
                tx.commit()?;
                return Ok(Applied::default());
            }
        }
        tx.execute("DELETE FROM hide_list", [])?;
        let mut entries = 0;
        for e in list.active() {
            entries += tx.execute(
                "INSERT OR IGNORE INTO hide_list (kind, h) VALUES (?1, ?2)",
                params![kind_name(e.kind), hex(&e.h)],
            )?;
        }
        tx.execute(
            "INSERT INTO hide_state (one, key, seq, published, entries, fetched_ms, signed, owner)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (one) DO UPDATE SET key = excluded.key, seq = excluded.seq,
                 published = excluded.published, entries = excluded.entries,
                 fetched_ms = excluded.fetched_ms, signed = excluded.signed,
                 owner = excluded.owner",
            params![
                list.key.to_string(),
                list.seq as i64,
                list.published,
                entries as i64,
                fetched_ms,
                signed,
                me.to_string()
            ],
        )?;
        let mut applied = Applied {
            shown: bring_back(&tx)?,
            ..Applied::default()
        };
        hide_held(&tx, &me.to_string(), &mut applied)?;
        tx.commit()?;
        Ok(applied)
    }

    /// Stops applying the list, as when the owner turns it off: everything
    /// it hid comes back, as after a retraction.
    pub fn clear_hide_list(&self) -> Result<Applied> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM hide_list", [])?;
        tx.execute("DELETE FROM hide_state", [])?;
        let shown = bring_back(&tx)?;
        tx.commit()?;
        Ok(Applied {
            shown,
            ..Applied::default()
        })
    }

    /// The list as last applied, if one is.
    pub fn hide_state(&self) -> Result<Option<HideState>> {
        let conn = self.conn();
        let row: Option<(String, i64, String, i64, i64)> = conn
            .query_row(
                "SELECT key, seq, published, entries, fetched_ms FROM hide_state",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(key, seq, published, entries, fetched_ms)| {
            Ok(HideState {
                key: key.parse()?,
                seq: seq.max(0) as u64,
                published,
                entries: entries.max(0) as u64,
                fetched_ms,
            })
        })
        .transpose()
    }

    /// The list as last applied, as signed, for phones.
    pub fn hide_list_signed(&self) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT signed FROM hide_state", [], |row| row.get(0))
            .optional()?)
    }

    /// Whether the list holds `hash`.
    pub fn hides_file(&self, hash: &BlobHash) -> Result<bool> {
        hides_file(&self.conn(), hash)
    }

    /// Whether `hash`, which the list holds, may still be fetched: a post
    /// by the owner or someone the owner excepted names it.
    pub fn may_fetch_listed(&self, hash: &BlobHash) -> Result<bool> {
        let conn = self.conn();
        let me = owner(&conn)?.unwrap_or_default();
        named_by_spared(&conn, &hash.to_string(), &me)
    }

    /// Whether the list hides the reply `id` by `actor`.
    pub fn hides_reply(&self, id: &ObjectId, actor: &PublicKey) -> Result<bool> {
        hides_reply(&self.conn(), id, actor)
    }

    /// The people the owner excepted from the list, first excepted first,
    /// with their names if their profiles are held.
    pub fn hide_exceptions(&self) -> Result<Vec<(PublicKey, Option<String>)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT e.actor, p.name FROM hide_exceptions e
             LEFT JOIN profiles p ON p.actor = e.actor
             ORDER BY e.at_ms, e.actor",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        let mut people = Vec::new();
        for row in rows {
            let (actor, name) = row?;
            if let Ok(key) = actor.parse() {
                people.push((key, name));
            }
        }
        Ok(people)
    }

    /// Excepts `actor` from the list, or with `except` false takes that
    /// back. While a list is applied, what it now hides or spares changes at
    /// once: the files it lets go come back in `files`. Says nothing changed
    /// with `None`.
    pub fn set_hide_exception(
        &self,
        actor: &PublicKey,
        except: bool,
        now_ms: i64,
    ) -> Result<Option<Applied>> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let changed = if except {
            tx.execute(
                "INSERT OR IGNORE INTO hide_exceptions (actor, at_ms) VALUES (?1, ?2)",
                params![actor.to_string(), now_ms],
            )?
        } else {
            tx.execute(
                "DELETE FROM hide_exceptions WHERE actor = ?1",
                params![actor.to_string()],
            )?
        };
        if changed == 0 {
            return Ok(None);
        }
        let mut applied = Applied::default();
        if let Some(me) = owner(&tx)? {
            applied.shown = bring_back(&tx)?;
            hide_held(&tx, &me, &mut applied)?;
        }
        tx.commit()?;
        Ok(Some(applied))
    }
}

/// Puts back what the list no longer hides: posts by people the owner
/// follows and their reshares, and posts those reshares point at. The rest
/// of what's no longer hidden is let go.
fn bring_back(tx: &Connection) -> Result<usize> {
    let rows: Vec<(String, String, i64)> = {
        // Posts before reshares, so a reshare sees whether its post stays.
        let mut stmt = tx.prepare(
            "SELECT actor, signed, received_ms FROM hidden_objects
             ORDER BY kind = 'Announce'",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    tx.execute("DELETE FROM hidden_objects", [])?;
    // What stays hidden goes back aside; the rest is free.
    let mut free = Vec::new();
    for (actor, json, received) in rows {
        let Ok(obj) = serde_json::from_str::<SignedObject>(&json) else {
            continue;
        };
        let Ok(payload) = obj.verify() else {
            continue;
        };
        if hides_object(tx, &obj.id(), &payload)? {
            keep_aside(tx, &obj, &payload, received)?;
        } else {
            free.push((actor, obj, payload, received));
        }
    }
    // Reshares first now, so the posts they point at are known to be wanted.
    free.reverse();
    let mut wanted = std::collections::HashSet::new();
    let mut shown = 0;
    for (actor, obj, payload, received) in free {
        let followed: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM follows WHERE actor = ?1)",
            params![actor],
            |row| row.get(0),
        )?;
        if !(followed || wanted.contains(&obj.id())) {
            continue;
        }
        if let Payload::Announce(a) = &payload {
            wanted.insert(a.object);
        }
        if insert_object(tx, &obj, &payload, received, None)? {
            // As when it first came, so its media can be fetched again.
            if let Payload::Note(note) = &payload {
                declare_sizes(tx, note, Some(&obj.id()))?;
            }
            shown += 1;
        }
    }
    Ok(shown)
}

/// Takes what the list hides out of view, and lets its files go.
fn hide_held(tx: &Connection, me: &str, applied: &mut Applied) -> Result<()> {
    // Posts before reshares, so a reshare of a post just hidden goes too.
    let rows: Vec<(String, String, i64)> = {
        let mut stmt = tx.prepare(
            "SELECT id, signed, received_ms FROM objects WHERE actor != ?1
             ORDER BY kind = 'Announce'",
        )?;
        let rows = stmt.query_map(params![me], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut gone = Vec::new();
    let mut blobs = Vec::new();
    for (id, json, received) in rows {
        let Ok(obj) = serde_json::from_str::<SignedObject>(&json) else {
            continue;
        };
        let Ok(payload) = obj.verify() else {
            continue;
        };
        if !hides_object(tx, &obj.id(), &payload)? {
            continue;
        }
        keep_aside(tx, &obj, &payload, received)?;
        if let Payload::Note(note) = &payload {
            blobs.extend(note.blobs());
        }
        gone.push(id);
    }
    applied.hidden = gone.len();
    drop_objects(tx, &gone, blobs, &mut applied.files)?;

    // Listed files still here, named by nothing hidden: someone's post the
    // owner keeps, or none. The owner's own posts keep theirs, and so do
    // those of people the owner excepted.
    let held: Vec<String> = {
        let mut stmt = tx.prepare("SELECT hash FROM blobs")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for hash_hex in held {
        let Ok(hash) = hash_hex.parse::<BlobHash>() else {
            continue;
        };
        if !hides_file(tx, &hash)? || applied.files.contains(&hash) {
            continue;
        }
        if !named_by_spared(tx, &hash_hex, me)? {
            forget_blob(tx, &hash_hex)?;
            applied.files.push(hash);
        }
    }
    Ok(())
}
