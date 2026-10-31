//! The simulated network harness ([B8 test plan]): one creator and many
//! followers, all real nodes in this process, with relays off. They talk
//! over loopback, each node's upload held to a home connection's speed with
//! the existing upload cap; or over a simulated network
//! (`osp_node::simnet`), where each node's connection also has a download
//! speed, a delay and packet loss, and can drop out. The creator posts one
//! video; the harness measures what the creator sends, and when each
//! follower holds the video.
//!
//! [B8 test plan]: ../../docs/testing/b8-creator-upload.md

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use n0_future::{BufferedStreamExt, StreamExt, join_all, stream};
use osp_node::{
    Config, Node, SimSettings,
    config::SyncConfig,
    simnet::{Link, NetStats, SimLink, SimNet},
};
use osp_proto::{
    BlobHash, EndpointId, ObjectId,
    rpc::{ClientRequest, ClientResponse, PeerRecord},
};
use serde::Serialize;
use tracing::info;

/// Bytes per second of video the node's encoder makes from noise: its
/// 1,500 kbit/s ceiling, plus audio and the container. Measured on a
/// 4-minute video; short ones come out a little bigger.
const NOISE_BYTES_PER_SEC: u64 = 201_600;
/// How often the harness looks at every node.
const TICK: Duration = Duration::from_secs(1);
/// Nodes started at once.
const STARTING_AT_ONCE: usize = 16;
/// Longest to wait for every follower to subscribe.
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(120);
/// The serving side records a transfer once it ends there, which can be just
/// after the other side has it, so the harness waits this long before
/// reading the final counts.
const SETTLE: Duration = Duration::from_secs(2);
/// The follower that reshares the post to the strangers, by its place in
/// follower order. It never drops out.
const RESHARER: usize = 0;
/// Longest a latecomer may take to fetch the video.
const LATECOMER_TIMEOUT: Duration = Duration::from_secs(300);
/// How often the harness looks at the latecomer fetching: often enough to
/// time a download of seconds.
const LATECOMER_TICK: Duration = Duration::from_millis(250);
/// The followers last to hold the video whose sources the report shows.
const LAST_SHOWN: usize = 3;
const MBPS: u64 = 1_000_000;
/// A run is flagged as on a busy machine when other programs took this
/// share of its CPUs in some period, or everything together this much.
const BUSY_OTHERS_PERCENT: f64 = 10.0;
const FULL_PERCENT: f64 = 90.0;

/// One run: who takes part, the video, and how fast each node can upload.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// Where the nodes' homes go, on a real disk. A full run needs about
    /// 16 GB. It must be empty or not exist yet.
    pub dir: PathBuf,
    pub followers: usize,
    /// About how big the video should be once the creator's node has
    /// encoded it.
    pub video_bytes: u64,
    /// The creator's upload speed, in megabits per second.
    pub creator_mbps: u64,
    /// Followers' upload speeds, given to followers in turn.
    pub follower_mbps: Vec<u64>,
    /// Run over this simulated network instead of loopback.
    pub network: Option<Network>,
    /// Whether followers join the follower swarm (`swarm.share_prefetched`).
    pub swarm: bool,
    /// Followers in the swarm spread their fetches of the video over up to
    /// this long. A real node uses `SimSettings::default().prefetch_spread`.
    pub prefetch_spread: Duration,
    /// Most nodes a follower or stranger fetches a file from at once, each
    /// sending a part of it. A real node uses
    /// `SimSettings::default().download_sources`; 1 fetches from one node
    /// at a time.
    pub download_sources: usize,
    /// Followers that stop this long after posting, the last of each group
    /// of `followers / drop_out` in follower order, so they have every
    /// upload speed. The times until 50%, 95% and all count only the
    /// followers that stay.
    pub drop_out: usize,
    pub drop_out_after: Duration,
    /// Nodes that don't follow the creator, only the first follower, which
    /// reshares the post, hosting its media, as soon as it holds the video.
    /// They upload at the followers' speeds, given in turn.
    pub strangers: usize,
    /// Nodes that follow the creator once the run is over, one after
    /// another, and fetch the video at once, without spreading their
    /// fetches out: how long each takes shows how fast a download from the
    /// swarm is. Like followers, they upload at the followers' speeds and
    /// share what they fetch. Nothing else counts them.
    pub latecomers: usize,
    /// The first latecomer follows no sooner than this long after posting,
    /// so followers that stopped have been gone a while.
    pub latecomers_after: Duration,
    /// The run ends when every follower that stays and every stranger holds
    /// the video, or this long after posting.
    pub time_limit: Duration,
    /// How often to note the creator's upload so far.
    pub sample: Duration,
    /// Free text kept with the report, such as the commit tested.
    pub label: String,
}

/// A simulated network for a run (`osp_node::simnet`). Each node's
/// connection has its upload speed from the scenario, and these.
#[derive(Debug, Clone, Serialize)]
pub struct Network {
    /// The creator's download speed in megabits per second, its delay in
    /// milliseconds each way to the middle of the network, and the
    /// percentage of packets it loses each way.
    pub creator_down_mbps: u64,
    pub creator_delay_ms: u64,
    pub creator_loss_percent: f64,
    /// The same for followers, given in turn, as their upload speeds are.
    /// Strangers and latecomers get them too.
    pub down_mbps: Vec<u64>,
    pub delay_ms: Vec<u64>,
    pub loss_percent: Vec<f64>,
    /// Nodes also hold their upload to their connection's speed with the
    /// upload cap (`max_upload_mbps`), which gives a slow connection fewer
    /// transfers at once too. Otherwise they set none, as a node doesn't
    /// unless its owner does.
    pub caps: bool,
    /// This many followers' connections are cut off for `outage_secs` in
    /// every `outage_every_secs` after posting, each at a different time:
    /// the middle one of each group of `followers / flaky`. Outages end
    /// with the run, before any latecomers.
    pub flaky: usize,
    pub outage_secs: u64,
    pub outage_every_secs: u64,
    /// The random packet losses follow from this.
    pub seed: u64,
}

/// A run's simulated network: its settings, and what it did with packets.
#[derive(Debug, Clone, Serialize)]
pub struct NetworkReport {
    pub settings: Network,
    /// The flaky followers, by their place in follower order, and how many
    /// times one was cut off in all.
    pub flaky_followers: Vec<usize>,
    pub outages: usize,
    /// Everything sent through the network, latecomers' traffic included.
    pub packets: NetStats,
}

/// What a run measured. Times are seconds after the creator posted.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub label: String,
    pub started: String,
    pub followers: usize,
    pub creator_mbps: u64,
    pub follower_mbps: Vec<u64>,
    pub swarm: bool,
    pub prefetch_spread_secs: f64,
    pub download_sources: usize,
    pub time_limit_secs: u64,
    /// The video, and its media altogether (the video and its thumbnail).
    pub video_bytes: u64,
    pub media_bytes: u64,
    /// When each follower held the whole verified video, in follower order;
    /// `None` for followers that never did.
    pub held_after_secs: Vec<Option<f64>>,
    pub ended_secs: f64,
    /// Media bytes the creator sent, from its own peer record (ADR 0011).
    /// The record fades by half every 14 days, so an hour-long run reads
    /// up to 0.2% low.
    pub creator_sent_bytes: f64,
    /// The same, so far, at the end of each sample period.
    pub sample_secs: f64,
    pub creator_sent_so_far: Vec<f64>,
    /// Requests for media the creator refused as busy.
    pub creator_refused_busy: f64,
    /// Media bytes each follower sent to any node, in follower order.
    pub follower_sent_bytes: Vec<f64>,
    /// Media bytes the followers received from the creator, by their own
    /// records. With the strangers', it should match `creator_sent_bytes`.
    pub received_from_creator: f64,
    /// The followers' download attempts, from any node, by how they ended.
    pub downloads: Outcomes,
    /// Followers that stopped partway through, by their place in follower
    /// order, and when they were due to. None stop if the run ends first.
    pub dropped_out: Vec<usize>,
    pub drop_out_after_secs: f64,
    /// Nodes that followed only the resharer, the first follower, and when
    /// it reshared the post, if it did.
    pub strangers: usize,
    pub reshared_after_secs: Option<f64>,
    /// The same as for followers, for the strangers.
    pub stranger_held_after_secs: Vec<Option<f64>>,
    pub stranger_sent_bytes: Vec<f64>,
    pub stranger_downloads: Outcomes,
    /// Media bytes the creator sent to strangers, part of
    /// `creator_sent_bytes`, and what the strangers' records say came from it.
    pub creator_sent_to_strangers: f64,
    pub strangers_received_from_creator: f64,
    /// Nodes that followed the creator after the run, one after another,
    /// and how long each took from following to holding the video, if it
    /// did within `LATECOMER_TIMEOUT`.
    pub latecomers: usize,
    /// When the first latecomer followed, after posting.
    pub latecomers_started_secs: f64,
    pub latecomer_took_secs: Vec<Option<f64>>,
    pub latecomer_downloads: Outcomes,
    /// Media bytes the creator sent to latecomers. Not part of
    /// `creator_sent_bytes`.
    pub creator_sent_to_latecomers: f64,
    /// For each latecomer, what it received from each node, most first.
    pub latecomer_sources: Vec<Vec<Received>>,
    /// What the followers received, by the upload speed of the nodes it
    /// came from, the creator apart.
    pub follower_sources: Vec<Received>,
    /// The followers that held the video last, or never did, last first:
    /// each one's place in follower order, and what it received from each
    /// node, most first. Says whether they waited on busy or unreachable
    /// nodes.
    pub last_to_hold: Vec<(usize, Vec<Received>)>,
    /// How many transfers the creator served at once when the run ended,
    /// and the same for each follower that stayed, in follower order.
    pub creator_serving: Serving,
    pub follower_serving: Vec<Option<Serving>>,
    /// How busy the machine was, where it can be read.
    pub machine: Option<Machine>,
    /// The simulated network, if the run had one.
    pub network: Option<NetworkReport>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Outcomes {
    pub completed: f64,
    pub busy: f64,
    pub not_hosting: f64,
    pub bad_data: f64,
    pub failed: f64,
    /// Handed to a faster node before sending anything.
    pub handed_on: f64,
    /// Times a node couldn't connect to another, to download or to ask.
    pub unreached: f64,
}

