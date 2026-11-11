//! Newcomers who find a post rather than follow its author ([ADR 0025]'s
//! step 4).
//!
//! The creator is listed by a directory. Once the followers' run and the
//! latecomers are done, newcomers arrive one after another, evenly over
//! hours. Each follows nobody: it reads the directory and opens the video
//! through a viewing endpoint ([ADR 0024]), from the hosts the directory
//! lists and those the creator's media endpoint names, the creator's own
//! last. Some then share it with hosting and ask to be listed. The directory
//! lists such a reshare as hosting once it has read the newcomer and checked
//! its copy, at its hourly read; the creator's node names it once it has
//! checked it, within seconds. Every timing is a real node's, but for each
//! newcomer reading the directory as it arrives and a sharer asking to be
//! listed at once.
//!
//! Measured: when each sharing newcomer first serves someone, how much of
//! newcomers' video came from earlier newcomers, and what the creator sent
//! them. Run with and without the place for a recently verified host.
//!
//! [ADR 0025]: ../../docs/decisions/0025-shared-posts-are-found-and-hosted.md
//! [ADR 0024]: ../../docs/decisions/0024-watching-found-posts-without-your-identity.md

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use n0_future::{FuturesUnordered, StreamExt, join_all};
use osp_node::{DirectRule, KnownDirectory, Node, SimSettings, simnet::SimNet};
use osp_proto::{
    BlobHash, EndpointId, ObjectId,
    rpc::{ClientRequest, ClientResponse},
};
use serde::Serialize;
use tracing::info;

use crate::{Scenario, call, in_turn, peers, sent, start, total};

/// Who arrives, and how.
#[derive(Debug, Clone)]
pub struct Newcomers {
    pub count: usize,
    /// They arrive evenly over this, the first at once.
    pub over: Duration,
    /// This share of them, in percent, share the post with hosting and ask
    /// to be listed, spread evenly: every fifth with 20, the first included.
    pub share_percent: usize,
}

/// A sharing newcomer has served someone once it has sent this much to
/// nodes other than the creator's and the directory's, whose checks of its
/// copy take a small piece.
const FIRST_UPLOAD_BYTES: f64 = 1e6;
/// How often sharing newcomers are looked at.
const LOOK_EVERY: Duration = Duration::from_secs(10);
/// Longest a newcomer's opening of the video may take.
const OPEN_TIMEOUT: Duration = Duration::from_secs(300);
/// Longest to wait for the directory to list the creator: it reads a node it
/// accepted at its next hourly read.
const LISTED_TIMEOUT: Duration = Duration::from_secs(90 * 60);
/// A sharer that shared at least this long before the last newcomer arrived
/// had the same chance to serve as any other: it's judged on whether it
/// served within this long.
pub const FIRST_UPLOAD_WINDOW: Duration = Duration::from_secs(30 * 60);

/// What the newcomers did.
#[derive(Debug, Clone, Serialize)]
pub struct NewcomersReport {
    pub count: usize,
    pub over_secs: f64,
    pub share_percent: usize,
    /// From posting: when the harness, looking once the followers were done,
    /// saw the directory list the creator; and when the first newcomer arrived.
    pub listed_after_secs: f64,
    pub started_secs: f64,
    /// From the first newcomer's arrival: when the last one was done.
    pub ended_secs: f64,
    pub video_bytes: u64,
    /// What the creator sent from the first arrival to the end.
    pub creator_sent_bytes: f64,
    pub newcomers: Vec<NewcomerRecord>,
}

/// One newcomer. Times are from the first newcomer's arrival unless said.
#[derive(Debug, Clone, Default, Serialize)]
pub struct NewcomerRecord {
    pub arrived_secs: f64,
    pub upload_mbps: u64,
    /// Opening the video took this long, from arriving; None if it failed.
    pub opened_after_secs: Option<f64>,
    pub error: Option<String>,
    pub shares: bool,
    pub shared_secs: Option<f64>,
    /// From sharing: when it had first served `FIRST_UPLOAD_BYTES`.
    pub first_upload_after_secs: Option<f64>,
    /// Media it sent to everyone but the creator's and the directory's nodes.
    pub sent_bytes: f64,
    /// Media it received, by whom from.
    pub from_creator: f64,
    pub from_newcomers: f64,
    pub from_others: f64,
}

/// What the run hands this phase.
pub(crate) struct Phase<'a> {
    pub s: &'a Scenario,
    pub newcomers: &'a Newcomers,
    pub net: &'a SimNet,
    pub creator: &'a Node,
    pub directory: &'a Node,
    pub post: ObjectId,
    pub video: BlobHash,
    pub video_bytes: u64,
    pub posted: Instant,
}

