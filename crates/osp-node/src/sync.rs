//! Gathering: syncing followed outboxes, prefetching their media (spread
//! out if this node joins the follower swarm, and tried again later when
//! the nodes with it are busy), holding new posts out of the feed until
//! their media is here, and finding nodes that can serve a blob.
//!
//! A followed node decides what its outbox says, so everything it sends is
//! checked and bounded: each page must move our position forward, a sync
//! takes a limited number of pages and a limited time, and only so many new
//! objects are taken from one identity per day.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, MutexGuard},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail, ensure};
use iroh::{EndpointAddr, EndpointId, PublicKey};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_proto::{
    BlobHash, Note, ObjectId, Payload, SignedObject,
    social::{MAX_GET_OBJECTS, MAX_OUTBOX_PAGE, SocialRequest, SocialResponse},
};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{
    db::{BlobClass, Follow, now_ms},
    node::{Core, check_addr, remember},
    social::profile_date,
    store,
};

/// How often we ask each followed node to keep notifying us.
const RESUBSCRIBE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Hosts taken from one "who hosts" reply. Enough to find a copy.
const MAX_HOSTS_PER_REPLY: usize = 20;
/// How long media someone opened waits for a followed node to name other
/// hosts before the download starts without them. Nodes that are up answer
/// in milliseconds.
const ASK_FIRST_WAIT: Duration = Duration::from_secs(3);
/// However often followed nodes notify us, we sync at most this often.
const MIN_SYNC_GAP: Duration = Duration::from_secs(5);
/// Followed nodes synced at once.
const SYNC_AT_ONCE: usize = 4;
/// Longest one followed node's sync may take. What arrived by then is kept.
const FOLLOW_SYNC_TIMEOUT: Duration = Duration::from_secs(120);
/// Outbox pages taken from one followed node per sync; the rest waits.
const MAX_PAGES_PER_SYNC: usize = 10;
/// New posts and reshares taken from one followed identity per day. Far
/// more than a person posts; it stops a node flooding ours. The rest
/// waits for the next day.
const MAX_NEW_PER_DAY: usize = 500;
/// When we start following someone, we take their latest posts and
/// reshares, not their whole history.
const BACKFILL: u64 = 100;
/// Media larger than this isn't prefetched; it's fetched when it's opened.
const MAX_PREFETCH_BYTES: u64 = 256 << 20;
/// Most files, and bytes, prefetched per sync.
const MAX_PREFETCH_FILES_PER_SYNC: usize = 200;
const MAX_PREFETCH_BYTES_PER_SYNC: u64 = 1 << 30;
/// How long a prefetch refused as busy waits before it is tried again. Each
/// later wait is twice as long, up to `MAX_RETRY_WAIT`, and each is varied
/// by up to half either way, so followers refused together don't come back
/// together.
pub(crate) const FIRST_RETRY_WAIT: Duration = Duration::from_secs(60);
const MAX_RETRY_WAIT: Duration = Duration::from_secs(15 * 60);
/// Tries again before giving up: about 45 minutes with the waits above.
/// The media is then fetched when it's opened.
const MAX_RETRIES: u32 = 6;
/// A node in the follower swarm waits a random while, up to this long,
/// before fetching a large file, so followers' fetches spread out and the
/// early ones can serve the rest (doc 13).
pub(crate) const PREFETCH_SPREAD: Duration = Duration::from_secs(10 * 60);
/// Prefetches waiting, spread out or to be tried again, at most; more are
/// dropped and fetched when they're opened.
pub(crate) const MAX_WAITING: usize = 1_000;
/// A new post from someone we follow is kept out of the feed until its
/// large media is on this node, so the person never taps a video that
/// isn't here yet, but at most this long. Long enough for a spread-out
/// prefetch and its download.
pub(crate) const MAX_HOLD: Duration = Duration::from_secs(20 * 60);

/// Polls followed nodes on the configured interval, or sooner when woken.
pub(crate) async fn run_loop(core: Arc<Core>) {
    let interval = Duration::from_secs(core.config.sync.interval_secs.max(5));
    loop {
        let started = Instant::now();
        if let Err(e) = core.sync_all(false).await {
            warn!("sync failed: {e:#}");
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = core.sync_now.notified() => {}
        }
        tokio::time::sleep(MIN_SYNC_GAP.saturating_sub(started.elapsed())).await;
    }
}

impl Core {
    /// Syncs every follow. Returns how many new objects arrived. With
    /// `wait_for_prefetch`, also waits until their media is downloaded.
    pub(crate) async fn sync_all(self: &Arc<Self>, wait_for_prefetch: bool) -> Result<usize> {
        let mut new_objects = 0;
        {
            let _guard = self.sync_lock.lock().await;
            // A few at once, each within a time limit, so one slow or
            // hostile node can't hold up the others.
            let results: Vec<_> = stream::iter(self.db.follows()?)
                .map(|follow| async move {
                    let result =
                        tokio::time::timeout(FOLLOW_SYNC_TIMEOUT, self.sync_follow(&follow))
                            .await
                            .unwrap_or_else(|_| {
                                Err(anyhow!("gave up after {} s", FOLLOW_SYNC_TIMEOUT.as_secs()))
                            });
                    (follow, result)
                })
                .buffered_unordered(SYNC_AT_ONCE)
                .collect()
                .await;
            let mut candidates = Vec::new();
            for (follow, result) in results {
                match result {
                    Ok((n, blobs)) => {
                        new_objects += n;
                        candidates.extend(blobs);
                    }
                    Err(e) => {
                        warn!(actor = %follow.actor.fmt_short(), "sync failed: {e:#}");
                        self.db.set_follow_error(&follow.actor, &format!("{e:#}"))?;
                    }
                }
            }
            let to_prefetch = choose_prefetch(candidates);
            self.plan_holds(&to_prefetch)?;
            if self.config.sync.prefetch && !to_prefetch.is_empty() {
                // Counted while still holding the sync lock, so a waiting
                // `Sync` that runs next is guaranteed to see this job.
                self.prefetch_jobs.send_modify(|n| *n += 1);
                let core = self.clone();
                tokio::spawn(async move {
                    core.prefetch(to_prefetch).await;
                    core.prefetch_jobs.send_modify(|n| *n -= 1);
                });
            }
        }
        if new_objects > 0 {
            info!(new_objects, "synced");
        }
        if wait_for_prefetch {
            // Includes prefetches started by earlier background syncs.
            let _ = self.prefetch_jobs.subscribe().wait_for(|n| *n == 0).await;
        }
        Ok(new_objects)
    }

