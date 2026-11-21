//! The host's side of a circuit (ADR 0043's design note, "Building a
//! circuit", step 1): a host's node keeps a link with its hop Q, and answers
//! viewers' circuits on an endpoint of its own.
//!
//! Say Jimmy watches Alice's video. Alice's node picks a seed carrier Q from
//! `[[carrier.known]]`, greets Q's machine over Tor, and reads Q's carrier
//! descriptor from its machine record. It dials Q's carrier endpoint from an
//! endpoint made for that link, so Q sees the link's address but no machine
//! name or public ID of Alice's, and Q gives the link a host slot. Alice's
//! node answers circuits on her circuit endpoint, which has no internet
//! address and no relay: only that link. When Jimmy's node asks how to reach
//! her media, over Tor or from a viewing endpoint, her node hands it Q's
//! descriptor, the slot and her circuit endpoint's ID (`CircuitOffer`), never
//! her address.
//!
//! The link is built again when it closes, and each day once Q's circuit key
//! is replaced, so the descriptor handed out is Q's current one. Until
//! ADR 0018, the hop is always a seed carrier.

use std::{
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use iroh::{Endpoint, EndpointId, SecretKey, TransportAddr, protocol::Router};
use iroh_blobs::BlobsProtocol;
use osp_carrier::{HostLink, HostSender, SETUP_TIMEOUT};
use osp_proto::social::{CircuitOffer, MEDIA_HOSTS_ALPN, REPLIES_ALPN};
use tokio::time::Instant;
use tracing::{debug, info};

use crate::{
    carrier::{kept_key, relay_mode, relayed_endpoint},
    home::Home,
    hosting::MediaHostsProtocol,
    machine::carrier_of,
    node::{Core, SharedBlobs},
    sync::{jitter, random_fraction},
    threads::RepliesProtocol,
};

/// How long after Q's circuit key runs out its new descriptor is asked for.
/// Q replaces the key then and renews its record within a minute.
const AFTER_KEY: Duration = Duration::from_secs(90);
/// The first wait after a failed try, doubled each time up to `LONGEST_WAIT`.
const FIRST_WAIT: Duration = Duration::from_secs(5);
const LONGEST_WAIT: Duration = Duration::from_secs(10 * 60);

/// The node's link to its hop, when it has one.
pub(crate) struct HostCircuit {
    /// The key its circuit endpoint answers under, kept across restarts.
    key: SecretKey,
    live: RwLock<Option<Live>>,
}

/// One link to a hop, and the circuit endpoint answering on it.
struct Live {
    /// Q's machine.
    hop: EndpointId,
    /// Q's descriptor, as a machine record carries it.
    carrier: String,
    sender: HostSender,
    /// The endpoint the link was dialled from.
    link_endpoint: Endpoint,
    router: Router,
}

impl Live {
    async fn end(self) {
        self.sender.end();
        let _ = self.router.shutdown().await;
        self.link_endpoint.close().await;
    }
}

impl HostCircuit {
    pub fn load(home: &Home) -> Result<Self> {
        Ok(Self {
            key: kept_key(&home.circuit_key())?,
            live: RwLock::new(None),
        })
    }

    /// The endpoint viewers' circuits reach.
    pub fn endpoint_id(&self) -> EndpointId {
        self.key.public()
    }

    /// What to hand a viewer now, while the link to the hop is up.
    pub fn offer(&self) -> Option<CircuitOffer> {
        let live = self.live.read().unwrap_or_else(|e| e.into_inner());
        let live = live.as_ref().filter(|l| !l.sender.is_closed())?;
        Some(CircuitOffer {
            carrier: live.carrier.clone(),
            slot: live.sender.slot(),
            endpoint: self.endpoint_id(),
        })
    }

    /// The hop's machine, while the link is up.
    pub fn hop(&self) -> Option<EndpointId> {
        let live = self.live.read().unwrap_or_else(|e| e.into_inner());
        live.as_ref()
            .filter(|l| !l.sender.is_closed())
            .map(|l| l.hop)
    }

    fn swap(&self, new: Option<Live>) -> Option<Live> {
        std::mem::replace(
            &mut *self.live.write().unwrap_or_else(|e| e.into_inner()),
            new,
        )
    }

    /// Ends the link, when the node stops.
    pub async fn close(&self) {
        if let Some(old) = self.swap(None) {
            old.end().await;
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Core {
    /// How a viewer reaches this node's media through its hop, while it has
    /// a link to one.
    pub(crate) fn circuit_offer(&self) -> Option<Box<CircuitOffer>> {
        self.host_circuit.as_ref()?.offer().map(Box::new)
    }

    /// Builds a link to one of the seed carriers, `prefer` first if it's
    /// one, the rest in an order of this node's own.
    async fn link_to_a_hop(
        self: &Arc<Self>,
        circuit: &HostCircuit,
        blobs: &BlobsProtocol,
        prefer: Option<EndpointId>,
    ) -> Result<(Live, u64)> {
        let mut seeds: Vec<EndpointId> = self
            .config
            .carrier
            .known
            .iter()
            .map(|s| s.machine)
            .collect();
        for i in (1..seeds.len()).rev() {
            let j = ((random_fraction() * (i + 1) as f64) as usize).min(i);
            seeds.swap(i, j);
        }
        if let Some(at) = prefer.and_then(|p| seeds.iter().position(|s| *s == p)) {
            seeds[..=at].rotate_right(1);
        }
        let mut last = anyhow!("no seed carrier is known");
        for seed in seeds {
            match self.link_through(circuit, blobs, seed).await {
                Ok(linked) => return Ok(linked),
                Err(e) => {
                    debug!(hop = %crate::machine::words(&seed), "couldn't link to a seed carrier: {e:#}");
                    last = e;
                }
            }
        }
        Err(last)
    }

    /// Links to the carrier of the machine `hop`, and answers circuits on
    /// that link. The link, and when its hop's circuit key runs out.
    async fn link_through(
        self: &Arc<Self>,
        circuit: &HostCircuit,
        blobs: &BlobsProtocol,
        hop: EndpointId,
    ) -> Result<(Live, u64)> {
        let greeting = self.machine()?.hello(hop).await?;
        let now = unix_now();
        let descriptor = carrier_of(&greeting.info, now)
            .context("its machine record names no carrier that checks out")?;
        let info = descriptor.verify(now)?;
        let relays = self.tracker.relays().clone();
        let relay = relay_mode(&self.config)?.context("relays are off")?;
        // A key for this link alone, so nothing ties it to this node's
        // public ID or machine name.
        let link_endpoint =
            relayed_endpoint(&self.config, relay, &relays, SecretKey::generate(), vec![])
                .await
                .context("starting the endpoint for the link to the hop")?;
        let linked = async {
            let dial = osp_carrier::relay_dial(link_endpoint.clone(), move |url| {
                relays.permits(&TransportAddr::Relay(url.clone()))
            });
            let conn = tokio::time::timeout(SETUP_TIMEOUT, dial(info.clone()))
                .await
                .map_err(|_| anyhow!("its carrier didn't answer in time"))??;
            let link = HostLink::open(conn).await?;
            let sender = link.sender();
            let endpoint = osp_carrier::host_endpoint(
                link,
                circuit.key.clone(),
                vec![
                    iroh_blobs::ALPN.to_vec(),
                    MEDIA_HOSTS_ALPN.to_vec(),
                    REPLIES_ALPN.to_vec(),
                ],
            )
            .await?;
            // What the media endpoint answers, and nothing else.
            let router = Router::builder(endpoint)
                .accept(iroh_blobs::ALPN, SharedBlobs(blobs.clone()))
                .accept(MEDIA_HOSTS_ALPN, MediaHostsProtocol::new(self.clone()))
                .accept(REPLIES_ALPN, RepliesProtocol::new(self.clone()))
                .spawn();
            anyhow::Ok((sender, router))
        }
        .await;
        let (sender, router) = match linked {
            Ok(linked) => linked,
            Err(e) => {
                link_endpoint.close().await;
                return Err(e);
            }
        };
        let carrier = data_encoding::BASE64URL_NOPAD.encode(&descriptor.to_bytes());
        Ok((
            Live {
                hop,
                carrier,
                sender,
                link_endpoint,
                router,
            },
            info.valid_until,
        ))
    }
}

/// Keeps a link to a hop while the node runs: builds one, hands it out, and
/// builds another when it closes or its hop's circuit key is replaced. Only
/// with seed carriers known.
pub(crate) async fn keep_linked(core: Arc<Core>, blobs: BlobsProtocol) {
    let Some(circuit) = &core.host_circuit else {
        return;
    };
    let mut wait = FIRST_WAIT;
    let mut hop = None;
    loop {
        if let Some(tor) = &core.tor {
            tor.ready().await;
        }
        match core.link_to_a_hop(circuit, &blobs, hop).await {
            Ok((live, key_until)) => {
                let sender = live.sender.clone();
                if hop != Some(live.hop) {
                    info!(hop = %crate::machine::words(&live.hop), "media can be reached through a carrier");
                }
                hop = Some(live.hop);
                // The new link is handed out before the old one goes.
                if let Some(old) = circuit.swap(Some(live)) {
                    old.end().await;
                }
                wait = FIRST_WAIT;
                let renew = Duration::from_secs(key_until.saturating_sub(unix_now())) + AFTER_KEY;
                tokio::select! {
                    _ = sender.closed() => debug!("the link to the hop closed; linking again"),
                    _ = tokio::time::sleep_until(Instant::now() + renew) => {
                        debug!("the hop's circuit key was replaced; linking again")
                    }
                }
            }
            Err(e) => {
                debug!("no link to a carrier for now: {e:#}");
                if circuit.hop().is_none()
                    && let Some(old) = circuit.swap(None)
                {
                    old.end().await;
                }
                tokio::time::sleep(jitter(wait)).await;
                wait = (wait * 2).min(LONGEST_WAIT);
            }
        }
    }
}

/// Why a node keeps no link to a hop, or None if it does.
pub(crate) fn why_not(core_has_machine: bool, media: bool, known: usize) -> Option<&'static str> {
    if known == 0 {
        Some("no seed carriers are known")
    } else if !core_has_machine {
        Some("it has no machine name to read carriers' records with")
    } else if !media {
        Some("it has no media endpoint, or relays are off")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_needs_seeds_a_machine_name_and_a_media_endpoint() {
        assert!(why_not(true, true, 1).is_none());
        assert!(why_not(true, true, 0).is_some());
        assert!(why_not(false, true, 1).is_some());
        assert!(why_not(true, false, 1).is_some());
    }

    #[test]
    fn no_link_no_offer_and_the_key_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path().to_path_buf());
        let circuit = HostCircuit::load(&home).unwrap();
        assert!(circuit.offer().is_none());
        assert!(circuit.hop().is_none());
        // The key is kept.
        let again = HostCircuit::load(&home).unwrap();
        assert_eq!(again.endpoint_id(), circuit.endpoint_id());
    }
}
