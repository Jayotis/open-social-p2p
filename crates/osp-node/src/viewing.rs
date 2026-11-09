//! Watching a found post without showing who you are (ADR 0024).
//!
//! Alice found Kim's post through a directory and follows nobody. Her node
//! fetches its media from a viewing endpoint: a new key, relay-only, never
//! bound to her public ID and never named to anyone. Kim's node sees an
//! endpoint it doesn't know fetch a public file, as any stranger's.
//!
//! This is a path of its own, not the usual one with a flag: nothing here
//! asks the nodes Alice follows, sends `WhoHosts` over Tor, or connects from
//! her public, media or direct endpoint. When no public source can be
//! reached this way, the fetch fails and says why; it never fills in from
//! her own identity.
//!
//! Not anonymity: the relay sees her node's IP address and what it fetched,
//! as for every relayed transfer.

use std::{
    pin::pin,
    sync::{
        Arc, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use iroh::{Endpoint, PublicKey, SecretKey};
use iroh_blobs::util::connection_pool::ConnectionPool;
use osp_proto::{BlobHash, Note};
use tokio::{sync::Notify, time::Instant};
use tracing::{debug, info};

use crate::{
    db::BlobClass,
    node::{Core, endpoint_builder_unpublished, on_simulated_network},
    relays::{self, RelayPolicy},
    store::{self, Source, Sources},
};

/// How long a viewing endpoint takes new requests, from when it was made. A
/// starting value. After that, anything new from that author gets a new key.
/// A transfer under way goes on past it.
pub(crate) const REUSE_FOR: Duration = Duration::from_secs(10 * 60);

/// Longest a viewing endpoint lives, whatever it's doing.
pub(crate) const MAX_LIFE: Duration = Duration::from_secs(60 * 60);

/// Most viewing endpoints at once. A starting value. Another author's post
/// waits for one to be free, rather than sharing one.
pub(crate) const MAX_VIEWING: usize = 4;

/// What the person is told when no public source could be reached privately.
pub(crate) const NO_PRIVATE_SOURCE: &str = "no public source of this post could be reached \
     without showing who you are; try again later, or follow its author";

/// One viewing endpoint: its key is new, and it connects only through the
/// relay.
pub(crate) struct ViewingEndpoint {
    pub endpoint: Endpoint,
    pub pool: ConnectionPool,
}

struct Slot {
    author: PublicKey,
    made: Instant,
    /// Fetches using it now.
    users: Arc<AtomicUsize>,
    inner: Arc<ViewingEndpoint>,
}

impl Slot {
    fn takes_new_requests(&self, reuse_for: Duration) -> bool {
        self.made.elapsed() < reuse_for
    }

    fn in_use(&self) -> bool {
        self.users.load(Ordering::Acquire) > 0
    }
}

/// The node's viewing endpoints.
pub(crate) struct Viewing {
    slots: tokio::sync::Mutex<Vec<Slot>>,
    freed: Notify,
    made: AtomicUsize,
    reuse_for: Duration,
    max_life: Duration,
}

/// A fetch's hold on a viewing endpoint. Dropping it lets the endpoint go
/// once nothing else uses it.
pub(crate) struct Lease<'a> {
    viewing: &'a Viewing,
    users: Arc<AtomicUsize>,
    pub inner: Arc<ViewingEndpoint>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.users.fetch_sub(1, Ordering::AcqRel);
        self.viewing.freed.notify_waiters();
    }
}

impl Viewing {
    pub fn new(reuse_for: Duration, max_life: Duration) -> Self {
        Self {
            slots: tokio::sync::Mutex::new(Vec::new()),
            freed: Notify::new(),
            made: AtomicUsize::new(0),
            reuse_for,
            max_life,
        }
    }

    /// How many viewing endpoints have been made since the node started.
    pub fn made(&self) -> usize {
        self.made.load(Ordering::Relaxed)
    }

