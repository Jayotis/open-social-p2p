//! The viewer's side of a circuit (ADR 0043 and its design note, steps 2 to
//! 4): picking its own hop, and fetching through it and the host's.
//!
//! Say Jimmy opens Alice's photo. Alice's node named its hop Q, a slot and
//! its circuit endpoint (`CircuitOffer`). Jimmy's node picks a hop of its own,
//! P: a machine with standing with it, or a seed carrier (`picks`), and
//! sometimes a machine in the middle, M. It dials P from an endpoint made for
//! this circuit, builds V → P → (M →) Q, hands Q the slot, and fetches over
//! the ends' own QUIC on an endpoint whose only path is the circuit. If both
//! ends picked the same machine, it's one hop (ADR 0043 §4).
//!
//! A circuit that can't be built, or a fetch that fails over it, leaves the
//! fetch to the way it went before: through the relays, the project's first
//! (ADR 0044). Never directly to Alice's node, whose address Jimmy's never
//! learns.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use iroh::{Endpoint, EndpointId, SecretKey, TransportAddr};
use iroh_blobs::util::connection_pool::ConnectionPool;
use osp_carrier::{Descriptor, SETUP_TIMEOUT, ViewerCircuit};
use osp_proto::{BlobHash, social::CircuitOffer};
use tracing::{debug, info};

use crate::{
    carrier::{relay_mode, relayed_endpoint},
    db::BlobClass,
    machine::carrier_of,
    node::Core,
    picks::{Known, Middle},
    store::{self, Source, Sources},
    viewing::in_own_order,
};

/// Nodes that offered a way to their own media through their hops, each
/// with its offer.
pub(crate) type Offers = Vec<(EndpointId, CircuitOffer)>;

/// A cached descriptor is asked for again this long before its circuit key
/// runs out, so a circuit isn't built on a key about to go.
const FRESH_FOR: u64 = 5 * 60;

/// With a machine in the middle, its circuit is built alongside the one
/// without it, and used if it's ready within this: stage D's mark for a
/// circuit to be usable (`docs/testing/carrier-lab.md`). After it, whichever
/// is ready first carries, so a middle that's gone or slow costs no more
/// than the circuit without it. Each still has the carriers' whole
/// `SETUP_TIMEOUT` to answer.
const PREFER_MIDDLE_FOR: Duration = Duration::from_secs(5);

/// A circuit ready to fetch over: the endpoint its first hop was dialled
/// from, and the one whose only path is the circuit.
struct Circuit {
    link: Endpoint,
    ends: Endpoint,
}

impl Circuit {
    async fn close(self) {
        self.ends.close().await;
        self.link.close().await;
    }
}

/// A circuit built to an offer, with what fetching over it needs: the hops
/// without the middle, for a second try and for credit; the middle, if one
/// was picked, and whether the circuit goes through it; and this node's own
/// hop, whose descriptor is forgotten if the fetch fails.
struct Built {
    circuit: Circuit,
    hops: Vec<Descriptor>,
    middle: Option<Middle>,
    through_middle: bool,
    known: Known,
    my_hop: EndpointId,
}

/// How the two builds `race` waited on came out.
struct Raced<T> {
    /// The circuit to fetch over, and whether it's the one with the middle.
    chosen: Result<(T, bool)>,
    /// The other, if it was built too: to close.
    unused: Option<T>,
    /// Why the circuit through the middle failed, if it did. Not set when
    /// it was only slower.
    middle_failed: Option<anyhow::Error>,
}

