//! Replies to this node's posts (ADR 0028), on the creator's side: the checks
//! on a reply, who it lets in, the owner's requests, and hiding and blocking.
//!
//! Kim posts. Alice is someone Kim follows; Jimmy is a stranger. Alice's
//! reply is published at once. Jimmy's waits among Kim's requests, under the
//! post's `network` policy, and is refused under `circle`. Following Kim gets
//! Jimmy nothing.

use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use iroh::{EndpointId, PublicKey};
use osp_proto::{
    ObjectId, Payload, Person, SignedObject,
    rpc::{ClientResponse, ReplyAnswer, ReplyInfo, ReplyStatus},
    social::{ReplyPolicy, ReplyState},
};
use tracing::{debug, info};

use crate::{
    db::{Choice, Held, ReceivedReply, ReplyRow, now_ms},
    node::Core,
    social::profile_date,
    trust::plain,
};

/// Published and hidden replies one post keeps, at most. Past it, new ones go
/// to requests while there's room.
pub(crate) const MAX_THREAD: usize = 2_000;
/// Requests waiting, from everyone together. When full, a new one is
/// refused: an old one isn't pushed out, or a flood would wipe out the real
/// ones.
pub(crate) const MAX_REQUESTS: usize = 100;
/// Requests waiting from one identity.
pub(crate) const MAX_REQUESTS_FROM_ONE: usize = 3;
/// New replies one node may send an hour, whoever they're from.
pub(crate) const MAX_NEW_FROM_NODE_PER_HOUR: u32 = 20;
/// New replies from one identity to one post a day.
pub(crate) const MAX_REPLIES_PER_DAY: usize = 20;
/// A request not answered in this long goes, as does a declined one.
pub(crate) const REQUESTS_KEPT: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Why a replier is let in without the owner looking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Circle {
    /// This node's own identity.
    Owner,
    /// Someone the owner follows.
    Followed,
    /// One of the owner's trusted followers (ADR 0013).
    Trusted,
    /// Someone the owner allowed to reply.
    Allowed,
}

impl Core {
    /// Takes `reply`, from the node `remote`, with its replier's `profile`.
    /// A reply that fails a check is refused with an error. One past a limit,
    /// from someone blocked, or refused by the post's policy gets
    /// `NotPublished`, the same for all, so nobody can tell they're blocked.
    pub(crate) fn submitted_reply(
        &self,
        remote: EndpointId,
        reply: &SignedObject,
        profile: &SignedObject,
    ) -> Result<ReplyState> {
        let Payload::Note(note) = reply.verify()? else {
            bail!("that isn't a reply");
        };
        note.check_reply()?;
        self.check_replier(remote, note.actor, profile)?;
        let id = reply.id();
        let now = now_ms();
        // Sent before: its state now, and no limit counts it again.
        if self.db.reply_deleted(&id)? {
            return Ok(ReplyState::Deleted);
        }
        if let Some(held) = self.db.received_reply_state(&id, now)? {
            return Ok(state_of(held));
        }
        let root = note.in_reply_to.context("that isn't a reply")?;
        let me = self.signer.identity();
        let Some(post) = self.db.outbox_object(&root)? else {
            if self.db.delete_of(&root, &me)?.is_some() {
                return Ok(ReplyState::Deleted);
            }
            bail!("{} isn't one of this node's posts", root.fmt_short());
        };
        match post.verify()? {
            Payload::Note(n) if n.actor == me && !n.is_reply() => {}
            Payload::Announce(_) => bail!("that answers a reshare; answer the post it shares"),
            _ => bail!("{} isn't one of this node's posts", root.fmt_short()),
        }
        if !self.reply_senders.allow(remote, MAX_NEW_FROM_NODE_PER_HOUR) {
            debug!(node = %remote.fmt_short(), "too many new replies from one node this hour");
            return Ok(ReplyState::NotPublished);
        }
        let policy = self.reply_policy(&root)?;
        if policy == ReplyPolicy::Off {
            return Ok(ReplyState::Closed);
        }
        let circle = match self.db.replier_choice(&note.actor)? {
            Some(Choice::Blocked) => return Ok(ReplyState::NotPublished),
            _ => self.in_circle(&note.actor)?,
        };
        if circle.is_none() && policy == ReplyPolicy::Circle {
            return Ok(ReplyState::NotPublished);
        }
        // To be kept: checked against the keys this node knows of now, as an
        // object it keeps (ADR 0010).
        self.keys_current(&[profile, reply])?;
        let received = ReceivedReply {
            id,
            post: root,
            actor: note.actor,
            node: remote,
            signed: reply.clone(),
            profile: profile.clone(),
            received_ms: now,
        };
        let held = self.db.add_received_reply(&received, circle.is_some())?;
        info!(
            replier = %note.actor.fmt_short(),
            post = %root.fmt_short(),
            ?circle,
            ?held,
            "received a reply"
        );
        Ok(held.map_or(ReplyState::NotPublished, state_of))
    }

