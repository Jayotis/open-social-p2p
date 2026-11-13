//! Who vouched for this node's owner (ADR 0029).
//!
//! Bob reshares Jimmy's post. Bob's node tells Jimmy's (`Reshared`), and
//! Jimmy's node keeps Bob's `Announce` as a vouch for Jimmy. When a post of
//! Jimmy's has 10 different resharers within 7 days of its first, Jimmy's node
//! credits the first 3 it heard of with finding it early: it signs a
//! `FoundEarly` for each and sends it to that finder's node, which keeps it as
//! a vouch for its owner. This is how a hunter is vouched for.

use std::{collections::HashSet, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use iroh::{EndpointAddr, EndpointId, PublicKey};
use osp_proto::{
    FoundEarly, ObjectId, Payload, SignedObject,
    social::{CarriedVouch, MAX_SIDE_VOUCHES, MAX_VOUCHES_ANSWERED, SocialRequest, SocialResponse},
};
use tracing::{debug, info, warn};

use crate::{
    db::{GivenCredit, NewVouch, SideVouch, VouchKind, now_ms},
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
/// "Who vouched for you" answered for one node an hour.
pub(crate) const MAX_VOUCH_QUESTIONS_PER_HOUR: u32 = 10;
/// How far back from the owner their side reaches: who vouched for them is 1.
const SIDE_STEPS: u8 = 3;
/// Nodes asked in one gathering of the owner's side, at most. Once a day,
/// so under 100 requests a day (ADR 0029).
pub(crate) const MAX_GATHER_REQUESTS: usize = 80;
/// How often the owner's side is gathered, give or take a tenth.
const GATHER_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long one node gets to answer "who vouched for you".
const QUESTION_TIMEOUT: Duration = Duration::from_secs(60);

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
        Ok(self
            .db
            .voucher_profile(actor)?
            .map(|p| profile_nodes(&p))
            .unwrap_or_default())
    }

    /// "Who vouched for your owner?" from the node `remote`: the newest vouch
    /// of each voucher, at most `MAX_VOUCHES_ANSWERED`, each with its
    /// voucher's profile. Vouches and profiles signed with keys their
    /// identities revoked are left out.
    pub(crate) fn who_vouched(&self, remote: EndpointId) -> Result<Vec<CarriedVouch>> {
        ensure!(
            self.vouch_questions
                .allow(remote, MAX_VOUCH_QUESTIONS_PER_HOUR),
            "this node answers no more of your questions about vouches this hour; try again later"
        );
        let mut vouches = Vec::new();
        for kept in self.db.newest_vouches(MAX_SIDE_VOUCHES)? {
            if vouches.len() >= MAX_VOUCHES_ANSWERED {
                break;
            }
            if self.db.signed_by_revoked_key(&kept.signed)?
                || self.db.signed_by_revoked_key(&kept.profile)?
            {
                continue;
            }
            vouches.push(CarriedVouch {
                vouch: kept.signed,
                profile: kept.profile,
            });
        }
        Ok(vouches)
    }

    /// Gathers the owner's side (ADR 0029): who vouched for them (kept
    /// here), then, asking their nodes, who vouched for those, then one step
    /// further back. Each voucher once, at the nearest step; at most
    /// `MAX_SIDE_VOUCHES` in all and `MAX_GATHER_REQUESTS` questions. Nodes
    /// that don't answer, or answer with what doesn't check out, are passed
    /// over. Replaces the side kept before.
    pub(crate) async fn gather_side(&self) -> Result<Gathered> {
        let me = self.signer.identity();
        let mut placed = HashSet::from([me]);
        let mut side = Vec::new();
        // Whom to ask about next: each with the profile that names their nodes.
        let mut frontier = Vec::new();
        for kept in self.db.newest_vouches(MAX_SIDE_VOUCHES)? {
            if self.db.signed_by_revoked_key(&kept.signed)? || !placed.insert(kept.voucher) {
                continue;
            }
            side.push(SideVouch {
                voucher: kept.voucher,
                step: 1,
                signed: kept.signed,
            });
            frontier.push((kept.voucher, kept.profile));
        }
        let mut asked = 0;
        for step in 2..=SIDE_STEPS {
            let mut next = Vec::new();
            for (subject, profile) in frontier {
                if side.len() >= MAX_SIDE_VOUCHES || asked >= MAX_GATHER_REQUESTS {
                    break;
                }
                let Some(&node) = profile_nodes(&profile).first() else {
                    continue;
                };
                asked += 1;
                let vouches = match self.ask_who_vouched(node).await {
                    Ok(vouches) => vouches,
                    Err(e) => {
                        debug!(
                            voucher = %subject.fmt_short(),
                            "couldn't ask who vouched for a voucher: {e:#}"
                        );
                        continue;
                    }
                };
                for carried in vouches.into_iter().take(MAX_VOUCHES_ANSWERED) {
                    if side.len() >= MAX_SIDE_VOUCHES {
                        break;
                    }
                    let voucher = match check_carried(&carried, &subject) {
                        Ok(voucher) => voucher,
                        Err(e) => {
                            debug!(
                                node = %node.fmt_short(),
                                "a node answered with a vouch that doesn't check out: {e:#}"
                            );
                            continue;
                        }
                    };
                    if self.db.signed_by_revoked_key(&carried.vouch)? || !placed.insert(voucher) {
                        continue;
                    }
                    side.push(SideVouch {
                        voucher,
                        step,
                        signed: carried.vouch,
                    });
                    next.push((voucher, carried.profile));
                }
            }
            frontier = next;
        }
        side.truncate(MAX_SIDE_VOUCHES);
        self.db.replace_side(&side, now_ms())?;
        let gathered = Gathered {
            side: side.len(),
            asked,
        };
        info!(
            side = gathered.side,
            asked = gathered.asked,
            "gathered who vouched for this node's owner"
        );
        Ok(gathered)
    }

    async fn ask_who_vouched(&self, node: EndpointId) -> Result<Vec<CarriedVouch>> {
        let addr = EndpointAddr::new(node);
        let ask = self.social_request(&addr, &SocialRequest::WhoVouched);
        match tokio::time::timeout(QUESTION_TIMEOUT, ask)
            .await
            .context("no answer in time")??
        {
            SocialResponse::Vouches { vouches } => Ok(vouches),
            other => bail!("unexpected reply to WhoVouched: {other:?}"),
        }
    }
}

