//! Replying to someone else's post (ADR 0028), on the replier's side.
//!
//! Jimmy replies to Kim's post. His node signs the reply and keeps it, then
//! sends it to Kim's node over the ordinary social endpoint (`SubmitReply`).
//! If Kim's node can't be reached, or doesn't answer, it's tried again after
//! 1 minute, 5, 30, 2 hours, 12 hours, then daily, for 30 days; `osp sync`
//! tries at once, and the tries go on after a restart. Any answer ends them,
//! a refusal too. The reply's ID keeps a retry from making a second copy.
//!
//! When Jimmy deletes it, his words go from his node at once, and his signed
//! `Delete` is sent the same way (`DeleteReply`). Kim replying to her own
//! post needs no network: her node takes it as any reply.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use iroh::{EndpointAddr, EndpointId, PublicKey};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_proto::{
    Delete, Note, ObjectId, Payload,
    rpc::{SentReplyInfo, SentStatus},
    social::{ReplyState, SocialRequest, SocialResponse},
};
use tracing::{debug, info, warn};

use crate::{
    db::{SentReply, now_ms},
    node::Core,
    social::BUSY,
};

/// How long after each failed try the next comes, for the first ones. Then
/// `RETRY_DAILY`.
const RETRY_AFTER: [Duration; 5] = [
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(30 * 60),
    Duration::from_secs(2 * 60 * 60),
    Duration::from_secs(12 * 60 * 60),
];
const RETRY_DAILY: Duration = Duration::from_secs(24 * 60 * 60);
/// No try starts later than this after the sending began.
pub(crate) const TRY_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Longest one try may take, over Tor's first connection too.
const TRY_TIMEOUT: Duration = Duration::from_secs(2 * 60);
/// How long a device waits for the first try's answer before it's told the
/// reply is being sent.
const DEVICE_WAIT: Duration = Duration::from_secs(10);
/// Most tries at once.
const TRIES_AT_ONCE: usize = 8;
/// Longest the loop sleeps, so a new reply's retry is never late by more.
const LOOK_EVERY: Duration = Duration::from_secs(60);

/// How long to wait after the `tries`th try failed.
fn retry_after(tries: u32) -> Duration {
    RETRY_AFTER
        .get(tries.saturating_sub(1) as usize)
        .copied()
        .unwrap_or(RETRY_DAILY)
}

/// How long a client waits for a reply's first try: on the node itself, for
/// the answer; a device, at most `DEVICE_WAIT`.
pub(crate) fn reply_wait(local: bool) -> Option<Duration> {
    (!local).then_some(DEVICE_WAIT)
}

fn status_of(state: ReplyState) -> SentStatus {
    match state {
        ReplyState::Published => SentStatus::Published,
        ReplyState::AwaitingApproval => SentStatus::AwaitingApproval,
        ReplyState::NotPublished => SentStatus::NotPublished,
        ReplyState::Closed => SentStatus::Closed,
        ReplyState::Deleted => SentStatus::Deleted,
    }
}

impl Core {
    /// Replies to `post` with `content` as the owner. Waits for the first
    /// try's answer, or with `wait` for at most that long; the tries go on
    /// in the background either way.
    pub(crate) async fn reply(
        self: &Arc<Self>,
        post: &ObjectId,
        content: String,
        wait: Option<Duration>,
    ) -> Result<SentReplyInfo> {
        let (root, note) = self.resolve_note(post)?;
        ensure!(
            !note.is_reply(),
            "that's a reply; answer the post it answers"
        );
        let me = self.signer.identity();
        let reply = Note {
            actor: me,
            content,
            published: osp_proto::now_rfc3339(),
            attachment: vec![],
            in_reply_to: Some(root),
        };
        reply.check_reply()?;
        let signed = self.signer.sign(&Payload::Note(reply))?;
        let id = signed.id();
        if note.actor == me {
            let state = self.submitted_reply(self.endpoint.id(), &signed, &self.profile)?;
            info!(post = %root.fmt_short(), "replied to an own post");
            return own_info(&signed, root, me, status_of(state));
        }
        let creator_node = self
            .creator_node(&note.actor)
            .with_context(|| format!("no node is known for {}", note.actor.fmt_short()))?;
        let now = now_ms();
        self.db.add_sent_reply(&SentReply {
            id,
            post: root,
            creator: note.actor,
            creator_node,
            signed: Some(signed),
            delete: None,
            status: SentStatus::Sending,
            error: None,
            tries: 0,
            // If this node stops before the first try is done, the loop
            // takes it up.
            next_try_ms: Some(now + RETRY_AFTER[0].as_millis() as i64),
            since_ms: now,
            sent_ms: now,
        })?;
        info!(post = %root.fmt_short(), creator = %note.actor.fmt_short(), "replied");
        let core = self.clone();
        let first = tokio::spawn(async move { core.try_sent_reply(&id).await });
        match wait {
            Some(wait) => {
                let _ = tokio::time::timeout(wait, first).await;
            }
            None => {
                let _ = first.await;
            }
        }
        let row = self.db.sent_reply(&id)?.context("the reply went")?;
        sent_info(&row, now_ms())
    }

