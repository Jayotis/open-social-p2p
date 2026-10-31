//! Node metadata in SQLite: objects, outbox, follows, blob classes and known hosts.
//!
//! Blob contents live in the iroh-blobs store (see `store.rs`); this database
//! only records which blobs we have and why.

use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use iroh::{EndpointAddr, EndpointId, PublicKey};
use osp_proto::{
    BlobHash, EarlierKeys, Key, Note, ObjectId, Payload, SignedObject,
    rpc::{FollowInfo, PeerCounts, StorageInfo, Upload, UploadId},
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::peers;

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

-- Why we keep each blob: own > replica > prefetch > cache (ADR 0003).
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
    // 4: what we know of each identity's signing keys (ADR 0010): its newest
    // key certificate, and the date of the newest one that revoked earlier keys.
    "CREATE TABLE identity_keys (
        actor             TEXT PRIMARY KEY,
        cert              TEXT NOT NULL,
        cert_ms           INTEGER NOT NULL,
        revoked_before_ms INTEGER
    )",
    // 5: objects of followed identities dropped because a key that signed
    // them was revoked, to ask their node for again (ADR 0010), with when
    // each first arrived.
    "CREATE TABLE refetch (
        id          TEXT PRIMARY KEY,
        actor       TEXT NOT NULL,
        received_ms INTEGER NOT NULL
    )",
    // 6: what this node measured about each peer node (ADR 0011): whether it
    // ever contacted the peer itself, first and last contact, and the faded
    // counts as JSON (`PeerCounts`), so a measure can be added without a
    // migration. Counts were last faded at the last contact.
    "CREATE TABLE peers (
        node     TEXT PRIMARY KEY,
        ours     INTEGER NOT NULL,
        first_ms INTEGER NOT NULL,
        last_ms  INTEGER NOT NULL,
        counts   TEXT NOT NULL
    );
    CREATE INDEX peers_by_contact ON peers(ours, last_ms);",
    // 7: the follower swarm (doc 13): whether each follower serves what it
    // fetches ahead of time to anyone, and which followers told us they
    // hold a copy of one of our blobs, last when.
    "ALTER TABLE followers ADD COLUMN shares INTEGER NOT NULL DEFAULT 0;
    CREATE TABLE swarm (
        blob    TEXT NOT NULL,
        node    TEXT NOT NULL,
        seen_ms INTEGER NOT NULL,
        PRIMARY KEY (blob, node)
    );",
    // 8: where each object sits in the feed: when it was published, except
    // for a followed author's post held back while its media is on its way
    // (`insert_object_held`), which takes its place when it's shown.
    "ALTER TABLE objects ADD COLUMN feed_ms INTEGER NOT NULL DEFAULT 0;
    UPDATE objects SET feed_ms = published_ms;",
    // 9: who posted each blob, so a download can leave the author's own
    // nodes until last when it fetches from several at once (doc 13).
    // Filled in from the posts already stored by `migrate`.
    "ALTER TABLE declared_sizes ADD COLUMN author TEXT",
    // 10: uploads from devices that can be resumed after a broken
    // connection (ADR 0008): the post's text and files (JSON `Upload`s),
    // how far it got, and the post ID or the reason it failed.
    "CREATE TABLE uploads (
        device     TEXT NOT NULL,
        id         TEXT NOT NULL,
        content    TEXT NOT NULL,
        files      TEXT NOT NULL,
        state      TEXT NOT NULL,
        outcome    TEXT,
        updated_ms INTEGER NOT NULL,
        PRIMARY KEY (device, id)
    )",
];

/// Hosts remembered per blob. More than enough to find a copy; a peer
/// can't grow the table without bound by naming hosts.
const MAX_HOSTS_PER_BLOB: i64 = 20;

/// Followers remembered as holding a copy of one of our blobs. Plenty to
/// pick a few from.
const MAX_SWARM_PER_BLOB: i64 = 100;
/// Of those, the places kept for the followers that have followed us
/// longest; the rest go to those noted last (`add_swarm_member`).
const SWARM_LONGEST: i64 = MAX_SWARM_PER_BLOB / 2;

/// A blob's `accessed_ms` is updated at most this often, so a phone
/// seeking through a video doesn't write to the database on every request.
const TOUCH_EVERY_MS: i64 = 60_000;

/// Why a blob is kept, which decides who it is served to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlobClass {
    /// Fetched when one of my devices opened it. Served only to them;
    /// evicted first.
    Cache,
    /// Fetched ahead of time because I follow its author or a resharer
    /// (doc 13). Served to anyone if I share prefetched copies
    /// (`swarm.share_prefetched`), otherwise only to my devices. Evicted
    /// after `Cache`.
    Prefetch,
    /// Media I chose to host by resharing. Served to anyone.
    Replica,
    /// Media of my own posts. Served to anyone; never evicted.
    Own,
}

impl BlobClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlobClass::Cache => "cache",
            BlobClass::Prefetch => "prefetch",
            BlobClass::Replica => "replica",
            BlobClass::Own => "own",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "cache" => BlobClass::Cache,
            "prefetch" => BlobClass::Prefetch,
            "replica" => BlobClass::Replica,
            "own" => BlobClass::Own,
            other => anyhow::bail!("unknown blob class {other}"),
        })
    }

    /// Whether anyone (not just my own devices) may download it, whatever
    /// the settings: hosting it is a promise.
    pub fn is_public(&self) -> bool {
        matches!(self, BlobClass::Own | BlobClass::Replica)
    }

    /// Whether anyone may download it now. Prefetched copies may be when
    /// the owner chose to share them.
    pub fn served_to_anyone(&self, share_prefetched: bool) -> bool {
        self.is_public() || (*self == BlobClass::Prefetch && share_prefetched)
    }

    /// Whether it counts against the cache quota and is evicted to make room.
    pub fn is_evictable(&self) -> bool {
        matches!(self, BlobClass::Cache | BlobClass::Prefetch)
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

/// What this node measured about one peer node (ADR 0011).
#[derive(Debug, Clone)]
pub struct PeerRow {
    pub node: EndpointId,
    /// Whether this node ever contacted it, rather than only the other way round.
    pub ours: bool,
    pub first_ms: i64,
    pub last_ms: i64,
    pub counts: PeerCounts,
}

/// An upload from a paired device (`uploads.rs`).
#[derive(Debug, Clone)]
pub struct UploadRow {
    pub device: EndpointId,
    pub id: UploadId,
    pub content: String,
    pub files: Vec<Upload>,
    pub state: UploadState,
    /// When it last started, stopped or changed state.
    pub updated_ms: i64,
}

/// How far an upload got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadState {
    /// Waiting for the files, or the rest of them.
    Receiving,
    /// All the files are here; the post is being transcoded and published.
    Preparing,
    Posted(ObjectId),
    Failed(String),
}

