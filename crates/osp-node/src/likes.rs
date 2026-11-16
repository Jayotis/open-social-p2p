//! Likes (ADR 0038), on both sides.
//!
//! Jimmy likes Kim's post. His node signs a `Like` and keeps it, then sends
//! it to Kim's node over the social endpoint, as a reply is sent, trying
//! again on the same steps for up to 7 days until it's answered. Taking it
//! back sends his signed `Undo` the same way. Only his latest choice is
//! kept, so liking and unliking quickly sends just the last.
//!
//! Kim's node keeps each person's newest like of each of her posts, and
//! shows her who. Anyone reading the post's thread learns how many, not
//! who. A like publishes nothing on Kim's behalf, so it needs no admission;
//! it counts toward no standing, vouch, hosting or feed order (ADR 0038
//! §2), and nothing outside this module reads it.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use iroh::{EndpointAddr, EndpointId};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_proto::{
    Like, ObjectId, Payload, Person, SignedObject, Undo,
    rpc::LikeInfo,
    social::{SocialRequest, SocialResponse},
};
use tracing::{debug, info};

use crate::{
    db::{Choice, HeldLike, SentLike, now_ms},
    node::Core,
    replying::retry_after,
    social::BUSY,
    trust::plain,
};

/// New likes the creator's node takes from one node an hour.
pub(crate) const MAX_FROM_NODE_PER_HOUR: u32 = 60;
/// New likes it takes an hour, from everyone together.
pub(crate) const MAX_PER_HOUR: u32 = 2_000;
/// Likes it keeps, at most: past it, those on the oldest posts go first.
pub(crate) const MAX_KEPT: usize = 100_000;
/// No try starts later than this after the sending began.
const TRY_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Longest one try may take, over Tor's first connection too.
const TRY_TIMEOUT: Duration = Duration::from_secs(2 * 60);
/// Most tries at once.
const TRIES_AT_ONCE: usize = 8;

impl Core {
    // ---- on the liker's node ----

    /// Likes `post` as the owner, or with `liked` false takes the like back.
    /// Kept at once and sent in the background.
    pub(crate) fn like(self: &Arc<Self>, post: &ObjectId, liked: bool) -> Result<()> {
        let (root, note) = self.resolve_note(post)?;
        ensure!(!note.is_reply(), "that's a reply; like the post it answers");
        let me = self.signer.identity();
        ensure!(note.actor != me, "that's your own post");
        let kept = self.db.sent_like(&root)?;
        if kept.as_ref().map(|k| k.liked) == Some(liked) {
            return Ok(());
        }
        let published = osp_proto::now_rfc3339();
        let (like_id, signed) = match (&kept, liked) {
            (_, true) => {
                let signed = self.signer.sign(&Payload::Like(Like {
                    actor: me,
                    object: root,
                    published,
                }))?;
                (signed.id(), signed)
            }
            (Some(kept), false) => {
                let signed = self.signer.sign(&Payload::Undo(Undo {
                    actor: me,
                    object: kept.like_id,
                    published,
                }))?;
                (kept.like_id, signed)
            }
            (None, false) => return Ok(()),
        };
        let creator_node = self
            .creator_node(&note.actor)
            .with_context(|| format!("no node is known for {}", note.actor.fmt_short()))?;
        let now = now_ms();
        self.db.keep_sent_like(&SentLike {
            post: root,
            creator: note.actor,
            creator_node,
            like_id,
            sent_id: signed.id(),
            signed,
            liked,
            tries: 0,
            // If this node stops before the first try is done, the loop
            // takes it up.
            next_try_ms: Some(now + retry_after(1).as_millis() as i64),
            since_ms: now,
            error: None,
        })?;
        info!(post = %root.fmt_short(), liked, "liked a post");
        let core = self.clone();
        tokio::spawn(async move {
            if let Err(e) = core.try_sent_like(&root).await {
                debug!("couldn't send a like: {e:#}");
            }
        });
        Ok(())
    }

    /// Whether the owner likes `post`, as their node has it.
    pub(crate) fn likes_post(&self, post: &ObjectId) -> Result<bool> {
        Ok(self.db.sent_like(post)?.is_some_and(|l| l.liked))
    }

    /// Tries each like that's due, or with `all` every one still waiting.
    /// Returns how many were tried.
    pub(crate) async fn send_waiting_likes(self: &Arc<Self>, all: bool) -> Result<usize> {
        let due = self.db.sent_likes_due(now_ms(), all)?;
        let tried = due.len();
        let mut tries = stream::iter(due)
            .map(|post| {
                let core = self.clone();
                async move { core.try_sent_like(&post).await }
            })
            .buffered_unordered(TRIES_AT_ONCE);
        while let Some(result) = tries.next().await {
            if let Err(e) = result {
                debug!("sending a like failed: {e:#}");
            }
        }
        Ok(tried)
    }

