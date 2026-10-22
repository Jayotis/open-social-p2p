//! Node metadata in SQLite: objects, outbox, follows, blob classes and known hosts.
//!
//! Blob contents live in the iroh-blobs store (see `store.rs`); this database
//! only records which blobs we have and why.

use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use iroh::{EndpointAddr, EndpointId, PublicKey};
use osp_proto::{
    BlobHash, Note, ObjectId, Payload, SignedObject,
    rpc::{FollowInfo, StorageInfo},
};
use rusqlite::{Connection, OptionalExtension, params};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS objects (
    id           TEXT PRIMARY KEY,
    actor        TEXT NOT NULL,
    kind         TEXT NOT NULL,
    published_ms INTEGER NOT NULL,
    received_ms  INTEGER NOT NULL,
    signed       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS objects_actor ON objects(actor, kind);

-- Our own identity's published objects, in order.
CREATE TABLE IF NOT EXISTS outbox (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id  TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS follows (
    actor      TEXT PRIMARY KEY,
    node_addr  TEXT NOT NULL,
    cursor     INTEGER NOT NULL DEFAULT 0,
    last_sync  INTEGER,
    last_error TEXT
);

CREATE TABLE IF NOT EXISTS followers (
    node      TEXT PRIMARY KEY,
    node_addr TEXT NOT NULL,
    since     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS profiles (
    actor  TEXT PRIMARY KEY,
    name   TEXT NOT NULL,
    signed TEXT NOT NULL
);

-- Why we keep each blob: own > replica > cache (ADR 0003).
CREATE TABLE IF NOT EXISTS blobs (
    hash        TEXT PRIMARY KEY,
    class       TEXT NOT NULL,
    size        INTEGER NOT NULL,
    added_ms    INTEGER NOT NULL,
    accessed_ms INTEGER NOT NULL
);

-- Nodes believed to serve a blob, learned from authors, hosting reshares and WhoHosts.
CREATE TABLE IF NOT EXISTS hosts (
    blob      TEXT NOT NULL,
    node      TEXT NOT NULL,
    node_addr TEXT NOT NULL,
    seen_ms   INTEGER NOT NULL,
    PRIMARY KEY (blob, node)
);

-- Paired phones (milestone M2).
CREATE TABLE IF NOT EXISTS devices (
    node      TEXT PRIMARY KEY,
    name      TEXT,
    paired_ms INTEGER NOT NULL
);
";

/// Changes to the schema above, applied in order to every database, old or
/// new. SQLite's `user_version` records how many have run. Only ever append.
const MIGRATIONS: &[&str] = &[
    // 1: when each follower last answered or subscribed, so long-silent ones can be dropped.
    "ALTER TABLE followers ADD COLUMN seen_ms INTEGER",
    // 2: the size stored posts declare for each blob, so a download the
    // phone asks for can be stopped once it passes that. Filled in from
    // the posts already stored by `migrate`.
    "CREATE TABLE declared_sizes (blob TEXT PRIMARY KEY, size INTEGER NOT NULL)",
    // 3: followers still to be told that this identity moved to this node (ADR 0009).
    "ALTER TABLE followers ADD COLUMN tell_moved INTEGER NOT NULL DEFAULT 0",
];

/// Hosts remembered per blob. More than enough to find a copy; a peer
/// can't grow the table without bound by naming hosts.
const MAX_HOSTS_PER_BLOB: i64 = 20;

/// A blob's `accessed_ms` is updated at most this often, so a phone
/// seeking through a video doesn't write to the database on every request.
const TOUCH_EVERY_MS: i64 = 60_000;

/// Why a blob is kept, which decides who it is served to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlobClass {
    /// Prefetched for my own devices. Served only to them; evicted first.
    Cache,
    /// Media I chose to host by resharing. Served to anyone.
    Replica,
    /// Media of my own posts. Served to anyone; never evicted.
    Own,
}

impl BlobClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlobClass::Cache => "cache",
            BlobClass::Replica => "replica",
            BlobClass::Own => "own",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "cache" => BlobClass::Cache,
            "replica" => BlobClass::Replica,
            "own" => BlobClass::Own,
            other => anyhow::bail!("unknown blob class {other}"),
        })
    }

    /// Whether anyone (not just my own devices) may download it.
    pub fn is_public(&self) -> bool {
        matches!(self, BlobClass::Own | BlobClass::Replica)
    }
}