    /// Fetches new outbox entries from one followed node. Returns the number
    /// of new objects, and the media worth prefetching.
    async fn sync_follow(&self, follow: &Follow) -> Result<(usize, Vec<Wanted>)> {
        let actor = follow.actor.fmt_short();
        // The profile gives us a display name and proves the node speaks for this identity.
        match self
            .social_request(&follow.addr, &SocialRequest::GetProfile)
            .await?
        {
            SocialResponse::Profile { profile } => {
                ensure!(
                    profile.signer() == follow.actor,
                    "node {} speaks for a different identity",
                    follow.addr.id.fmt_short()
                );
                let Payload::Person(person) = self.verify_received(&profile)? else {
                    bail!("profile is not a Person");
                };
                // One dated far ahead would outrank every later move (ADR 0009).
                match profile_date(&person) {
                    Ok(_) => self
                        .db
                        .upsert_profile(&person.actor, &person.name, &profile)?,
                    Err(e) => warn!(%actor, "keeping the profile we have: {e:#}"),
                }
            }
            _ => bail!("unexpected reply to GetProfile"),
        }
        // What was dropped after a revocation comes back before new posts.
        // If asking fails, the rest is asked for at the next sync.
        if let Err(e) = self.refetch_revoked(follow).await {
            warn!(%actor, "fetching posts again after a revocation failed: {e:#}");
        }

        let mut cursor = follow.cursor;
        if !follow.synced {
            // A new follow starts near the end of the outbox. Asking for no
            // entries still tells us where the end is.
            let req = SocialRequest::GetOutbox { since: 0, limit: 0 };
            let SocialResponse::Outbox { latest, .. } =
                self.social_request(&follow.addr, &req).await?
            else {
                bail!("unexpected reply to GetOutbox");
            };
            cursor = latest.saturating_sub(BACKFILL);
        }

        // Posts from before the follow aren't held back: they'd all land on
        // top as their media came in.
        let hold = follow.synced && self.config.sync.prefetch;
        let day_ago = now_ms() - 24 * 60 * 60 * 1000;
        let mut budget =
            MAX_NEW_PER_DAY.saturating_sub(self.db.objects_received_since(&follow.actor, day_ago)?);
        let mut new_objects = 0;
        let mut blobs = Vec::new();
        for _ in 0..MAX_PAGES_PER_SYNC {
            if budget == 0 {
                warn!(%actor, "took {MAX_NEW_PER_DAY} new objects today; the rest waits");
                break;
            }
            // Never ask for more than today's budget allows.
            let limit = MAX_OUTBOX_PAGE.min(budget as u32);
            let req = SocialRequest::GetOutbox {
                since: cursor,
                limit,
            };
            let SocialResponse::Outbox {
                entries,
                referenced,
                latest,
            } = self.social_request(&follow.addr, &req).await?
            else {
                bail!("unexpected reply to GetOutbox");
            };
            ensure!(
                entries.len() <= limit as usize,
                "outbox page has {} entries; we asked for at most {limit}",
                entries.len()
            );
            // Each reshare may bring the post it points at and that post's author.
            ensure!(
                referenced.len() <= 2 * entries.len(),
                "outbox page bundles more objects than its reshares need"
            );

            // Check every entry first. Sequence numbers must rise, so each
            // page moves us forward; a node can't keep us going round.
            let page_len = entries.len();
            let mut checked = Vec::new();
            for entry in entries {
                ensure!(entry.seq > cursor, "outbox entries are out of order");
                cursor = entry.seq;
                if entry.object.signer() != follow.actor {
                    warn!(%actor, "skipping an outbox entry signed by someone else");
                    continue;
                }
                match self.verify_received(&entry.object) {
                    Ok(Payload::Key(_)) => {
                        warn!(%actor, "skipping a key certificate; they come inside objects")
                    }
                    Ok(payload) => checked.push((entry.object, payload)),
                    Err(e) => warn!(%actor, "skipping an invalid outbox entry: {e:#}"),
                }
            }

            // Keep only what this page's reshares point at: the reshared
            // post, and its author's profile.
            let mut wanted_posts = HashSet::new();
            let mut wanted_authors = HashSet::new();
            for (_, payload) in &checked {
                if let Payload::Announce(a) = payload {
                    wanted_posts.insert(a.object);
                    wanted_authors.insert(a.object_actor);
                }
            }
            let mut notes: HashMap<ObjectId, Note> = HashMap::new();
            for obj in referenced {
                // What it's for comes first, so a page can't make us learn
                // the keys of identities no reshare names.
                if !wanted_posts.contains(&obj.id()) && !wanted_authors.contains(&obj.signer()) {
                    debug!(%actor, "ignoring a referenced object no reshare points at");
                    continue;
                }
                let Ok(payload) = self.verify_received(&obj) else {
                    warn!(%actor, "dropping an invalid referenced object");
                    continue;
                };
                match &payload {
                    Payload::Person(person) if wanted_authors.contains(&person.actor) => {
                        // Second-hand, possibly stale: only use it if we know nothing better.
                        if self.db.profile(&person.actor)?.is_none() {
                            self.db.upsert_profile(&person.actor, &person.name, &obj)?;
                        }
                    }
                    Payload::Note(note) if wanted_posts.contains(&obj.id()) => {
                        if self.store_object(&obj, &payload, Some(note), hold).await? {
                            new_objects += 1;
                        }
                        notes.insert(obj.id(), note.clone());
                    }
                    _ => debug!(%actor, "ignoring a referenced object no reshare points at"),
                }
            }

            for (object, payload) in checked {
                // A later entry in the page may have revoked the key that signed this one.
                if self.db.signed_by_revoked_key(&object)? {
                    warn!(%actor, "skipping an outbox entry signed by a key revoked since");
                    continue;
                }
                // The post a reshare points at, sent with this page or stored before.
                let reshared = match &payload {
                    Payload::Announce(announce) => match notes.get(&announce.object) {
                        Some(note) => Some(note.clone()),
                        None => self.stored_note(&announce.object)?,
                    },
                    _ => None,
                };
                let note = match &payload {
                    Payload::Note(note) => Some(note),
                    _ => reshared.as_ref(),
                };
                let is_new = self.store_object(&object, &payload, note, hold).await?;
                if is_new {
                    new_objects += 1;
                    budget = budget.saturating_sub(1);
                }
                match &payload {
                    Payload::Note(note) => {
                        // An author's node serves the media of their own posts.
                        for blob in note.blobs() {
                            self.db.add_host(&blob, &follow.addr)?;
                        }
                        if is_new {
                            blobs.extend(prefetchable(note, Some(&follow.addr)));
                        }
                    }
                    Payload::Announce(announce) => {
                        if let Some(note) = reshared {
                            // A resharer that hosts the media keeps track of
                            // its followers' copies too.
                            let tracker = announce.hosting.then_some(&follow.addr);
                            if let Some(addr) = tracker {
                                for blob in note.blobs() {
                                    self.db.add_host(&blob, addr)?;
                                }
                            }
                            if is_new {
                                blobs.extend(prefetchable(&note, tracker));
                            }
                        }
                    }
                    Payload::Person(_) | Payload::Key(_) => {}
                }
            }
            self.db.set_follow_synced(&follow.actor, cursor)?;
            if page_len == 0 || cursor >= latest {
                break;
            }
        }
        self.subscribe_if_due(follow).await;
        Ok((new_objects, blobs))
    }

