//! Which relays the phone connects through (ADR 0044).
//!
//! Until it pairs, and with a node that has no relay token, the phone uses
//! n0's public relays. A node with a token passes on its own relays and the
//! token in the pairing code, and the phone then uses only those, as its node
//! does. With `fallback` it adds n0's while none of them lets it in for a
//! minute, and drops n0's again once a check every two minutes finds one does.
//! iroh picks its home relay by latency, not by order, so given both at once
//! it would often pick n0's.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use iroh::{Endpoint, RelayConfig, RelayMap, RelayMode, RelayUrl, Watcher, endpoint::presets};
use osp_proto::PairingRelays;
use tokio::task::AbortHandle;
use tracing::{debug, info, warn};

/// How long none of the phone's own relays may fail to let it in before it
/// turns to n0's. The same as a node's.
const FALLBACK_AFTER: Duration = Duration::from_secs(60);
/// How often a phone on n0's relays checks whether its own let it in again.
const PROBE_EVERY: Duration = Duration::from_secs(120);
/// How long a check waits for one of its own relays to let it in.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// The relay mode to start the endpoint with: the node's relays if the
/// pairing has them, otherwise n0's; none when relays are off (tests).
pub(crate) fn initial_mode(enabled: bool, relays: Option<&PairingRelays>) -> Result<RelayMode> {
    Ok(match (enabled, relays) {
        (false, _) => RelayMode::Disabled,
        (true, None) => RelayMode::Default,
        (true, Some(relays)) => RelayMode::Custom(own_map(relays)?),
    })
}

/// The node's relays, with the token.
fn own_map(relays: &PairingRelays) -> Result<RelayMap> {
    let urls = relays
        .urls
        .iter()
        .map(|u| u.parse::<RelayUrl>())
        .collect::<Result<Vec<_>, _>>()
        .context("the pairing code names an invalid relay")?;
    Ok(RelayMap::from_iter(urls).with_auth_token(relays.token.as_str()))
}

fn n0_urls() -> Vec<RelayUrl> {
    RelayMode::Default.relay_map().urls()
}

/// Keeps the endpoint's relays in step with the pairing.
pub(crate) struct Relays {
    endpoint: Endpoint,
    /// Off in tests: the endpoint then has no relays at all.
    enabled: bool,
    state: tokio::sync::Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The node's relays the endpoint has now, if it's using them.
    own: Vec<RelayUrl>,
    /// The task that turns n0's relays on and off.
    watcher: Option<AbortHandle>,
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(w) = self.watcher.take() {
            w.abort();
        }
    }
}

impl Relays {
    /// For an endpoint started with `initial_mode(enabled, relays)`.
    pub fn new(endpoint: Endpoint, enabled: bool, relays: Option<&PairingRelays>) -> Result<Self> {
        let mut state = State::default();
        if let (true, Some(relays)) = (enabled, relays) {
            let map = own_map(relays)?;
            state.own = map.urls();
            if relays.fallback {
                state.watcher = Some(watch(endpoint.clone(), map));
            }
        }
        Ok(Self {
            endpoint,
            enabled,
            state: tokio::sync::Mutex::new(state),
        })
    }

    /// Switches to the relays a pairing names: the node's own with their
    /// token, or n0's for `None`.
    pub async fn set(&self, relays: Option<&PairingRelays>) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let map = relays.map(own_map).transpose()?;
        let mut state = self.state.lock().await;
        if let Some(w) = state.watcher.take() {
            w.abort();
        }
        let wanted = match &map {
            Some(map) => map.urls(),
            None => n0_urls(),
        };
        // The new ones first, so the endpoint is never without a relay.
        match &map {
            Some(map) => {
                for url in &wanted {
                    if let Some(config) = map.get(url) {
                        self.endpoint.insert_relay(url.clone(), config).await;
                    }
                }
            }
            None => insert(&self.endpoint, &wanted).await,
        }
        // Whatever else may be there: the old node's relays, or n0's,
        // left on by the watcher or from before pairing.
        for url in state.own.iter().chain(&n0_urls()) {
            if !wanted.contains(url) {
                self.endpoint.remove_relay(url).await;
            }
        }
        state.own = map.as_ref().map(|m| m.urls()).unwrap_or_default();
        if let (Some(map), Some(relays)) = (map, relays)
            && relays.fallback
        {
            state.watcher = Some(watch(self.endpoint.clone(), map));
        }
        Ok(())
    }
}

/// n0's relays, without the token.
async fn insert(endpoint: &Endpoint, urls: &[RelayUrl]) {
    for url in urls {
        let config = Arc::new(RelayConfig::from(url.clone()));
        endpoint.insert_relay(url.clone(), config).await;
    }
}

/// Adds n0's relays to the endpoint while none of `own` lets it in, as a
/// node does with its fallback.
fn watch(endpoint: Endpoint, own: RelayMap) -> AbortHandle {
    let own_urls = own.urls::<Vec<_>>();
    let step = Duration::from_secs(5);
    tokio::spawn(async move {
        let mut down_for = Duration::ZERO;
        let mut on_n0 = false;
        while !endpoint.is_closed() {
            if on_n0 {
                tokio::time::sleep(PROBE_EVERY).await;
                if lets_in(own.clone()).await {
                    info!("the node's relays let the phone in again: off n0's");
                    for url in n0_urls() {
                        endpoint.remove_relay(&url).await;
                    }
                    on_n0 = false;
                    down_for = Duration::ZERO;
                }
                continue;
            }
            tokio::time::sleep(step).await;
            let status = endpoint.home_relay_status().get();
            if status
                .iter()
                .any(|s| s.is_connected() && own_urls.contains(s.url()))
            {
                down_for = Duration::ZERO;
                continue;
            }
            down_for += step;
            if let Some(s) = status.iter().find(|s| own_urls.contains(s.url())) {
                debug!(relay = %s.url(), error = ?s.last_error(), "not let in by the node's relay");
            }
            if down_for >= FALLBACK_AFTER {
                warn!("none of the node's relays lets the phone in: using n0's until one does");
                insert(&endpoint, &n0_urls()).await;
                on_n0 = true;
            }
        }
    })
    .abort_handle()
}

/// Whether a relay in `own` lets in a new endpoint that has nothing else to
/// connect through.
async fn lets_in(own: RelayMap) -> bool {
    let endpoint = match Endpoint::builder(presets::Minimal)
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
