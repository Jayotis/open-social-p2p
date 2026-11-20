//! The node's carrier (ADR 0043 and its design note): every node carries other
//! nodes' encrypted media from the middle of a path, under its machine name.
//!
//! Say Kim's new node sits between Jimmy's hop and Alice's on one of Jimmy's
//! fetches. Kim's node runs a [`Carrier`] on an endpoint of its own, with a
//! relay and published nowhere: no DNS or pkarr record. Its machine record
//! names that endpoint in a descriptor signed by its machine key, and Jimmy's
//! node reaches it by that ID through the relay the descriptor names. Kim's
//! node sees the two hops next to it, never Jimmy's or Alice's address, and
//! never what it carries.
//!
//! The carrier endpoint's key is kept, so a descriptor stays good while its
//! circuit key does, and goes when the machine name does.

use std::{
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Context, Result};
use iroh::{Endpoint, RelayMode, SecretKey, TransportAddr, endpoint::presets};
use n0_future::task::AbortOnDropHandle;
use osp_carrier::{Carrier, Dial, Limits};
use tracing::{debug, info, warn};

use crate::{
    config::{Config, RelayChoice},
    home::Home,
    keys,
    machine::{MachineSide, Published},
    node::own_relay_map,
    relays::RelayPolicy,
};

/// How often the carrier endpoint's relay is looked at.
const RELAY_CHECK: Duration = Duration::from_secs(5);
/// How often the descriptor in the machine record is renewed. A circuit key
/// lasts a day, so this only needs to catch its replacement soon.
const RENEW: Duration = Duration::from_secs(60);

pub(crate) struct CarrierSide {
    /// The endpoint it carries on.
    pub endpoint: Endpoint,
    /// The carrier, once the endpoint has a relay to name. Hosts' slots and
    /// status read it from part 4b on.
    #[allow(dead_code)]
    carrier: Arc<OnceLock<Carrier>>,
    _task: AbortOnDropHandle<()>,
}

impl CarrierSide {
    /// Starts carrying, unless `[carrier]` is off. It needs the machine name,
    /// which signs its descriptor, and a relay, which neighbours reach it
    /// through.
    pub async fn start(
        home: &Home,
        config: &Config,
        machine: &MachineSide,
        relays: &RelayPolicy,
    ) -> Result<Option<Self>> {
        if !config.carrier.enabled {
            return Ok(None);
        }
        let net = &config.network;
        let relay_mode = match &net.relay {
            RelayChoice::Disabled => {
                info!("not carrying for other nodes: carrying needs a relay, and relays are off");
                return Ok(None);
            }
            RelayChoice::Default => RelayMode::Default,
            RelayChoice::Custom(_) => RelayMode::Custom(own_relay_map(net)?),
        };
        let path = home.carrier_key();
        let key = match path.exists() {
            true => keys::load(&path)?,
            false => keys::create(&path)?,
        };
        let endpoint = carrier_endpoint(config, relay_mode, key).await?;
        if let Some(fallback) = relays.fallback() {
            fallback.follow(&endpoint);
        }
        let limits = match config.carrier.seed {
            true => Limits::seed(),
            false => Limits::home(),
        };
        // It dials only through relays the owner's policy takes.
        let policy = relays.clone();
        let dial = osp_carrier::relay_dial(endpoint.clone(), move |url| {
            policy.permits(&TransportAddr::Relay(url.clone()))
        });
        let carrier = Arc::new(OnceLock::new());
        let task = tokio::spawn(keep_carrying(
            endpoint.clone(),
            machine.key().clone(),
            machine.published(),
            limits,
            dial,
            carrier.clone(),
        ));
        Ok(Some(Self {
            endpoint,
            carrier,
            _task: AbortOnDropHandle::new(task),
        }))
    }

    /// The carrier, once it's running.
    #[allow(dead_code)]
    pub fn carrier(&self) -> Option<&Carrier> {
        self.carrier.get()
    }
}

/// An endpoint for carrying: a relay, no address lookup and nothing
/// published, so neighbours reach it only by the ID and relay in its
/// descriptor. A relay-only node binds no IP sockets here either.
async fn carrier_endpoint(
    config: &Config,
    relay_mode: RelayMode,
    key: SecretKey,
) -> Result<Endpoint> {
    let net = &config.network;
    let mut builder = Endpoint::builder(presets::Minimal)
        .relay_mode(relay_mode)
        .alpns(vec![osp_carrier::ALPN.to_vec()])
        .secret_key(key);
    if net.relay_only {
        builder = builder.clear_ip_transports();
    } else if let Some(bind) = net.bind {
        // On the main endpoint's address, on a port of its own.
        builder = builder.bind_addr(SocketAddr::new(bind.ip(), 0))?;
    }
    builder
        .bind()
        .await
        .context("starting the carrier endpoint")
}

/// Starts the carrier once its endpoint has a relay, tells it when the relay
/// moves, and keeps its descriptor in the machine record fresh. Stops when
/// the endpoint closes.
async fn keep_carrying(
    endpoint: Endpoint,
    machine: SecretKey,
    published: Published,
    limits: Limits,
    dial: Dial,
    cell: Arc<OnceLock<Carrier>>,
) {
    let mut named: Option<String> = None;
    let mut renewed: Option<tokio::time::Instant> = None;
    loop {
        let relay = endpoint
            .addr()
            .relay_urls()
            .next()
            .map(|url| url.to_string());
        if let Some(relay) = relay
            && named.as_ref() != Some(&relay)
        {
            match cell.get() {
                Some(carrier) => {
                    if carrier.set_relay(relay.clone()).await.is_err() {
                        return;
                    }
                }
                None => {
                    let config = osp_carrier::Config {
                        machine: machine.clone(),
                        relay: relay.clone(),
                        offers: Vec::new(),
                        limits: limits.clone(),
                        dial: dial.clone(),
                    };
                    match Carrier::spawn(endpoint.clone(), config) {
                        Ok(carrier) => {
                            let _ = cell.set(carrier);
                            info!("carrying for other nodes, from the middle of a path");
                        }
                        Err(e) => {
                            warn!("couldn't start carrying: {e:#}");
                            return;
                        }
                    }
                }
            }
            named = Some(relay);
            renewed = None;
        }
        if let Some(carrier) = cell.get()
            && renewed.is_none_or(|at| at.elapsed() >= RENEW)
        {
            match carrier.descriptor().await {
                Ok(descriptor) => {
                    published.set(&descriptor);
                    renewed = Some(tokio::time::Instant::now());
                }
                Err(e) => {
                    debug!("the carrier stopped: {e:#}");
                    return;
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(RELAY_CHECK) => {}
            _ = endpoint.closed() => return,
        }
    }
}
