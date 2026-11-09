//! `osp-month`: the model of three months of posts for ADR 0017's stage A
//! (docs/testing/hops-model.md). It runs no nodes.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use osp_sim::month::{calibrate, months};

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
    /// Three months of posts, one start and one way.
    Run {
        #[arg(long, value_enum)]
        start: months::Start,
        #[arg(long, value_enum)]
        way: months::Way,
        #[arg(long, value_enum, default_value = "accepted")]
        rules: months::RuleSet,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Also write each post's figures here, as JSON.
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
        Command::Run {
            start,
            way,
            rules,
            seed,
            out,
        } => {
            let m = months::run(start, way, rules, seed);
            print_months(&m);
            if let Some(path) = out {
                std::fs::write(path, serde_json::to_string_pretty(&m)?)?;
            }
        }
    }
    Ok(())
}

fn print_months(m: &months::Months) {
    const DAY: f64 = 86_400.0;
    println!(
        "post | day | present | 50% / 95% / all (s) | creator copies | relayed (copies) | off the relay | busy | failed | B8 | direct pairs | day-0 linked | layers 0-4, not Kim | most MB, days | from partners | from operator"
    );
    for p in &m.posts {
        println!(
            "{:>4} | {:>4.1} | {:>3} | {} / {} / {} | {:.1} | {:.1} | {:.0}% | {} | {} | {} | {} | {:.0}% | {:?} | {:.0}, {:.1} | {:.0}% | {:.0}%",
            p.index + 1,
            p.at / DAY,
            p.present,
            time(p.p50),
            time(p.p95),
            time(p.all),
            p.creator_copies,
            p.relayed / osp_sim::month::spread::VIDEO_BYTES,
            100.0 * p.delivered_off_relay / p.delivered.max(1.0),
            p.busy,
            p.failed,
            if p.marks { "met" } else { "missed" },
            p.direct_pairs,
            100.0 * p.day0_linked,
            p.layers,
            p.most_bytes / 1e6,
            p.most_days,
            100.0 * p.from_partners,
            100.0 * p.from_operator,
        );
    }
    // Weeks 5 to 8 and 10 to 13, as marks 1 to 3 compare them.
    for (name, first, last) in [("weeks 5-8", 5, 8), ("weeks 10-13", 10, 13)] {
        let ps: Vec<_> = m
            .posts
            .iter()
            .filter(|p| {
                let week = (p.at / DAY / 7.0) as usize + 1;
                (first..=last).contains(&week)
            })
            .collect();
        let relayed: f64 = ps.iter().map(|p| p.relayed).sum();
        let delivered: f64 = ps.iter().map(|p| p.delivered).sum();
        let off: f64 = ps.iter().map(|p| p.delivered_off_relay).sum();
        let met = ps.iter().filter(|p| p.marks).count();
        println!(
            "{name}: {} posts, B8 met on {met}; relayed {:.1} GB; {:.0}% delivered off the relay",
            ps.len(),
            relayed / 1e9,
            100.0 * off / delivered.max(1.0),
        );
    }
}