    /// Asks a followed node again for the objects we dropped when its
    /// identity revoked a key (ADR 0010), `MAX_GET_OBJECTS` at a time. A copy
    /// is kept if it's one we asked for (its ID is the hash of its payload,
    /// which names its author) and passes `verify_received`, so one still
    /// signed by a revoked key isn't. It takes the place of the one dropped,
    /// keeping when that arrived, so it isn't counted against today's limit.
    /// What doesn't come back, such as posts made by whoever had the revoked
    /// key, stays gone.
    async fn refetch_revoked(&self, follow: &Follow) -> Result<()> {
        let actor = follow.actor.fmt_short();
        loop {
            let due = self.db.refetch_due(&follow.actor, MAX_GET_OBJECTS)?;
            if due.is_empty() {
                return Ok(());
            }
            let ids: Vec<ObjectId> = due.iter().map(|(id, _)| *id).collect();
            let req = SocialRequest::GetObjects { ids: ids.clone() };
            let SocialResponse::Objects { objects } =
                self.social_request(&follow.addr, &req).await?
            else {
                bail!("unexpected reply to GetObjects");
            };
            let received: HashMap<ObjectId, i64> = due.into_iter().collect();
            let mut restored = 0;
            for obj in objects {
                let Some(&received_ms) = received.get(&obj.id()) else {
                    warn!(%actor, "skipping an object we didn't ask for");
                    continue;
                };
                match self.verify_received(&obj) {
                    Ok(payload) => {
                        if self
                            .db
                            .insert_object_received(&obj, &payload, received_ms)?
                        {
                            restored += 1;
                        }
                    }
                    Err(e) => warn!(%actor, "skipping an invalid object: {e:#}"),
                }
            }
            self.db.refetch_done(&ids)?;
            info!(%actor, asked = ids.len(), restored, "fetched posts again after a revocation");
        }
    }

    /// Asks a followed node to notify us of new posts: after following,
    /// after a restart, and then once a day. Repeating it keeps us from
    /// being forgotten as a silent follower, and updates our address there.
    /// Best effort: polling finds new posts anyway.
    async fn subscribe_if_due(&self, follow: &Follow) {
        {
            let mut subscribed = self.lock_subscribed();
            if subscribed
                .get(&follow.actor)
                .is_some_and(|last| last.elapsed() < RESUBSCRIBE_EVERY)
            {
                return;
            }
            // Counted as done even if it fails, so a busy node isn't asked every sync.
            subscribed.insert(follow.actor, Instant::now());
        }
        let me = self.shareable_addr().await;
        if let Err(e) = self
            .social_request(
                &follow.addr,
                &SocialRequest::Subscribe {
                    addr: me,
                    shares_prefetched: self.config.swarm.share_prefetched,
                },
            )
            .await
        {
            warn!(actor = %follow.actor.fmt_short(), "subscribing failed: {e:#}");
        }
    }

