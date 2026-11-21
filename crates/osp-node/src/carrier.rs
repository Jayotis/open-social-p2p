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

use tokio::sync::watch;

use anyhow::{Context, Result};
use iroh::{Endpoint, RelayMode, SecretKey, TransportAddr, endpoint::presets};
use n0_future::task::AbortOnDropHandle;
use osp_carrier::{Carrier, Dial, Limits};
use tracing::{debug, info, warn};

use crate::{
    config::{CarrierConfig, Config, RelayChoice},
    db::Db,
    home::Home,
    keys,
    machine::{MachineSide, Published},
    node::own_relay_map,
    relays::RelayPolicy,
};

/// The upload a node with no figure for it is taken to have, in Mbit/s: a
/// slow home's, so carrying starts small until the owner says.
pub(crate) const ASSUMED_UPLOAD_MBPS: f64 = 10.0;
/// A home node's upload share for carrying, from the sizing stage.
const HOME_SHARE: u8 = 50;

/// What the owner set for carrying. It changes while the node runs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Controls {
    pub seed: bool,
    pub paused: bool,
    pub metered: bool,
    pub jobs: Option<usize>,
    pub upload_share: Option<u8>,
    /// The connection's upload speed, and the cap on serving, in Mbit/s; 0
    /// for none.
    pub upload_mbps: u64,
    pub max_upload_mbps: u64,
}

impl Controls {
    pub fn of(config: &Config) -> Self {
        let c: &CarrierConfig = &config.carrier;
        Self {
            seed: c.seed,
            paused: c.paused,
            metered: c.metered,
            jobs: c.jobs,
            upload_share: c.upload_share,
            upload_mbps: config.serving.upload_mbps,
            max_upload_mbps: config.serving.max_upload_mbps,
        }
    }

    /// Whether it carries nothing for now: paused, or on a metered
    /// connection.
    pub fn stopped(&self) -> bool {
        self.paused || self.metered
    }

    /// The share of the upload carrying may use, in percent.
    pub fn share(&self) -> u8 {
        self.upload_share
            .unwrap_or(if self.seed { 100 } else { HOME_SHARE })
            .min(100)
    }

    /// The upload the share is taken of, in Mbit/s, and where that figure
    /// came from: the speed asked at setup, else the cap on serving, else
    /// the fastest measured, else a slow home's.
    pub fn upload(&self, measured: Option<f64>) -> (f64, UploadFrom) {
        if self.upload_mbps > 0 {
            (self.upload_mbps as f64, UploadFrom::Set)
        } else if self.max_upload_mbps > 0 {
            (self.max_upload_mbps as f64, UploadFrom::Cap)
        } else if let Some(m) = measured.filter(|m| *m > 0.0) {
            (m, UploadFrom::Measured)
        } else {
            (ASSUMED_UPLOAD_MBPS, UploadFrom::Assumed)
        }
    }

    /// The carrier's limits. A seed carrier with no share set carries with
    /// its whole upload, uncapped.
    pub fn limits(&self, measured: Option<f64>) -> Limits {
        let base = match self.seed {
            true => Limits::seed(),
            false => Limits::home(),
        };
        let share = self.share();
        let bytes_per_sec = match (self.seed && self.upload_share.is_none(), share) {
            (true, _) => None,
            (false, share) => {
                let (mbps, _) = self.upload(measured);
                Some((mbps * 1e6 / 8.0 * f64::from(share) / 100.0) as u64)
            }
        };
        Limits {
            jobs: self.jobs.unwrap_or(base.jobs),
            bytes_per_sec,
            ..base
        }
    }
}

/// Where the upload figure carrying goes by came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UploadFrom {
    /// The owner's, asked at setup.
    Set,
    /// The cap on serving media.
    Cap,
    /// The fastest the node was measured serving media.
    Measured,
    /// No figure: a slow home's is taken.
    Assumed,
}

/// The fastest the node was measured serving media, once the store is open.
pub(crate) type Measured = Arc<OnceLock<Arc<dyn Fn() -> Option<f64> + Send + Sync>>>;

