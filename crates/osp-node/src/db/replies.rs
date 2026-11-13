//! Replies to this node's posts (ADR 0028): each post's policy, the replies
//! received and what became of each, the repliers allowed or blocked, and the
//! repliers' `Delete`s.

use anyhow::{Context, Result, bail, ensure};
use iroh::{EndpointId, PublicKey};
use osp_proto::{ObjectId, SignedObject, social::ReplyPolicy};
use rusqlite::{Connection, OptionalExtension, params};

use super::Db;
use crate::replies::{
    MAX_REPLIES_PER_DAY, MAX_REQUESTS, MAX_REQUESTS_FROM_ONE, MAX_THREAD, REQUESTS_KEPT,
};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Where a received reply stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Held {
    Published,
    /// Taken out of the thread by the post's owner, without a `Delete`.
    Hidden,
    /// Waiting for the owner.
    Request,
    /// A request the owner declined, kept so it isn't asked again.
    Declined,
}

impl Held {
    fn as_str(self) -> &'static str {
        match self {
            Held::Published => "published",
            Held::Hidden => "hidden",
            Held::Request => "request",
            Held::Declined => "declined",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "published" => Held::Published,
            "hidden" => Held::Hidden,
            "request" => Held::Request,
            "declined" => Held::Declined,
            _ => bail!("unknown reply state {s:?}"),
        })
    }
}

/// What the owner chose for a replier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Choice {
    Allowed,
    Blocked,
}

/// A reply this node received, checked.
#[derive(Debug, Clone)]
pub(crate) struct ReceivedReply {
    pub id: ObjectId,
    pub post: ObjectId,
    pub actor: PublicKey,
    pub node: EndpointId,
    pub signed: SignedObject,
    pub profile: SignedObject,
    pub received_ms: i64,
}

/// A reply as it's kept, with where it stands.
#[derive(Debug, Clone)]
pub(crate) struct ReplyRow {
    pub reply: ReceivedReply,
    pub held: Held,
}

