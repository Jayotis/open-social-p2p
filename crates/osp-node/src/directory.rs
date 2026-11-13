//! Directories (ADR 0015): an opt-in role any node can take on, so that people
//! with no follows have someone to find.
//!
//! A directory holds two lists. The people are those who asked to be listed,
//! from their public endpoint: each one's signed profile and latest posts, read
//! from their own node as a follower would. The map is the records of machines
//! that greeted it and answered when it greeted them back. It gives both to any
//! machine that asks, and neither says who talks to whom.
//!
//! A node reads its directories in the background under its machine name, and
//! keeps who it finds, capped and out of the feed, so a search looks through
//! them at home and no word anyone types leaves the node.

use std::{
    collections::{BTreeMap, HashSet},
    hash::{Hash, Hasher},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use iroh::{EndpointAddr, EndpointId};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_proto::{
    Announce, Note, ObjectId, Payload, Person, SignedObject,
    machine::{
        ListedReshare, ListingEntry, ListingItem, MAX_LISTED_PERSON_BYTES, MAX_LISTED_POSTS,
        MAX_LISTED_RESHARES, MAX_LISTING_PAGE, OFFERS_DIRECTORY,
    },
    social::{SocialRequest, SocialResponse},
};
use tokio::{
    sync::{Semaphore, mpsc},
    task::JoinHandle,
};
use tracing::{debug, info, warn};

use crate::{
    db::{Db, FoundReshare, HostingClaim, Listed, now_ms},
    health,
    machine::{self, MachineSide},
    node::Core,
    social::{PerHour, profile_date},
    store::Source,
    sync::{jitter, random_fraction},
};

/// Most people a directory lists. Each is read every `LISTED_READ_EVERY`.
const MAX_LISTED: usize = 5_000;
/// New listings per hour, at most. Identities are free, so this stops a flood.
/// Someone refused asks again later.
pub(crate) const MAX_NEW_LISTINGS_PER_HOUR: usize = 100;
/// How often a listed node is read. It also limits how often one person's
/// listing can change, and so how much their posts cost the directory's readers.
pub(crate) const LISTED_READ_EVERY: Duration = Duration::from_secs(60 * 60);
/// How often a directory looks for listed nodes due to be read.
const LISTED_TICK: Duration = Duration::from_secs(60);
/// Listed nodes read at once.
const LISTED_AT_ONCE: usize = 4;
/// Longest one listed node's reading may take.
const LISTED_READ_TIMEOUT: Duration = Duration::from_secs(120);
/// Someone whose node hasn't answered for this long is dropped. They can ask
/// again, as a node does every day.
const LISTED_SILENCE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How far back in an outbox a directory looks for someone's latest posts and
/// reshares. A long run of one just means fewer of the other are listed.
const OUTBOX_TAIL: u32 = 20;
/// How often a directory checks again that a listed resharer serves the copy
/// its reshare says it hosts (ADR 0025), at one of its reads of them.
pub(crate) const RESHARE_RECHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Checks failed in a row, after one passed, that take the hosting mark off.
const RESHARE_FAILS_UNMARK: u32 = 2;
/// Most hosting reshares of one post a directory lists (ADR 0025). When
/// more qualify, the day's draw picks them, and the rest are listed plain.
pub(crate) const MAX_HOSTING_RESHARES: usize = 16;
/// Of those, at most this many go to people listed for less than
/// `NEW_NAME_FOR`, so an operator can't take every place with names made
/// for it.
pub(crate) const MAX_NEW_NAME_HOSTS: usize = 8;
/// Someone listed for less than this is a new name.
pub(crate) const NEW_NAME_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// A hosting reshare qualifies only from someone listed at least this long:
/// a name made today can't host today.
pub(crate) const HOSTING_AFTER_LISTED: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// A machine on the map isn't greeted back again for this long.
const GREET_AGAIN_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Machines greeted back per hour, at most, whoever they say they are. Names
/// are free, and each costs Tor circuits. One left out greets again another time.
pub(crate) const MAX_GREETS_BACK_PER_HOUR: u32 = 60;
/// Machines greeted back at once.
const GREET_BACK_AT_ONCE: usize = 4;
/// Greetings waiting to be answered, at most; more are left for another time.
pub(crate) const GREETINGS_WAITING: usize = 64;
/// A machine on the map not heard from for this long is taken off it. Machines
/// check in far more often than this.
const MAP_SILENCE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// A node asks to be listed once a while after it starts, at a time of its own,
/// up to this long, so it isn't at the moment it greets a machine.
pub(crate) const ASK_LISTING_WAIT: Duration = Duration::from_secs(60 * 60);
/// Then about this often, so a directory that lost it lists it again, and one
/// on a new node is read there.
const ASK_LISTING_AGAIN: Duration = Duration::from_secs(24 * 60 * 60);
/// How soon it asks again when a directory didn't answer.
const ASK_LISTING_RETRY: Duration = Duration::from_secs(60 * 60);

/// How long after it starts a node first reads its directories, at most, at a
/// time of its own.
pub(crate) const FIRST_READ_WAIT: Duration = Duration::from_secs(10 * 60);
/// How often a node reads its directories. Discovery can wait, and each read
/// costs Tor circuits.
pub(crate) const READ_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Soonest a node reads its directories again because someone is using it: a
/// search or the feed starts a read, as what's new is wanted then (ADR 0015,
/// amended 2026-11-12). Reads started otherwise count too.
pub(crate) const USE_READ_GAP: Duration = Duration::from_secs(10 * 60);
/// Longest a search waits for the read it started, then searches what the
/// node holds. The read goes on.
pub(crate) const SEARCH_READ_WAIT: Duration = Duration::from_secs(15);
/// How soon it goes on after a read that failed or wasn't finished. Also the
/// soonest a node with no machine left to meet reads again (`soonest_after`).
const READ_AGAIN: Duration = Duration::from_secs(10 * 60);
/// Just after a node starts, how soon it tries again after a read that some
/// directory didn't answer, for its first `SOON_RETRIES` reads: a new Tor
/// client's first contacts can take minutes, and a new tester would otherwise
/// find nobody for 10 to 20 minutes. A search after such a read may also
/// start one this soon. Later failures wait `READ_AGAIN`, so a directory
/// that's off isn't asked every minute.
pub(crate) const RETRY_SOON: Duration = Duration::from_secs(60);
const SOON_RETRIES: u32 = 5;
/// The kinds of entry this reader takes from a directory's lists, as a number
/// that rises whenever a kind is added: 1 first took machines (ADR 0015's step
/// 3). A reader passes over kinds it doesn't know, and its cursor moves on past
/// them, so a later one reads each directory again from the start.
const READER: u32 = 1;
/// Pages taken in one conversation. With the rest of a large directory for next
/// time, one read stays a small thing over Tor.
const MAX_PAGES_PER_READ: usize = 64;
/// Most people kept from directories, and most bytes of them. A person's
/// listing measured about 8.6 KB with 5 photo posts, and 21 KB with 5 hosting
/// reshares too (2026-11-09), so the bytes fill first: 64 MiB holds about
/// 3,000 to 7,800 people.
pub(crate) const MAX_FOUND_PEOPLE: usize = 5_000;
const MAX_FOUND_BYTES: u64 = 64 << 20;

/// What a machine name needs of its node to act as a directory: its lists, and
/// a way to say who greeted it. It's handed no more, as a machine's replies come
/// only from what its machine learned and what people asked to have listed.
#[derive(Clone)]
pub(crate) struct DirectoryLists {
    db: Arc<Db>,
    greeted: mpsc::Sender<EndpointId>,
}

impl DirectoryLists {
    pub fn new(db: Arc<Db>, greeted: mpsc::Sender<EndpointId>) -> Self {
        Self { db, greeted }
    }

    /// A page of the lists, and the newest sequence number in them.
    pub fn listing(&self, since: u64, limit: u32) -> Result<(Vec<ListingEntry>, u64)> {
        self.db.listing_since(since, limit)
    }

    /// A machine greeted this directory. It's greeted back when there's room.
    pub fn greeted(&self, machine: EndpointId) {
        let _ = self.greeted.try_send(machine);
    }
}

impl Core {
    /// Someone asks this directory to list them, or, with a profile that says
    /// they may not be listed, to drop them.
    pub(crate) fn asked_listing(&self, remote: EndpointId, profile: &SignedObject) -> Result<()> {
        ensure!(self.config.directory.serve, "this node isn't a directory");
        // Only checked for now: their key is learned once they're kept.
        let Payload::Person(person) = profile.verify()? else {
            bail!("not a profile");
        };
        ensure!(
            person.nodes.contains(&remote),
            "the profile doesn't list your node"
        );
        profile_date(&person)?;
        let actor = person.actor;
        if !person.discoverable {
            if self.db.drop_listed(&actor)? {
                info!("dropped someone from the directory, as they asked");
            }
            return Ok(());
        }
        let node = self.db.listed_node(&actor)?;
        if node.is_none() {
            ensure!(
                self.db.listed_count()? < MAX_LISTED,
                "this directory lists as many people as it can"
            );
            ensure!(
                self.db.new_listings_since(now_ms() - 60 * 60 * 1000)?
                    < self.sim.new_listings_per_hour,
                "this directory is busy with new listings; try again later"
            );
        }
        self.verify_received(profile)?;
        // Asking again changes nothing, so it can't make the directory read
        // a node any oftener.
        if node != Some(remote) {
            self.db.add_listed(&actor, &remote)?;
            self.listed_now.notify_one();
        }
        Ok(())
    }

    /// Reads the listed nodes that are due, a few at once, each within a time
    /// limit, and drops those silent too long.
    async fn read_listed_due(&self) -> Result<()> {
        let now = now_ms();
        let every = self.sim.listed_read_every.as_millis() as i64;
        let due = self.db.listed_due(now - every)?;
        let _: Vec<()> = stream::iter(due)
            .map(|listed| async move {
                if let Err(e) = self.db.set_listed_tried(&listed.actor, now_ms()) {
                    warn!("couldn't note reading a listed node: {e:#}");
                }
                let read = tokio::time::timeout(LISTED_READ_TIMEOUT, self.read_listed(&listed))
                    .await
                    .unwrap_or_else(|_| Err(anyhow!("took too long")));
                if let Err(e) = read {
                    debug!(actor = %listed.actor.fmt_short(), "reading a listed node failed: {e:#}");
                }
                // Its turn is over: a directory keeps no circuit open to each.
                self.forget_conn(&listed.node);
            })
            .buffered_unordered(LISTED_AT_ONCE)
            .collect()
            .await;
        if let Err(e) = self.repick_daily() {
            warn!("couldn't pick posts' hosts for the day: {e:#}");
        }
        let dropped = self
            .db
            .drop_silent_listed(now - LISTED_SILENCE.as_millis() as i64)?;
        if dropped > 0 {
            info!(dropped, "dropped listed people whose nodes went silent");
        }
        self.db
            .forget_silent_machines(now - MAP_SILENCE.as_millis() as i64)
    }

    /// Reads someone listed from their own node, and updates what the directory
    /// gives readers if it changed. Their node saying they may no longer be
    /// listed, or that it no longer speaks for them, drops them.
    async fn read_listed(&self, listed: &Listed) -> Result<()> {
        let addr = EndpointAddr::new(listed.node);
        let reply = self
            .social_request(&addr, &SocialRequest::GetProfile)
            .await?;
        let SocialResponse::Profile { profile } = reply else {
            bail!("unexpected reply to GetProfile");
        };
        ensure!(
            profile.signer() == listed.actor,
            "node {} speaks for a different identity",
            listed.node.fmt_short()
        );
        let Payload::Person(person) = self.verify_received(&profile)? else {
            bail!("profile is not a Person");
        };
        profile_date(&person)?;
        if !person.discoverable || !person.nodes.contains(&listed.node) {
            if self.db.drop_listed(&listed.actor)? {
                info!("dropped someone from the directory, as their profile says");
            }
            return Ok(());
        }
        // Asking for no entries still says where the end is.
        let req = SocialRequest::GetOutbox { since: 0, limit: 0 };
        let SocialResponse::Outbox { latest, .. } = self.social_request(&addr, &req).await? else {
            bail!("unexpected reply to GetOutbox");
        };
        let same_profile = matches!(
            self.db.listed_item(&listed.actor)?,
            Some(ListingItem::Person { profile: known, .. }) if known == profile
        );
        let recheck = self.sim.reshare_recheck_every.as_millis() as i64;
        let rechecks_due = self
            .db
            .reshare_checks_due(&listed.actor, now_ms() - recheck)?;
        if same_profile && latest == listed.latest && !rechecks_due {
            return self.db.set_listed_read(&listed.actor, latest, now_ms());
        }
        let (posts, reshares) = self.latest_items(listed, &addr, latest).await?;
        let mut listed_reshares = Vec::new();
        for (announce, a, post, note) in reshares {
            let hosting = self.reshare_hosting(&person, &announce, &a, &note).await?;
            listed_reshares.push(ListedReshare {
                announce,
                post,
                hosting,
            });
        }
        let item = person_item(profile, posts, listed_reshares);
        let ListingItem::Person { reshares, .. } = &item else {
            unreachable!("person_item makes a person's listing");
        };
        let kept: Vec<ObjectId> = reshares.iter().map(|r| r.announce.id()).collect();
        self.db.forget_reshare_checks(&listed.actor, &kept)?;
        self.db.put_listed(&listed.actor, &item, latest, now_ms())
    }

    /// Whether a listed reshare is to be marked hosting (ADR 0025): it says
    /// so, a piece of the post's media fetched from the resharer's media
    /// endpoint checked out, its rechecks haven't failed twice in a row since,
    /// and it's in the day's pick of the post's hosts (`repick_hosts`).
    async fn reshare_hosting(
        &self,
        resharer: &Person,
        announce: &SignedObject,
        a: &Announce,
        note: &Note,
    ) -> Result<bool> {
        let id = announce.id();
        let now = now_ms();
        if !a.hosting {
            self.db.note_plain_reshare(&id, &a.actor, &a.object, now)?;
            return Ok(false);
        }
        let recheck = self.sim.reshare_recheck_every.as_millis() as i64;
        match self.db.reshare_check(&id)? {
            Some(check) if now - check.checked_ms < recheck => {}
            _ => {
                let passed = match self.check_resharer_copy(resharer, note).await {
                    Ok(()) => true,
                    Err(e) => {
                        debug!(resharer = %a.actor.fmt_short(), "a resharer's copy failed its check: {e:#}");
                        false
                    }
                };
                self.db
                    .note_reshare_check(&id, &a.actor, &a.object, passed, now)?;
            }
        }
        let picked = self.repick_hosts(&a.object, Some(&a.actor), false)?;
        let hosting = picked.contains(&id);
        if !hosting && picked.len() >= self.sim.max_hosting_reshares {
            info!(
                post = %a.object.fmt_short(),
                "a post has more hosting reshares than are listed; this one is listed plain today"
            );
        }
        Ok(hosting)
    }

    /// The rules for picking a post's hosts, as this node runs them.
    fn host_rules(&self) -> HostRules {
        HostRules {
            most: self.sim.max_hosting_reshares,
            most_new: self.sim.max_new_name_hosts,
            after_listed_ms: self.sim.hosting_after_listed.as_millis() as i64,
            new_for_ms: self.sim.new_name_for.as_millis() as i64,
        }
    }

    /// Marks today's pick of `post`'s hosting reshares as hosting and the
    /// rest as plain (ADR 0025), and returns the pick. A listing whose mark
    /// changed is given to readers again, except `reading`'s, whose read is
    /// making it now. `log` says so when some that qualify aren't picked:
    /// a sign the rotation is needed in the wild.
    fn repick_hosts(
        &self,
        post: &ObjectId,
        reading: Option<&iroh::PublicKey>,
        log: bool,
    ) -> Result<HashSet<ObjectId>> {
        let claims = self.db.hosting_claims(post, RESHARE_FAILS_UNMARK)?;
        let (picked, qualifying) = pick_hosts(post, now_ms(), &claims, &self.host_rules());
        if log && qualifying > picked.len() {
            info!(
                post = %post.fmt_short(),
                qualifying,
                listed = picked.len(),
                "a post has more hosting reshares than are listed; today's draw picked them"
            );
        }
        for claim in &claims {
            let hosting = picked.contains(&claim.announce);
            if hosting == claim.hosting {
                continue;
            }
            self.db.set_reshare_hosting(&claim.announce, hosting)?;
            if Some(&claim.actor) != reading {
                self.remark_listed(&claim.actor, &claim.announce, hosting)?;
            }
        }
        Ok(picked)
    }

    /// Changes the hosting mark of `actor`'s listed reshare `announce`,
    /// without reading them again.
    fn remark_listed(
        &self,
        actor: &iroh::PublicKey,
        announce: &ObjectId,
        hosting: bool,
    ) -> Result<()> {
        let Some(ListingItem::Person {
            profile,
            posts,
            mut reshares,
        }) = self.db.listed_item(actor)?
        else {
            return Ok(());
        };
        let Some(reshare) = reshares
            .iter_mut()
            .find(|r| r.announce.id() == *announce && r.hosting != hosting)
        else {
            return Ok(());
        };
        reshare.hosting = hosting;
        let item = ListingItem::Person {
            profile,
            posts,
            reshares,
        };
        self.db.replace_listed_item(actor, &item)
    }

    /// Once a day, the first time it's called that day, picks again the
    /// hosts of every post with a hosting reshare listed: the pick changes
    /// each day, and people cross `HOSTING_AFTER_LISTED` and `NEW_NAME_FOR`.
    fn repick_daily(&self) -> Result<()> {
        let day = now_ms() / DAY_MS;
        if self.hosts_picked_day.swap(day, Ordering::Relaxed) == day {
            return Ok(());
        }
        for post in self.db.posts_with_hosting_claims()? {
            self.repick_hosts(&post, None, true)?;
        }
        Ok(())
    }

    /// Fetches one random 1 KiB piece of the post's media from the
    /// resharer's media endpoint, by the binding in their profile, and checks
    /// it against the hash. Evidence it serves a copy, not proof.
    async fn check_resharer_copy(&self, resharer: &Person, note: &Note) -> Result<()> {
        let bound = resharer
            .nodes
            .iter()
            .find_map(|node| resharer.media_endpoint(node))
            .context("their profile gives no media endpoint")?;
        let blobs: Vec<_> = note.sized_blobs().collect();
        ensure!(!blobs.is_empty(), "the post has no media");
        let pick = ((random_fraction() * blobs.len() as f64) as usize).min(blobs.len() - 1);
        let (blob, size) = blobs[pick];
        let source = Source {
            node: bound.public,
            endpoint: bound.endpoint,
            from_direct: false,
        };
        self.store.check_piece(&blob, size, source).await
    }

    /// Keeps a tombstone for a post its author deleted, and drops every
    /// listed reshare of it (ADR 0025), so it can't be listed again.
    async fn listed_delete(&self, delete: &SignedObject, d: &osp_proto::Delete) -> Result<()> {
        let mut forwards = Vec::new();
        // That drops it from the lists too (`drop_listed`).
        self.act_on_delete(delete, d, true, &mut forwards).await
    }

    /// Drops a deleted post from what this directory lists at once, however it
    /// learned of the `Delete` (ADR 0025): from its author's listing, and every
    /// listed reshare of it. A post this node follows is deleted through its
    /// follow, before the next read of its author would see it.
    pub(crate) fn drop_listed(&self, post: &ObjectId, author: &iroh::PublicKey) -> Result<()> {
        if !self.config.directory.serve || !self.db.is_tombstoned(post)? {
            return Ok(());
        }
        let mut actors = self.db.take_reshares_of(post)?;
        if !actors.contains(author) {
            actors.push(*author);
        }
        for actor in actors {
            let Some(ListingItem::Person {
                profile,
                posts,
                reshares,
            }) = self.db.listed_item(&actor)?
            else {
                continue;
            };
            let (before_posts, before_reshares) = (posts.len(), reshares.len());
            let posts: Vec<_> = posts.into_iter().filter(|p| p.id() != *post).collect();
            let reshares: Vec<_> = reshares
                .into_iter()
                .filter(|r| r.post.id() != *post)
                .collect();
            if posts.len() == before_posts && reshares.len() == before_reshares {
                continue;
            }
            let item = ListingItem::Person {
                profile,
                posts,
                reshares,
            };
            self.db.replace_listed_item(&actor, &item)?;
            info!(post = %post.fmt_short(), "dropped a deleted post from a listing");
        }
        Ok(())
    }

    /// The posts and reshares at the end of a listed node's outbox, newest
    /// first, each reshare with the post it points at. A `Delete` there, the
    /// author's or one a resharer passes on, is acted on: its post is
    /// tombstoned here, and no reshare of it is listed again (ADR 0025).
    async fn latest_items(
        &self,
        listed: &Listed,
        addr: &EndpointAddr,
        latest: u64,
    ) -> Result<(Vec<SignedObject>, Vec<ListedCandidate>)> {
        let req = SocialRequest::GetOutbox {
            since: latest.saturating_sub(OUTBOX_TAIL as u64),
            limit: OUTBOX_TAIL,
        };
        let SocialResponse::Outbox {
            entries,
            referenced,
            ..
        } = self.social_request(addr, &req).await?
        else {
            bail!("unexpected reply to GetOutbox");
        };
        ensure!(
            entries.len() <= OUTBOX_TAIL as usize,
            "outbox page has {} entries; we asked for at most {OUTBOX_TAIL}",
            entries.len()
        );
        let referenced = |id: &ObjectId| referenced.iter().find(|obj| obj.id() == *id);
        let mut posts = Vec::new();
        let mut reshares = Vec::new();
        // Deletes first, so a reshare deleted in the same page isn't listed.
        for entry in &entries {
            if entry.object.signer() != listed.actor {
                continue;
            }
            let delete = match self.verify_received(&entry.object) {
                Ok(Payload::Delete(d)) => Some((entry.object.clone(), d)),
                Ok(Payload::ForwardDelete(f)) => {
                    referenced(&f.delete).and_then(|obj| match self.verify_received(obj) {
                        Ok(Payload::Delete(d)) if obj.signer() == d.actor => Some((obj.clone(), d)),
                        _ => None,
                    })
                }
                _ => None,
            };
            if let Some((obj, d)) = delete
                && let Err(e) = self.listed_delete(&obj, &d).await
            {
                debug!("skipping a delete in a listed outbox: {e:#}");
            }
        }
        for entry in entries.into_iter().rev() {
            if entry.object.signer() != listed.actor {
                continue;
            }
            match self.verify_received(&entry.object) {
                Ok(Payload::Note(note)) if !note.is_reply() => posts.push(entry.object),
                Ok(Payload::Announce(a)) if reshares.len() < MAX_LISTED_RESHARES => {
                    if self.db.is_tombstoned(&a.object)? {
                        continue;
                    }
                    let Some(post) = referenced(&a.object) else {
                        continue;
                    };
                    match self.verify_received(post) {
                        Ok(Payload::Note(note))
                            if post.signer() == a.object_actor && !note.is_reply() =>
                        {
                            reshares.push((entry.object, a, post.clone(), note));
                        }
                        Ok(_) => debug!("skipping a reshare of something that isn't its post"),
                        Err(e) => debug!("skipping a reshared post: {e:#}"),
                    }
                }
                Ok(_) => {}
                Err(e) => debug!("skipping an invalid outbox entry: {e:#}"),
            }
        }
        Ok((posts, reshares))
    }

    /// Asks each directory this node knows to list its owner, from the public
    /// endpoint and never the machine name. Says whether all of them answered.
    pub(crate) async fn ask_to_be_listed(&self) -> bool {
        let req = SocialRequest::AskListing {
            profile: self.profile.clone(),
        };
        let mut answered = true;
        for directory in &self.config.directory.known {
            // A directory can't read its own node.
            if directory.node == self.endpoint.id() {
                continue;
            }
            let addr = EndpointAddr::new(directory.node);
            if let Err(e) = self.social_request(&addr, &req).await {
                warn!(
                    directory = %directory.node.fmt_short(),
                    "asking to be listed failed: {e:#}"
                );
                answered = false;
            }
        }
        answered
    }
}

/// How a read of the directories went.
pub(crate) struct Read {
    /// It got to the end of them all.
    pub finished: bool,
    /// Entries it took in, of any kind.
    pub entries: usize,
    /// The directories that didn't answer, by their machine name's words,
    /// and why.
    pub failed: Vec<String>,
}

/// A node reading the directories it knows.
impl Core {
    /// Reads each directory this node knows, under its machine name. One not
    /// answering doesn't stop the others; if none could be read, says why.
    pub(crate) async fn read_directories(&self) -> Result<Read> {
        *self.directories_read_at.lock().expect("poisoned") = Some(Instant::now());
        let side = self.machine()?;
        if let Some(tor) = &self.tor {
            tor.check_ready()?;
        }
        let mut failing = self.reading_directories.lock().await;
        // Strangers' posts go first when space is short.
        if health::disk_free(self.home.root()).is_some_and(|free| free < self.sim.found_needs_free)
        {
            let dropped = self.db.trim_found(0, 0)?;
            info!(
                dropped,
                "the disk is nearly full, so dropped people found through directories"
            );
            return Ok(Read {
                finished: true,
                entries: 0,
                failed: Vec::new(),
            });
        }
        let mut read = Read {
            finished: true,
            entries: 0,
            failed: Vec::new(),
        };
        let mut answered = false;
        for directory in &self.config.directory.known {
            if directory.machine == side.id() {
                continue;
            }
            let short = directory.machine.fmt_short();
            match self.read_directory(side, directory.machine).await {
                Ok((done, entries)) => {
                    if failing.remove(&directory.machine) {
                        info!(directory = %short, "a directory answers again");
                    }
                    answered = true;
                    read.finished &= done;
                    read.entries += entries;
                }
                Err(e) => {
                    // Said once: one that's gone is tried every few minutes.
                    if failing.insert(directory.machine) {
                        warn!(directory = %short, "reading a directory failed: {e:#}");
                    } else {
                        debug!(directory = %short, "reading a directory failed again: {e:#}");
                    }
                    read.finished = false;
                    read.failed.push(format!(
                        "{}: {}",
                        machine::words(&directory.machine),
                        e.root_cause()
                    ));
                }
            }
        }
        if !read.failed.is_empty() {
            // The next read for a search may come after `RETRY_SOON`, not
            // `use_read_gap`.
            let gap = self.sim.use_read_gap.saturating_sub(RETRY_SOON);
            *self.directories_read_at.lock().expect("poisoned") = Instant::now().checked_sub(gap);
            if !answered {
                bail!("no directory answered: {}", read.failed.join("; "));
            }
        }
        Ok(read)
    }

    /// Starts a read of the directories for someone using the node, unless it
    /// knows none or a read started in the last `use_read_gap`. Returns the
    /// read's task, for a search to wait on.
    pub(crate) fn read_directories_for_use(self: &Arc<Self>) -> Option<JoinHandle<()>> {
        if self.config.directory.known.is_empty() || self.machine.is_none() {
            return None;
        }
        {
            let mut at = self.directories_read_at.lock().expect("poisoned");
            if at.is_some_and(|at| at.elapsed() < self.sim.use_read_gap) {
                return None;
            }
            *at = Some(Instant::now());
        }
        let core = self.clone();
        Some(tokio::spawn(async move {
            if let Err(e) = core.read_directories().await {
                debug!("reading directories for a search or the feed failed: {e:#}");
            }
        }))
    }

    /// One conversation with a directory: a greeting, then pages from where
    /// this node got to. Returns whether it reached the end, and the entries
    /// it took in.
    async fn read_directory(
        &self,
        side: &MachineSide,
        directory: EndpointId,
    ) -> Result<(bool, usize)> {
        let conn = side.connect(directory).await?;
        let read = self.read_pages(side, &conn, directory).await;
        machine::close(&conn);
        read
    }

    async fn read_pages(
        &self,
        side: &MachineSide,
        conn: &iroh::endpoint::Connection,
        directory: EndpointId,
    ) -> Result<(bool, usize)> {
        let info = side.hello_on(conn, directory).await?;
        ensure!(
            info.offers.iter().any(|offer| offer == OFFERS_DIRECTORY),
            "that machine doesn't say it's a directory"
        );
        let mut cursor = self.db.directory_cursor(&directory, READER)?;
        let mut taken = 0;
        for _ in 0..MAX_PAGES_PER_READ {
            let (entries, latest) = side
                .listing(conn, directory, cursor, MAX_LISTING_PAGE)
                .await?;
            // Its numbers never go down, so this is a directory starting over.
            if latest < cursor {
                info!("a directory started over, so reading it from the start");
                self.db.forget_directory(&directory)?;
                cursor = 0;
                continue;
            }
            ensure!(
                entries.len() <= MAX_LISTING_PAGE as usize,
                "a page of its lists has {} entries; we asked for at most {MAX_LISTING_PAGE}",
                entries.len()
            );
            let done = entries.is_empty();
            taken += entries.len();
            cursor = self.take_page(side, directory, cursor, entries, latest)?;
            self.db.set_directory_cursor(&directory, cursor, READER)?;
            self.db
                .trim_found(self.sim.max_found_people, MAX_FOUND_BYTES)?;
            if done || cursor >= latest {
                return Ok((true, taken));
            }
        }
        Ok((false, taken))
    }

    /// Takes in a page of a directory's lists. Numbers must rise, so each page
    /// moves on, and a directory can't keep this node going round. One bad entry
    /// costs only that entry. Returns the number reached.
    fn take_page(
        &self,
        side: &MachineSide,
        directory: EndpointId,
        mut cursor: u64,
        entries: Vec<ListingEntry>,
        latest: u64,
    ) -> Result<u64> {
        for entry in entries {
            ensure!(
                entry.seq > cursor && entry.seq <= latest,
                "a directory's entries are out of order"
            );
            cursor = entry.seq;
            match entry.item {
                ListingItem::Person {
                    profile,
                    posts,
                    reshares,
                } => {
                    if let Err(e) = self.take_person(directory, &profile, posts, reshares) {
                        debug!("skipping a directory's entry: {e:#}");
                    }
                }
                ListingItem::Gone { actor } => self.db.forget_found(&directory, &actor)?,
                // The node's first machines to meet, counted against the
                // directory as their source, as any other machine's would be.
                ListingItem::Machine { record } => side.acquaintances.on_map(directory, &record),
                ListingItem::Unknown => {}
            }
        }
        Ok(cursor)
    }

    /// Keeps someone a directory lists, once all of it checks out. A signature
    /// shows who wrote something, so the directory needn't be trusted, though it
    /// can leave things out or be behind.
    fn take_person(
        &self,
        directory: EndpointId,
        profile: &SignedObject,
        posts: Vec<SignedObject>,
        reshares: Vec<ListedReshare>,
    ) -> Result<()> {
        let actor = profile.signer();
        // This node has better than a directory's copy of these.
        if actor == self.signer.identity() || self.db.is_following(&actor)? {
            return Ok(());
        }
        ensure!(
            posts.len() <= MAX_LISTED_POSTS,
            "it lists {} posts of one person",
            posts.len()
        );
        ensure!(
            reshares.len() <= MAX_LISTED_RESHARES,
            "it lists {} reshares of one person",
            reshares.len()
        );
        // All checked before a key is learned from any of it, so what's refused
        // leaves nothing behind (`verify_received`).
        let Payload::Person(person) = profile.verify()? else {
            bail!("not a profile");
        };
        profile_date(&person)?;
        // Consent travels with the record, and a later profile can take it back.
        if !person.discoverable || self.taken_back(&person)? {
            return self.db.forget_found(&directory, &actor);
        }
        for post in &posts {
            ensure!(post.signer() == actor, "a post signed by someone else");
            ensure!(
                matches!(post.verify()?, Payload::Note(n) if !n.is_reply()),
                "not a post"
            );
        }
        // Each reshare is its owner's, of a post signed by the author it
        // names, and the directory's hosting mark only on one that says so.
        for r in &reshares {
            ensure!(
                r.announce.signer() == actor,
                "a reshare signed by someone else"
            );
            let Payload::Announce(a) = r.announce.verify()? else {
                bail!("not a reshare");
            };
            ensure!(a.actor == actor, "a reshare made for someone else");
            ensure!(
                r.post.id() == a.object,
                "a reshare listed with another post"
            );
            ensure!(
                r.post.signer() == a.object_actor,
                "a reshared post signed by someone else"
            );
            ensure!(
                matches!(r.post.verify()?, Payload::Note(n) if !n.is_reply()),
                "not a post"
            );
        }
        self.verify_received(profile)?;
        let mut checked = Vec::new();
        for post in posts {
            let payload = self.verify_received(&post)?;
            checked.push((post, payload));
        }
        let mut found = Vec::new();
        for r in reshares {
            // A post known deleted stays out, however old the directory's data.
            if self.db.is_tombstoned(&r.post.id())? {
                continue;
            }
            let announce_payload = self.verify_received(&r.announce)?;
            let post_payload = self.verify_received(&r.post)?;
            let claims = matches!(&announce_payload, Payload::Announce(a) if a.hosting);
            found.push(FoundReshare {
                announce: r.announce,
                announce_payload,
                post: r.post,
                post_payload,
                hosting: r.hosting && claims,
            });
        }
        self.db
            .keep_found_with(&directory, &person, profile, &checked, &found, now_ms())
    }

    /// Whether a later profile than `person` is stored, saying they may not be
    /// listed. A directory that's behind, or won't let go, can't bring them back.
    fn taken_back(&self, person: &osp_proto::Person) -> Result<bool> {
        let Some(stored) = self.db.profile(&person.actor)? else {
            return Ok(false);
        };
        let Ok(Payload::Person(known)) = stored.verify() else {
            return Ok(false);
        };
        Ok(!known.discoverable
            && osp_proto::parse_rfc3339_ms(&known.published)
                > osp_proto::parse_rfc3339_ms(&person.published))
    }
}

/// Someone's listing: their profile, and as many of their posts and then of
/// their reshares, newest first, as fit. One that would pass the limit is left
/// out, rather than one person taking up the page. The largest profile fits on
/// its own.
fn person_item(
    profile: SignedObject,
    newest_posts: Vec<SignedObject>,
    newest_reshares: Vec<ListedReshare>,
) -> ListingItem {
    let mut bytes = encoded_len(&profile);
    let mut posts = Vec::new();
    for post in newest_posts {
        let len = encoded_len(&post);
        if posts.len() < MAX_LISTED_POSTS && bytes + len <= MAX_LISTED_PERSON_BYTES {
            bytes += len;
            posts.push(post);
        }
    }
    let mut reshares = Vec::new();
    for reshare in newest_reshares {
        let len = encoded_len(&reshare.announce) + encoded_len(&reshare.post) + 32;
        if reshares.len() < MAX_LISTED_RESHARES && bytes + len <= MAX_LISTED_PERSON_BYTES {
            bytes += len;
            reshares.push(reshare);
        }
    }
    ListingItem::Person {
        profile,
        posts,
        reshares,
    }
}

/// How a directory picks a post's hosts (ADR 0025).
#[derive(Debug, Clone, Copy)]
pub(crate) struct HostRules {
    /// Most listed as hosting.
    pub most: usize,
    /// Most of those from people listed for less than `new_for_ms`.
    pub most_new: usize,
    /// Someone listed for less than this doesn't qualify.
    pub after_listed_ms: i64,
    pub new_for_ms: i64,
}

/// Which of `post`'s hosting claims are listed as hosting on the day of
/// `now`, and how many people qualified. A claim qualifies if its checks
/// pass and its resharer has been listed `after_listed_ms`. Each person
/// counts once, by one of their reshares. When more qualify than are
/// listed, a draw from the post and the day picks them: it holds all day
/// and changes the next, and nothing a host does moves it up. People listed
/// for less than `new_for_ms` take at most `most_new` places, even if
/// places are left empty.
pub(crate) fn pick_hosts(
    post: &ObjectId,
    now: i64,
    claims: &[HostingClaim],
    rules: &HostRules,
) -> (HashSet<ObjectId>, usize) {
    let mut people: BTreeMap<iroh::PublicKey, (ObjectId, i64)> = BTreeMap::new();
    for claim in claims {
        let Some(listed_ms) = claim.listed_ms else {
            continue;
        };
        if !claim.passing || now - listed_ms < rules.after_listed_ms {
            continue;
        }
        let entry = people
            .entry(claim.actor)
            .or_insert((claim.announce, listed_ms));
        entry.0 = entry.0.min(claim.announce);
    }
    let qualifying = people.len();
    let day = now / DAY_MS;
    let mut drawn: Vec<_> = people.into_iter().collect();
    drawn.sort_by_cached_key(|(actor, _)| daily_rank(post.as_bytes(), day, actor.as_bytes()));
    let mut picked = HashSet::new();
    let mut new = 0;
    for (_, (announce, listed_ms)) in drawn {
        if picked.len() >= rules.most {
            break;
        }
        if now - listed_ms < rules.new_for_ms {
            if new >= rules.most_new {
                continue;
            }
            new += 1;
        }
        picked.insert(announce);
    }
    (picked, qualifying)
}

/// Where `who` comes in the day's draw for `what`: the same all day, another
/// the next, and the same on every node that draws.
pub(crate) fn daily_rank(what: &[u8], day: i64, who: &[u8]) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    (what, day, who).hash(&mut h);
    h.finish()
}

