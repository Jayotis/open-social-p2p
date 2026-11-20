//! ADR 0043's sizing stage ([plan]): way 4 with every node carrying, seed
//! carriers and the project's relays, over the settings swept, against
//! every transfer through the project's relays and through n0's.
//!
//! [plan]: ../../../docs/testing/carrier-sizing.md

use serde::Serialize;

use super::meeting::Asking;
use super::months::{self, Carrying, Kind, Months, RuleSet, Sizing, Start, Way};
use super::records::{BytesRule, DAY, DayRule};
use super::spread::VIDEO_BYTES;

/// The settings swept, in the plan's order: 2 seed carriers over every
/// upload share, jobs at once and newcomer rate, then 1 and 4 seed carriers
/// at 50%, 4 jobs and 10%.
pub fn settings() -> Vec<Sizing> {
    let mut v = Vec::new();
    for share in [0.25, 0.5] {
        for jobs in [2, 4] {
            for newcomer_rate in [0.05, 0.10, 0.20] {
                v.push(Sizing {
                    share,
                    jobs,
                    newcomer_rate,
                    seeds: 2,
                });
            }
        }
    }
    for seeds in [1, 4] {
        v.push(Sizing {
            share: 0.5,
            jobs: 4,
            newcomer_rate: 0.10,
            seeds,
        });
    }
    v
}

