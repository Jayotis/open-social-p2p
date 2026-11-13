//! Who vouched for this node's owner (ADR 0029).
//!
//! Bob reshares Jimmy's post. Bob's node tells Jimmy's (`Reshared`), and
//! Jimmy's node keeps Bob's `Announce` as a vouch for Jimmy. When a post of
//! Jimmy's has 10 different resharers within 7 days of its first, Jimmy's node
//! credits the first 3 it heard of with finding it early: it signs a
//! `FoundEarly` for each and sends it to that finder's node, which keeps it as
//! a vouch for its owner. This is how a hunter is vouched for.

use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use iroh::{EndpointAddr, EndpointId, PublicKey};
use osp_proto::{
    FoundEarly, ObjectId, Payload, SignedObject,
    social::{SocialRequest, SocialResponse},
};
use tracing::{debug, info, warn};

use crate::{
    db::{GivenCredit, NewVouch, VouchKind, now_ms},
    node::Core,
    social::profile_date,
    sync::{jitter, random_fraction},
};

/// Vouches by reshare kept at most. Past it, those of the oldest posts go.
pub(crate) const MAX_RESHARE_VOUCHES: usize = 10_000;
/// Found-early credits kept at most: one for each of the owner's reshares
/// that took off, so far fewer.
pub(crate) const MAX_CREDITS_KEPT: usize = 1_000;
/// New resharers, from everyone together, taken an hour. Past it they're
/// refused, so a flood can't push out the real ones.
pub(crate) const MAX_NEW_RESHARERS_PER_HOUR: u32 = 200;
/// Credits one node may send an hour.
pub(crate) const MAX_CREDITS_FROM_NODE_PER_HOUR: u32 = 10;
/// Different resharers a post needs, within `CREDIT_WINDOW` of its first,
/// for its first finders to be credited.
pub(crate) const RESHARERS_FOR_CREDIT: usize = 10;
pub(crate) const CREDIT_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Finders credited for one post.
pub(crate) const FINDERS_CREDITED: usize = 3;
/// The owner's reshares told to their authors' nodes in one daily pass, at
/// most, newest first: the rest the next day.
const RESHARES_TOLD_A_PASS: usize = 100;
/// How often untold reshares and credits are tried again.
const TOLD_AGAIN_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

impl Core {
    /// The node `remote` says its owner reshared one of this node's owner's
    /// posts. Kept as a vouch, with the resharer's profile; then, if the post
    /// has taken off, its first finders are credited.
    pub(crate) fn reshared(
        &self,
        remote: EndpointId,
        announce: &SignedObject,
        profile: &SignedObject,
    ) -> Result<()> {
        let me = self.signer.identity();
        let Payload::Announce(a) = announce.verify()? else {
            bail!("that isn't a reshare");
        };
        ensure!(
            a.object_actor == me && a.actor != me,
            "that reshare isn't of this node's posts"
        );
        let post_ms = match self.db.outbox_object(&a.object)?.map(|o| o.verify()) {
            Some(Ok(Payload::Note(note))) if note.actor == me => {
                osp_proto::parse_rfc3339_ms(&note.published).unwrap_or_else(now_ms)
            }
            _ => bail!("{} isn't one of this node's posts", a.object.fmt_short()),
        };
        let profile_ms = check_profile(remote, a.actor, profile)?;
        if !self.db.knows_voucher(&a.actor)?
            && !self
                .new_resharers
                .allow(self.endpoint.id(), MAX_NEW_RESHARERS_PER_HOUR)
        {
            debug!(resharer = %a.actor.fmt_short(), "too many new resharers this hour");
            bail!("this node takes no more new resharers this hour; try again later");
        }
        // To be kept: checked against the keys this node knows of now (ADR 0010).
        self.keys_current(&[profile, announce])?;
        let now = now_ms();
        self.db.add_vouch(
            &NewVouch {
                kind: VouchKind::Reshare,
                post: a.object,
                voucher: a.actor,
                signed: announce,
                post_ms,
                now_ms: now,
            },
            MAX_RESHARE_VOUCHES,
        )?;
        self.db
            .keep_voucher_profile(&a.actor, profile, profile_ms)?;
        debug!(
            resharer = %a.actor.fmt_short(),
            post = %a.object.fmt_short(),
            "a resharer told us of their reshare"
        );
        if self.credit_finders(&a.object)? {
            self.tell_vouches_now.notify_one();
        }
        Ok(())
    }