    pub(crate) fn lock_subscribed(&self) -> MutexGuard<'_, HashMap<PublicKey, Instant>> {
        self.subscribed.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_holds(&self) -> MutexGuard<'_, HashMap<ObjectId, Hold>> {
        self.holds.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stores a post or reshare from a followed node; `note` is the post.
    /// If `hold`, a new one whose large media isn't here yet is kept out of
    /// the feed until it is. Returns false if we already had it.
    async fn store_object(
        &self,
        obj: &SignedObject,
        payload: &Payload,
        note: Option<&Note>,
        hold: bool,
    ) -> Result<bool> {
        let mut waiting = HashSet::new();
        if hold && let Some(note) = note {
            for blob in worth_holding_for(note) {
                if !self.store.has(&blob).await? {
                    waiting.insert(blob);
                }
            }
        }
        if waiting.is_empty() {
            return self.db.insert_object(obj, payload);
        }
        let until = now_ms() + self.sim.max_hold.as_millis() as i64;
        let is_new = self.db.insert_object_held(obj, payload, until)?;
        if is_new {
            let hold = Hold {
                waiting,
                planned: false,
            };
            self.lock_holds().insert(obj.id(), hold);
        }
        Ok(is_new)
    }

    /// Once a sync has chosen what to prefetch, shows the posts it held
    /// back that wait for nothing else (see `plan_holds`).
    fn plan_holds(&self, to_prefetch: &[Wanted]) -> Result<()> {
        let ready = plan_holds(&mut self.lock_holds(), to_prefetch);
        self.db.show_held(&ready)
    }

    /// A file fetched ahead of time is on this node, or won't be: shows
    /// the posts held back that waited only for it.
    fn prefetch_done(&self, blob: &BlobHash) {
        let ready = blob_done(&mut self.lock_holds(), blob);
        if let Err(e) = self.db.show_held(&ready) {
            warn!("couldn't show posts held back for their media: {e:#}");
        }
    }

    /// Fetches media ahead of time, one file at a time, in the order given.
    /// A large file this node gets through the follower swarm waits a
    /// random while first (`PREFETCH_SPREAD`), and one refused as busy is
    /// tried again later, both in `later_loop`.
    async fn prefetch(&self, wanted: Vec<Wanted>) {
        for wanted in wanted {
            // One already here, as when a post we have is reshared, isn't
            // spread out: there's nothing to fetch, only trackers to tell.
            let here = self.store.has(&wanted.blob).await.unwrap_or(false);
            if self.joins_swarm(&wanted) && !here {
                let wait = self.sim.prefetch_spread.mul_f64(random_fraction());
                self.fetch_later(Later {
                    wanted,
                    due: Instant::now() + wait,
                    refusals: 0,
                });
            } else if let Some(later) = self.fetch_ahead(wanted, 0).await {
                self.fetch_later(later);
            }
        }
    }

    /// Whether this node gets `wanted` through the follower swarm: it shares
    /// what it prefetches, the file is large, and it follows a node that
    /// knows the followers holding a copy: the author's, or a resharer's
    /// that hosts it.
    fn joins_swarm(&self, wanted: &Wanted) -> bool {
        self.config.swarm.share_prefetched
            && wanted.size > store::SMALL_BLOB
            && !wanted.trackers.is_empty()
    }

    /// Fetches one file ahead of time, refused as busy `refusals` times so
    /// far. If the nodes with it are busy again, says when to try again,
    /// unless that has happened `MAX_RETRIES` times. Otherwise the file is
    /// here or won't be, and posts held back for it are shown.
    async fn fetch_ahead(&self, wanted: Wanted, refusals: u32) -> Option<Later> {
        let hash = wanted.blob;
        let later = self.try_fetch_ahead(wanted, refusals).await;
        if later.is_none() {
            self.prefetch_done(&hash);
        }
        later
    }

    async fn try_fetch_ahead(&self, wanted: Wanted, refusals: u32) -> Option<Later> {
        let blob = wanted.blob.fmt_short();
        let fetched = if self.joins_swarm(&wanted) {
            self.fetch_from_swarm(&wanted).await
        } else {
            self.ensure_blob(&wanted.blob, BlobClass::Prefetch, wanted.size)
                .await
        };
        match fetched {
            Ok(_) => {
                if refusals > 0 {
                    info!(%blob, refusals, "prefetched after the nodes with it were busy");
                }
                None
            }
            Err(e) if store::is_busy(&e) && refusals < MAX_RETRIES => {
                info!(%blob, "the nodes with this media are busy; trying again later");
                let wait = jitter(retry_wait(self.sim.first_retry_wait, refusals));
                Some(Later {
                    wanted,
                    due: Instant::now() + wait,
                    refusals: refusals + 1,
                })
            }
            Err(e) => {
                warn!(%blob, "prefetch failed; it's fetched when it's opened: {e:#}");
                None
            }
        }
    }

    fn fetch_later(&self, later: Later) {
        if let Err(e) = self.prefetch_later.try_send(later) {
            warn!("too many prefetches waiting; the rest are fetched when they're opened");
            self.prefetch_done(&e.into_inner().wanted.blob);
        }
    }