/// A directory on a fast link of the simulated network, at real nodes'
/// timings, taking as many new listings an hour as the run needs.
pub(crate) async fn start_directory(s: &Scenario, net: &SimNet, listings: usize) -> Result<Node> {
    let sim = SimSettings {
        network: Some(net.link(Default::default())),
        machine_names: true,
        tor_delay: s.tor_delay(),
        new_listings_per_hour: listings.max(SimSettings::default().new_listings_per_hour),
        ..SimSettings::default()
    };
    start(
        &s.dir.join("directory"),
        "The directory",
        0,
        false,
        Some(s.endpoints.unwrap_or(DirectRule::Nobody)),
        sim,
        |config| config.directory.serve = true,
    )
    .await
}

/// The directory as a node's config names it.
pub(crate) fn known(directory: &Node) -> Result<KnownDirectory> {
    Ok(KnownDirectory {
        machine: directory
            .machine_id()
            .context("the directory has no machine name")?,
        node: directory.node_id(),
    })
}

/// Whether newcomer `i` is one of the `percent` in a hundred that share.
fn shares(i: usize, percent: usize) -> bool {
    (i * percent) % 100 < percent
}

/// How a newcomer's arrival went.
struct Arrival {
    opened_after: Option<f64>,
    error: Option<String>,
    shared_at: Option<Instant>,
}

/// One newcomer to start.
struct Newcomer {
    home: PathBuf,
    name: String,
    upload_cap: u64,
    endpoints: Option<DirectRule>,
    sim: SimSettings,
    directory: KnownDirectory,
    sharer: bool,
}

/// Starts a newcomer, has it read the directory and open the video, and
/// share it if it's one that does.
async fn arrive(n: Newcomer, (post, video): (ObjectId, BlobHash)) -> Result<(Node, Arrival)> {
    let Newcomer {
        home,
        name,
        upload_cap,
        endpoints,
        sim,
        directory,
        sharer,
    } = n;
    let node = start(&home, &name, upload_cap, false, endpoints, sim, |c| {
        c.directory.known = vec![directory];
        c.directory.list_me = sharer;
    })
    .await?;
    let arrived = Instant::now();
    let mut arrival = Arrival {
        opened_after: None,
        error: None,
        shared_at: None,
    };
    let opened = async {
        match call(&node, ClientRequest::ReadDirectories).await? {
            ClientResponse::DirectoriesRead { found } => {
                ensure!(found > 0, "the directory listed nobody")
            }
            other => bail!("unexpected {other:?}"),
        }
        tokio::time::timeout(OPEN_TIMEOUT, node.open_blob(&video))
            .await
            .context("opening the video timed out")?
    };
    match opened.await {
        Ok(_) => arrival.opened_after = Some(arrived.elapsed().as_secs_f64()),
        Err(e) => arrival.error = Some(format!("{e:#}")),
    }
    if sharer && arrival.opened_after.is_some() {
        call(
            &node,
            ClientRequest::Reshare {
                id: post,
                host: true,
            },
        )
        .await?;
        arrival.shared_at = Some(Instant::now());
        ensure!(
            node.ask_to_be_listed().await,
            "{name} couldn't ask to be listed"
        );
    }
    Ok((node, arrival))
}

/// Media `seen` shows sent to anyone but `checkers`.
fn served(seen: &[osp_proto::rpc::PeerRecord], checkers: &HashSet<EndpointId>) -> f64 {
    total(
        seen.iter()
            .filter(|p| !checkers.contains(&p.node))
            .map(|p| p.counts.sent_bytes),
    )
}

