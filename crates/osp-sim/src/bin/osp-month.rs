//! `osp-month`: the model of three months of posts for ADR 0017's stage A
//! (docs/testing/hops-model.md). It runs no nodes.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use osp_sim::month::{calibrate, found, months, sizing};

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
    /// ADR 0025's step 5 (docs/testing/found-over-weeks.md): 1,000
    /// newcomers find 10 creators' videos over 28 days, and some share them
    /// with hosting, at 5, 10 and 20%. The marks are judged at 10%.
    Found {
        /// The first seed; the marks are judged on it.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// How many seeds, one after another from `--seed`, for context.
        #[arg(long, default_value_t = 5)]
        seeds: u64,
        /// Viewers take the hosts in the order given, as before ADR 0025's
        /// amendment, rather than each in its own.
        #[arg(long)]
        same_order: bool,
        /// Also write every run's figures here, as JSON.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// ADR 0043's sizing stage (docs/testing/carrier-sizing.md): every
    /// setting swept, and the ways compared against, on each seed.
    Sizing {
        /// The first seed; the marks are judged on it.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// How many seeds, one after another from `--seed`, for context.
        #[arg(long, default_value_t = 5)]
        seeds: u64,
        /// Runs at once.
        #[arg(long, default_value_t = 6)]
        threads: usize,
        /// Also write every run's figures here, as JSON.
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
        /// How days of work are counted to reach a layer.
        #[arg(long, value_enum, default_value = "window")]
        days: osp_sim::month::records::DayRule,
        /// How layer 4's checked bytes are counted to reach it.
        #[arg(long, value_enum, default_value = "window")]
        bytes: osp_sim::month::records::BytesRule,
        /// When honest machines ask others for their lists.
        #[arg(long, value_enum, default_value = "when-reason")]
        lists: osp_sim::month::meeting::Asking,
        /// Experiment A, B or C, or several (docs/testing/hops-experiments.md).
        #[arg(long, value_enum, default_value = "baseline")]
        experiment: months::Experiment,
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
        Command::Found {
            seed,
            seeds,
            same_order,
            out,
        } => {
            let seeds: Vec<u64> = (seed..seed + seeds.max(1)).collect();
            let mut runs = Vec::new();
            for percent in [5.0, 10.0, 20.0] {
                let rate: Vec<found::Run> = seeds
                    .iter()
                    .map(|&s| found::run(percent, same_order, s))
                    .collect();
                print_found(&rate);
                runs.push(rate);
            }
            if let Some(path) = out {
                std::fs::write(path, serde_json::to_string_pretty(&runs)?)?;
            }
        }
        Command::Sizing {
            seed,
            seeds,
            threads,
            out,
        } => {
            let seeds: Vec<u64> = (seed..seed + seeds.max(1)).collect();
            let report = sizing::run(&seeds, threads);
            print_sizing(&report);
            if let Some(path) = out {
                std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
            }
        }
        Command::Run {
            start,
            way,
            rules,
            days,
            bytes,
            lists,
            experiment,
            seed,
            out,
        } => {
            let m = months::run(
                start, way, rules, days, bytes, lists, experiment, seed, None,
            );
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
    let mut summary = Vec::new();
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
        summary.push(format!(
            "{name} relayed {:.1} GB, {:.1}% off",
            relayed / 1e9,
            100.0 * off / delivered.max(1.0)
        ));
        println!(
            "{name}: {} posts, B8 met on {met}; relayed {:.1} GB; {:.0}% delivered off the relay",
            ps.len(),
            relayed / 1e9,
            100.0 * off / delivered.max(1.0),
        );
    }
    // Mark 4 looks at day 60: the last post before it.
    let day60 = m.posts.iter().rfind(|p| p.at <= 60.0 * DAY);
    let last = m.posts.last();
    summary.push(format!(
        "day-60 linked {:.0}%, direct pairs at day 60 {} and at the end {}",
        day60.map_or(0.0, |p| 100.0 * p.day0_linked),
        day60.map_or(0, |p| p.direct_pairs),
        last.map_or(0, |p| p.direct_pairs),
    ));
    for (name, kind) in [
        ("directory newcomers", months::Kind::NewcomerDirectory),
        ("given newcomers", months::Kind::NewcomerGiven),
    ] {
        let group: Vec<_> = m.newcomers.iter().filter(|n| n.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        let within = |d: Option<f64>, days: f64| d.is_some_and(|d| d <= days);
        let regular21 = group.iter().filter(|n| within(n.regular, 21.0)).count();
        let regular = group.iter().filter(|n| n.regular.is_some()).count();
        let linked = group.iter().filter(|n| n.linked.is_some()).count();
        summary.push(format!(
            "{name}: layer 2 with someone within 21 days {regular21}/{}, by the end {regular}, linked {linked}",
            group.len()
        ));
    }
    if m.way == months::Way::Hops {
        let h = &m.hops;
        let share = h.end_hops_operator as f64 / h.end_hops.max(1) as f64;
        println!(
            "hops: {} transfers through hops, {} with a hop at both ends; {} middle turns; the operator held {:.1}% of end hops (squared {:.2}%), and both ends of {} transfers ({:.2}% of those with both)",
            h.with_hops,
            h.both_ends,
            h.middle_turns,
            100.0 * share,
            100.0 * share * share,
            h.operator_both_ends,
            100.0 * h.operator_both_ends as f64 / h.both_ends.max(1) as f64,
        );
        for (name, kind) in [
            ("directory newcomers", months::Kind::NewcomerDirectory),
            ("given newcomers", months::Kind::NewcomerGiven),
        ] {
            let g: Vec<_> = m
                .newcomers
                .iter()
                .filter(|n| n.kind == kind && n.carries)
                .collect();
            if g.is_empty() {
                continue;
            }
            let within = |d: Option<f64>, days: f64| d.is_some_and(|d| d <= days);
            println!(
                "mark 7, {name} that carry ({}): a middle turn within 7 days {}, standing with a node (jobs on 3 days) within 21 days {}",
                g.len(),
                g.iter().filter(|n| within(n.middle, 7.0)).count(),
                g.iter().filter(|n| within(n.carry_standing, 21.0)).count(),
            );
        }
    }
    println!("summary: {}", summary.join("; "));
    let names = [
        "consent",
        "too few days either way",
        "one way only",
        "bytes",
        "age",
        "caps",
    ];
    let show = |name: &str, b: &months::Blocked| {
        let list = |xs: [usize; 6]| -> String {
            names[1..]
                .iter()
                .zip(&xs[1..])
                .map(|(n, c)| format!("{n} {c}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "  {name}: {} pairs, {} direct; one said no {} (everything else met {}); both yes, not direct {}: first reason {}; only reason {}; any {}",
            b.pairs,
            b.direct,
            b.consent,
            b.consent_only,
            b.both_yes_blocked,
            list(b.first),
            list(b.only),
            list(b.any)
        );
    };
    for r in m.pairs_day60.iter().chain(std::iter::once(&m.pairs_end)) {
        println!("why follower pairs aren't direct, day {:.0}:", r.day);
        show("all that dealt with each other", &r.all);
        show("partners", &r.partners);
    }
    if m.way == months::Way::Hops {
        let a = &m.hops.available;
        let c = &m.hops.available_from_creator;
        println!(
            "transfers not direct, by the hops their ends could pick: both {}, host's only {}, viewer's only {}, neither {} (of those from Kim: {}, {}, {}, {})",
            a[0], a[1], a[2], a[3], c[0], c[1], c[2], c[3]
        );
    }
    for (name, r) in [
        ("honest", &m.reciprocity_honest),
        ("with an operator's node", &m.reciprocity_operator),
    ] {
        println!(
            "partner pairs at the end, {name}: {}; the side serving less served a median {:.0}% of the pair's bytes; under a tenth {}, none {}",
            r.pairs,
            100.0 * r.median_lesser_share,
            r.under_a_tenth,
            r.one_way_only
        );
    }
    let g = &m.meet;
    println!(
        "meeting: largest partner group {} of {} present; lists asked {}, refused {}, given {} ({} to the operator, which asked {}); introductions sent {}, counted {}, of the operator's nodes {}; established that carry hearing of new machines after day 56: {} of {} (directory newcomers {} of {})",
        m.largest_partner_group,
        m.present_end,
        g.lists_asked,
        g.lists_refused,
        g.lists_given,
        g.operator_lists,
        g.operator_lists_asked,
        g.introductions_sent,
        g.introductions_counted,
        g.operator_introduced,
        m.established_heard_new,
        m.established_carrying,
        m.directory_newcomers_heard_new,
        m.directory_newcomers_carrying,
    );
    let k = &m.spot_checks;
    println!(
        "Kim's spot checks (1 KiB each): {} followers at layer 2, {} at layer 3; most checked from one follower {:.1} KiB in 30 days ({:.1} KiB faded) against 100 MB; Kim's links {}",
        k.at_layer2,
        k.at_layer3,
        k.most_window_bytes / 1024.0,
        k.most_faded_bytes / 1024.0,
        k.links
    );
}

fn pct(x: Option<f64>) -> String {
    x.map_or("-".into(), |x| format!("{:.0}%", 100.0 * x))
}

fn copies(x: Option<f64>) -> String {
    x.map_or("-".into(), |x| format!("{x:.3}"))
}

/// One sharing rate: each video on the first seed, then the marks, with how
/// many seeds meet each.
fn print_found(runs: &[found::Run]) {
    let first = &runs[0];
    println!(
        "\nsharing with hosting {}%, seed {}, {}:",
        first.share_percent,
        first.seed,
        if first.same_order {
            "every viewer in the order given"
        } else {
            "each viewer in its own order"
        }
    );
    println!(
        "video | viewers | sharers | from resharers, first 50 / after | creator copies per viewer, first 50 / after | creator copies | top resharer / top 3 | open median / slowest (s) | failed"
    );
    for v in &first.videos {
        println!(
            "{:>5} | {:>7} | {:>7} | {} / {} | {} / {} | {:.1} | {} / {} | {} / {} | {}",
            v.rank,
            v.viewers,
            v.sharers,
            pct(Some(v.from_resharers_first)),
            pct(v.from_resharers_after),
            copies(Some(v.creator_per_viewer_first)),
            copies(v.creator_per_viewer_after),
            v.creator_copies,
            pct(Some(v.top_resharer)),
            pct(Some(v.top_three)),
            time(v.open_median),
            time(v.open_slowest),
            v.failed,
        );
    }
    println!(
        "fair share (recorded): video | first 3 listed, served of fair | 4th-16th listed, served of fair | listed after 16th: sharers, share served"
    );
    for v in &first.videos {
        println!(
            "{:>5} | {} | {} | {}, {}",
            v.rank,
            pct(v.first_of_fair),
            pct(v.late_of_fair),
            v.beyond_list,
            pct(Some(v.beyond_list_served)),
        );
    }
    let count = |f: fn(&found::Marks) -> bool| runs.iter().filter(|r| f(&r.marks)).count();
    let judged = first.share_percent == 10.0;
    let say = |m: bool| {
        if !judged {
            "recorded"
        } else if m {
            "met"
        } else {
            "missed"
        }
    };
    let n = runs.len();
    let m = &first.marks;
    println!(
        "mark 1, resharers carry: {} ({} of {n} seeds); mark 2, creator's cost falls: {} ({} of {n}); mark 3, everyone watches: {} ({} of {n})",
        say(m.resharers_carry),
        count(|m| m.resharers_carry),
        say(m.creator_cost_falls),
        count(|m| m.creator_cost_falls),
        say(m.everyone_watches),
        count(|m| m.everyone_watches),
    );
    println!(
        "one resharer served over half a video's bytes: {} ({} of {n} seeds)",
        if m.one_resharer_most { "yes" } else { "no" },
        count(|m| m.one_resharer_most),
    );
}

fn print_sizing(r: &sizing::Report) {
    let first = |v: &[sizing::Figures]| v[0].clone();
    println!("compared against (seed {}):", r.seeds[0]);
    for (name, runs) in [("project relay", &r.project), ("n0", &r.n0)] {
        for (n, v) in runs {
            let f = first(v);
            let late_p95: Vec<f64> = f.p95.iter().flatten().copied().collect();
            println!(
                "  {name}, {n} seed carriers idle: weeks 10-13 B8 met on {} of {}, relayed {:.0} copies; 95% of followers in a median {:.0} s; busiest relay second {:.0} Mbit/s",
                f.late_b8,
                f.late_posts,
                sizing::copies(f.late_relayed),
                {
                    let mut x = late_p95.clone();
                    x.sort_by(f64::total_cmp);
                    x.get(x.len() / 2).copied().unwrap_or(f64::NAN)
                },
                f.relay_peak_mbps,
            );
        }
    }
    println!(
        "\nseed carriers | share | jobs | newcomer rate | 1 speed: no later / worst / B8 late | 2 relayed vs project | 3 newcomers dir (mid7, st28) / given | 4 operator both vs 2p² | all [seeds meeting all]"
    );
    for (s, runs) in &r.carriers {
        let (f, m) = &runs[0];
        let all = runs.iter().filter(|(_, m)| m.all()).count();
        let d = &f.directory_newcomers;
        let g = &f.given_newcomers;
        let p = f.operator_end_share;
        println!(
            "{} | {:.0}% | {} | {:.0}% | {} {}/{} {:.2}x B8 {} | {} {:.0}% | {} {}/{} {}/{} / {}/{} {}/{} | {} {:.1}% vs {:.1}% | {} [{}/{}]",
            s.seeds,
            100.0 * s.share,
            s.jobs,
            100.0 * s.newcomer_rate,
            verdict(Some(m.speed)),
            m.no_later,
            m.posts,
            m.worst,
            f.late_b8,
            verdict(Some(m.relay)),
            100.0 * m.relayed_ratio,
            verdict(Some(m.newcomers)),
            d.middle_7,
            d.size,
            d.standing_28,
            d.size,
            g.middle_7,
            g.size,
            g.standing_28,
            g.size,
            verdict(Some(m.operator)),
            100.0 * f.operator_both_share,
            200.0 * p * p,
            verdict(Some(m.all())),
            all,
            runs.len(),
        );
    }
    println!(
        "\nrecorded, seed {}: seed carriers | share | jobs | rate | through carriers late | relay peak Mbit/s | hops 0/1/2/3 | one seed both ends / seeds both | home carried MB a day median, most | seed GB each | circuits from one neighbour home/seed | a minute home/seed | newcomers median days to middle, standing (dir; given) | honest seen by operator",
        r.seeds[0]
    );
    for (s, runs) in &r.carriers {
        let (f, _) = &runs[0];
        let c = &f.carrying;
        let days = |x: Option<f64>| x.map_or("-".into(), |x| format!("{x:.0}"));
        println!(
            "{} | {:.0}% | {} | {:.0}% | {:.0}% | {:.0} | {:?} | {:.0}% / {:.0}% | {:.0}, {:.0} | {} | {}/{} | {}/{} | {}, {}; {}, {} | {} of {}",
            s.seeds,
            100.0 * s.share,
            s.jobs,
            100.0 * s.newcomer_rate,
            100.0 * f.late_through_hops,
            f.relay_peak_mbps,
            f.hops_taken,
            100.0 * f.seed_single_share,
            100.0 * f.seed_both_share,
            c.median_a_day / 1e6,
            c.most_a_day / 1e6,
            c.seed_bytes
                .iter()
                .map(|b| format!("{:.0}", b / 1e9))
                .collect::<Vec<_>>()
                .join("/"),
            c.most_from_neighbour,
            c.seed_most_from_neighbour,
            c.most_a_minute,
            c.seed_most_a_minute,
            days(f.directory_newcomers.median_middle),
            days(f.directory_newcomers.median_standing),
            days(f.given_newcomers.median_middle),
            days(f.given_newcomers.median_standing),
            f.exposed_operator,
            f.honest,
        );
    }
    // The plan's choice: 2 seed carriers, all marks on seed 1, smallest
    // share, then fewest jobs, then lowest rate (settings() is in that order).
    let chosen = r
        .carriers
        .iter()
        .find(|(s, runs)| s.seeds == 2 && runs[0].1.all());
    match chosen {
        Some((s, runs)) => {
            let from = runs
                .iter()
                .map(|(f, _)| f.carrying.most_from_neighbour)
                .max()
                .unwrap_or(0);
            let minute = runs
                .iter()
                .map(|(f, _)| f.carrying.most_a_minute)
                .max()
                .unwrap_or(0);
            println!(
                "\nset: upload share {:.0}%, {} jobs at once, newcomer rate {:.0}%; circuits from one neighbour {} (2 x {from}), new circuits a minute {} (2 x {minute})",
                100.0 * s.share,
                s.jobs,
                100.0 * s.newcomer_rate,
                2 * from,
                2 * minute,
            );
        }
        None => println!("\nno setting with 2 seed carriers meets every mark: nothing is set"),
    }
    let fewest = r
        .carriers
        .iter()
        .filter(|(s, _)| s.share == 0.5 && s.jobs == 4 && s.newcomer_rate == 0.10)
        .filter(|(_, runs)| runs[0].1.speed && runs[0].1.relay)
        .map(|(s, _)| s.seeds)
        .min();
    println!(
        "seed carriers meeting marks 1 and 2 at 50%, 4 jobs, 10%: {}",
        fewest.map_or("none of 1, 2 and 4".into(), |n| format!("fewest {n}"))
    );
}
