//! The owner's replies to others' posts (ADR 0028), on the replier's side:
//! each kept from when it's signed until the creator's node answers, with its
//! tries, and then where it stands. Deleting one keeps only its `Delete`,
//! until that's sent too.

use anyhow::{Context, Result, bail};
use iroh::{EndpointId, PublicKey};
use osp_proto::{ObjectId, SignedObject, rpc::SentStatus};
use rusqlite::{OptionalExtension, params};

use super::Db;

/// One of the owner's replies, as it's kept.
#[derive(Debug, Clone)]
pub(crate) struct SentReply {
    pub id: ObjectId,
    pub post: ObjectId,
    pub creator: PublicKey,
    /// The node to send to, when no newer profile of the creator names one.
    pub creator_node: EndpointId,
    /// The signed reply; none once the owner deleted it.
    pub signed: Option<SignedObject>,
    /// The owner's `Delete` of it, while that's being sent.
    pub delete: Option<SignedObject>,
    pub status: SentStatus,
    pub error: Option<String>,
    pub tries: u32,
    /// None once there's nothing left to send.
    pub next_try_ms: Option<i64>,
    /// When this sending began, the reply's or its `Delete`'s.
    pub since_ms: i64,
    pub sent_ms: i64,
}

fn status_str(status: SentStatus) -> &'static str {
    match status {
        SentStatus::Sending => "sending",
        SentStatus::Published => "published",
        SentStatus::AwaitingApproval => "awaiting_approval",
        SentStatus::NotPublished => "not_published",
        SentStatus::Closed => "closed",
        SentStatus::Deleted => "deleted",
        SentStatus::Refused => "refused",
        SentStatus::GaveUp => "gave_up",
        SentStatus::Deleting => "deleting",
    }
}

fn parse_status(s: &str) -> Result<SentStatus> {
    Ok(match s {
        "sending" => SentStatus::Sending,
        "published" => SentStatus::Published,
        "awaiting_approval" => SentStatus::AwaitingApproval,
        "not_published" => SentStatus::NotPublished,
        "closed" => SentStatus::Closed,
        "deleted" => SentStatus::Deleted,
        "refused" => SentStatus::Refused,
        "gave_up" => SentStatus::GaveUp,
        "deleting" => SentStatus::Deleting,
        _ => bail!("unknown sent reply state {s:?}"),
    })
}

const COLUMNS: &str = "id, post, creator, creator_node, signed, delete_signed, state, error,
     tries, next_try_ms, since_ms, sent_ms";