/// How often the carrier endpoint's relay is looked at.
const RELAY_CHECK: Duration = Duration::from_secs(5);
/// How often the descriptor in the machine record is renewed. A circuit key
/// lasts a day, so this only needs to catch its replacement soon.
const RENEW: Duration = Duration::from_secs(60);

pub(crate) struct CarrierSide {
    /// The endpoint it carries on.
    pub endpoint: Endpoint,
    /// The carrier, once the endpoint has a relay to name. Status reads it
    /// from part 5 on.
    carrier: Arc<OnceLock<Carrier>>,
    controls: watch::Sender<Controls>,
    /// The fastest measured serving, read for the upload share.
    pub measured: Measured,
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
        db: &Arc<Db>,
    ) -> Result<Option<Self>> {
        if !config.carrier.enabled {
            return Ok(None);
        }
        let Some(relay_mode) = relay_mode(config)? else {
            info!("not carrying for other nodes: carrying needs a relay, and relays are off");
            return Ok(None);
        };
        let key = kept_key(&home.carrier_key())?;
        let endpoint = relayed_endpoint(
            config,
            relay_mode,
            relays,
            key,
            vec![
                osp_carrier::ALPN.to_vec(),
                osp_proto::social::WORK_OFFERS_ALPN.to_vec(),
            ],
        )
        .await
        .context("starting the carrier endpoint")?;
        let (controls, watching) = watch::channel(Controls::of(config));
        let measured: Measured = Arc::default();
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
            (watching, measured.clone()),
            dial,
            crate::work_offers::answerer(db.clone(), machine.acquaintances.clone()),
            carrier.clone(),
        ));
        Ok(Some(Self {
            endpoint,
            carrier,
            controls,
            measured,
            _task: AbortOnDropHandle::new(task),
        }))
    }

    /// The carrier, once it's running.
    pub fn carrier(&self) -> Option<&Carrier> {
        self.carrier.get()
    }

    /// What the owner set, as it applies now.
    pub fn controls(&self) -> Controls {
        self.controls.borrow().clone()
    }

    /// Applies what the owner set from now on.
    pub fn set_controls(&self, controls: Controls) {
        self.controls.send_replace(controls);
    }

    /// The fastest measured serving, once known.
    pub fn measured(&self) -> Option<f64> {
        self.measured.get().and_then(|f| f())
    }
}

/// The relays a link between carriers goes through, or None with relays
/// off: neighbours reach each other through a relay first.
pub(crate) fn relay_mode(config: &Config) -> Result<Option<RelayMode>> {
    let net = &config.network;
    Ok(match &net.relay {
        RelayChoice::Disabled => None,
        RelayChoice::Default => Some(RelayMode::Default),
        RelayChoice::Custom(_) => Some(RelayMode::Custom(own_relay_map(net)?)),
    })
}

/// The key at `path`, made the first time.
pub(crate) fn kept_key(path: &std::path::Path) -> Result<SecretKey> {
    match path.exists() {
        true => keys::load(path),
        false => keys::create(path),
    }
}

/// An endpoint for links between carriers, or between a host and its hop: a
/// relay, no address lookup and nothing published, so the other end reaches
/// it only by an ID it was given. A relay-only node binds no IP sockets here
/// either.
pub(crate) async fn relayed_endpoint(
    config: &Config,
    relay_mode: RelayMode,
    relays: &RelayPolicy,
    key: SecretKey,
    alpns: Vec<Vec<u8>>,
) -> Result<Endpoint> {
    let net = &config.network;
    let mut builder = Endpoint::builder(presets::Minimal)
        .relay_mode(relay_mode)
        .alpns(alpns)
        .secret_key(key);
    if net.relay_only {
        builder = builder.clear_ip_transports();
    } else if let Some(bind) = net.bind {
        // On the main endpoint's address, on a port of its own.
        builder = builder.bind_addr(SocketAddr::new(bind.ip(), 0))?;
    }
    let endpoint = builder.bind().await?;
    if let Some(fallback) = relays.fallback() {
        fallback.follow(&endpoint);
    }
    Ok(endpoint)
}