    /// Sends what's kept for `post`, once, and notes the answer or when to
    /// try again. An answer to an older choice changes nothing.
    async fn try_sent_like(&self, post: &ObjectId) -> Result<()> {
        let Some(row) = self.db.sent_like(post)? else {
            return Ok(());
        };
        if row.next_try_ms.is_none() {
            return Ok(());
        }
        let profile = self.profile();
        let req = match row.liked {
            true => SocialRequest::Like {
                like: row.signed.clone(),
                profile,
            },
            false => SocialRequest::Unlike {
                undo: row.signed.clone(),
                profile,
            },
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
            debug!(post = %post.fmt_short(), tries, "a like's creator's node didn't answer: {why}");
            if next.is_none() {
                info!(post = %post.fmt_short(), "gave up sending a like after 7 days");
            }
            self.db.sent_like_unanswered(post, &row.sent_id, next, &why)
        };
        match answer {
            Ok(Ok(SocialResponse::Ok)) => {
                debug!(post = %post.fmt_short(), liked = row.liked, "a like's creator's node took it");
                self.db.sent_like_answered(post, &row.sent_id, None)
            }
            Ok(Ok(SocialResponse::Error { message })) if message == BUSY => unanswered(message),
            Ok(Ok(SocialResponse::Error { message })) => {
                info!(post = %post.fmt_short(), "a like's creator's node refused it: {message}");
                self.db
                    .sent_like_answered(post, &row.sent_id, Some(&message))
            }
            Ok(Ok(other)) => self.db.sent_like_answered(
                post,
                &row.sent_id,
                Some(&format!("unexpected answer {other:?}")),
            ),
            Ok(Err(e)) => unanswered(format!("{e:#}")),
            Err(_) => unanswered("no answer in time".to_string()),
        }
    }

    // ---- on the creator's node ----

    /// Takes `like`, from the node `remote`, with its liker's `profile`. One
    /// that fails a check is refused with an error; one past a limit gets
    /// [`BUSY`], to be sent again later. One from someone the owner blocked
    /// is dropped, answered as if kept, so nobody can tell they're blocked.
    pub(crate) fn received_like(
        &self,
        remote: EndpointId,
        like: &SignedObject,
        profile: &SignedObject,
    ) -> Result<SocialResponse> {
        let Payload::Like(l) = like.verify()? else {
            bail!("that isn't a like");
        };
        self.check_replier(remote, l.actor, profile)?;
        let me = self.signer.identity();
        ensure!(l.actor != me, "a like of one's own post");
        let published = osp_proto::parse_rfc3339_ms(&l.published).context("a like's bad date")?;
        if self.db.delete_of(&l.object, &me)?.is_some() {
            return Ok(SocialResponse::Ok);
        }
        match self.db.outbox_object(&l.object)?.map(|o| o.verify()) {
            Some(Ok(Payload::Note(n))) if n.actor == me && !n.is_reply() => {}
            _ => bail!("{} isn't one of this node's posts", l.object.fmt_short()),
        }
        let held = self.db.held_like(&l.object, &l.actor)?;
        let now = now_ms();
        let published = published.min(now);
        // Sent before, or older than the one held: nothing changes, and no
        // limit counts it again.
        if held
            .as_ref()
            .is_some_and(|h| h.id == like.id() || h.published_ms >= published)
        {
            return Ok(SocialResponse::Ok);
        }
        if self.db.replier_choice(&l.actor)? == Some(Choice::Blocked) {
            debug!(liker = %l.actor.fmt_short(), "dropped a like from someone blocked");
            return Ok(SocialResponse::Ok);
        }
        if !self.like_senders.allow(remote, MAX_FROM_NODE_PER_HOUR)
            || !self.like_senders.allow(self.endpoint.id(), MAX_PER_HOUR)
        {
            debug!(node = %remote.fmt_short(), "too many new likes this hour");
            return Ok(SocialResponse::Error {
                message: BUSY.to_string(),
            });
        }
        // To be kept: checked against the keys this node knows of now, as an
        // object it keeps (ADR 0010).
        self.keys_current(&[profile, like])?;
        self.db.keep_like(
            &HeldLike {
                post: l.object,
                actor: l.actor,
                id: like.id(),
                node: remote,
                signed: like.clone(),
                profile: profile.clone(),
                published_ms: published,
                received_ms: now,
            },
            MAX_KEPT,
        )?;
        // A new liker moves the post's likes up the owner's Activity; a
        // newer like by the same person doesn't.
        if held.is_none() {
            self.db
                .note_activity(&format!("liked:{}", l.object), true, false, now)?;
        }
        info!(liker = %l.actor.fmt_short(), post = %l.object.fmt_short(), "received a like");
        Ok(SocialResponse::Ok)
    }

    /// Takes a liker's `Undo` of their like, from `remote`, with their
    /// `profile`. `Ok` whether the like was held or not.
    pub(crate) fn received_unlike(
        &self,
        remote: EndpointId,
        undo: &SignedObject,
        profile: &SignedObject,
    ) -> Result<SocialResponse> {
        let Payload::Undo(u) = undo.verify()? else {
            bail!("that isn't an undo");
        };
        self.check_replier(remote, u.actor, profile)?;
        if self.db.drop_like(&u.object, &u.actor)? {
            info!(liker = %u.actor.fmt_short(), "a liker took their like back");
        }
        Ok(SocialResponse::Ok)
    }

    /// Who likes one of the owner's own posts, newest first.
    pub(crate) fn who_likes(&self, post: &ObjectId) -> Result<Vec<LikeInfo>> {
        let (root, note) = self.resolve_note(post)?;
        ensure!(
            note.actor == self.signer.identity(),
            "only its creator's node knows who likes a post"
        );
        let likes = self.db.likes_of(&root)?;
        Ok(likes
            .into_iter()
            .map(|l| {
                let name = match l.profile.verify() {
                    Ok(Payload::Person(Person { name, .. })) => plain(&name),
                    _ => String::new(),
                };
                LikeInfo {
                    actor: l.actor,
                    name,
                    published: osp_proto::rfc3339_from_ms(l.published_ms.max(0) as u64),
                }
            })
            .collect())
    }
}
