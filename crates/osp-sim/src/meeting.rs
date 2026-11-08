//! Machines meeting machines, for ADR 0015's step 3 and ADR 0016's stage A:
//! honest nodes that know only a directory, with Tor-like delays, left alone.
//! The directory goes off part way, and then one operator's names arrive,
//! greeting honest machines and passing on one another and fakes. The times
//! among machines run faster than real ones (`MeetingTimes::faster`), and Tor's
//! delays don't, so a conversation takes as long as over Tor.
//!
//! Newcomers, honest nodes told only of the directory, may arrive later, to
//! see whether the others meet them, keep them and pass them on.
//!
//! Each node's view comes from `Node::acquaintances`, the record that never
//! leaves a real node. Circuits are an estimate (`SimTor` in `osp-node`).

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_node::{
    Config, KnownDirectory, MeetingTimes, Node, SimSettings, Stage,
    simnet::{self, Link, SimNet, TorDelay},
};
use osp_proto::{
    EndpointId, MACHINE_ALPN,
    framing::{read_frame_max, write_frame},
    machine::{
        IntroduceRequest, MAX_KNOWN, MAX_REQUEST_LEN, MachineRecord, MachineRequest,
        MachineResponse,
    },
};
use serde::Serialize;
use tracing::info;

/// Nodes started at once.
const STARTING_AT_ONCE: usize = 16;
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const HOUR: Duration = Duration::from_secs(60 * 60);
/// Pass marks for newcomers: the established honest machines each meets
/// within so many simulated hours, and the established nodes that meet each.
const NEWCOMER_MEETS: usize = 3;
const NEWCOMER_WITHIN_HOURS: f64 = 6.0;
const NEWCOMER_MET_BY: usize = 10;
/// The places of those who greet: 32, and 8 taken in turn (`osp-node`'s
/// `meet`). Stage A2's mark holds the operator's names not yet known within
/// these (ADR 0016).
const GREETERS_PLACES: usize = 40;
/// Honest machines each hostile name introduces another of its names to, in a
/// simulated hour, of those that met it.
const HOSTILE_INTRODUCES_PER_HOUR: usize = 4;

#[derive(Debug, Clone, Serialize)]
pub struct MeetScenario {
    pub dir: PathBuf,
    pub honest: usize,
    /// One operator's names.
    pub hostile: usize,
    /// Honest machines each hostile name greets when it arrives. The operator
    /// read the directory's map before it went off.
    pub hostile_greets: usize,
    /// Times among machines run this many times as fast as real ones.
    pub speed: f64,
    /// How long, in the simulation's hours, a machine answers before it's
    /// known. A real node waits a month.
    pub known_after_hours: f64,
    /// In real time, from the start.
    pub directory_off_after: Duration,
    pub hostile_after: Duration,
    /// Whether the operator's names ask honest machines to introduce them, as
    /// often as those pass on for one machine, and introduce one another to
    /// honest machines that met them (ADR 0016's stage A3).
    pub hostile_introduces: bool,
    /// How long, in the simulation's hours, before a machine's introductions
    /// count. A real node waits 3 days.
    pub introducer_after_hours: f64,
    /// Honest nodes arriving later, told of the directory only.
    pub newcomers: usize,
    pub newcomers_after: Duration,
    /// The median of established nodes that met each newcomer in an earlier
    /// run, for stage A3's mark: newcomers met by more.
    pub compare_met_by: Option<usize>,
    pub run_for: Duration,
    pub sample: Duration,
    pub seed: u64,
    pub label: String,
}

/// What one honest node knows at a moment.
#[derive(Debug, Clone, Default, Serialize)]
pub struct View {
    /// Honest machines met (or known), and those of them known.
    pub honest_met: usize,
    pub honest_known: usize,
    /// Honest machines only heard of.
    pub honest_heard: usize,
    /// Of the honest machines met, those whose record came from another
    /// honest machine, from the directory's map, or that greeted it.
    pub met_through_machines: usize,
    pub met_through_directory: usize,
    pub met_greeted: usize,
    /// The operator's names held, at any stage; those of them known, whose
    /// records count as sources of their own; and those in places kept
    /// longest.
    pub hostile: usize,
    pub hostile_known: usize,
    pub hostile_kept_longest: usize,
    /// The operator's names counting against those who greet, and those of
    /// them held through an introduction.
    pub hostile_greeters_share: usize,
    pub hostile_introduced: usize,
    /// Fakes held: records of machines that never existed.
    pub fakes: usize,
    /// Newcomers held, at any stage, and met; and those held through an
    /// introduction.
    pub newcomers_held: usize,
    pub newcomers_met: usize,
    pub newcomers_introduced: usize,
    /// Machines held, of any kind.
    pub held: usize,
    pub circuits: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub secs: f64,
    pub views: Vec<View>,
    /// The newcomers' own views, once they've arrived.
    pub newcomers: Vec<View>,
}

/// One newcomer, at the end.
#[derive(Debug, Clone, Serialize)]
pub struct Newcomer {
    /// From its arrival to the first sample in which it had met
    /// `NEWCOMER_MEETS` established honest machines.
    pub met_three_after_secs: Option<f64>,
    pub honest_met: usize,
    /// Established nodes holding it, at any stage; those that met it; and those
    /// that met it through another machine's record.
    pub held_by: usize,
    pub met_by: usize,
    pub passed_on: usize,
    /// Established nodes holding it through an introduction, and those of
    /// them that met it.
    pub introduced_by: usize,
    pub met_through_introduction: usize,
    /// Its requests to be introduced, and those passed on, by the word of the
    /// machine asked.
    pub asked: u64,
    pub asked_passed_on: u64,
}

