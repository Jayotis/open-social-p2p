//! The viewer's side of a circuit (ADR 0043 and its design note, steps 2 to
//! 4): picking its own hop, and fetching through it and the host's.
//!
//! Say Jimmy opens Alice's photo. Alice's node named its hop Q, a slot and
//! its circuit endpoint (`CircuitOffer`). Jimmy's node picks a hop of its own,
//! P, from the seed carriers it knows: until ADR 0017's standing is built
//! (part 4d), only those may sit next to an end. It dials P from an endpoint
//! made for this circuit, builds V → P → Q, hands Q the slot, and fetches
//! over the ends' own QUIC on an endpoint whose only path is the circuit. If
//! both ends picked the same machine, it's one hop (ADR 0043 §4).
//!
//! A circuit that can't be built, or a fetch that fails over it, leaves the
//! fetch to the way it went before: through the relays, the project's first
//! (ADR 0044). Never directly to Alice's node, whose address Jimmy's never
//! learns.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use iroh::{EndpointId, SecretKey, TransportAddr};
use iroh_blobs::util::connection_pool::ConnectionPool;
use osp_carrier::{Descriptor, SETUP_TIMEOUT, ViewerCircuit};
use osp_proto::{BlobHash, social::CircuitOffer};
use tracing::{debug, info};

use crate::{
    carrier::{relay_mode, relayed_endpoint},
    db::BlobClass,
    machine::carrier_of,
    node::Core,
    store::{self, Source, Sources},
    viewing::in_own_order,
};

/// Nodes that offered a way to their own media through their hops, each
/// with its offer.
pub(crate) type Offers = Vec<(EndpointId, CircuitOffer)>;

/// A cached descriptor is asked for again this long before its circuit key
/// runs out, so a circuit isn't built on a key about to go.
const FRESH_FOR: u64 = 5 * 60;

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
    /// Whether this node fetches through carriers: it knows seed carriers,
    /// which its own hop is picked from, and has a machine name to read
    /// their records with.
    pub(crate) fn fetches_through_carriers(&self) -> bool {
        !self.config.carrier.known.is_empty()
            && self.machine.is_some()
            && self.config.network.relay != crate::config::RelayChoice::Disabled
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

    /// This node's own hop for a circuit: the first seed carrier, in an
    /// order of its own, whose descriptor can be had.
    async fn own_hop(&self) -> Result<Descriptor> {
        let mut last = anyhow!("no seed carrier is known");
        for seed in self.seeds_in_own_order() {
            match self.seed_descriptor(seed, true).await {
                Ok(descriptor) => return Ok(descriptor),
                Err(e) => last = e,
            }
        }
        Err(last.context("no hop of this node's own could be had"))
    }

    /// Fetches `blob` from `host`'s node through a circuit to what `offer`
    /// names: this node's own hop, then the host's, then the host's circuit
    /// endpoint. Nothing goes any other way.
    pub(crate) async fn fetch_over_circuit(
        &self,
        blob: &BlobHash,
        host: EndpointId,
        offer: &CircuitOffer,
        class: BlobClass,
        declared: u64,
    ) -> Result<u64> {
        let theirs = offered_hop(offer, unix_now())?;
        let mine = self.own_hop().await?;
        let my_hop = mine.verify(unix_now())?.machine;
        let hops = match my_hop == theirs.verify(unix_now())?.machine {
            // Both ends picked it: one hop.
            true => vec![theirs],
            false => vec![mine, theirs],
        };
        let first = hops[0].verify(unix_now())?;
        let relays = self.tracker.relays().clone();
        let relay = relay_mode(&self.config)?.context("relays are off")?;
        // A key made for this circuit, published nowhere, so the first hop
        // learns an address that's fetching, not an account.
        let link_endpoint =
            relayed_endpoint(&self.config, relay, &relays, SecretKey::generate(), vec![])
                .await
                .context("starting the endpoint for the circuit")?;
        let fetched = async {
            let dial = osp_carrier::relay_dial(link_endpoint.clone(), move |url| {
                relays.permits(&TransportAddr::Relay(url.clone()))
            });
            let conn = tokio::time::timeout(SETUP_TIMEOUT, dial(first))
                .await
                .map_err(|_| anyhow!("the first hop didn't answer in time"))??;
            let circuit = ViewerCircuit::build(conn, &hops, offer.slot).await?;
            let ends = osp_carrier::viewer_endpoint(circuit, offer.endpoint).await?;
            let pool = store::new_pool(&ends);
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
            ends.close().await;
            size
        }
        .await;
        link_endpoint.close().await;
        let size = fetched.inspect_err(|_| {
            // Asked for again next time, in case the hop restarted.
            self.seed_descriptors.0.lock().unwrap().remove(&my_hop);
        })?;
        info!(
            blob = %blob.fmt_short(),
            hops = hops.len(),
            "fetched through carriers"
        );
        Ok(size)
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

    /// Downloads `blob` from `sources`, as the store does, but reaches the
    /// nodes in `offers` through carriers rather than their relayed media
    /// endpoints when this node fetches through carriers (ADR 0043): the
    /// other sources asked at once first, and any reached directly, then
    /// each offer's circuit, then all of `sources` as before, the relays
    /// included. A node's direct endpoint is never held back. With `via`, a viewing endpoint's pool, the
    /// sources are reached only through it (ADR 0024).
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
        let offered = |s: &Source| !s.from_direct && offers.iter().any(|(node, _)| *node == s.node);
        // Those asked one at a time are the ones to spare, such as the
        // author's: they wait for the circuits unless reached directly.
        let mut others = sources.clone();
        others.at_once.retain(|s| !offered(s));
        others.in_turn.retain(|s| s.from_direct);
        if !others.is_empty() {
            match self
                .store
                .download_with(blob, others, class, declared, via)
                .await
            {
                Ok(size) => return Ok(size),
                Err(e) => {
                    debug!(blob = %blob.fmt_short(), "the other sources failed; through carriers next: {e:#}")
                }
            }
        }
        for (node, offer) in &offers {
            if let Some(size) = self
                .try_over_circuit(blob, *node, Some(offer), class, declared)
                .await
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
