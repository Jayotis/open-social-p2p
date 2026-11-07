//! Deleting a post, for ADR 0019's step 1 gate: a creator, its followers,
//! and two levels of reshares whose followers don't follow the creator. When
//! the creator deletes the post, some nodes are offline, one resharer no
//! longer follows the creator, and some followers also follow a resharer
//! that's offline, whose stale reshare they read when it comes back.
//! Messages take Tor-like delays (`TorDelay`); every node sits on a fast
//! link of the simulated network, which is how one goes offline.
//!
//! Measured: how long each node takes to hold neither the post nor its file,
//! from the delete for those online, and from coming back for those offline;
//! and whether any node that had dropped it shows it again.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_node::{
    Config, Node, SimSettings,
    config::SyncConfig,
    simnet::{Link, SimNet, TorDelay},
};
use osp_proto::{
    BlobHash, ObjectId,
    rpc::{ClientRequest, ClientResponse},
};
use serde::Serialize;
use tracing::info;

/// Nodes started, and synced, at once.
const AT_ONCE: usize = 16;
/// How often the harness looks at every node after the delete.
const TICK: Duration = Duration::from_secs(5);
/// Longest to wait for every node to hold the photo before the delete.
const SPREAD_TIMEOUT: Duration = Duration::from_secs(900);
/// ADR 0019's first mark: online nodes drop it within this.
const ONLINE_MARK: Duration = Duration::from_secs(15 * 60);
const MBPS: u64 = 1_000_000;

#[derive(Debug, Clone, Serialize)]
pub struct DeleteScenario {
    pub dir: PathBuf,
    /// Nodes following the creator.
    pub followers: usize,
    /// Of them, those that reshare the post, hosting it. The first is offline
    /// at the delete, and the second no longer follows the creator.
    pub resharers: usize,
    /// Nodes following each resharer, and not the creator.
    pub per_resharer: usize,
    /// Of each resharer's followers, those that reshare it again.
    pub second_resharers: usize,
    /// Nodes following each of those, and nobody else.
    pub per_second: usize,
    /// Nodes following the creator and the first resharer, which is offline
    /// at the delete: when it comes back, they read its stale reshare.
    pub watchers: usize,
    /// Of everyone else, but the creator and the first two resharers, this
    /// share is offline at the delete.
    pub offline_percent: f64,
    /// In real time, after the delete.
    pub back_after: Duration,
    /// When the resharer that no longer follows the creator asks about its
    /// reshares, as it does once a day.
    pub daily_check_after: Duration,
    pub run_for: Duration,
    pub tor_delay: bool,
    pub seed: u64,
    pub label: String,
}

/// Where a node sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Group {
    Follower,
    Resharer,
    ResharersFollower,
    SecondResharer,
    SecondResharersFollower,
    Watcher,
    /// The resharer that no longer follows the creator, and those below it:
    /// they hear only once it asks.
    AfterTheDailyCheck,
    /// Below the resharer that's offline: they hear once it's back.
    BelowOfflineResharer,
}

impl Group {
    fn name(self) -> &'static str {
        match self {
            Group::Follower => "followers",
            Group::Resharer => "resharers",
            Group::ResharersFollower => "resharers' followers",
            Group::SecondResharer => "second resharers",
            Group::SecondResharersFollower => "their followers",
            Group::Watcher => "watchers of a stale reshare",
            Group::AfterTheDailyCheck => "after the daily check",
            Group::BelowOfflineResharer => "below an offline resharer",
        }
    }

    /// Whether it hears of the delete straight away, or once it's back.
    fn hears_at_once(self) -> bool {
        !matches!(
            self,
            Group::AfterTheDailyCheck | Group::BelowOfflineResharer
        )
    }

    const ALL: [Group; 8] = [
        Group::Follower,
        Group::Resharer,
        Group::ResharersFollower,
        Group::SecondResharer,
        Group::SecondResharersFollower,
        Group::Watcher,
        Group::AfterTheDailyCheck,
        Group::BelowOfflineResharer,
    ];
}

/// One node, after the run.
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub group: Group,
    pub offline: bool,
    /// Seconds from the delete, or from coming back for one offline, until it
    /// held neither the post nor its file; `None` if it still did.
    pub gone_secs: Option<f64>,
    /// Whether it showed the post again after it had dropped it.
    pub came_back: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteReport {
    pub label: String,
    pub started: String,
    pub scenario: DeleteScenario,
    pub nodes: usize,
    /// Seconds from posting until every node held the photo.
    pub spread_secs: f64,
    pub outcomes: Vec<Outcome>,
}

struct Member {
    node: Node,
    group: Group,
    offline: bool,
    /// When it held neither, from the delete or from coming back.
    gone: Option<Duration>,
    came_back: bool,
}