/// What nodes received from one node, or from all the nodes with one
/// upload speed, by their own records.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Received {
    /// The node's name, or "creator" or the speed for a total.
    pub from: String,
    pub upload_mbps: u64,
    /// How many nodes sent something.
    pub nodes: usize,
    pub bytes: f64,
    /// Time taken by the attempts that brought the bytes.
    pub ms: f64,
    pub completed: f64,
    pub busy: f64,
    /// Attempts that ended without the file, most often a node that sent
    /// nothing for 30 s.
    pub failed: f64,
    pub handed_on: f64,
    /// Times the node couldn't be reached, to download from it or to ask.
    pub unreached: f64,
}

impl Received {
    /// The rate the bytes came at while they came.
    pub fn mbps(&self) -> f64 {
        if self.ms > 0.0 {
            self.bytes * 8.0 / self.ms / 1000.0
        } else {
            0.0
        }
    }
}

/// A node's media transfer places, and with no upload cap, the fastest it
/// was measured sending, in Mbit/s, if it was.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Serving {
    pub places: usize,
    pub measured_mbps: Option<f64>,
}

/// How busy the machine's CPUs were, from the post on, as shares of all of
/// them: other programs taking much of them makes a run's times longer
/// than the network alone would.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Machine {
    pub cpus: usize,
    /// For each period, this run's share and other programs' share, in %.
    /// The periods are `sample_secs` long, but for the end of the run and
    /// the latecomers' part.
    pub run_percent: Vec<f64>,
    pub others_percent: Vec<f64>,
}

/// The B8 pass marks: whether this run met each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marks {
    /// The creator sent at most a tenth as many copies as there are nodes
    /// that get the video: followers, and strangers if there are any.
    pub creator_copies: bool,
    /// 95% of followers held the video within 15 minutes.
    pub most_within_15_min: bool,
    /// Every follower held it within 30 minutes.
    pub all_within_30_min: bool,
    /// With strangers, the same two for them, counted from the reshare.
    pub strangers_most_within_15_min: Option<bool>,
    pub strangers_all_within_30_min: Option<bool>,
}