/// Introductions over the run, from each side.
#[derive(Debug, Clone, Default, Serialize)]
pub struct IntroductionTotals {
    /// Requests the established nodes passed on, and refused as over their
    /// limits; and for each, how many it refused.
    pub passed_on: u64,
    pub refused_at_limit: u64,
    pub refused_by_each: Vec<u64>,
    /// Introductions the established nodes accepted, and declined, and those
    /// declined by why (`IntroductionCounts`).
    pub accepted: u64,
    pub declined: u64,
    pub declined_untrusted: u64,
    pub declined_greeter: u64,
    pub declined_at_limit: u64,
    pub declined_held: u64,
    pub declined_no_place: u64,
    /// Requests the established nodes made to be introduced, and those passed
    /// on, by the word of the machine asked.
    pub asked: u64,
    pub asked_passed_on: u64,
    /// Requests the operator's names made, those passed on, and the
    /// introductions they passed on themselves.
    pub hostile_asked: u64,
    pub hostile_asked_passed_on: u64,
    pub hostile_introduced: u64,
}

#[derive(Debug, Serialize)]
pub struct MeetReport {
    pub scenario: MeetScenario,
    pub snapshots: Vec<Snapshot>,
    /// Honest machines each node had met when the directory went off, and
    /// those met after.
    pub met_at_directory_off: Vec<usize>,
    pub met_after_directory_off: Vec<usize>,
    /// Honest machines each node knew when the operator arrived, and how many
    /// of those it no longer held at the end.
    pub known_at_hostile: Vec<usize>,
    pub pushed_out: Vec<usize>,
    /// Circuits each node built in the simulation's last day, estimated.
    pub circuits_last_day: Vec<u64>,
    /// How many times the operator's names were asked `Known`.
    pub hostile_asked_known: u64,
    /// Machines each established node held when the newcomers arrived.
    pub held_at_newcomers: Vec<usize>,
    pub newcomers: Vec<Newcomer>,
    /// For each established machine, at the end: the other established nodes
    /// that met it, and that met it through another machine's record.
    pub established_met_by: Vec<usize>,
    pub established_passed_on: Vec<usize>,
    pub introductions: IntroductionTotals,
    pub wall_secs: f64,
}

/// Which machines are which.
#[derive(Default)]
struct Kinds {
    /// The established honest nodes.
    honest: HashSet<EndpointId>,
    hostile: HashSet<EndpointId>,
    newcomers: HashSet<EndpointId>,
}

/// One node's machines, sorted by kind, at a moment.
fn view(node: &Node, kinds: &Kinds) -> View {
    let mut v = View {
        circuits: node.machine_circuits().unwrap_or(0),
        ..View::default()
    };
    for a in node.acquaintances() {
        v.held += 1;
        if kinds.honest.contains(&a.machine) {
            match a.stage {
                Stage::Heard => v.honest_heard += 1,
                stage => {
                    v.honest_met += 1;
                    v.honest_known += (stage == Stage::Known) as usize;
                    match a.source {
                        Some(from)
                            if kinds.honest.contains(&from) || kinds.newcomers.contains(&from) =>
                        {
                            v.met_through_machines += 1
                        }
                        Some(_) => v.met_through_directory += 1,
                        None => v.met_greeted += 1,
                    }
                }
            }
        } else if kinds.newcomers.contains(&a.machine) {
            v.newcomers_held += 1;
            v.newcomers_met += (a.stage != Stage::Heard) as usize;
            v.newcomers_introduced += a.introduced as usize;
        } else if kinds.hostile.contains(&a.machine) {
            v.hostile += 1;
            v.hostile_known += (a.stage == Stage::Known) as usize;
            v.hostile_kept_longest += a.kept_longest as usize;
            v.hostile_greeters_share += a.greeters_share as usize;
            v.hostile_introduced += a.introduced as usize;
        } else {
            v.fakes += 1;
        }
    }
    v
}

/// The honest machines a node knows.
fn known_honest(node: &Node, honest: &HashSet<EndpointId>) -> HashSet<EndpointId> {
    node.acquaintances()
        .into_iter()
        .filter(|a| a.stage == Stage::Known && honest.contains(&a.machine))
        .map(|a| a.machine)
        .collect()
}

fn held(node: &Node) -> HashSet<EndpointId> {
    node.acquaintances()
        .into_iter()
        .map(|a| a.machine)
        .collect()
}