pub async fn run(s: &DeleteScenario) -> Result<DeleteReport> {
    ensure!(s.resharers >= 2, "a run needs two resharers at least");
    ensure!(s.followers >= s.resharers, "resharers are followers too");
    ensure!(
        s.second_resharers <= s.per_resharer,
        "second resharers are among a resharer's followers"
    );
    std::fs::create_dir_all(&s.dir)?;
    let started = osp_proto::now_rfc3339();
    let photo = make_photo(&s.dir).await?;
    let net = SimNet::new(s.seed);
    let sim = SimSettings {
        tor_delay: s.tor_delay.then(TorDelay::default),
        new_followers_per_hour: 10_000,
        ..SimSettings::default()
    };
    // Each node gets a link of its own.
    let start_node = |name: String| {
        let home = s.dir.join(name.to_lowercase());
        let sim = SimSettings {
            network: Some(net.link(Link {
                up_bps: 100 * MBPS,
                down_bps: 100 * MBPS,
                delay: Duration::from_millis(10),
                loss: 0.0,
            })),
            ..sim.clone()
        };
        async move { start(&home, &name, sim).await }
    };
    let start_all = |prefix: String, count: usize| {
        stream::iter(0..count)
            .map(move |i| start_node(format!("{prefix}{i:03}")))
            .buffered_ordered(AT_ONCE)
            .collect::<Vec<_>>()
    };
    let creator = start_node("Creator".into()).await?;
    let followers: Vec<Node> = start_all("F".into(), s.followers)
        .await
        .into_iter()
        .collect::<Result<_>>()?;
    let watchers: Vec<Node> = start_all("W".into(), s.watchers)
        .await
        .into_iter()
        .collect::<Result<_>>()?;
    let mut level1: Vec<Vec<Node>> = Vec::new();
    let mut level2: Vec<Vec<Vec<Node>>> = Vec::new();
    for r in 0..s.resharers {
        let theirs: Vec<Node> = start_all(format!("S{r}-"), s.per_resharer)
            .await
            .into_iter()
            .collect::<Result<_>>()
            .with_context(|| format!("resharer {r}'s followers"))?;
        let mut below = Vec::new();
        for k in 0..s.second_resharers {
            let nodes: Vec<Node> = start_all(format!("T{r}-{k}-"), s.per_second)
                .await
                .into_iter()
                .collect::<Result<_>>()?;
            below.push(nodes);
        }
        level1.push(theirs);
        level2.push(below);
    }
    info!("nodes started");

    // Who follows whom, and the post going round.
    follow_all(&creator, followers.iter().chain(&watchers)).await?;
    let posted = Instant::now();
    let (post, file) = post_photo(&creator, &photo).await?;
    sync_all(followers.iter().chain(&watchers)).await?;
    for resharer in &followers[..s.resharers] {
        reshare(resharer, post).await?;
    }
    follow_all(&followers[0], watchers.iter()).await?;
    for (r, theirs) in level1.iter().enumerate() {
        follow_all(&followers[r], theirs.iter()).await?;
        sync_all(theirs.iter()).await?;
        for (k, below) in level2[r].iter().enumerate() {
            reshare(&theirs[k], post).await?;
            follow_all(&theirs[k], below.iter()).await?;
            sync_all(below.iter()).await?;
        }
    }
    sync_all(watchers.iter()).await?;

    // Everyone in place, then holding the photo.
    let mut members = Vec::new();
    let mut add = |node: Node, group: Group| {
        members.push(Member {
            node,
            group,
            offline: false,
            gone: None,
            came_back: false,
        })
    };
    for (i, node) in followers.into_iter().enumerate() {
        let group = match i {
            1 => Group::AfterTheDailyCheck,
            i if i < s.resharers => Group::Resharer,
            _ => Group::Follower,
        };
        add(node, group);
    }
    for node in watchers {
        add(node, Group::Watcher);
    }
    for (r, (theirs, below)) in level1.into_iter().zip(level2).enumerate() {
        let late = match r {
            0 => Some(Group::BelowOfflineResharer),
            1 => Some(Group::AfterTheDailyCheck),
            _ => None,
        };
        for (k, node) in theirs.into_iter().enumerate() {
            let group = match (late, k < s.second_resharers) {
                (Some(group), _) => group,
                (None, true) => Group::SecondResharer,
                (None, false) => Group::ResharersFollower,
            };
            add(node, group);
        }
        for node in below.into_iter().flatten() {
            add(node, late.unwrap_or(Group::SecondResharersFollower));
        }
    }
    let spread_by = Instant::now() + SPREAD_TIMEOUT;
    for m in &members {
        while m.node.blob_class(&file)?.is_none() {
            ensure!(
                Instant::now() < spread_by,
                "not every node got the photo in time"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    let spread_secs = posted.elapsed().as_secs_f64();
    info!(
        spread_secs,
        nodes = members.len(),
        "every node holds the photo"
    );

    // The second resharer stops following the creator; the first goes
    // offline, with a share of the rest.
    call(
        &members[1].node,
        ClientRequest::Unfollow {
            actor: creator.identity(),
        },
    )
    .await?;
    let every = if s.offline_percent > 0.0 {
        (100.0 / s.offline_percent).round().max(1.0) as usize
    } else {
        usize::MAX
    };
    for (i, m) in members.iter_mut().enumerate() {
        m.offline = i == 0 || (i >= 2 && i % every == 0);
        if m.offline {
            set_online(&net, &m.node, false);
        }
    }
    let offline = members.iter().filter(|m| m.offline).count();
    info!(offline, "nodes offline; deleting");

    let deleted = Instant::now();
    call(&creator, ClientRequest::Delete { id: post }).await?;
    let mut back = None;
    let mut checked = false;
    while deleted.elapsed() < s.run_for {
        if back.is_none() && deleted.elapsed() >= s.back_after {
            for m in members.iter().filter(|m| m.offline) {
                set_online(&net, &m.node, true);
            }
            back = Some(Instant::now());
            info!("offline nodes back");
        }
        if !checked && deleted.elapsed() >= s.daily_check_after {
            let asked = members[1].node.check_reshares_now().await?;
            checked = true;
            info!(
                asked,
                "the resharer that no longer follows asked about its reshares"
            );
        }
        for m in members.iter_mut() {
            let holds = holds(&m.node, post, &file).await?;
            match (holds, m.gone) {
                (false, None) => {
                    let from = if m.offline { back } else { Some(deleted) };
                    if let Some(from) = from {
                        m.gone = Some(from.elapsed());
                    }
                }
                (true, Some(_)) => m.came_back = true,
                _ => {}
            }
        }
        let gone = members.iter().filter(|m| m.gone.is_some()).count();
        info!(
            gone,
            of = members.len(),
            secs = deleted.elapsed().as_secs(),
            "dropped it"
        );
        if gone == members.len() && back.is_some() && checked {
            break;
        }
        tokio::time::sleep(TICK).await;
    }

    let outcomes = members
        .iter()
        .map(|m| Outcome {
            group: m.group,
            offline: m.offline,
            gone_secs: m.gone.map(|d| d.as_secs_f64()),
            came_back: m.came_back,
        })
        .collect();
    let nodes = members.len() + 1;
    for m in members {
        m.node.shutdown().await.ok();
    }
    creator.shutdown().await.ok();
    Ok(DeleteReport {
        label: s.label.clone(),
        started,
        scenario: s.clone(),
        nodes,
        spread_secs,
        outcomes,
    })
}

impl DeleteReport {
    /// Whether ADR 0019's marks were met: 99% of the online nodes that hear
    /// straight away dropped it within 15 minutes; every other node dropped
    /// it in the end; and none showed it again.
    pub fn passed(&self) -> bool {
        let online: Vec<_> = self
            .outcomes
            .iter()
            .filter(|o| !o.offline && o.group.hears_at_once())
            .collect();
        let in_time = online
            .iter()
            .filter(|o| o.gone_secs.is_some_and(|s| s <= ONLINE_MARK.as_secs_f64()))
            .count();
        let enough = in_time * 100 >= online.len() * 99;
        enough
            && self.outcomes.iter().all(|o| o.gone_secs.is_some())
            && !self.outcomes.iter().any(|o| o.came_back)
    }

    pub fn summary(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} nodes ({}); every node held the photo {:.0} s after it was posted",
            self.nodes, self.label, self.spread_secs
        );
        let _ = writeln!(
            out,
            "{:<28} {:>5} {:>8} {:>10} {:>10} {:>8}",
            "", "nodes", "offline", "median s", "slowest s", "not yet"
        );
        for group in Group::ALL {
            for offline in [false, true] {
                let these: Vec<&Outcome> = self
                    .outcomes
                    .iter()
                    .filter(|o| o.group == group && o.offline == offline)
                    .collect();
                if these.is_empty() {
                    continue;
                }
                let mut secs: Vec<f64> = these.iter().filter_map(|o| o.gone_secs).collect();
                secs.sort_by(f64::total_cmp);
                let median = secs.get(secs.len() / 2).copied();
                let slowest = secs.last().copied();
                let not_yet = these.len() - secs.len();
                let shown = |s: Option<f64>| s.map_or("-".into(), |s| format!("{s:.0}"));
                let _ = writeln!(
                    out,
                    "{:<28} {:>5} {:>8} {:>10} {:>10} {:>8}",
                    group.name(),
                    these.len(),
                    if offline { "yes" } else { "" },
                    shown(median),
                    shown(slowest),
                    not_yet
                );
            }
        }
        let came_back = self.outcomes.iter().filter(|o| o.came_back).count();
        let _ = writeln!(out, "showed it again after dropping it: {came_back}");
        let _ = writeln!(
            out,
            "offline nodes count from coming back, the rest from the delete"
        );
        let _ = writeln!(
            out,
            "ADR 0019's marks: {}",
            if self.passed() { "met" } else { "MISSED" }
        );
        out
    }
}

/// Whether `node` holds the post, in its feed or as a reshare, or its file.
async fn holds(node: &Node, post: ObjectId, file: &BlobHash) -> Result<bool> {
    if node.blob_class(file)?.is_some() {
        return Ok(true);
    }
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 50 }).await?
    else {
        bail!("no feed");
    };
    Ok(items.iter().any(|item| item.id == post))
}

