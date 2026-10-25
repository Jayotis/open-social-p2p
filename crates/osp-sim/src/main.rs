//! `osp-sim`: runs one creator and many followers as real nodes in this
//! process, and measures the creator's upload (docs/testing/b8-creator-upload.md).

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use osp_node::SimSettings;
use osp_sim::Scenario;

/// Runs one creator and many followers as real nodes in this process. The
/// creator posts a video; this measures what the creator sends and when
/// each follower holds the video.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Directory for the nodes' homes, on a real disk: a full run needs about
    /// 16 GB. It must be empty or not exist yet, and is deleted afterwards.
    #[arg(long)]
    dir: PathBuf,
    #[arg(long, default_value_t = 300)]
    followers: usize,
    /// About how big the video is once encoded, in megabytes.
    #[arg(long, default_value_t = 52.3)]
    video_mb: f64,
    /// The creator's upload speed, in megabits per second.
    #[arg(long, default_value_t = 20)]
    creator_mbps: u64,
    /// Followers' upload speeds, in megabits per second, given in turn.
    #[arg(long, value_delimiter = ',', default_values_t = [5, 10, 20])]
    follower_mbps: Vec<u64>,
    /// Followers join the follower swarm: they share what they prefetch.
    #[arg(long)]
    swarm: bool,
    /// With `--swarm`, followers spread their fetches of the video over up
    /// to this many seconds. The default is a real node's.
    #[arg(long)]
    spread_secs: Option<u64>,
    /// Most nodes a follower fetches a file from at once, each sending a
    /// part of it. The default is a real node's; 1 fetches from one node
    /// at a time.
    #[arg(long, default_value_t = SimSettings::default().download_sources)]
    sources: usize,
    /// This many followers stop partway through: the last of each group of
    /// `followers / drop-out`.
    #[arg(long, default_value_t = 0)]
    drop_out: usize,
    /// When they stop, in seconds after posting. By default, when about 40%
    /// held the video in the B8 swarm run, and the creator had sent all it
    /// would.
    #[arg(long, default_value_t = 480)]
    drop_out_secs: u64,
    /// Also run this many nodes that follow only the first follower, which
    /// reshares the post, hosting its media, as soon as it holds the video.
    #[arg(long, default_value_t = 0)]
    strangers: usize,
    /// Then this many nodes follow the creator, one after another, each
    /// fetching the video at once: how long each takes shows how fast a
    /// download from the swarm is.
    #[arg(long, default_value_t = 0)]
    latecomers: usize,
    /// End this many minutes after posting, if not every follower and
    /// stranger holds the video by then.
    #[arg(long, default_value_t = 60)]
    minutes: u64,
    /// Write the full report here, as JSON.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Free text kept with the report, such as the commit tested.
    #[arg(long, default_value = "")]
    label: String,
    /// Keep the nodes' homes afterwards.
    #[arg(long)]
    keep: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "osp_sim=info,osp_node=warn,iroh=error,iroh_blobs=error".into()
            }),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    // Checked here too, before anything can delete the directory.
    anyhow::ensure!(
        !cli.dir.exists() || cli.dir.read_dir()?.next().is_none(),
        "{} isn't empty",
        cli.dir.display()
    );
    let scenario = Scenario {
        dir: cli.dir.clone(),
        followers: cli.followers,
        video_bytes: (cli.video_mb * 1e6) as u64,
        creator_mbps: cli.creator_mbps,
        follower_mbps: cli.follower_mbps,
        swarm: cli.swarm,
        prefetch_spread: cli.spread_secs.map_or_else(
            || SimSettings::default().prefetch_spread,
            Duration::from_secs,
        ),
        download_sources: cli.sources,
        drop_out: cli.drop_out,
        drop_out_after: Duration::from_secs(cli.drop_out_secs),
        strangers: cli.strangers,
        latecomers: cli.latecomers,
        time_limit: Duration::from_secs(cli.minutes * 60),
        sample: Duration::from_secs(60),
        label: cli.label,
    };
    let result = osp_sim::run(&scenario).await;
    if !cli.keep && cli.dir.exists() {
        std::fs::remove_dir_all(&cli.dir)
            .with_context(|| format!("removing {}", cli.dir.display()))?;
    }
    let report = result?;
    if let Some(out) = &cli.out {
        std::fs::write(out, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("writing {}", out.display()))?;
    }
    print!("{}", report.summary());
    Ok(())
}