/// Starts the carrier once its endpoint has a relay, tells it when the relay
/// moves, and keeps its descriptor in the machine record fresh. Stops when
/// the endpoint closes.
async fn keep_carrying(
    endpoint: Endpoint,
    machine: SecretKey,
    published: Published,
    (mut controls, measured): (watch::Receiver<Controls>, Measured),
    dial: Dial,
    others: osp_carrier::Others,
    cell: Arc<OnceLock<Carrier>>,
) {
    let mut named: Option<String> = None;
    let mut renewed: Option<tokio::time::Instant> = None;
    // What the carrier was last given: its limits and whether it's stopped.
    let mut applied: Option<(Limits, bool)> = None;
    let measure = || measured.get().and_then(|f| f());
    loop {
        let now = controls.borrow_and_update().clone();
        let limits = now.limits(measure());
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
                        others: Some(others.clone()),
                    };
                    match Carrier::spawn(endpoint.clone(), config) {
                        Ok(carrier) => {
                            let _ = cell.set(carrier);
                            applied = Some((limits.clone(), false));
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
        if let Some(carrier) = cell.get() {
            let want = (limits, now.stopped());
            if applied.as_ref() != Some(&want) {
                let (limits, stopped) = want.clone();
                let changed_stop = applied.as_ref().is_none_or(|(_, s)| *s != stopped);
                if carrier.set_limits(limits).await.is_err()
                    || (changed_stop && carrier.set_paused(stopped).await.is_err())
                {
                    return;
                }
                if changed_stop {
                    match (stopped, now.metered) {
                        (true, true) => info!("carrying stopped: the connection is metered"),
                        (true, false) => info!("carrying paused"),
                        (false, _) => info!("carrying again"),
                    }
                }
                applied = Some(want);
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(RELAY_CHECK) => {}
            changed = controls.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            _ = endpoint.closed() => return,
        }
    }
}

impl crate::node::Core {
    /// Carrying as it goes, and what the owner set for it.
    pub(crate) async fn carrier_status(&self) -> Result<osp_proto::rpc::CarrierStatus> {
        let side = self.carrier.as_ref();
        let controls = match side {
            Some(side) => side.controls(),
            // Nothing runs to hold them: as last saved.
            None => Controls::of(&self.saved_config()?),
        };
        let measured = side.and_then(CarrierSide::measured);
        let (upload_mbps, from) = controls.upload(measured);
        let limits = controls.limits(measured);
        let stats = match side.and_then(CarrierSide::carrier) {
            Some(carrier) => Some(carrier.stats().await?),
            None => None,
        };
        let off = match (side, &stats) {
            (Some(_), Some(_)) => None,
            (Some(_), None) => Some("it's waiting for a relay to be reached through".to_string()),
            (None, _) => Some(self.why_not_carrying().to_string()),
        };
        let now = crate::hops::unix_now() as i64;
        let (circuits, bytes, busy) = stats
            .as_ref()
            .map_or((0, 0, 0), |s| (s.circuits, s.bytes, s.busy));
        Ok(osp_proto::rpc::CarrierStatus {
            carrying: stats.is_some() && !controls.stopped(),
            off,
            paused: controls.paused,
            metered: controls.metered,
            seed: controls.seed,
            jobs: limits.jobs,
            upload_share: controls.share(),
            upload_mbps,
            upload_from: match from {
                UploadFrom::Set => "set",
                UploadFrom::Cap => "cap",
                UploadFrom::Measured => "measured",
                UploadFrom::Assumed => "assumed",
            }
            .to_string(),
            bytes_per_sec: limits.bytes_per_sec,
            circuits,
            bytes,
            busy,
            hops: *self.hops.borrow(),
            seeds: self.config.carrier.known.len(),
            standing: self.db.carriers_at_standing(now)?.len(),
            probation: self.db.probationers()?.len(),
        })
    }

    fn why_not_carrying(&self) -> &'static str {
        if !self.config.carrier.enabled {
            "carrying is off in config.toml"
        } else if self.machine.is_none() {
            "it has no machine name: set enabled = true under [tor] and under [machine]"
        } else if self.config.network.relay == RelayChoice::Disabled {
            "carrying needs a relay, and relays are off"
        } else {
            "it couldn't start: see the node's log"
        }
    }

    /// config.toml as saved, which the owner's changes go into.
    fn saved_config(&self) -> Result<Config> {
        let path = self.home.config();
        match path.exists() {
            true => Config::load(&path),
            false => Ok(self.config.clone()),
        }
    }

    /// Changes what carrying may use. Each `None` stays as it is.
    pub(crate) fn set_carrying(
        &self,
        paused: Option<bool>,
        metered: Option<bool>,
        jobs: Option<usize>,
        upload_share: Option<u8>,
        upload_mbps: Option<u64>,
    ) -> Result<()> {
        if let Some(jobs) = jobs {
            anyhow::ensure!(
                (1..=MOST_JOBS).contains(&jobs),
                "jobs at once go from 1 to {MOST_JOBS}; to carry nothing, pause instead"
            );
        }
        if let Some(share) = upload_share {
            anyhow::ensure!(
                (1..=100).contains(&share),
                "the upload share goes from 1 to 100 percent; to carry nothing, pause instead"
            );
        }
        let mut config = self.saved_config()?;
        let c = &mut config.carrier;
        if let Some(paused) = paused {
            c.paused = paused;
        }
        if let Some(metered) = metered {
            c.metered = metered;
        }
        if jobs.is_some() {
            c.jobs = jobs;
        }
        if upload_share.is_some() {
            c.upload_share = upload_share;
        }
        if let Some(mbps) = upload_mbps {
            config.serving.upload_mbps = mbps;
        }
        config.save(&self.home.config())?;
        if let Some(side) = &self.carrier {
            // The seed setting isn't the owner's to change while it runs.
            side.set_controls(Controls {
                seed: side.controls().seed,
                ..Controls::of(&config)
            });
        }
        Ok(())
    }

    /// The owner's answer on the hops this node picks seeing its address.
    /// Applied at once: with no, the host's link to its hop goes, and its
    /// next fetches go through relays.
    pub(crate) fn set_hops(&self, yes: bool) -> Result<()> {
        let mut config = self.saved_config()?;
        config.carrier.hops = Some(yes);
        config.save(&self.home.config())?;
        self.hops.send_replace(Some(yes));
        info!(
            yes,
            "the owner answered whether the hops it picks may see its address"
        );
        Ok(())
    }
}

/// The most circuits an owner may set carrying at once.
const MOST_JOBS: usize = 100;

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> Controls {
        Controls {
            seed: false,
            paused: false,
            metered: false,
            jobs: None,
            upload_share: None,
            upload_mbps: 0,
            max_upload_mbps: 0,
        }
    }

    #[test]
    fn the_share_is_of_the_upload_asked_then_the_cap_then_the_measured() {
        let c = home();
        assert_eq!(c.upload(None), (10.0, UploadFrom::Assumed));
        assert_eq!(c.upload(Some(30.0)), (30.0, UploadFrom::Measured));
        let c = Controls {
            max_upload_mbps: 8,
            ..home()
        };
        assert_eq!(c.upload(Some(30.0)), (8.0, UploadFrom::Cap));
        let c = Controls {
            upload_mbps: 40,
            ..c
        };
        assert_eq!(c.upload(Some(30.0)), (40.0, UploadFrom::Set));
        // Half of 40 Mbit/s: 2.5 MB a second.
        assert_eq!(c.limits(None).bytes_per_sec, Some(2_500_000));
        assert_eq!(c.limits(None).jobs, 2);
    }

    #[test]
    fn a_seed_carrier_carries_with_its_whole_upload_unless_its_owner_says() {
        let c = Controls {
            seed: true,
            ..home()
        };
        assert_eq!(c.share(), 100);
        assert_eq!(c.limits(None).bytes_per_sec, None);
        assert_eq!(c.limits(None).jobs, 40);
        let c = Controls {
            upload_share: Some(50),
            upload_mbps: 200,
            jobs: Some(10),
            ..c
        };
        assert_eq!(c.limits(None).bytes_per_sec, Some(12_500_000));
        assert_eq!(c.limits(None).jobs, 10);
    }

    #[test]
    fn paused_or_metered_carries_nothing() {
        assert!(!home().stopped());
        assert!(
            Controls {
                paused: true,
                ..home()
            }
            .stopped()
        );
        assert!(
            Controls {
                metered: true,
                ..home()
            }
            .stopped()
        );
    }
}