    /// Fetches a large file through the follower swarm (doc 13): asks the
    /// trackers who holds a copy, tries the followers they name first, the
    /// nodes known to host it and the trackers after them, and the author's
    /// own nodes last, then tells the trackers that this node holds a copy
    /// too.
    async fn fetch_from_swarm(&self, wanted: &Wanted) -> Result<u64> {
        let blob = &wanted.blob;
        let mut providers = Vec::new();
        if self.store.stored_size(blob).await?.is_none() {
            for tracker in &wanted.trackers {
                for id in self.named_hosts(tracker, blob).await {
                    if !providers.contains(&id) {
                        providers.push(id);
                    }
                }
            }
            // A crowd asks these first, so they come last.
            let trackers = wanted.trackers.iter().map(|t| t.id);
            for id in self.known_providers(blob)?.into_iter().chain(trackers) {
                if !providers.contains(&id) {
                    providers.push(id);
                }
            }
            // The author's own nodes, which a resharer names too, after
            // everyone else: they're the ones to spare.
            let authors = self.nodes_of(&wanted.author);
            providers.sort_by_key(|id| authors.contains(id));
        }
        let size = self
            .store
            .download(blob, providers, BlobClass::Prefetch, wanted.size)
            .await?;
        for tracker in &wanted.trackers {
            let told = self
                .social_request(tracker, &SocialRequest::Hosting { blob: *blob })
                .await;
            if let Err(e) = told {
                debug!(blob = %blob.fmt_short(), "couldn't tell a tracker we hold a copy: {e:#}");
            }
        }
        Ok(size)
    }

    /// The home nodes `actor`'s profile lists, if we have it.
    fn nodes_of(&self, actor: &PublicKey) -> Vec<EndpointId> {
        if let Ok(Some(profile)) = self.db.profile(actor)
            && let Ok(Payload::Person(person)) = profile.verify()
        {
            return person.nodes;
        }
        Vec::new()
    }

    /// The nodes `node` names as hosts of `blob`, other than itself and
    /// this node. An author's node names followers holding a copy to its
    /// followers (doc 13); they're used for this download only (see
    /// `ask_who_hosts`).
    async fn named_hosts(&self, node: &EndpointAddr, blob: &BlobHash) -> Vec<EndpointId> {
        let mut named = Vec::new();
        let req = SocialRequest::WhoHosts { blob: *blob };
        if let Ok(SocialResponse::Hosts { nodes }) = self.social_request(node, &req).await {
            let me = self.endpoint.id();
            let others = nodes
                .iter()
                .filter(|a| a.id != me && a.id != node.id && check_addr(a).is_ok())
                .take(MAX_HOSTS_PER_REPLY);
            for addr in others {
                remember(&self.lookup, addr);
                if !named.contains(&addr.id) {
                    named.push(addr.id);
                }
            }
        }
        named
    }

    /// Makes sure a blob is stored locally, downloading at most `declared`
    /// bytes of it if needed, and records it under at least `class`. If the
    /// last try failed because a node with it was busy, the error says so
    /// (`store::is_busy`).
    pub(crate) async fn ensure_blob(
        &self,
        blob: &BlobHash,
        class: BlobClass,
        declared: u64,
    ) -> Result<u64> {
        let providers = if self.store.has(blob).await? {
            Vec::new()
        } else if class == BlobClass::Cache {
            // Opened on demand, on one of our devices or with `osp fetch`.
            self.providers_asking_first(blob).await?
        } else {
            self.known_providers(blob)?
        };
        match self.store.download(blob, providers, class, declared).await {
            Ok(size) => Ok(size),
            Err(first_error) => {
                // Ask the nodes we follow who else has it, then try again.
                let found = self.ask_who_hosts(blob).await;
                if found.is_empty() {
                    return Err(first_error);
                }
                self.store.download(blob, found, class, declared).await
            }
        }
    }

    /// Fetches media we promised to host by resharing with hosting, but
    /// don't have: after a restore, which leaves replicas out (ADR 0009).
    /// Within the replica quota, like any replica. Runs once at startup.
    pub(crate) async fn refetch_replicas(&self) {
        let Ok(outbox) = self.db.outbox_since(0, u32::MAX) else {
            return;
        };
        for (_, obj) in outbox {
            let Ok(Payload::Announce(announce)) = obj.verify() else {
                continue;
            };
            if !announce.hosting {
                continue;
            }
            let Ok(Some(note)) = self.stored_note(&announce.object) else {
                continue;
            };
            for (blob, size) in note.sized_blobs() {
                let held = self.db.blob_class(&blob).ok().flatten();
                if held.is_some_and(|c| c.is_public()) {
                    continue;
                }
                match self.ensure_blob(&blob, BlobClass::Replica, size).await {
                    Ok(_) => info!(blob = %blob.fmt_short(), "fetched media we host again"),
                    Err(e) => warn!(blob = %blob.fmt_short(), "media we host is missing: {e:#}"),
                }
            }
        }
    }

    /// Nodes to fetch media opened on demand from, when it wasn't fetched
    /// ahead of time: first the hosts named by the nodes we follow
    /// among those known to serve it, such as its author's, which may name
    /// followers with a copy, as a prefetch through the swarm does; then
    /// the nodes known to serve it. So everyone who opens a post doesn't go
    /// to its author.
    async fn providers_asking_first(&self, blob: &BlobHash) -> Result<Vec<EndpointId>> {
        let known = self.known_providers(blob)?;
        let mut providers = Vec::new();
        for follow in self.db.follows()? {
            if !known.contains(&follow.addr.id) {
                continue;
            }
            // Someone is waiting: a node that doesn't answer at once is
            // only tried for the download, with the rest.
            let ask = self.named_hosts(&follow.addr, blob);
            let named = tokio::time::timeout(ASK_FIRST_WAIT, ask)
                .await
                .unwrap_or_default();
            for id in named {
                if !known.contains(&id) && !providers.contains(&id) {
                    providers.push(id);
                }
            }
        }
        providers.extend(known);
        Ok(providers)
    }

