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

use std::{sync::Arc, time::Duration};

use anyhow::{Result, anyhow, bail, ensure};
use iroh::{EndpointAddr, EndpointId};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_proto::{
    Payload, SignedObject,
    machine::{
        ListingEntry, ListingItem, MAX_LISTED_PERSON_BYTES, MAX_LISTED_POSTS, MAX_LISTING_PAGE,
        OFFERS_DIRECTORY,
    },
    social::{SocialRequest, SocialResponse},
};
use tokio::sync::{Semaphore, mpsc};
use tracing::{debug, info, warn};

use crate::{
    db::{Db, Listed, now_ms},
    health,
    machine::{self, MachineSide},
    node::Core,
    social::{PerHour, profile_date},
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
/// How far back in an outbox a directory looks for someone's latest posts. Most
/// entries are posts; a run of reshares just means fewer are listed.
const OUTBOX_TAIL: u32 = 20;
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
/// How soon it goes on after a read that failed or wasn't finished.
const READ_AGAIN: Duration = Duration::from_secs(10 * 60);
/// Pages taken in one conversation. With the rest of a large directory for next
/// time, one read stays a small thing over Tor.
const MAX_PAGES_PER_READ: usize = 64;
/// Most people kept from directories, and most bytes of them. Typically about
/// 1 KB a person, so these are far apart; the bytes are there for the hostile.
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
        if same_profile && latest == listed.latest {
            return self.db.set_listed_read(&listed.actor, latest, now_ms());
        }
        let posts = self.latest_posts(listed, &addr, latest).await?;
        let item = person_item(profile, posts);
        self.db.put_listed(&listed.actor, &item, latest, now_ms())
    }

    /// The posts at the end of a listed node's outbox, newest first.
    async fn latest_posts(
        &self,
        listed: &Listed,
        addr: &EndpointAddr,
        latest: u64,
    ) -> Result<Vec<SignedObject>> {
        let req = SocialRequest::GetOutbox {
            since: latest.saturating_sub(OUTBOX_TAIL as u64),
            limit: OUTBOX_TAIL,
        };
        let SocialResponse::Outbox { entries, .. } = self.social_request(addr, &req).await? else {
            bail!("unexpected reply to GetOutbox");
        };
        ensure!(
            entries.len() <= OUTBOX_TAIL as usize,
            "outbox page has {} entries; we asked for at most {OUTBOX_TAIL}",
            entries.len()
        );
        let mut posts = Vec::new();
        for entry in entries.into_iter().rev() {
            if entry.object.signer() != listed.actor {
                continue;
            }
            match self.verify_received(&entry.object) {
                Ok(Payload::Note(_)) => posts.push(entry.object),
                Ok(_) => {}
                Err(e) => debug!("skipping an invalid outbox entry: {e:#}"),
            }
        }
        Ok(posts)
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

/// A node reading the directories it knows.
impl Core {
    /// Reads each directory this node knows, under its machine name. Returns
    /// whether it got to the end of them all. One not answering doesn't stop the
    /// others; if none could be read, says why.
    pub(crate) async fn read_directories(&self) -> Result<bool> {
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
            return Ok(true);
        }
        let mut finished = true;
        let mut failed = None;
        let mut answered = false;
        for directory in &self.config.directory.known {
            if directory.machine == side.id() {
                continue;
            }
            let short = directory.machine.fmt_short();
            match self.read_directory(side, directory.machine).await {
                Ok(done) => {
                    if failing.remove(&directory.machine) {
                        info!(directory = %short, "a directory answers again");
                    }
                    answered = true;
                    finished &= done;
                }
                Err(e) => {
                    // Said once: one that's gone is tried every few minutes.
                    if failing.insert(directory.machine) {
                        warn!(directory = %short, "reading a directory failed: {e:#}");
                    } else {
                        debug!(directory = %short, "reading a directory failed again: {e:#}");
                    }
                    finished = false;
                    failed = Some(e);
                }
            }
        }
        match failed {
            Some(e) if !answered => Err(e),
            _ => Ok(finished),
        }
    }

    /// One conversation with a directory: a greeting, then pages from where
    /// this node got to. Returns whether it reached the end.
    async fn read_directory(&self, side: &MachineSide, directory: EndpointId) -> Result<bool> {
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
    ) -> Result<bool> {
        let info = side.hello_on(conn, directory).await?;
        ensure!(
            info.offers.iter().any(|offer| offer == OFFERS_DIRECTORY),
            "that machine doesn't say it's a directory"
        );
        let mut cursor = self.db.directory_cursor(&directory)?;
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
            cursor = self.take_page(side, directory, cursor, entries, latest)?;
            self.db.set_directory_cursor(&directory, cursor)?;
            self.db
                .trim_found(self.sim.max_found_people, MAX_FOUND_BYTES)?;
            if done || cursor >= latest {
                return Ok(true);
            }
        }
        Ok(false)
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
                ListingItem::Person { profile, posts } => {
                    if let Err(e) = self.take_person(directory, &profile, posts) {
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
            ensure!(matches!(post.verify()?, Payload::Note(_)), "not a post");
        }
        self.verify_received(profile)?;
        let mut checked = Vec::new();
        for post in posts {
            let payload = self.verify_received(&post)?;
            checked.push((post, payload));
        }
        self.db
            .keep_found(&directory, &person, profile, &checked, now_ms())
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

/// Someone's listing: their profile, and as many of their posts, newest first,
/// as fit. A post that would pass the limit is left out, rather than one person
/// taking up the page. The largest profile fits on its own.
fn person_item(profile: SignedObject, newest_first: Vec<SignedObject>) -> ListingItem {
    let mut bytes = encoded_len(&profile);
    let mut posts = Vec::new();
    for post in newest_first {
        let len = encoded_len(&post);
        if posts.len() < MAX_LISTED_POSTS && bytes + len <= MAX_LISTED_PERSON_BYTES {
            bytes += len;
            posts.push(post);
        }
    }
    ListingItem::Person { profile, posts }
}

fn encoded_len(object: &SignedObject) -> usize {
    serde_json::to_vec(object).map_or(usize::MAX / 2, |json| json.len())
}

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
/// `READ_AGAIN` after the last read.
pub(crate) async fn read_loop(core: Arc<Core>) {
    let again = READ_AGAIN.min(core.sim.read_every);
    tokio::time::sleep(core.sim.first_read_wait.mul_f64(random_fraction())).await;
    loop {
        // Messages wait for Tor, and go nowhere else (ADR 0014).
        if let Some(tor) = &core.tor {
            tor.ready().await;
        }
        let finished = core.read_directories().await.unwrap_or(false);
        let wait = match finished {
            true => jitter(core.sim.read_every),
            false => jitter(again),
        };
        let soonest = tokio::time::Instant::now() + again;
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = core.read_directories_now.notified() => {
                tokio::time::sleep_until(soonest).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;
    use osp_proto::{Note, Person};

    use super::*;

    const DATE: &str = "2026-11-01T00:00:00Z";

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
        let listed = listed_posts(person_item(profile(), posts.clone()));
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
        let listed = listed_posts(person_item(profile(), posts.clone()));
        assert_eq!(listed, [posts[0].clone(), posts[2].clone()]);
    }
}
