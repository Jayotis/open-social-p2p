//! Which relays the node connects through (ADR 0007).
//!
//! iroh connects to whatever relay a peer's address names. Addresses come
//! from other people (tickets, subscriptions, `WhoHosts` replies, DNS
//! records), so a peer could point the node at a server of its own: in
//! relay-only mode that server would learn the node's IP address, and in
//! any mode it could be a device on the home network. Every address from
//! outside therefore passes through a [`RelayPolicy`] before iroh uses it.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayConfig, RelayMap, RelayMode, RelayUrl, TransportAddr,
    Watcher,
    address_lookup::{
        AddressLookup, AddressLookupBuilder, AddressLookupBuilderError, EndpointData, EndpointInfo,
        Error, Item,
    },
    dns::{BoxIter, DnsError, DnsResolver, Resolver, TxtRecordData},
    endpoint::presets,
};
use n0_future::{
    StreamExt,
    boxed::{BoxFuture, BoxStream},
};
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
    /// What the endpoints look names up with: no private addresses but for
    /// `own`'s.
    dns: DnsResolver,
}

impl RelayPolicy {
    pub fn new(own: Vec<RelayUrl>, relay_only: bool) -> Self {
        Self {
            dns: public_answers(DnsResolver::new(), &own),
            own,
            relay_only,
            fallback: None,
        }
    }

    /// The resolver for endpoints under this policy.
    pub fn dns(&self) -> &DnsResolver {
        &self.dns
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

/// Leaves private addresses out of A and AAAA answers, for any host but
/// those of the node's own relays (a lab's may be on the home network). A
/// peer can name a relay whose host name points into Kim's network:
/// `is_public_https` checks only the name, and this where it leads, at the
/// lookup iroh connects by.
#[derive(Debug, Clone)]
struct PublicAnswers {
    inner: DnsResolver,
    own: Arc<Vec<String>>,
}

/// Longest the lookup under `PublicAnswers` takes. iroh's own timeout, from
/// the outside, is shorter.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

/// `inner`, leaving out private addresses but for the hosts of `own`.
fn public_answers(inner: DnsResolver, own: &[RelayUrl]) -> DnsResolver {
    let own = own
        .iter()
        .filter_map(|u| u.host_str())
        .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
        .collect();
    DnsResolver::custom(PublicAnswers {
        inner,
        own: Arc::new(own),
    })
}

impl PublicAnswers {
    /// Whether every address `host` resolves to is kept, private or not.
    fn keeps_all(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        self.own.contains(&host)
    }
}

impl Resolver for PublicAnswers {
    fn lookup_ipv4(&self, host: String) -> BoxFuture<Result<BoxIter<Ipv4Addr>, DnsError>> {
        let (inner, all) = (self.inner.clone(), self.keeps_all(&host));
        Box::pin(async move {
            let found = inner.lookup_ipv4(&host, LOOKUP_TIMEOUT).await?;
            let kept: Vec<Ipv4Addr> = found
                .filter(|a| all || is_public_ip(*a))
                .filter_map(|a| match a {
                    IpAddr::V4(v4) => Some(v4),
                    IpAddr::V6(_) => None,
                })
                .collect();
            if kept.is_empty() {
                debug!(%host, "a name that leads only to private addresses");
            }
            Ok(Box::new(kept.into_iter()) as BoxIter<Ipv4Addr>)
        })
    }

    fn lookup_ipv6(&self, host: String) -> BoxFuture<Result<BoxIter<Ipv6Addr>, DnsError>> {
        let (inner, all) = (self.inner.clone(), self.keeps_all(&host));
        Box::pin(async move {
            let found = inner.lookup_ipv6(&host, LOOKUP_TIMEOUT).await?;
            let kept: Vec<Ipv6Addr> = found
                .filter(|a| all || is_public_ip(*a))
                .filter_map(|a| match a {
                    IpAddr::V6(v6) => Some(v6),
                    IpAddr::V4(_) => None,
                })
                .collect();
            Ok(Box::new(kept.into_iter()) as BoxIter<Ipv6Addr>)
        })
    }

    fn lookup_txt(&self, host: String) -> BoxFuture<Result<BoxIter<TxtRecordData>, DnsError>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let found: Vec<TxtRecordData> =
                inner.lookup_txt(&host, LOOKUP_TIMEOUT).await?.collect();
            Ok(Box::new(found.into_iter()) as BoxIter<TxtRecordData>)
        })
    }

    fn clear_cache(&self) {
        self.inner.clear_cache();
    }

    fn reset(&self) -> Box<dyn Resolver> {
        self.inner.reset();
        Box::new(self.clone())
    }
}