impl Phase<'_> {
    pub(crate) async fn run(&self) -> Result<NewcomersReport> {
        let n = self.newcomers;
        let s = self.s;
        let listed_after_secs = self.wait_for_listing().await?;
        let creator_before = sent(&peers(self.creator).await?);
        let directory = known(self.directory)?;
        let checkers: HashSet<EndpointId> = [self.creator, self.directory]
            .iter()
            .flat_map(|node| node.endpoint_ids())
            .collect();
        let gap = n.over.div_f64(n.count.max(1) as f64);
        info!(
            newcomers = n.count,
            secs_apart = gap.as_secs_f64(),
            "newcomers start arriving"
        );

        let first = Instant::now();
        let started_secs = self.posted.elapsed().as_secs_f64();
        let mut records = vec![NewcomerRecord::default(); n.count];
        let mut nodes: Vec<Option<Node>> = (0..n.count).map(|_| None).collect();
        let mut shared_at: Vec<Option<Instant>> = vec![None; n.count];
        let mut arriving = FuturesUnordered::new();
        let mut next = 0;
        let mut looked = Instant::now();
        loop {
            while next < n.count && first.elapsed() >= gap * next as u32 {
                let i = next;
                let sharer = shares(i, n.share_percent);
                let mbps = in_turn(&s.follower_mbps, i);
                records[i] = NewcomerRecord {
                    arrived_secs: first.elapsed().as_secs_f64(),
                    upload_mbps: mbps,
                    shares: sharer,
                    ..Default::default()
                };
                let sim = SimSettings {
                    network: s.follower_link(Some(self.net), i),
                    machine_names: true,
                    tor_delay: s.tor_delay(),
                    ..SimSettings::default()
                };
                let newcomer = Newcomer {
                    home: s.dir.join(format!("n{i:04}")),
                    name: format!("N{i}"),
                    upload_cap: s.upload_cap(mbps),
                    endpoints: s.endpoints,
                    sim,
                    directory: directory.clone(),
                    sharer,
                };
                let arrival = arrive(newcomer, (self.post, self.video));
                arriving.push(async move { (i, arrival.await) });
                next += 1;
            }
            if next == n.count && arriving.is_empty() {
                break;
            }
            let until_next = match next < n.count {
                true => (gap * next as u32).saturating_sub(first.elapsed()),
                false => LOOK_EVERY,
            };
            tokio::select! {
                Some((i, arrived)) = arriving.next() => {
                    let (node, arrival) = arrived?;
                    let record = &mut records[i];
                    record.opened_after_secs = arrival.opened_after;
                    record.error = arrival.error;
                    record.shared_secs = arrival
                        .shared_at
                        .map(|t| t.duration_since(first).as_secs_f64());
                    shared_at[i] = arrival.shared_at;
                    nodes[i] = Some(node);
                }
                _ = tokio::time::sleep(until_next.min(LOOK_EVERY)) => {}
            }
            if looked.elapsed() >= LOOK_EVERY {
                looked = Instant::now();
                self.look(&nodes, &shared_at, &mut records, &checkers)
                    .await?;
                let opened = records.iter().filter(|r| r.opened_after_secs.is_some());
                info!(
                    secs = first.elapsed().as_secs(),
                    arrived = next,
                    opened = opened.count(),
                    shared = shared_at.iter().flatten().count(),
                    served = records
                        .iter()
                        .filter(|r| r.first_upload_after_secs.is_some())
                        .count(),
                    "newcomers"
                );
            }
        }
        self.look(&nodes, &shared_at, &mut records, &checkers)
            .await?;
        let ended_secs = first.elapsed().as_secs_f64();

        let mut owner: HashMap<EndpointId, Option<usize>> = HashMap::new();
        owner.insert(self.creator.node_id(), None);
        for (i, node) in nodes.iter().enumerate() {
            if let Some(node) = node {
                owner.insert(node.node_id(), Some(i));
            }
        }
        for (i, node) in nodes.iter().enumerate() {
            let Some(node) = node else { continue };
            let seen = peers(node).await?;
            let record = &mut records[i];
            record.sent_bytes = served(&seen, &checkers);
            for p in &seen {
                let bytes = p.counts.received_bytes;
                match owner.get(&p.node) {
                    Some(None) => record.from_creator += bytes,
                    Some(Some(_)) => record.from_newcomers += bytes,
                    None => record.from_others += bytes,
                }
            }
        }
        let creator_sent_bytes = sent(&peers(self.creator).await?) - creator_before;
        for result in join_all(nodes.into_iter().flatten().map(Node::shutdown)).await {
            result?;
        }
        Ok(NewcomersReport {
            count: n.count,
            over_secs: n.over.as_secs_f64(),
            share_percent: n.share_percent,
            listed_after_secs,
            started_secs,
            ended_secs,
            video_bytes: self.video_bytes,
            creator_sent_bytes,
            newcomers: records,
        })
    }

    /// Waits until the directory lists the creator, who asked once it had
    /// posted. Returns when, from posting.
    async fn wait_for_listing(&self) -> Result<f64> {
        let started = Instant::now();
        loop {
            let ClientResponse::Status(status) =
                call(self.directory, ClientRequest::Status).await?
            else {
                bail!("no status");
            };
            if status.directory.is_some_and(|d| d.people > 0) {
                let secs = self.posted.elapsed().as_secs_f64();
                info!(secs, "the directory lists the creator");
                return Ok(secs);
            }
            ensure!(
                started.elapsed() < LISTED_TIMEOUT,
                "the directory didn't list the creator"
            );
            tokio::time::sleep(LOOK_EVERY).await;
        }
    }

    /// Notes when each sharer that hasn't yet first served someone has.
    async fn look(
        &self,
        nodes: &[Option<Node>],
        shared_at: &[Option<Instant>],
        records: &mut [NewcomerRecord],
        checkers: &HashSet<EndpointId>,
    ) -> Result<()> {
        for (i, node) in nodes.iter().enumerate() {
            let (Some(node), Some(shared)) = (node, shared_at[i]) else {
                continue;
            };
            if records[i].first_upload_after_secs.is_some() {
                continue;
            }
            if served(&peers(node).await?, checkers) >= FIRST_UPLOAD_BYTES {
                records[i].first_upload_after_secs = Some(shared.elapsed().as_secs_f64());
            }
        }
        Ok(())
    }
}