/// Runs a scenario from start to finish, then stops every node.
pub async fn run(s: &Scenario) -> Result<Report> {
    ensure!(s.followers > 0, "a run needs followers");
    ensure!(!s.follower_mbps.is_empty(), "followers need upload speeds");
    ensure!(
        s.drop_out < s.followers,
        "at least one follower has to stay"
    );
    ensure!(
        !s.dir.exists() || s.dir.read_dir()?.next().is_none(),
        "{} isn't empty",
        s.dir.display()
    );
    if let Some(n) = &s.network {
        ensure!(
            !n.down_mbps.is_empty() && !n.delay_ms.is_empty() && !n.loss_percent.is_empty(),
            "followers' connections need download speeds, delays and losses"
        );
        ensure!(
            2 * n.flaky < s.followers,
            "fewer than half the followers can be flaky"
        );
        ensure!(
            n.flaky == 0 || n.outage_secs < n.outage_every_secs,
            "an outage has to be shorter than the time from one to the next"
        );
    }
    std::fs::create_dir_all(&s.dir)?;
    let started = osp_proto::now_rfc3339();
    let source = make_video(&s.dir, s.video_bytes).await?;

    let net = s.network.as_ref().map(|n| SimNet::new(n.seed));
    let lifted = SimSettings {
        new_followers_per_hour: (s.followers + s.latecomers)
            .max(SimSettings::default().new_followers_per_hour),
        network: s.creator_link(net.as_ref()),
        ..SimSettings::default()
    };
    let creator = start(
        &s.dir.join("creator"),
        "Creator",
        s.upload_cap(s.creator_mbps),
        false,
        lifted,
    )
    .await?;
    let spread = SimSettings {
        prefetch_spread: s.prefetch_spread,
        download_sources: s.download_sources,
        // The resharer gathered its followers over time too.
        new_followers_per_hour: s
            .strangers
            .max(SimSettings::default().new_followers_per_hour),
        ..SimSettings::default()
    };
    let followers = start_all(s, net.as_ref(), "f", "F", s.followers, &spread).await?;
    let strangers = start_all(s, net.as_ref(), "s", "S", s.strangers, &spread).await?;
    let at_once = SimSettings {
        prefetch_spread: Duration::ZERO,
        download_sources: s.download_sources,
        ..SimSettings::default()
    };
    let latecomers = start_all(s, net.as_ref(), "l", "L", s.latecomers, &at_once).await?;
    info!(
        followers = followers.len(),
        strangers = strangers.len(),
        latecomers = latecomers.len(),
        "nodes started"
    );
    let mut hosts = HashMap::new();
    hosts.insert(creator.node_id(), Host::creator(s.creator_mbps));
    for (prefix, nodes) in [("F", &followers), ("S", &strangers), ("L", &latecomers)] {
        for (i, node) in nodes.iter().enumerate() {
            let host = Host::node(format!("{prefix}{i}"), in_turn(&s.follower_mbps, i));
            hosts.insert(node.node_id(), host);
        }
    }

    follow_all(&creator, &followers).await?;
    info!("every follower subscribed");
    if !strangers.is_empty() {
        follow_all(&followers[RESHARER], &strangers).await?;
        info!("every stranger subscribed to the resharer");
    }

    let ClientResponse::Posted { id: post } = call(
        &creator,
        ClientRequest::Post {
            content: "B8 test video".into(),
            attachments: vec![source],
            uploads: vec![],
            upload: None,
        },
    )
    .await?
    else {
        bail!("post failed");
    };
    let posted = Instant::now();
    let mut load = Load::start();
    let ClientResponse::Item { item } = call(&creator, ClientRequest::Show { id: post }).await?
    else {
        bail!("can't show the post");
    };
    let attachment = &item
        .attachments
        .first()
        .context("post has no video")?
        .attachment;
    let video = attachment.blob;
    let video_bytes = attachment.size;
    let media_bytes = video_bytes + attachment.thumbnail.as_ref().map_or(0, |t| t.size);
    info!(video_bytes, "posted");

    let dropping: Vec<usize> = (0..s.followers)
        .filter(|&i| drops_out(i, s.drop_out, s.followers))
        .collect();
    let flaky: Vec<usize> = match &s.network {
        Some(n) => (0..s.followers)
            .filter(|&i| is_flaky(i, n.flaky, s.followers))
            .collect(),
        None => Vec::new(),
    };
    // Flaky followers cut off now, and how many times one was.
    let mut cut: HashSet<usize> = HashSet::new();
    let mut outages = 0;
    // `None` once stopped; what a stopped follower measured is kept.
    let mut followers: Vec<Option<Node>> = followers.into_iter().map(Some).collect();
    let mut stopped: HashMap<usize, Vec<PeerRecord>> = HashMap::new();
    let mut held: Vec<Option<f64>> = vec![None; s.followers];
    let mut stranger_held: Vec<Option<f64>> = vec![None; s.strangers];
    let mut reshared_after_secs = None;
    let mut so_far = Vec::new();
    loop {
        note_holders(
            followers.iter().map(Option::as_ref),
            &mut held,
            post,
            video,
            posted,
        )
        .await?;
        note_holders(
            strangers.iter().map(Some),
            &mut stranger_held,
            post,
            video,
            posted,
        )
        .await?;
        let now = posted.elapsed();

        if let (Some(net), Some(n)) = (&net, &s.network) {
            for (k, &i) in flaky.iter().enumerate() {
                let Some(node) = &followers[i] else {
                    continue;
                };
                let off = cut_off_at(now, k, flaky.len(), n);
                if off != cut.contains(&i) {
                    let on_the_network = net.set_online(node.node_id(), !off);
                    if off {
                        cut.insert(i);
                        outages += usize::from(on_the_network);
                    } else {
                        cut.remove(&i);
                    }
                }
            }
        }
        if !strangers.is_empty() && reshared_after_secs.is_none() && held[RESHARER].is_some() {
            let resharer = followers[RESHARER]
                .as_ref()
                .context("the resharer stopped")?;
            call(
                resharer,
                ClientRequest::Reshare {
                    id: post,
                    host: true,
                },
            )
            .await?;
            reshared_after_secs = Some(posted.elapsed().as_secs_f64());
            info!(secs = now.as_secs(), "the first follower reshared the post");
        }
        if now >= s.drop_out_after && stopped.len() < dropping.len() {
            let leaving: Vec<(usize, Node)> = dropping
                .iter()
                .filter_map(|&i| Some((i, followers[i].take()?)))
                .collect();
            // All at once, each keeping what it measured.
            let left = join_all(leaving.into_iter().map(|(i, node)| async move {
                let seen = peers(&node).await;
                node.shutdown().await?;
                Ok::<_, anyhow::Error>((i, seen?))
            }))
            .await;
            for result in left {
                let (i, seen) = result?;
                stopped.insert(i, seen);
            }
            info!(
                secs = now.as_secs(),
                followers = dropping.len(),
                "followers stopped"
            );
        }

        let periods = (now.as_secs_f64() / s.sample.as_secs_f64()) as usize;
        if so_far.len() < periods {
            let sent = sent(&peers(&creator).await?);
            so_far.resize(periods, sent);
            load.sample();
            info!(
                secs = now.as_secs(),
                holding = held.iter().flatten().count(),
                strangers_holding = stranger_held.iter().flatten().count(),
                creator_copies = format!("{:.1}", sent / media_bytes as f64),
                "progress"
            );
        }
        // Followers due to stop count until they have.
        let all_held = held
            .iter()
            .enumerate()
            .all(|(i, h)| h.is_some() || stopped.contains_key(&i))
            && stranger_held.iter().all(Option::is_some);
        if all_held || now >= s.time_limit {
            break;
        }
        tokio::time::sleep(TICK).await;
    }
    let ended_secs = posted.elapsed().as_secs_f64();
    load.sample();
    // Outages end with the run.
    if let Some(net) = &net {
        for i in cut.drain() {
            if let Some(node) = &followers[i] {
                net.set_online(node.node_id(), true);
            }
        }
    }
    tokio::time::sleep(SETTLE).await;

    let mut dropped_out: Vec<usize> = stopped.keys().copied().collect();
    dropped_out.sort();
    let creator_seen = peers(&creator).await?;
    let mut follower_records = Vec::new();
    for (i, node) in followers.iter().enumerate() {
        follower_records.push(match node {
            Some(node) => peers(node).await?,
            None => stopped.remove(&i).context("a stopped follower's record")?,
        });
    }
    let mut stranger_records = Vec::new();
    for node in &strangers {
        stranger_records.push(peers(node).await?);
    }
    let from_followers = tally(&follower_records, creator.node_id());
    let from_strangers = tally(&stranger_records, creator.node_id());
    let stranger_ids: HashSet<EndpointId> = strangers.iter().map(Node::node_id).collect();
    let creator_sent_to_strangers = total(
        creator_seen
            .iter()
            .filter(|p| stranger_ids.contains(&p.node))
            .map(|p| p.counts.sent_bytes),
    );

    if !latecomers.is_empty() {
        tokio::time::sleep(s.latecomers_after.saturating_sub(posted.elapsed())).await;
    }
    let latecomers_started_secs = posted.elapsed().as_secs_f64();
    let latecomer_took_secs = latecomers_fetch(&creator, &latecomers, post, video).await?;
    tokio::time::sleep(SETTLE).await;
    if !latecomers.is_empty() {
        load.sample();
    }
    let mut latecomer_records = Vec::new();
    for node in &latecomers {
        latecomer_records.push(peers(node).await?);
    }
    let from_latecomers = tally(&latecomer_records, creator.node_id());
    let latecomer_sources = latecomer_records
        .iter()
        .map(|seen| received(seen, &hosts))
        .collect();
    let follower_sources = by_speed(&follower_records, &hosts);
    let last_to_hold = last_to_hold(&held, &dropped_out, LAST_SHOWN)
        .into_iter()
        .map(|i| (i, received(&follower_records[i], &hosts)))
        .collect();
    let latecomer_ids: HashSet<EndpointId> = latecomers.iter().map(Node::node_id).collect();
    let creator_sent_to_latecomers = total(
        peers(&creator)
            .await?
            .iter()
            .filter(|p| latecomer_ids.contains(&p.node))
            .map(|p| p.counts.sent_bytes),
    );
    let creator_serving = serving(&creator).await?;
    let mut follower_serving = Vec::new();
    for node in &followers {
        follower_serving.push(match node {
            Some(node) => Some(serving(node).await?),
            None => None,
        });
    }

    let running = followers
        .into_iter()
        .flatten()
        .chain(strangers)
        .chain(latecomers);
    for result in join_all(running.map(Node::shutdown)).await {
        result?;
    }
    creator.shutdown().await?;

    Ok(Report {
        label: s.label.clone(),
        started,
        followers: s.followers,
        creator_mbps: s.creator_mbps,
        follower_mbps: s.follower_mbps.clone(),
        swarm: s.swarm,
        prefetch_spread_secs: s.prefetch_spread.as_secs_f64(),
        download_sources: s.download_sources,
        time_limit_secs: s.time_limit.as_secs(),
        video_bytes,
        media_bytes,
        held_after_secs: held,
        ended_secs,
        creator_sent_bytes: sent(&creator_seen),
        sample_secs: s.sample.as_secs_f64(),
        creator_sent_so_far: so_far,
        creator_refused_busy: total(creator_seen.iter().map(|p| p.counts.refused_busy)),
        follower_sent_bytes: from_followers.sent,
        received_from_creator: from_followers.from_creator,
        downloads: from_followers.downloads,
        dropped_out,
        drop_out_after_secs: s.drop_out_after.as_secs_f64(),
        strangers: s.strangers,
        reshared_after_secs,
        stranger_held_after_secs: stranger_held,
        stranger_sent_bytes: from_strangers.sent,
        stranger_downloads: from_strangers.downloads,
        creator_sent_to_strangers,
        strangers_received_from_creator: from_strangers.from_creator,
        latecomers: s.latecomers,
        latecomers_started_secs,
        latecomer_took_secs,
        latecomer_downloads: from_latecomers.downloads,
        creator_sent_to_latecomers,
        latecomer_sources,
        follower_sources,
        last_to_hold,
        creator_serving,
        follower_serving,
        machine: load.machine,
        network: s.network.as_ref().zip(net).map(|(n, net)| NetworkReport {
            settings: n.clone(),
            flaky_followers: flaky,
            outages,
            packets: net.stats(),
        }),
    })
}

impl Scenario {
    /// The creator's place on the simulated network, if the run has one.
    fn creator_link(&self, net: Option<&SimNet>) -> Option<SimLink> {
        let n = self.network.as_ref()?;
        Some(net?.link(Link {
            up_bps: self.creator_mbps * MBPS,
            down_bps: n.creator_down_mbps * MBPS,
            delay: Duration::from_millis(n.creator_delay_ms),
            loss: n.creator_loss_percent / 100.0,
        }))
    }

    /// The same for the `i`th follower, stranger or latecomer.
    fn follower_link(&self, net: Option<&SimNet>, i: usize) -> Option<SimLink> {
        let n = self.network.as_ref()?;
        Some(net?.link(Link {
            up_bps: in_turn(&self.follower_mbps, i) * MBPS,
            down_bps: in_turn(&n.down_mbps, i) * MBPS,
            delay: Duration::from_millis(in_turn(&n.delay_ms, i)),
            loss: in_turn(&n.loss_percent, i) / 100.0,
        }))
    }

    /// The upload cap a node with this upload speed sets: on loopback, the
    /// speed itself; on the simulated network, none unless `caps`.
    fn upload_cap(&self, mbps: u64) -> u64 {
        match &self.network {
            Some(n) if !n.caps => 0,
            _ => mbps,
        }
    }
}

/// The `i`th setting of a list given in turn.
fn in_turn<T: Copy>(list: &[T], i: usize) -> T {
    list[i % list.len()]
}

impl Report {
    /// When each follower that stayed held the video.
    fn stayed(&self) -> Vec<Option<f64>> {
        let held = self.held_after_secs.iter().enumerate();
        held.filter(|(i, _)| !self.dropped_out.contains(i))
            .map(|(_, h)| *h)
            .collect()
    }

    /// How many followers that stayed held the video by the end.
    pub fn holding(&self) -> usize {
        self.stayed().iter().flatten().count()
    }

    /// When `percent` of the followers that stayed held the video, if they
    /// ever did.
    pub fn time_until(&self, percent: usize) -> Option<f64> {
        time_until(&self.stayed(), percent)
    }