/// Waits on `with` the middle and `without` it, both started: `with` if it's
/// ready within `prefer_for`, else the first ready after that. One that
/// fails leaves the other to wait for. The one not chosen is dropped if
/// it's still building.
async fn race<T>(
    with: impl Future<Output = Result<T>>,
    without: impl Future<Output = Result<T>>,
    prefer_for: Duration,
) -> Raced<T> {
    tokio::pin!(with, without);
    let preferred = tokio::time::sleep(prefer_for);
    tokio::pin!(preferred);
    let (mut w, mut wo): (Option<Result<T>>, Option<Result<T>>) = (None, None);
    let mut waited = false;
    loop {
        match (&w, &wo) {
            (Some(Ok(_)), _) | (Some(Err(_)), Some(_)) => break,
            (None, Some(Ok(_))) if waited => break,
            _ => {}
        }
        tokio::select! {
            r = &mut with, if w.is_none() => w = Some(r),
            r = &mut without, if wo.is_none() => wo = Some(r),
            _ = &mut preferred, if !waited => waited = true,
        }
    }
    match (w, wo) {
        (Some(Ok(circuit)), other) => Raced {
            chosen: Ok((circuit, true)),
            unused: other.and_then(Result::ok),
            middle_failed: None,
        },
        (w, Some(Ok(circuit))) => Raced {
            chosen: Ok((circuit, false)),
            unused: None,
            middle_failed: w.and_then(Result::err),
        },
        (w, wo) => Raced {
            chosen: Err(wo
                .and_then(Result::err)
                .unwrap_or_else(|| anyhow!("no circuit was built"))),
            unused: None,
            middle_failed: w.and_then(Result::err),
        },
    }
}

/// Waits on `tried` with `building` started beside it. If `tried` succeeds,
/// what was built by then, to close, with a build still going dropped; if it
/// fails, the build, waited for if it isn't done. So a try that fails costs
/// no more than the build alone.
async fn try_while_building<T, B>(
    tried: impl Future<Output = Result<T>>,
    building: impl Future<Output = B>,
) -> (Result<T>, Option<B>) {
    tokio::pin!(tried, building);
    let mut built = None;
    let tried = loop {
        tokio::select! {
            r = &mut tried => break r,
            b = &mut building, if built.is_none() => built = Some(b),
        }
    };
    if tried.is_err() && built.is_none() {
        built = Some(building.await);
    }
    (tried, built)
}

/// The seed carriers' descriptors, as their machine records last gave them.
#[derive(Default)]
pub(crate) struct SeedDescriptors(Mutex<HashMap<EndpointId, (Descriptor, u64)>>);

/// Files whose hosts are being asked for circuit offers now, so a second
/// fetch of one, such as an open while it's fetched ahead of time, doesn't
/// ask again (`OffersAsked::start`).
#[derive(Default)]
pub(crate) struct OffersAsked(Mutex<HashSet<BlobHash>>);

impl OffersAsked {
    /// Notes that `blob`'s hosts are asked, until the guard is dropped.
    /// None if they're being asked already.
    pub(crate) fn start(&self, blob: BlobHash) -> Option<OffersAsking<'_>> {
        let mut asked = self.0.lock().unwrap_or_else(|e| e.into_inner());
        asked
            .insert(blob)
            .then(|| OffersAsking { asked: self, blob })
    }
}

pub(crate) struct OffersAsking<'a> {
    asked: &'a OffersAsked,
    blob: BlobHash,
}

impl Drop for OffersAsking<'_> {
    fn drop(&mut self) {
        let mut asked = self.asked.0.lock().unwrap_or_else(|e| e.into_inner());
        asked.remove(&self.blob);
    }
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Q's descriptor in an offer, checked.
fn offered_hop(offer: &CircuitOffer, now: u64) -> Result<Descriptor> {
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(offer.carrier.as_bytes())
        .map_err(|_| anyhow!("the offer's descriptor isn't base64url"))?;
    let descriptor = Descriptor::from_bytes(&bytes)?;
    descriptor.verify(now)?;
    Ok(descriptor)
}

impl Core {
    /// Whether this node fetches through carriers: its owner said the hops
    /// it picks may see its address, it knows seed carriers, which its own
    /// hop is picked from, and it has a machine name to read their records
    /// with.
    pub(crate) fn fetches_through_carriers(&self) -> bool {
        self.uses_hops()
            && !self.config.carrier.known.is_empty()
            && self.machine.is_some()
            && self.config.network.relay != crate::config::RelayChoice::Disabled
    }

    /// Whether the owner said yes to the hops it picks seeing its address.
    pub(crate) fn uses_hops(&self) -> bool {
        *self.hops.borrow() == Some(true)
    }

