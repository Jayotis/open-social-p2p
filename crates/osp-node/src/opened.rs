//! Opening a post from an OSP code (ADR 0039 §4), on the owner's node.
//!
//! Jimmy sends Kim a code for Alice's post. If Kim's node holds it, it opens
//! as any post. If not, and Kim doesn't follow Alice, her node asks the node
//! the code names for the post and Alice's profile, from a throwaway key
//! (over Tor, or relay-only without it, as `connected` asks), so Alice's
//! node can't tell it was Kim. It keeps them as an opened post
//! (`db::opened`): out of every list, for 7 days from when Kim last opened,
//! liked or replied to it, within `MAX_OPENED` and `MAX_OPENED_BYTES`. Its
//! media comes as a found post's does (ADR 0024), never from Kim's identity,
//! and is never served to anyone but her devices.
//!
//! Opening it again asks Alice's node again, after answering, so a post she
//! deleted goes (ADR 0019).

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use iroh::EndpointAddr;
use osp_proto::{
    FollowTicket, ObjectId, Payload, Person, SOCIAL_ALPN, SignedObject,
    rpc::FeedItem,
    social::{SocialRequest, SocialResponse},
};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::{
    connected::{ASK_TIMEOUT, DIAL_AT_LEAST},
    db::now_ms,
    node::Core,
    social::{profile_date, request_on},
};

/// Most opened posts kept. A starting value.
pub(crate) const MAX_OPENED: usize = 100;

/// Most bytes of opened posts kept, their media as declared included. A
/// starting value, for a single-board computer's disk.
pub(crate) const MAX_OPENED_BYTES: u64 = 500 * 1024 * 1024;

/// Most posts the node fetches to open in an hour. A code is opened by a
/// tap; this only stops something on a device from making the node dial
/// on and on.
pub(crate) const MAX_OPENS_PER_HOUR: u32 = 30;

/// How often opened posts past their 7 days are dropped.
const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);

/// What the author's node said of a post.
enum Fetched {
    /// It's kept here now.
    Post,
    /// Its author deleted it; the delete was acted on here.
    Deleted,
}

impl Core {
    /// Opens `id` from an OSP code naming `ticket` (`ClientRequest::OpenPost`).
    pub(crate) async fn open_post(
        self: &Arc<Self>,
        id: &ObjectId,
        ticket: &FollowTicket,
    ) -> Result<FeedItem> {
        if self.db.get_object(id)?.is_some() {
            if self.db.touch_opened(id, now_ms())? {
                let (core, id, ticket) = (self.clone(), *id, ticket.clone());
                tokio::spawn(async move {
                    if let Err(e) = core.fetch_opened(&id, &ticket).await {
                        debug!(post = %id.fmt_short(), "asking again for an opened post: {e:#}");
                    }
                });
            }
            return self.show(id).await;
        }
        ensure!(
            ticket.actor != self.signer.identity(),
            "that post of yours isn't on your node any more"
        );
        ensure!(
            !self.db.is_following(&ticket.actor)?,
            "that post isn't on your node yet; it comes with the next sync with its author"
        );
        match self.fetch_opened(id, ticket).await? {
            Fetched::Deleted => bail!("its author deleted that post"),
            Fetched::Post => {}
        }
        self.trim_opened().await?;
        info!(post = %id.fmt_short(), author = %ticket.actor.fmt_short(), "opened a shared post");
        self.show(id).await
    }

    /// Drops an opened post and its media now (`ClientRequest::ForgetOpened`).
    pub(crate) async fn forget_opened(&self, id: &ObjectId) -> Result<()> {
        let gone = self
            .db
            .drop_opened(id)?
            .context("that isn't an opened post")?;
        self.store.let_go(&gone).await;
        Ok(())
    }