    /// Takes a replier's `Delete` of their reply, from `remote`, with their
    /// `profile`. The reply goes, and the `Delete` is kept so an old copy
    /// can't come back. `Deleted` also for a reply that isn't here.
    pub(crate) fn deleted_reply(
        &self,
        remote: EndpointId,
        delete: &SignedObject,
        profile: &SignedObject,
    ) -> Result<ReplyState> {
        let Payload::Delete(d) = delete.verify()? else {
            bail!("that isn't a delete");
        };
        self.check_replier(remote, d.actor, profile)?;
        if self.db.reply_deleted(&d.object)? {
            return Ok(ReplyState::Deleted);
        }
        let held = self
            .db
            .received_reply(&d.object)?
            .is_some_and(|row| row.reply.actor == d.actor);
        if held {
            self.keys_current(&[profile, delete])?;
            if self
                .db
                .delete_received_reply(&d.object, &d.actor, delete, now_ms())?
            {
                info!(replier = %d.actor.fmt_short(), "a replier deleted their reply");
            }
        }
        Ok(ReplyState::Deleted)
    }

    /// Checks that `profile` is `actor`'s, is dated sensibly and lists `remote`.
    fn check_replier(
        &self,
        remote: EndpointId,
        actor: PublicKey,
        profile: &SignedObject,
    ) -> Result<()> {
        ensure!(profile.signer() == actor, "the profile isn't the replier's");
        let Payload::Person(person) = profile.verify()? else {
            bail!("that isn't a profile");
        };
        profile_date(&person)?;
        ensure!(
            person.nodes.contains(&remote),
            "the profile doesn't list your node"
        );
        Ok(())
    }

    /// Checks that no key their signers revoked signed `objects`, learning of
    /// newer key certificates from them (ADR 0010). Only for objects this
    /// node keeps, so strangers can't fill the key table. All are checked
    /// again at the end, as a later one's certificate may revoke the key that
    /// signed an earlier one.
    pub(crate) fn keys_current(&self, objects: &[&SignedObject]) -> Result<()> {
        for obj in objects {
            self.verify_received(obj)?;
        }
        for obj in objects {
            ensure!(
                !self.db.signed_by_revoked_key(obj)?,
                "signed with a key that {} has revoked",
                obj.signer().fmt_short()
            );
        }
        Ok(())
    }

    /// Why `actor` is in the owner's circle, if they are.
    pub(crate) fn in_circle(&self, actor: &PublicKey) -> Result<Option<Circle>> {
        if *actor == self.signer.identity() {
            return Ok(Some(Circle::Owner));
        }
        if self.db.follows()?.iter().any(|f| f.actor == *actor) {
            return Ok(Some(Circle::Followed));
        }
        if self.db.trusted()?.iter().any(|t| t.actor == *actor) {
            return Ok(Some(Circle::Trusted));
        }
        if self.db.replier_choice(actor)? == Some(Choice::Allowed) {
            return Ok(Some(Circle::Allowed));
        }
        Ok(None)
    }

    /// The policy of `post`: the one the owner set, or the default.
    pub(crate) fn reply_policy(&self, post: &ObjectId) -> Result<ReplyPolicy> {
        Ok(self
            .db
            .reply_policy(post)?
            .unwrap_or(self.config.replies.default_policy))
    }

    /// Sets the policy of `post`, one of the owner's posts.
    pub(crate) fn set_reply_policy(&self, post: &ObjectId, policy: ReplyPolicy) -> Result<()> {
        let obj = self
            .db
            .outbox_object(post)?
            .with_context(|| format!("{} isn't one of your posts", post.fmt_short()))?;
        ensure!(
            matches!(obj.verify()?, Payload::Note(_)),
            "{} is a reshare; only your own posts take replies",
            post.fmt_short()
        );
        self.db.set_reply_policy(post, policy)?;
        info!(post = %post.fmt_short(), policy = policy.as_str(), "set who may reply");
        Ok(())
    }