    /// Nodes believed to serve `blob`, most recently seen first.
    fn known_providers(&self, blob: &BlobHash) -> Result<Vec<EndpointId>> {
        let me = self.endpoint.id();
        let mut ids = Vec::new();
        for addr in self.db.hosts(blob)? {
            if addr.id == me || ids.contains(&addr.id) {
                continue;
            }
            remember(&self.lookup, &addr);
            ids.push(addr.id);
        }
        Ok(ids)
    }

    /// Asks the nodes we follow who else hosts `blob`. Returns the nodes they
    /// name, then those that answered and those we knew of, which were
    /// likely asked already. What they name is used for this download only,
    /// not remembered: an author's node names followers holding a copy only
    /// to its followers (doc 13), and we mustn't pass them on.
    async fn ask_who_hosts(&self, blob: &BlobHash) -> Vec<EndpointId> {
        let me = self.endpoint.id();
        let mut found = Vec::new();
        let mut answered = Vec::new();
        for follow in self.db.follows().unwrap_or_default() {
            let req = SocialRequest::WhoHosts { blob: *blob };
            if let Ok(SocialResponse::Hosts { nodes }) =
                self.social_request(&follow.addr, &req).await
            {
                let usable = nodes
                    .iter()
                    .filter(|a| a.id != me && check_addr(a).is_ok())
                    .take(MAX_HOSTS_PER_REPLY);
                for addr in usable {
                    remember(&self.lookup, addr);
                    let list = if addr.id == follow.addr.id {
                        &mut answered
                    } else {
                        &mut found
                    };
                    if !list.contains(&addr.id) {
                        list.push(addr.id);
                    }
                }
            }
        }
        let known = self.known_providers(blob).unwrap_or_default();
        for id in answered.into_iter().chain(known) {
            if !found.contains(&id) {
                found.push(id);
            }
        }
        found
    }

    /// A Note we have stored, verified.
    pub(crate) fn stored_note(&self, id: &ObjectId) -> Result<Option<Note>> {
        match self.db.get_object(id)? {
            Some(obj) => match obj.verify()? {
                Payload::Note(note) => Ok(Some(note)),
                _ => Ok(None),
            },
            None => Ok(None),
        }
    }
}

/// Media of a new post worth fetching ahead of time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wanted {
    blob: BlobHash,
    /// The size its post declares.
    size: u64,
    /// Who made the post.
    author: PublicKey,
    /// Followed nodes that know which of their followers hold a copy (doc
    /// 13): the author's, when the post came from its outbox, and those of
    /// resharers that host it.
    trackers: Vec<EndpointAddr>,
}

impl Wanted {
    /// Adds the trackers of the same file wanted again, each once.
    fn add_trackers(&mut self, trackers: Vec<EndpointAddr>) {
        for tracker in trackers {
            if !self.trackers.iter().any(|t| t.id == tracker.id) {
                self.trackers.push(tracker);
            }
        }
    }
}

/// A post held back from the feed (`Db::insert_object_held`) until the
/// files it waits for are on this node, or won't be.
#[derive(Debug)]
pub(crate) struct Hold {
    waiting: HashSet<BlobHash>,
    /// Whether the sync it came with has chosen what to prefetch. Until
    /// then, `waiting` may hold files that won't be.
    planned: bool,
}

/// Stops holding back the posts a sync brought for files it won't
/// prefetch: over the limits of `choose_prefetch`, or from a followed node
/// whose sync failed partway. Returns the posts that wait for nothing else.
fn plan_holds(holds: &mut HashMap<ObjectId, Hold>, to_prefetch: &[Wanted]) -> Vec<ObjectId> {
    let chosen: HashSet<BlobHash> = to_prefetch.iter().map(|w| w.blob).collect();
    for hold in holds.values_mut().filter(|hold| !hold.planned) {
        hold.waiting.retain(|blob| chosen.contains(blob));
        hold.planned = true;
    }
    take_ready(holds)
}

/// Stops holding back posts for `blob`, which is here or won't be.
/// Returns the posts that wait for nothing else.
fn blob_done(holds: &mut HashMap<ObjectId, Hold>, blob: &BlobHash) -> Vec<ObjectId> {
    for hold in holds.values_mut() {
        hold.waiting.remove(blob);
    }
    take_ready(holds)
}

/// Takes the posts held back that wait for nothing any more.
fn take_ready(holds: &mut HashMap<ObjectId, Hold>) -> Vec<ObjectId> {
    let ready: Vec<ObjectId> = holds
        .iter()
        .filter(|(_, hold)| hold.waiting.is_empty())
        .map(|(id, _)| *id)
        .collect();
    for id in &ready {
        holds.remove(id);
    }
    ready
}

/// A file to fetch ahead of time once it's `due`: spread out, or refused as
/// busy `refusals` times so far.
#[derive(Debug)]
pub(crate) struct Later {
    wanted: Wanted,
    due: Instant,
    refusals: u32,
}

/// Fetches, one at a time, the files `prefetch` puts off until later.
/// Runs until the node stops.
pub(crate) async fn later_loop(core: Arc<Core>, mut later: mpsc::Receiver<Later>) {
    let mut waiting: HashMap<BlobHash, Later> = HashMap::new();
    loop {
        let next = waiting.values().map(|l| l.due).min();
        tokio::select! {
            got = later.recv() => {
                let Some(l) = got else { return };
                if let Some(dropped) = wait_for(&mut waiting, l) {
                    // It's fetched when it's opened.
                    core.prefetch_done(&dropped);
                }
            }
            _ = sleep_until(next) => {
                let now = Instant::now();
                let due: Vec<BlobHash> = waiting
                    .iter()
                    .filter(|(_, l)| l.due <= now)
                    .map(|(blob, _)| *blob)
                    .collect();
                for blob in due {
                    let Some(l) = waiting.remove(&blob) else {
                        continue;
                    };
                    if let Some(again) = core.fetch_ahead(l.wanted, l.refusals).await {
                        waiting.insert(blob, again);
                    }
                }
            }
        }
    }
}