impl NewcomersReport {
    /// The sharers that shared at least `FIRST_UPLOAD_WINDOW` before the
    /// last newcomer arrived.
    pub fn judged_sharers(&self) -> Vec<&NewcomerRecord> {
        let last = self
            .newcomers
            .iter()
            .map(|r| r.arrived_secs)
            .fold(0.0, f64::max);
        let window = FIRST_UPLOAD_WINDOW.as_secs_f64();
        self.newcomers
            .iter()
            .filter(|r| r.shared_secs.is_some_and(|t| t + window <= last))
            .collect()
    }

    /// Of the judged sharers: how many served within the window, and the
    /// median time to first serving, None if half never did.
    pub fn first_uploads(&self) -> (usize, usize, Option<f64>) {
        let judged = self.judged_sharers();
        let window = FIRST_UPLOAD_WINDOW.as_secs_f64();
        let within = judged
            .iter()
            .filter(|r| r.first_upload_after_secs.is_some_and(|t| t <= window))
            .count();
        let mut times: Vec<f64> = judged
            .iter()
            .map(|r| r.first_upload_after_secs.unwrap_or(f64::INFINITY))
            .collect();
        times.sort_by(f64::total_cmp);
        let median = times.get(times.len().saturating_sub(1) / 2).copied();
        (judged.len(), within, median.filter(|t| t.is_finite()))
    }

    /// The share of what newcomers received that came from earlier
    /// newcomers: of all of them, and of the later half.
    pub fn from_newcomers_share(&self) -> (f64, f64) {
        let share = |records: &[NewcomerRecord]| {
            let from = total(records.iter().map(|r| r.from_newcomers));
            let all = total(
                records
                    .iter()
                    .map(|r| r.from_newcomers + r.from_creator + r.from_others),
            );
            if all > 0.0 { from / all } else { 0.0 }
        };
        let half = self.newcomers.len() / 2;
        (share(&self.newcomers), share(&self.newcomers[half..]))
    }

    pub fn creator_copies(&self) -> f64 {
        self.creator_sent_bytes / self.video_bytes as f64
    }

    /// A few lines for people to read.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        let opened: Vec<f64> = {
            let mut t: Vec<f64> = self
                .newcomers
                .iter()
                .filter_map(|r| r.opened_after_secs)
                .collect();
            t.sort_by(f64::total_cmp);
            t
        };
        let failed: Vec<&NewcomerRecord> = self
            .newcomers
            .iter()
            .filter(|r| r.error.is_some())
            .collect();
        let _ = writeln!(
            out,
            "Newcomers: {} arrived over {:.0} min, from {:.0} s after posting; the directory listed the creator by {:.0} s.",
            self.count,
            self.over_secs / 60.0,
            self.started_secs,
            self.listed_after_secs,
        );
        let _ = writeln!(
            out,
            "  opened the video: {} (median {:.1} s, slowest {:.1} s); failed: {}{}",
            opened.len(),
            opened.get(opened.len() / 2).copied().unwrap_or(0.0),
            opened.last().copied().unwrap_or(0.0),
            failed.len(),
            failed
                .first()
                .and_then(|r| r.error.as_deref())
                .map(|e| format!(", the first: {e}"))
                .unwrap_or_default(),
        );
        let sharers = self
            .newcomers
            .iter()
            .filter(|r| r.shared_secs.is_some())
            .count();
        let served = self
            .newcomers
            .iter()
            .filter(|r| r.first_upload_after_secs.is_some())
            .count();
        let (judged, within, median) = self.first_uploads();
        let window = FIRST_UPLOAD_WINDOW.as_secs() / 60;
        let _ = writeln!(
            out,
            "  shared with hosting: {sharers}; served someone by the end: {served}. Of the {judged} that shared {window} min or more before the last arrival: {within} served within {window} min; median to first serving {}.",
            median.map_or("never (half didn't)".to_string(), |t| format!("{t:.0} s")),
        );
        let (all, later) = self.from_newcomers_share();
        let _ = writeln!(
            out,
            "  from earlier newcomers: {:.0}% of what newcomers received ({:.0}% for the later half); the creator sent them {:.1} copies of the video.",
            all * 100.0,
            later * 100.0,
            self.creator_copies(),
        );
        let most = self
            .newcomers
            .iter()
            .map(|r| r.sent_bytes / self.video_bytes as f64)
            .fold(0.0, f64::max);
        let _ = writeln!(out, "  the most one newcomer sent: {most:.1} copies.");
        out
    }
}