    /// The same, for the strangers.
    pub fn stranger_time_until(&self, percent: usize) -> Option<f64> {
        time_until(&self.stranger_held_after_secs, percent)
    }

    /// When `percent` of the strangers held the video, counted from the
    /// reshare: they couldn't start before it.
    pub fn stranger_time_after_reshare(&self, percent: usize) -> Option<f64> {
        Some(self.stranger_time_until(percent)? - self.reshared_after_secs?)
    }

    /// How long it took `percent` of the latecomers to hold the video, from
    /// when each followed the creator.
    pub fn latecomer_time(&self, percent: usize) -> Option<f64> {
        time_until(&self.latecomer_took_secs, percent)
    }

    /// The creator's upload, in copies of the post's media.
    pub fn creator_copies(&self) -> f64 {
        self.creator_sent_bytes / self.media_bytes as f64
    }

    /// The most copies the creator may send: a tenth of the nodes that get
    /// the video.
    pub fn creator_allowance(&self) -> f64 {
        (self.followers + self.strangers) as f64 / 10.0
    }

    pub fn marks(&self) -> Marks {
        let within = |percent, secs| self.time_until(percent).is_some_and(|t| t <= secs);
        let strangers_within = |percent, secs| {
            let met = self.stranger_time_after_reshare(percent);
            (self.strangers > 0).then_some(met.is_some_and(|t| t <= secs))
        };
        Marks {
            creator_copies: self.creator_copies() <= self.creator_allowance(),
            most_within_15_min: within(95, 15.0 * 60.0),
            all_within_30_min: within(100, 30.0 * 60.0),
            strangers_most_within_15_min: strangers_within(95, 15.0 * 60.0),
            strangers_all_within_30_min: strangers_within(100, 30.0 * 60.0),
        }
    }

    /// A few lines for people to read.
    pub fn summary(&self) -> String {
        let copies = |bytes: f64| bytes / self.media_bytes as f64;
        let time = |t: Option<f64>| match t {
            Some(t) => format!("{t:.0} s"),
            None => "not reached".to_string(),
        };
        // Most, median, and how many sent nothing.
        let spread = |sent: &[f64]| {
            let mut sent = sent.to_vec();
            sent.sort_by(f64::total_cmp);
            let most = sent.last().copied().unwrap_or(0.0);
            let median = sent.get(sent.len() / 2).copied().unwrap_or(0.0);
            let none = sent.iter().filter(|b| **b < 1.0).count();
            (copies(most), copies(median), none)
        };
        let outcomes = |d: &Outcomes| {
            format!(
                "{:.0} completed, {:.0} busy, {:.0} not hosting, {:.0} bad data, {:.0} failed, {:.0} handed on; couldn't connect {:.0} times",
                d.completed, d.busy, d.not_hosting, d.bad_data, d.failed, d.handed_on, d.unreached,
            )
        };
        let marks = self.marks();
        let yes = |ok| if ok { "met" } else { "missed" };
        let (most, median, none) = spread(&self.follower_sent_bytes);
        let stayed = self.followers - self.dropped_out.len();

        let swarm = if self.swarm {
            format!(
                "swarm on, fetches spread over {:.0} s",
                self.prefetch_spread_secs
            )
        } else {
            "swarm off".to_string()
        };
        let sources = match self.download_sources {
            1 => "each file from one node at a time".to_string(),
            n => format!("each file from up to {n} nodes at once"),
        };
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} followers, {:.1} MB video, creator {} Mbit/s, followers {} Mbit/s, {swarm}, {sources}",
            self.followers,
            self.video_bytes as f64 / 1e6,
            self.creator_mbps,
            slashed(&self.follower_mbps),
        );
        if let Some(net) = &self.network {
            let n = &net.settings;
            let _ = writeln!(
                out,
                "simulated network, speeds above up: creator {} Mbit/s down, {} ms each way, {}% loss; followers {} Mbit/s down, {} ms each way, {}% loss; {}",
                n.creator_down_mbps,
                n.creator_delay_ms,
                n.creator_loss_percent,
                slashed(&n.down_mbps),
                slashed(&n.delay_ms),
                slashed(&n.loss_percent),
                if n.caps {
                    "nodes cap their upload at those speeds"
                } else {
                    "no upload caps"
                },
            );
            if !net.flaky_followers.is_empty() {
                let _ = writeln!(
                    out,
                    "{} followers cut off for {} s in every {} s, taking turns: {} outages in all",
                    net.flaky_followers.len(),
                    n.outage_secs,
                    n.outage_every_secs,
                    net.outages,
                );
            }
            let p = &net.packets;
            let share = |count: u64| 100.0 * count as f64 / p.packets.max(1) as f64;
            let _ = writeln!(
                out,
                "packets: {:.2} million sent, {:.2}% lost, {:.2}% dropped by full queues, {:.2}% while cut off",
                p.packets as f64 / 1e6,
                share(p.lost),
                share(p.queue_full),
                share(p.cut_off),
            );
        }
        if !self.dropped_out.is_empty() {
            let had_it = self.dropped_out.iter();
            let had_it = had_it.filter(|i| self.held_after_secs[**i].is_some());
            let _ = writeln!(
                out,
                "{} followers stopped at {:.0} s, {} of them holding the video; the rest of this counts only the {stayed} that stayed",
                self.dropped_out.len(),
                self.drop_out_after_secs,
                had_it.count(),
            );
        }
        let _ = writeln!(
            out,
            "held the video: {} of {stayed} when the run ended at {:.0} s; 50% at {}, 95% at {}, all at {}",
            self.holding(),
            self.ended_secs,
            time(self.time_until(50)),
            time(self.time_until(95)),
            time(self.time_until(100)),
        );
        let _ = writeln!(
            out,
            "creator sent {:.1} copies ({:.1} MB) and refused {:.0} requests as busy; followers' records say {:.1} copies came from it",
            self.creator_copies(),
            self.creator_sent_bytes / 1e6,
            self.creator_refused_busy,
            copies(self.received_from_creator),
        );
        let _ = writeln!(
            out,
            "creator's copies so far, every {:.0} s: {}",
            self.sample_secs,
            self.creator_sent_so_far
                .iter()
                .map(|b| format!("{:.1}", copies(*b)))
                .collect::<Vec<_>>()
                .join(" "),
        );
        let _ = writeln!(
            out,
            "followers sent: most {most:.1} copies, median {median:.1}; {none} sent nothing",
        );
        let _ = writeln!(out, "followers' downloads: {}", outcomes(&self.downloads));
        if self.strangers > 0 {
            let _ = writeln!(
                out,
                "strangers: the first follower reshared at {} and sent {:.1} copies; {} of {} strangers held the video; 50% at {}, 95% at {}, all at {}; after the reshare: {}, {}, {}",
                time(self.reshared_after_secs),
                copies(self.follower_sent_bytes[RESHARER]),
                self.stranger_held_after_secs.iter().flatten().count(),
                self.strangers,
                time(self.stranger_time_until(50)),
                time(self.stranger_time_until(95)),
                time(self.stranger_time_until(100)),
                time(self.stranger_time_after_reshare(50)),
                time(self.stranger_time_after_reshare(95)),
                time(self.stranger_time_after_reshare(100)),
            );
            let (most, median, none) = spread(&self.stranger_sent_bytes);
            let _ = writeln!(
                out,
                "the creator sent strangers {:.1} copies, and their records say {:.1}; strangers sent: most {most:.1} copies, median {median:.1}; {none} sent nothing",
                copies(self.creator_sent_to_strangers),
                copies(self.strangers_received_from_creator),
            );
            let _ = writeln!(
                out,
                "strangers' downloads: {}",
                outcomes(&self.stranger_downloads),
            );
        }
        if self.latecomers > 0 {
            let time = |t: Option<f64>| match t {
                Some(t) => format!("{t:.1} s"),
                None => "not reached".to_string(),
            };
            let _ = writeln!(
                out,
                "latecomers, following one after another from {:.0} s after posting and fetching at once: {} of {} held the video; half within {}, all within {}; the creator sent them {:.2} copies",
                self.latecomers_started_secs,
                self.latecomer_took_secs.iter().flatten().count(),
                self.latecomers,
                time(self.latecomer_time(50)),
                time(self.latecomer_time(100)),
                copies(self.creator_sent_to_latecomers),
            );
            let _ = writeln!(
                out,
                "latecomers' downloads: {}",
                outcomes(&self.latecomer_downloads),
            );
            for (i, sources) in self.latecomer_sources.iter().enumerate() {
                let took = self.latecomer_took_secs.get(i).copied().flatten();
                let _ = writeln!(
                    out,
                    "latecomer {i}, {}: {}",
                    time(took),
                    sources
                        .iter()
                        .map(|r| r.line(false))
                        .collect::<Vec<_>>()
                        .join("; "),
                );
            }
        }
        for (i, sources) in &self.last_to_hold {
            let held = match self.held_after_secs[*i] {
                Some(t) => format!("{t:.0} s"),
                None => "never".to_string(),
            };
            let _ = writeln!(
                out,
                "last to hold the video: F{i}, {held}: {}",
                sources
                    .iter()
                    .map(|r| r.line(false))
                    .collect::<Vec<_>>()
                    .join("; "),
            );
        }
        if !self.follower_sources.is_empty() {
            let _ = writeln!(
                out,
                "followers received, by the sending nodes' upload: {}",
                self.follower_sources
                    .iter()
                    .map(|r| r.line(true))
                    .collect::<Vec<_>>()
                    .join("; "),
            );
        }
        // Reports from before the places were measured have none.
        if self.creator_serving.places > 0 {
            let c = &self.creator_serving;
            let measured = match c.measured_mbps {
                Some(mbps) => format!(", measured {mbps:.1} Mbit/s"),
                None => String::new(),
            };
            let _ = writeln!(
                out,
                "transfer places when the run ended: creator {}{measured}; followers at {}",
                c.places,
                places_by_speed(&self.follower_mbps, &self.follower_serving).join("; "),
            );
        }
        if let Some(m) = &self.machine {
            let average =
                |shares: &[f64]| total(shares.iter().copied()) / shares.len().max(1) as f64;
            let most = |shares: &[f64]| shares.iter().copied().fold(0.0, f64::max);
            let busiest = m
                .run_percent
                .iter()
                .zip(&m.others_percent)
                .map(|(r, o)| r + o);
            let warning = if most(&m.others_percent) >= BUSY_OTHERS_PERCENT
                || busiest.fold(0.0, f64::max) >= FULL_PERCENT
            {
                "; a busy machine, so compare this run's times with care"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "machine: {} CPUs; this run took {:.0}% of them on average, most {:.0}%; other programs {:.0}%, most {:.0}%{warning}",
                m.cpus,
                average(&m.run_percent),
                most(&m.run_percent),
                average(&m.others_percent),
                most(&m.others_percent),
            );
        }
        let _ = write!(
            out,
            "pass marks: creator at most {:.0} copies {}; 95% within 15 min {}; all within 30 min {}",
            self.creator_allowance(),
            yes(marks.creator_copies),
            yes(marks.most_within_15_min),
            yes(marks.all_within_30_min),
        );
        if let (Some(most), Some(all)) = (
            marks.strangers_most_within_15_min,
            marks.strangers_all_within_30_min,
        ) {
            let _ = write!(
                out,
                "; strangers 95% within 15 min of the reshare {}; all within 30 min of it {}",
                yes(most),
                yes(all),
            );
        }
        out.push('\n');
        out
    }
}

