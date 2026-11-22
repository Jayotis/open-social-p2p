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
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use tokio::sync::watch;

use anyhow::{Context, Result};
use iroh::{Endpoint, RelayMode, SecretKey, TransportAddr, endpoint::presets};
use n0_future::task::AbortOnDropHandle;
use osp_carrier::{Carrier, Dial, Limits, Stats};
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
/// Carrying learns the upload (`Learned`): a figure whose share held
/// packets back for a while, with no link backed up, is raised by this
/// much, and one with a link backed up is lowered by as much.
const LEARN_STEP: f64 = 1.25;
/// A spell shorter than this isn't judged.
const LEARN_OVER: Duration = Duration::from_secs(4);
/// The share was the limit in a spell when packets waited for it at least
/// this much of the time.
const LEARN_HELD: f64 = 0.5;
/// No higher than this is learned, in Mbit/s.
const LEARN_MAX_MBPS: f64 = 1000.0;

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
    /// The month's budget for carrying and the plan's data cap, in GB.
    pub monthly_gb: Option<u64>,
    pub data_cap_gb: u64,
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
            monthly_gb: c.monthly_gb,
            data_cap_gb: config.serving.data_cap_gb,
        }
    }

    /// The bytes carrying may use in a UTC month, both ways: the owner's
    /// budget, else half the plan's data cap. None for no limit.
    pub fn monthly_budget(&self) -> Option<u64> {
        let gb = match self.monthly_gb {
            Some(gb) => gb,
            None => self.data_cap_gb / 2,
        };
        (gb > 0).then(|| gb.saturating_mul(1_000_000_000))
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

/// What `keep_carrying` reads the owner's settings from, and writes what it
/// carried to.
pub(crate) struct Owner {
    pub controls: watch::Receiver<Controls>,
    pub measured: Measured,
    pub learned: Arc<Mutex<Learned>>,
    pub db: Arc<Db>,
}

/// The upload as carrying showed it, for a node with no figure from its
/// owner. The share starts from the assumed upload. A spell in which
/// packets waited for the share most of the time (`LEARN_HELD`), and no
/// link the carrier sends on backed up, asked for more than the share
/// allowed while the path took all it was given: the figure goes up by
/// `LEARN_STEP`. A link that backs up brings it down by as much, to the
/// assumed upload at the least. So 10 Mbit/s is where it starts, not a
/// ceiling. It isn't kept over a restart, like the serving measurement.
///
/// What was carried isn't the test: in stage D's second run the ends'
/// QUIC, slowed by packets dropped from a full queue, averaged two-thirds of
/// the share, so a rule asking for most of it carried never rose again.
#[derive(Debug, Default)]
pub(crate) struct Learned {
    mbps: Option<f64>,
    /// When the spell being judged began, and the carrier's counts then.
    since: Option<(tokio::time::Instant, Stats)>,
}

impl Learned {
    /// The upload learned, in Mbit/s, once there is one.
    pub fn mbps(&self) -> Option<f64> {
        self.mbps
    }

    /// Judges the spell up to `now`, when the carrier's counts are `stats`,
    /// the share was `rate` bytes a second, and the upload it was taken of
    /// `upload` Mbit/s. Only a figure carrying may change goes in: not one
    /// the owner set or capped.
    pub fn judge(
        &mut self,
        stats: &Stats,
        rate: Option<u64>,
        upload: (f64, UploadFrom),
        now: tokio::time::Instant,
    ) {
        let Some((since, before)) = self.since.take() else {
            self.since = Some((now, stats.clone()));
            return;
        };
        let took = now.duration_since(since);
        if took < LEARN_OVER {
            self.since = Some((since, before));
            return;
        }
        self.since = Some((now, stats.clone()));
        // A share at all: a seed carrier with none has nothing to learn.
        let (Some(_), (mbps, UploadFrom::Measured | UploadFrom::Assumed)) = (rate, upload) else {
            return;
        };
        if stats.link_full > before.link_full {
            if let Some(m) = self.mbps {
                let lower = (m / LEARN_STEP).max(ASSUMED_UPLOAD_MBPS);
                debug!(mbps = %format!("{lower:.1}"), "a link backed up: the upload learned from carrying goes down");
                self.mbps = Some(lower);
            }
            return;
        }
        let waited = Duration::from_millis(stats.held_ms.saturating_sub(before.held_ms));
        let wanted_more = waited.as_secs_f64() >= LEARN_HELD * took.as_secs_f64();
        let higher = (mbps * LEARN_STEP).min(LEARN_MAX_MBPS);
        if wanted_more && self.mbps.is_none_or(|m| m < higher) {
            debug!(mbps = %format!("{higher:.1}"), waited_ms = waited.as_millis() as u64, "packets waited for the share with no link backed up: the upload learned from carrying goes up");
            self.mbps = Some(higher);
        }
    }
}

/// The faster of what serving measured and what carrying learned.
fn faster(measured: Option<f64>, learned: Option<f64>) -> Option<f64> {
    match (measured, learned) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// The bytes a carrier moved, as they count against a data plan: each was
/// received, then sent on.
fn both_ways(carried: u64) -> u64 {
    carried.saturating_mul(2)
}

/// This month's bytes carried, both ways, and the day it is.
fn carried_this_month(db: &Db) -> Result<(u64, i64)> {
    let today = crate::db::day_of(crate::hops::unix_now() as i64 * 1000);
    let first = crate::db::carriers::month_start(today);
    Ok((db.carried_between(first, today)?, today))
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
    learned: Arc<Mutex<Learned>>,
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
        let learned: Arc<Mutex<Learned>> = Arc::default();
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
            Owner {
                controls: watching,
                measured: measured.clone(),
                learned: learned.clone(),
                db: db.clone(),
            },
            dial,
            crate::work_offers::answerer(db.clone(), machine.acquaintances.clone()),
            carrier.clone(),
        ));
        Ok(Some(Self {
            endpoint,
            carrier,
            controls,
            measured,
            learned,
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

    /// The upload measured, by serving or by carrying, once known.
    pub fn measured(&self) -> Option<f64> {
        let learned = self
            .learned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mbps();
        faster(self.measured.get().and_then(|f| f()), learned)
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
    owner: Owner,
    dial: Dial,
    others: osp_carrier::Others,
    cell: Arc<OnceLock<Carrier>>,
) {
    let mut named: Option<String> = None;
    let mut renewed: Option<tokio::time::Instant> = None;
    // What the carrier was last given: its limits and whether it's stopped.
    let mut applied: Option<(Limits, bool)> = None;
    let Owner {
        mut controls,
        measured,
        learned,
        db,
    } = owner;
    let lock = || learned.lock().unwrap_or_else(|e| e.into_inner());
    let measure = || faster(measured.get().and_then(|f| f()), lock().mbps());
    // What the carrier had carried when last written down.
    let mut counted = 0u64;
    let mut over_budget = false;
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
            // What it carried since last time goes towards the month.
            match carrier.stats().await {
                Ok(stats) => {
                    let measured = measure();
                    lock().judge(
                        &stats,
                        limits.bytes_per_sec,
                        now.upload(measured),
                        tokio::time::Instant::now(),
                    );
                    let new = stats.bytes.saturating_sub(counted);
                    counted = stats.bytes;
                    let month = (new > 0)
                        .then(|| {
                            let day = crate::db::day_of(crate::hops::unix_now() as i64 * 1000);
                            db.add_carried(day, both_ways(new))
                        })
                        .transpose()
                        .and_then(|_| carried_this_month(&db));
                    match month {
                        Ok((used, _)) => {
                            let over = now.monthly_budget().is_some_and(|b| used >= b);
                            if over && !over_budget {
                                info!(used, "carrying stopped: this month's budget is used up");
                            }
                            over_budget = over;
                        }
                        Err(e) => warn!("couldn't count what was carried: {e:#}"),
                    }
                }
                Err(_) => return,
            }
            let want = (limits, now.stopped() || over_budget);
            if applied.as_ref() != Some(&want) {
                let (limits, stopped) = want.clone();
                let changed_stop = applied.as_ref().is_none_or(|(_, s)| *s != stopped);
                if carrier.set_limits(limits).await.is_err()
                    || (changed_stop && carrier.set_paused(stopped).await.is_err())
                {
                    return;
                }
                if changed_stop {
                    match (stopped, now.metered, now.paused) {
                        (true, true, _) => info!("carrying stopped: the connection is metered"),
                        (true, _, true) => info!("carrying paused"),
                        (true, _, _) => {}
                        (false, _, _) => info!("carrying again"),
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
        let (month_bytes, _) = carried_this_month(&self.db)?;
        let month_budget = controls.monthly_budget();
        let over_budget = month_budget.is_some_and(|b| month_bytes >= b);
        Ok(osp_proto::rpc::CarrierStatus {
            carrying: stats.is_some() && !controls.stopped() && !over_budget,
            month_bytes,
            month_budget,
            over_budget,
            data_cap_gb: controls.data_cap_gb,
            monthly_gb: controls.monthly_gb,
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
        (monthly_gb, data_cap_gb): (Option<u64>, Option<u64>),
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
        if monthly_gb.is_some() {
            config.carrier.monthly_gb = monthly_gb;
        }
        if let Some(gb) = data_cap_gb {
            config.serving.data_cap_gb = gb;
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
            monthly_gb: None,
            data_cap_gb: 0,
        }
    }

    #[test]
    fn the_months_budget_is_half_the_plans_cap_unless_the_owner_sets_one() {
        assert_eq!(home().monthly_budget(), None);
        let plan = Controls {
            data_cap_gb: 400,
            ..home()
        };
        assert_eq!(plan.monthly_budget(), Some(200_000_000_000));
        let own = Controls {
            monthly_gb: Some(50),
            ..plan.clone()
        };
        assert_eq!(own.monthly_budget(), Some(50_000_000_000));
        let none = Controls {
            monthly_gb: Some(0),
            ..plan
        };
        assert_eq!(none.monthly_budget(), None);
        assert_eq!(both_ways(10), 20);
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

    /// One spell of `LEARN_OVER` carried at `share` of the rate the
    /// current figure gives, with packets waiting for it `waited` of the
    /// time.
    fn spell(
        l: &mut Learned,
        c: &Controls,
        at: &mut tokio::time::Instant,
        share: f64,
        waited: f64,
    ) {
        let limits = c.limits(l.mbps());
        let rate = limits.bytes_per_sec.unwrap();
        let before = l
            .since
            .as_ref()
            .map_or_else(Stats::default, |(_, s)| s.clone());
        *at += LEARN_OVER;
        let secs = LEARN_OVER.as_secs_f64();
        let stats = Stats {
            bytes: before.bytes + (rate as f64 * share * secs) as u64,
            held: before.held + u64::from(waited > 0.0),
            held_ms: before.held_ms + (waited * secs * 1000.0) as u64,
            ..before
        };
        l.judge(&stats, Some(rate), c.upload(l.mbps()), *at);
    }

    #[test]
    fn a_share_that_held_packets_back_raises_the_upload_from_the_assumed() {
        let c = home();
        let mut l = Learned::default();
        let mut at = tokio::time::Instant::now();
        l.judge(&Stats::default(), Some(625_000), c.upload(None), at);
        assert_eq!(l.mbps(), None, "the first look only starts the clock");
        spell(&mut l, &c, &mut at, 1.0, 1.0);
        assert_eq!(l.mbps(), Some(12.5));
        assert_eq!(c.upload(l.mbps()), (12.5, UploadFrom::Measured));
        // From 10 Mbit/s to past 100 in a minute of the share holding.
        for _ in 0..14 {
            spell(&mut l, &c, &mut at, 1.0, 1.0);
        }
        assert!(l.mbps().unwrap() > 100.0, "{:?}", l.mbps());
        for _ in 0..40 {
            spell(&mut l, &c, &mut at, 1.0, 1.0);
        }
        assert_eq!(l.mbps(), Some(LEARN_MAX_MBPS));
    }

    /// Stage D's second run: what was carried came to two-thirds of the
    /// share, the ends' QUIC backing off after packets dropped from a full
    /// queue, but packets waited for the share most of the time.
    #[test]
    fn it_rises_with_packets_waiting_though_less_than_the_share_was_carried() {
        let c = home();
        let mut l = Learned::default();
        let mut at = tokio::time::Instant::now();
        l.judge(&Stats::default(), Some(625_000), c.upload(None), at);
        for _ in 0..4 {
            spell(&mut l, &c, &mut at, 0.66, 0.7);
        }
        assert!(l.mbps().unwrap() > 24.0, "{:?}", l.mbps());
    }

    #[test]
    fn a_share_that_held_little_or_nothing_back_teaches_nothing() {
        let c = home();
        let mut l = Learned::default();
        let mut at = tokio::time::Instant::now();
        l.judge(&Stats::default(), Some(625_000), c.upload(None), at);
        spell(&mut l, &c, &mut at, 1.0, 0.3);
        assert_eq!(l.mbps(), None, "packets waited, but under half the time");
        spell(&mut l, &c, &mut at, 1.0, 0.0);
        assert_eq!(l.mbps(), None, "the whole share, but nothing waited for it");
        // A spell too short isn't judged, and the clock keeps running.
        let stats = Stats {
            bytes: 10_000_000,
            held: 99,
            held_ms: 60_000,
            ..Stats::default()
        };
        l.judge(&stats, Some(625_000), c.upload(None), at + LEARN_OVER / 2);
        assert_eq!(l.mbps(), None);
    }

    #[test]
    fn a_backed_up_link_lowers_it_but_not_under_the_assumed() {
        let c = home();
        let mut l = Learned::default();
        let mut at = tokio::time::Instant::now();
        l.judge(&Stats::default(), Some(625_000), c.upload(None), at);
        for _ in 0..3 {
            spell(&mut l, &c, &mut at, 1.0, 1.0);
        }
        let high = l.mbps().unwrap();
        let (_, before) = l.since.clone().unwrap();
        at += LEARN_OVER;
        let mut stats = before.clone();
        stats.link_full += 1;
        stats.held += 1;
        stats.held_ms += LEARN_OVER.as_millis() as u64;
        stats.bytes += 10_000_000;
        l.judge(&stats, Some(1), c.upload(l.mbps()), at);
        assert_eq!(l.mbps(), Some(high / LEARN_STEP), "backed up: down, not up");
        for _ in 0..10 {
            at += LEARN_OVER;
            stats.link_full += 1;
            l.judge(&stats, Some(1), c.upload(l.mbps()), at);
        }
        assert_eq!(l.mbps(), Some(ASSUMED_UPLOAD_MBPS));
    }

    #[test]
    fn an_upload_the_owner_set_or_capped_isnt_learned_over() {
        for c in [
            Controls {
                upload_mbps: 8,
                ..home()
            },
            Controls {
                max_upload_mbps: 8,
                ..home()
            },
        ] {
            let mut l = Learned::default();
            let mut at = tokio::time::Instant::now();
            l.judge(&Stats::default(), Some(500_000), c.upload(None), at);
            spell(&mut l, &c, &mut at, 1.0, 1.0);
            assert_eq!(l.mbps(), None);
        }
    }

    #[test]
    fn the_faster_of_serving_and_carrying() {
        assert_eq!(faster(None, None), None);
        assert_eq!(faster(Some(30.0), None), Some(30.0));
        assert_eq!(faster(None, Some(12.5)), Some(12.5));
        assert_eq!(faster(Some(30.0), Some(12.5)), Some(30.0));
    }
}