impl UploadState {
    fn to_columns(&self) -> (&'static str, Option<String>) {
        match self {
            UploadState::Receiving => ("receiving", None),
            UploadState::Preparing => ("preparing", None),
            UploadState::Posted(id) => ("posted", Some(id.to_string())),
            UploadState::Failed(reason) => ("failed", Some(reason.clone())),
        }
    }

    fn from_columns(state: &str, outcome: Option<String>) -> Result<Self> {
        Ok(match (state, outcome) {
            ("receiving", _) => UploadState::Receiving,
            ("preparing", _) => UploadState::Preparing,
            ("posted", Some(id)) => UploadState::Posted(id.parse()?),
            ("failed", reason) => UploadState::Failed(reason.unwrap_or_default()),
            (other, _) => anyhow::bail!("unknown upload state {other}"),
        })
    }

    pub fn is_finished(&self) -> bool {
        matches!(self, UploadState::Posted(_) | UploadState::Failed(_))
    }
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
        self.insert(obj, payload, now_ms(), None)
    }

    /// Like `insert_object`, for an object that first arrived at `received`:
    /// a copy fetched again after a revocation takes the place of the one
    /// that was dropped, so it doesn't count as new today (ADR 0010).
    pub fn insert_object_received(
        &self,
        obj: &SignedObject,
        payload: &Payload,
        received: i64,
    ) -> Result<bool> {
        self.insert(obj, payload, received, None)
    }

    /// Like `insert_object`, for a post kept out of the feed until
    /// `until_ms`, or until `show_held` is called for it, while its media
    /// is on its way.
    pub fn insert_object_held(
        &self,
        obj: &SignedObject,
        payload: &Payload,
        until_ms: i64,
    ) -> Result<bool> {
        self.insert(obj, payload, now_ms(), Some(until_ms))
    }

    fn insert(
        &self,
        obj: &SignedObject,
        payload: &Payload,
        received: i64,
        held_until: Option<i64>,
    ) -> Result<bool> {
        // Clamp author-claimed time so a future-dated post can't pin itself to the top.
        let published = osp_proto::parse_rfc3339_ms(payload.published())
            .unwrap_or(received)
            .min(received);
        let conn = self.conn();
        let n = conn.execute(
            "INSERT OR IGNORE INTO objects
                 (id, actor, kind, published_ms, received_ms, signed, feed_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                obj.id().to_string(),
                payload.actor().to_string(),
                payload.kind(),
                published,
                received,
                serde_json::to_string(obj)?,
                held_until.unwrap_or(published)
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

    /// Stores `obj` in place of the object with its ID: the same payload,
    /// signed again by a newer key (ADR 0010).
    pub fn replace_signed(&self, obj: &SignedObject) -> Result<()> {
        self.conn().execute(
            "UPDATE objects SET signed = ?2 WHERE id = ?1",
            params![obj.id().to_string(), serde_json::to_string(obj)?],
        )?;
        Ok(())
    }

    /// Posts and reshares by the given identities, newest first: by when
    /// they were published, or, for one that was held back, when it was
    /// shown, so it lands above what the person may have scrolled past.
    pub fn feed(&self, actors: &[PublicKey], limit: u32) -> Result<Vec<SignedObject>> {
        if actors.is_empty() {
            return Ok(vec![]);
        }
        let placeholders = vec!["?"; actors.len()].join(",");
        let now = now_ms();
        let sql = format!(
            "SELECT signed FROM objects
             WHERE kind IN ('Note', 'Announce') AND actor IN ({placeholders}) AND feed_ms <= {now}
             ORDER BY feed_ms DESC LIMIT {limit}"
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let actors: Vec<String> = actors.iter().map(|a| a.to_string()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(actors), |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    /// Shows posts held back from the feed now, at the top: their media
    /// arrived, or won't. One shown already stays where it is.
    pub fn show_held(&self, ids: &[ObjectId]) -> Result<()> {
        let now = now_ms();
        let conn = self.conn();
        for id in ids {
            conn.execute(
                "UPDATE objects SET feed_ms = ?2 WHERE id = ?1 AND feed_ms > ?2",
                params![id.to_string(), now],
            )?;
        }
        Ok(())
    }

    /// Shows every post held back from the feed now. Returns how many.
    pub fn show_all_held(&self) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE objects SET feed_ms = ?1 WHERE feed_ms > ?1",
            params![now_ms()],
        )?)
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

    /// An object in our own outbox, by ID.
    pub fn outbox_object(&self, id: &ObjectId) -> Result<Option<SignedObject>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT objects.signed FROM outbox
                 JOIN objects ON objects.id = outbox.id
                 WHERE outbox.id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|j| serde_json::from_str(&j).context("corrupt stored object"))
            .transpose()
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

    /// Stops following `actor`, and forgets what was still to be fetched
    /// again from its node.
    pub fn remove_follow(&self, actor: &PublicKey) -> Result<bool> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let n = tx.execute(
            "DELETE FROM follows WHERE actor = ?1",
            params![actor.to_string()],
        )?;
        tx.execute(
            "DELETE FROM refetch WHERE actor = ?1",
            params![actor.to_string()],
        )?;
        tx.commit()?;
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
    pub fn add_follower(&self, addr: &EndpointAddr, shares: bool) -> Result<()> {
        self.conn().execute(
            "INSERT INTO followers (node, node_addr, since, seen_ms, shares) VALUES (?1, ?2, ?3, ?3, ?4)
             ON CONFLICT(node) DO UPDATE SET node_addr = excluded.node_addr,
                 seen_ms = excluded.seen_ms, tell_moved = 0, shares = excluded.shares",
            params![
                addr.id.to_string(),
                serde_json::to_string(addr)?,
                now_ms(),
                shares
            ],
        )?;
        Ok(())
    }

    /// Whether a follower said it shares what it fetches ahead of time.
    pub fn follower_shares(&self, node: &EndpointId) -> Result<bool> {
        let shares: Option<bool> = self
            .conn()
            .query_row(
                "SELECT shares FROM followers WHERE node = ?1",
                params![node.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(shares == Some(true))
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

    // ---- identity keys (ADR 0010) ----

    /// Checks the key certificate `cert`, dated `cert_ms`, against what we
    /// know of its identity's keys, and keeps it if it's the newest we've
    /// seen. Refuses a key that a newer certificate revoked. Returns true
    /// if this certificate revoked the keys before it.
    pub fn check_key(&self, cert: &SignedObject, key: &Key, cert_ms: i64) -> Result<bool> {
        let conn = self.conn();
        let actor = key.actor.to_string();
        let known: Option<(i64, Option<i64>)> = conn
            .query_row(
                "SELECT cert_ms, revoked_before_ms FROM identity_keys WHERE actor = ?1",
                params![actor],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let revoked_before = known.and_then(|(_, before)| before);
        ensure!(
            revoked_before.is_none_or(|before| cert_ms >= before),
            "signed with a key that {} has revoked",
            key.actor.fmt_short()
        );
        if known.is_some_and(|(newest, _)| cert_ms <= newest) {
            return Ok(false);
        }
        let revokes = key.earlier_keys == EarlierKeys::Revoked;
        let revoked_before = if revokes {
            Some(cert_ms)
        } else {
            revoked_before
        };
        conn.execute(
            "INSERT INTO identity_keys (actor, cert, cert_ms, revoked_before_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(actor) DO UPDATE SET cert = excluded.cert,
                 cert_ms = excluded.cert_ms, revoked_before_ms = excluded.revoked_before_ms",
            params![actor, serde_json::to_string(cert)?, cert_ms, revoked_before],
        )?;
        Ok(revokes)
    }

    /// Whether `obj` was signed by a key its identity has since revoked.
    pub fn signed_by_revoked_key(&self, obj: &SignedObject) -> Result<bool> {
        let before: Option<Option<i64>> = self
            .conn()
            .query_row(
                "SELECT revoked_before_ms FROM identity_keys WHERE actor = ?1",
                params![obj.signer().to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(before
            .flatten()
            .is_some_and(|before| signed_before(obj, before)))
    }

    /// Deletes the stored objects of `actor` signed by keys certified before
    /// `before_ms`, which a newer certificate revoked (ADR 0010), and its
    /// stored profile if one of those keys signed it. With `refetch`, keeps
    /// each deleted object's ID and when it arrived, to ask the identity's
    /// node for a copy signed by its new key. Returns how many it deleted.
    pub fn drop_revoked(&self, actor: &PublicKey, before_ms: i64, refetch: bool) -> Result<usize> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let mut dropped = Vec::new();
        {
            let mut stmt =
                tx.prepare("SELECT id, received_ms, signed FROM objects WHERE actor = ?1")?;
            let rows = stmt.query_map(params![actor.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (id, received, json) = row?;
                let obj: SignedObject =
                    serde_json::from_str(&json).context("corrupt stored object")?;
                if signed_before(&obj, before_ms) {
                    dropped.push((id, received));
                }
            }
        }
        for (id, received) in &dropped {
            tx.execute("DELETE FROM objects WHERE id = ?1", params![id])?;
            if refetch {
                tx.execute(
                    "INSERT OR IGNORE INTO refetch (id, actor, received_ms) VALUES (?1, ?2, ?3)",
                    params![id, actor.to_string(), received],
                )?;
            }
        }
        let profile: Option<String> = tx
            .query_row(
                "SELECT signed FROM profiles WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(json) = profile
            && signed_before(&serde_json::from_str(&json)?, before_ms)
        {
            tx.execute(
                "DELETE FROM profiles WHERE actor = ?1",
                params![actor.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(dropped.len())
    }

    /// Up to `limit` objects of `actor` still to ask its node for again,
    /// with when each first arrived.
    pub fn refetch_due(&self, actor: &PublicKey, limit: usize) -> Result<Vec<(ObjectId, i64)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id, received_ms FROM refetch WHERE actor = ?1 LIMIT ?2")?;
        let rows = stmt.query_map(params![actor.to_string(), limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.map(|r| {
            let (id, received) = r?;
            Ok((id.parse()?, received))
        })
        .collect()
    }

    /// Forgets objects that were asked for again, whether they came back or not.
    pub fn refetch_done(&self, ids: &[ObjectId]) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        for id in ids {
            tx.execute("DELETE FROM refetch WHERE id = ?1", params![id.to_string()])?;
        }
        tx.commit()?;
        Ok(())
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
        Ok(self.blob(hash)?.map(|(class, _)| class))
    }

    /// A blob's class and size, if we hold it.
    pub fn blob(&self, hash: &BlobHash) -> Result<Option<(BlobClass, u64)>> {
        let row: Option<(String, i64)> = self
            .conn()
            .query_row(
                "SELECT class, size FROM blobs WHERE hash = ?1",
                params![hash.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(class, size)| Ok((BlobClass::parse(&class)?, size as u64)))
            .transpose()
    }

    /// Records that we hold a blob. A blob's class only ever moves up
    /// (cache -> prefetch -> replica -> own), never down.
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

    /// Total size of the blobs under the cache quota: cached and prefetched.
    pub fn evictable_bytes(&self) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT COALESCE(SUM(size), 0) FROM blobs WHERE class IN ('cache', 'prefetch')",
            [],
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

    /// Forgets cached blobs, then prefetched ones, each in the order they
    /// were used, longest ago first, until at least `bytes` are freed or
    /// none are left (doc 13). Returns them, so their data can be released.
    /// Never touches other classes.
    pub fn evict_cache(&self, bytes: u64) -> Result<Vec<BlobHash>> {
        let conn = self.conn();
        let mut evicted = Vec::new();
        let mut freed = 0;
        {
            let mut stmt = conn.prepare(
                "SELECT hash, size FROM blobs WHERE class IN ('cache', 'prefetch')
                 ORDER BY class = 'prefetch', accessed_ms, added_ms",
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

    /// Who posted `blob`, from the posts we have.
    pub fn blob_author(&self, blob: &BlobHash) -> Result<Option<PublicKey>> {
        let author: Option<Option<String>> = self
            .conn()
            .query_row(
                "SELECT author FROM declared_sizes WHERE blob = ?1",
                params![blob.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(author.flatten().map(|a| a.parse()).transpose()?)
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

    /// Every node known to host some blob.
    pub fn host_nodes(&self) -> Result<Vec<EndpointId>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT DISTINCT node FROM hosts")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|r| Ok(r?.parse()?)).collect()
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

    // ---- the follower swarm (doc 13) ----

    /// Whether `node` is noted as holding a copy of `blob`.
    pub fn in_swarm(&self, blob: &BlobHash, node: &EndpointId) -> Result<bool> {
        let found: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM swarm WHERE blob = ?1 AND node = ?2",
                params![blob.to_string(), node.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Notes that a follower holds a copy of `blob`. Noting it again
    /// changes nothing. Only so many are kept for each blob, those that
    /// still share first: half the followers that have followed us longest,
    /// so a crowd of new followers can't push them out, and half those
    /// noted last, so followers that got it later, with their uploads free,
    /// are named too. A new one is always kept, as the last noted.
    pub fn add_swarm_member(&self, blob: &BlobHash, node: &EndpointId) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO swarm (blob, node, seen_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(blob, node) DO NOTHING",
            params![blob.to_string(), node.to_string(), now_ms()],
        )?;
        conn.execute(
            "WITH members AS (
                 SELECT s.node, f.shares IS 1 AS sharing, f.since, s.seen_ms
                 FROM swarm s LEFT JOIN followers f ON f.node = s.node
                 WHERE s.blob = ?1),
             longest AS (
                 SELECT node FROM members ORDER BY sharing DESC, since, seen_ms LIMIT ?2),
             latest AS (
                 SELECT node FROM members WHERE node NOT IN (SELECT node FROM longest)
                 ORDER BY sharing DESC, seen_ms DESC LIMIT ?3)
             DELETE FROM swarm WHERE blob = ?1
                 AND node NOT IN (SELECT node FROM longest)
                 AND node NOT IN (SELECT node FROM latest)",
            params![
                blob.to_string(),
                SWARM_LONGEST,
                MAX_SWARM_PER_BLOB - SWARM_LONGEST
            ],
        )?;
        Ok(())
    }

    /// Up to `n` followers, other than `except`, picked at random from those
    /// that hold a copy of `blob` and still say they share.
    pub fn swarm_sample(
        &self,
        blob: &BlobHash,
        except: &EndpointId,
        n: usize,
    ) -> Result<Vec<EndpointAddr>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT f.node_addr FROM swarm s JOIN followers f ON f.node = s.node
             WHERE s.blob = ?1 AND f.shares = 1 AND s.node != ?2
             ORDER BY RANDOM() LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![blob.to_string(), except.to_string(), n as i64],
            |row| row.get::<_, String>(0),
        )?;
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

    // ---- uploads from devices ----

    /// Records a new upload as receiving. False if the device already has
    /// one with this ID.
    pub fn add_upload(
        &self,
        device: &EndpointId,
        id: &UploadId,
        content: &str,
        files: &[Upload],
        now: i64,
    ) -> Result<bool> {
        let n = self.conn().execute(
            "INSERT OR IGNORE INTO uploads (device, id, content, files, state, updated_ms)
             VALUES (?1, ?2, ?3, ?4, 'receiving', ?5)",
            params![
                device.to_string(),
                id.to_string(),
                content,
                serde_json::to_string(files)?,
                now
            ],
        )?;
        Ok(n > 0)
    }

    pub fn upload(&self, device: &EndpointId, id: &UploadId) -> Result<Option<UploadRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT device, id, content, files, state, outcome, updated_ms FROM uploads
             WHERE device = ?1 AND id = ?2",
        )?;
        stmt.query_row(params![device.to_string(), id.to_string()], upload_columns)
            .optional()?
            .map(parse_upload)
            .transpose()
    }

    /// Every upload, oldest change first.
    pub fn uploads(&self) -> Result<Vec<UploadRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT device, id, content, files, state, outcome, updated_ms FROM uploads
             ORDER BY updated_ms, rowid",
        )?;
        let rows = stmt.query_map([], upload_columns)?;
        rows.map(|r| parse_upload(r?)).collect()
    }

    /// Changes an upload's state, and when it last changed.
    pub fn set_upload_state(
        &self,
        device: &EndpointId,
        id: &UploadId,
        state: &UploadState,
        now: i64,
    ) -> Result<()> {
        let (state, outcome) = state.to_columns();
        self.conn().execute(
            "UPDATE uploads SET state = ?3, outcome = ?4, updated_ms = ?5
             WHERE device = ?1 AND id = ?2",
            params![device.to_string(), id.to_string(), state, outcome, now],
        )?;
        Ok(())
    }

    pub fn remove_upload(&self, device: &EndpointId, id: &UploadId) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM uploads WHERE device = ?1 AND id = ?2",
            params![device.to_string(), id.to_string()],
        )?;
        Ok(n > 0)
    }

    /// Forgets finished uploads that last changed before `before_ms`.
    pub fn forget_finished_uploads(&self, before_ms: i64) -> Result<usize> {
        Ok(self.conn().execute(
            "DELETE FROM uploads WHERE state IN ('posted', 'failed') AND updated_ms < ?1",
            params![before_ms],
        )?)
    }

    /// The nodes each stored profile lists, with the profile's identity and name.
    pub fn profile_nodes(&self) -> Result<Vec<(PublicKey, String, Vec<EndpointId>)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT name, signed FROM profiles")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut found = Vec::new();
        for row in rows {
            let (name, json) = row?;
            let obj: SignedObject =
                serde_json::from_str(&json).context("corrupt stored profile")?;
            if let Ok(Payload::Person(person)) = obj.verify() {
                found.push((person.actor, name, person.nodes));
            }
        }
        Ok(found)
    }

    // ---- peers (ADR 0011) ----

    /// Adds to what this node measured about a peer node, after fading what
    /// was there to `now`. `ours` says this node made the contact. Paired
    /// devices are the owner's own, not peers, and aren't recorded. A new
    /// row, or one joining the group of nodes this node contacted, makes
    /// room by dropping the row in its group with the oldest contact; rows
    /// with no contact for `peers::FORGET_AFTER` go too.
    pub fn update_peer(
        &self,
        node: &EndpointId,
        ours: bool,
        now: i64,
        update: impl FnOnce(&mut PeerCounts),
    ) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let id = node.to_string();
        let device: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM devices WHERE node = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        if device.is_some() {
            return Ok(());
        }
        let known: Option<(bool, i64, String)> = tx
            .query_row(
                "SELECT ours, last_ms, counts FROM peers WHERE node = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (was_ours, mut counts) = match known {
            Some((was_ours, last_ms, json)) => (
                Some(was_ours),
                peers::fade(serde_json::from_str(&json)?, now - last_ms),
            ),
            None => (None, PeerCounts::default()),
        };
        update(&mut counts);
        let ours = ours || was_ours == Some(true);
        if was_ours != Some(ours) {
            // New to this group.
            tx.execute(
                "DELETE FROM peers WHERE last_ms < ?1",
                params![now - peers::FORGET_AFTER.as_millis() as i64],
            )?;
            let in_group: i64 = tx.query_row(
                "SELECT COUNT(*) FROM peers WHERE ours = ?1",
                params![ours],
                |row| row.get(0),
            )?;
            if in_group >= peers::MAX_PER_GROUP as i64 {
                tx.execute(
                    "DELETE FROM peers WHERE node =
                     (SELECT node FROM peers WHERE ours = ?1 ORDER BY last_ms LIMIT 1)",
                    params![ours],
                )?;
            }
        }
        tx.execute(
            "INSERT INTO peers (node, ours, first_ms, last_ms, counts) VALUES (?1, ?2, ?3, ?3, ?4)
             ON CONFLICT(node) DO UPDATE SET
                 ours = excluded.ours, last_ms = excluded.last_ms, counts = excluded.counts",
            params![id, ours, now, serde_json::to_string(&counts)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// What this node measured about each peer node, faded to `now`, most
    /// recent contact first. Forgets peers not in contact for `peers::FORGET_AFTER`.
    pub fn peers(&self, now: i64) -> Result<Vec<PeerRow>> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM peers WHERE last_ms < ?1",
            params![now - peers::FORGET_AFTER.as_millis() as i64],
        )?;
        let mut stmt = conn.prepare(
            "SELECT node, ours, first_ms, last_ms, counts FROM peers ORDER BY last_ms DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        rows.map(|r| {
            let (node, ours, first_ms, last_ms, json) = r?;
            Ok(PeerRow {
                node: node.parse()?,
                ours,
                first_ms,
                last_ms,
                counts: peers::fade(serde_json::from_str(&json)?, now - last_ms),
            })
        })
        .collect()
    }

    pub fn follower_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM followers", [], |row| row.get(0))?;
        Ok(n as usize)
    }
}

/// An `uploads` row as stored: device, id, content, files, state, outcome, updated_ms.
type UploadColumns = (String, String, String, String, String, Option<String>, i64);

fn upload_columns(row: &rusqlite::Row<'_>) -> rusqlite::Result<UploadColumns> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn parse_upload(columns: UploadColumns) -> Result<UploadRow> {
    let (device, id, content, files, state, outcome, updated_ms) = columns;
    Ok(UploadRow {
        device: device.parse()?,
        id: id.parse()?,
        content,
        files: serde_json::from_str(&files)?,
        state: UploadState::from_columns(&state, outcome)?,
        updated_ms,
    })
}

/// Runs the migrations this database hasn't had yet, each in one transaction.
fn migrate(conn: &Connection) -> Result<()> {
    let done: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(done as usize) {
        let version = i as i64 + 1;
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        // `declared_sizes` has had all its columns since 9, and every
        // database gets there in the same call.
        if version == 9 {
            backfill_declared_sizes(&tx)?;
        }
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

/// Whether a key certified before `before_ms` signed `obj`. Never true of
/// an object the identity key signed itself: that key is never revoked.
fn signed_before(obj: &SignedObject, before_ms: i64) -> bool {
    let Some(cert) = obj.certificate() else {
        return false;
    };
    match cert.verify() {
        Ok(Payload::Key(key)) => {
            osp_proto::parse_rfc3339_ms(&key.published).is_some_and(|cert_ms| cert_ms < before_ms)
        }
        _ => false,
    }
}

/// Records the sizes a post declares for its media and thumbnails, and who
/// posted them. If two people post the same file, the first is kept.
fn declare_sizes(conn: &Connection, note: &Note) -> Result<()> {
    for (blob, size) in note.sized_blobs() {
        conn.execute(
            "INSERT INTO declared_sizes (blob, size, author) VALUES (?1, ?2, ?3)
             ON CONFLICT(blob) DO UPDATE SET size = MAX(size, excluded.size),
                 author = COALESCE(author, excluded.author)",
            params![blob.to_string(), size as i64, note.actor.to_string()],
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
        db.put_blob(&h, BlobClass::Prefetch, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Prefetch));
        db.put_blob(&h, BlobClass::Cache, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Prefetch));
        db.put_blob(&h, BlobClass::Replica, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Replica));
        db.put_blob(&h, BlobClass::Prefetch, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Replica));
    }

    #[test]
    fn the_swarm_is_sampled_from_followers_that_share_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let blob = BlobHash::from_bytes([5; 32]);
        let sharing: Vec<EndpointAddr> = (0..3).map(|_| some_addr()).collect();
        for addr in &sharing {
            db.add_follower(addr, true).unwrap();
            db.add_swarm_member(&blob, &addr.id).unwrap();
        }
        // One that doesn't share, and one that isn't a follower any more.
        let quiet = some_addr();
        db.add_follower(&quiet, false).unwrap();
        let gone = some_addr();
        for node in [quiet.id, gone.id] {
            db.add_swarm_member(&blob, &node).unwrap();
        }
        assert!(db.follower_shares(&sharing[0].id).unwrap());
        assert!(!db.follower_shares(&quiet.id).unwrap());
        assert!(!db.follower_shares(&gone.id).unwrap());

        let sampled: Vec<EndpointId> = db
            .swarm_sample(&blob, &sharing[0].id, 10)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(sampled.len(), 2, "{sampled:?}");
        assert!(sampled.contains(&sharing[1].id) && sampled.contains(&sharing[2].id));
        assert_eq!(db.swarm_sample(&blob, &quiet.id, 1).unwrap().len(), 1);
        let other = BlobHash::from_bytes([6; 32]);
        assert!(db.swarm_sample(&other, &quiet.id, 10).unwrap().is_empty());

        for _ in 0..MAX_SWARM_PER_BLOB + 5 {
            let addr = some_addr();
            db.add_follower(&addr, true).unwrap();
            db.add_swarm_member(&blob, &addr.id).unwrap();
        }
        let kept: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM swarm", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, MAX_SWARM_PER_BLOB);
    }

    #[test]
    fn a_full_swarm_keeps_the_longest_followers_and_those_noted_last() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let blob = BlobHash::from_bytes([5; 32]);
        // A follower that shares, following us since `day`.
        let follower = |day: i64| {
            let addr = some_addr();
            db.add_follower(&addr, true).unwrap();
            let since = params![addr.id.to_string(), day * DAY_MS];
            let sql = "UPDATE followers SET since = ?2 WHERE node = ?1";
            db.conn().execute(sql, since).unwrap();
            addr
        };
        // Notes a follower now, then dates it `order`, so those noted
        // later are noted after it. Says whether it was kept.
        let add = |addr: &EndpointAddr, order: i64| {
            db.add_swarm_member(&blob, &addr.id).unwrap();
            let sql = "UPDATE swarm SET seen_ms = ?2 WHERE node = ?1";
            let seen = params![addr.id.to_string(), order];
            db.conn().execute(sql, seen).unwrap();
            db.in_swarm(&blob, &addr.id).unwrap()
        };
        let kept = |addrs: &[EndpointAddr]| -> Vec<bool> {
            addrs
                .iter()
                .map(|a| db.in_swarm(&blob, &a.id).unwrap())
                .collect()
        };
        let count = || -> i64 {
            let sql = "SELECT COUNT(*) FROM swarm WHERE blob = ?1";
            let key = params![blob.to_string()];
            db.conn().query_row(sql, key, |row| row.get(0)).unwrap()
        };
        // Full, noted in the order they followed.
        let members: Vec<EndpointAddr> = (1..=MAX_SWARM_PER_BLOB).map(follower).collect();
        for (i, member) in members.iter().enumerate() {
            assert!(add(member, i as i64 + 1));
        }

        // A follower following for less time than all of them is kept, as
        // the last noted, in place of the one noted longest ago of the
        // half kept for that.
        let half = SWARM_LONGEST as usize;
        let late = follower(MAX_SWARM_PER_BLOB + 1);
        assert!(add(&late, 1_000));
        assert_eq!(kept(&members[half..half + 1]), [false]);
        // One following for longer than all of them takes a place among
        // those that have followed longest; the newest of those moves to
        // the other half, in place of the one noted longest ago there.
        let early = follower(0);
        assert!(add(&early, 1_001));
        assert_eq!(kept(&members[half - 1..half]), [false]);
        assert_eq!(count(), MAX_SWARM_PER_BLOB);

        // A crowd of new followers takes only the half noted last.
        let crowd: Vec<EndpointAddr> = (0..60).map(|d| follower(1_000 + d)).collect();
        for (i, newcomer) in crowd.iter().enumerate() {
            assert!(add(newcomer, 2_000 + i as i64));
        }
        assert!(db.in_swarm(&blob, &early.id).unwrap());
        assert!(kept(&members[..half - 1]).iter().all(|&k| k));
        assert!(kept(&crowd[..10]).iter().all(|&k| !k));
        assert!(kept(&crowd[10..]).iter().all(|&k| k));
        assert_eq!(count(), MAX_SWARM_PER_BLOB);

        // A member that stopped sharing gives up its place to anyone, in
        // either half.
        db.add_follower(&members[0], false).unwrap();
        assert!(add(&follower(2_000), 3_000));
        assert!(!db.in_swarm(&blob, &members[0].id).unwrap());
        db.add_follower(&crowd[59], false).unwrap();
        assert!(add(&follower(2_001), 3_001));
        assert_eq!(kept(&crowd[58..]), [true, false]);
        assert_eq!(count(), MAX_SWARM_PER_BLOB);

        // Noted again, a member keeps the time it was first noted.
        let seen = |node: &EndpointId| -> i64 {
            let sql = "SELECT seen_ms FROM swarm WHERE blob = ?1 AND node = ?2";
            let keys = params![blob.to_string(), node.to_string()];
            db.conn().query_row(sql, keys, |row| row.get(0)).unwrap()
        };
        let sql = "UPDATE swarm SET seen_ms = 1 WHERE node = ?1";
        db.conn()
            .execute(sql, params![early.id.to_string()])
            .unwrap();
        db.add_swarm_member(&blob, &early.id).unwrap();
        assert_eq!(seen(&early.id), 1);
    }

    fn some_addr() -> EndpointAddr {
        EndpointAddr::new(iroh::SecretKey::generate().public())
    }

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    /// Adds `n` peers to one group, the first with the oldest contact.
    fn add_peers(db: &Db, ours: bool, n: usize, from_ms: i64) -> Vec<EndpointId> {
        let conn = db.conn();
        let tx = conn.unchecked_transaction().unwrap();
        let mut nodes = Vec::new();
        for i in 0..n {
            let node = some_addr().id;
            tx.execute(
                "INSERT INTO peers (node, ours, first_ms, last_ms, counts) VALUES (?1, ?2, ?3, ?3, '{}')",
                params![node.to_string(), ours, from_ms + i as i64],
            )
            .unwrap();
            nodes.push(node);
        }
        tx.commit().unwrap();
        nodes
    }

    fn peer(db: &Db, node: &EndpointId, now: i64) -> Option<PeerRow> {
        db.peers(now).unwrap().into_iter().find(|p| p.node == *node)
    }

    fn count(c: &mut PeerCounts) {
        c.requests += 1.0;
    }

    #[test]
    fn peers_are_kept_in_two_groups_of_limited_size() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let now = now_ms();
        let max = peers::MAX_PER_GROUP;
        let ours = add_peers(&db, true, max, now - 1000 * max as i64);
        let theirs = add_peers(&db, false, max, now - 2000 * max as i64);

        // A new node this node contacted takes the place of the one in its
        // group with the oldest contact, however old the other group's are.
        let new = some_addr().id;
        db.update_peer(&new, true, now, count).unwrap();
        let rows = db.peers(now).unwrap();
        assert_eq!(rows.iter().filter(|p| p.ours).count(), max);
        assert_eq!(rows.iter().filter(|p| !p.ours).count(), max);
        assert!(peer(&db, &ours[0], now).is_none());
        assert!(peer(&db, &ours[1], now).is_some());
        assert!(peer(&db, &theirs[0], now).is_some());

        // A stranger the node then contacts moves into that group, and
        // makes room there too.
        db.update_peer(&theirs[5], true, now, count).unwrap();
        assert!(peer(&db, &theirs[5], now).unwrap().ours);
        assert!(peer(&db, &ours[1], now).is_none());
        let rows = db.peers(now).unwrap();
        assert_eq!(rows.iter().filter(|p| p.ours).count(), max);
        assert_eq!(rows.iter().filter(|p| !p.ours).count(), max - 1);

        // A node contacting us later doesn't leave that group, and a known
        // node doesn't push anyone out.
        db.update_peer(&theirs[5], false, now, count).unwrap();
        assert!(peer(&db, &theirs[5], now).unwrap().ours);
        assert!(peer(&db, &ours[2], now).is_some());

        // A new stranger fills the free place without pushing anyone out,
        // and the next one pushes out the stranger with the oldest contact.
        db.update_peer(&some_addr().id, false, now, count).unwrap();
        assert!(peer(&db, &theirs[0], now).is_some());
        db.update_peer(&some_addr().id, false, now, count).unwrap();
        assert!(peer(&db, &theirs[0], now).is_none());
        assert!(peer(&db, &ours[2], now).is_some());
    }

    #[test]
    fn devices_and_long_silent_peers_arent_kept() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let now = now_ms();
        let phone = some_addr().id;
        db.add_device(&phone, "phone").unwrap();
        db.update_peer(&phone, true, now, count).unwrap();
        assert!(db.peers(now).unwrap().is_empty());

        let forget = peers::FORGET_AFTER.as_millis() as i64;
        let silent = add_peers(&db, true, 2, now - forget - 1);
        let recent = add_peers(&db, true, 1, now - forget + DAY_MS);
        // Listing forgets the silent ones...
        assert!(peer(&db, &silent[0], now).is_none());
        assert!(peer(&db, &recent[0], now).is_some());
        // ...and so does a new row.
        let silent = add_peers(&db, false, 1, now - forget - 1);
        db.update_peer(&some_addr().id, false, now, count).unwrap();
        let left: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM peers WHERE node = ?1",
                params![silent[0].to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn peer_counts_fade_before_they_are_added_to_and_when_read() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let start = now_ms() - 28 * DAY_MS;
        let node = some_addr().id;
        db.update_peer(&node, true, start, |c| c.requests += 4.0)
            .unwrap();
        db.update_peer(&node, false, start + 14 * DAY_MS, count)
            .unwrap();
        let row = peer(&db, &node, start + 28 * DAY_MS).unwrap();
        // 4 faded to 2, plus 1, all faded by half again.
        assert!((row.counts.requests - 1.5).abs() < 1e-9, "{row:?}");
        assert_eq!(row.first_ms, start);
        assert_eq!(row.last_ms, start + 14 * DAY_MS);
        assert!(row.ours);
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
        db.add_follower(&quiet, false).unwrap();
        db.add_follower(&chatty, false).unwrap();
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
            db.add_follower(f, false).unwrap();
        }
        assert!(db.followers_to_tell_moved().unwrap().is_empty());
        assert_eq!(db.mark_followers_to_tell_moved().unwrap(), 3);
        db.told_moved(&a.id).unwrap();
        // Subscribing to this node means b already knows where we are.
        db.add_follower(&b, false).unwrap();
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
        assert_eq!(db.blob_author(&old).unwrap(), Some(key.public()));

        let (obj, payload) = note_with_media(&key, &[(new, 300)]);
        db.insert_object(&obj, &payload).unwrap();
        assert_eq!(db.declared_size(&new).unwrap(), Some(300));
        // Another post can't shrink it, only raise it.
        let (obj, payload) = note_with_media(&key, &[(new, 10), (old, 900)]);
        db.insert_object(&obj, &payload).unwrap();
        assert_eq!(db.declared_size(&new).unwrap(), Some(300));
        assert_eq!(db.declared_size(&old).unwrap(), Some(900));
        // The first to post it stays its author.
        let other = iroh::SecretKey::generate();
        let (obj, payload) = note_with_media(&other, &[(new, 300)]);
        db.insert_object(&obj, &payload).unwrap();
        assert_eq!(db.blob_author(&new).unwrap(), Some(key.public()));
        assert_eq!(
            db.blob_author(&BlobHash::from_bytes([3; 32])).unwrap(),
            None
        );
    }

    #[test]
    fn a_database_from_before_authors_learns_who_posted_each_blob() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let key = iroh::SecretKey::generate();
        let blob = BlobHash::from_bytes([1; 32]);
        {
            // A database from before migration 9, holding a post with media.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            for sql in &MIGRATIONS[..8] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 8).unwrap();
            let (obj, _) = note_with_media(&key, &[(blob, 500)]);
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
            conn.execute(
                "INSERT INTO declared_sizes (blob, size) VALUES (?1, 500)",
                params![blob.to_string()],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.declared_size(&blob).unwrap(), Some(500));
        assert_eq!(db.blob_author(&blob).unwrap(), Some(key.public()));
    }

    #[test]
    fn held_posts_join_the_feed_on_top_when_shown_or_when_their_time_is_up() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let key = iroh::SecretKey::generate();
        let ids = |db: &Db| -> Vec<ObjectId> {
            let feed = db.feed(&[key.public()], 10).unwrap();
            feed.iter().map(|o| o.id()).collect()
        };
        let post = |text: &str| {
            let payload = Payload::Note(Note {
                actor: key.public(),
                content: text.into(),
                published: osp_proto::now_rfc3339(),
                attachment: vec![],
                in_reply_to: None,
            });
            (SignedObject::sign(&key, &payload).unwrap(), payload)
        };
        // Times are in milliseconds; keep steps that must be ordered apart.
        let pause = || std::thread::sleep(std::time::Duration::from_millis(5));
        let far = now_ms() + DAY_MS;
        let (first, payload) = post("first");
        assert!(db.insert_object_held(&first, &payload, far).unwrap());
        let (second, payload) = post("second");
        db.insert_object(&second, &payload).unwrap();
        assert_eq!(ids(&db), vec![second.id()]);

        // Shown above the post that came after it, and only moved once.
        pause();
        db.show_held(&[first.id()]).unwrap();
        assert_eq!(ids(&db), vec![first.id(), second.id()]);
        pause();
        let (third, payload) = post("third");
        db.insert_object(&third, &payload).unwrap();
        db.show_held(&[first.id()]).unwrap();
        assert_eq!(ids(&db), vec![third.id(), first.id(), second.id()]);

        // One whose time is up is shown without being asked.
        let (fourth, payload) = post("fourth");
        db.insert_object_held(&fourth, &payload, now_ms() + 50)
            .unwrap();
        assert_eq!(ids(&db).len(), 3);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(ids(&db)[0], fourth.id());

        // And a restart shows them all.
        let (fifth, payload) = post("fifth");
        db.insert_object_held(&fifth, &payload, far).unwrap();
        assert_eq!(db.show_all_held().unwrap(), 1);
        assert_eq!(ids(&db)[0], fifth.id());
    }

    #[test]
    fn eviction_takes_cache_blobs_used_longest_ago_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let [own, replica, stale, recent, prefetched, prefetched_later] =
            [1u8, 2, 3, 4, 5, 6].map(|b| BlobHash::from_bytes([b; 32]));
        db.put_blob(&own, BlobClass::Own, 100).unwrap();
        db.put_blob(&replica, BlobClass::Replica, 100).unwrap();
        db.put_blob(&stale, BlobClass::Cache, 100).unwrap();
        db.put_blob(&recent, BlobClass::Cache, 100).unwrap();
        db.put_blob(&prefetched, BlobClass::Prefetch, 100).unwrap();
        db.put_blob(&prefetched_later, BlobClass::Prefetch, 100)
            .unwrap();
        db.conn()
            .execute("UPDATE blobs SET accessed_ms = 1", [])
            .unwrap();
        // Used longest ago, but prefetched copies go after cached ones.
        db.conn()
            .execute(
                "UPDATE blobs SET accessed_ms = 0 WHERE class = 'prefetch'",
                [],
            )
            .unwrap();
        db.touch_blob(&recent).unwrap();
        db.touch_blob(&prefetched_later).unwrap();
        assert_eq!(db.class_bytes(BlobClass::Cache).unwrap(), 200);
        assert_eq!(db.evictable_bytes().unwrap(), 400);

        assert_eq!(db.evict_cache(0).unwrap(), vec![]);
        assert_eq!(db.evict_cache(1).unwrap(), vec![stale]);
        assert_eq!(db.blob_class(&stale).unwrap(), None);
        assert_eq!(db.evict_cache(1).unwrap(), vec![recent]);
        assert_eq!(db.evict_cache(1).unwrap(), vec![prefetched]);
        // Asking for more than the cache holds empties the cache, no more.
        assert_eq!(db.evict_cache(1 << 30).unwrap(), vec![prefetched_later]);
        assert_eq!(db.evictable_bytes().unwrap(), 0);
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