async fn make_photo(dir: &Path) -> Result<PathBuf> {
    let out = dir.join("photo.png");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=640x480",
            "-frames:v",
            "1",
        ])
        .arg(&out)
        .status()
        .await
        .context("running ffmpeg")?;
    ensure!(status.success(), "ffmpeg failed making the test photo");
    Ok(out)
}

async fn start(home: &Path, name: &str, sim: SimSettings) -> Result<Node> {
    let mut config = Config::local_test(name);
    // Poll like a real node; test nodes poll only hourly.
    config.sync.interval_secs = SyncConfig::default().interval_secs;
    Node::init(home, &config)?;
    Node::spawn_sim(home, sim).await
}

async fn call(node: &Node, req: ClientRequest) -> Result<ClientResponse> {
    match node.handle(req).await {
        ClientResponse::Error { message } => bail!("{message}"),
        resp => Ok(resp),
    }
}

async fn post_photo(node: &Node, photo: &Path) -> Result<(ObjectId, BlobHash)> {
    let req = ClientRequest::Post {
        content: "a photo posted by mistake".into(),
        attachments: vec![photo.to_path_buf()],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, req).await? else {
        bail!("post failed");
    };
    let ClientResponse::Item { item } = call(node, ClientRequest::Show { id }).await? else {
        bail!("can't show the post");
    };
    let file = item
        .attachments
        .first()
        .context("post has no photo")?
        .attachment
        .blob;
    Ok((id, file))
}

