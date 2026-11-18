//! Blocks (ADR 0042 §5, ADR 0019's `osp block`). Alice blocks Kim, one of
//! Kim's posts, or a file: her node drops what it holds of it, and won't
//! fetch, store, serve or show it again until she unblocks it. Nothing is
//! sent to anyone, so Kim never learns of it.
//!
//! The owner's own posts and reshares are never dropped by a block: those
//! go only by deleting them, which tells her followers.

use std::collections::HashSet;

use anyhow::{Context, Result};
use iroh::PublicKey;
use osp_proto::{
    BlobHash, ObjectId, Payload, SignedObject,
    rpc::{BlockInfo, BlockTarget},
};
use rusqlite::{Connection, OptionalExtension, params};

use super::{Db, drop_objects, forget_blob, now_ms, opened, remove_from_search, upsert_profile};

/// What a block dropped here.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Dropped {
    /// Posts and reshares taken away.
    pub posts: usize,
    /// Files no post kept here names any more: the caller lets them go
    /// from the store.
    pub files: Vec<BlobHash>,
}

fn key(target: &BlockTarget) -> (&'static str, String) {
    match target {
        BlockTarget::Account { actor } => ("account", actor.to_string()),
        BlockTarget::Post { id } => ("post", id.to_string()),
        BlockTarget::File { hash } => ("file", hash.to_string()),
    }
}

