//! Which relays the node connects through (ADR 0007).
//!
//! iroh connects to whatever relay a peer's address names. Addresses come
//! from other people (tickets, subscriptions, `WhoHosts` replies, DNS
//! records), so a peer could point the node at a server of its own: in
//! relay-only mode that server would learn the node's IP address, and in
//! any mode it could be a device on the home network. Every address from
//! outside therefore passes through a [`RelayPolicy`] before iroh uses it.

use std::{net::IpAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayConfig, RelayMap, RelayMode, RelayUrl, TransportAddr,
    Watcher,
    address_lookup::{
        AddressLookup, AddressLookupBuilder, AddressLookupBuilderError, EndpointData, EndpointInfo,
        Error, Item,
    },
    endpoint::presets,
};
use n0_future::{StreamExt, boxed::BoxStream};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::config::{NetworkConfig, RelayChoice};

#[derive(Debug, Clone)]
pub(crate) struct RelayPolicy {
    /// The relays in this node's own configuration, always allowed.
    own: Vec<RelayUrl>,
    /// Relay-only mode: no other relays, and no IP addresses.
    relay_only: bool,
    /// Relays also allowed while none of `own` lets the node in.
    fallback: Option<Arc<RelayFallback>>,
}

impl RelayPolicy {
    pub fn new(own: Vec<RelayUrl>, relay_only: bool) -> Self {
        Self {
            own,
            relay_only,
            fallback: None,
        }
    }

    pub fn from_config(net: &NetworkConfig) -> Result<Self> {
        Ok(Self {
            fallback: RelayFallback::from_config(net)?,
            ..Self::new(own_relays(&net.relay)?, net.relay_only)
        })
    }

    /// The same relays, fallback included, with no IP addresses: for the
    /// endpoints that are relay-only whatever the node's setting.
    pub fn relay_only(&self) -> Self {
        Self {
            relay_only: true,
            ..self.clone()
        }
    }

    pub fn fallback(&self) -> Option<&Arc<RelayFallback>> {
        self.fallback.as_ref()
    }

    /// Whether the node may use this part of a peer's address.
    pub fn permits(&self, addr: &TransportAddr) -> bool {
        match addr {
            TransportAddr::Relay(url) => {
                self.own.contains(url)
                    || self.fallback.as_ref().is_some_and(|f| f.allows(url))
                    || (!self.relay_only && is_public_https(url))
            }
            // Without IP sockets they're no use, so they aren't kept either.
            TransportAddr::Ip(_) => !self.relay_only,
            _ => false,
        }
    }

    /// The parts of `addr` the node may use.
    pub fn filter(&self, addr: &EndpointAddr) -> EndpointAddr {
        EndpointAddr::from_parts(
            addr.id,
            addr.addrs.iter().filter(|a| self.permits(a)).cloned(),
        )
    }

    /// The relays in `addr` the node won't use, for error messages.
    pub fn refused_relays(&self, addr: &EndpointAddr) -> Vec<String> {
        addr.addrs
            .iter()
            .filter(|a| a.is_relay() && !self.permits(a))
            .map(|a| match a {
                TransportAddr::Relay(url) => url.to_string(),
                other => format!("{other:?}"),
            })
            .collect()
    }
}

/// The relays this node itself uses, as configured.
pub(crate) fn own_relays(choice: &RelayChoice) -> Result<Vec<RelayUrl>> {
    Ok(match choice {
        RelayChoice::Default => iroh::endpoint::default_relay_mode().relay_map().urls(),
        RelayChoice::Disabled => Vec::new(),
        RelayChoice::Custom(urls) => urls
            .iter()
            .map(|u| u.parse::<RelayUrl>())
            .collect::<Result<_, _>>()
            .context("invalid relay URL in config")?,
    })
}

/// How long none of the node's own relays may fail to let it in before it
/// turns to its fallback relays (ADR 0044).
pub(crate) const FALLBACK_AFTER: Duration = Duration::from_secs(60);
/// How often a node on its fallback relays checks whether its own let it in
/// again.
pub(crate) const PROBE_EVERY: Duration = Duration::from_secs(120);
/// How long a check waits for one of its own relays to let it in.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Relays a node uses only while none of its own lets it in: n0's, behind
/// the project's relays (ADR 0044). iroh picks its home relay by latency,
/// not by order, so given both it would often pick n0's; the fallback
/// relays are therefore added to every endpoint's relays only while the
/// node's own fail, and taken out once one lets it in again.
#[derive(Debug)]
pub(crate) struct RelayFallback {
    relays: Vec<RelayUrl>,
    /// Whether the node is on its fallback relays now.
    on: watch::Sender<bool>,
}