/// Makes a video of noise that comes out at about `bytes` once a node has
/// encoded it. Noise can't be compressed, so the encoder runs at its ceiling.
async fn make_video(dir: &Path, bytes: u64) -> Result<PathBuf> {
    let secs = (bytes / NOISE_BYTES_PER_SEC).max(1).to_string();
    let out = dir.join("noise.mp4");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "rawvideo", "-pix_fmt", "gray", "-s", "320x180"])
        .args(["-r", "30", "-i", "/dev/urandom"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440"])
        .args(["-t", &secs, "-vf", "scale=1280:720:flags=neighbor"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "23"])
        .args(["-c:a", "aac", "-shortest"])
        .arg(&out)
        .status()
        .await
        .context("running ffmpeg")?;
    ensure!(status.success(), "ffmpeg failed making the test video");
    Ok(out)
}

/// Starts a node that caps its upload at `upload_cap` Mbit/s (0: no cap).
async fn start(
    home: &Path,
    name: &str,
    upload_cap: u64,
    swarm: bool,
    sim: SimSettings,
) -> Result<Node> {
    let mut config = Config::local_test(name);
    // Poll like a real node; test nodes poll only hourly.
    config.sync.interval_secs = SyncConfig::default().interval_secs;
    config.serving.max_upload_mbps = upload_cap;
    config.swarm.share_prefetched = swarm;
    // Noise always reaches the bitrate ceiling, so the preset changes only
    // how long encoding takes, not the size.
    config.media.x264_preset = "ultrafast".into();
    Node::init(home, &config)?;
    Node::spawn_sim(home, sim).await
}

async fn call(node: &Node, req: ClientRequest) -> Result<ClientResponse> {
    match node.handle(req).await {
        ClientResponse::Error { message } => bail!("{message}"),
        resp => Ok(resp),
    }
}

async fn wait_for_followers(creator: &Node, followers: usize) -> Result<()> {
    let start = Instant::now();
    loop {
        let ClientResponse::Status(status) = call(creator, ClientRequest::Status).await? else {
            bail!("no status");
        };
        if status.followers >= followers {
            return Ok(());
        }
        ensure!(
            start.elapsed() < SUBSCRIBE_TIMEOUT,
            "only {} of {followers} followers subscribed",
            status.followers
        );
        tokio::time::sleep(TICK).await;
    }
}

/// Whether `node` holds the whole verified video of `post`.
async fn holds(node: &Node, post: ObjectId, video: BlobHash) -> Result<bool> {
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 20 }).await?
    else {
        bail!("no feed");
    };
    Ok(items.iter().any(|item| {
        item.id == post
            && item
                .attachments
                .iter()
                .any(|a| a.attachment.blob == video && a.local)
    }))
}

async fn peers(node: &Node) -> Result<Vec<PeerRecord>> {
    let ClientResponse::Peers { peers } = call(node, ClientRequest::Peers).await? else {
        bail!("no peer record");
    };
    Ok(peers)
}

async fn serving(node: &Node) -> Result<Serving> {
    let ClientResponse::Status(status) = call(node, ClientRequest::Status).await? else {
        bail!("no status");
    };
    Ok(Serving {
        places: status.transfer_places,
        measured_mbps: status.upload_measured_mbps,
    })
}

/// The followers' transfer places, grouped by their upload speed, for
/// people to read: "1 Mbit/s: places 2 (270), measured 0.9 Mbit/s (0.5 to
/// 1.0)". Followers that stopped aren't counted.
fn places_by_speed(follower_mbps: &[u64], serving: &[Option<Serving>]) -> Vec<String> {
    let mut speeds = follower_mbps.to_vec();
    speeds.sort();
    speeds.dedup();
    speeds
        .into_iter()
        .map(|speed| {
            let nodes: Vec<Serving> = serving
                .iter()
                .enumerate()
                .filter(|(i, _)| in_turn(follower_mbps, *i) == speed)
                .filter_map(|(_, s)| *s)
                .collect();
            let mut places: BTreeMap<usize, usize> = BTreeMap::new();
            for node in &nodes {
                *places.entry(node.places).or_default() += 1;
            }
            let places: Vec<String> = places
                .iter()
                .map(|(places, nodes)| format!("{places} ({nodes})"))
                .collect();
            let mut measured: Vec<f64> = nodes.iter().filter_map(|n| n.measured_mbps).collect();
            measured.sort_by(f64::total_cmp);
            let measured = match (measured.first(), measured.last()) {
                (Some(least), Some(most)) => format!(
                    "measured {:.1} Mbit/s ({least:.1} to {most:.1})",
                    measured[measured.len() / 2]
                ),
                _ => "not measured".to_string(),
            };
            let unmeasured = nodes.iter().filter(|n| n.measured_mbps.is_none()).count();
            let unmeasured = if unmeasured > 0 && unmeasured < nodes.len() {
                format!(", {unmeasured} not measured")
            } else {
                String::new()
            };
            format!(
                "{speed} Mbit/s: places {}, {measured}{unmeasured}",
                places.join(", ")
            )
        })
        .collect()
}

/// Media bytes sent to every peer together.
fn sent(peers: &[PeerRecord]) -> f64 {
    total(peers.iter().map(|p| p.counts.sent_bytes))
}

/// Settings given in turn, for people to read: "5/10/20".
fn slashed<T: ToString>(list: &[T]) -> String {
    let list: Vec<String> = list.iter().map(T::to_string).collect();
    list.join("/")
}

/// Adds numbers up. Unlike `sum`, which makes -0.0 of nothing, this makes 0.
fn total(numbers: impl Iterator<Item = f64>) -> f64 {
    numbers.fold(0.0, |sum, n| sum + n)
}

/// When `percent` of a group held the video, if they ever did. Counts whole
/// nodes, rounding up.
fn time_until(held: &[Option<f64>], percent: usize) -> Option<f64> {
    let needed = (held.len() * percent).div_ceil(100).max(1);
    let mut times: Vec<f64> = held.iter().flatten().copied().collect();
    times.sort_by(f64::total_cmp);
    times.get(needed - 1).copied()
}

/// Whether follower `i` of `followers` is one of the `drop_out` that stop:
/// the last of each group of `followers / drop_out`.
fn drops_out(i: usize, drop_out: usize, followers: usize) -> bool {
    (i + 1) * drop_out / followers > i * drop_out / followers
}

