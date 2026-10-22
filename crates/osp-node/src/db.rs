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
    BlobHash, ObjectId, Payload, SignedObject,
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
];

/// Hosts remembered per blob. More than enough to find a copy; a peer
/// can't grow the table without bound by naming hosts.
const MAX_HOSTS_PER_BLOB: i64 = 20;

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
        let n = self.conn().execute(
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
        Ok(n > 0)
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

    pub fn upsert_follow(&self, actor: &PublicKey, addr: &EndpointAddr) -> Result<()> {
        self.conn().execute(
            "INSERT INTO follows (actor, node_addr) VALUES (?1, ?2)
             ON CONFLICT(actor) DO UPDATE SET node_addr = excluded.node_addr",
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
        let mut stmt = conn.prepare("SELECT actor, node_addr, cursor FROM follows")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|r| {
            let (actor, addr, cursor) = r?;
            Ok(Follow {
                actor: actor.parse()?,
                addr: serde_json::from_str(&addr)?,
                cursor: cursor as u64,
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
    /// way it counts as heard from now.
    pub fn add_follower(&self, addr: &EndpointAddr) -> Result<()> {
        self.conn().execute(
            "INSERT INTO followers (node, node_addr, since, seen_ms) VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(node) DO UPDATE SET node_addr = excluded.node_addr, seen_ms = excluded.seen_ms",
            params![addr.id.to_string(), serde_json::to_string(addr)?, now_ms()],
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

/// Runs the migrations this database hasn't had yet.
fn migrate(conn: &Connection) -> Result<()> {
    let done: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(done as usize) {
        conn.execute_batch(&format!(
            "BEGIN; {sql}; PRAGMA user_version = {}; COMMIT;",
            i + 1
        ))?;
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