    /// Answers the request `id`.
    pub(crate) fn answer_reply_request(&self, id: &ObjectId, answer: ReplyAnswer) -> Result<()> {
        let row = self
            .db
            .received_reply(id)?
            .filter(|row| row.held == Held::Request)
            .with_context(|| format!("no request {} is waiting", id.fmt_short()))?;
        let now = now_ms();
        match answer {
            ReplyAnswer::Publish => {
                self.db.move_reply(id, Held::Published, now)?;
            }
            ReplyAnswer::PublishAndAllow => {
                self.db.move_reply(id, Held::Published, now)?;
                self.db
                    .set_replier(&row.reply.actor, Some(Choice::Allowed), now)?;
            }
            ReplyAnswer::Decline => {
                self.db.move_reply(id, Held::Declined, now)?;
            }
            ReplyAnswer::Block => {
                self.db
                    .set_replier(&row.reply.actor, Some(Choice::Blocked), now)?;
            }
        }
        info!(reply = %id.fmt_short(), ?answer, "answered a reply request");
        Ok(())
    }

    /// Hides a published reply from its thread, or shows a hidden one again.
    /// Never a `Delete`: the owner doesn't sign one for someone else's words.
    pub(crate) fn hide_reply(&self, id: &ObjectId, hidden: bool) -> Result<()> {
        let row = self
            .db
            .received_reply(id)?
            .with_context(|| format!("no reply {} is kept here", id.fmt_short()))?;
        let (from, to) = match hidden {
            true => (Held::Published, Held::Hidden),
            false => (Held::Hidden, Held::Published),
        };
        ensure!(
            row.held == from,
            "reply {} isn't {}",
            id.fmt_short(),
            match hidden {
                true => "published",
                false => "hidden",
            }
        );
        self.db.move_reply(id, to, now_ms())?;
        Ok(())
    }

    /// Allows `actor` to reply from now on, blocks them, or with `None`
    /// forgets either.
    pub(crate) fn set_replier(&self, actor: &PublicKey, choice: Option<Choice>) -> Result<()> {
        ensure!(
            *actor != self.signer.identity(),
            "that's you; you can always reply to your own posts"
        );
        self.db.set_replier(actor, choice, now_ms())
    }
}

impl Core {
    /// The replies kept to `post`, one of the owner's, in thread order:
    /// published and hidden ones, and its policy.
    pub(crate) fn own_thread(&self, post: &ObjectId) -> Result<ClientResponse> {
        let obj = self
            .db
            .outbox_object(post)?
            .with_context(|| format!("{} isn't one of your posts", post.fmt_short()))?;
        ensure!(
            matches!(obj.verify()?, Payload::Note(_)),
            "{} is a reshare; replies are to the post it shares",
            post.fmt_short()
        );
        let replies = self
            .db
            .kept_thread(post)?
            .iter()
            .map(|row| reply_info(row, None))
            .collect::<Result<_>>()?;
        Ok(ClientResponse::Thread {
            policy: self.reply_policy(post)?,
            replies,
        })
    }

    /// Requests waiting, newest first, each with why it's there.
    pub(crate) fn reply_request_list(&self) -> Result<Vec<ReplyInfo>> {
        let mut requests = Vec::new();
        for row in self.db.reply_requests(now_ms())? {
            let why = match self.in_circle(&row.reply.actor)? {
                Some(_) => "the thread was full".to_string(),
                None if self.db.is_follower(&row.reply.node)? => {
                    "you don't follow them; they follow you".to_string()
                }
                None => "you don't follow them".to_string(),
            };
            requests.push(reply_info(&row, Some(why))?);
        }
        Ok(requests)
    }
}

fn reply_info(row: &ReplyRow, why: Option<String>) -> Result<ReplyInfo> {
    let Payload::Note(note) = row.reply.signed.verify()? else {
        bail!("a stored reply isn't a note");
    };
    let name = match row.reply.profile.verify() {
        Ok(Payload::Person(Person { name, .. })) => plain(&name),
        _ => String::new(),
    };
    Ok(ReplyInfo {
        id: row.reply.id,
        post: row.reply.post,
        actor: row.reply.actor,
        name,
        content: note.content,
        published: note.published,
        state: match row.held {
            Held::Published => ReplyStatus::Published,
            Held::Hidden => ReplyStatus::Hidden,
            Held::Request | Held::Declined => ReplyStatus::Request,
        },
        why,
    })
}

/// What a replier is told of a reply kept here.
fn state_of(held: Held) -> ReplyState {
    match held {
        Held::Published => ReplyState::Published,
        Held::Request => ReplyState::AwaitingApproval,
        Held::Hidden | Held::Declined => ReplyState::NotPublished,
    }
}