/// Whether follower `i` of `followers` is one of the `flaky`: the middle one
/// of each group of `followers / flaky`. So with groups of at least three,
/// none is one that stops partway (the last of each), and with fewer than
/// half the followers flaky, the resharer (the first) isn't.
fn is_flaky(i: usize, flaky: usize, followers: usize) -> bool {
    // How many middles come before `i`, rounding to the nearest.
    let middles = |i: usize| (2 * i * flaky + followers) / (2 * followers);
    middles(i + 1) > middles(i)
}

/// Whether the `k`th of `count` flaky followers is cut off `after` posting.
/// Each is cut off for `outage_secs` in every `outage_every_secs`, the
/// `k`th first `(k + 1) / (count + 1)` of the way into the first period, so
/// that they take turns.
fn cut_off_at(after: Duration, k: usize, count: usize, n: &Network) -> bool {
    let every = Duration::from_secs(n.outage_every_secs);
    let first = every.mul_f64((k + 1) as f64 / (count + 1) as f64);
    after >= first
        && (after - first).as_millis() % every.as_millis() < u128::from(n.outage_secs) * 1_000
}

/// Starts `count` nodes at the followers' upload speeds, given in turn, with
/// homes `{dir}0000`, `{dir}0001` and so on, on `net` if there is one.
async fn start_all(
    s: &Scenario,
    net: Option<&SimNet>,
    dir: &str,
    name: &str,
    count: usize,
    sim: &SimSettings,
) -> Result<Vec<Node>> {
    stream::iter(0..count)
        .map(|i| {
            let home = s.dir.join(format!("{dir}{i:04}"));
            let cap = s.upload_cap(in_turn(&s.follower_mbps, i));
            let sim = SimSettings {
                network: s.follower_link(net, i),
                ..sim.clone()
            };
            async move { start(&home, &format!("{name}{i}"), cap, s.swarm, sim).await }
        })
        .buffered_ordered(STARTING_AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}

/// Has every node in `nodes` follow `followed`, and waits until all have
/// subscribed.
async fn follow_all(followed: &Node, nodes: &[Node]) -> Result<()> {
    let ClientResponse::Ticket { ticket } = call(followed, ClientRequest::Invite).await? else {
        bail!("no ticket");
    };
    for node in nodes {
        call(
            node,
            ClientRequest::Follow {
                ticket: ticket.clone(),
            },
        )
        .await?;
    }
    wait_for_followers(followed, nodes.len()).await
}

/// Has each latecomer in turn follow the creator and fetch the video at
/// once. Returns how long each took, from following to holding it.
async fn latecomers_fetch(
    creator: &Node,
    latecomers: &[Node],
    post: ObjectId,
    video: BlobHash,
) -> Result<Vec<Option<f64>>> {
    let ClientResponse::Ticket { ticket } = call(creator, ClientRequest::Invite).await? else {
        bail!("no ticket");
    };
    let mut took = Vec::new();
    for (i, node) in latecomers.iter().enumerate() {
        let started = Instant::now();
        let follow = ClientRequest::Follow {
            ticket: ticket.clone(),
        };
        call(node, follow).await?;
        call(node, ClientRequest::Sync).await?;
        let held = loop {
            if holds(node, post, video).await? {
                break Some(started.elapsed().as_secs_f64());
            }
            if started.elapsed() >= LATECOMER_TIMEOUT {
                break None;
            }
            tokio::time::sleep(LATECOMER_TICK).await;
        };
        info!(latecomer = i, secs = ?held, "a latecomer fetched the video");
        took.push(held);
    }
    Ok(took)
}

/// Notes the time since `posted` for each running node that has come to
/// hold the video. `held` is in the same order as `nodes`.
async fn note_holders<'a>(
    nodes: impl Iterator<Item = Option<&'a Node>>,
    held: &mut [Option<f64>],
    post: ObjectId,
    video: BlobHash,
    posted: Instant,
) -> Result<()> {
    let waiting: Vec<(usize, &Node)> = nodes
        .enumerate()
        .filter(|(i, _)| held[*i].is_none())
        .filter_map(|(i, node)| Some((i, node?)))
        .collect();
    let checks = join_all(waiting.iter().map(|(_, node)| holds(node, post, video))).await;
    let now = posted.elapsed().as_secs_f64();
    for ((i, _), check) in waiting.iter().zip(checks) {
        if check? {
            held[*i] = Some(now);
        }
    }
    Ok(())
}

/// What a group of nodes sent and received, by their own records.
struct Tally {
    /// Media bytes each sent to any node, in order.
    sent: Vec<f64>,
    /// Media bytes they received from the creator, together.
    from_creator: f64,
    downloads: Outcomes,
}

fn tally(records: &[Vec<PeerRecord>], creator: EndpointId) -> Tally {
    let mut t = Tally {
        sent: Vec::new(),
        from_creator: 0.0,
        downloads: Outcomes::default(),
    };
    for seen in records {
        t.sent.push(sent(seen));
        for p in seen {
            let c = &p.counts;
            if p.node == creator {
                t.from_creator += c.received_bytes;
            }
            let d = &mut t.downloads;
            d.completed += c.completed;
            d.busy += c.busy;
            d.not_hosting += c.not_hosting;
            d.bad_data += c.bad_data;
            d.failed += c.failed;
            d.handed_on += c.handed_on;
            d.unreached += c.unreached;
        }
    }
    t
}

/// A node in the run, as the report names it.
struct Host {
    name: String,
    upload_mbps: u64,
    creator: bool,
}

impl Host {
    fn creator(upload_mbps: u64) -> Self {
        Self {
            name: "creator".to_string(),
            upload_mbps,
            creator: true,
        }
    }

    fn node(name: String, upload_mbps: u64) -> Self {
        Self {
            name,
            upload_mbps,
            creator: false,
        }
    }
}

/// What one node's records say it received from each node it downloaded
/// from, or tried to, most first.
fn received(seen: &[PeerRecord], hosts: &HashMap<EndpointId, Host>) -> Vec<Received> {
    let mut list: Vec<Received> = seen
        .iter()
        .filter(|p| {
            let c = &p.counts;
            c.received_bytes > 0.0
                || c.completed + c.busy + c.failed + c.handed_on + c.unreached > 0.0
        })
        .map(|p| {
            let (from, upload_mbps) = match hosts.get(&p.node) {
                Some(h) => (h.name.clone(), h.upload_mbps),
                None => (p.node.fmt_short().to_string(), 0),
            };
            Received {
                from,
                upload_mbps,
                nodes: 1,
                bytes: p.counts.received_bytes,
                ms: p.counts.receive_ms,
                completed: p.counts.completed,
                busy: p.counts.busy,
                failed: p.counts.failed,
                handed_on: p.counts.handed_on,
                unreached: p.counts.unreached,
            }
        })
        .collect();
    list.sort_by(|a, b| b.bytes.total_cmp(&a.bytes));
    list
}

/// The `n` followers that stayed and held the video last, or never did,
/// last first, by their place in follower order.
fn last_to_hold(held: &[Option<f64>], dropped_out: &[usize], n: usize) -> Vec<usize> {
    let mut stayed: Vec<usize> = (0..held.len())
        .filter(|i| !dropped_out.contains(i))
        .collect();
    let when = |i: &usize| held[*i].unwrap_or(f64::INFINITY);
    stayed.sort_by(|a, b| when(b).total_cmp(&when(a)));
    stayed.truncate(n);
    stayed
}

/// What a group of nodes received, added up by the upload speed of the
/// nodes it came from, slowest first, with the creator last on its own.
fn by_speed(records: &[Vec<PeerRecord>], hosts: &HashMap<EndpointId, Host>) -> Vec<Received> {
    // Keyed by (from the creator, speed); the senders seen, to count them.
    let mut totals: std::collections::BTreeMap<(bool, u64), (Received, HashSet<EndpointId>)> =
        Default::default();
    for seen in records {
        for p in seen {
            let Some(host) = hosts.get(&p.node) else {
                continue;
            };
            let (t, senders) = totals
                .entry((host.creator, host.upload_mbps))
                .or_insert_with(|| {
                    let from = if host.creator {
                        "creator".to_string()
                    } else {
                        format!("{} Mbit/s nodes", host.upload_mbps)
                    };
                    let r = Received {
                        from,
                        upload_mbps: host.upload_mbps,
                        ..Received::default()
                    };
                    (r, HashSet::new())
                });
            let c = &p.counts;
            if c.received_bytes > 0.0 {
                senders.insert(p.node);
            }
            t.bytes += c.received_bytes;
            t.ms += c.receive_ms;
            t.completed += c.completed;
            t.busy += c.busy;
            t.failed += c.failed;
            t.handed_on += c.handed_on;
            t.unreached += c.unreached;
        }
    }
    totals
        .into_values()
        .map(|(mut t, senders)| {
            t.nodes = senders.len();
            t
        })
        .collect()
}