impl RelayFallback {
    fn from_config(net: &NetworkConfig) -> Result<Option<Arc<Self>>> {
        let relays = own_relays(&net.relay_fallback)?;
        Ok((!relays.is_empty()).then(|| {
            Arc::new(Self {
                relays,
                on: watch::Sender::new(false),
            })
        }))
    }

    pub fn is_on(&self) -> bool {
        *self.on.borrow()
    }

    fn allows(&self, url: &RelayUrl) -> bool {
        self.is_on() && self.relays.contains(url)
    }

    /// Keeps `endpoint`'s relays in step with the fallback, until it closes.
    pub fn follow(self: &Arc<Self>, endpoint: &Endpoint) {
        let (fallback, endpoint) = (self.clone(), endpoint.clone());
        let mut on = self.on.subscribe();
        tokio::spawn(async move {
            loop {
                let now = *on.borrow_and_update();
                for url in &fallback.relays {
                    if now {
                        let config = RelayConfig::from(url.clone());
                        endpoint.insert_relay(url.clone(), Arc::new(config)).await;
                    } else {
                        endpoint.remove_relay(url).await;
                    }
                }
                tokio::select! {
                    changed = on.changed() => if changed.is_err() { break },
                    _ = endpoint.closed() => break,
                }
            }
        });
    }

    /// Watches the node's main endpoint, and turns the fallback on when
    /// none of `own` has let it in for `after`, and off once a check finds
    /// that one does again. Stops when the endpoint closes.
    pub fn watch(
        self: &Arc<Self>,
        endpoint: &Endpoint,
        own: RelayMap,
        after: Duration,
        probe_every: Duration,
    ) {
        let (fallback, endpoint) = (self.clone(), endpoint.clone());
        let own_urls = own.urls::<Vec<_>>();
        let step = (after / 4).min(Duration::from_secs(5));
        tokio::spawn(async move {
            let mut down_for = Duration::ZERO;
            while !endpoint.is_closed() {
                if fallback.is_on() {
                    tokio::time::sleep(probe_every).await;
                    if lets_in(own.clone()).await {
                        info!(
                            "one of the node's own relays lets it in again: off the fallback relays"
                        );
                        fallback.on.send_replace(false);
                        down_for = Duration::ZERO;
                    }
                    continue;
                }
                tokio::time::sleep(step).await;
                let status = endpoint.home_relay_status().get();
                let admitted = status
                    .iter()
                    .any(|s| s.is_connected() && own_urls.contains(s.url()));
                if admitted {
                    down_for = Duration::ZERO;
                    continue;
                }
                down_for += step;
                if let Some(s) = status.iter().find(|s| own_urls.contains(s.url())) {
                    debug!(relay = %s.url(), error = ?s.last_error(), "not let in by own relay");
                }
                if down_for >= after {
                    warn!(
                        relays = ?fallback.relays,
                        "none of the node's own relays lets it in: using the fallback relays until one does"
                    );
                    fallback.on.send_replace(true);
                }
            }
        });
    }
}

/// Whether a relay in `own` lets in a new endpoint that has nothing else to
/// connect through.
async fn lets_in(own: RelayMap) -> bool {
    let endpoint = match iroh::Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(own))
        .clear_ip_transports()
        .bind()
        .await
    {
        Ok(endpoint) => endpoint,
        Err(e) => {
            debug!("couldn't start an endpoint to check the relays: {e:#}");
            return false;
        }
    };
    let online = tokio::time::timeout(PROBE_TIMEOUT, endpoint.online())
        .await
        .is_ok();
    endpoint.close().await;
    online
}

