//! The simulated network harness ([B8 test plan]): one creator and many
//! followers, all real nodes in this process, talking over loopback with
//! relays off. Each node's upload is held to a home connection's speed with
//! the existing upload cap. The creator posts one video; the harness
//! measures what the creator sends, and when each follower holds the video.
//!
//! [B8 test plan]: ../../docs/testing/b8-creator-upload.md

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use n0_future::{BufferedStreamExt, StreamExt, join_all, stream};
use osp_node::{Config, Node, SimSettings, config::SyncConfig};
use osp_proto::{
    BlobHash, ObjectId,
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
    /// The run ends when every follower holds the video, or this long after
    /// posting.
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
    /// records. It should match `creator_sent_bytes`.
    pub received_from_creator: f64,
    /// The followers' download attempts, from any node, by how they ended.
    pub downloads: Outcomes,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Outcomes {
    pub completed: f64,
    pub busy: f64,
    pub not_hosting: f64,
    pub bad_data: f64,
    pub failed: f64,
}

/// The B8 pass marks: whether this run met each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marks {
    /// The creator sent at most a tenth as many copies as there are followers.
    pub creator_copies: bool,
    /// 95% of followers held the video within 15 minutes.
    pub most_within_15_min: bool,
    /// Every follower held it within 30 minutes.
    pub all_within_30_min: bool,
}

/// Runs a scenario from start to finish, then stops every node.
pub async fn run(s: &Scenario) -> Result<Report> {
    ensure!(s.followers > 0, "a run needs followers");
    ensure!(!s.follower_mbps.is_empty(), "followers need upload speeds");
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
    };
    let creator = start(&s.dir.join("creator"), "Creator", s.creator_mbps, lifted).await?;
    let followers: Vec<Node> = stream::iter(0..s.followers)
        .map(|i| {
            let home = s.dir.join(format!("f{i:04}"));
            let mbps = s.follower_mbps[i % s.follower_mbps.len()];
            async move { start(&home, &format!("F{i}"), mbps, SimSettings::default()).await }
        })
        .buffered_ordered(STARTING_AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_>>()?;
    info!(followers = followers.len(), "nodes started");

    let ClientResponse::Ticket { ticket } = call(&creator, ClientRequest::Invite).await? else {
        bail!("no ticket");
    };
    for follower in &followers {
        call(
            follower,
            ClientRequest::Follow {
                ticket: ticket.clone(),
            },
        )
        .await?;
    }
    wait_for_followers(&creator, s.followers).await?;
    info!("every follower subscribed");

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

    let mut held: Vec<Option<f64>> = vec![None; followers.len()];
    let mut so_far = Vec::new();
    loop {
        let checks = join_all(
            followers
                .iter()
                .zip(&held)
                .filter(|(_, held)| held.is_none())
                .map(|(node, _)| holds(node, post, video)),
        )
        .await;
        let now = posted.elapsed();
        let waiting = held.iter_mut().filter(|h| h.is_none());
        for (slot, check) in waiting.zip(checks) {
            if check? {
                *slot = Some(now.as_secs_f64());
            }
        }
        let periods = (now.as_secs_f64() / s.sample.as_secs_f64()) as usize;
        if so_far.len() < periods {
            let sent = sent(&peers(&creator).await?);
            so_far.resize(periods, sent);
            let holding = held.iter().flatten().count();
            info!(
                secs = now.as_secs(),
                holding,
                creator_copies = format!("{:.1}", sent / media_bytes as f64),
                "progress"
            );
        }
        if held.iter().all(Option::is_some) || now >= s.time_limit {
            break;
        }
        tokio::time::sleep(TICK).await;
    }
    let ended_secs = posted.elapsed().as_secs_f64();
    tokio::time::sleep(SETTLE).await;

    let creator_seen = peers(&creator).await?;
    let mut follower_sent_bytes = Vec::new();
    let mut received_from_creator = 0.0;
    let mut downloads = Outcomes::default();
    for follower in &followers {
        let seen = peers(follower).await?;
        follower_sent_bytes.push(sent(&seen));
        for p in &seen {
            let c = &p.counts;
            if p.node == creator.node_id() {
                received_from_creator += c.received_bytes;
            }
            downloads.completed += c.completed;
            downloads.busy += c.busy;
            downloads.not_hosting += c.not_hosting;
            downloads.bad_data += c.bad_data;
            downloads.failed += c.failed;
        }
    }

    for stopped in join_all(followers.into_iter().map(Node::shutdown)).await {
        stopped?;
    }
    creator.shutdown().await?;

    Ok(Report {
        label: s.label.clone(),
        started,
        followers: s.followers,
        creator_mbps: s.creator_mbps,
        follower_mbps: s.follower_mbps.clone(),
        time_limit_secs: s.time_limit.as_secs(),
        video_bytes,
        media_bytes,
        held_after_secs: held,
        ended_secs,
        creator_sent_bytes: sent(&creator_seen),
        sample_secs: s.sample.as_secs_f64(),
        creator_sent_so_far: so_far,
        creator_refused_busy: creator_seen.iter().map(|p| p.counts.refused_busy).sum(),
        follower_sent_bytes,
        received_from_creator,
        downloads,
    })
}