fn encoded_len(object: &SignedObject) -> usize {
    serde_json::to_vec(object).map_or(usize::MAX / 2, |json| json.len())
}

/// A reshare read from a listed outbox: the `Announce`, and the post it
/// points at, each signed and decoded.
type ListedCandidate = (SignedObject, Announce, SignedObject, Note);

/// A directory's reading of those who asked to be listed.
pub(crate) async fn listed_loop(core: Arc<Core>) {
    loop {
        // Messages wait for Tor, and go nowhere else (ADR 0014).
        if let Some(tor) = &core.tor {
            tor.ready().await;
        }
        if let Err(e) = core.read_listed_due().await {
            warn!("reading listed nodes failed: {e:#}");
        }
        tokio::select! {
            _ = tokio::time::sleep(LISTED_TICK.min(core.sim.listed_read_every)) => {}
            _ = core.listed_now.notified() => {}
        }
    }
}

/// A directory greeting back the machines that greeted it, within limits, as
/// anyone can make a name for free. Those that answer go on the map.
pub(crate) async fn greet_back_loop(core: Arc<Core>, mut greeted: mpsc::Receiver<EndpointId>) {
    let Some(me) = core.machine.as_ref().map(|side| side.id()) else {
        return;
    };
    let limits = PerHour::default();
    let places = Arc::new(Semaphore::new(GREET_BACK_AT_ONCE));
    while let Some(machine) = greeted.recv().await {
        let now = now_ms();
        match core.db.heard_machine(&machine, now) {
            Ok(Some(when)) if now - when < GREET_AGAIN_AFTER.as_millis() as i64 => continue,
            Ok(_) => {}
            Err(e) => {
                warn!("couldn't note a machine heard from: {e:#}");
                continue;
            }
        }
        if machine == me
            || !limits.allow(machine, 1)
            || !limits.allow(me, core.sim.greets_back_per_hour)
        {
            continue;
        }
        let Ok(place) = places.clone().try_acquire_owned() else {
            continue;
        };
        let core = core.clone();
        tokio::spawn(async move {
            if let Some(tor) = &core.tor {
                tor.ready().await;
            }
            if let Some(side) = &core.machine {
                let kept = match side.hello(machine).await {
                    Ok(greeting) => core.db.put_machine(&machine, &greeting.record, now_ms()),
                    Err(e) => Err(e),
                };
                // Not which, as a log holds the public ID too, and the error
                // names it.
                if kept.is_err() {
                    debug!("greeting a machine back failed");
                }
            }
            drop(place);
        });
    }
}