pub async fn run(s: &MeetScenario) -> Result<MeetReport> {
    let started = Instant::now();
    std::fs::create_dir_all(&s.dir)?;
    let net = SimNet::new(s.seed);
    let times = MeetingTimes {
        known_after: HOUR.mul_f64(s.known_after_hours).div_f64(s.speed),
        introducer_after: HOUR.mul_f64(s.introducer_after_hours).div_f64(s.speed),
        ..MeetingTimes::default().faster(s.speed)
    };
    let settings = || SimSettings {
        network: Some(net.link(Link::default())),
        machine_names: true,
        tor_delay: Some(TorDelay::default()),
        meeting: times,
        first_read_wait: Duration::from_secs(120),
        read_every: (HOUR * 6).div_f64(s.speed),
        // So the map isn't held back by the directory's hourly limit, which
        // isn't sped up.
        greets_back_per_hour: 100_000,
        ..SimSettings::default()
    };

    let mut config = Config::local_test("The directory");
    config.directory.serve = true;
    let directory = start(&s.dir.join("directory"), config, settings()).await?;
    let known = KnownDirectory {
        machine: directory
            .machine_id()
            .context("the directory has no machine name")?,
        node: directory.node_id(),
    };
    info!(honest = s.honest, "starting");
    let nodes = start_honest(s, 0..s.honest, &known, &settings).await?;
    let honest_list = machine_ids(&nodes)?;
    let operator = Operator::new(s.hostile, s.seed);
    let mut kinds = Kinds {
        honest: honest_list.iter().copied().collect(),
        hostile: operator.names().collect(),
        newcomers: HashSet::new(),
    };

    let begun = Instant::now();
    let mut snapshots = Vec::new();
    let mut met_at_off = None;
    let mut known_at_hostile: Option<Vec<HashSet<EndpointId>>> = None;
    let mut routers = Vec::new();
    let mut newcomers: Vec<Node> = Vec::new();
    let mut newcomers_arrived: Option<Duration> = None;
    let mut held_at_newcomers = Vec::new();
    let mut next_sample = Duration::ZERO;
    while begun.elapsed() < s.run_for {
        let now = begun.elapsed();
        if met_at_off.is_none() && now >= s.directory_off_after {
            for id in directory.endpoint_ids() {
                net.set_online(id, false);
            }
            let met: Vec<usize> = nodes.iter().map(|n| view(n, &kinds).honest_met).collect();
            info!(median_met = median(&met), "the directory goes off");
            met_at_off = Some(met);
        }
        if known_at_hostile.is_none() && now >= s.hostile_after {
            known_at_hostile = Some(
                nodes
                    .iter()
                    .map(|n| known_honest(n, &kinds.honest))
                    .collect(),
            );
            routers = operator.arrive(&net, &honest_list, s).await?;
            info!(names = s.hostile, "the operator arrives");
        }
        if newcomers_arrived.is_none() && s.newcomers > 0 && now >= s.newcomers_after {
            held_at_newcomers = nodes.iter().map(|n| view(n, &kinds).held).collect();
            newcomers =
                start_honest(s, s.honest..s.honest + s.newcomers, &known, &settings).await?;
            kinds.newcomers = machine_ids(&newcomers)?.into_iter().collect();
            newcomers_arrived = Some(begun.elapsed());
            info!(
                newcomers = s.newcomers,
                median_held = median(&held_at_newcomers),
                "the newcomers arrive"
            );
        }
        if now >= next_sample {
            let views: Vec<View> = nodes.iter().map(|n| view(n, &kinds)).collect();
            let newcomer_views: Vec<View> = newcomers.iter().map(|n| view(n, &kinds)).collect();
            let met: Vec<usize> = views.iter().map(|v| v.honest_met).collect();
            let fakes: Vec<usize> = views.iter().map(|v| v.hostile + v.fakes).collect();
            let newcomers_met: Vec<usize> = views.iter().map(|v| v.newcomers_met).collect();
            info!(
                mins = now.as_secs() / 60,
                median_met = median(&met),
                min_met = met.iter().min().copied().unwrap_or(0),
                median_hostile = median(&fakes),
                median_newcomers_met = median(&newcomers_met),
                "sample"
            );
            snapshots.push(Snapshot {
                secs: begun.elapsed().as_secs_f64(),
                views,
                newcomers: newcomer_views,
            });
            next_sample += s.sample;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let last: Vec<View> = nodes.iter().map(|n| view(n, &kinds)).collect();
    snapshots.push(Snapshot {
        secs: begun.elapsed().as_secs_f64(),
        views: last.clone(),
        newcomers: newcomers.iter().map(|n| view(n, &kinds)).collect(),
    });
    let met_at_off = met_at_off.unwrap_or_else(|| vec![0; nodes.len()]);
    let known_at_hostile = known_at_hostile.unwrap_or_else(|| vec![HashSet::new(); nodes.len()]);
    let pushed_out = nodes
        .iter()
        .zip(&known_at_hostile)
        .map(|(node, known)| known.difference(&held(node)).count())
        .collect();
    // The simulation's last day, from the samples.
    let day_secs = DAY.div_f64(s.speed).as_secs_f64();
    let end = snapshots.last().map_or(0.0, |snap| snap.secs);
    let day_ago = snapshots
        .iter()
        .rev()
        .find(|snap| snap.secs <= end - day_secs)
        .unwrap_or(&snapshots[0]);
    let circuits_last_day = last
        .iter()
        .zip(&day_ago.views)
        .map(|(now, then)| {
            let span = (end - day_ago.secs).max(1.0);
            ((now.circuits - then.circuits) as f64 * day_secs / span) as u64
        })
        .collect();
    // For each machine: the established nodes holding it, those that met it,
    // and those that met it through another machine's record, not an
    // introduction; and those holding it through an introduction, and that met
    // it so.
    let mut met_by: HashMap<EndpointId, HeldBy> = HashMap::new();
    for node in &nodes {
        for a in node.acquaintances() {
            let counts = met_by.entry(a.machine).or_default();
            counts.held += 1;
            counts.introduced += a.introduced as usize;
            if a.stage != Stage::Heard {
                counts.met += 1;
                counts.met_introduced += a.introduced as usize;
                counts.passed_on +=
                    (!a.introduced && a.source.is_some_and(|from| from != known.machine)) as usize;
            }
        }
    }
    let counts = |machine: &EndpointId| met_by.get(machine).copied().unwrap_or_default();
    let arrived = newcomers_arrived.map_or(0.0, |at| at.as_secs_f64());
    let newcomer_ids = machine_ids(&newcomers)?;
    let newcomer_results = newcomer_ids
        .iter()
        .zip(&newcomers)
        .enumerate()
        .map(|(i, (id, node))| {
            let c = counts(id);
            let asked = node.introductions();
            let met = |snap: &Snapshot| snap.newcomers.get(i).map_or(0, |v| v.honest_met);
            Newcomer {
                met_three_after_secs: snapshots
                    .iter()
                    .find(|snap| met(snap) >= NEWCOMER_MEETS)
                    .map(|snap| snap.secs - arrived),
                honest_met: snapshots.last().map_or(0, met),
                held_by: c.held,
                met_by: c.met,
                passed_on: c.passed_on,
                introduced_by: c.introduced,
                met_through_introduction: c.met_introduced,
                asked: asked.asked,
                asked_passed_on: asked.asked_passed_on,
            }
        })
        .collect();
    let mut introductions = IntroductionTotals::default();
    for node in &nodes {
        let c = node.introductions();
        introductions.passed_on += c.passed_on;
        introductions.refused_at_limit += c.refused_at_limit;
        introductions.refused_by_each.push(c.refused_at_limit);
        introductions.accepted += c.accepted;
        introductions.declined += c.declined;
        introductions.declined_untrusted += c.declined_untrusted;
        introductions.declined_greeter += c.declined_greeter;
        introductions.declined_at_limit += c.declined_at_limit;
        introductions.declined_held += c.declined_held;
        introductions.declined_no_place += c.declined_no_place;
        introductions.asked += c.asked;
        introductions.asked_passed_on += c.asked_passed_on;
    }
    introductions.hostile_asked = operator.introduce_asked.load(Relaxed);
    introductions.hostile_asked_passed_on = operator.introduce_passed_on.load(Relaxed);
    introductions.hostile_introduced = operator.introduced.load(Relaxed);
    let report = MeetReport {
        scenario: s.clone(),
        met_after_directory_off: last
            .iter()
            .zip(&met_at_off)
            .map(|(v, then)| v.honest_met.saturating_sub(*then))
            .collect(),
        met_at_directory_off: met_at_off,
        known_at_hostile: known_at_hostile.iter().map(|k| k.len()).collect(),
        pushed_out,
        circuits_last_day,
        hostile_asked_known: operator.asked.load(Relaxed),
        held_at_newcomers,
        newcomers: newcomer_results,
        established_met_by: honest_list.iter().map(|m| counts(m).met).collect(),
        established_passed_on: honest_list.iter().map(|m| counts(m).passed_on).collect(),
        introductions,
        snapshots,
        wall_secs: started.elapsed().as_secs_f64(),
    };
    for router in routers {
        let _ = router.shutdown().await;
    }
    for node in nodes.into_iter().chain(newcomers).chain([directory]) {
        node.shutdown().await?;
    }
    Ok(report)
}

/// How the established nodes hold one machine.
#[derive(Debug, Clone, Copy, Default)]
struct HeldBy {
    held: usize,
    met: usize,
    passed_on: usize,
    introduced: usize,
    met_introduced: usize,
}

/// Honest nodes, each told of the directory and nothing else.
async fn start_honest(
    s: &MeetScenario,
    numbers: std::ops::Range<usize>,
    known: &KnownDirectory,
    settings: &impl Fn() -> SimSettings,
) -> Result<Vec<Node>> {
    stream::iter(numbers)
        .map(|i| {
            let mut config = Config::local_test(&format!("Node {i}"));
            config.directory.known = vec![known.clone()];
            let home = s.dir.join(format!("node-{i}"));
            let settings = settings();
            async move { start(&home, config, settings).await }
        })
        .buffered_unordered(STARTING_AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}

fn machine_ids(nodes: &[Node]) -> Result<Vec<EndpointId>> {
    nodes
        .iter()
        .map(|n| n.machine_id().context("no machine name"))
        .collect()
}

async fn start(home: &Path, config: Config, sim: SimSettings) -> Result<Node> {
    Node::init(home, &config)?;
    Node::spawn_sim(home, sim).await
}

fn not_yet_known(v: &View) -> usize {
    v.hostile - v.hostile_known
}

fn median(values: &[usize]) -> usize {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or(0)
}

/// One operator's names: each answers a greeting, and `Known` with some of the
/// others and fakes, freshly signed, as the operator holds all their keys.
/// With `MeetScenario::hostile_introduces`, each also asks the honest machines
/// it greeted to introduce it, as often as they'd pass on for one machine, and
/// introduces the others to honest machines that met it.
struct Operator {
    keys: Arc<Vec<iroh::SecretKey>>,
    asked: Arc<AtomicU64>,
    introduce_asked: Arc<AtomicU64>,
    introduce_passed_on: Arc<AtomicU64>,
    introduced: Arc<AtomicU64>,
    seed: u64,
}

impl Operator {
    fn new(names: usize, seed: u64) -> Self {
        Self {
            keys: Arc::new((0..names).map(|_| iroh::SecretKey::generate()).collect()),
            asked: Arc::default(),
            introduce_asked: Arc::default(),
            introduce_passed_on: Arc::default(),
            introduced: Arc::default(),
            seed,
        }
    }

    fn names(&self) -> impl Iterator<Item = EndpointId> + '_ {
        self.keys.iter().map(|key| key.public())
    }

    /// Puts its names on the network, and has each greet `greets` honest
    /// machines picked at random. Returns the routers, to keep.
    async fn arrive(
        &self,
        net: &SimNet,
        honest: &[EndpointId],
        s: &MeetScenario,
    ) -> Result<Vec<iroh::protocol::Router>> {
        let greets = s.hostile_greets;
        // As often as a machine passes on for one asker: 3 a simulated day.
        let ask_every = DAY.div_f64(3.0 * s.speed);
        let hour = HOUR.div_f64(s.speed);
        let mut routers = Vec::new();
        let mut rng = Rng(self.seed);
        for (i, key) in self.keys.iter().enumerate() {
            let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
                .relay_mode(iroh::RelayMode::Disabled)
                .clear_ip_transports()
                .add_custom_transport(net.link(Link::default()).transport(key.public()))
                .address_lookup(net.link(Link::default()).lookup())
                .secret_key(key.clone())
                .bind()
                .await?;
            let name = HostileName {
                me: i,
                keys: self.keys.clone(),
                asked: self.asked.clone(),
                rng: Arc::new(std::sync::Mutex::new(Rng(self.seed ^ i as u64))),
                met_by: Arc::default(),
            };
            let targets: Vec<EndpointId> = (0..greets.min(honest.len()))
                .map(|_| honest[rng.below(honest.len())])
                .collect();
            let greeter = endpoint.clone();
            let key = key.clone();
            let introduces = s.hostile_introduces.then(|| {
                (
                    name.clone(),
                    self.introduce_asked.clone(),
                    self.introduce_passed_on.clone(),
                    self.introduced.clone(),
                )
            });
            tokio::spawn(async move {
                for machine in &targets {
                    let _ = greet(&greeter, &key, *machine).await;
                }
                let Some((name, asked, passed_on, introduced)) = introduces else {
                    return;
                };
                let mut last_introduced = Instant::now();
                while !greeter.is_closed() {
                    for machine in &targets {
                        tokio::time::sleep(ask_every.div_f64(targets.len() as f64)).await;
                        asked.fetch_add(1, Relaxed);
                        if ask_introduce(&greeter, &key, *machine, &name.keys)
                            .await
                            .unwrap_or(false)
                        {
                            passed_on.fetch_add(1, Relaxed);
                        }
                        if last_introduced.elapsed() >= hour {
                            last_introduced = Instant::now();
                            let n = name.introduce_others(&greeter).await;
                            introduced.fetch_add(n, Relaxed);
                        }
                    }
                }
            });
            routers.push(
                iroh::protocol::Router::builder(endpoint)
                    .accept(MACHINE_ALPN, name)
                    .spawn(),
            );
        }
        Ok(routers)
    }
}

async fn greet(
    endpoint: &iroh::Endpoint,
    key: &iroh::SecretKey,
    machine: EndpointId,
) -> Result<()> {
    let conn = connect(endpoint, machine).await?;
    let record = MachineRecord::new(key, vec![], osp_proto::now_rfc3339())?;
    let asked = ask(&conn, &MachineRequest::Hello { record }).await;
    conn.close(0u32.into(), b"done");
    asked.map(|_| ())
}

async fn connect(
    endpoint: &iroh::Endpoint,
    machine: EndpointId,
) -> Result<iroh::endpoint::Connection> {
    let addr = iroh::EndpointAddr::from_parts(
        machine,
        [iroh::TransportAddr::Custom(simnet::addr(machine))],
    );
    Ok(endpoint.connect(addr, MACHINE_ALPN).await?)
}

async fn ask(
    conn: &iroh::endpoint::Connection,
    request: &MachineRequest,
) -> Result<MachineResponse> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, request).await?;
    send.finish()?;
    read_frame_max(&mut recv, 128 * 1024).await
}