/// A followed identity and where we sync it from.
#[derive(Debug, Clone)]
pub struct Follow {
    pub actor: PublicKey,
    pub addr: EndpointAddr,
    pub cursor: u64,
    /// Whether a sync has ever completed a page from this follow.
    pub synced: bool,
}

/// SQLite connection shared by the node's tasks.
///
/// Queries are short, so a plain mutex is fine; a busy node could move to a
/// connection pool later.
pub struct Db {
    conn: Mutex<Connection>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn).with_context(|| format!("updating {}", path.display()))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves SQLite itself consistent, so keep going.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- objects ----

    /// Stores a verified object. Returns false if we already had it.
    pub fn insert_object(&self, obj: &SignedObject, payload: &Payload) -> Result<bool> {
        let received = now_ms();
        // Clamp author-claimed time so a future-dated post can't pin itself to the top.
        let published = osp_proto::parse_rfc3339_ms(payload.published())
            .unwrap_or(received)
            .min(received);
        let conn = self.conn();
        let n = conn.execute(
            "INSERT OR IGNORE INTO objects (id, actor, kind, published_ms, received_ms, signed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                obj.id().to_string(),
                payload.actor().to_string(),
                payload.kind(),
                published,
                received,
                serde_json::to_string(obj)?
            ],
        )?;
        if n > 0
            && let Payload::Note(note) = payload
        {
            declare_sizes(&conn, note)?;
        }
        Ok(n > 0)
    }

    /// How many objects by `actor` arrived at or after `since_ms`.
    pub fn objects_received_since(&self, actor: &PublicKey, since_ms: i64) -> Result<usize> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM objects WHERE actor = ?1 AND received_ms >= ?2",
            params![actor.to_string(), since_ms],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    pub fn get_object(&self, id: &ObjectId) -> Result<Option<SignedObject>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT signed FROM objects WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|j| serde_json::from_str(&j).context("corrupt stored object"))
            .transpose()
    }

    /// Posts and reshares by the given identities, newest first.
    pub fn feed(&self, actors: &[PublicKey], limit: u32) -> Result<Vec<SignedObject>> {
        if actors.is_empty() {
            return Ok(vec![]);
        }
        let placeholders = vec!["?"; actors.len()].join(",");
        let sql = format!(
            "SELECT signed FROM objects
             WHERE kind IN ('Note', 'Announce') AND actor IN ({placeholders})
             ORDER BY published_ms DESC LIMIT {limit}"
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let actors: Vec<String> = actors.iter().map(|a| a.to_string()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(actors), |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    // ---- our outbox ----

    pub fn append_outbox(&self, id: &ObjectId) -> Result<u64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO outbox (id) VALUES (?1)",
            params![id.to_string()],
        )?;
        Ok(conn.last_insert_rowid() as u64)
    }

    pub fn outbox_since(&self, since: u64, limit: u32) -> Result<Vec<(u64, SignedObject)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT outbox.seq, objects.signed FROM outbox
             JOIN objects ON objects.id = outbox.id
             WHERE outbox.seq > ?1 ORDER BY outbox.seq LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![since as i64, limit], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|r| {
            let (seq, json) = r?;
            Ok((seq as u64, serde_json::from_str(&json)?))
        })
        .collect()
    }

    pub fn outbox_latest(&self) -> Result<u64> {
        let seq: Option<i64> = self
            .conn()
            .query_row("SELECT MAX(seq) FROM outbox", [], |row| row.get(0))?;
        Ok(seq.unwrap_or(0) as u64)
    }

    // ---- follows ----

    /// Follows `actor` at `addr`, or updates the address of a follow. A
    /// follow that moves to a different node starts over like a new one,
    /// since that node numbers its outbox from 1 (ADR 0009).
    pub fn upsert_follow(&self, actor: &PublicKey, addr: &EndpointAddr) -> Result<()> {
        let conn = self.conn();
        let current: Option<String> = conn
            .query_row(
                "SELECT node_addr FROM follows WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let same_node = match current {
            Some(json) => serde_json::from_str::<EndpointAddr>(&json)?.id == addr.id,
            None => false,
        };
        let sql = if same_node {
            "UPDATE follows SET node_addr = ?2 WHERE actor = ?1"
        } else {
            // Replacing the row resets its position, sync time and error.
            "INSERT OR REPLACE INTO follows (actor, node_addr) VALUES (?1, ?2)"
        };
        conn.execute(
            sql,
            params![actor.to_string(), serde_json::to_string(addr)?],
        )?;
        Ok(())
    }

    pub fn remove_follow(&self, actor: &PublicKey) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM follows WHERE actor = ?1",
            params![actor.to_string()],
        )?;
        Ok(n > 0)
    }

    pub fn follows(&self) -> Result<Vec<Follow>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT actor, node_addr, cursor, last_sync IS NOT NULL FROM follows")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        rows.map(|r| {
            let (actor, addr, cursor, synced) = r?;
            Ok(Follow {
                actor: actor.parse()?,
                addr: serde_json::from_str(&addr)?,
                cursor: cursor as u64,
                synced,
            })
        })
        .collect()
    }

    pub fn follow_infos(&self) -> Result<Vec<FollowInfo>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT follows.actor, profiles.name, cursor, last_sync, last_error
             FROM follows LEFT JOIN profiles ON profiles.actor = follows.actor",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        rows.map(|r| {
            let (actor, name, cursor, last_sync, last_error) = r?;
            Ok(FollowInfo {
                actor: actor.parse()?,
                name,
                cursor: cursor as u64,
                last_sync: last_sync.map(format_ms),
                last_error,
            })
        })
        .collect()
    }

    pub fn set_follow_synced(&self, actor: &PublicKey, cursor: u64) -> Result<()> {
        // Never move the cursor backwards if two syncs overlap.
        self.conn().execute(
            "UPDATE follows SET cursor = MAX(cursor, ?2), last_sync = ?3, last_error = NULL
             WHERE actor = ?1",
            params![actor.to_string(), cursor as i64, now_ms()],
        )?;
        Ok(())
    }

    pub fn set_follow_error(&self, actor: &PublicKey, error: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE follows SET last_error = ?2 WHERE actor = ?1",
            params![actor.to_string(), error],
        )?;
        Ok(())
    }

    // ---- followers ----

    /// Records a follower, or updates the address of one we have. Either
    /// way it counts as heard from now. A follower subscribing here knows
    /// where we are, so it needn't be told we moved.
    pub fn add_follower(&self, addr: &EndpointAddr) -> Result<()> {
        self.conn().execute(
            "INSERT INTO followers (node, node_addr, since, seen_ms) VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(node) DO UPDATE SET node_addr = excluded.node_addr,
                 seen_ms = excluded.seen_ms, tell_moved = 0",
            params![addr.id.to_string(), serde_json::to_string(addr)?, now_ms()],
        )?;
        Ok(())
    }

    /// Marks every follower as still to be told that we moved (ADR 0009).
    pub fn mark_followers_to_tell_moved(&self) -> Result<usize> {
        Ok(self
            .conn()
            .execute("UPDATE followers SET tell_moved = 1", [])?)
    }

    pub fn followers_to_tell_moved(&self) -> Result<Vec<EndpointAddr>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT node_addr FROM followers WHERE tell_moved = 1")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    /// Notes that a follower heard we moved.
    pub fn told_moved(&self, node: &EndpointId) -> Result<()> {
        self.conn().execute(
            "UPDATE followers SET tell_moved = 0, seen_ms = ?2 WHERE node = ?1",
            params![node.to_string(), now_ms()],
        )?;
        Ok(())
    }

    pub fn is_follower(&self, node: &EndpointId) -> Result<bool> {
        let found: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM followers WHERE node = ?1",
                params![node.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Notes that a follower's node answered.
    pub fn follower_seen(&self, node: &EndpointId) -> Result<()> {
        self.conn().execute(
            "UPDATE followers SET seen_ms = ?2 WHERE node = ?1",
            params![node.to_string(), now_ms()],
        )?;
        Ok(())
    }

    /// How many followers first subscribed at or after `since_ms`.
    pub fn new_followers_since(&self, since_ms: i64) -> Result<usize> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM followers WHERE since >= ?1",
            params![since_ms],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    /// Forgets followers not heard from since `before_ms`. Returns how many.
    pub fn drop_silent_followers(&self, before_ms: i64) -> Result<usize> {
        let n = self.conn().execute(
            "DELETE FROM followers WHERE COALESCE(seen_ms, since) < ?1",
            params![before_ms],
        )?;
        Ok(n)
    }

    pub fn followers(&self) -> Result<Vec<EndpointAddr>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT node_addr FROM followers")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    // ---- profiles ----

    pub fn upsert_profile(&self, actor: &PublicKey, name: &str, obj: &SignedObject) -> Result<()> {
        self.conn().execute(
            "INSERT INTO profiles (actor, name, signed) VALUES (?1, ?2, ?3)
             ON CONFLICT(actor) DO UPDATE SET name = excluded.name, signed = excluded.signed",
            params![actor.to_string(), name, serde_json::to_string(obj)?],
        )?;
        Ok(())
    }

    pub fn profile(&self, actor: &PublicKey) -> Result<Option<SignedObject>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT signed FROM profiles WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|j| serde_json::from_str(&j).context("corrupt stored profile"))
            .transpose()
    }

    pub fn profile_name(&self, actor: &PublicKey) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT name FROM profiles WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?)
    }

    // ---- blobs ----

    pub fn blob_class(&self, hash: &BlobHash) -> Result<Option<BlobClass>> {
        let class: Option<String> = self
            .conn()
            .query_row(
                "SELECT class FROM blobs WHERE hash = ?1",
                params![hash.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        class.map(|c| BlobClass::parse(&c)).transpose()
    }

    /// Records that we hold a blob. A blob's class only ever moves up
    /// (cache -> replica -> own), never down.
    pub fn put_blob(&self, hash: &BlobHash, class: BlobClass, size: u64) -> Result<()> {
        let conn = self.conn();
        let current: Option<String> = conn
            .query_row(
                "SELECT class FROM blobs WHERE hash = ?1",
                params![hash.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let class = match current.map(|c| BlobClass::parse(&c)).transpose()? {
            Some(existing) => existing.max(class),
            None => class,
        };
        let now = now_ms();
        conn.execute(
            "INSERT INTO blobs (hash, class, size, added_ms, accessed_ms) VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(hash) DO UPDATE SET class = excluded.class, accessed_ms = excluded.accessed_ms",
            params![hash.to_string(), class.as_str(), size as i64, now],
        )?;
        Ok(())
    }

    /// Forgets a blob, whatever its class.
    pub fn remove_blob(&self, hash: &BlobHash) -> Result<()> {
        self.conn().execute(
            "DELETE FROM blobs WHERE hash = ?1",
            params![hash.to_string()],
        )?;
        Ok(())
    }

    /// Every blob we hold, with its class.
    pub fn blobs(&self) -> Result<Vec<(BlobHash, BlobClass)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT hash, class FROM blobs")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|r| {
            let (hash, class) = r?;
            Ok((hash.parse()?, BlobClass::parse(&class)?))
        })
        .collect()
    }

    /// Total size of the blobs kept under `class`.
    pub fn class_bytes(&self, class: BlobClass) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT COALESCE(SUM(size), 0) FROM blobs WHERE class = ?1",
            params![class.as_str()],
            |row| row.get(0),
        )?;
        Ok(n as u64)
    }

    /// Notes that one of my devices just used a blob, so the cache keeps
    /// it longer.
    pub fn touch_blob(&self, hash: &BlobHash) -> Result<()> {
        let now = now_ms();
        self.conn().execute(
            "UPDATE blobs SET accessed_ms = ?2 WHERE hash = ?1 AND accessed_ms < ?3",
            params![hash.to_string(), now, now - TOUCH_EVERY_MS],
        )?;
        Ok(())
    }

    /// Forgets the cache blobs used longest ago until at least `bytes`
    /// are freed, or the cache is empty. Returns them, so their data can
    /// be released. Never touches other classes.
    pub fn evict_cache(&self, bytes: u64) -> Result<Vec<BlobHash>> {
        let conn = self.conn();
        let mut evicted = Vec::new();
        let mut freed = 0;
        {
            let mut stmt = conn.prepare(
                "SELECT hash, size FROM blobs WHERE class = 'cache' ORDER BY accessed_ms, added_ms",
            )?;
            let mut rows = stmt.query([])?;
            while freed < bytes
                && let Some(row) = rows.next()?
            {
                evicted.push(row.get::<_, String>(0)?.parse::<BlobHash>()?);
                freed += row.get::<_, i64>(1)? as u64;
            }
        }
        for hash in &evicted {
            conn.execute(
                "DELETE FROM blobs WHERE hash = ?1",
                params![hash.to_string()],
            )?;
        }
        Ok(evicted)
    }

    /// The largest size any stored post declares for a blob. The blob's
    /// real size is fixed by its hash, so a smaller claim elsewhere is
    /// simply wrong, and it mustn't stop the honest post's media loading.
    pub fn declared_size(&self, blob: &BlobHash) -> Result<Option<u64>> {
        let size: Option<i64> = self
            .conn()
            .query_row(
                "SELECT size FROM declared_sizes WHERE blob = ?1",
                params![blob.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(size.map(|s| s as u64))
    }

    pub fn storage_summary(&self) -> Result<Vec<StorageInfo>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT class, COUNT(*), COALESCE(SUM(size), 0) FROM blobs GROUP BY class ORDER BY class",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StorageInfo {
                class: row.get(0)?,
                blobs: row.get::<_, i64>(1)? as u64,
                bytes: row.get::<_, i64>(2)? as u64,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- hosts ----

    /// Records a host of a blob, keeping only the most recently seen few.
    pub fn add_host(&self, blob: &BlobHash, addr: &EndpointAddr) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO hosts (blob, node, node_addr, seen_ms) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(blob, node) DO UPDATE SET node_addr = excluded.node_addr, seen_ms = excluded.seen_ms",
            params![
                blob.to_string(),
                addr.id.to_string(),
                serde_json::to_string(addr)?,
                now_ms()
            ],
        )?;
        conn.execute(
            "DELETE FROM hosts WHERE blob = ?1 AND node NOT IN
             (SELECT node FROM hosts WHERE blob = ?1 ORDER BY seen_ms DESC LIMIT ?2)",
            params![blob.to_string(), MAX_HOSTS_PER_BLOB],
        )?;
        Ok(())
    }

    /// Known hosts of a blob, most recently seen first.
    pub fn hosts(&self, blob: &BlobHash) -> Result<Vec<EndpointAddr>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT node_addr FROM hosts WHERE blob = ?1 ORDER BY seen_ms DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![blob.to_string(), MAX_HOSTS_PER_BLOB], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    // ---- devices ----

    /// Records a paired device, or renames one paired before.
    pub fn add_device(&self, node: &EndpointId, name: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO devices (node, name, paired_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(node) DO UPDATE SET name = excluded.name",
            params![node.to_string(), name, now_ms()],
        )?;
        Ok(())
    }

    pub fn remove_device(&self, node: &EndpointId) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM devices WHERE node = ?1",
            params![node.to_string()],
        )?;
        Ok(n > 0)
    }

    /// Paired devices with their names and pairing times, oldest first.
    pub fn devices(&self) -> Result<Vec<(EndpointId, Option<String>, String)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT node, name, paired_ms FROM devices ORDER BY paired_ms")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|r| {
            let (node, name, paired) = r?;
            Ok((node.parse()?, name, format_ms(paired)))
        })
        .collect()
    }

    pub fn is_device(&self, node: &EndpointId) -> Result<bool> {
        let found: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM devices WHERE node = ?1",
                params![node.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn follower_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM followers", [], |row| row.get(0))?;
        Ok(n as usize)
    }
}

