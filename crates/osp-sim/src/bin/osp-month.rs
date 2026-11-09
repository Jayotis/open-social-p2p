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
    /// against the runs measured on real nodes, judged by each figure's
    /// median over the seeds.
    Calibrate {
        /// The first seed.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// How many seeds, one after another from `--seed`.
        #[arg(long, default_value_t = 5)]
        seeds: u64,
        /// Also write the figures here, as JSON.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn time(t: Option<f64>) -> String {
    t.map_or("-".into(), |t| format!("{t:.0}"))
}

fn verdict(mark: Option<bool>) -> &'static str {
    mark.map_or("-", |m| if m { "met" } else { "missed" })
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Calibrate { seed, seeds, out } => {
            let cases: Vec<Vec<calibrate::Run>> = [0, 20, 50, 100]
                .into_iter()
                .map(|percent| {
                    (seed..seed + seeds.max(1))
                        .map(|s| calibrate::run(percent, s))
                        .collect()
                })
                .collect();
            println!(
                "seed | behind relay | 50% / 95% / all (s) | creator copies | relay copies | extra received | busy | failed | latecomers half/all (s) | mark 0"
            );
            for (i, s) in (seed..seed + seeds.max(1)).enumerate() {
                for case in &cases {
                    let r = &case[i];
                    let mut late = r.latecomers.clone();
                    late.sort_by(f64::total_cmp);
                    let half = late.get(late.len().saturating_sub(1) / 2).copied();
                    println!(
                        "{s:>4} | {:>11}% | {} / {} / {} | {:.1} | {:.1} | {:.1}% | {} | {} | {} / {} | {}",
                        r.percent,
                        time(r.p50),
                        time(r.p95),
                        time(r.all),
                        r.creator_copies,
                        r.relay_copies,
                        100.0 * r.followers_extra,
                        r.busy,
                        r.failed,
                        time(half),
                        time(late.last().copied()),
                        verdict(r.mark),
                    );
                }
            }
            let medians: Vec<_> = cases.iter().map(|c| calibrate::medians(c)).collect();
            println!("\nmedians, as mark 0 judges them:");
            println!("behind relay | 50% / 95% / all (s) | creator copies | relay copies | mark 0");
            for m in &medians {
                println!(
                    "{:>11}% | {} / {} / {} | {:.1} | {:.1} | {}",
                    m.percent,
                    time(m.p50),
                    time(m.p95),
                    time(m.all),
                    m.creator_copies,
                    m.relay_copies,
                    verdict(m.mark),
                );
            }
            let met = medians.iter().filter_map(|m| m.mark).all(|m| m);
            println!("mark 0: {}", if met { "met" } else { "missed" });
            if let Some(path) = out {
                let json = serde_json::json!({ "runs": cases, "medians": medians });
                std::fs::write(path, serde_json::to_string_pretty(&json)?)?;
            }
        }
    }
    Ok(())
}
