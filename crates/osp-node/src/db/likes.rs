//! Likes (ADR 0038): on the creator's node, who likes each of its owner's
//! posts; on the liker's node, each like or take-back of the owner's until
//! the creator's node answers.

use anyhow::{Context, Result};
use iroh::{EndpointId, PublicKey};
use osp_proto::{ObjectId, SignedObject};
use rusqlite::{OptionalExtension, params};

use super::Db;

/// Someone's like of one of the owner's posts, as the creator's node keeps
/// it.
#[derive(Debug, Clone)]
pub(crate) struct HeldLike {
    pub post: ObjectId,
    pub actor: PublicKey,
    pub id: ObjectId,
    pub node: EndpointId,
    pub signed: SignedObject,
    pub profile: SignedObject,
    /// Its date, never later than its arrival.
    pub published_ms: i64,
    pub received_ms: i64,
}

/// The owner's like of someone's post, or its take-back, as it's kept.
#[derive(Debug, Clone)]
pub(crate) struct SentLike {
    pub post: ObjectId,
    pub creator: PublicKey,
    /// The node to send to, when no newer profile of the creator names one.
    pub creator_node: EndpointId,
    /// The `Like` the creator's node may hold, which an `Undo` names.
    pub like_id: ObjectId,
    /// The signed `Like` or `Undo` to send, and its ID.
    pub signed: SignedObject,
    pub sent_id: ObjectId,
    pub liked: bool,
    pub tries: u32,
    /// None once answered.
    pub next_try_ms: Option<i64>,
    pub since_ms: i64,
    pub error: Option<String>,
}

fn parse<T: std::str::FromStr<Err = anyhow::Error>>(s: String) -> Result<T> {
    s.parse()
}

fn json<T: serde::de::DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_str(s).context("corrupt stored like")
}

const HELD_COLUMNS: &str = "post, actor, id, node, signed, profile, published_ms, received_ms";
const SENT_COLUMNS: &str = "post, creator, creator_node, like_id, signed, sent_id, liked, tries, \
                            next_try_ms, since_ms, error";

type HeldRaw = (String, String, String, String, String, String, i64, i64);

fn held_raw(row: &rusqlite::Row) -> rusqlite::Result<HeldRaw> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn held(raw: HeldRaw) -> Result<HeldLike> {
    let (post, actor, id, node, signed, profile, published_ms, received_ms) = raw;
    Ok(HeldLike {
        post: parse(post)?,
        actor: actor.parse()?,
        id: parse(id)?,
        node: node.parse()?,
        signed: json(&signed)?,
        profile: json(&profile)?,
        published_ms,
        received_ms,
    })
}

type SentRaw = (
    String,
    String,
    String,
    String,
    String,
    String,
    bool,
    u32,
    Option<i64>,
    i64,
    Option<String>,
);

fn sent_raw(row: &rusqlite::Row) -> rusqlite::Result<SentRaw> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

fn sent(raw: SentRaw) -> Result<SentLike> {
    let (post, creator, creator_node, like_id, signed, sent_id, liked, tries, next, since, error) =
        raw;
    Ok(SentLike {
        post: parse(post)?,
        creator: creator.parse()?,
        creator_node: creator_node.parse()?,
        like_id: parse(like_id)?,
        signed: json(&signed)?,
        sent_id: parse(sent_id)?,
        liked,
        tries,
        next_try_ms: next,
        since_ms: since,
        error,
    })
}

impl Db {
    // ---- on the creator's node ----

    /// Someone's like of `post`, if they like it.
    pub(crate) fn held_like(&self, post: &ObjectId, actor: &PublicKey) -> Result<Option<HeldLike>> {
        let raw = self
            .conn()
            .query_row(
                &format!("SELECT {HELD_COLUMNS} FROM likes WHERE post = ?1 AND actor = ?2"),
                params![post.to_string(), actor.to_string()],
                held_raw,
            )
            .optional()?;
        raw.map(held).transpose()
    }