    /// The descriptor the seed carrier `machine`'s record names: with
    /// `cached`, the one held while its circuit key has a while to go; else,
    /// or without one, asked for over Tor. A carrier that restarts has a new
    /// circuit key, so a circuit refused through it forgets the one held.
    pub(crate) async fn seed_descriptor(
        &self,
        machine: EndpointId,
        cached: bool,
    ) -> Result<Descriptor> {
        let now = unix_now();
        if cached
            && let Some((descriptor, until)) = self.seed_descriptors.0.lock().unwrap().get(&machine)
            && *until > now + FRESH_FOR
        {
            return Ok(descriptor.clone());
        }
        let greeting = self.machine()?.hello(machine).await?;
        let descriptor = carrier_of(&greeting.info, unix_now())
            .context("its machine record names no carrier that checks out")?;
        let until = descriptor.verify(unix_now())?.valid_until;
        self.seed_descriptors
            .0
            .lock()
            .unwrap()
            .insert(machine, (descriptor.clone(), until));
        Ok(descriptor)
    }

    /// The seed carriers known, in an order of this node's own, so ends
    /// spread over them.
    pub(crate) fn seeds_in_own_order(&self) -> Vec<EndpointId> {
        let mut seeds: Vec<EndpointId> = self
            .config
            .carrier
            .known
            .iter()
            .map(|s| s.machine)
            .collect();
        in_own_order(&mut seeds);
        seeds
    }

    /// Fetches `blob` from `host`'s node through a circuit to what `offer`
    /// names: this node's own hop, perhaps a machine in the middle (the
    /// `picks` module), then the host's hop, then the host's circuit
    /// endpoint. Nothing goes any other way. With a machine in the middle,
    /// the circuit without it is built at the same time, and carries the
    /// fetch if the middle's is late or fails (`open_preferring_middle`); a
    /// fetch over the middle's that fails is tried once more without it. A
    /// fetch that passes the hash check credits the hops that carried it.
    pub(crate) async fn fetch_over_circuit(
        &self,
        blob: &BlobHash,
        host: EndpointId,
        offer: &CircuitOffer,
        class: BlobClass,
        declared: u64,
    ) -> Result<u64> {
        let built = self.circuit_to(blob, offer).await?;
        self.fetch_over_built(built, blob, host, offer, (class, declared))
            .await
    }

    /// The circuit `fetch_over_circuit` fetches over, built and ready.
    async fn circuit_to(&self, blob: &BlobHash, offer: &CircuitOffer) -> Result<Built> {
        let theirs = offered_hop(offer, unix_now())?;
        let their_hop = theirs.verify(unix_now())?.machine;
        let known = self.known_carriers()?;
        let mine = self.own_hop(&known, their_hop).await?;
        let my_hop = mine.verify(unix_now())?.machine;
        let hops = match my_hop == their_hop {
            // Both ends picked it: one hop, and nothing in the middle, which
            // would sit next to an end.
            true => vec![theirs],
            false => vec![mine, theirs],
        };
        let middle = match hops.len() {
            2 => self.middle_hop(&known, &[my_hop, their_hop]),
            _ => None,
        };
        let opened = match &middle {
            None => self.open(blob, offer, &hops).await.map(|c| (c, false)),
            Some(m) => {
                let mut through = hops.clone();
                through.insert(1, m.descriptor.clone());
                // Boxed: two circuits' builds at once are a large future.
                Box::pin(self.open_preferring_middle(blob, offer, &through, &hops, m)).await
            }
        };
        let (circuit, through_middle) = opened.inspect_err(|_| self.forget_descriptor(&my_hop))?;
        Ok(Built {
            circuit,
            hops,
            middle,
            through_middle,
            known,
            my_hop,
        })
    }

    /// A seed carrier's descriptor is asked for again next time, in case the
    /// hop restarted.
    fn forget_descriptor(&self, machine: &EndpointId) {
        self.seed_descriptors.0.lock().unwrap().remove(machine);
    }