/// A request to be introduced, signed by `asker`, through `via` to `target`.
fn introduce_request(
    asker: &iroh::SecretKey,
    via: EndpointId,
    target: EndpointId,
) -> Result<IntroduceRequest> {
    let mut id = [0; 16];
    id.copy_from_slice(&iroh::SecretKey::generate().to_bytes()[..16]);
    let expires =
        humantime::format_rfc3339_seconds(std::time::SystemTime::now() + Duration::from_secs(1800));
    let record = MachineRecord::new(asker, vec![], osp_proto::now_rfc3339())?;
    IntroduceRequest::new(asker, id, via, target, expires.to_string(), record)
}

/// Greets `machine` as `key`, asks it `Known`, and asks it to introduce `key`
/// to one of the honest machines named. Returns whether it said it would.
async fn ask_introduce(
    endpoint: &iroh::Endpoint,
    key: &iroh::SecretKey,
    machine: EndpointId,
    own: &[iroh::SecretKey],
) -> Result<bool> {
    let conn = connect(endpoint, machine).await?;
    let asked = async {
        let record = MachineRecord::new(key, vec![], osp_proto::now_rfc3339())?;
        ask(&conn, &MachineRequest::Hello { record }).await?;
        let MachineResponse::Known { records } = ask(&conn, &MachineRequest::Known).await? else {
            return Ok(false);
        };
        let own: HashSet<EndpointId> = own.iter().map(|k| k.public()).collect();
        let Some(target) = records
            .iter()
            .filter_map(|r| r.verify().ok())
            .map(|info| info.machine)
            .find(|m| !own.contains(m))
        else {
            return Ok(false);
        };
        let request = introduce_request(key, machine, target)?;
        Ok(matches!(
            ask(&conn, &MachineRequest::Introduce { request }).await?,
            MachineResponse::Introduce { passed_on: true }
        ))
    }
    .await;
    conn.close(0u32.into(), b"done");
    asked
}