impl Received {
    /// One entry of a summary line, for one node or, with `total`, for
    /// what [`by_speed`] added up.
    fn line(&self, total: bool) -> String {
        let mut line = if !total {
            format!("{} ({} Mbit/s up)", self.from, self.upload_mbps)
        } else if self.from == "creator" {
            self.from.clone()
        } else {
            format!("{} ({} sending)", self.from, self.nodes)
        };
        let _ = write!(
            line,
            " {:.1} MB at {:.2} Mbit/s, {:.0} completed",
            self.bytes / 1e6,
            self.mbps(),
            self.completed,
        );
        if self.busy > 0.0 {
            let _ = write!(line, ", {:.0} busy", self.busy);
        }
        if self.failed > 0.0 {
            let _ = write!(line, ", {:.0} failed", self.failed);
        }
        if self.handed_on > 0.0 {
            let _ = write!(line, ", {:.0} handed on", self.handed_on);
        }
        if self.unreached > 0.0 {
            let _ = write!(line, ", couldn't connect {:.0} times", self.unreached);
        }
        line
    }
}

/// CPU time so far, in clock ticks: the whole machine's busy and total
/// time over all its CPUs, and this process's.
#[derive(Clone, Copy)]
struct CpuTimes {
    busy: u64,
    total: u64,
    ours: u64,
}

/// Reads [`CpuTimes`] from `/proc`, on Linux.
fn cpu_times() -> Option<CpuTimes> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().next()?.strip_prefix("cpu ")?;
    // user, nice, system, idle, iowait, irq, softirq, steal.
    let n: Vec<u64> = line
        .split_whitespace()
        .take(8)
        .map(|f| f.parse().ok())
        .collect::<Option<_>>()?;
    let total: u64 = n.iter().sum();
    let idle = n.get(3)? + n.get(4)?;
    let own = std::fs::read_to_string("/proc/self/stat").ok()?;
    // After the program's name, which can hold spaces: state is the first
    // field, user time the 12th and system time the 13th.
    let fields: Vec<&str> = own[own.rfind(')')? + 1..].split_whitespace().collect();
    let ours = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    Some(CpuTimes {
        busy: total - idle,
        total,
        ours,
    })
}

/// Samples how busy the machine is, period by period.
struct Load {
    last: Option<CpuTimes>,
    machine: Option<Machine>,
}

impl Load {
    fn start() -> Self {
        let last = cpu_times();
        Self {
            last,
            machine: last.map(|_| Machine {
                cpus: std::thread::available_parallelism().map_or(1, usize::from),
                ..Machine::default()
            }),
        }
    }