    /// The node to send a reply to `creator` to: the first their newest
    /// profile held lists, or the one followed.
    fn creator_node(&self, creator: &PublicKey) -> Option<EndpointId> {
        if let Some(node) = self.nodes_of(creator).first() {
            return Some(*node);
        }
        let follows = self.db.follows().ok()?;
        follows
            .into_iter()
            .find(|f| f.actor == *creator)
            .map(|f| f.addr.id)
    }

    /// Deletes one of the owner's replies: on its creator's node by a
    /// `DeleteReply`, or here if it's on one of the owner's own posts.
    /// Returns whether `id` was a reply of theirs.
    pub(crate) async fn delete_reply(self: &Arc<Self>, id: &ObjectId) -> Result<bool> {
        let me = self.signer.identity();
        let sign_delete = || {
            self.signer.sign(&Payload::Delete(Delete {
                actor: me,
                object: *id,
                published: osp_proto::now_rfc3339(),
            }))
        };
        if let Some(row) = self.db.received_reply(id)?
            && row.reply.actor == me
        {
            self.deleted_reply(self.endpoint.id(), &sign_delete()?, &self.profile)?;
            info!(reply = %id.fmt_short(), "deleted a reply to an own post");
            return Ok(true);
        }
        let Some(row) = self.db.sent_reply(id)? else {
            return Ok(false);
        };
        ensure!(row.delete.is_none(), "that reply is being deleted already");
        // The creator's node never kept these: nothing to tell it.
        if matches!(row.status, SentStatus::Refused | SentStatus::Closed) {
            self.db.forget_sent_reply(id)?;
            return Ok(true);
        }
        self.db.sent_reply_deleting(id, &sign_delete()?, now_ms())?;
        info!(reply = %id.fmt_short(), "deleted a reply; telling its post's creator");
        let core = self.clone();
        let id = *id;
        tokio::spawn(async move {
            if let Err(e) = core.try_sent_reply(&id).await {
                debug!("couldn't send a reply's delete: {e:#}");
            }
        });
        Ok(true)
    }

    /// The owner's replies to others' posts, newest first.
    pub(crate) fn sent_reply_list(&self) -> Result<Vec<SentReplyInfo>> {
        let now = now_ms();
        self.db
            .sent_replies()?
            .iter()
            .map(|row| sent_info(row, now))
            .collect()
    }

    /// `creator`'s node was just reached, so the owner's replies to them
    /// waiting for a later try go now: a node that couldn't be reached for a
    /// while, say over Tor after its restart, needn't wait out the retry
    /// steps once it can be.
    pub(crate) fn retry_replies_to(self: &Arc<Self>, creator: &PublicKey) {
        match self.db.sent_replies_due_now_to(creator, now_ms()) {
            Ok(0) => {}
            Ok(n) => {
                debug!(creator = %creator.fmt_short(), replies = n, "reached a creator again; sending replies now");
                let core = self.clone();
                tokio::spawn(async move {
                    if let Err(e) = core.send_waiting_replies(false).await {
                        warn!("sending waiting replies failed: {e:#}");
                    }
                });
            }
            Err(e) => warn!("couldn't bring replies forward: {e:#}"),
        }
    }

    /// Tries each reply or `Delete` that's due, or with `all` every one still
    /// waiting. Returns how many were tried.
    pub(crate) async fn send_waiting_replies(self: &Arc<Self>, all: bool) -> Result<usize> {
        let now = now_ms();
        if all {
            self.db.sent_replies_due_now(now)?;
        }
        let due = self.db.sent_replies_due(now)?;
        let tried = due.len();
        let mut tries = stream::iter(due)
            .map(|row| {
                let core = self.clone();
                async move { core.try_sent_reply(&row.id).await }
            })
            .buffered_unordered(TRIES_AT_ONCE);
        while let Some(result) = tries.next().await {
            if let Err(e) = result {
                warn!("sending a reply failed: {e:#}");
            }
        }
        Ok(tried)
    }