#[derive(Clone)]
struct HostileName {
    me: usize,
    keys: Arc<Vec<iroh::SecretKey>>,
    asked: Arc<AtomicU64>,
    rng: Arc<std::sync::Mutex<Rng>>,
    /// Machines that greeted it, so have met it, and when last.
    met_by: Arc<std::sync::Mutex<HashMap<EndpointId, Instant>>>,
}

impl std::fmt::Debug for HostileName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostileName")
    }
}

impl HostileName {
    /// Introduces another of the operator's names, picked at random, to each of
    /// up to `HOSTILE_INTRODUCES_PER_HOUR` honest machines that met this one
    /// lately, those that greeted it last. Returns how many it passed on.
    async fn introduce_others(&self, endpoint: &iroh::Endpoint) -> u64 {
        let mut targets: Vec<(EndpointId, Instant)> = {
            let met_by = self.met_by.lock().unwrap_or_else(|e| e.into_inner());
            met_by.iter().map(|(m, at)| (*m, *at)).collect()
        };
        targets.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        let me = self.keys[self.me].public();
        let mut passed = 0;
        for (target, _) in targets.into_iter().take(HOSTILE_INTRODUCES_PER_HOUR) {
            let other = {
                let mut rng = self.rng.lock().unwrap_or_else(|e| e.into_inner());
                rng.below(self.keys.len())
            };
            if other == self.me {
                continue;
            }
            let Ok(request) = introduce_request(&self.keys[other], me, target) else {
                continue;
            };
            let Ok(conn) = connect(endpoint, target).await else {
                continue;
            };
            if ask(&conn, &MachineRequest::Introduction { request })
                .await
                .is_ok()
            {
                passed += 1;
            }
            conn.close(0u32.into(), b"done");
        }
        passed
    }