/// A node asking to be listed: at a time of its own after it starts, then now
/// and then. Its profile says it may be, so a directory can check.
pub(crate) async fn ask_listing_loop(core: Arc<Core>) {
    let mut wait = core.sim.ask_listing_wait.mul_f64(random_fraction());
    loop {
        tokio::time::sleep(wait).await;
        if let Some(tor) = &core.tor {
            tor.ready().await;
        }
        wait = match core.ask_to_be_listed().await {
            true => jitter(ASK_LISTING_AGAIN),
            false => jitter(ASK_LISTING_RETRY),
        };
    }
}

/// A node reading its directories: at a time of its own after it starts, then
/// now and then, and sooner if it didn't get to the end, or if the machines it
/// met have all stopped answering (ADR 0016), though no sooner than
/// `Backoff` says after the last read.
pub(crate) async fn read_loop(core: Arc<Core>) {
    let every = core.sim.read_every;
    let again = READ_AGAIN.min(every);
    let mut backoff = Backoff {
        again,
        every,
        empty: 0,
    };
    tokio::time::sleep(core.sim.first_read_wait.mul_f64(random_fraction())).await;
    // Whether this read is an early one, as no machine was left to meet.
    let mut early = false;
    let mut soon_retries = SOON_RETRIES;
    loop {
        // Messages wait for Tor, and go nowhere else (ADR 0014).
        if let Some(tor) = &core.tor {
            tor.ready().await;
        }
        let read = core.read_directories().await.ok();
        let some_failed = read.as_ref().is_none_or(|read| !read.failed.is_empty());
        let wait = match read.as_ref().is_some_and(|read| read.finished) {
            true => jitter(every),
            false if some_failed && soon_retries > 0 => {
                soon_retries -= 1;
                jitter(RETRY_SOON.min(again))
            }
            false => jitter(again),
        };
        let soonest = tokio::time::Instant::now() + backoff.after(early, read.as_ref());
        early = tokio::select! {
            _ = tokio::time::sleep(wait) => false,
            _ = core.read_directories_now.notified() => {
                tokio::time::sleep_until(soonest).await;
                true
            }
        };
    }
}