    /// Fetches `blob` from `host` over `built`, then credits the hops that
    /// carried it, as `fetch_over_circuit` says.
    async fn fetch_over_built(
        &self,
        built: Built,
        blob: &BlobHash,
        host: EndpointId,
        offer: &CircuitOffer,
        job: (BlobClass, u64),
    ) -> Result<u64> {
        let Built {
            circuit,
            mut hops,
            middle,
            through_middle,
            known,
            my_hop,
        } = built;
        let (_, declared) = job;
        let fetched = match (&middle, through_middle) {
            (Some(m), true) => match self.download_over(circuit, blob, host, offer, job).await {
                Ok(size) => Ok((size, true)),
                Err(e) => {
                    debug!(machine = %m.machine.fmt_short(), "the fetch through a machine in the middle failed; without it: {e:#}");
                    self.middle_failed(m);
                    self.fetch_through_hops(blob, host, offer, &hops, job)
                        .await
                        .map(|size| (size, false))
                }
            },
            _ => self
                .download_over(circuit, blob, host, offer, job)
                .await
                .map(|size| (size, false)),
        };
        let (size, through_middle) = fetched.inspect_err(|_| self.forget_descriptor(&my_hop))?;
        // Only the hops that carried it are credited.
        let middle = middle.filter(|_| through_middle);
        if let Some(m) = &middle {
            hops.insert(1, m.descriptor.clone());
        }
        let machines: Vec<EndpointId> = hops
            .iter()
            .filter_map(|d| d.verify(unix_now()).ok().map(|i| i.machine))
            .collect();
        if declared > store::SMALL_BLOB
            && let Err(e) = self.credit_hops(&known, &machines, middle.as_ref())
        {
            debug!("couldn't note the hops' job: {e:#}");
        }
        info!(
            blob = %blob.fmt_short(),
            hops = hops.len(),
            "fetched through carriers"
        );
        self.may_ask_for_work();
        Ok(size)
    }

    /// One try at fetching `blob` through a circuit through `hops`.
    async fn fetch_through_hops(
        &self,
        blob: &BlobHash,
        host: EndpointId,
        offer: &CircuitOffer,
        hops: &[Descriptor],
        job: (BlobClass, u64),
    ) -> Result<u64> {
        let circuit = self.open(blob, offer, hops).await?;
        self.download_over(circuit, blob, host, offer, job).await
    }

    /// A circuit through `hops` to `offer`'s slot, from an endpoint of its own.
    async fn open(
        &self,
        blob: &BlobHash,
        offer: &CircuitOffer,
        hops: &[Descriptor],
    ) -> Result<Circuit> {
        let link = self.link_endpoint().await?;
        match self.build_circuit(&link, blob, offer, hops).await {
            Ok(ends) => Ok(Circuit { link, ends }),
            Err(e) => {
                link.close().await;
                Err(e)
            }
        }
    }

    /// Builds the circuit through `through`, which has a machine in the
    /// middle, and the one through `without`, at once. The middle's is used
    /// if it's ready within `PREFER_MIDDLE_FOR`, else whichever is ready
    /// first after that. A middle whose circuit fails is noted
    /// (`middle_failed`); one that is only slower than the other isn't. The
    /// circuit not used is closed. Whether the middle's is the one returned.
    async fn open_preferring_middle(
        &self,
        blob: &BlobHash,
        offer: &CircuitOffer,
        through: &[Descriptor],
        without: &[Descriptor],
        middle: &Middle,
    ) -> Result<(Circuit, bool)> {
        let (with_link, without_link) = tokio::join!(self.link_endpoint(), self.link_endpoint());
        let (with_link, without_link) = match (with_link, without_link) {
            (Ok(a), Ok(b)) => (a, b),
            (Ok(link), Err(e)) | (Err(e), Ok(link)) => {
                link.close().await;
                return Err(e);
            }
            (Err(e), Err(_)) => return Err(e),
        };
        let raced = race(
            Box::pin(self.build_circuit(&with_link, blob, offer, through)),
            Box::pin(self.build_circuit(&without_link, blob, offer, without)),
            PREFER_MIDDLE_FOR,
        )
        .await;
        let machine = middle.machine.fmt_short();
        if let Some(e) = &raced.middle_failed {
            debug!(%machine, "the circuit through a machine in the middle failed; without it: {e:#}");
            self.middle_failed(middle);
        }
        if let Some(ends) = raced.unused {
            ends.close().await;
        }
        match raced.chosen {
            Ok((ends, true)) => {
                without_link.close().await;
                Ok((
                    Circuit {
                        link: with_link,
                        ends,
                    },
                    true,
                ))
            }
            Ok((ends, false)) => {
                if raced.middle_failed.is_none() {
                    debug!(%machine, "the circuit without the machine in the middle was ready first; it carries this one");
                }
                with_link.close().await;
                Ok((
                    Circuit {
                        link: without_link,
                        ends,
                    },
                    false,
                ))
            }
            Err(e) => {
                with_link.close().await;
                without_link.close().await;
                Err(e)
            }
        }
    }