/// Runs the migrations this database hasn't had yet, each in one transaction.
fn migrate(conn: &Connection) -> Result<()> {
    let done: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(done as usize) {
        let version = i as i64 + 1;
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        if version == 2 {
            backfill_declared_sizes(&tx)?;
        }
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

/// Records the sizes a post declares for its media and thumbnails.
fn declare_sizes(conn: &Connection, note: &Note) -> Result<()> {
    for (blob, size) in note.sized_blobs() {
        conn.execute(
            "INSERT INTO declared_sizes (blob, size) VALUES (?1, ?2)
             ON CONFLICT(blob) DO UPDATE SET size = MAX(size, excluded.size)",
            params![blob.to_string(), size as i64],
        )?;
    }
    Ok(())
}

/// Fills in `declared_sizes` from the posts stored before it existed.
fn backfill_declared_sizes(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT signed FROM objects WHERE kind = 'Note'")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for json in rows {
        let obj: SignedObject = serde_json::from_str(&json?).context("corrupt stored object")?;
        // Checked when it arrived; one that fails today's checks gets no sizes.
        if let Ok(Payload::Note(note)) = obj.verify() {
            declare_sizes(conn, &note)?;
        }
    }
    Ok(())
}

fn format_ms(ms: i64) -> String {
    let t = UNIX_EPOCH + std::time::Duration::from_millis(ms.max(0) as u64);
    humantime::format_rfc3339_seconds(t).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_class_only_moves_up() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let h = BlobHash::from_bytes([7; 32]);
        db.put_blob(&h, BlobClass::Cache, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Cache));
        db.put_blob(&h, BlobClass::Replica, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Replica));
        db.put_blob(&h, BlobClass::Cache, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Replica));
    }

    fn some_addr() -> EndpointAddr {
        EndpointAddr::new(iroh::SecretKey::generate().public())
    }

    #[test]
    fn a_database_from_before_migrations_is_updated_and_keeps_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let old = some_addr();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO followers (node, node_addr, since) VALUES (?1, ?2, 1)",
                params![old.id.to_string(), serde_json::to_string(&old).unwrap()],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert!(db.is_follower(&old.id).unwrap());
        db.follower_seen(&old.id).unwrap();
        drop(db);
        // Opening again doesn't run the migrations twice.
        let db = Db::open(&path).unwrap();
        assert_eq!(db.follower_count().unwrap(), 1);
    }

    #[test]
    fn silent_followers_are_dropped_and_new_ones_counted() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (quiet, chatty) = (some_addr(), some_addr());
        db.add_follower(&quiet).unwrap();
        db.add_follower(&chatty).unwrap();
        let start = now_ms();
        assert_eq!(db.new_followers_since(start - 60_000).unwrap(), 2);
        // Only the quiet one was last heard from long ago.
        db.conn()
            .execute(
                "UPDATE followers SET since = 1, seen_ms = 1 WHERE node = ?1",
                params![quiet.id.to_string()],
            )
            .unwrap();
        assert_eq!(db.new_followers_since(start - 60_000).unwrap(), 1);
        assert_eq!(db.drop_silent_followers(start - 60_000).unwrap(), 1);
        assert!(!db.is_follower(&quiet.id).unwrap());
        assert!(db.is_follower(&chatty.id).unwrap());
    }

    #[test]
    fn followers_are_told_of_a_move_until_they_hear_or_subscribe() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (a, b, c) = (some_addr(), some_addr(), some_addr());
        for f in [&a, &b, &c] {
            db.add_follower(f).unwrap();
        }
        assert!(db.followers_to_tell_moved().unwrap().is_empty());
        assert_eq!(db.mark_followers_to_tell_moved().unwrap(), 3);
        db.told_moved(&a.id).unwrap();
        // Subscribing to this node means b already knows where we are.
        db.add_follower(&b).unwrap();
        let left: Vec<_> = db.followers_to_tell_moved().unwrap();
        assert_eq!(left.iter().map(|f| f.id).collect::<Vec<_>>(), [c.id]);
    }

    #[test]
    fn a_follow_that_moves_to_another_node_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let actor = iroh::SecretKey::generate().public();
        let node = some_addr();
        db.upsert_follow(&actor, &node).unwrap();
        db.set_follow_synced(&actor, 50).unwrap();
        db.set_follow_error(&actor, "unreachable").unwrap();

        // The same node at a new address keeps its place.
        let relay: iroh::RelayUrl = "https://relay.example.org".parse().unwrap();
        db.upsert_follow(&actor, &node.clone().with_relay_url(relay))
            .unwrap();
        let [follow] = &db.follows().unwrap()[..] else {
            panic!("one follow expected")
        };
        assert_eq!((follow.cursor, follow.synced), (50, true));

        // Another node starts again from the latest entries.
        let other = some_addr();
        db.upsert_follow(&actor, &other).unwrap();
        let [follow] = &db.follows().unwrap()[..] else {
            panic!("one follow expected")
        };
        assert_eq!(follow.addr.id, other.id);
        assert_eq!((follow.cursor, follow.synced), (0, false));
        assert_eq!(db.follow_infos().unwrap()[0].last_error, None);
    }

    fn note_with_media(
        key: &iroh::SecretKey,
        media: &[(BlobHash, u64)],
    ) -> (SignedObject, Payload) {
        let attachment = media
            .iter()
            .map(|&(blob, size)| osp_proto::Attachment {
                kind: osp_proto::MediaKind::Video,
                media_type: "video/mp4".into(),
                blob,
                size,
                width: 1,
                height: 1,
                duration_ms: None,
                name: None,
                thumbnail: None,
                profile: "v1-720p".into(),
            })
            .collect();
        let payload = Payload::Note(Note {
            actor: key.public(),
            content: "media".into(),
            published: osp_proto::now_rfc3339(),
            attachment,
            in_reply_to: None,
        });
        (SignedObject::sign(key, &payload).unwrap(), payload)
    }

    #[test]
    fn declared_sizes_come_from_new_posts_and_posts_stored_before() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let key = iroh::SecretKey::generate();
        let (old, new) = (BlobHash::from_bytes([1; 32]), BlobHash::from_bytes([2; 32]));
        {
            // A database from before migration 2, holding a post with media.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            let (obj, _) = note_with_media(&key, &[(old, 500)]);
            conn.execute(
                "INSERT INTO objects (id, actor, kind, published_ms, received_ms, signed)
                 VALUES (?1, ?2, 'Note', 1, 1, ?3)",
                params![
                    obj.id().to_string(),
                    key.public().to_string(),
                    serde_json::to_string(&obj).unwrap()
                ],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.declared_size(&old).unwrap(), Some(500));
        assert_eq!(db.declared_size(&new).unwrap(), None);

        let (obj, payload) = note_with_media(&key, &[(new, 300)]);
        db.insert_object(&obj, &payload).unwrap();
        assert_eq!(db.declared_size(&new).unwrap(), Some(300));
        // Another post can't shrink it, only raise it.
        let (obj, payload) = note_with_media(&key, &[(new, 10), (old, 900)]);
        db.insert_object(&obj, &payload).unwrap();
        assert_eq!(db.declared_size(&new).unwrap(), Some(300));
        assert_eq!(db.declared_size(&old).unwrap(), Some(900));
    }

    #[test]
    fn eviction_takes_cache_blobs_used_longest_ago_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let [own, replica, stale, recent] = [1u8, 2, 3, 4].map(|b| BlobHash::from_bytes([b; 32]));
        db.put_blob(&own, BlobClass::Own, 100).unwrap();
        db.put_blob(&replica, BlobClass::Replica, 100).unwrap();
        db.put_blob(&stale, BlobClass::Cache, 100).unwrap();
        db.put_blob(&recent, BlobClass::Cache, 100).unwrap();
        db.conn()
            .execute("UPDATE blobs SET accessed_ms = 1", [])
            .unwrap();
        db.touch_blob(&recent).unwrap();
        assert_eq!(db.class_bytes(BlobClass::Cache).unwrap(), 200);

        assert_eq!(db.evict_cache(0).unwrap(), vec![]);
        assert_eq!(db.evict_cache(1).unwrap(), vec![stale]);
        assert_eq!(db.blob_class(&stale).unwrap(), None);
        // Asking for more than the cache holds empties the cache, no more.
        assert_eq!(db.evict_cache(1 << 30).unwrap(), vec![recent]);
        assert_eq!(db.class_bytes(BlobClass::Cache).unwrap(), 0);
        assert_eq!(db.blob_class(&own).unwrap(), Some(BlobClass::Own));
        assert_eq!(db.blob_class(&replica).unwrap(), Some(BlobClass::Replica));
    }

    #[test]
    fn hosts_per_blob_are_capped() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let blob = BlobHash::from_bytes([3; 32]);
        for _ in 0..MAX_HOSTS_PER_BLOB + 5 {
            db.add_host(&blob, &some_addr()).unwrap();
        }
        // Inserts can share a millisecond; make the order unambiguous.
        db.conn()
            .execute("UPDATE hosts SET seen_ms = 1", [])
            .unwrap();
        let latest = some_addr();
        db.add_host(&blob, &latest).unwrap();
        let hosts = db.hosts(&blob).unwrap();
        assert_eq!(hosts.len(), MAX_HOSTS_PER_BLOB as usize);
        assert!(hosts.contains(&latest));
        let stored: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM hosts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, MAX_HOSTS_PER_BLOB);
    }
}