    /// Sends the reply `id`, or its `Delete`, once, and notes the answer or
    /// when to try again.
    async fn try_sent_reply(&self, id: &ObjectId) -> Result<()> {
        let Some(row) = self.db.sent_reply(id)? else {
            return Ok(());
        };
        if row.next_try_ms.is_none() {
            return Ok(()); // Answered meanwhile.
        }
        let req = match (&row.delete, &row.signed) {
            (Some(delete), _) => SocialRequest::DeleteReply {
                delete: delete.clone(),
                profile: self.profile.clone(),
            },
            (None, Some(reply)) => SocialRequest::SubmitReply {
                reply: reply.clone(),
                profile: self.profile.clone(),
            },
            (None, None) => bail!("a kept reply {} has nothing to send", id.fmt_short()),
        };
        let node = self.creator_node(&row.creator).unwrap_or(row.creator_node);
        let answer = tokio::time::timeout(
            TRY_TIMEOUT,
            self.social_answer(&EndpointAddr::new(node), &req),
        )
        .await;
        let unanswered = |why: String| {
            let tries = row.tries + 1;
            let next = now_ms() + retry_after(tries).as_millis() as i64;
            let next = (next <= row.since_ms + TRY_FOR.as_millis() as i64).then_some(next);
            debug!(reply = %id.fmt_short(), tries, "a reply's creator's node didn't answer: {why}");
            if next.is_none() {
                info!(reply = %id.fmt_short(), "gave up sending a reply after 30 days");
            }
            self.db.sent_reply_unanswered(id, next, &why)
        };
        match answer {
            Ok(Ok(SocialResponse::Reply { state })) => {
                debug!(reply = %id.fmt_short(), ?state, "a reply's creator's node answered");
                self.db.sent_reply_answered(id, status_of(state), None)
            }
            Ok(Ok(SocialResponse::Error { message })) if message == BUSY => unanswered(message),
            Ok(Ok(SocialResponse::Error { message })) => {
                info!(reply = %id.fmt_short(), "a reply's creator's node refused it: {message}");
                self.db
                    .sent_reply_answered(id, SentStatus::Refused, Some(&message))
            }
            Ok(Ok(other)) => self.db.sent_reply_answered(
                id,
                SentStatus::Refused,
                Some(&format!("unexpected answer {other:?}")),
            ),
            Ok(Err(e)) => unanswered(format!("{e:#}")),
            Err(_) => unanswered("no answer in time".to_string()),
        }
    }
}

/// Tries replies again as they fall due, for as long as the node runs.
pub(crate) async fn send_loop(core: Arc<Core>) {
    loop {
        if let Some(tor) = &core.tor {
            tor.ready().await;
        }
        if let Err(e) = core.send_waiting_replies(false).await {
            warn!("sending waiting replies failed: {e:#}");
        }
        let wait = match core.db.next_sent_reply_try() {
            Ok(Some(next)) => Duration::from_millis((next - now_ms()).max(1000) as u64),
            _ => LOOK_EVERY,
        };
        tokio::time::sleep(wait.min(LOOK_EVERY)).await;
    }
}

fn sent_info(row: &SentReply, now: i64) -> Result<SentReplyInfo> {
    let (content, published) = match row.signed.as_ref().map(|s| s.verify()) {
        Some(Ok(Payload::Note(note))) => (note.content, note.published),
        Some(_) => bail!("a kept reply isn't a note"),
        None => (String::new(), String::new()),
    };
    Ok(SentReplyInfo {
        id: row.id,
        post: row.post,
        creator: row.creator,
        content,
        published,
        status: row.status,
        error: row.error.clone(),
        next_try_secs: row
            .next_try_ms
            .map(|next| ((next - now).max(0) / 1000) as u64),
    })
}

/// A reply to one of the owner's own posts, taken here.
fn own_info(
    signed: &osp_proto::SignedObject,
    post: ObjectId,
    me: PublicKey,
    status: SentStatus,
) -> Result<SentReplyInfo> {
    let Payload::Note(note) = signed.verify()? else {
        bail!("not a note");
    };
    Ok(SentReplyInfo {
        id: signed.id(),
        post,
        creator: me,
        content: note.content,
        published: note.published,
        status,
        error: None,
        next_try_secs: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tries_come_after_a_minute_then_longer_then_daily() {
        let mins: Vec<u64> = (1..=8).map(|t| retry_after(t).as_secs() / 60).collect();
        assert_eq!(mins, [1, 5, 30, 120, 720, 1440, 1440, 1440]);
        // 30 days of them: 5 at first, then one a day.
        let mut at = Duration::ZERO;
        let mut tries = 1;
        while at + retry_after(tries) <= TRY_FOR {
            at += retry_after(tries);
            tries += 1;
        }
        assert_eq!(tries, 35);
    }
}