/// How soon a node with no machine left to meet reads again: `again` after a
/// read, and twice as long after each early read in a row that got to the end
/// and brought nothing new, up to `every`. So a node alone, or whose
/// directories have nobody new, doesn't ask every few minutes for good. The
/// usual reads don't count, so a node that met machines for months still reads
/// soon once they all go quiet.
struct Backoff {
    again: Duration,
    every: Duration,
    /// Early reads in a row that brought nothing.
    empty: u32,
}

impl Backoff {
    /// Notes a read, early or not, and `None` if it failed. Returns how long
    /// an early one waits after it.
    fn after(&mut self, early: bool, read: Option<&Read>) -> Duration {
        self.empty = match read {
            Some(read) if early && read.finished && read.entries == 0 => {
                self.empty.saturating_add(1)
            }
            _ => 0,
        };
        let doublings = self.empty.saturating_sub(1).min(16);
        self.again.saturating_mul(1 << doublings).min(self.every)
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;
    use osp_proto::{Note, Person};

    use super::*;

    const DATE: &str = "2026-11-01T00:00:00Z";

    const RULES: HostRules = HostRules {
        most: 16,
        most_new: 8,
        after_listed_ms: 7 * DAY_MS,
        new_for_ms: 30 * DAY_MS,
    };

    /// A passing hosting claim by someone listed `days` ago, on day 100.
    fn claim(n: u8, days: i64) -> HostingClaim {
        HostingClaim {
            announce: ObjectId::from_bytes([n; 32]),
            actor: SecretKey::from_bytes(&[n; 32]).public(),
            listed_ms: Some(NOW - days * DAY_MS),
            passing: true,
            hosting: false,
        }
    }

    const NOW: i64 = 100 * DAY_MS + 1;

    fn picked(claims: &[HostingClaim], now: i64) -> HashSet<ObjectId> {
        pick_hosts(&ObjectId::from_bytes([200; 32]), now, claims, &RULES).0
    }

    #[test]
    fn up_to_the_most_qualifying_hosts_are_all_listed() {
        let claims: Vec<_> = (1..=16).map(|n| claim(n, 60)).collect();
        assert_eq!(picked(&claims, NOW).len(), 16);
        // Not yet listed a week, failing their checks, or not listed now.
        let mut not = vec![claim(1, 6), claim(2, 60), claim(3, 60)];
        not[1].passing = false;
        not[2].listed_ms = None;
        assert!(picked(&not, NOW).is_empty());
        assert_eq!(picked(&[claim(1, 7)], NOW).len(), 1);
    }

    #[test]
    fn past_the_most_the_days_draw_picks_them_and_holds_all_day() {
        let claims: Vec<_> = (1..=40).map(|n| claim(n, 60)).collect();
        let (today, qualifying) =
            pick_hosts(&ObjectId::from_bytes([200; 32]), NOW, &claims, &RULES);
        assert_eq!((today.len(), qualifying), (16, 40));
        // Later the same day, and in another order: the same pick.
        let mut shuffled = claims.clone();
        shuffled.reverse();
        assert_eq!(picked(&shuffled, NOW + DAY_MS - 2), today);
        // The next day another, and over a month everyone gets a turn.
        assert_ne!(picked(&claims, NOW + DAY_MS), today);
        let mut seen = HashSet::new();
        for day in 0..30 {
            seen.extend(picked(&claims, NOW + day * DAY_MS));
        }
        assert_eq!(seen.len(), 40);
        // Another post, another pick.
        assert_ne!(
            pick_hosts(&ObjectId::from_bytes([201; 32]), NOW, &claims, &RULES).0,
            today
        );
    }

    #[test]
    fn new_names_take_at_most_half_the_places_even_if_some_stay_empty() {
        let new: Vec<_> = (1..=20).map(|n| claim(n, 10)).collect();
        assert_eq!(picked(&new, NOW).len(), 8);
        let mut mixed = new.clone();
        mixed.extend((21..=30).map(|n| claim(n, 31)));
        let pick = picked(&mixed, NOW);
        assert_eq!(pick.len(), 16);
        let old = pick.iter().filter(|a| a.as_bytes()[0] > 20).count();
        assert!(old >= 8, "{old} of 16 listed a month");
    }

    #[test]
    fn each_person_counts_once() {
        let mut twice = claim(1, 60);
        twice.announce = ObjectId::from_bytes([99; 32]);
        let claims = [claim(1, 60), twice, claim(2, 60)];
        let (pick, qualifying) = pick_hosts(&ObjectId::from_bytes([200; 32]), NOW, &claims, &RULES);
        assert_eq!((pick.len(), qualifying), (2, 2));
        assert!(pick.contains(&ObjectId::from_bytes([1; 32])));
    }

    #[test]
    fn a_node_with_nobody_to_meet_reads_less_often_while_reads_bring_nothing() {
        let mut backoff = Backoff {
            again: READ_AGAIN,
            every: READ_EVERY,
            empty: 0,
        };
        let nothing = Read {
            finished: true,
            entries: 0,
            failed: Vec::new(),
        };
        let mut minutes = |early, read| backoff.after(early, read).as_secs() / 60;
        // The usual reads bringing nothing don't count.
        for _ in 0..10 {
            assert_eq!(minutes(false, Some(&nothing)), 10);
        }
        let waits: Vec<u64> = (0..9).map(|_| minutes(true, Some(&nothing))).collect();
        assert_eq!(waits, [10, 20, 40, 80, 160, 320, 360, 360, 360]);
        // One that brings something starts over, as does one that fails.
        let something = Read {
            failed: Vec::new(),
            finished: true,
            entries: 1,
        };
        assert_eq!(minutes(true, Some(&something)), 10);
        minutes(true, Some(&nothing));
        assert_eq!(minutes(true, Some(&nothing)), 20);
        assert_eq!(minutes(true, None), 10);
    }

    fn key() -> SecretKey {
        SecretKey::from_bytes(&[3; 32])
    }

    fn post(text: String) -> SignedObject {
        let note = Note {
            actor: key().public(),
            content: text,
            published: DATE.into(),
            attachment: vec![],
            in_reply_to: None,
        };
        SignedObject::sign(&key(), &Payload::Note(note)).unwrap()
    }

    fn profile() -> SignedObject {
        let person = Person {
            actor: key().public(),
            name: "Alice".into(),
            summary: None,
            discoverable: true,
            nodes: vec![],
            media_endpoints: vec![],
            published: DATE.into(),
        };
        SignedObject::sign(&key(), &Payload::Person(person)).unwrap()
    }

    fn listed_posts(item: ListingItem) -> Vec<SignedObject> {
        match item {
            ListingItem::Person { posts, .. } => posts,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_listing_has_the_latest_few_posts() {
        let posts: Vec<_> = (0..MAX_LISTED_POSTS + 3)
            .map(|i| post(format!("post {i}")))
            .collect();
        let listed = listed_posts(person_item(profile(), posts.clone(), vec![]));
        assert_eq!(listed, posts[..MAX_LISTED_POSTS]);
    }

    #[test]
    fn a_listing_leaves_out_a_post_too_large_and_goes_on_to_the_next() {
        let huge = "\u{1}".repeat(osp_proto::types::MAX_CONTENT_BYTES);
        let posts = vec![
            post("newest".into()),
            post(huge.clone()),
            post("older".into()),
        ];
        let listed = listed_posts(person_item(profile(), posts.clone(), vec![]));
        assert_eq!(listed, [posts[0].clone(), posts[2].clone()]);
    }
}