    fn answer(&self, from: EndpointId, request: MachineRequest) -> Result<MachineResponse> {
        let now = osp_proto::now_rfc3339();
        Ok(match request {
            MachineRequest::Hello { .. } => {
                if !self.keys.iter().any(|k| k.public() == from) {
                    let mut met_by = self.met_by.lock().unwrap_or_else(|e| e.into_inner());
                    met_by.insert(from, Instant::now());
                }
                MachineResponse::Hello {
                    record: MachineRecord::new(&self.keys[self.me], vec![], now)?,
                }
            }
            // It says it passes them on, and drops them.
            MachineRequest::Introduce { .. } => MachineResponse::Introduce { passed_on: true },
            MachineRequest::Introduction { .. } => MachineResponse::Introduction,
            MachineRequest::Known => {
                self.asked.fetch_add(1, Relaxed);
                let mut rng = self.rng.lock().unwrap_or_else(|e| e.into_inner());
                let mut records = Vec::new();
                // Half the others' names, half fakes.
                for _ in 0..MAX_KNOWN / 2 {
                    let other = rng.below(self.keys.len());
                    if other != self.me {
                        records.push(MachineRecord::new(&self.keys[other], vec![], now.clone())?);
                    }
                }
                while records.len() < MAX_KNOWN {
                    let fake = iroh::SecretKey::generate();
                    records.push(MachineRecord::new(&fake, vec![], now.clone())?);
                }
                MachineResponse::Known { records }
            }
            _ => MachineResponse::Error {
                message: "no".into(),
            },
        })
    }
}

impl iroh::protocol::ProtocolHandler for HostileName {
    async fn accept(
        &self,
        conn: iroh::endpoint::Connection,
    ) -> Result<(), iroh::protocol::AcceptError> {
        let from = conn.remote_id();
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let Ok(request) = read_frame_max(&mut recv, MAX_REQUEST_LEN).await else {
                break;
            };
            let Ok(reply) = self.answer(from, request) else {
                break;
            };
            if write_frame(&mut send, &reply).await.is_err() || send.finish().is_err() {
                break;
            }
        }
        Ok(())
    }
}

/// splitmix64, so a run's picks follow from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