/// An https relay on the internet, not on this machine or a private network,
/// by its name: `PublicAnswers` checks where a host name leads.
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

    /// Answers every name with the same addresses.
    #[derive(Debug, Clone)]
    struct Fixed(Vec<IpAddr>);

    impl Resolver for Fixed {
        fn lookup_ipv4(&self, _: String) -> BoxFuture<Result<BoxIter<Ipv4Addr>, DnsError>> {
            let v4: Vec<Ipv4Addr> = self
                .0
                .iter()
                .filter_map(|a| match a {
                    IpAddr::V4(v4) => Some(*v4),
                    IpAddr::V6(_) => None,
                })
                .collect();
            Box::pin(async move { Ok(Box::new(v4.into_iter()) as BoxIter<Ipv4Addr>) })
        }

        fn lookup_ipv6(&self, _: String) -> BoxFuture<Result<BoxIter<Ipv6Addr>, DnsError>> {
            let v6: Vec<Ipv6Addr> = self
                .0
                .iter()
                .filter_map(|a| match a {
                    IpAddr::V6(v6) => Some(*v6),
                    IpAddr::V4(_) => None,
                })
                .collect();
            Box::pin(async move { Ok(Box::new(v6.into_iter()) as BoxIter<Ipv6Addr>) })
        }

        fn lookup_txt(&self, _: String) -> BoxFuture<Result<BoxIter<TxtRecordData>, DnsError>> {
            Box::pin(async { Ok(Box::new(std::iter::empty()) as BoxIter<TxtRecordData>) })
        }

        fn clear_cache(&self) {}

        fn reset(&self) -> Box<dyn Resolver> {
            Box::new(self.clone())
        }
    }

    /// A router, a public address, a unique-local one, a public one, loopback.
    fn answers() -> DnsResolver {
        let addrs = [
            "192.168.2.1",
            "8.8.8.8",
            "fd00::1",
            "2001:4860:4860::8888",
            "127.0.0.1",
        ];
        DnsResolver::custom(Fixed(addrs.iter().map(|a| a.parse().unwrap()).collect()))
    }

    async fn v4(dns: &DnsResolver, host: &str) -> Vec<IpAddr> {
        let timeout = Duration::from_secs(5);
        dns.lookup_ipv4(host, timeout).await.unwrap().collect()
    }

    async fn v6(dns: &DnsResolver, host: &str) -> Vec<IpAddr> {
        let timeout = Duration::from_secs(5);
        dns.lookup_ipv6(host, timeout).await.unwrap().collect()
    }

    fn ips(addrs: &[&str]) -> Vec<IpAddr> {
        addrs.iter().map(|a| a.parse().unwrap()).collect()
    }

    /// A peer names `https://relay.evil.example`, whose records point at
    /// Kim's router: the node's lookups leave those out, but keep a lab
    /// relay of its own on the home network.
    #[tokio::test]
    async fn a_name_leads_into_the_home_network_only_for_the_nodes_own_relays() {
        let dns = public_answers(answers(), &[url(OWN)]);
        assert_eq!(v4(&dns, "relay.evil.example").await, ips(&["8.8.8.8"]));
        assert_eq!(
            v6(&dns, "relay.evil.example").await,
            ips(&["2001:4860:4860::8888"])
        );
        let all = ips(&["192.168.2.1", "8.8.8.8", "127.0.0.1"]);
        assert_eq!(v4(&dns, "relay.mine.example").await, all);
        assert_eq!(v4(&dns, "Relay.Mine.Example.").await, all);
        assert_eq!(
            v6(&dns, "relay.mine.example").await,
            ips(&["fd00::1", "2001:4860:4860::8888"])
        );
    }

    /// The endpoints the node builds look names up through its policy's
    /// resolver.
    #[tokio::test]
    async fn the_nodes_endpoints_look_names_up_through_the_policy() {
        let mut policy = RelayPolicy::new(vec![url(OWN)], false);
        policy.dns = public_answers(answers(), &policy.own);
        let net = crate::Config::local_test("Kim").network;
        let endpoint = crate::node::endpoint_builder(
            &net,
            &iroh::address_lookup::MemoryLookup::new(),
            &policy,
        )
        .unwrap()
        .bind()
        .await
        .unwrap();
        let dns = endpoint.dns_resolver().unwrap();
        assert_eq!(v4(dns, "relay.evil.example").await, ips(&["8.8.8.8"]));
        endpoint.close().await;
    }

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
