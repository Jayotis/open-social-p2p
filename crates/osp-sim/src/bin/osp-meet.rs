//! `osp-meet`: honest nodes that know only a directory, left alone with
//! Tor-like delays, then the directory off and one operator's names arriving
//! (ADR 0015's step 3, ADR 0016's stage A). See `osp_sim::meeting`.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use osp_sim::meeting::{self, MeetScenario};

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Directory for the nodes' homes. It must be empty or not exist yet, and
    /// is deleted afterwards.
    #[arg(long)]
    dir: PathBuf,
    #[arg(long, default_value_t = 100)]
    honest: usize,
    /// One operator's names, which pass on one another and fakes.
    #[arg(long, default_value_t = 30)]
    hostile: usize,
    /// Honest machines each hostile name greets.
    #[arg(long, default_value_t = 20)]
    hostile_greets: usize,
    /// Times among machines run this many times as fast: at 60, an hour is a
    /// minute. Tor's delays aren't sped up.
    #[arg(long, default_value_t = 60.0)]
    speed: f64,
    /// Hours, in the simulation, before a machine that answers is known.
    #[arg(long, default_value_t = 12.0)]
    known_after_hours: f64,
    /// Minutes, in real time.
    #[arg(long, default_value_t = 15)]
    directory_off_mins: u64,
    #[arg(long, default_value_t = 25)]
    hostile_mins: u64,
    #[arg(long, default_value_t = 60)]
    minutes: u64,
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
    let scenario = MeetScenario {
        dir: cli.dir.clone(),
        honest: cli.honest,
        hostile: cli.hostile,
        hostile_greets: cli.hostile_greets,
        speed: cli.speed,
        known_after_hours: cli.known_after_hours,
        directory_off_after: mins(cli.directory_off_mins),
        hostile_after: mins(cli.hostile_mins),
        run_for: mins(cli.minutes),
        sample: Duration::from_secs(60),
        seed: cli.seed,
        label: cli.label,
    };
    let result = meeting::run(&scenario).await;
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
    Ok(())
}
