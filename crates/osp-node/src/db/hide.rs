//! The project's hide list as this node applies it (ADR 0042 §6). A listed
//! post, a reshare of one, or a post naming a listed file is taken out of
//! every table that shows it and kept aside; a listed file isn't fetched or
//! served, and its copies go. The owner's own posts are never hidden: an
//! author keeps what they posted.
//!
//! When an entry is retracted, the posts it hid come back: those of people
//! the owner follows, and posts they reshared. Others, found through a
//! directory or opened from a code, come back as they're found again.

use anyhow::{Result, bail};
use iroh::PublicKey;
use osp_proto::{
    BlobHash, ObjectId, Payload, SignedObject,
    lists::{HideKind, HideList, file_entry, post_entry},
};
use rusqlite::{Connection, OptionalExtension, params};

use super::{Db, drop_objects, forget_blob, insert_object};

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

/// Whether the list hides `payload`, an object with ID `id`: the post
/// itself, a file it names, or for a reshare the post it points at, listed
/// or hidden here already. Never one of the owner's own.
pub(super) fn hides_object(conn: &Connection, id: &ObjectId, payload: &Payload) -> Result<bool> {
    let any: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM hide_list)", [], |row| {
        row.get(0)
    })?;
    if !any || owner(conn)?.is_some_and(|me| me == payload.actor().to_string()) {
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
        Payload::Announce(a) => Ok(listed(conn, HideKind::Post, &post_entry(&a.object))?
            || conn
                .query_row(
                    "SELECT 1 FROM hidden_objects WHERE id = ?1",
                    params![a.object.to_string()],
                    |_| Ok(()),
                )
                .optional()?
                .is_some()),
        _ => Ok(false),
    }
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

/// Whether the list holds `hash`: it isn't fetched or served.
pub(super) fn hides_file(conn: &Connection, hash: &BlobHash) -> Result<bool> {
    listed(conn, HideKind::File, &file_entry(hash))
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

    /// Whether the list holds `hash`.
    pub fn hides_file(&self, hash: &BlobHash) -> Result<bool> {
        hides_file(&self.conn(), hash)
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
    // owner keeps, or none. The owner's own posts keep theirs.
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
        let own: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM refs JOIN objects ON objects.id = refs.id
                            WHERE refs.ref = ?1 AND objects.actor = ?2)",
            params![hash_hex, me],
            |row| row.get(0),
        )?;
        if !own {
            forget_blob(tx, &hash_hex)?;
            applied.files.push(hash);
        }
    }
    Ok(())
}