impl Report {
    /// How many followers held the video by the end.
    pub fn holding(&self) -> usize {
        self.held_after_secs.iter().flatten().count()
    }

    /// When `percent` of the followers held the video, if they ever did.
    pub fn time_until(&self, percent: usize) -> Option<f64> {
        let needed = (self.followers * percent).div_ceil(100).max(1);
        let mut times: Vec<f64> = self.held_after_secs.iter().flatten().copied().collect();
        times.sort_by(f64::total_cmp);
        times.get(needed - 1).copied()
    }

    /// The creator's upload, in copies of the post's media.
    pub fn creator_copies(&self) -> f64 {
        self.creator_sent_bytes / self.media_bytes as f64
    }

    pub fn marks(&self) -> Marks {
        let within = |percent, secs| self.time_until(percent).is_some_and(|t| t <= secs);
        Marks {
            creator_copies: self.creator_copies() <= self.followers as f64 / 10.0,
            most_within_15_min: within(95, 15.0 * 60.0),
            all_within_30_min: within(100, 30.0 * 60.0),
        }
    }

    /// A few lines for people to read.
    pub fn summary(&self) -> String {
        let copies = |bytes: f64| bytes / self.media_bytes as f64;
        let time = |percent| match self.time_until(percent) {
            Some(t) => format!("{t:.0} s"),
            None => "not reached".to_string(),
        };
        let mut sent = self.follower_sent_bytes.clone();
        sent.sort_by(f64::total_cmp);
        let most = sent.last().copied().unwrap_or(0.0);
        let median = sent.get(sent.len() / 2).copied().unwrap_or(0.0);
        let none = sent.iter().filter(|b| **b < 1.0).count();
        let d = &self.downloads;
        let marks = self.marks();
        let yes = |ok| if ok { "met" } else { "missed" };

        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} followers, {:.1} MB video, creator {} Mbit/s, followers {} Mbit/s",
            self.followers,
            self.video_bytes as f64 / 1e6,
            self.creator_mbps,
            self.follower_mbps
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join("/"),
        );
        let _ = writeln!(
            out,
            "held the video: {} of {} when the run ended at {:.0} s; 50% at {}, 95% at {}, all at {}",
            self.holding(),
            self.followers,
            self.ended_secs,
            time(50),
            time(95),
            time(100),
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
            "followers sent: most {:.1} copies, median {:.1}; {} sent nothing",
            copies(most),
            copies(median),
            none,
        );
        let _ = writeln!(
            out,
            "followers' downloads: {:.0} completed, {:.0} busy, {:.0} not hosting, {:.0} bad data, {:.0} failed",
            d.completed, d.busy, d.not_hosting, d.bad_data, d.failed,
        );
        let _ = writeln!(
            out,
            "pass marks: creator at most {:.0} copies {}; 95% within 15 min {}; all within 30 min {}",
            self.followers as f64 / 10.0,
            yes(marks.creator_copies),
            yes(marks.most_within_15_min),
            yes(marks.all_within_30_min),
        );
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

async fn start(home: &Path, name: &str, mbps: u64, sim: SimSettings) -> Result<Node> {
    let mut config = Config::local_test(name);
    // Poll like a real node; test nodes poll only hourly.
    config.sync.interval_secs = SyncConfig::default().interval_secs;
    config.serving.max_upload_mbps = mbps;
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
    peers.iter().map(|p| p.counts.sent_bytes).sum()
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
}