impl MeetReport {
    /// Each pass mark, and whether it was met.
    pub fn marks(&self) -> Vec<(String, bool)> {
        let n = self.met_at_directory_off.len();
        let last = &self.snapshots.last().expect("a last snapshot").views;
        let everyone_met = last.iter().all(|v| v.honest_met > 0);
        // Those with more to meet when the directory went off met some after.
        let others = n.saturating_sub(1);
        let kept_meeting = self
            .met_at_directory_off
            .iter()
            .zip(&self.met_after_directory_off)
            .all(|(then, after)| *then >= others || *after > 0);
        let pushed_out: usize = self.pushed_out.iter().sum();
        let mut marks = vec![
            (
                "every node met another with no human act".into(),
                everyone_met,
            ),
            (
                "with the directory off, every node with more to meet met more".into(),
                kept_meeting,
            ),
            (
                format!(
                    "the operator pushed out no machine known longest ({pushed_out} pushed out)"
                ),
                pushed_out == 0,
            ),
        ];
        if !self.newcomers.is_empty() {
            let within = HOUR
                .mul_f64(NEWCOMER_WITHIN_HOURS)
                .div_f64(self.scenario.speed)
                .as_secs_f64();
            let of = self.newcomers.len();
            let count =
                |f: &dyn Fn(&Newcomer) -> bool| self.newcomers.iter().filter(|n| f(n)).count();
            let soon = count(&|n| n.met_three_after_secs.is_some_and(|t| t <= within));
            let met_by = count(&|n| n.met_by >= NEWCOMER_MET_BY);
            let passed_on = count(&|n| n.passed_on > 0);
            marks.extend([
                (
                    format!(
                        "each newcomer met {NEWCOMER_MEETS} established honest machines within {NEWCOMER_WITHIN_HOURS} simulated hours ({soon} of {of})"
                    ),
                    soon == of,
                ),
                (
                    format!(
                        "each newcomer was met by at least {NEWCOMER_MET_BY} established honest nodes ({met_by} of {of})"
                    ),
                    met_by == of,
                ),
                (
                    format!("each newcomer was passed on by another machine ({passed_on} of {of})"),
                    passed_on == of,
                ),
            ]);
            if self.scenario.hostile > 0 {
                let most = last.iter().map(not_yet_known).max().unwrap_or(0);
                marks.push((
                    format!(
                        "the operator's names not yet known, on each node, within the {GREETERS_PLACES} places of those who greet (at most {most})"
                    ),
                    most <= GREETERS_PLACES,
                ));
            }
            if let Some(before) = self.scenario.compare_met_by {
                let now = median(&self.newcomers.iter().map(|n| n.met_by).collect::<Vec<_>>());
                marks.push((
                    format!(
                        "newcomers met by more established nodes than before: a median of {now}, against {before}"
                    ),
                    now > before,
                ));
            }
            if self.scenario.hostile_introduces {
                let introduced = count(&|n| n.introduced_by > 0);
                let at_limit = self
                    .introductions
                    .refused_by_each
                    .iter()
                    .filter(|r| **r > 0)
                    .count();
                marks.push((
                    format!(
                        "each newcomer was introduced, while the operator used up introducers' limits ({introduced} of {of}; {at_limit} of {n} introducers refused some at their limit)"
                    ),
                    introduced == of,
                ));
            }
        }
        marks
    }