    /// Adds the period since the last sample.
    fn sample(&mut self) {
        let (Some(last), Some(machine), Some(now)) = (self.last, &mut self.machine, cpu_times())
        else {
            return;
        };
        let total = now.total.saturating_sub(last.total);
        if total == 0 {
            return;
        }
        let ours = now.ours.saturating_sub(last.ours);
        let others = now.busy.saturating_sub(last.busy).saturating_sub(ours);
        let share = |ticks: u64| 100.0 * ticks as f64 / total as f64;
        machine.run_percent.push(share(ours));
        machine.others_percent.push(share(others));
        self.last = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Followers that held the video after 1, 2, 3, ... seconds, then
    /// `missing` that never did.
    fn report(held: usize, missing: usize) -> Report {
        let mut held_after_secs: Vec<_> = (1..=held).map(|t| Some(t as f64)).collect();
        held_after_secs.extend(std::iter::repeat_n(None, missing));
        Report {
            followers: held + missing,
            held_after_secs,
            media_bytes: 1_000,
            ..Report::default()
        }
    }

    #[test]
    fn times_count_whole_followers_rounding_up() {
        // 95% of 12 is 11.4 followers: it takes the 12th.
        let r = report(12, 0);
        assert_eq!(r.time_until(95), Some(12.0));
        assert_eq!(r.time_until(50), Some(6.0));
        assert_eq!(r.time_until(100), Some(12.0));
        assert_eq!(r.time_until(0), Some(1.0));
        // Held out of order, still counted in order of time.
        let mut shuffled = r.clone();
        shuffled.held_after_secs.reverse();
        assert_eq!(shuffled.time_until(50), Some(6.0));

        let r = report(285, 15);
        assert_eq!(r.time_until(95), Some(285.0));
        assert_eq!(r.time_until(96), None, "never reached");
        assert_eq!(r.holding(), 285);
    }

    #[test]
    fn the_last_to_hold_the_video_are_those_that_stayed_never_holding_it_first() {
        let held = [Some(5.0), None, Some(9.0), Some(1.0), None, Some(7.0)];
        // Follower 1 dropped out without it.
        assert_eq!(last_to_hold(&held, &[1], 3), [4, 2, 5]);
        assert_eq!(last_to_hold(&held, &[1, 4], 2), [2, 5]);
        assert_eq!(last_to_hold(&held, &[], 10).len(), 6);
    }

    #[test]
    fn pass_marks_are_the_plans() {
        let mut r = report(300, 0);
        // Everyone within 300 s.
        r.creator_sent_bytes = 30.0 * 1_000.0;
        assert_eq!(
            r.marks(),
            Marks {
                creator_copies: true,
                most_within_15_min: true,
                all_within_30_min: true,
                strangers_most_within_15_min: None,
                strangers_all_within_30_min: None,
            }
        );
        r.creator_sent_bytes = 30.1 * 1_000.0;
        assert!(!r.marks().creator_copies);

        // The 285th follower at exactly 15 minutes passes; a second later doesn't.
        let set_last_16 = |r: &mut Report, secs| {
            for t in &mut r.held_after_secs[284..] {
                *t = Some(secs);
            }
        };
        set_last_16(&mut r, 900.0);
        assert!(r.marks().most_within_15_min);
        set_last_16(&mut r, 901.0);
        assert!(!r.marks().most_within_15_min);
        assert!(r.marks().all_within_30_min);
        r.held_after_secs[299] = Some(1801.0);
        assert!(!r.marks().all_within_30_min);
        r.held_after_secs[299] = None;
        assert!(!r.marks().all_within_30_min);
    }

    #[test]
    fn strangers_count_toward_the_creator_and_from_the_reshare() {
        let mut r = report(300, 0);
        r.follower_sent_bytes = vec![0.0; 300];
        r.strangers = 200;
        r.reshared_after_secs = Some(600.0);
        // 190 strangers at 15 minutes after the reshare, the last 10 at 30.
        r.stranger_held_after_secs = (0..200)
            .map(|i| Some(if i < 190 { 1500.0 } else { 2400.0 }))
            .collect();
        r.creator_sent_bytes = 50.0 * 1_000.0;
        let marks = r.marks();
        assert!(marks.creator_copies, "a tenth of 500 nodes");
        assert_eq!(marks.strangers_most_within_15_min, Some(true));
        assert_eq!(marks.strangers_all_within_30_min, Some(true));
        assert!(
            r.summary()
                .contains("of the reshare met; all within 30 min of it met")
        );

        r.creator_sent_bytes = 50.1 * 1_000.0;
        r.stranger_held_after_secs[199] = Some(2401.0);
        let marks = r.marks();
        assert!(!marks.creator_copies);
        assert_eq!(marks.strangers_most_within_15_min, Some(true));
        assert_eq!(marks.strangers_all_within_30_min, Some(false));
        r.stranger_held_after_secs[1] = Some(1501.0);
        assert_eq!(r.marks().strangers_most_within_15_min, Some(false));
        // Without a reshare, no stranger could start.
        r.reshared_after_secs = None;
        assert_eq!(r.marks().strangers_all_within_30_min, Some(false));
    }

    #[test]
    fn followers_dropping_out_are_spread_through_the_order() {
        let dropping = |n, f| (0..f).filter(|i| drops_out(*i, n, f)).collect::<Vec<_>>();
        assert_eq!(dropping(3, 12), [3, 7, 11]);
        assert_eq!(dropping(60, 300).len(), 60);
        assert_eq!(dropping(60, 300)[..3], [4, 9, 14]);
        assert_eq!(dropping(2, 7), [3, 6]);
        assert_eq!(dropping(0, 300), [] as [usize; 0]);
        // The resharer, the first, stays unless everyone goes.
        assert!(!(1..300).any(|n| drops_out(RESHARER, n, 300)));

        // Only those that stayed are counted.
        let mut r = report(4, 0);
        r.dropped_out = vec![3];
        r.held_after_secs[3] = None;
        assert_eq!((r.holding(), r.time_until(100)), (3, Some(3.0)));
    }

    fn network() -> Network {
        Network {
            creator_down_mbps: 100,
            creator_delay_ms: 20,
            creator_loss_percent: 0.0,
            down_mbps: vec![50, 100],
            delay_ms: vec![10, 40],
            loss_percent: vec![0.0, 0.5],
            caps: false,
            flaky: 0,
            outage_secs: 30,
            outage_every_secs: 300,
            seed: 1,
        }
    }

    #[test]
    fn flaky_followers_are_spread_through_the_order() {
        let flaky = |n, f| (0..f).filter(|i| is_flaky(*i, n, f)).collect::<Vec<_>>();
        assert_eq!(flaky(3, 12), [1, 5, 9]);
        assert_eq!(flaky(0, 300), [] as [usize; 0]);
        for (n, f) in [(3, 12), (30, 300), (60, 300), (100, 300), (149, 300)] {
            let picked = flaky(n, f);
            assert_eq!(picked.len(), n);
            assert!(!picked.contains(&RESHARER));
            // Groups of 3 or more: none of them is one that stops.
            assert!(picked.iter().all(|i| !drops_out(*i, n, f)));
        }
    }

    #[test]
    fn flaky_followers_take_turns_being_cut_off() {
        let n = Network {
            flaky: 2,
            ..network()
        };
        let at = |secs, k| cut_off_at(Duration::from_secs(secs), k, 2, &n);
        // Every 300 s, for 30 s: the first from a third of the way in, the
        // second from two thirds.
        assert!(!at(0, 0) && !at(99, 0) && at(100, 0) && at(129, 0) && !at(130, 0));
        assert!(!at(199, 1) && at(200, 1) && at(229, 1) && !at(230, 1));
        assert!(!at(150, 0) && !at(150, 1));
        assert!(at(400, 0) && !at(430, 0) && at(500, 1) && !at(530, 1));
    }

    #[test]
    fn each_node_gets_its_connection_in_turn() {
        let s = Scenario {
            dir: PathBuf::new(),
            followers: 4,
            video_bytes: 0,
            creator_mbps: 20,
            follower_mbps: vec![5, 10, 20],
            network: Some(network()),
            swarm: true,
            prefetch_spread: Duration::ZERO,
            download_sources: 3,
            drop_out: 0,
            drop_out_after: Duration::ZERO,
            strangers: 0,
            latecomers: 0,
            latecomers_after: Duration::ZERO,
            time_limit: Duration::ZERO,
            sample: Duration::ZERO,
            label: String::new(),
        };
        let net = SimNet::new(1);
        let link = |i| s.follower_link(Some(&net), i).unwrap().link;
        assert_eq!(
            link(3),
            Link {
                up_bps: 5 * MBPS,
                down_bps: 100 * MBPS,
                delay: Duration::from_millis(40),
                loss: 0.005,
            }
        );
        assert_eq!(link(4).up_bps, 10 * MBPS);
        assert_eq!(link(4).delay, Duration::from_millis(10));
        let creator = s.creator_link(Some(&net)).unwrap().link;
        assert_eq!(
            (creator.up_bps, creator.down_bps, creator.loss),
            (20 * MBPS, 100 * MBPS, 0.0)
        );
        // No upload caps unless asked for; on loopback, always.
        assert_eq!(s.upload_cap(5), 0);
        let mut capped = s.clone();
        capped.network.as_mut().unwrap().caps = true;
        assert_eq!(capped.upload_cap(5), 5);
        let loopback = Scenario {
            network: None,
            ..s.clone()
        };
        assert_eq!(loopback.upload_cap(5), 5);
        assert!(loopback.follower_link(Some(&net), 0).is_none());
    }

    #[test]
    fn a_tally_adds_up_each_nodes_record() {
        let creator = osp_proto::SecretKey::generate().public();
        let other = osp_proto::SecretKey::generate().public();
        let record = |node, sent_bytes, received_bytes| PeerRecord {
            node,
            actor: None,
            name: None,
            followed: false,
            follower: false,
            host: false,
            contacted: true,
            first_contact_secs_ago: 0,
            last_contact_secs_ago: 0,
            counts: osp_proto::rpc::PeerCounts {
                sent_bytes,
                received_bytes,
                completed: 1.0,
                busy: 2.0,
                not_hosting: 3.0,
                bad_data: 4.0,
                failed: 5.0,
                unreached: 6.0,
                handed_on: 7.0,
                ..Default::default()
            },
        };
        let t = tally(
            &[
                vec![record(creator, 10.0, 100.0), record(other, 1.0, 1000.0)],
                vec![record(other, 5.0, 0.0)],
            ],
            creator,
        );
        assert_eq!(t.sent, [11.0, 5.0]);
        assert!(sent(&[]).is_sign_positive(), "0, not -0");
        assert_eq!(t.from_creator, 100.0);
        let d = t.downloads;
        assert_eq!(
            [
                d.completed,
                d.busy,
                d.not_hosting,
                d.bad_data,
                d.failed,
                d.unreached,
                d.handed_on
            ],
            [3.0, 6.0, 9.0, 12.0, 15.0, 18.0, 21.0]
        );
    }

    #[test]
    fn what_came_from_where_is_added_up_by_speed() {
        let key = || osp_proto::SecretKey::generate().public();
        let (creator, slow1, slow2, fast, stranger) = (key(), key(), key(), key(), key());
        let gone = key();
        let hosts = HashMap::from([
            (creator, Host::creator(1)),
            (slow1, Host::node("F1".into(), 1)),
            (slow2, Host::node("F2".into(), 1)),
            (fast, Host::node("F3".into(), 10)),
            (gone, Host::node("F4".into(), 1)),
        ]);
        let record = |node, received_bytes, receive_ms, failed| PeerRecord {
            node,
            actor: None,
            name: None,
            followed: false,
            follower: false,
            host: false,
            contacted: true,
            first_contact_secs_ago: 0,
            last_contact_secs_ago: 0,
            counts: osp_proto::rpc::PeerCounts {
                received_bytes,
                receive_ms,
                completed: f64::from(received_bytes > 0.0),
                failed,
                ..Default::default()
            },
        };
        let records = [
            vec![
                record(slow1, 1e6, 8000.0, 0.0),
                record(fast, 4e6, 4000.0, 0.0),
                record(creator, 1e6, 1000.0, 0.0),
                // Asked, but never downloaded from: left out.
                record(stranger, 0.0, 0.0, 0.0),
            ],
            vec![
                record(slow1, 1e6, 8000.0, 0.0),
                record(slow2, 0.0, 0.0, 1.0),
                {
                    let mut gone = record(gone, 0.0, 0.0, 0.0);
                    gone.counts.unreached = 2.0;
                    gone
                },
            ],
        ];

        let one = received(&records[0], &hosts);
        let names: Vec<&str> = one.iter().map(|r| r.from.as_str()).collect();
        assert_eq!(names, ["F3", "F1", "creator"], "most bytes first");
        assert_eq!(one[0].mbps(), 8.0);
        assert_eq!(one[1].mbps(), 1.0);

        // A node that was never reached is listed.
        let two = received(&records[1], &hosts);
        let names: Vec<&str> = two.iter().map(|r| r.from.as_str()).collect();
        assert_eq!(names, ["F1", "F2", "F4"]);
        assert!(two[2].line(false).ends_with("couldn't connect 2 times"));

        let speeds = by_speed(&records, &hosts);
        let names: Vec<&str> = speeds.iter().map(|r| r.from.as_str()).collect();
        assert_eq!(names, ["1 Mbit/s nodes", "10 Mbit/s nodes", "creator"]);
        let slow = &speeds[0];
        assert_eq!((slow.bytes, slow.ms, slow.failed), (2e6, 16000.0, 1.0));
        assert_eq!(slow.unreached, 2.0);
        assert_eq!(slow.nodes, 1, "the node that only failed sent nothing");
        assert_eq!(slow.mbps(), 1.0);
        assert_eq!(speeds[2].bytes, 1e6);
        assert_eq!(Received::default().mbps(), 0.0, "nothing came, not NaN");
    }

    #[test]
    fn places_are_grouped_by_upload_speed() {
        let node = |places, measured_mbps| {
            Some(Serving {
                places,
                measured_mbps,
            })
        };
        // In turn: 1, 10, 1, 10, 1. The last stopped.
        let serving = [
            node(2, Some(0.9)),
            node(6, Some(9.5)),
            node(2, Some(0.7)),
            node(8, Some(12.4)),
            None,
        ];
        assert_eq!(
            places_by_speed(&[1, 10], &serving),
            [
                "1 Mbit/s: places 2 (2), measured 0.9 Mbit/s (0.7 to 0.9)",
                "10 Mbit/s: places 6 (1), 8 (1), measured 12.4 Mbit/s (9.5 to 12.4)",
            ]
        );
        let serving = [node(2, None), node(2, None), node(2, Some(0.8))];
        assert_eq!(
            places_by_speed(&[1, 10], &serving),
            [
                "1 Mbit/s: places 2 (2), measured 0.8 Mbit/s (0.8 to 0.8), 1 not measured",
                "10 Mbit/s: places 2 (1), not measured",
            ]
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn the_machine_load_counts_this_process() {
        let mut load = Load::start();
        let busy_until = Instant::now() + Duration::from_millis(300);
        let mut n = 0u64;
        while Instant::now() < busy_until {
            n = std::hint::black_box(n.wrapping_add(1));
        }
        load.sample();
        let m = load.machine.expect("readable on Linux");
        assert!(m.cpus >= 1);
        let (run, others) = (m.run_percent[0], m.others_percent[0]);
        assert!(run > 0.0, "{run}");
        assert!(others >= 0.0 && run + others <= 100.0, "{run} {others}");
    }
}