/// Adds a file to those waiting, within `MAX_WAITING`. One already waiting
/// keeps its time, and gains the new one's trackers, as when a post spread
/// out is then reshared by a node that hosts it. Returns the file if it's
/// dropped.
fn wait_for(waiting: &mut HashMap<BlobHash, Later>, l: Later) -> Option<BlobHash> {
    let blob = l.wanted.blob;
    if let Some(earlier) = waiting.get_mut(&blob) {
        earlier.wanted.add_trackers(l.wanted.trackers);
    } else if waiting.len() < MAX_WAITING {
        waiting.insert(blob, l);
    } else {
        return Some(blob);
    }
    None
}

/// How long to wait before trying again a prefetch refused as busy
/// `refusals` times before this one, before `jitter`.
fn retry_wait(first: Duration, refusals: u32) -> Duration {
    first
        .saturating_mul(1 << refusals.min(16))
        .min(MAX_RETRY_WAIT)
}

/// Waits until `at`, or forever if there's nothing to wait for.
async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

/// `d` varied at random by up to half either way.
fn jitter(d: Duration) -> Duration {
    d.mul_f64(0.5 + random_fraction())
}

/// A random number from 0 to 1.
fn random_fraction() -> f64 {
    let mut bytes = [0; 8];
    // Without random numbers, the middle is as good as anything.
    match getrandom::fill(&mut bytes) {
        Ok(()) => u64::from_le_bytes(bytes) as f64 / u64::MAX as f64,
        Err(_) => 0.5,
    }
}

/// A post's media worth fetching ahead of time: thumbnails always, media
/// files up to `MAX_PREFETCH_BYTES`. `tracker` is the followed node it came
/// from, if that node keeps track of its followers' copies.
fn prefetchable(note: &Note, tracker: Option<&EndpointAddr>) -> Vec<Wanted> {
    let mut wanted = Vec::new();
    let mut add = |blob, size| {
        wanted.push(Wanted {
            blob,
            size,
            author: note.actor,
            trackers: tracker.into_iter().cloned().collect(),
        })
    };
    for a in &note.attachment {
        if a.size <= MAX_PREFETCH_BYTES {
            add(a.blob, a.size);
        }
        if let Some(thumb) = &a.thumbnail {
            add(thumb.blob, thumb.size);
        }
    }
    wanted
}

/// The files of a post worth holding it back from the feed for: large ones
/// fetched ahead of time. Small files come within seconds, not spread out.
fn worth_holding_for(note: &Note) -> Vec<BlobHash> {
    prefetchable(note, None)
        .into_iter()
        .filter(|wanted| wanted.size > store::SMALL_BLOB)
        .map(|wanted| wanted.blob)
        .collect()
}

/// Picks what one sync prefetches, within the per-sync limits, smallest
/// first, so thumbnails aren't held up behind videos. A file wanted twice,
/// as when a followed author's post is also reshared, is kept once, with
/// the trackers of both.
fn choose_prefetch(mut candidates: Vec<Wanted>) -> Vec<Wanted> {
    candidates.sort_by_key(|w| w.blob);
    // `dedup_by` hands over the later of two neighbours first.
    candidates.dedup_by(|later, kept| {
        if later.blob != kept.blob {
            return false;
        }
        kept.add_trackers(std::mem::take(&mut later.trackers));
        true
    });
    candidates.sort_by_key(|w| w.size);
    let mut chosen = Vec::new();
    let mut bytes = 0;
    for wanted in candidates {
        if chosen.len() == MAX_PREFETCH_FILES_PER_SYNC {
            break;
        }
        if bytes + wanted.size > MAX_PREFETCH_BYTES_PER_SYNC {
            continue;
        }
        bytes += wanted.size;
        chosen.push(wanted);
    }
    chosen
}

#[cfg(test)]
mod tests {
    use osp_proto::{Attachment, BlobRef, MediaKind};

    use super::*;

    #[test]
    fn prefetches_go_smallest_first_each_once() {
        let [video, thumb, photo] = [1u8, 2, 3].map(|b| BlobHash::from_bytes([b; 32]));
        let author = iroh::SecretKey::generate().public();
        let [authors_node, resharer] =
            [(); 2].map(|_| EndpointAddr::new(iroh::SecretKey::generate().public()));
        let wanted = |blob, size, trackers: &[&EndpointAddr]| Wanted {
            blob,
            size,
            author,
            trackers: trackers.iter().map(|&t| t.clone()).collect(),
        };
        let chosen = choose_prefetch(vec![
            // Reshared by a node that hosts it, then the author's own post,
            // then reshared without hosting.
            wanted(video, 50 << 20, &[&resharer]),
            wanted(thumb, 37 << 10, &[&authors_node]),
            wanted(video, 50 << 20, &[&authors_node]),
            wanted(photo, 2 << 20, &[]),
            wanted(video, 50 << 20, &[]),
        ]);
        assert_eq!(chosen.len(), 3);
        assert_eq!(chosen[0], wanted(thumb, 37 << 10, &[&authors_node]));
        assert_eq!(chosen[1], wanted(photo, 2 << 20, &[]));
        let trackers: HashSet<EndpointId> = chosen[2].trackers.iter().map(|t| t.id).collect();
        assert_eq!(chosen[2].blob, video);
        assert_eq!(trackers, HashSet::from([authors_node.id, resharer.id]));
    }