/// What a gathering of the owner's side came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gathered {
    /// People on the side.
    pub side: usize,
    /// Nodes asked.
    pub asked: usize,
}

/// Who signed a vouch, and whom it's for. Checks its signatures.
pub(crate) fn vouch_parts(signed: &SignedObject) -> Result<(PublicKey, PublicKey)> {
    let (voucher, subject) = match signed.verify()? {
        Payload::Announce(a) => (a.actor, a.object_actor),
        Payload::FoundEarly(f) => (f.actor, f.finder),
        other => bail!("a {} isn't a vouch", other.kind()),
    };
    ensure!(
        signed.signer() == voucher,
        "the vouch isn't signed by its voucher"
    );
    ensure!(voucher != subject, "nobody vouches for themselves");
    Ok((voucher, subject))
}

/// Checks a vouch an answer to `WhoVouched` carried: that it verifies, is
/// for `subject`, and comes with its voucher's profile. Returns the voucher.
fn check_carried(carried: &CarriedVouch, subject: &PublicKey) -> Result<PublicKey> {
    let (voucher, of) = vouch_parts(&carried.vouch)?;
    ensure!(of == *subject, "the vouch is for someone else");
    ensure!(
        carried.profile.signer() == voucher,
        "the profile isn't the voucher's"
    );
    let Payload::Person(_) = carried.profile.verify()? else {
        bail!("that isn't a profile");
    };
    Ok(voucher)
}