    /// An endpoint for a circuit's first link: with a key made for this
    /// circuit, published nowhere, so the first hop learns an address
    /// that's fetching, not an account.
    async fn link_endpoint(&self) -> Result<Endpoint> {
        let relays = self.tracker.relays().clone();
        let relay = relay_mode(&self.config)?.context("relays are off")?;
        relayed_endpoint(&self.config, relay, &relays, SecretKey::generate(), vec![])
            .await
            .context("starting the endpoint for the circuit")
    }

    /// Dials `hops`' first from `link` and builds the circuit to `offer`'s
    /// slot: the endpoint whose only path is the circuit.
    async fn build_circuit(
        &self,
        link: &Endpoint,
        blob: &BlobHash,
        offer: &CircuitOffer,
        hops: &[Descriptor],
    ) -> Result<Endpoint> {
        let first = hops[0].verify(unix_now())?;
        let relays = self.tracker.relays().clone();
        let started = tokio::time::Instant::now();
        let dial = osp_carrier::relay_dial(link.clone(), move |url| {
            relays.permits(&TransportAddr::Relay(url.clone()))
        });
        let conn = tokio::time::timeout(SETUP_TIMEOUT, dial(first))
            .await
            .map_err(|_| anyhow!("the first hop didn't answer in time"))??;
        let circuit = ViewerCircuit::build(conn, hops, offer.slot).await?;
        let ends = osp_carrier::viewer_endpoint(circuit, offer.endpoint).await?;
        debug!(
            blob = %blob.fmt_short(),
            hops = hops.len(),
            took_ms = started.elapsed().as_millis() as u64,
            "the circuit is ready"
        );
        Ok(ends)
    }

    /// Fetches `blob` from `host` over `circuit`, then closes it.
    async fn download_over(
        &self,
        circuit: Circuit,
        blob: &BlobHash,
        host: EndpointId,
        offer: &CircuitOffer,
        (class, declared): (BlobClass, u64),
    ) -> Result<u64> {
        let pool = store::new_pool(&circuit.ends);
        let source = Source {
            node: host,
            endpoint: offer.endpoint,
            from_direct: false,
        };
        let size = self
            .store
            .download_through(
                blob,
                Sources::one_at_a_time(vec![source]),
                class,
                declared,
                &pool,
            )
            .await;
        drop(pool);
        circuit.close().await;
        size
    }

    /// As `fetch_over_circuit`, when this node fetches through carriers and
    /// the host offers a way, logging why not otherwise. Whether it fetched.
    pub(crate) async fn try_over_circuit(
        &self,
        blob: &BlobHash,
        host: EndpointId,
        offer: Option<&CircuitOffer>,
        class: BlobClass,
        declared: u64,
    ) -> Option<u64> {
        let offer = offer?;
        if !self.fetches_through_carriers() {
            return None;
        }
        match self
            .fetch_over_circuit(blob, host, offer, class, declared)
            .await
        {
            Ok(size) => Some(size),
            Err(e) => {
                debug!(blob = %blob.fmt_short(), "couldn't fetch through carriers; the relays instead: {e:#}");
                None
            }
        }
    }

    /// As `try_over_circuit`'s first step: the circuit to `offer`, built,
    /// or why not logged.
    async fn circuit_offered(&self, blob: &BlobHash, offer: &CircuitOffer) -> Option<Built> {
        self.circuit_to(blob, offer)
            .await
            .inspect_err(|e| debug!(blob = %blob.fmt_short(), "couldn't fetch through carriers; the relays instead: {e:#}"))
            .ok()
    }

    /// As `try_over_circuit`'s second step: the fetch over `built`.
    async fn fetch_offered(
        &self,
        built: Built,
        blob: &BlobHash,
        host: EndpointId,
        offer: &CircuitOffer,
        job: (BlobClass, u64),
    ) -> Option<u64> {
        self.fetch_over_built(built, blob, host, offer, job)
            .await
            .inspect_err(|e| debug!(blob = %blob.fmt_short(), "couldn't fetch through carriers; the relays instead: {e:#}"))
            .ok()
    }