impl Db {
    /// Keeps a reply just signed, to be sent at once.
    pub fn add_sent_reply(&self, reply: &SentReply) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO sent_replies ({COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
            ),
            params![
                reply.id.to_string(),
                reply.post.to_string(),
                reply.creator.to_string(),
                reply.creator_node.to_string(),
                reply
                    .signed
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                reply
                    .delete
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                status_str(reply.status),
                reply.error,
                reply.tries,
                reply.next_try_ms,
                reply.since_ms,
                reply.sent_ms,
            ],
        )?;
        Ok(())
    }

    pub fn sent_reply(&self, id: &ObjectId) -> Result<Option<SentReply>> {
        let conn = self.conn();
        let mut rows = sent_rows(&conn, "WHERE id = ?1", params![id.to_string()])?;
        Ok(rows.pop())
    }

    /// All of them, newest first.
    pub fn sent_replies(&self) -> Result<Vec<SentReply>> {
        sent_rows(&self.conn(), "ORDER BY sent_ms DESC, id", [])
    }

    /// Those due a try by `now`, oldest due first.
    pub fn sent_replies_due(&self, now: i64) -> Result<Vec<SentReply>> {
        sent_rows(
            &self.conn(),
            "WHERE next_try_ms IS NOT NULL AND next_try_ms <= ?1 ORDER BY next_try_ms",
            params![now],
        )
    }

    /// When the next try of any is due, if one is waiting.
    pub fn next_sent_reply_try(&self) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row("SELECT MIN(next_try_ms) FROM sent_replies", [], |row| {
                row.get(0)
            })?)
    }

    /// Makes every one waiting due `now`, as `osp sync` tries at once.
    pub fn sent_replies_due_now(&self, now: i64) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE sent_replies SET next_try_ms = ?1
             WHERE next_try_ms IS NOT NULL AND next_try_ms > ?1",
            params![now],
        )?)
    }

    /// The creator's node answered: nothing more to send. A `Delete` that
    /// was answered `Deleted` leaves nothing to keep.
    pub fn sent_reply_answered(
        &self,
        id: &ObjectId,
        status: SentStatus,
        error: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn();
        let deleting: Option<bool> = conn
            .query_row(
                "SELECT delete_signed IS NOT NULL FROM sent_replies WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        match deleting {
            None => {}
            Some(true) if status == SentStatus::Deleted => {
                conn.execute(
                    "DELETE FROM sent_replies WHERE id = ?1",
                    params![id.to_string()],
                )?;
            }
            Some(_) => {
                conn.execute(
                    "UPDATE sent_replies
                     SET state = ?2, error = ?3, next_try_ms = NULL, tries = tries + 1
                     WHERE id = ?1",
                    params![id.to_string(), status_str(status), error],
                )?;
            }
        }
        Ok(())
    }

    /// A try got no answer: tried again at `next`, or given up if `None`.
    pub fn sent_reply_unanswered(
        &self,
        id: &ObjectId,
        next: Option<i64>,
        error: &str,
    ) -> Result<()> {
        let conn = self.conn();
        let id = id.to_string();
        match next {
            Some(next) => conn.execute(
                "UPDATE sent_replies SET tries = tries + 1, next_try_ms = ?2, error = ?3
                 WHERE id = ?1",
                params![id, next, error],
            )?,
            // A `Delete` given up on leaves nothing worth keeping: the reply
            // itself is gone here already.
            None => match conn.execute(
                "DELETE FROM sent_replies WHERE id = ?1 AND delete_signed IS NOT NULL",
                params![id],
            )? {
                0 => conn.execute(
                    "UPDATE sent_replies
                     SET state = 'gave_up', tries = tries + 1, next_try_ms = NULL, error = ?2
                     WHERE id = ?1",
                    params![id, error],
                )?,
                n => n,
            },
        };
        Ok(())
    }

    /// The owner deleted the reply: its words go, and its `Delete` is sent
    /// from `now`, with tries of its own.
    pub fn sent_reply_deleting(
        &self,
        id: &ObjectId,
        delete: &SignedObject,
        now: i64,
    ) -> Result<()> {
        let changed = self.conn().execute(
            "UPDATE sent_replies
             SET signed = NULL, delete_signed = ?2, state = 'deleting', error = NULL,
                 tries = 0, next_try_ms = ?3, since_ms = ?3
             WHERE id = ?1",
            params![id.to_string(), serde_json::to_string(delete)?, now],
        )?;
        anyhow::ensure!(
            changed == 1,
            "no reply {} of yours is kept here",
            id.fmt_short()
        );
        Ok(())
    }

    pub fn forget_sent_reply(&self, id: &ObjectId) -> Result<()> {
        self.conn().execute(
            "DELETE FROM sent_replies WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    /// The reply was seen in its thread: it's published, whatever the
    /// creator's node said before. Not one being deleted.
    pub fn sent_reply_seen_published(&self, id: &ObjectId) -> Result<()> {
        self.conn().execute(
            "UPDATE sent_replies SET state = 'published', error = NULL, next_try_ms = NULL
             WHERE id = ?1 AND delete_signed IS NULL AND state != 'published'",
            params![id.to_string()],
        )?;
        Ok(())
    }
}

fn sent_rows(
    conn: &rusqlite::Connection,
    rest: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<SentReply>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM sent_replies {rest}"))?;
    let rows = stmt.query_map(params, |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, u32>(8)?,
            row.get::<_, Option<i64>>(9)?,
            row.get::<_, i64>(10)?,
            row.get::<_, i64>(11)?,
        ))
    })?;
    rows.map(|row| {
        let (id, post, creator, node, signed, delete, state, error, tries, next, since, sent) =
            row?;
        let object = |s: Option<String>| -> Result<Option<SignedObject>> {
            s.map(|s| serde_json::from_str(&s).context("corrupt stored reply"))
                .transpose()
        };
        Ok(SentReply {
            id: id.parse()?,
            post: post.parse()?,
            creator: creator.parse()?,
            creator_node: node.parse()?,
            signed: object(signed)?,
            delete: object(delete)?,
            status: parse_status(&state)?,
            error,
            tries,
            next_try_ms: next,
            since_ms: since,
            sent_ms: sent,
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;
    use osp_proto::{Delete, Note, Payload};

    use super::*;

    fn signed(key: &SecretKey, payload: Payload) -> SignedObject {
        SignedObject::sign(key, &payload).unwrap()
    }

    fn kept(db: &Db, key: &SecretKey, now: i64) -> SentReply {
        let post = signed(
            key,
            Payload::Note(Note {
                actor: key.public(),
                content: "a post".into(),
                published: osp_proto::now_rfc3339(),
                attachment: vec![],
                in_reply_to: None,
            }),
        )
        .id();
        let reply = signed(
            key,
            Payload::Note(Note {
                actor: key.public(),
                // Its own words, so two made in one millisecond differ.
                content: format!("a reply {}", SecretKey::generate().public()),
                published: osp_proto::now_rfc3339(),
                attachment: vec![],
                in_reply_to: Some(post),
            }),
        );
        let row = SentReply {
            id: reply.id(),
            post,
            creator: SecretKey::generate().public(),
            creator_node: SecretKey::generate().public(),
            signed: Some(reply),
            delete: None,
            status: SentStatus::Sending,
            error: None,
            tries: 0,
            next_try_ms: Some(now + 60_000),
            since_ms: now,
            sent_ms: now,
        };
        db.add_sent_reply(&row).unwrap();
        row
    }

    #[test]
    fn a_reply_waits_its_turn_and_one_never_answered_is_given_up() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let key = SecretKey::generate();
        let now = 1_000_000;
        let row = kept(&db, &key, now);
        assert!(db.sent_replies_due(now).unwrap().is_empty());
        assert_eq!(db.next_sent_reply_try().unwrap(), Some(now + 60_000));
        // `osp sync` makes it due at once.
        db.sent_replies_due_now(now).unwrap();
        assert_eq!(db.sent_replies_due(now).unwrap().len(), 1);

        db.sent_reply_unanswered(&row.id, Some(now + 300_000), "off")
            .unwrap();
        let got = db.sent_reply(&row.id).unwrap().unwrap();
        assert_eq!((got.tries, got.next_try_ms), (1, Some(now + 300_000)));
        assert_eq!(got.status, SentStatus::Sending);

        db.sent_reply_unanswered(&row.id, None, "off").unwrap();
        let got = db.sent_reply(&row.id).unwrap().unwrap();
        assert_eq!((got.status, got.next_try_ms), (SentStatus::GaveUp, None));
        assert_eq!(db.next_sent_reply_try().unwrap(), None);
    }

    #[test]
    fn a_deleted_reply_keeps_only_its_delete_until_thats_answered() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let key = SecretKey::generate();
        let now = 1_000_000;
        let row = kept(&db, &key, now);
        db.sent_reply_answered(&row.id, SentStatus::AwaitingApproval, None)
            .unwrap();
        let delete = signed(
            &key,
            Payload::Delete(Delete {
                actor: key.public(),
                object: row.id,
                published: osp_proto::now_rfc3339(),
            }),
        );
        db.sent_reply_deleting(&row.id, &delete, now + 5).unwrap();
        let got = db.sent_reply(&row.id).unwrap().unwrap();
        assert!(got.signed.is_none());
        assert_eq!(got.status, SentStatus::Deleting);
        assert_eq!((got.tries, got.next_try_ms), (0, Some(now + 5)));
        // Seeing it in a thread doesn't bring it back.
        db.sent_reply_seen_published(&row.id).unwrap();
        assert_eq!(
            db.sent_reply(&row.id).unwrap().unwrap().status,
            SentStatus::Deleting
        );
        db.sent_reply_answered(&row.id, SentStatus::Deleted, None)
            .unwrap();
        assert!(db.sent_reply(&row.id).unwrap().is_none());

        // One whose `Delete` is never answered goes too.
        let other = kept(&db, &key, now);
        db.sent_reply_deleting(&other.id, &delete, now).unwrap();
        db.sent_reply_unanswered(&other.id, None, "off").unwrap();
        assert!(db.sent_replies().unwrap().is_empty());
    }
}
