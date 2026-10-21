//! Gathering: syncing followed outboxes, prefetching their media, and finding
//! nodes that can serve a blob.

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Result, bail, ensure};
use iroh::EndpointId;
use osp_proto::{
    BlobHash, Note, ObjectId, Payload,
    social::{MAX_OUTBOX_PAGE, SocialRequest, SocialResponse},
};
use tracing::{info, warn};

use crate::{
    db::{BlobClass, Follow},
    node::{Core, remember},
};

/// Polls followed nodes on the configured interval, or sooner when woken.
pub(crate) async fn run_loop(core: Arc<Core>) {
    let interval = Duration::from_secs(core.config.sync.interval_secs.max(5));
    loop {
        if let Err(e) = core.sync_all(false).await {
            warn!("sync failed: {e:#}");
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = core.sync_now.notified() => {}
        }
    }
}

impl Core {
    /// Syncs every follow. Returns how many new objects arrived. With
    /// `wait_for_prefetch`, also waits until their media is downloaded.
    pub(crate) async fn sync_all(self: &Arc<Self>, wait_for_prefetch: bool) -> Result<usize> {
        let mut new_objects = 0;
        {
            let _guard = self.sync_lock.lock().await;
            let mut to_prefetch = Vec::new();
            for follow in self.db.follows()? {
                match self.sync_follow(&follow).await {
                    Ok((n, blobs)) => {
                        new_objects += n;
                        to_prefetch.extend(blobs);
                    }
                    Err(e) => {
                        warn!(actor = %follow.actor.fmt_short(), "sync failed: {e:#}");
                        self.db.set_follow_error(&follow.actor, &format!("{e:#}"))?;
                    }
                }
            }
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
    /// of new objects and the blobs worth prefetching.
    async fn sync_follow(&self, follow: &Follow) -> Result<(usize, Vec<BlobHash>)> {
        // The profile gives us a display name and proves the node speaks for this identity.
        match self
            .social_request(&follow.addr, &SocialRequest::GetProfile)
            .await?
        {
            SocialResponse::Profile { profile } => {
                let Payload::Person(person) = profile.verify()? else {
                    bail!("profile is not a Person");
                };
                ensure!(
                    person.actor == follow.actor,
                    "node {} speaks for a different identity",
                    follow.addr.id.fmt_short()
                );
                self.db
                    .upsert_profile(&person.actor, &person.name, &profile)?;
            }
            _ => bail!("unexpected reply to GetProfile"),
        }

        let mut cursor = follow.cursor;
        let mut new_objects = 0;
        let mut blobs = Vec::new();
        loop {
            let req = SocialRequest::GetOutbox {
                since: cursor,
                limit: MAX_OUTBOX_PAGE,
            };
            let SocialResponse::Outbox {
                entries,
                referenced,
                latest,
            } = self.social_request(&follow.addr, &req).await?
            else {
                bail!("unexpected reply to GetOutbox");
            };

            // Store referenced objects first, so reshares in this page can be resolved.
            let mut notes: HashMap<ObjectId, Note> = HashMap::new();
            for obj in referenced {
                let Ok(payload) = obj.verify() else {
                    warn!("dropping referenced object with a bad signature");
                    continue;
                };
                if let Payload::Person(person) = &payload {
                    // Second-hand, possibly stale: only use it if we know nothing better.
                    if self.db.profile(&person.actor)?.is_none() {
                        self.db.upsert_profile(&person.actor, &person.name, &obj)?;
                    }
                    continue;
                }
                if self.db.insert_object(&obj, &payload)? {
                    new_objects += 1;
                }
                if let Payload::Note(note) = payload {
                    notes.insert(obj.id(), note);
                }
            }

            let page_len = entries.len();
            for entry in entries {
                let payload = entry.object.verify()?;
                ensure!(
                    payload.actor() == follow.actor,
                    "outbox entry signed by someone else"
                );
                let is_new = self.db.insert_object(&entry.object, &payload)?;
                if is_new {
                    new_objects += 1;
                }
                match &payload {
                    Payload::Note(note) => {
                        // An author's node serves the media of their own posts.
                        for blob in note.blobs() {
                            self.db.add_host(&blob, &follow.addr)?;
                        }
                        if is_new {
                            blobs.extend(note.blobs());
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
                                blobs.extend(note.blobs());
                            }
                        }
                    }
                    Payload::Person(_) => {}
                }
                cursor = cursor.max(entry.seq);
            }
            self.db.set_follow_synced(&follow.actor, cursor)?;
            if page_len == 0 || cursor >= latest {
                break;
            }
        }
        Ok((new_objects, blobs))
    }

    async fn prefetch(&self, mut blobs: Vec<BlobHash>) {
        blobs.sort();
        blobs.dedup();
        for blob in blobs {
            if let Err(e) = self.ensure_blob(&blob, BlobClass::Cache).await {
                warn!(blob = %blob.fmt_short(), "prefetch failed: {e:#}");
            }
        }
    }

    /// Makes sure a blob is stored locally, downloading it if needed, and
    /// records it under at least `class`.
    pub(crate) async fn ensure_blob(&self, blob: &BlobHash, class: BlobClass) -> Result<u64> {
        let providers = if self.store.has(blob).await? {
            Vec::new()
        } else {
            self.known_providers(blob)?
        };
        match self.store.download(blob, providers, class).await {
            Ok(size) => Ok(size),
            Err(first_error) => {
                // Ask the nodes we follow who else has it, then try again.
                let found = self.ask_who_hosts(blob).await;
                if found.is_empty() {
                    return Err(first_error);
                }
                self.store.download(blob, found, class).await
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
                for addr in nodes.iter().filter(|a| a.id != me) {
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
