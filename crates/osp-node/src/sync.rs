//! Gathering: syncing followed outboxes, prefetching their media, and finding
//! nodes that can serve a blob.
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
use iroh::{EndpointId, PublicKey};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_proto::{
    BlobHash, Note, ObjectId, Payload,
    social::{MAX_GET_OBJECTS, MAX_OUTBOX_PAGE, SocialRequest, SocialResponse},
};
use tracing::{debug, info, warn};

use crate::{
    db::{BlobClass, Follow, now_ms},
    node::{Core, check_addr, remember},
    social::profile_date,
};

/// How often we ask each followed node to keep notifying us.
const RESUBSCRIBE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Hosts taken from one "who hosts" reply. Enough to find a copy.
const MAX_HOSTS_PER_REPLY: usize = 20;
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
    /// of new objects, and the media worth prefetching with declared sizes.
    async fn sync_follow(&self, follow: &Follow) -> Result<(usize, Vec<(BlobHash, u64)>)> {
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
                        if self.db.insert_object(&obj, &payload)? {
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
                let is_new = self.db.insert_object(&object, &payload)?;
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
                            blobs.extend(prefetchable(note));
                        }
                    }
                    Payload::Announce(announce) => {
                        let note = match notes.get(&announce.object) {
                            Some(note) => Some(note.clone()),
                            None => self.stored_note(&announce.object)?,
                        };
                        if let Some(note) = note {
                            if announce.hosting {
                                for blob in note.blobs() {
                                    self.db.add_host(&blob, &follow.addr)?;
                                }
                            }
                            if is_new {
                                blobs.extend(prefetchable(&note));
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
            .social_request(&follow.addr, &SocialRequest::Subscribe { addr: me })
            .await
        {
            warn!(actor = %follow.actor.fmt_short(), "subscribing failed: {e:#}");
        }
    }

    pub(crate) fn lock_subscribed(&self) -> MutexGuard<'_, HashMap<PublicKey, Instant>> {
        self.subscribed.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn prefetch(&self, blobs: Vec<(BlobHash, u64)>) {
        for (blob, size) in blobs {
            if let Err(e) = self.ensure_blob(&blob, BlobClass::Cache, size).await {
                warn!(blob = %blob.fmt_short(), "prefetch failed: {e:#}");
            }
        }
    }

    /// Makes sure a blob is stored locally, downloading at most `declared`
    /// bytes of it if needed, and records it under at least `class`.
    pub(crate) async fn ensure_blob(
        &self,
        blob: &BlobHash,
        class: BlobClass,
        declared: u64,
    ) -> Result<u64> {
        let providers = if self.store.has(blob).await? {
            Vec::new()
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

    async fn ask_who_hosts(&self, blob: &BlobHash) -> Vec<EndpointId> {
        let me = self.endpoint.id();
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
                    self.db.add_host(blob, addr).ok();
                }
            }
        }
        self.known_providers(blob).unwrap_or_default()
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

/// A post's media worth fetching ahead of time, with declared sizes:
/// thumbnails always, media files up to `MAX_PREFETCH_BYTES`.
fn prefetchable(note: &Note) -> Vec<(BlobHash, u64)> {
    let mut blobs = Vec::new();
    for a in &note.attachment {
        if a.size <= MAX_PREFETCH_BYTES {
            blobs.push((a.blob, a.size));
        }
        if let Some(thumb) = &a.thumbnail {
            blobs.push((thumb.blob, thumb.size));
        }
    }
    blobs
}

/// Picks what one sync prefetches, within the per-sync limits.
fn choose_prefetch(mut candidates: Vec<(BlobHash, u64)>) -> Vec<(BlobHash, u64)> {
    candidates.sort();
    candidates.dedup_by_key(|(blob, _)| *blob);
    let mut chosen = Vec::new();
    let mut bytes = 0;
    for (blob, size) in candidates {
        if chosen.len() == MAX_PREFETCH_FILES_PER_SYNC {
            break;
        }
        if bytes + size > MAX_PREFETCH_BYTES_PER_SYNC {
            continue;
        }
        bytes += size;
        chosen.push((blob, size));
    }
    chosen
}