    #[test]
    fn held_posts_wait_only_for_large_files_fetched_ahead_of_time() {
        let [video, thumb, photo, huge] = [1u8, 2, 3, 4].map(|b| BlobHash::from_bytes([b; 32]));
        let attachment = |blob, size, thumbnail: Option<(BlobHash, u64)>| Attachment {
            kind: MediaKind::Video,
            media_type: "video/mp4".into(),
            blob,
            size,
            width: 1,
            height: 1,
            duration_ms: None,
            name: None,
            thumbnail: thumbnail.map(|(blob, size)| BlobRef {
                blob,
                size,
                media_type: "image/webp".into(),
            }),
            profile: "v1-720p".into(),
        };
        let note = Note {
            actor: iroh::SecretKey::generate().public(),
            content: String::new(),
            published: osp_proto::now_rfc3339(),
            attachment: vec![
                attachment(video, 50 << 20, Some((thumb, 37 << 10))),
                attachment(photo, 1 << 20, None),
                // Not prefetched: fetched when it's opened.
                attachment(huge, MAX_PREFETCH_BYTES + 1, None),
            ],
            in_reply_to: None,
        };
        assert_eq!(worth_holding_for(&note), vec![video]);
    }

    #[test]
    fn held_posts_wait_only_for_files_still_to_be_prefetched() {
        let [video, photo, skipped] = [1u8, 2, 3].map(|b| BlobHash::from_bytes([b; 32]));
        let [earlier, both, too_big] = [b"a", b"b", b"c"].map(|p| ObjectId::of(p));
        let hold = |files: &[BlobHash], planned| Hold {
            waiting: files.iter().copied().collect(),
            planned,
        };
        let mut holds = HashMap::from([
            (earlier, hold(&[video], true)),
            (both, hold(&[video, photo], false)),
            (too_big, hold(&[skipped], false)),
        ]);
        // One of a post's files is done; it still waits for the other.
        assert_eq!(blob_done(&mut holds, &photo), vec![]);
        // The sync chose the video (waiting since an earlier sync too) and
        // the photo, but not the third file.
        let chosen = [video, photo].map(|blob| Wanted {
            blob,
            size: 2 << 20,
            author: iroh::SecretKey::generate().public(),
            trackers: Vec::new(),
        });
        assert_eq!(plan_holds(&mut holds, &chosen), vec![too_big]);
        let ready: HashSet<ObjectId> = blob_done(&mut holds, &video).into_iter().collect();
        assert_eq!(ready, HashSet::from([earlier, both]));
        assert!(holds.is_empty());
    }

    #[test]
    fn a_file_waiting_gains_the_trackers_of_a_later_reshare() {
        let [video, other] = [1u8, 2].map(|b| BlobHash::from_bytes([b; 32]));
        let [authors_node, resharer] =
            [(); 2].map(|_| EndpointAddr::new(iroh::SecretKey::generate().public()));
        let author = iroh::SecretKey::generate().public();
        let soon = Instant::now() + Duration::from_secs(60);
        let later = |blob, tracker: &EndpointAddr, due| Later {
            wanted: Wanted {
                blob,
                size: 50 << 20,
                author,
                trackers: vec![tracker.clone()],
            },
            due,
            refusals: 0,
        };
        let mut waiting = HashMap::new();
        assert_eq!(
            wait_for(&mut waiting, later(video, &authors_node, soon)),
            None
        );
        // Reshared while it waits: it keeps its time, and both are told.
        let much_later = soon + Duration::from_secs(600);
        assert_eq!(
            wait_for(&mut waiting, later(video, &resharer, much_later)),
            None
        );
        assert_eq!(
            wait_for(&mut waiting, later(video, &resharer, much_later)),
            None
        );
        let waited = &waiting[&video];
        assert_eq!(waited.due, soon);
        let ids: Vec<EndpointId> = waited.wanted.trackers.iter().map(|t| t.id).collect();
        assert_eq!(ids, [authors_node.id, resharer.id]);

        // When the queue is full, a new file is dropped, but one waiting
        // still gains trackers.
        for b in 0..MAX_WAITING - 1 {
            let mut bytes = [3; 32];
            bytes[..8].copy_from_slice(&(b as u64).to_le_bytes());
            let blob = BlobHash::from_bytes(bytes);
            assert_eq!(wait_for(&mut waiting, later(blob, &resharer, soon)), None);
        }
        assert_eq!(waiting.len(), MAX_WAITING);
        assert_eq!(
            wait_for(&mut waiting, later(other, &resharer, soon)),
            Some(other)
        );
        let third = EndpointAddr::new(iroh::SecretKey::generate().public());
        assert_eq!(wait_for(&mut waiting, later(video, &third, soon)), None);
        assert_eq!(waiting[&video].wanted.trackers.len(), 3);
    }

    #[test]
    fn retries_wait_longer_each_time_up_to_a_limit() {
        let minute = Duration::from_secs(60);
        let waits: Vec<_> = (0..MAX_RETRIES).map(|t| retry_wait(minute, t)).collect();
        assert_eq!(waits, [1, 2, 4, 8, 15, 15].map(|m| minute * m));
        assert_eq!(retry_wait(minute, u32::MAX), MAX_RETRY_WAIT);
        for _ in 0..100 {
            let varied = jitter(minute);
            assert!(
                varied >= minute / 2 && varied <= minute * 3 / 2,
                "{varied:?}"
            );
        }
    }
}
