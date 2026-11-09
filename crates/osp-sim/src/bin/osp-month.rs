//! `osp-month`: the model of three months of posts for ADR 0017's stage A
//! (docs/testing/hops-model.md). It runs no nodes.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use osp_sim::month::calibrate;

#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Mark 0: one post with 0, 50 and 100% of followers behind the relay,
    /// against the runs measured on real nodes.
    Calibrate {
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Also write the figures here, as JSON.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Calibrate { seed, out } => {
            let runs: Vec<_> = [0, 20, 50, 100]
                .into_iter()
                .map(|percent| calibrate::run(percent, seed))
                .collect();
            println!("behind relay | 50% / 95% / all (s) | creator copies | relay copies | extra received | busy | failed | latecomers half/all (s) | mark 0");
            for r in &runs {
                let s = |t: Option<f64>| t.map_or("-".into(), |t| format!("{t:.0}"));
                let mut late = r.latecomers.clone();
                late.sort_by(f64::total_cmp);
                let half = late.get(late.len().saturating_sub(1) / 2).copied();
                println!(
                    "{:>11}% | {} / {} / {} | {:.1} | {:.1} | {:.1}% | {} | {} | {} / {} | {}",
                    r.percent,
                    s(r.p50),
                    s(r.p95),
                    s(r.all),
                    r.creator_copies,
                    r.relay_copies,
                    100.0 * r.followers_extra,
                    r.busy,
                    r.failed,
                    s(half),
                    s(late.last().copied()),
                    r.mark.map_or("-", |m| if m { "met" } else { "missed" }),
                );
            }
            if let Some(path) = out {
                std::fs::write(path, serde_json::to_string_pretty(&runs)?)?;
            }
        }
    }
    Ok(())
}