    /// Credits the first `FINDERS_CREDITED` resharers of `post`, in the order
    /// this node heard of them, once `RESHARERS_FOR_CREDIT` different ones
    /// came within `CREDIT_WINDOW` of the first, if that wasn't done before.
    /// Says whether it gave credits now.
    pub(crate) fn credit_finders(&self, post: &ObjectId) -> Result<bool> {
        if self.db.credited(post)? {
            return Ok(false);
        }
        let resharers = self.db.resharers_in_order(post)?;
        let Some(&(_, first_ms)) = resharers.first() else {
            return Ok(false);
        };
        let window_end = first_ms + CREDIT_WINDOW.as_millis() as i64;
        let within = resharers.iter().filter(|(_, ms)| *ms <= window_end).count();
        if within < RESHARERS_FOR_CREDIT {
            return Ok(false);
        }
        let me = self.signer.identity();
        let published = osp_proto::now_rfc3339();
        let mut credits = Vec::new();
        for (finder, _) in resharers.iter().take(FINDERS_CREDITED) {
            let signed = self.signer.sign(&Payload::FoundEarly(FoundEarly {
                actor: me,
                post: *post,
                finder: *finder,
                published: published.clone(),
            }))?;
            credits.push(GivenCredit {
                post: *post,
                finder: *finder,
                signed,
            });
        }
        self.db.add_credits(&credits)?;
        info!(post = %post.fmt_short(), "a post took off; credited its first finders");
        Ok(true)
    }

    /// The node `remote` says its owner credits this node's owner with
    /// finding one of their posts early. Kept as a vouch, with the author's
    /// profile, if this node's owner did reshare that post.
    pub(crate) fn found_early(
        &self,
        remote: EndpointId,
        credit: &SignedObject,
        profile: &SignedObject,
    ) -> Result<()> {
        let me = self.signer.identity();
        let Payload::FoundEarly(f) = credit.verify()? else {
            bail!("that isn't a found-early credit");
        };
        ensure!(
            f.finder == me && f.actor != me,
            "that credit isn't for this node's owner"
        );
        let profile_ms = check_profile(remote, f.actor, profile)?;
        ensure!(
            self.credit_senders
                .allow(remote, MAX_CREDITS_FROM_NODE_PER_HOUR),
            "this node takes no more credits from yours this hour; try again later"
        );
        ensure!(
            self.db.owner_reshared(&me, &f.post)?,
            "this node's owner didn't reshare {}",
            f.post.fmt_short()
        );
        ensure!(
            self.stored_note(&f.post)?
                .is_some_and(|note| note.actor == f.actor),
            "that credit isn't from the post's author"
        );
        self.keys_current(&[profile, credit])?;
        let now = now_ms();
        self.db.add_vouch(
            &NewVouch {
                kind: VouchKind::FoundEarly,
                post: f.post,
                voucher: f.actor,
                signed: credit,
                post_ms: osp_proto::parse_rfc3339_ms(&f.published).unwrap_or(now),
                now_ms: now,
            },
            MAX_CREDITS_KEPT,
        )?;
        self.db
            .keep_voucher_profile(&f.actor, profile, profile_ms)?;
        info!(
            author = %f.actor.fmt_short(),
            post = %f.post.fmt_short(),
            "credited with finding a post early"
        );
        Ok(())
    }

    /// Tells the node of the author of the post `announce` reshares of it.
    /// Noted once its node took it.
    pub(crate) async fn tell_author_reshared(&self, announce: &SignedObject) -> Result<()> {
        let Payload::Announce(a) = announce.verify()? else {
            bail!("not a reshare");
        };
        let node = *self
            .nodes_of(&a.object_actor)
            .first()
            .context("no node known for the post's author")?;
        let req = SocialRequest::Reshared {
            announce: announce.clone(),
            profile: self.profile.clone(),
        };
        match self.social_request(&EndpointAddr::new(node), &req).await? {
            SocialResponse::Ok => self.db.note_reshare_told(&announce.id()),
            other => bail!("unexpected reply to Reshared: {other:?}"),
        }
    }