impl Db {
    /// The policy the owner set for `post`, if they set one.
    pub fn reply_policy(&self, post: &ObjectId) -> Result<Option<ReplyPolicy>> {
        let policy: Option<String> = self
            .conn()
            .query_row(
                "SELECT policy FROM reply_policies WHERE post = ?1",
                params![post.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        policy.map(|p| p.parse()).transpose()
    }

    pub fn set_reply_policy(&self, post: &ObjectId, policy: ReplyPolicy) -> Result<()> {
        self.conn().execute(
            "INSERT INTO reply_policies (post, policy) VALUES (?1, ?2)
             ON CONFLICT (post) DO UPDATE SET policy = excluded.policy",
            params![post.to_string(), policy.as_str()],
        )?;
        Ok(())
    }

    /// What the owner chose for `actor`, if anything.
    pub fn replier_choice(&self, actor: &PublicKey) -> Result<Option<Choice>> {
        let choice: Option<String> = self
            .conn()
            .query_row(
                "SELECT choice FROM repliers WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(match choice.as_deref() {
            Some("allowed") => Some(Choice::Allowed),
            Some("blocked") => Some(Choice::Blocked),
            _ => None,
        })
    }

    /// Allows or blocks `actor`, or with `None` forgets the choice. Blocking
    /// hides their published replies on this node's posts and declines their
    /// requests; allowing them again shows nothing hidden.
    pub fn set_replier(&self, actor: &PublicKey, choice: Option<Choice>, now: i64) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let actor = actor.to_string();
        match choice {
            None => {
                tx.execute("DELETE FROM repliers WHERE actor = ?1", params![actor])?;
            }
            Some(choice) => {
                let choice = match choice {
                    Choice::Allowed => "allowed",
                    Choice::Blocked => "blocked",
                };
                tx.execute(
                    "INSERT INTO repliers (actor, choice, at_ms) VALUES (?1, ?2, ?3)
                     ON CONFLICT (actor) DO UPDATE SET choice = excluded.choice, at_ms = excluded.at_ms",
                    params![actor, choice, now],
                )?;
                if choice == "blocked" {
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
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Where the reply `id` stands here, if it's kept.
    pub fn received_reply_state(&self, id: &ObjectId, now: i64) -> Result<Option<Held>> {
        let conn = self.conn();
        forget_old_requests(&conn, now)?;
        let state: Option<String> = conn
            .query_row(
                "SELECT state FROM received_replies WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        state.map(|s| Held::parse(&s)).transpose()
    }

    /// Whether the replier deleted the reply `id`.
    pub fn reply_deleted(&self, id: &ObjectId) -> Result<bool> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM reply_deletes WHERE reply = ?1",
            params![id.to_string()],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }

    /// Keeps a reply that's new here, published if `publish` and the thread
    /// has room, else as a request if there's room for one. Within the
    /// limits (`crate::replies`): a replier's replies to one post a day, the
    /// thread, requests in all and from one replier. `None` if it's refused
    /// over a limit; a request isn't pushed out to make room, or a flood would
    /// wipe out the real ones.
    pub fn add_received_reply(&self, reply: &ReceivedReply, publish: bool) -> Result<Option<Held>> {
        let now = reply.received_ms;
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        forget_old_requests(&tx, now)?;
        let (post, actor) = (reply.post.to_string(), reply.actor.to_string());
        let today: i64 = tx.query_row(
            "SELECT COUNT(*) FROM received_replies
             WHERE actor = ?1 AND post = ?2 AND received_ms >= ?3",
            params![actor, post, now - DAY_MS],
            |row| row.get(0),
        )?;
        if today as usize >= MAX_REPLIES_PER_DAY {
            return Ok(None);
        }
        let in_thread: i64 = tx.query_row(
            "SELECT COUNT(*) FROM received_replies
             WHERE post = ?1 AND state IN ('published', 'hidden')",
            params![post],
            |row| row.get(0),
        )?;
        let held = if publish && (in_thread as usize) < MAX_THREAD {
            Held::Published
        } else {
            let (all, theirs): (i64, i64) = tx.query_row(
                "SELECT COUNT(*), COALESCE(SUM(actor = ?1), 0) FROM received_replies
                 WHERE state = 'request'",
                params![actor],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if all as usize >= MAX_REQUESTS || theirs as usize >= MAX_REQUESTS_FROM_ONE {
                return Ok(None);
            }
            Held::Request
        };
        let seq = match held {
            Held::Published => Some(next_thread_seq(&tx, now)?),
            _ => None,
        };
        tx.execute(
            "INSERT INTO received_replies
                 (id, post, actor, node, state, thread_seq, signed, profile, received_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                reply.id.to_string(),
                post,
                actor,
                reply.node.to_string(),
                held.as_str(),
                seq,
                serde_json::to_string(&reply.signed)?,
                serde_json::to_string(&reply.profile)?,
                now
            ],
        )?;
        tx.commit()?;
        Ok(Some(held))
    }

    /// The reply `id`, as it's kept.
    pub fn received_reply(&self, id: &ObjectId) -> Result<Option<ReplyRow>> {
        let conn = self.conn();
        reply_rows(&conn, "WHERE id = ?1", params![id.to_string()]).map(|mut rows| rows.pop())
    }

    /// Requests waiting, newest first.
    pub fn reply_requests(&self, now: i64) -> Result<Vec<ReplyRow>> {
        let conn = self.conn();
        forget_old_requests(&conn, now)?;
        reply_rows(
            &conn,
            "WHERE state = 'request' ORDER BY received_ms DESC",
            [],
        )
    }

    /// Published and hidden replies to `post`, in thread order.
    pub fn kept_thread(&self, post: &ObjectId) -> Result<Vec<ReplyRow>> {
        let conn = self.conn();
        reply_rows(
            &conn,
            "WHERE post = ?1 AND state IN ('published', 'hidden') ORDER BY thread_seq",
            params![post.to_string()],
        )
    }

    /// Moves a kept reply to `held`: publishing gives it a new place at the
    /// end of its thread, if the thread has room. Says what it was before.
    pub fn move_reply(&self, id: &ObjectId, held: Held, now: i64) -> Result<Held> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let (was, post): (String, String) = tx
            .query_row(
                "SELECT state, post FROM received_replies WHERE id = ?1",
                params![id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .with_context(|| format!("no reply {} is kept here", id.fmt_short()))?;
        let was = Held::parse(&was)?;
        let seq = match held {
            Held::Published if was != Held::Published => {
                if was != Held::Hidden {
                    let in_thread: i64 = tx.query_row(
                        "SELECT COUNT(*) FROM received_replies
                         WHERE post = ?1 AND state IN ('published', 'hidden')",
                        params![post],
                        |row| row.get(0),
                    )?;
                    ensure!(
                        (in_thread as usize) < MAX_THREAD,
                        "that post's thread is full"
                    );
                }
                Some(next_thread_seq(&tx, now)?)
            }
            _ => None,
        };
        match seq {
            Some(seq) => tx.execute(
                "UPDATE received_replies SET state = ?2, thread_seq = ?3 WHERE id = ?1",
                params![id.to_string(), held.as_str(), seq],
            )?,
            None => tx.execute(
                "UPDATE received_replies SET state = ?2 WHERE id = ?1",
                params![id.to_string(), held.as_str()],
            )?,
        };
        tx.commit()?;
        Ok(was)
    }

    /// Acts on `actor`'s `Delete` of their reply `id`: the reply goes and the
    /// `Delete` is kept. False if no reply of theirs by that ID is kept.
    pub fn delete_received_reply(
        &self,
        id: &ObjectId,
        actor: &PublicKey,
        delete: &SignedObject,
        now: i64,
    ) -> Result<bool> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let post: Option<String> = tx
            .query_row(
                "SELECT post FROM received_replies WHERE id = ?1 AND actor = ?2",
                params![id.to_string(), actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(post) = post else {
            return Ok(false);
        };
        tx.execute(
            "DELETE FROM received_replies WHERE id = ?1",
            params![id.to_string()],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO reply_deletes (reply, post, actor, signed, received_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id.to_string(),
                post,
                actor.to_string(),
                serde_json::to_string(delete)?,
                now
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }
}

/// Everything about replies to `post`, which its owner deleted: the thread is
/// closed, and readers get the owner's `Delete` (ADR 0028).
pub(super) fn forget_replies_to(conn: &Connection, post: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM received_replies WHERE post = ?1",
        params![post],
    )?;
    conn.execute("DELETE FROM reply_deletes WHERE post = ?1", params![post])?;
    conn.execute("DELETE FROM reply_policies WHERE post = ?1", params![post])?;
    Ok(())
}

/// Forgets requests and declined replies older than `REQUESTS_KEPT`.
fn forget_old_requests(conn: &Connection, now: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM received_replies
         WHERE state IN ('request', 'declined') AND received_ms < ?1",
        params![now - REQUESTS_KEPT.as_millis() as i64],
    )?;
    Ok(())
}

/// A place in a thread after every other: the time, or one more than the
/// last place if the clock went back, so a reader that got as far as a reply
/// since deleted still gets those published after it.
fn next_thread_seq(conn: &Connection, now: i64) -> Result<i64> {
    let last: i64 = conn.query_row(
        "SELECT COALESCE(MAX(thread_seq), 0) FROM received_replies",
        [],
        |row| row.get(0),
    )?;
    Ok(now.max(last + 1))
}

fn reply_rows(
    conn: &Connection,
    rest: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<ReplyRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT id, post, actor, node, state, signed, profile, received_ms
         FROM received_replies {rest}"
    ))?;
    let rows = stmt.query_map(params, |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, i64>(7)?,
        ))
    })?;
    rows.map(|row| {
        let (id, post, actor, node, state, signed, profile, received_ms) = row?;
        Ok(ReplyRow {
            reply: ReceivedReply {
                id: id.parse()?,
                post: post.parse()?,
                actor: actor.parse()?,
                node: node.parse()?,
                signed: serde_json::from_str(&signed).context("corrupt stored reply")?,
                profile: serde_json::from_str(&profile).context("corrupt stored profile")?,
                received_ms,
            },
            held: Held::parse(&state)?,
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;
    use osp_proto::{Note, Payload};

    use super::*;

    fn reply(key: &SecretKey, post: ObjectId, text: &str) -> SignedObject {
        let note = Payload::Note(Note {
            actor: key.public(),
            content: text.into(),
            published: osp_proto::now_rfc3339(),
            attachment: vec![],
            in_reply_to: Some(post),
        });
        SignedObject::sign(key, &note).unwrap()
    }

    fn received(key: &SecretKey, post: ObjectId, text: &str, at_ms: i64) -> ReceivedReply {
        let signed = reply(key, post, text);
        ReceivedReply {
            id: signed.id(),
            post,
            actor: key.public(),
            node: SecretKey::generate().public(),
            profile: signed.clone(),
            signed,
            received_ms: at_ms,
        }
    }

    fn a_post() -> ObjectId {
        let key = SecretKey::generate();
        let note = Payload::Note(Note {
            actor: key.public(),
            content: "a post".into(),
            published: osp_proto::now_rfc3339(),
            attachment: vec![],
            in_reply_to: None,
        });
        SignedObject::sign(&key, &note).unwrap().id()
    }

    #[test]
    fn a_full_thread_sends_new_replies_to_requests_while_theres_room() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let post = a_post();
        let now = super::super::now_ms();
        for i in 0..MAX_THREAD {
            let r = received(&SecretKey::generate(), post, &format!("{i}"), now);
            assert_eq!(
                db.add_received_reply(&r, true).unwrap(),
                Some(Held::Published)
            );
        }
        // Hidden ones count too.
        let first = db.kept_thread(&post).unwrap()[0].reply.id;
        db.move_reply(&first, Held::Hidden, now).unwrap();
        let r = received(&SecretKey::generate(), post, "late", now);
        assert_eq!(
            db.add_received_reply(&r, true).unwrap(),
            Some(Held::Request)
        );
        // Published from the requests only once there's room.
        assert!(db.move_reply(&r.id, Held::Published, now).is_err());
        assert_eq!(
            db.move_reply(&first, Held::Published, now).unwrap(),
            Held::Hidden
        );
        // Another post's thread isn't touched.
        let other = received(&SecretKey::generate(), a_post(), "elsewhere", now);
        assert_eq!(
            db.add_received_reply(&other, true).unwrap(),
            Some(Held::Published)
        );
    }

    #[test]
    fn one_identity_sends_at_most_twenty_replies_to_a_post_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (jimmy, post) = (SecretKey::generate(), a_post());
        let now = super::super::now_ms();
        for i in 0..MAX_REPLIES_PER_DAY {
            let r = received(&jimmy, post, &format!("{i}"), now - DAY_MS + 1000);
            assert_eq!(
                db.add_received_reply(&r, true).unwrap(),
                Some(Held::Published)
            );
        }
        let r = received(&jimmy, post, "more", now);
        assert_eq!(db.add_received_reply(&r, true).unwrap(), None);
        let elsewhere = received(&jimmy, a_post(), "elsewhere", now);
        assert!(db.add_received_reply(&elsewhere, true).unwrap().is_some());
        // A day after the first ones, there's room again.
        let r = received(&jimmy, post, "next day", now + 2000);
        assert_eq!(
            db.add_received_reply(&r, true).unwrap(),
            Some(Held::Published)
        );
    }

    #[test]
    fn old_requests_go_and_published_replies_stay() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let post = a_post();
        let then = super::super::now_ms() - REQUESTS_KEPT.as_millis() as i64 - 1000;
        let waiting = received(&SecretKey::generate(), post, "waiting", then);
        let published = received(&SecretKey::generate(), post, "published", then);
        db.add_received_reply(&waiting, false).unwrap();
        db.add_received_reply(&published, true).unwrap();
        let now = super::super::now_ms();
        assert!(db.reply_requests(now).unwrap().is_empty());
        assert_eq!(db.received_reply_state(&waiting.id, now).unwrap(), None);
        assert_eq!(
            db.received_reply_state(&published.id, now).unwrap(),
            Some(Held::Published)
        );
    }

    #[test]
    fn a_reply_isnt_stored_as_a_post() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let obj = reply(&SecretKey::generate(), a_post(), "hi");
        let payload = obj.verify().unwrap();
        assert!(db.insert_object(&obj, &payload).is_err());
    }
}
