//! Machines meeting machines, for ADR 0015's step 3 and ADR 0016's stage A:
//! honest nodes that know only a directory, with Tor-like delays, left alone.
//! The directory goes off part way, and then one operator's names arrive,
//! greeting honest machines and passing on one another and fakes. The times
//! among machines run faster than real ones (`MeetingTimes::faster`), and Tor's
//! delays don't, so a conversation takes as long as over Tor.
//!
//! Each node's view comes from `Node::acquaintances`, the record that never
//! leaves a real node. Circuits are an estimate (`SimTor` in `osp-node`).

use std::{
    collections::HashSet,
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
    machine::{MAX_KNOWN, MAX_REQUEST_LEN, MachineRecord, MachineRequest, MachineResponse},
};
use serde::Serialize;
use tracing::info;

/// Nodes started at once.
const STARTING_AT_ONCE: usize = 16;
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const HOUR: Duration = Duration::from_secs(60 * 60);

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
    /// The operator's names held, at any stage, and in places kept longest.
    pub hostile: usize,
    pub hostile_kept_longest: usize,
    /// Fakes held: records of machines that never existed.
    pub fakes: usize,
    pub circuits: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub secs: f64,
    pub views: Vec<View>,
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
    pub wall_secs: f64,
}

/// One node's machines, sorted by kind, at a moment.
fn view(node: &Node, honest: &HashSet<EndpointId>, hostile: &HashSet<EndpointId>) -> View {
    let mut v = View {
        circuits: node.machine_circuits().unwrap_or(0),
        ..View::default()
    };
    for a in node.acquaintances() {
        if honest.contains(&a.machine) {
            match a.stage {
                Stage::Heard => v.honest_heard += 1,
                stage => {
                    v.honest_met += 1;
                    v.honest_known += (stage == Stage::Known) as usize;
                    match a.source {
                        Some(from) if honest.contains(&from) => v.met_through_machines += 1,
                        Some(_) => v.met_through_directory += 1,
                        None => v.met_greeted += 1,
                    }
                }
            }
        } else if hostile.contains(&a.machine) {
            v.hostile += 1;
            v.hostile_kept_longest += a.kept_longest as usize;
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
    let nodes: Vec<Node> = stream::iter(0..s.honest)
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
        .collect::<Result<_>>()?;
    let honest_list: Vec<EndpointId> = nodes
        .iter()
        .map(|n| n.machine_id().context("no machine name"))
        .collect::<Result<_>>()?;
    let honest: HashSet<EndpointId> = honest_list.iter().copied().collect();
    let operator = Operator::new(s.hostile, s.seed);
    let hostile: HashSet<EndpointId> = operator.names().collect();

    let begun = Instant::now();
    let mut snapshots = Vec::new();
    let mut met_at_off = None;
    let mut known_at_hostile: Option<Vec<HashSet<EndpointId>>> = None;
    let mut routers = Vec::new();
    let mut next_sample = Duration::ZERO;
    while begun.elapsed() < s.run_for {
        let now = begun.elapsed();
        if met_at_off.is_none() && now >= s.directory_off_after {
            for id in directory.endpoint_ids() {
                net.set_online(id, false);
            }
            let met: Vec<usize> = nodes
                .iter()
                .map(|n| view(n, &honest, &hostile).honest_met)
                .collect();
            info!(median_met = median(&met), "the directory goes off");
            met_at_off = Some(met);
        }
        if known_at_hostile.is_none() && now >= s.hostile_after {
            known_at_hostile = Some(nodes.iter().map(|n| known_honest(n, &honest)).collect());
            routers = operator
                .arrive(&net, &honest_list, s.hostile_greets)
                .await?;
            info!(names = s.hostile, "the operator arrives");
        }
        if now >= next_sample {
            let views: Vec<View> = nodes.iter().map(|n| view(n, &honest, &hostile)).collect();
            let met: Vec<usize> = views.iter().map(|v| v.honest_met).collect();
            let fakes: Vec<usize> = views.iter().map(|v| v.hostile + v.fakes).collect();
            info!(
                mins = now.as_secs() / 60,
                median_met = median(&met),
                min_met = met.iter().min().copied().unwrap_or(0),
                median_hostile = median(&fakes),
                "sample"
            );
            snapshots.push(Snapshot {
                secs: now.as_secs_f64(),
                views,
            });
            next_sample += s.sample;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let last: Vec<View> = nodes.iter().map(|n| view(n, &honest, &hostile)).collect();
    snapshots.push(Snapshot {
        secs: begun.elapsed().as_secs_f64(),
        views: last.clone(),
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
        snapshots,
        wall_secs: started.elapsed().as_secs_f64(),
    };
    for router in routers {
        let _ = router.shutdown().await;
    }
    for node in nodes.into_iter().chain([directory]) {
        node.shutdown().await?;
    }
    Ok(report)
}

async fn start(home: &Path, config: Config, sim: SimSettings) -> Result<Node> {
    Node::init(home, &config)?;
    Node::spawn_sim(home, sim).await
}

fn median(values: &[usize]) -> usize {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or(0)
}

/// One operator's names: each answers a greeting, and `Known` with some of the
/// others and fakes, freshly signed, as the operator holds all their keys.
struct Operator {
    keys: Arc<Vec<iroh::SecretKey>>,
    asked: Arc<AtomicU64>,
    seed: u64,
}

impl Operator {
    fn new(names: usize, seed: u64) -> Self {
        Self {
            keys: Arc::new((0..names).map(|_| iroh::SecretKey::generate()).collect()),
            asked: Arc::default(),
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
        greets: usize,
    ) -> Result<Vec<iroh::protocol::Router>> {
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
            };
            let targets: Vec<EndpointId> = (0..greets.min(honest.len()))
                .map(|_| honest[rng.below(honest.len())])
                .collect();
            let greeter = endpoint.clone();
            let key = key.clone();
            tokio::spawn(async move {
                for machine in targets {
                    let _ = greet(&greeter, &key, machine).await;
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
    let addr = iroh::EndpointAddr::from_parts(
        machine,
        [iroh::TransportAddr::Custom(simnet::addr(machine))],
    );
    let conn = endpoint.connect(addr, MACHINE_ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    let record = MachineRecord::new(key, vec![], osp_proto::now_rfc3339())?;
    write_frame(&mut send, &MachineRequest::Hello { record }).await?;
    send.finish()?;
    let _: MachineResponse = read_frame_max(&mut recv, 128 * 1024).await?;
    conn.close(0u32.into(), b"done");
    Ok(())
}

#[derive(Clone)]
struct HostileName {
    me: usize,
    keys: Arc<Vec<iroh::SecretKey>>,
    asked: Arc<AtomicU64>,
    rng: Arc<std::sync::Mutex<Rng>>,
}

impl std::fmt::Debug for HostileName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostileName")
    }
}

impl HostileName {
    fn answer(&self, request: MachineRequest) -> Result<MachineResponse> {
        let now = osp_proto::now_rfc3339();
        Ok(match request {
            MachineRequest::Hello { .. } => MachineResponse::Hello {
                record: MachineRecord::new(&self.keys[self.me], vec![], now)?,
            },
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
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let Ok(request) = read_frame_max(&mut recv, MAX_REQUEST_LEN).await else {
                break;
            };
            let Ok(reply) = self.answer(request) else {
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
        vec![
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
        ]
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
            "The operator's names held: {}; in places kept longest: {}; fakes held: {}",
            range(&pick(|v| v.hostile)),
            range(&pick(|v| v.hostile_kept_longest)),
            range(&pick(|v| v.fakes))
        );
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