    /// The viewing endpoint for a new request for `author`'s media: the one
    /// made for them in the last `reuse_for`, or a new one. With
    /// `MAX_VIEWING` in use for others, waits for one to be free.
    async fn lease<F>(&self, author: PublicKey, make: impl Fn() -> F) -> Result<Lease<'_>>
    where
        F: Future<Output = Result<Endpoint>>,
    {
        loop {
            // Registered before looking, so a lease dropped meanwhile wakes it.
            let mut freed = pin!(self.freed.notified());
            freed.as_mut().enable();
            let mut slots = self.slots.lock().await;
            // Past their life, or not taking new requests and unused: gone.
            let (gone, kept): (Vec<Slot>, Vec<Slot>) = slots.drain(..).partition(|slot| {
                slot.made.elapsed() >= self.max_life
                    || (!slot.takes_new_requests(self.reuse_for) && !slot.in_use())
            });
            *slots = kept;
            close(gone);
            if let Some(slot) = slots
                .iter()
                .find(|s| s.author == author && s.takes_new_requests(self.reuse_for))
            {
                return Ok(self.hold(slot));
            }
            if slots.len() >= MAX_VIEWING
                && let Some(idle) = slots.iter().position(|slot| !slot.in_use())
            {
                // Unused, but still taking requests for its author: it gives
                // way to someone waiting.
                close(vec![slots.remove(idle)]);
            }
            if slots.len() < MAX_VIEWING {
                let endpoint = make().await?;
                self.made.fetch_add(1, Ordering::Relaxed);
                let inner = Arc::new(ViewingEndpoint {
                    pool: store::new_pool(&endpoint),
                    endpoint,
                });
                self.end_at_life(&inner);
                slots.push(Slot {
                    author,
                    made: Instant::now(),
                    users: Arc::default(),
                    inner,
                });
                let slot = slots.last().expect("just pushed");
                return Ok(self.hold(slot));
            }
            drop(slots);
            debug!("all viewing endpoints are in use; waiting for one");
            freed.await;
        }
    }

    fn hold(&self, slot: &Slot) -> Lease<'_> {
        slot.users.fetch_add(1, Ordering::AcqRel);
        Lease {
            viewing: self,
            users: slot.users.clone(),
            inner: slot.inner.clone(),
        }
    }

    /// Closes the endpoint once it has lived `max_life`, transfers or not.
    fn end_at_life(&self, inner: &Arc<ViewingEndpoint>) {
        let weak: Weak<ViewingEndpoint> = Arc::downgrade(inner);
        let life = self.max_life;
        tokio::spawn(async move {
            tokio::time::sleep(life).await;
            if let Some(inner) = weak.upgrade() {
                debug!("a viewing endpoint reached its longest life");
                inner.endpoint.close().await;
            }
        });
    }

    /// Closes them all, when the node stops.
    pub async fn close_all(&self) {
        let gone: Vec<Slot> = self.slots.lock().await.drain(..).collect();
        for slot in gone {
            slot.inner.endpoint.close().await;
        }
    }
}

fn close(slots: Vec<Slot>) {
    for slot in slots {
        tokio::spawn(async move { slot.inner.endpoint.close().await });
    }
}

impl Core {
    /// Fetches media of `note`, a found post that its owner hasn't followed or
    /// shared, through a viewing endpoint, from public sources only: the
    /// media endpoints its author's profile gives. Other hosts come with
    /// ADR 0025. Never through this node's own endpoints.
    pub(crate) async fn watch_found(
        &self,
        blob: &BlobHash,
        note: &Note,
        class: BlobClass,
        declared: u64,
    ) -> Result<u64> {
        let sources: Vec<Source> = self
            .db
            .media_endpoints_of(&note.actor)?
            .into_iter()
            .map(|bound| Source {
                node: bound.public,
                endpoint: bound.endpoint,
                from_direct: false,
            })
            .collect();
        if sources.is_empty() {
            bail!("{NO_PRIVATE_SOURCE} (its author's profile gives no media endpoint)");
        }
        let lease = self
            .viewing
            .lease(note.actor, || self.viewing_endpoint())
            .await
            .context(NO_PRIVATE_SOURCE)?;
        let fetched = self
            .store
            .download_through(
                blob,
                Sources::one_at_a_time(sources),
                class,
                declared,
                &lease.inner.pool,
            )
            .await;
        drop(lease);
        match fetched {
            Ok(size) => {
                info!(blob = %blob.fmt_short(), "fetched a found post's media through a viewing endpoint");
                Ok(size)
            }
            Err(e) => Err(e.context(NO_PRIVATE_SOURCE)),
        }
    }