    /// Drops the opened posts past their 7 days, then the least lately
    /// opened beyond the caps.
    pub(crate) async fn trim_opened(&self) -> Result<usize> {
        let ids = self
            .db
            .opened_to_drop(now_ms(), MAX_OPENED, MAX_OPENED_BYTES)?;
        for id in &ids {
            if let Some(gone) = self.db.drop_opened(id)? {
                self.store.let_go(&gone).await;
            }
        }
        if !ids.is_empty() {
            debug!(dropped = ids.len(), "dropped opened posts");
        }
        Ok(ids.len())
    }

    /// Asks the node `ticket` names for `id` and its author's profile, from a
    /// throwaway key, and keeps what it answers: the post as opened, or its
    /// author's delete.
    async fn fetch_opened(&self, id: &ObjectId, ticket: &FollowTicket) -> Result<Fetched> {
        let deadline = Instant::now() + ASK_TIMEOUT;
        let working = match &self.tor {
            Some(tor) if !tor.dialled() => tor.wait_dialled(deadline - DIAL_AT_LEAST).await,
            _ => true,
        };
        ensure!(
            !working
                || self
                    .opened_asks
                    .allow(self.endpoint.id(), MAX_OPENS_PER_HOUR),
            "too many shared posts opened this hour; try again later"
        );
        let endpoint = self.throwaway_endpoint().await?;
        let asked = async {
            let conn = endpoint
                .connect(EndpointAddr::new(ticket.node.id), SOCIAL_ALPN)
                .await
                .context("connecting to its author's node")?;
            let objects = request_on(&conn, &SocialRequest::GetObjects { ids: vec![*id] }).await?;
            let profile = request_on(&conn, &SocialRequest::GetProfile).await;
            anyhow::Ok((objects, profile))
        };
        let answer = tokio::time::timeout_at(deadline, asked).await;
        endpoint.close().await;
        let (objects, profile) = answer.context("its author's node didn't answer in time")??;
        let objects = match objects {
            SocialResponse::Objects { objects } => objects,
            SocialResponse::Error { message } => bail!("its author's node replied: {message}"),
            other => bail!("unexpected reply to GetObjects: {other:?}"),
        };
        let me = self.signer.identity();
        for obj in objects {
            match self.verify_received(&obj) {
                Ok(Payload::Note(note))
                    if obj.id() == *id && note.actor == ticket.actor && !note.is_reply() =>
                {
                    let profile = match profile {
                        Ok(SocialResponse::Profile { profile }) => {
                            self.opened_profile(profile, ticket)
                        }
                        _ => None,
                    };
                    // Held already if it arrived another way meanwhile: it opens
                    // as that.
                    self.db.keep_opened(
                        &obj,
                        &note,
                        profile.as_ref().map(|(p, s)| (p, s)),
                        now_ms(),
                    )?;
                    return Ok(Fetched::Post);
                }
                Ok(Payload::Delete(d)) if d.object == *id && d.actor == ticket.actor => {
                    if let Some(deleted) = self.db.apply_delete(&obj, &d, true, &me)? {
                        self.store.let_go(&deleted.files).await;
                    }
                    return Ok(Fetched::Deleted);
                }
                Ok(_) => {}
                Err(e) => warn!(post = %id.fmt_short(), "a shared post that doesn't verify: {e:#}"),
            }
        }
        bail!("its author's node doesn't have that post")
    }

    /// The profile an author's node gave, if it's theirs and well dated.
    fn opened_profile(
        &self,
        profile: SignedObject,
        ticket: &FollowTicket,
    ) -> Option<(Person, SignedObject)> {
        if profile.signer() != ticket.actor {
            return None;
        }
        match self.verify_received(&profile) {
            Ok(Payload::Person(person)) if profile_date(&person).is_ok() => Some((person, profile)),
            _ => None,
        }
    }
}

/// Drops opened posts past their 7 days, every `SWEEP_EVERY`.
pub(crate) async fn sweep_loop(core: Arc<Core>) {
    loop {
        tokio::time::sleep(SWEEP_EVERY).await;
        if let Err(e) = core.trim_opened().await {
            warn!("dropping old opened posts: {e:#}");
        }
    }
}