/// An https relay on the internet, not on this machine or a private network.
/// A host name that resolves to a private address isn't caught; this only
/// keeps peers from naming one outright.
fn is_public_https(url: &RelayUrl) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(ip) => is_public_ip(ip),
        // A name without a dot is looked up on the local network.
        Err(_) => host.contains('.') && !host.ends_with(".localhost") && !host.ends_with(".local"),
    }
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            let shared = a == 100 && (b & 0xc0) == 64; // 100.64.0.0/10, carrier-grade NAT
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || shared)
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public_ip(v4.into()),
            None => {
                !(v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || v6.is_unique_local()
                    || v6.is_unicast_link_local())
            }
        },
    }
}

/// An address lookup whose answers pass a [`RelayPolicy`] before iroh uses
/// them.
#[derive(Debug)]
pub(crate) struct Checked<T> {
    inner: T,
    policy: RelayPolicy,
}

impl<T> Checked<T> {
    pub fn new(inner: T, policy: RelayPolicy) -> Self {
        Self { inner, policy }
    }
}

impl<B: AddressLookupBuilder> AddressLookupBuilder for Checked<B> {
    fn into_address_lookup(
        self,
        endpoint: &Endpoint,
    ) -> Result<impl AddressLookup, AddressLookupBuilderError> {
        Ok(CheckedLookup {
            inner: self.inner.into_address_lookup(endpoint)?,
            policy: self.policy,
        })
    }
}

#[derive(Debug)]
struct CheckedLookup<T> {
    inner: T,
    policy: RelayPolicy,
}

impl<T: AddressLookup> AddressLookup for CheckedLookup<T> {
    fn publish(&self, data: &EndpointData) {
        self.inner.publish(data);
    }

    fn resolve(&self, endpoint_id: EndpointId) -> Option<BoxStream<Result<Item, Error>>> {
        let policy = self.policy.clone();
        let items = self.inner.resolve(endpoint_id)?;
        Some(Box::pin(items.map(move |item| {
            item.map(|item| {
                let addr = policy.filter(&item.to_endpoint_addr());
                Item::new(
                    EndpointInfo::from(addr),
                    item.provenance(),
                    item.last_updated(),
                )
            })
        })))
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;

    fn url(s: &str) -> RelayUrl {
        s.parse().unwrap()
    }

    fn relay(s: &str) -> TransportAddr {
        TransportAddr::Relay(url(s))
    }

    const OWN: &str = "https://relay.mine.example/";

    #[test]
    fn relay_only_uses_nothing_but_its_own_relays() {
        let policy = RelayPolicy::new(vec![url(OWN)], true);
        assert!(policy.permits(&relay(OWN)));
        assert!(!policy.permits(&relay("https://relay.theirs.example/")));
        let ip = TransportAddr::Ip("203.0.113.7:4433".parse::<SocketAddr>().unwrap());
        assert!(!policy.permits(&ip));

        let id = iroh::SecretKey::generate().public();
        let addr =
            EndpointAddr::from_parts(id, [relay(OWN), relay("https://relay.theirs.example/"), ip]);
        assert_eq!(
            policy.filter(&addr),
            EndpointAddr::from_parts(id, [relay(OWN)])
        );
        assert_eq!(
            policy.refused_relays(&addr),
            vec!["https://relay.theirs.example/".to_string()]
        );
    }

    #[test]
    fn direct_mode_takes_public_https_relays_only() {
        let policy = RelayPolicy::new(vec![url("http://127.0.0.1:3340/")], false);
        // Its own relays always, even plain http on this machine.
        assert!(policy.permits(&relay("http://127.0.0.1:3340/")));
        assert!(policy.permits(&relay("https://relay.theirs.example/")));
        assert!(policy.permits(&relay("https://8.8.8.8/")));
        for refused in [
            "http://relay.theirs.example/",
            "https://198.51.100.1/",
            "https://localhost/",
            "https://router/",
            "https://printer.local/",
            "https://127.0.0.1/",
            "https://192.168.0.1/",
            "https://10.0.0.1:8443/",
            "https://100.64.0.1/",
            "https://169.254.1.1/",
            "https://[::1]/",
            "https://[fd00::1]/",
            "https://[fe80::1]/",
            "https://[::ffff:192.168.0.1]/",
        ] {
            assert!(!policy.permits(&relay(refused)), "{refused}");
        }
        let ip = TransportAddr::Ip("203.0.113.7:4433".parse::<SocketAddr>().unwrap());
        assert!(policy.permits(&ip), "direct mode keeps IP addresses");
    }
}
