//! `osp-vouch`: replies let in by who vouched, among a few hundred people in
//! communities, with an operator of throwaway identities (ADR 0029's step
//! 4). See `osp_sim::vouching`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use osp_sim::vouching::{self, Plan, VouchScenario};

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Directory for the nodes' homes. It must be empty or not exist yet, and
    /// is deleted afterwards.
    #[arg(long)]
    dir: PathBuf,
    #[arg(long, default_value_t = 10)]
    communities: usize,
    #[arg(long, default_value_t = 30)]
    members: usize,
    #[arg(long, default_value_t = 2)]
    popular_creators: usize,
    #[arg(long, default_value_t = 1)]
    hunters: usize,
    #[arg(long, default_value_t = 5)]
    ordinary_repliers: usize,
    #[arg(long, default_value_t = 8)]
    follows_in: usize,
    #[arg(long, default_value_t = 1)]
    follows_out: usize,
    #[arg(long, default_value_t = 18)]
    popular_own: usize,
    #[arg(long, default_value_t = 6)]
    popular_out: usize,
    #[arg(long, default_value_t = 2)]
    popular_communities: usize,
    #[arg(long, default_value_t = 0.3)]
    ordinary_reshare: f64,
    #[arg(long, default_value_t = 200)]
    throwaways: usize,
    /// Leave out Tor-like delays on messages.
    #[arg(long)]
    no_tor_delay: bool,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Only work out and print the plan; start no nodes.
    #[arg(long)]
    plan: bool,
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
    let scenario = VouchScenario {
        dir: cli.dir.clone(),
        communities: cli.communities,
        members: cli.members,
        popular_creators: cli.popular_creators,
        hunters: cli.hunters,
        ordinary_repliers: cli.ordinary_repliers,
        follows_in: cli.follows_in,
        follows_out: cli.follows_out,
        popular_own: cli.popular_own,
        popular_out: cli.popular_out,
        popular_communities: cli.popular_communities,
        ordinary_reshare: cli.ordinary_reshare,
        throwaways: cli.throwaways,
        tor_delay: !cli.no_tor_delay,
        seed: cli.seed,
        label: cli.label,
    };
    if cli.plan {
        print!("{}", Plan::new(&scenario)?.summary());
        return Ok(());
    }
    anyhow::ensure!(
        !cli.dir.exists() || cli.dir.read_dir()?.next().is_none(),
        "{} isn't empty",
        cli.dir.display()
    );
    let result = vouching::run(&scenario).await;
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
    anyhow::ensure!(report.passed(), "ADR 0029's marks were missed");
    Ok(())
}