    /// Tells authors' nodes of the owner's reshares they weren't told of yet,
    /// newest first, at most `RESHARES_TOLD_A_PASS`. Returns how many were told.
    pub(crate) async fn tell_authors_reshared(&self) -> Result<usize> {
        let me = self.signer.identity();
        let mut untold = Vec::new();
        for (_, obj) in self.db.outbox_since(0, u32::MAX)?.into_iter().rev() {
            if untold.len() >= RESHARES_TOLD_A_PASS {
                break;
            }
            if let Ok(Payload::Announce(a)) = obj.verify()
                && a.object_actor != me
                && !self.db.reshare_told(&obj.id())?
            {
                untold.push(obj);
            }
        }
        let mut told = 0;
        for announce in untold {
            match self.tell_author_reshared(&announce).await {
                Ok(()) => told += 1,
                Err(e) => debug!("couldn't tell an author's node of our reshare: {e:#}"),
            }
        }
        Ok(told)
    }

    /// Sends each credit the owner gave that its finder's node doesn't have
    /// yet. Returns how many were taken.
    pub(crate) async fn tell_finders(&self) -> Result<usize> {
        let mut told = 0;
        for credit in self.db.untold_credits()? {
            match self.tell_finder(&credit).await {
                Ok(()) => {
                    self.db.credit_told(&credit.post, &credit.finder)?;
                    told += 1;
                }
                Err(e) => debug!(
                    finder = %credit.finder.fmt_short(),
                    "couldn't give a finder their credit: {e:#}"
                ),
            }
        }
        Ok(told)
    }

    async fn tell_finder(&self, credit: &GivenCredit) -> Result<()> {
        let node = *self
            .voucher_nodes(&credit.finder)?
            .first()
            .context("no node known for the finder")?;
        let req = SocialRequest::FoundEarly {
            credit: credit.signed.clone(),
            profile: self.profile.clone(),
        };
        match self.social_request(&EndpointAddr::new(node), &req).await? {
            SocialResponse::Ok => Ok(()),
            other => bail!("unexpected reply to FoundEarly: {other:?}"),
        }
    }

    /// The home nodes a voucher's kept profile lists.
    fn voucher_nodes(&self, actor: &PublicKey) -> Result<Vec<EndpointId>> {
        Ok(match self.db.voucher_profile(actor)?.map(|p| p.verify()) {
            Some(Ok(Payload::Person(person))) => person.nodes,
            _ => Vec::new(),
        })
    }
}

/// Checks that `profile` is `actor`'s, is dated sensibly and lists `remote`.
/// Returns its date.
fn check_profile(remote: EndpointId, actor: PublicKey, profile: &SignedObject) -> Result<i64> {
    ensure!(
        profile.signer() == actor,
        "the profile isn't the sender's owner's"
    );
    let Payload::Person(person) = profile.verify()? else {
        bail!("that isn't a profile");
    };
    let published = profile_date(&person)?;
    ensure!(
        person.nodes.contains(&remote),
        "the profile doesn't list your node"
    );
    Ok(published)
}

/// Tells authors' nodes of the owner's reshares, and finders' nodes of the
/// credits the owner gave: at once when woken, and each day for those
/// missed while a node was off.
pub(crate) async fn vouches_loop(core: std::sync::Arc<Core>) {
    let mut wait = Duration::from_secs(10 * 60).mul_f64(random_fraction());
    loop {
        tokio::select! {
            _ = core.tell_vouches_now.notified() => {}
            _ = tokio::time::sleep(wait) => {}
        }
        // Messages wait for Tor, and go nowhere else (ADR 0014).
        if let Some(tor) = &core.tor {
            tor.ready().await;
        }
        if let Err(e) = core.tell_finders().await {
            warn!("giving finders their credits failed: {e:#}");
        }
        if let Err(e) = core.tell_authors_reshared().await {
            warn!("telling authors of our reshares failed: {e:#}");
        }
        wait = jitter(TOLD_AGAIN_EVERY);
    }
}
