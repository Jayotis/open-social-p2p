//! The simulated network harness ([B8 test plan]): one creator and many
//! followers, all real nodes in this process, talking over loopback with
//! relays off. Each node's upload is held to a home connection's speed with
//! the existing upload cap. The creator posts one video; the harness
//! measures what the creator sends, and when each follower holds the video.
//!
//! [B8 test plan]: ../../docs/testing/b8-creator-upload.md

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use n0_future::{BufferedStreamExt, StreamExt, join_all, stream};
use osp_node::{Config, Node, SimSettings, config::SyncConfig};
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
    /// Whether followers join the follower swarm (`swarm.share_prefetched`).
    pub swarm: bool,
    /// Followers in the swarm spread their fetches of the video over up to
    /// this long. A real node uses `SimSettings::default().prefetch_spread`.
    pub prefetch_spread: Duration,
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
    /// The run ends when every follower that stays and every stranger holds
    /// the video, or this long after posting.
    pub time_limit: Duration,
    /// How often to note the creator's upload so far.
    pub sample: Duration,
    /// Free text kept with the report, such as the commit tested.
    pub label: String,
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
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Outcomes {
    pub completed: f64,
    pub busy: f64,
    pub not_hosting: f64,
    pub bad_data: f64,
    pub failed: f64,
    /// Times a node couldn't connect to another, to download or to ask.
    pub unreached: f64,
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
    std::fs::create_dir_all(&s.dir)?;
    let started = osp_proto::now_rfc3339();
    let source = make_video(&s.dir, s.video_bytes).await?;

    let lifted = SimSettings {
        new_followers_per_hour: s
            .followers
            .max(SimSettings::default().new_followers_per_hour),
        ..SimSettings::default()
    };
    let creator = start(
        &s.dir.join("creator"),
        "Creator",
        s.creator_mbps,
        false,
        lifted,
    )
    .await?;
    let spread = SimSettings {
        prefetch_spread: s.prefetch_spread,
        // The resharer gathered its followers over time too.
        new_followers_per_hour: s
            .strangers
            .max(SimSettings::default().new_followers_per_hour),
        ..SimSettings::default()
    };
    let followers = start_all(s, "f", "F", s.followers, &spread).await?;
    let strangers = start_all(s, "s", "S", s.strangers, &spread).await?;
    info!(
        followers = followers.len(),
        strangers = strangers.len(),
        "nodes started"
    );

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
        },
    )
    .await?
    else {
        bail!("post failed");
    };
    let posted = Instant::now();
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

    let running = followers.into_iter().flatten().chain(strangers);
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
    })
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
                "{:.0} completed, {:.0} busy, {:.0} not hosting, {:.0} bad data, {:.0} failed; couldn't connect {:.0} times",
                d.completed, d.busy, d.not_hosting, d.bad_data, d.failed, d.unreached,
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
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} followers, {:.1} MB video, creator {} Mbit/s, followers {} Mbit/s, {swarm}",
            self.followers,
            self.video_bytes as f64 / 1e6,
            self.creator_mbps,
            self.follower_mbps
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join("/"),
        );
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

async fn start(home: &Path, name: &str, mbps: u64, swarm: bool, sim: SimSettings) -> Result<Node> {
    let mut config = Config::local_test(name);
    // Poll like a real node; test nodes poll only hourly.
    config.sync.interval_secs = SyncConfig::default().interval_secs;
    config.serving.max_upload_mbps = mbps;
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

/// Media bytes sent to every peer together.
fn sent(peers: &[PeerRecord]) -> f64 {
    total(peers.iter().map(|p| p.counts.sent_bytes))
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

/// Starts `count` nodes at the followers' upload speeds, given in turn, with
/// homes `{dir}0000`, `{dir}0001` and so on.
async fn start_all(
    s: &Scenario,
    dir: &str,
    name: &str,
    count: usize,
    sim: &SimSettings,
) -> Result<Vec<Node>> {
    stream::iter(0..count)
        .map(|i| {
            let home = s.dir.join(format!("{dir}{i:04}"));
            let mbps = s.follower_mbps[i % s.follower_mbps.len()];
            let sim = sim.clone();
            async move { start(&home, &format!("{name}{i}"), mbps, s.swarm, sim).await }
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
            d.unreached += c.unreached;
        }
    }
    t
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
                d.unreached
            ],
            [3.0, 6.0, 9.0, 12.0, 15.0, 18.0]
        );
    }
}