    /// Downloads `blob` from `sources`, as the store does, but reaches the
    /// nodes in `offers` through carriers rather than their relayed media
    /// endpoints when this node fetches through carriers (ADR 0043): the
    /// other sources asked at once first, and any reached directly, then
    /// each offer's circuit, then all of `sources` as before, the relays
    /// included. A node's direct endpoint is never held back. With `via`, a viewing endpoint's pool, the
    /// sources are reached only through it (ADR 0024).
    ///
    /// The first offer's circuit is built while the other sources are
    /// tried, so a try that fails doesn't then wait for it: stage D's first
    /// byte (`docs/testing/carrier-lab.md`). Only the circuit is built, and
    /// nothing is downloaded over it unless the try fails. If it succeeds,
    /// the circuit is closed, or its build dropped.
    pub(crate) async fn download_offered(
        &self,
        blob: &BlobHash,
        sources: Sources,
        offers: Offers,
        (class, declared): (BlobClass, u64),
        via: Option<&ConnectionPool>,
    ) -> Result<u64> {
        if offers.is_empty() || !self.fetches_through_carriers() {
            return self
                .store
                .download_with(blob, sources, class, declared, via)
                .await;
        }
        let job = (class, declared);
        let offered = |s: &Source| !s.from_direct && offers.iter().any(|(node, _)| *node == s.node);
        // Those asked one at a time are the ones to spare, such as the
        // author's: they wait for the circuits unless reached directly.
        let mut others = sources.clone();
        others.at_once.retain(|s| !offered(s));
        others.in_turn.retain(|s| s.from_direct);
        let mut first_built = None;
        if !others.is_empty() {
            let (_, first) = &offers[0];
            let (tried, built) = try_while_building(
                self.store.download_with(blob, others, class, declared, via),
                // Boxed: a circuit's build, with a middle's beside it, is large.
                Box::pin(self.circuit_offered(blob, first)),
            )
            .await;
            match tried {
                Ok(size) => {
                    // Not needed: closed if ready, else its build dropped.
                    if let Some(Some(b)) = built {
                        b.circuit.close().await;
                    }
                    return Ok(size);
                }
                Err(e) => {
                    debug!(blob = %blob.fmt_short(), "the other sources failed; through carriers next: {e:#}");
                    first_built = built;
                }
            }
        }
        for (i, (node, offer)) in offers.iter().enumerate() {
            // Built while the other sources were tried: if that took long,
            // it may have gone stale, so a fetch over it that fails is tried
            // once more over a new one. One whose build failed isn't.
            if let Some(built) = first_built.take().filter(|_| i == 0) {
                let Some(built) = built else { continue };
                if let Some(size) = self.fetch_offered(built, blob, *node, offer, job).await {
                    return Ok(size);
                }
            }
            if let Some(built) = self.circuit_offered(blob, offer).await
                && let Some(size) = self.fetch_offered(built, blob, *node, offer, job).await
            {
                return Ok(size);
            }
        }
        self.store
            .download_with(blob, sources, class, declared, via)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build that takes `secs` and comes out as `ok`, named `name`.
    async fn build(name: &'static str, secs: f64, ok: bool) -> Result<&'static str> {
        tokio::time::sleep(Duration::from_secs_f64(secs)).await;
        match ok {
            true => Ok(name),
            false => Err(anyhow!("{name} failed")),
        }
    }

    async fn raced(with: (f64, bool), without: (f64, bool)) -> (Raced<&'static str>, f64) {
        let started = tokio::time::Instant::now();
        let r = race(
            build("middle", with.0, with.1),
            build("two", without.0, without.1),
            PREFER_MIDDLE_FOR,
        )
        .await;
        // To a tenth of a second: timers are rounded to milliseconds.
        let took = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        (r, took)
    }

    #[tokio::test(start_paused = true)]
    async fn a_middle_ready_in_time_is_used_even_after_the_other() {
        let (r, took) = raced((4.5, true), (1.0, true)).await;
        assert_eq!(r.chosen.unwrap(), ("middle", true));
        assert_eq!(r.unused, Some("two"), "built, so it's closed");
        assert!(r.middle_failed.is_none());
        assert_eq!(took, 4.5);
    }

    #[tokio::test(start_paused = true)]
    async fn a_middle_ready_first_doesnt_wait_for_the_other() {
        let (r, took) = raced((2.0, true), (4.0, true)).await;
        assert_eq!(r.chosen.unwrap(), ("middle", true));
        assert_eq!(r.unused, None, "still building, so dropped");
        assert_eq!(took, 2.0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_middle_loses_to_the_other_without_failing() {
        // Stage D: a middle that never answered held the fetch 22.8 s.
        let (r, took) = raced((20.0, false), (4.2, true)).await;
        assert_eq!(r.chosen.unwrap(), ("two", false));
        assert!(r.middle_failed.is_none(), "only slower: not failed");
        assert_eq!(took, 5.0, "the middle's 5 s, then the one ready");
        let (r, took) = raced((30.0, true), (7.0, true)).await;
        assert_eq!(r.chosen.unwrap(), ("two", false));
        assert_eq!(took, 7.0, "after the 5 s, the first ready");
    }

    #[tokio::test(start_paused = true)]
    async fn a_middle_that_fails_is_noted_and_the_other_carries() {
        // Stage D: a middle answered `Gone` after 5.4 s.
        let (r, took) = raced((1.0, false), (4.2, true)).await;
        assert_eq!(r.chosen.unwrap(), ("two", false));
        assert!(r.middle_failed.is_some());
        assert_eq!(took, 4.2, "no wait for the 5 s once the middle failed");
    }

    #[tokio::test(start_paused = true)]
    async fn the_middle_carries_when_the_other_fails_even_late() {
        let (r, took) = raced((9.0, true), (1.0, false)).await;
        assert_eq!(r.chosen.unwrap(), ("middle", true));
        assert_eq!(took, 9.0);
        let (r, _) = raced((2.0, false), (3.0, false)).await;
        assert!(r.chosen.unwrap_err().to_string().contains("two failed"));
        assert!(r.middle_failed.is_some());
    }

    /// A try that takes `secs` and comes out as `ok`, with the circuit
    /// built in `build_secs` beside it: how they came out, and the time.
    async fn tried(secs: f64, ok: bool, build_secs: f64) -> (bool, Option<&'static str>, f64) {
        let started = tokio::time::Instant::now();
        let (r, built) = try_while_building(build("direct", secs, ok), async move {
            tokio::time::sleep(Duration::from_secs_f64(build_secs)).await;
            "circuit"
        })
        .await;
        let took = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        (r.is_ok(), built, took)
    }

    #[tokio::test(start_paused = true)]
    async fn a_direct_try_that_fails_leaves_the_circuit_built_beside_it() {
        // Stage D: the direct try failed after 1 s, then the circuit took
        // 4 s more. Built beside it, the 4 s are all it costs.
        assert_eq!(tried(1.0, false, 4.0).await, (false, Some("circuit"), 4.0));
        assert_eq!(tried(6.0, false, 4.0).await, (false, Some("circuit"), 6.0));
    }

    #[tokio::test(start_paused = true)]
    async fn a_direct_try_that_succeeds_doesnt_wait_for_the_circuit() {
        assert_eq!(tried(1.0, true, 4.0).await, (true, None, 1.0), "dropped");
        assert_eq!(
            tried(30.0, true, 4.0).await,
            (true, Some("circuit"), 30.0),
            "built, so handed back to be closed"
        );
    }

    #[test]
    fn a_files_hosts_are_asked_for_offers_once_at_a_time() {
        let asked = OffersAsked::default();
        let blob = BlobHash::from_bytes([1; 32]);
        let first = asked.start(blob).expect("not asked yet");
        assert!(asked.start(blob).is_none(), "asked twice at once");
        assert!(asked.start(BlobHash::from_bytes([2; 32])).is_some());
        drop(first);
        assert!(asked.start(blob).is_some(), "not asked again once done");
    }

    #[test]
    fn an_offer_whose_descriptor_doesnt_check_out_is_refused() {
        let offer = CircuitOffer {
            carrier: "not-a-descriptor!".into(),
            slot: [0; 16],
            endpoint: SecretKey::generate().public(),
        };
        assert!(offered_hop(&offer, unix_now()).is_err());
        let offer = CircuitOffer {
            carrier: data_encoding::BASE64URL_NOPAD.encode(&[7; 40]),
            ..offer
        };
        assert!(offered_hop(&offer, unix_now()).is_err());
    }
}