fn is_blocked(conn: &Connection, kind: &str, target: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM blocks WHERE kind = ?1 AND target = ?2",
            params![kind, target],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Whether `payload`, an object with ID `id`, is blocked here: the post
/// itself, its author, a file it names, or for a reshare the post it points
/// at or that post's author.
pub(super) fn blocks_object(conn: &Connection, id: &ObjectId, payload: &Payload) -> Result<bool> {
    let any: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM blocks)", [], |row| row.get(0))?;
    if !any {
        return Ok(false);
    }
    if is_blocked(conn, "post", &id.to_string())?
        || is_blocked(conn, "account", &payload.actor().to_string())?
    {
        return Ok(true);
    }
    match payload {
        Payload::Note(note) => {
            for blob in note.blobs() {
                if is_blocked(conn, "file", &blob.to_string())? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Payload::Announce(a) => Ok(is_blocked(conn, "post", &a.object.to_string())?
            || is_blocked(conn, "account", &a.object_actor.to_string())?),
        _ => Ok(false),
    }
}

/// Whether `actor` is blocked here: their profile isn't kept either.
pub(super) fn blocks_account(conn: &Connection, actor: &str) -> Result<bool> {
    is_blocked(conn, "account", actor)
}

impl Db {
    /// Blocks `target` and drops what's held of it, leaving `me`'s own
    /// posts and reshares. Blocking again drops nothing more.
    pub fn block(&self, target: &BlockTarget, me: &PublicKey) -> Result<Dropped> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let (kind, hex) = key(target);
        // A person's profile is kept with the block, out of sight, for an
        // unblock to put back.
        let (name, profile): (Option<String>, Option<String>) = match target {
            BlockTarget::Account { actor } => tx
                .query_row(
                    "SELECT name, signed FROM profiles WHERE actor = ?1",
                    params![actor.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .unwrap_or_default(),
            _ => (None, None),
        };
        tx.execute(
            "INSERT INTO blocks (kind, target, name, at_ms, profile) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (kind, target) DO NOTHING",
            params![kind, hex, name, now_ms(), profile],
        )?;

        // What goes: the posts the block names, then reshares of them.
        let mine = me.to_string();
        let posts: Vec<(String, String)> = match target {
            BlockTarget::Account { .. } => {
                select_pairs(&tx, "SELECT id, signed FROM objects WHERE actor = ?1", &hex)?
            }
            BlockTarget::Post { .. } => {
                select_pairs(&tx, "SELECT id, signed FROM objects WHERE id = ?1", &hex)?
            }
            BlockTarget::File { .. } => select_pairs(
                &tx,
                "SELECT objects.id, objects.signed FROM refs
                 JOIN objects ON objects.id = refs.id
                 WHERE refs.ref = ?1 AND objects.kind = 'Note'",
                &hex,
            )?,
        };
        let mut gone = Vec::new();
        let mut blobs = Vec::new();
        let mut seen = HashSet::new();
        for (id, json) in &posts {
            let obj: SignedObject = serde_json::from_str(json).context("corrupt stored object")?;
            let payload = obj.verify().ok();
            if payload
                .as_ref()
                .is_some_and(|p| p.actor().to_string() == mine)
            {
                continue;
            }
            if let Some(Payload::Note(note)) = &payload {
                blobs.extend(note.blobs());
            }
            if seen.insert(id.clone()) {
                gone.push(id.clone());
            }
            for (reshare, actor) in select_pairs(
                &tx,
                "SELECT refs.id, objects.actor FROM refs JOIN objects ON objects.id = refs.id
                 WHERE refs.ref = ?1 AND objects.kind = 'Announce'",
                id,
            )? {
                if actor != mine && seen.insert(reshare.clone()) {
                    gone.push(reshare);
                }
            }
        }
        // A person's posts come back by following them again; a post's or a
        // file's are asked for again if the block is undone.
        if !matches!(target, BlockTarget::Account { .. }) {
            for id in &gone {
                tx.execute(
                    "INSERT OR IGNORE INTO block_dropped (kind, target, id, actor, received_ms)
                     SELECT ?1, ?2, id, actor, received_ms FROM objects WHERE id = ?3",
                    params![kind, hex, id],
                )?;
            }
        }
        let mut dropped = Dropped {
            posts: gone.len(),
            files: Vec::new(),
        };
        drop_objects(&tx, &gone, blobs, &mut dropped.files)?;

        match target {
            BlockTarget::Account { actor } => {
                let actor = actor.to_string();
                tx.execute("DELETE FROM follows WHERE actor = ?1", params![actor])?;
                tx.execute("DELETE FROM refetch WHERE actor = ?1", params![actor])?;
                tx.execute("DELETE FROM profiles WHERE actor = ?1", params![actor])?;
                remove_from_search(&tx, &actor)?;
                // Their replies to the owner's posts are hidden, and new
                // ones declined, as for a replier blocked (ADR 0028).
                tx.execute(
                    "INSERT INTO repliers (actor, choice, at_ms) VALUES (?1, 'blocked', ?2)
                     ON CONFLICT (actor) DO UPDATE SET choice = 'blocked', at_ms = excluded.at_ms",
                    params![actor, now_ms()],
                )?;
                tx.execute(
                    "UPDATE received_replies SET state = 'hidden'
                     WHERE actor = ?1 AND state = 'published'",
                    params![actor],
                )?;
                tx.execute(
                    "UPDATE received_replies SET state = 'declined'
                     WHERE actor = ?1 AND state = 'request'",
                    params![actor],
                )?;
                opened::settle_opened_person(&tx, &actor)?;
            }
            BlockTarget::File { hash } => {
                // Gone even if one of the owner's own posts names it.
                if !dropped.files.contains(hash) {
                    forget_blob(&tx, &hex)?;
                    dropped.files.push(*hash);
                }
            }
            BlockTarget::Post { .. } => {}
        }
        tx.commit()?;
        Ok(dropped)
    }

    /// Undoes a block. Returns false if there was none. A person unblocked
    /// may reply again, and their profile as it was is back, so they can be
    /// followed again; what was dropped comes back only as it's fetched.
    pub fn unblock(&self, target: &BlockTarget) -> Result<bool> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let (kind, hex) = key(target);
        let profile: Option<Option<String>> = tx
            .query_row(
                "SELECT profile FROM blocks WHERE kind = ?1 AND target = ?2",
                params![kind, hex],
                |row| row.get(0),
            )
            .optional()?;
        let n = tx.execute(
            "DELETE FROM blocks WHERE kind = ?1 AND target = ?2",
            params![kind, hex],
        )?;
        // What it dropped is asked for again at the next sync with each
        // author's node, as after a revocation (ADR 0010).
        tx.execute(
            "INSERT OR IGNORE INTO refetch (id, actor, received_ms)
             SELECT id, actor, received_ms FROM block_dropped WHERE kind = ?1 AND target = ?2",
            params![kind, hex],
        )?;
        tx.execute(
            "DELETE FROM block_dropped WHERE kind = ?1 AND target = ?2",
            params![kind, hex],
        )?;
        if n > 0 && kind == "account" {
            tx.execute(
                "DELETE FROM repliers WHERE actor = ?1 AND choice = 'blocked'",
                params![hex],
            )?;
            if let Some(json) = profile.flatten() {
                let obj: SignedObject =
                    serde_json::from_str(&json).context("corrupt stored profile")?;
                if let Ok(Payload::Person(person)) = obj.verify() {
                    upsert_profile(&tx, &person, &obj)?;
                }
            }
        }
        tx.commit()?;
        Ok(n > 0)
    }

    /// Everything blocked here, newest first.
    pub fn blocks(&self) -> Result<Vec<BlockInfo>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT kind, target, name, at_ms FROM blocks ORDER BY at_ms DESC, rowid DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        rows.map(|r| {
            let (kind, target, name, at) = r?;
            let target = match kind.as_str() {
                "account" => BlockTarget::Account {
                    actor: target.parse()?,
                },
                "post" => BlockTarget::Post {
                    id: target.parse()?,
                },
                _ => BlockTarget::File {
                    hash: target.parse()?,
                },
            };
            Ok(BlockInfo {
                target,
                name,
                since: osp_proto::rfc3339_from_ms(at.max(0) as u64),
            })
        })
        .collect()
    }

    /// Whether `target` is blocked here.
    pub fn is_blocked(&self, target: &BlockTarget) -> Result<bool> {
        let (kind, hex) = key(target);
        is_blocked(&self.conn(), kind, &hex)
    }
}

fn select_pairs(tx: &Connection, sql: &str, arg: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = tx.prepare(sql)?;
    let rows = stmt.query_map(params![arg], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
