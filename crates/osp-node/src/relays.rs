//! Which relays the node connects through (ADR 0007).
//!
//! iroh connects to whatever relay a peer's address names. Addresses come
//! from other people (tickets, subscriptions, `WhoHosts` replies, DNS
//! records), so a peer could point the node at a server of its own: in
//! relay-only mode that server would learn the node's IP address, and in
//! any mode it could be a device on the home network. Every address from
//! outside therefore passes through a [`RelayPolicy`] before iroh uses it.

use std::net::IpAddr;

use anyhow::{Context, Result};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayUrl, TransportAddr,
    address_lookup::{
        AddressLookup, AddressLookupBuilder, AddressLookupBuilderError, EndpointData, EndpointInfo,
        Error, Item,
    },
};
use n0_future::{StreamExt, boxed::BoxStream};

use crate::config::{NetworkConfig, RelayChoice};

#[derive(Debug, Clone)]
pub(crate) struct RelayPolicy {
    /// The relays in this node's own configuration, always allowed.
    own: Vec<RelayUrl>,
    /// Relay-only mode: no other relays, and no IP addresses.
    relay_only: bool,
}

impl RelayPolicy {
    pub fn new(own: Vec<RelayUrl>, relay_only: bool) -> Self {
        Self { own, relay_only }
    }

    pub fn from_config(net: &NetworkConfig) -> Result<Self> {
        Ok(Self::new(own_relays(&net.relay)?, net.relay_only))
    }

    /// Whether the node may use this part of a peer's address.
    pub fn permits(&self, addr: &TransportAddr) -> bool {
        match addr {
            TransportAddr::Relay(url) => {
                self.own.contains(url) || (!self.relay_only && is_public_https(url))
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