    pub fn summary(&self) -> String {
        let s = &self.scenario;
        let last = &self.snapshots.last().expect("a last snapshot").views;
        let pick = |f: fn(&View) -> usize| -> Vec<usize> { last.iter().map(f).collect() };
        let range = |values: &[usize]| {
            format!(
                "median {}, from {} to {}",
                median(values),
                values.iter().min().copied().unwrap_or(0),
                values.iter().max().copied().unwrap_or(0)
            )
        };
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} honest nodes, {} hostile names; meeting {}x as fast (a day is {:.0} min), known after {} hours; {:.0} min",
            s.honest,
            s.hostile,
            s.speed,
            DAY.div_f64(s.speed).as_secs_f64() / 60.0,
            s.known_after_hours,
            s.run_for.as_secs_f64() / 60.0
        );
        if !s.label.is_empty() {
            let _ = writeln!(out, "{}", s.label);
        }
        let _ = writeln!(
            out,
            "Honest machines met by each node: {}",
            range(&pick(|v| v.honest_met))
        );
        let _ = writeln!(out, "  of them known: {}", range(&pick(|v| v.honest_known)));
        let _ = writeln!(
            out,
            "  through another machine: {}; through the directory: {}; greeted: {}",
            range(&pick(|v| v.met_through_machines)),
            range(&pick(|v| v.met_through_directory)),
            range(&pick(|v| v.met_greeted))
        );
        let _ = writeln!(
            out,
            "When the directory went off at {:.0} min: {}; met after: {}",
            s.directory_off_after.as_secs_f64() / 60.0,
            range(&self.met_at_directory_off),
            range(&self.met_after_directory_off)
        );
        let _ = writeln!(
            out,
            "Known when the operator arrived at {:.0} min: {}; pushed out since: {}",
            s.hostile_after.as_secs_f64() / 60.0,
            range(&self.known_at_hostile),
            range(&self.pushed_out)
        );
        let _ = writeln!(
            out,
            "The operator's names held: {}; known: {}; in places kept longest: {}; fakes held: {}",
            range(&pick(|v| v.hostile)),
            range(&pick(|v| v.hostile_known)),
            range(&pick(|v| v.hostile_kept_longest)),
            range(&pick(|v| v.fakes))
        );
        let _ = writeln!(
            out,
            "  not yet known: {}; against those who greet: {}; through an introduction: {}",
            range(&pick(not_yet_known)),
            range(&pick(|v| v.hostile_greeters_share)),
            range(&pick(|v| v.hostile_introduced))
        );
        // The most on any node at any sample, and when.
        let most = |f: fn(&View) -> usize| {
            self.snapshots
                .iter()
                .map(|snap| (snap.views.iter().map(f).max().unwrap_or(0), snap.secs))
                .max_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)))
                .unwrap_or_default()
        };
        let (nyk, nyk_at) = most(not_yet_known);
        let (gs, gs_at) = most(|v| v.hostile_greeters_share);
        let _ = writeln!(
            out,
            "  the most on one node during the run: not yet known {nyk} (at {:.0} min); against those who greet {gs} (at {:.0} min)",
            nyk_at / 60.0,
            gs_at / 60.0
        );
        if !self.newcomers.is_empty() {
            let newcomers = &self.newcomers;
            let pick =
                |f: fn(&Newcomer) -> usize| -> Vec<usize> { newcomers.iter().map(f).collect() };
            let hours: Vec<usize> = newcomers
                .iter()
                .filter_map(|n| n.met_three_after_secs)
                .map(|secs| (secs * s.speed / 3600.0).round() as usize)
                .collect();
            let _ = writeln!(
                out,
                "{} newcomers at {:.0} min; the established nodes' tables then held: {}",
                newcomers.len(),
                s.newcomers_after.as_secs_f64() / 60.0,
                range(&self.held_at_newcomers)
            );
            let _ = writeln!(
                out,
                "  established honest machines each newcomer met: {}; hours to meet {NEWCOMER_MEETS}: {} ({} never did)",
                range(&pick(|n| n.honest_met)),
                range(&hours),
                newcomers.len() - hours.len()
            );
            let _ = writeln!(
                out,
                "  established nodes holding each newcomer: {}; that met it: {}; through another machine: {}",
                range(&pick(|n| n.held_by)),
                range(&pick(|n| n.met_by)),
                range(&pick(|n| n.passed_on))
            );
            let _ = writeln!(
                out,
                "  for an established machine, established nodes that met it: {}; through another machine: {}",
                range(&self.established_met_by),
                range(&self.established_passed_on)
            );
            let pick64 = |f: fn(&Newcomer) -> u64| -> Vec<usize> {
                newcomers.iter().map(|n| f(n) as usize).collect()
            };
            let _ = writeln!(
                out,
                "  established nodes holding each newcomer through an introduction: {}; that met it so: {}",
                range(&pick(|n| n.introduced_by)),
                range(&pick(|n| n.met_through_introduction))
            );
            let _ = writeln!(
                out,
                "  requests each newcomer made to be introduced: {}; passed on: {}",
                range(&pick64(|n| n.asked)),
                range(&pick64(|n| n.asked_passed_on))
            );
            let i = &self.introductions;
            let refused: Vec<usize> = i.refused_by_each.iter().map(|r| *r as usize).collect();
            let _ = writeln!(
                out,
                "Introductions: the established nodes passed on {} and refused {} at their limits (each: {}), accepted {} and declined {}",
                i.passed_on,
                i.refused_at_limit,
                range(&refused),
                i.accepted,
                i.declined
            );
            let _ = writeln!(
                out,
                "  declined: introducer not met long enough or its records failed {}, among those who greet {}; the hour's one taken {}; already met or held {}; no place {}",
                i.declined_untrusted,
                i.declined_greeter,
                i.declined_at_limit,
                i.declined_held,
                i.declined_no_place
            );
            let _ = writeln!(
                out,
                "  the established nodes asked {} times, {} passed on by the word of the machine asked",
                i.asked, i.asked_passed_on
            );
            if s.hostile_introduces {
                let _ = writeln!(
                    out,
                    "  the operator asked {} times, {} passed on, and passed on {} introductions of its own names",
                    i.hostile_asked, i.hostile_asked_passed_on, i.hostile_introduced
                );
            }
        }
        let circuits: Vec<usize> = self.circuits_last_day.iter().map(|c| *c as usize).collect();
        let _ = writeln!(
            out,
            "Tor circuits a node built in the last simulated day, estimated: {}",
            range(&circuits)
        );
        let _ = writeln!(
            out,
            "The operator was asked Known {} times",
            self.hostile_asked_known
        );
        for (mark, met) in self.marks() {
            let _ = writeln!(out, "{} {mark}", if met { "PASS" } else { "FAIL" });
        }
        let _ = writeln!(out, "Took {:.0} s", self.wall_secs);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_median_is_the_middle_one() {
        assert_eq!(median(&[3, 1, 2]), 2);
        assert_eq!(median(&[]), 0);
    }

    #[test]
    fn picks_follow_from_the_seed() {
        let (mut a, mut b) = (Rng(7), Rng(7));
        let picks: Vec<usize> = (0..10).map(|_| a.below(5)).collect();
        assert_eq!(picks, (0..10).map(|_| b.below(5)).collect::<Vec<_>>());
        assert!(picks.iter().all(|p| *p < 5));
    }
}