    /// A new viewing endpoint: a key made for it, relay-only, published
    /// nowhere. It finds sources by their IDs but doesn't publish its own.
    async fn viewing_endpoint(&self) -> Result<Endpoint> {
        let net = &self.config.network;
        if net.relay == crate::config::RelayChoice::Disabled && !on_simulated_network(&self.sim) {
            bail!("viewing endpoints need a relay, and relays are off");
        }
        let key = SecretKey::generate();
        let relays = RelayPolicy::new(relays::own_relays(&net.relay)?, true);
        let builder = endpoint_builder_unpublished(net, &self.lookup, &relays)?
            .hooks(self.tracker.with_relays(relays))
            .clear_ip_transports();
        crate::endpoints::sim_network(builder, &self.sim, &key, true)
            .secret_key(key)
            .bind()
            .await
            .context("starting a viewing endpoint")
    }
}

#[cfg(test)]
mod tests {
    use iroh::{RelayMode, endpoint::presets};

    use super::*;

    async fn endpoint() -> Result<Endpoint> {
        Ok(Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>()?)?
            .bind()
            .await?)
    }

    fn author(byte: u8) -> PublicKey {
        SecretKey::from_bytes(&[byte; 32]).public()
    }

    fn id(lease: &Lease<'_>) -> iroh::EndpointId {
        lease.inner.endpoint.id()
    }

    #[tokio::test]
    async fn one_author_shares_an_endpoint_and_another_gets_their_own() {
        let viewing = Viewing::new(REUSE_FOR, MAX_LIFE);
        let thumbnail = viewing.lease(author(1), endpoint).await.unwrap();
        let video = viewing.lease(author(1), endpoint).await.unwrap();
        assert_eq!(id(&thumbnail), id(&video));
        // And after both are done, for that author's other posts.
        let first = id(&thumbnail);
        drop((thumbnail, video));
        assert_eq!(
            id(&viewing.lease(author(1), endpoint).await.unwrap()),
            first
        );
        let other = viewing.lease(author(2), endpoint).await.unwrap();
        assert_ne!(id(&other), first);
        assert_eq!(viewing.made(), 2);
        viewing.close_all().await;
    }

    #[tokio::test]
    async fn a_new_request_after_the_reuse_time_gets_a_new_key_and_a_transfer_goes_on() {
        let viewing = Viewing::new(Duration::from_millis(200), MAX_LIFE);
        let under_way = viewing.lease(author(1), endpoint).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let later = viewing.lease(author(1), endpoint).await.unwrap();
        assert_ne!(id(&later), id(&under_way));
        // The transfer under way keeps its endpoint.
        assert!(!under_way.inner.endpoint.is_closed());
        // Once it's done, its endpoint goes, at the next look.
        let old = under_way.inner.clone();
        drop(under_way);
        drop(viewing.lease(author(2), endpoint).await.unwrap());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(old.endpoint.is_closed());
        viewing.close_all().await;
    }

    #[tokio::test]
    async fn a_fifth_author_waits_for_a_free_endpoint() {
        let viewing = Arc::new(Viewing::new(REUSE_FOR, MAX_LIFE));
        let mut held = Vec::new();
        for byte in 1..=MAX_VIEWING as u8 {
            held.push(viewing.lease(author(byte), endpoint).await.unwrap());
        }
        let fifth = viewing.lease(author(9), endpoint);
        let waited = tokio::time::timeout(Duration::from_millis(300), fifth).await;
        assert!(
            waited.is_err(),
            "a fifth author shared or went over the limit"
        );
        assert_eq!(viewing.made(), MAX_VIEWING);

        // One finishing frees a place, and the fifth gets a key of its own.
        let ids: Vec<_> = held.iter().map(id).collect();
        let waiting = {
            let viewing = viewing.clone();
            tokio::spawn(async move {
                let lease = viewing.lease(author(9), endpoint).await.unwrap();
                id(&lease)
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiting.is_finished());
        drop(held.remove(0));
        let fifth = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the fifth went on waiting")
            .unwrap();
        assert!(!ids.contains(&fifth));
        drop(held);
        viewing.close_all().await;
    }

    #[tokio::test]
    async fn an_endpoint_ends_at_its_longest_life_even_in_use() {
        let viewing = Viewing::new(REUSE_FOR, Duration::from_millis(200));
        let lease = viewing.lease(author(1), endpoint).await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(lease.inner.endpoint.is_closed());
        let next = viewing.lease(author(1), endpoint).await.unwrap();
        assert_ne!(id(&next), id(&lease));
        viewing.close_all().await;
    }
}