/// One run's figures, as the plan reads them.
#[derive(Clone, Debug, Serialize)]
pub struct Figures {
    pub seed: u64,
    pub way: Way,
    pub sizing: Sizing,
    /// Each post's time for 95% of followers, in seconds.
    pub p95: Vec<Option<f64>>,
    /// Weeks 10 to 13: posts, posts meeting B8's marks, bytes through the
    /// relay, and the share of bytes delivered through carriers.
    pub late_posts: usize,
    pub late_b8: usize,
    pub late_relayed: f64,
    pub late_through_hops: f64,
    /// The busiest second through the relay over the run, in Mbit/s.
    pub relay_peak_mbps: f64,
    /// Each newcomer group: (size, a middle turn within 7 days, standing
    /// within 28 days, median days to a middle turn, to standing).
    pub directory_newcomers: Group,
    pub given_newcomers: Group,
    /// Mark 4: the operator's share of hops next to an end, and of
    /// transfers with a hop at both ends, those where it held them all.
    pub operator_end_share: f64,
    pub operator_both_share: f64,
    pub both_ends: usize,
    /// Transfers by hops taken (0 to 3); of those with a hop at both ends,
    /// the share where one seed carrier was both, and seed carriers both.
    pub hops_taken: [usize; 4],
    pub seed_single_share: f64,
    pub seed_both_share: f64,
    /// Transfers whose viewer's hop was an ordinary node it earned;
    /// probationers taken on and graduated; the relay's bytes on way 4's
    /// transfers by why: an end said no, or no hop had room.
    pub earned_viewer_hops: usize,
    pub probationers: usize,
    pub graduated: usize,
    pub relayed_said_no: f64,
    pub relayed_no_room: f64,
    pub carrying: Carrying,
    pub exposed_operator: usize,
    pub honest: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Group {
    pub size: usize,
    pub middle_7: usize,
    pub standing_28: usize,
    pub median_middle: Option<f64>,
    pub median_standing: Option<f64>,
}

impl Group {
    /// 9 in 10 got each.
    pub fn met(&self) -> bool {
        self.size > 0
            && 10 * self.middle_7 >= 9 * self.size
            && 10 * self.standing_28 >= 9 * self.size
    }
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied()
}

fn group(m: &Months, kind: Kind) -> Group {
    let g: Vec<_> = m.newcomers.iter().filter(|n| n.kind == kind).collect();
    let within = |d: Option<f64>, days: f64| d.is_some_and(|d| d <= days);
    Group {
        size: g.len(),
        middle_7: g.iter().filter(|n| within(n.middle, 7.0)).count(),
        standing_28: g.iter().filter(|n| within(n.carry_standing, 28.0)).count(),
        median_middle: median(g.iter().filter_map(|n| n.middle).collect()),
        median_standing: median(g.iter().filter_map(|n| n.carry_standing).collect()),
    }
}

pub fn figures(m: &Months) -> Figures {
    let late: Vec<_> = m
        .posts
        .iter()
        .filter(|p| (10..=13).contains(&((p.at / DAY / 7.0) as usize + 1)))
        .collect();
    let delivered: f64 = late.iter().map(|p| p.delivered).sum();
    let h = &m.hops;
    Figures {
        seed: m.seed,
        way: m.way,
        sizing: m.sizing.expect("a sizing run"),
        p95: m.posts.iter().map(|p| p.p95).collect(),
        late_posts: late.len(),
        late_b8: late.iter().filter(|p| p.marks).count(),
        late_relayed: late.iter().map(|p| p.relayed).sum(),
        late_through_hops: late.iter().map(|p| p.delivered_through_hops).sum::<f64>()
            / delivered.max(1.0),
        relay_peak_mbps: m.posts.iter().map(|p| p.relay_peak).fold(0.0, f64::max) * 8.0 / 1e6,
        directory_newcomers: group(m, Kind::NewcomerDirectory),
        given_newcomers: group(m, Kind::NewcomerGiven),
        operator_end_share: h.end_hops_operator as f64 / h.end_hops.max(1) as f64,
        operator_both_share: h.operator_both_ends as f64 / h.both_ends.max(1) as f64,
        both_ends: h.both_ends,
        hops_taken: h.hops_taken,
        seed_single_share: h.seed_both_single as f64 / h.both_ends.max(1) as f64,
        seed_both_share: h.seed_both_ends as f64 / h.both_ends.max(1) as f64,
        earned_viewer_hops: h.earned_viewer_hops,
        probationers: h.probationers,
        graduated: h.graduated,
        relayed_said_no: h.relayed_said_no,
        relayed_no_room: h.relayed_no_room,
        carrying: m.carrying.clone(),
        exposed_operator: m.extra.exposed_operator,
        honest: m.extra.honest,
    }
}

/// The four marks for a carriers run against the project-relay run of the
/// same seed.
#[derive(Clone, Debug, Serialize)]
pub struct Marks {
    /// Mark 1 (amended 2026-11-20): posts within a tenth of the project
    /// relay's time, the worst ratio, and B8 met in weeks 10 to 13 against
    /// the project relay's.
    pub within_tenth: usize,
    pub posts: usize,
    pub worst: f64,
    pub speed: bool,
    /// Mark 2: relayed bytes in weeks 10 to 13 against the project relay's.
    pub relayed_ratio: f64,
    pub relay: bool,
    pub newcomers: bool,
    pub operator: bool,
}

impl Marks {
    pub fn all(&self) -> bool {
        self.speed && self.relay && self.newcomers && self.operator
    }
}

pub fn judge(c: &Figures, project: &Figures) -> Marks {
    let mut within_tenth = 0;
    let mut worst = 0.0f64;
    let mut posts = 0;
    for (a, b) in c.p95.iter().zip(&project.p95) {
        posts += 1;
        // A post nobody finished counts as later than any finished.
        let ratio = match (a, b) {
            (Some(a), Some(b)) => a / b,
            (Some(_), None) => 0.0,
            (None, Some(_)) => f64::INFINITY,
            (None, None) => 1.0,
        };
        within_tenth += (ratio <= 1.1) as usize;
        worst = worst.max(ratio);
    }
    let speed = 10 * within_tenth >= 9 * posts && worst <= 1.2 && c.late_b8 >= project.late_b8;
    let relayed_ratio = c.late_relayed / project.late_relayed.max(1.0);
    let p = c.operator_end_share;
    Marks {
        within_tenth,
        posts,
        worst,
        speed,
        relayed_ratio,
        relay: relayed_ratio <= 0.5,
        newcomers: c.directory_newcomers.met() && c.given_newcomers.met(),
        operator: c.operator_both_share <= 2.0 * p * p,
    }
}

fn one(way: Way, sizing: Sizing, seed: u64) -> Figures {
    let m = months::run(
        Start::Z,
        way,
        RuleSet::Accepted,
        DayRule::Window,
        BytesRule::Window,
        Asking::WhenReason,
        months::Experiment::Baseline,
        seed,
        Some(sizing),
    );
    figures(&m)
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub seeds: Vec<u64>,
    /// For each seed-carrier count used, the two ways compared against,
    /// each seed in turn.
    pub project: Vec<(usize, Vec<Figures>)>,
    pub n0: Vec<(usize, Vec<Figures>)>,
    /// For each setting, each seed's figures and marks.
    pub carriers: Vec<(Sizing, Vec<(Figures, Marks)>)>,
}

/// Every run the plan names, `threads` at a time.
pub fn run(seeds: &[u64], threads: usize) -> Report {
    let sets = settings();
    let mut counts: Vec<usize> = sets.iter().map(|s| s.seeds).collect();
    counts.sort_unstable();
    counts.dedup();
    // The jobs: (way, setting, seed). The ways compared against are the
    // same for every setting with the same seed carriers, which sit idle.
    let mut jobs: Vec<(Way, Sizing, u64)> = Vec::new();
    for &n in &counts {
        let s = sets.iter().find(|s| s.seeds == n).copied().unwrap();
        for &seed in seeds {
            jobs.push((Way::ProjectRelays, s, seed));
            jobs.push((Way::Relays, s, seed));
        }
    }
    for &s in &sets {
        for &seed in seeds {
            jobs.push((Way::Carriers, s, seed));
        }
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::Mutex::new(vec![None; jobs.len()]);
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(&(way, s, seed)) = jobs.get(k) else {
                        break;
                    };
                    let f = one(way, s, seed);
                    eprintln!(
                        "{:?} seeds {} share {} jobs {} rate {} seed {seed}: done",
                        way, s.seeds, s.share, s.jobs, s.newcomer_rate
                    );
                    done.lock().unwrap()[k] = Some(f);
                }
            });
        }
    });
    let done: Vec<Figures> = done
        .into_inner()
        .unwrap()
        .into_iter()
        .map(Option::unwrap)
        .collect();
    let find = |way: Way, n: usize, seed: u64| -> Figures {
        done.iter()
            .find(|f| f.way == way && f.sizing.seeds == n && f.seed == seed)
            .cloned()
            .unwrap()
    };
    let compared = |way: Way| -> Vec<(usize, Vec<Figures>)> {
        counts
            .iter()
            .map(|&n| (n, seeds.iter().map(|&s| find(way, n, s)).collect()))
            .collect()
    };
    let carriers = sets
        .iter()
        .map(|&s| {
            let runs = seeds
                .iter()
                .map(|&seed| {
                    let f = done
                        .iter()
                        .find(|f| {
                            f.way == Way::Carriers
                                && f.seed == seed
                                && f.sizing.seeds == s.seeds
                                && f.sizing.share == s.share
                                && f.sizing.jobs == s.jobs
                                && f.sizing.newcomer_rate == s.newcomer_rate
                        })
                        .cloned()
                        .unwrap();
                    let marks = judge(&f, &find(Way::ProjectRelays, s.seeds, seed));
                    (f, marks)
                })
                .collect();
            (s, runs)
        })
        .collect();
    Report {
        seeds: seeds.to_vec(),
        project: compared(Way::ProjectRelays),
        n0: compared(Way::Relays),
        carriers,
    }
}

/// Bytes as copies of the video.
pub fn copies(bytes: f64) -> f64 {
    bytes / VIDEO_BYTES
}