async fn reshare(node: &Node, post: ObjectId) -> Result<()> {
    call(
        node,
        ClientRequest::Reshare {
            id: post,
            host: true,
        },
    )
    .await?;
    Ok(())
}

/// Has every one of `nodes` follow `followed`.
async fn follow_all<'a>(followed: &Node, nodes: impl Iterator<Item = &'a Node>) -> Result<()> {
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
    Ok(())
}

/// Syncs each of `nodes`, a few at once, waiting for what they prefetch.
async fn sync_all<'a>(nodes: impl Iterator<Item = &'a Node>) -> Result<()> {
    let results: Vec<Result<ClientResponse>> = stream::iter(nodes)
        .map(|node| call(node, ClientRequest::Sync))
        .buffered_unordered(AT_ONCE)
        .collect()
        .await;
    for result in results {
        result?;
    }
    Ok(())
}

/// Cuts off or connects again every endpoint of a node.
fn set_online(net: &SimNet, node: &Node, online: bool) {
    for id in node.endpoint_ids() {
        net.set_online(id, online);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small run without Tor's delays meets the marks, so the harness and
    /// what it measures hold together.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_small_run_meets_adr_0019s_marks() {
        let dir = tempfile::tempdir().unwrap();
        let s = DeleteScenario {
            dir: dir.path().join("run"),
            followers: 4,
            resharers: 2,
            per_resharer: 2,
            second_resharers: 1,
            per_second: 1,
            watchers: 1,
            offline_percent: 0.0,
            back_after: Duration::from_secs(5),
            daily_check_after: Duration::from_secs(5),
            run_for: Duration::from_secs(180),
            tor_delay: false,
            seed: 1,
            label: "test".into(),
        };
        let report = run(&s).await.unwrap();
        assert!(report.passed(), "{}", report.summary());
        assert_eq!(report.nodes, 12);
        // The offline resharer, and those below it, heard once it was back.
        assert!(
            report
                .outcomes
                .iter()
                .any(|o| o.offline && o.gone_secs.is_some())
        );
        assert!(
            report
                .outcomes
                .iter()
                .filter(|o| o.group == Group::BelowOfflineResharer)
                .all(|o| o.gone_secs.is_some())
        );
    }
}