/// The home nodes a profile lists, if it checks out.
fn profile_nodes(profile: &SignedObject) -> Vec<EndpointId> {
    match profile.verify() {
        Ok(Payload::Person(person)) => person.nodes,
        _ => Vec::new(),
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

/// Gathers the owner's side once a day, at a time of its own: first a while
/// after starting, if the last was a day ago or more.
pub(crate) async fn gather_loop(core: std::sync::Arc<Core>) {
    loop {
        let since = match core.db.side_gathered_ms() {
            Ok(Some(ms)) => Duration::from_millis(now_ms().saturating_sub(ms).max(0) as u64),
            _ => GATHER_EVERY,
        };
        let every = GATHER_EVERY.mul_f64(0.9 + 0.2 * random_fraction());
        let wait =
            every.saturating_sub(since) + Duration::from_secs(10 * 60).mul_f64(random_fraction());
        tokio::time::sleep(wait).await;
        if let Some(tor) = &core.tor {
            tor.ready().await;
        }
        if let Err(e) = core.gather_side().await {
            warn!("gathering who vouched for this node's owner failed: {e:#}");
            tokio::time::sleep(Duration::from_secs(60 * 60)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;
    use osp_proto::{Announce, Note, Person};

    use super::*;

    fn profile(who: &SecretKey) -> SignedObject {
        let person = Payload::Person(Person {
            actor: who.public(),
            name: "Someone".into(),
            summary: None,
            discoverable: false,
            nodes: vec![],
            media_endpoints: vec![],
            published: osp_proto::now_rfc3339(),
        });
        SignedObject::sign(who, &person).unwrap()
    }

    fn reshare(who: &SecretKey, of: PublicKey) -> SignedObject {
        let a = Payload::Announce(Announce {
            actor: who.public(),
            object: ObjectId::of(b"a post"),
            object_actor: of,
            published: osp_proto::now_rfc3339(),
            hosting: false,
        });
        SignedObject::sign(who, &a).unwrap()
    }

    fn carried(vouch: SignedObject, profile: SignedObject) -> CarriedVouch {
        CarriedVouch { vouch, profile }
    }

    #[test]
    fn a_carried_vouch_counts_only_if_it_is_for_whom_it_says_with_its_vouchers_profile() {
        let (bob, carol, dan) = (
            SecretKey::generate(),
            SecretKey::generate(),
            SecretKey::generate(),
        );
        let good = carried(reshare(&carol, bob.public()), profile(&carol));
        assert_eq!(check_carried(&good, &bob.public()).unwrap(), carol.public());

        // For someone else than asked about.
        let err = check_carried(&good, &dan.public()).unwrap_err();
        assert!(err.to_string().contains("for someone else"), "{err:#}");
        // With someone else's profile.
        let wrong = carried(reshare(&carol, bob.public()), profile(&dan));
        let err = check_carried(&wrong, &bob.public()).unwrap_err();
        assert!(err.to_string().contains("isn't the voucher's"), "{err:#}");
        // Something else where the profile goes.
        let not_profile = carried(reshare(&carol, bob.public()), reshare(&carol, dan.public()));
        let err = check_carried(&not_profile, &bob.public()).unwrap_err();
        assert!(err.to_string().contains("isn't a profile"), "{err:#}");
        // Not a vouch.
        let note = Payload::Note(Note {
            actor: carol.public(),
            content: "hi".into(),
            published: osp_proto::now_rfc3339(),
            attachment: vec![],
            in_reply_to: None,
        });
        let note = carried(SignedObject::sign(&carol, &note).unwrap(), profile(&carol));
        let err = check_carried(&note, &bob.public()).unwrap_err();
        assert!(err.to_string().contains("isn't a vouch"), "{err:#}");
        // For oneself.
        let own = carried(reshare(&bob, bob.public()), profile(&bob));
        let err = check_carried(&own, &bob.public()).unwrap_err();
        assert!(err.to_string().contains("themselves"), "{err:#}");
    }

    #[test]
    fn a_vouch_that_doesnt_verify_doesnt_count() {
        let (bob, carol) = (SecretKey::generate(), SecretKey::generate());
        let mut vouch = reshare(&carol, bob.public());
        let mut json = serde_json::to_value(&vouch).unwrap();
        // Signed by Carol, but claimed by someone else.
        json["signer"] = serde_json::to_value(bob.public()).unwrap();
        vouch = serde_json::from_value(json).unwrap();
        assert!(check_carried(&carried(vouch, profile(&carol)), &bob.public()).is_err());
    }
}
