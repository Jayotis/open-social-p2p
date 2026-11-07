//! `osp-delete`: a creator deletes a post that went round through two levels
//! of reshares, with some nodes offline (ADR 0019's step 1 gate). See
//! `osp_sim::deleting`.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use osp_sim::deleting::{self, DeleteScenario};

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Directory for the nodes' homes. It must be empty or not exist yet, and
    /// is deleted afterwards.
    #[arg(long)]
    dir: PathBuf,
    #[arg(long, default_value_t = 40)]
    followers: usize,
    #[arg(long, default_value_t = 4)]
    resharers: usize,
    #[arg(long, default_value_t = 8)]
    per_resharer: usize,
    #[arg(long, default_value_t = 2)]
    second_resharers: usize,
    #[arg(long, default_value_t = 4)]
    per_second: usize,
    #[arg(long, default_value_t = 4)]
    watchers: usize,
    #[arg(long, default_value_t = 10.0)]
    offline_percent: f64,
    /// Minutes after the delete, in real time.
    #[arg(long, default_value_t = 5)]
    back_after_mins: u64,
    #[arg(long, default_value_t = 8)]
    daily_check_after_mins: u64,
    #[arg(long, default_value_t = 20)]
    minutes: u64,
    /// Leave out Tor-like delays on messages.
    #[arg(long)]
    no_tor_delay: bool,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Write the full report here, as JSON.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Free text kept with the report, such as the commit tested.
    #[arg(long, default_value = "")]
    label: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "osp_sim=info,osp_node=warn,iroh=error".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    anyhow::ensure!(
        !cli.dir.exists() || cli.dir.read_dir()?.next().is_none(),
        "{} isn't empty",
        cli.dir.display()
    );
    let mins = |m: u64| Duration::from_secs(m * 60);
    let scenario = DeleteScenario {
        dir: cli.dir.clone(),
        followers: cli.followers,
        resharers: cli.resharers,
        per_resharer: cli.per_resharer,
        second_resharers: cli.second_resharers,
        per_second: cli.per_second,
        watchers: cli.watchers,
        offline_percent: cli.offline_percent,
        back_after: mins(cli.back_after_mins),
        daily_check_after: mins(cli.daily_check_after_mins),
        run_for: mins(cli.minutes),
        tor_delay: !cli.no_tor_delay,
        seed: cli.seed,
        label: cli.label,
    };
    let result = deleting::run(&scenario).await;
    if cli.dir.exists() {
        std::fs::remove_dir_all(&cli.dir)
            .with_context(|| format!("removing {}", cli.dir.display()))?;
    }
    let report = result?;
    if let Some(out) = &cli.out {
        std::fs::write(out, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("writing {}", out.display()))?;
    }
    print!("{}", report.summary());
    anyhow::ensure!(report.passed(), "ADR 0019's marks were missed");
    Ok(())
}