    /// Keeps `like`, in place of any earlier one by the same person of the
    /// same post. Then, past `max` likes in all, drops those on the oldest
    /// posts first.
    pub(crate) fn keep_like(&self, like: &HeldLike, max: usize) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            &format!("INSERT OR REPLACE INTO likes ({HELD_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
            params![
                like.post.to_string(),
                like.actor.to_string(),
                like.id.to_string(),
                like.node.to_string(),
                serde_json::to_string(&like.signed)?,
                serde_json::to_string(&like.profile)?,
                like.published_ms,
                like.received_ms,
            ],
        )?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM likes", [], |r| r.get(0))?;
        let excess = count - max as i64;
        if excess > 0 {
            tx.execute(
                "DELETE FROM likes WHERE rowid IN (
                    SELECT l.rowid FROM likes l LEFT JOIN objects o ON o.id = l.post
                    ORDER BY COALESCE(o.published_ms, 0), l.received_ms
                    LIMIT ?1)",
                params![excess],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Drops the like `id` by `actor`. Says whether it was held.
    pub(crate) fn drop_like(&self, id: &ObjectId, actor: &PublicKey) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM likes WHERE id = ?1 AND actor = ?2",
            params![id.to_string(), actor.to_string()],
        )?;
        Ok(n > 0)
    }

    /// How many people like `post`.
    pub(crate) fn like_count(&self, post: &ObjectId) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM likes WHERE post = ?1",
            params![post.to_string()],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }

    /// Who likes `post`, newest first.
    pub(crate) fn likes_of(&self, post: &ObjectId) -> Result<Vec<HeldLike>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {HELD_COLUMNS} FROM likes WHERE post = ?1 ORDER BY published_ms DESC"
        ))?;
        let raws = stmt
            .query_map(params![post.to_string()], held_raw)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raws.into_iter().map(held).collect()
    }

    // ---- on the liker's node ----

    /// The owner's like of `post`, or its take-back, if there is one.
    pub(crate) fn sent_like(&self, post: &ObjectId) -> Result<Option<SentLike>> {
        let raw = self
            .conn()
            .query_row(
                &format!("SELECT {SENT_COLUMNS} FROM sent_likes WHERE post = ?1"),
                params![post.to_string()],
                sent_raw,
            )
            .optional()?;
        raw.map(sent).transpose()
    }

    /// Keeps `like` to send, in place of anything kept for its post.
    pub(crate) fn keep_sent_like(&self, like: &SentLike) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT OR REPLACE INTO sent_likes ({SENT_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
            ),
            params![
                like.post.to_string(),
                like.creator.to_string(),
                like.creator_node.to_string(),
                like.like_id.to_string(),
                serde_json::to_string(&like.signed)?,
                like.sent_id.to_string(),
                like.liked,
                like.tries,
                like.next_try_ms,
                like.since_ms,
                like.error,
            ],
        )?;
        Ok(())
    }

    /// The creator's node answered `sent`, refusing it with `error` or not.
    /// Nothing changes if the owner has changed their mind since.
    pub(crate) fn sent_like_answered(
        &self,
        post: &ObjectId,
        sent: &ObjectId,
        error: Option<&str>,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE sent_likes SET next_try_ms = NULL, error = ?3
             WHERE post = ?1 AND sent_id = ?2",
            params![post.to_string(), sent.to_string(), error],
        )?;
        Ok(())
    }

    /// The creator's node didn't answer `sent`: try at `next`, or never.
    pub(crate) fn sent_like_unanswered(
        &self,
        post: &ObjectId,
        sent: &ObjectId,
        next: Option<i64>,
        why: &str,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE sent_likes SET tries = tries + 1, next_try_ms = ?3, error = ?4
             WHERE post = ?1 AND sent_id = ?2",
            params![post.to_string(), sent.to_string(), next, why],
        )?;
        Ok(())
    }

    /// The posts whose like or take-back is due by `now`, or with `all`
    /// every one still waiting.
    pub(crate) fn sent_likes_due(&self, now: i64, all: bool) -> Result<Vec<ObjectId>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT post FROM sent_likes
             WHERE next_try_ms IS NOT NULL AND (?2 OR next_try_ms <= ?1)",
        )?;
        let posts = stmt
            .query_map(params![now, all], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        posts.into_iter().map(parse).collect()
    }

    /// When the next like is due to be tried, if any is.
    pub(crate) fn next_sent_like_try(&self) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row("SELECT MIN(next_try_ms) FROM sent_likes", [], |r| r.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use osp_proto::{Like, Note, Payload};

    use super::*;

    fn post(db: &Db, key: &iroh::SecretKey, published: &str) -> ObjectId {
        let note = Payload::Note(Note {
            actor: key.public(),
            content: published.into(),
            published: published.into(),
            attachment: vec![],
            in_reply_to: None,
        });
        let signed = SignedObject::sign(key, &note).unwrap();
        db.insert_object(&signed, &note).unwrap();
        signed.id()
    }

    fn like(post: ObjectId, at: i64) -> HeldLike {
        let liker = iroh::SecretKey::generate();
        let signed = SignedObject::sign(
            &liker,
            &Payload::Like(Like {
                actor: liker.public(),
                object: post,
                published: osp_proto::now_rfc3339(),
            }),
        )
        .unwrap();
        HeldLike {
            post,
            actor: liker.public(),
            id: signed.id(),
            node: iroh::SecretKey::generate().public(),
            profile: signed.clone(),
            signed,
            published_ms: at,
            received_ms: at,
        }
    }

    #[test]
    fn past_the_most_kept_the_oldest_posts_likes_go_first() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("osp.db")).unwrap();
        let kim = iroh::SecretKey::generate();
        let old = post(&db, &kim, "2026-01-01T00:00:00Z");
        let new = post(&db, &kim, "2026-06-01T00:00:00Z");
        db.keep_like(&like(new, 1), 3).unwrap();
        db.keep_like(&like(old, 2), 3).unwrap();
        db.keep_like(&like(new, 3), 3).unwrap();
        assert_eq!(
            (db.like_count(&old).unwrap(), db.like_count(&new).unwrap()),
            (1, 2)
        );
        // A fourth: the old post's like goes, though it came later.
        db.keep_like(&like(new, 4), 3).unwrap();
        assert_eq!(
            (db.like_count(&old).unwrap(), db.like_count(&new).unwrap()),
            (0, 3)
        );
    }
}
