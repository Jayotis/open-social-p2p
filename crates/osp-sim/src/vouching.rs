//! Replies let in by who vouched (ADR 0029's step 4): a few hundred people in
//! communities, following and resharing, as real nodes on the simulated
//! network with Tor-like delays.
//!
//! In each community one person is a creator whose replies are watched (Kim,
//! for that community). Some people are popular: creators everyone around
//! reshares, and hunters, who also reshare popular posts first and are
//! credited with finding them. Popular people and some ordinary ones reply
//! to a creator in another community; a popular one picks a creator the plan
//! puts 4 to 6 steps away. Before replying, each replier's node gathers its
//! side, as it does once a day.
//!
//! One operator holds many throwaway identities that vouch for each other
//! and for the operator. One real person, placed 3 steps from the first
//! creator, carelessly reshared one of them, which puts the operator 5 steps
//! from that creator. The operator's node is made by hand: it replies to
//! every creator and shows the fullest side it can make, the throwaways'
//! vouches with the careless reshare and whoever vouched for the careless
//! person, which anyone can ask that person's node for.
//!
//! Measured, from the creators' nodes: which replies were let in, the
//! circle's nodes each meeting asked, and each meeting's own work. From the
//! repliers' nodes: how many nodes gathering their side asked.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{Endpoint, endpoint::Connection};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_node::{
    Config, Meeting, Node, SimSettings,
    config::SyncConfig,
    simnet::{Link, SimNet, TorDelay},
};
use osp_proto::{
    Announce, Note, ObjectId, Payload, Person, PublicKey, SOCIAL_ALPN, SecretKey, SignedObject,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, SentStatus},
    social::{MAX_SIDE_VOUCHES, ReplyState, SocialRequest, SocialResponse},
};
use serde::Serialize;
use tracing::info;

use crate::month::rng::Rng;

/// Nodes started, or asked to do something, at once.
const AT_ONCE: usize = 32;
/// Replies sent at once, so meetings' own work isn't measured on a machine
/// busy with hundreds of others.
const REPLIES_AT_ONCE: usize = 4;
/// Longest to wait for reshares and credits to be heard of.
const HEARD_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Longest to wait for every reply to be answered.
const ANSWERED_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// How long the harness waits before having every node make the pass it
/// would make the next day (`nudge`).
const NUDGE_EVERY: Duration = Duration::from_secs(60);
const MBPS: u64 = 1_000_000;
/// Resharers a post needs before its author credits its first finders.
const RESHARERS_FOR_CREDIT: usize = 10;
const FINDERS_CREDITED: usize = 3;
/// ADR 0029's pass marks.
const POPULAR_PUBLISHED_PERCENT: usize = 80;
const MAX_QUESTIONS: usize = 16;
const MAX_OWN_WORK_MS_ON_SBC: f64 = 150.0;
const MAX_GATHER_REQUESTS_A_DAY: usize = 100;
/// How much slower an SBC-sized node does this work than the desktop the
/// round runs on: a Raspberry Pi 4's core against a Ryzen 5 3600XT's, about.
/// An estimate, not measured; no SBC was at hand.
pub const SBC_SLOWER: f64 = 5.0;

#[derive(Debug, Clone, Serialize)]
pub struct VouchScenario {
    pub dir: PathBuf,
    pub communities: usize,
    /// People in each community, the creator whose replies are watched and
    /// the popular included.
    pub members: usize,
    /// In each community: popular creators, and hunters.
    pub popular_creators: usize,
    pub hunters: usize,
    /// In each community: ordinary people who reply.
    pub ordinary_repliers: usize,
    /// Everyone follows this many others in their community, and this many
    /// outside it.
    pub follows_in: usize,
    pub follows_out: usize,
    /// A popular person is followed by at least this many in their own
    /// community, and by this many in each of `popular_communities` others.
    pub popular_own: usize,
    pub popular_out: usize,
    pub popular_communities: usize,
    /// The chance that someone reshares an ordinary person they follow.
    /// Popular people are reshared by everyone who follows them.
    pub ordinary_reshare: f64,
    pub throwaways: usize,
    pub tor_delay: bool,
    pub seed: u64,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Role {
    /// Whose replies are watched: one in each community.
    Creator,
    Popular,
    Hunter,
    /// An ordinary person who replies.
    Ordinary,
    Member,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Kind {
    Popular,
    Ordinary,
    Throwaway,
}

/// Who follows and reshares whom, worked out before any node runs.
#[derive(Debug, Clone)]
pub struct Plan {
    pub community: Vec<usize>,
    pub role: Vec<Role>,
    /// Each person's follows, in the order followed.
    pub follows: Vec<Vec<usize>>,
    /// Each popular creator's post, with the hunters who reshare it first.
    pub first_finders: Vec<(usize, Vec<usize>)>,
    /// Whose posts each person reshares.
    pub reshares: Vec<BTreeSet<usize>>,
    /// The creators whose replies are watched, one per community.
    pub creators: Vec<usize>,
    /// Who replies to whom, and the plan's distance between them.
    pub replies: Vec<PlannedReply>,
    /// Who carelessly reshared a throwaway identity, 3 steps from the first
    /// creator.
    pub careless: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PlannedReply {
    pub replier: usize,
    pub creator: usize,
    pub kind: Kind,
    /// Steps between them by the plan, without the limits on how much a
    /// side holds; `None` if the sides don't meet.
    pub distance: Option<u8>,
}

impl Plan {
    pub fn new(s: &VouchScenario) -> Result<Self> {
        let per = s.members;
        ensure!(
            1 + s.popular_creators + s.hunters + s.ordinary_repliers < per,
            "a community needs room for its members"
        );
        ensure!(
            s.popular_communities < s.communities,
            "popular people are followed in fewer communities than there are"
        );
        let n = s.communities * per;
        let mut rng = Rng::new(s.seed);
        let community: Vec<usize> = (0..n).map(|p| p / per).collect();
        let role: Vec<Role> = (0..n)
            .map(|p| match p % per {
                0 => Role::Creator,
                i if i <= s.popular_creators => Role::Popular,
                i if i <= s.popular_creators + s.hunters => Role::Hunter,
                i if i <= s.popular_creators + s.hunters + s.ordinary_repliers => Role::Ordinary,
                _ => Role::Member,
            })
            .collect();
        let mut follows: Vec<Vec<usize>> = vec![Vec::new(); n];
        let follow = |follows: &mut Vec<Vec<usize>>, a: usize, b: usize| {
            if a != b && !follows[a].contains(&b) {
                follows[a].push(b);
            }
        };
        let members_of = |c: usize| (c * per..(c + 1) * per).collect::<Vec<_>>();
        for p in 0..n {
            let c = p / per;
            let mut own = members_of(c);
            rng.shuffle(&mut own);
            for q in own.into_iter().filter(|q| *q != p).take(s.follows_in) {
                follow(&mut follows, p, q);
            }
            for _ in 0..s.follows_out {
                let other = ring(c, rng.below(2), s.communities);
                follow(&mut follows, p, other * per + rng.below(per));
            }
        }
        let popular: Vec<usize> = (0..n)
            .filter(|p| matches!(role[*p], Role::Popular | Role::Hunter))
            .collect();
        for &x in &popular {
            let c = community[x];
            let mut own = members_of(c);
            rng.shuffle(&mut own);
            for q in own {
                let followers = members_of(c)
                    .into_iter()
                    .filter(|f| follows[*f].contains(&x))
                    .count();
                if followers >= s.popular_own {
                    break;
                }
                follow(&mut follows, q, x);
            }
            for d in (0..s.popular_communities).map(|k| ring(c, k, s.communities)) {
                let mut theirs = members_of(d);
                rng.shuffle(&mut theirs);
                for q in theirs.into_iter().take(s.popular_out) {
                    follow(&mut follows, q, x);
                }
            }
        }
        // Hunters follow popular creators in their own and the nearest
        // communities, to reshare their posts first.
        let hunters: Vec<usize> = (0..n).filter(|p| role[*p] == Role::Hunter).collect();
        let mut first_finders = Vec::new();
        if !hunters.is_empty() {
            let creators: Vec<usize> = (0..n).filter(|p| role[*p] == Role::Popular).collect();
            for &author in &creators {
                let c = community[author];
                let near = std::iter::once(c)
                    .chain((0..s.communities - 1).map(|k| ring(c, k, s.communities)));
                let community = &community;
                let finders: Vec<usize> = near
                    .flat_map(|d| hunters.iter().copied().filter(move |h| community[*h] == d))
                    .take(FINDERS_CREDITED)
                    .collect();
                for &h in &finders {
                    follow(&mut follows, h, author);
                }
                first_finders.push((author, finders));
            }
        }
        let mut reshares = vec![BTreeSet::new(); n];
        for p in 0..n {
            for &f in &follows[p] {
                let popular = matches!(role[f], Role::Popular | Role::Hunter);
                if popular || rng.chance(s.ordinary_reshare) {
                    reshares[p].insert(f);
                }
            }
        }
        let creators: Vec<usize> = (0..n).filter(|p| role[*p] == Role::Creator).collect();
        let mut plan = Plan {
            community,
            role,
            follows,
            first_finders,
            reshares,
            creators,
            replies: Vec::new(),
            careless: 0,
        };
        plan.replies = plan.pick_replies(&mut rng);
        plan.careless = plan
            .pick_careless(&mut rng)
            .context("nobody is 3 steps from the first creator")?;
        Ok(plan)
    }

    pub fn people(&self) -> usize {
        self.role.len()
    }

    /// Who vouched for `x`: those who reshared them, and the authors who
    /// credited them with finding a post early.
    fn vouchers_of(&self) -> Vec<Vec<usize>> {
        let mut vouchers = vec![Vec::new(); self.people()];
        for (p, authors) in self.reshares.iter().enumerate() {
            for &a in authors {
                vouchers[a].push(p);
            }
        }
        for (author, finders) in self.credited() {
            for f in finders {
                vouchers[f].push(author);
            }
        }
        vouchers
    }

    /// The first finders of each popular post that gets enough resharers
    /// to credit them.
    pub fn credited(&self) -> Vec<(usize, Vec<usize>)> {
        self.first_finders
            .iter()
            .filter(|(author, _)| self.resharers(*author) >= RESHARERS_FOR_CREDIT)
            .cloned()
            .collect()
    }

    pub fn resharers(&self, author: usize) -> usize {
        self.reshares.iter().filter(|r| r.contains(&author)).count()
    }

    /// A replier's side: who vouched for them, and so on 3 steps back.
    fn side(&self, vouchers: &[Vec<usize>], replier: usize) -> HashMap<usize, u8> {
        let mut steps = HashMap::new();
        let mut frontier = vec![replier];
        for step in 1..=3u8 {
            let mut next = Vec::new();
            for x in frontier {
                for &v in &vouchers[x] {
                    if v != replier && !steps.contains_key(&v) {
                        steps.insert(v, step);
                        next.push(v);
                    }
                }
            }
            frontier = next;
        }
        steps
    }

    /// Whom a creator's node places, and at how many steps: on its own, and
    /// through the nodes of up to 16 people it follows.
    fn placed_by(&self, creator: usize) -> HashMap<usize, u8> {
        let mut placed: HashMap<usize, u8> = HashMap::new();
        let mut place = |x: usize, steps: u8| {
            if x != creator {
                let at = placed.entry(x).or_insert(steps);
                *at = (*at).min(steps);
            }
        };
        let circle = &self.follows[creator];
        for &a in circle {
            place(a, 1);
            for &r in &self.reshares[a] {
                place(r, 2);
            }
        }
        for &a in circle.iter().take(MAX_QUESTIONS) {
            for &b in &self.follows[a] {
                place(b, 2);
                for &r in &self.reshares[b] {
                    place(r, 3);
                }
            }
        }
        placed
    }

    fn distance(&self, side: &HashMap<usize, u8>, placed: &HashMap<usize, u8>) -> Option<u8> {
        side.iter()
            .filter_map(|(x, back)| placed.get(x).map(|at| at + back))
            .min()
    }

    /// Each popular person replies to a creator in another community 4 to 6
    /// steps away by the plan, spread evenly; each ordinary replier to one
    /// in another community, at random.
    fn pick_replies(&self, rng: &mut Rng) -> Vec<PlannedReply> {
        let vouchers = self.vouchers_of();
        let placed: Vec<HashMap<usize, u8>> =
            self.creators.iter().map(|c| self.placed_by(*c)).collect();
        let mut load = vec![0usize; self.creators.len()];
        let mut replies = Vec::new();
        for p in 0..self.people() {
            let kind = match self.role[p] {
                Role::Popular | Role::Hunter => Kind::Popular,
                Role::Ordinary => Kind::Ordinary,
                _ => continue,
            };
            let side = self.side(&vouchers, p);
            let others: Vec<usize> = (0..self.creators.len())
                .filter(|k| self.community[self.creators[*k]] != self.community[p])
                .collect();
            let distance = |k: usize| self.distance(&side, &placed[k]);
            let k = match kind {
                Kind::Popular => {
                    let far: Vec<usize> = others
                        .iter()
                        .copied()
                        .filter(|k| distance(*k).is_some_and(|d| (4..=6).contains(&d)))
                        .collect();
                    let from = if far.is_empty() { &others } else { &far };
                    *from
                        .iter()
                        .min_by_key(|k| (load[**k], rng.next_u64()))
                        .expect("another community")
                }
                _ => others[rng.below(others.len())],
            };
            load[k] += 1;
            replies.push(PlannedReply {
                replier: p,
                creator: self.creators[k],
                kind,
                distance: distance(k),
            });
        }
        replies
    }

    /// An ordinary member the first creator's node places at 3 steps,
    /// through a person its owner follows.
    fn pick_careless(&self, rng: &mut Rng) -> Option<usize> {
        let placed = self.placed_by(self.creators[0]);
        let mut at_three: Vec<usize> = placed
            .iter()
            .filter(|(x, at)| **at == 3 && self.role[**x] == Role::Member)
            .map(|(x, _)| *x)
            .collect();
        at_three.sort();
        (!at_three.is_empty()).then(|| at_three[rng.below(at_three.len())])
    }

    /// Different people vouching for `p`, and the communities they're in.
    pub fn vouched(&self, p: usize) -> (usize, usize) {
        let vouchers = self.vouchers_of();
        let people: HashSet<usize> = vouchers[p].iter().copied().collect();
        let communities: HashSet<usize> = people.iter().map(|v| self.community[*v]).collect();
        (people.len(), communities.len())
    }

    pub fn summary(&self) -> String {
        let mut out = String::new();
        let follows: usize = self.follows.iter().map(Vec::len).sum();
        let reshares: usize = self.reshares.iter().map(BTreeSet::len).sum();
        let _ = writeln!(
            out,
            "plan: {} people, {} follows, {} reshares, {} credited posts",
            self.people(),
            follows,
            reshares,
            self.credited().len()
        );
        let popular: Vec<usize> = (0..self.people())
            .filter(|p| matches!(self.role[*p], Role::Popular | Role::Hunter))
            .collect();
        let least = popular
            .iter()
            .map(|p| self.vouched(*p))
            .min()
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "the least vouched-for popular person: {} people in {} communities",
            least.0, least.1
        );
        for kind in [Kind::Popular, Kind::Ordinary] {
            let mut at: HashMap<Option<u8>, usize> = HashMap::new();
            for r in self.replies.iter().filter(|r| r.kind == kind) {
                *at.entry(r.distance).or_default() += 1;
            }
            let mut at: Vec<_> = at.into_iter().collect();
            at.sort();
            let shown: Vec<String> = at
                .iter()
                .map(|(d, n)| match d {
                    Some(d) => format!("{d} steps: {n}"),
                    None => format!("don't meet: {n}"),
                })
                .collect();
            let _ = writeln!(out, "{kind:?} replies by plan: {}", shown.join(", "));
        }
        let (near, far) = self.careless_meets();
        let _ = writeln!(
            out,
            "the careless person, 3 steps from the first creator, has {near} vouchers within 3 steps of them and {far} more beyond",
        );
        out
    }

    /// Of those who vouched for the careless person, how many the first
    /// creator's node, with its circle's, places, and how many it doesn't.
    /// Each placed one would be another meeting point for the operator, 3
    /// steps back from them.
    pub fn careless_meets(&self) -> (usize, usize) {
        let placed = self.placed_by(self.creators[0]);
        let vouchers: HashSet<usize> = self.vouchers_of()[self.careless].iter().copied().collect();
        let near = vouchers.iter().filter(|v| placed.contains_key(v)).count();
        (near, vouchers.len() - near)
    }
}

/// One reply, after the run.
#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub kind: Kind,
    pub replier: usize,
    pub creator: usize,
    pub planned_distance: Option<u8>,
    /// As the replier's node has it, or the operator's node was answered.
    pub status: String,
    pub published: bool,
    pub meeting: Option<MeetingRecord>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct MeetingRecord {
    pub side: usize,
    pub asked: usize,
    pub answered: usize,
    pub own_work_ms: f64,
    pub steps: Option<u8>,
    pub points: usize,
    pub through: usize,
    pub let_in: bool,
    /// For each distance from the creator, 0 to 6: separate meeting points
    /// within it, and the people in the creator's circle they come through.
    pub within: [(usize, usize); 7],
}

impl MeetingRecord {
    /// Whether `table` would have let it in.
    fn let_in_by(&self, table: Table) -> bool {
        self.within
            .iter()
            .enumerate()
            .any(|(d, &(points, roots))| table.enough(d, points, roots))
    }
}

/// Meeting points needed at 2 or 3 steps, at 4, and at 5 or 6; from 4 steps,
/// through at least 2 people in the creator's circle (ADR 0029's table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Table(pub usize, pub usize, pub usize);

impl Table {
    fn enough(self, distance: usize, points: usize, roots: usize) -> bool {
        match distance {
            0..=3 => points >= self.0,
            4 => points >= self.1 && roots >= 2,
            _ => points >= self.2 && roots >= 2,
        }
    }
}

/// The table as built, and others to compare it with: the first, before
/// separate chains, and those either side of the one built.
pub const TABLES: [Table; 7] = [
    Table(3, 4, 6),
    Table(3, 5, 10),
    Table(3, 5, 8),
    Table(3, 4, 8),
    Table(3, 4, 5),
    Table(3, 3, 4),
    Table(2, 3, 4),
];

impl From<Meeting> for MeetingRecord {
    fn from(m: Meeting) -> Self {
        Self {
            side: m.side,
            asked: m.asked,
            answered: m.answered,
            own_work_ms: m.own_work.as_secs_f64() * 1000.0,
            steps: m.steps,
            points: m.points,
            through: m.through,
            let_in: m.let_in,
            within: m.within,
        }
    }
}

/// One replier's gathering of their side.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Gathering {
    pub replier: usize,
    pub side: usize,
    pub asked: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct VouchReport {
    pub label: String,
    pub started: String,
    pub scenario: VouchScenario,
    pub nodes: usize,
    pub setup_secs: f64,
    /// Each popular person: different people who vouched for them by the
    /// plan, and the communities they're in.
    pub popular_vouched: Vec<(usize, usize)>,
    /// Hunters' found-early credits, as their nodes hold them.
    pub hunter_credits: Vec<usize>,
    pub gatherings: Vec<Gathering>,
    /// The operator's side, as shown.
    pub throwaway_side: usize,
    pub outcomes: Vec<Outcome>,
}

struct World {
    nodes: Vec<Node>,
    ids: HashMap<PublicKey, usize>,
}

pub async fn run(s: &VouchScenario) -> Result<VouchReport> {
    let plan = Plan::new(s)?;
    info!("{}", plan.summary().trim_end());
    std::fs::create_dir_all(&s.dir)?;
    let started = osp_proto::now_rfc3339();
    let setup = Instant::now();
    let net = SimNet::new(s.seed);
    let sim = SimSettings {
        tor_delay: s.tor_delay.then(TorDelay::default),
        new_followers_per_hour: 10_000,
        ..SimSettings::default()
    };
    let link = || {
        net.link(Link {
            up_bps: 100 * MBPS,
            down_bps: 100 * MBPS,
            delay: Duration::from_millis(10),
            loss: 0.0,
        })
    };
    let nodes: Vec<Node> = stream::iter(0..plan.people())
        .map(|p| {
            let home = s.dir.join(format!("p{p:03}"));
            let sim = SimSettings {
                network: Some(link()),
                ..sim.clone()
            };
            let vouchers = plan.role[p] == Role::Creator;
            async move { start(&home, &format!("P{p:03}"), vouchers, sim).await }
        })
        .buffered_ordered(AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_>>()?;
    let ids = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.identity(), i))
        .collect();
    let w = World { nodes, ids };
    info!(nodes = w.nodes.len(), "nodes started");

    // Who follows whom, and everyone's post.
    let tickets: Vec<_> = stream::iter(&w.nodes)
        .map(|node| async move {
            match call(node, ClientRequest::Invite).await? {
                ClientResponse::Ticket { ticket } => Ok(ticket),
                other => bail!("no ticket: {other:?}"),
            }
        })
        .buffered_ordered(AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_>>()?;
    let posts: Vec<ObjectId> = stream::iter(&w.nodes)
        .map(|node| post(node, "something worth passing on"))
        .buffered_ordered(AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_>>()?;
    each(&w.nodes, |p, node| {
        let tickets = &tickets;
        let follows = &plan.follows[p];
        async move {
            for &f in follows {
                let ticket = tickets[f].clone();
                call(node, ClientRequest::Follow { ticket }).await?;
            }
            Ok(())
        }
    })
    .await?;
    info!("everyone follows");
    sync_everyone(&w.nodes).await?;
    info!("everyone synced");

    // Hunters reshare popular posts first, and their authors hear of it.
    let mut first: Vec<(usize, usize)> = Vec::new();
    for (author, finders) in &plan.first_finders {
        for &h in finders {
            first.push((h, *author));
        }
    }
    stream::iter(&first)
        .map(|&(h, author)| reshare(&w.nodes[h], posts[author]))
        .buffered_unordered(AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let mut expected: HashMap<usize, usize> = HashMap::new();
    for (_, author) in &first {
        *expected.entry(*author).or_default() += 1;
    }
    wait_heard(&w, &posts, &expected).await?;
    info!("first finders heard of");

    // Then everyone else's reshares.
    each(&w.nodes, |p, node| {
        let authors: Vec<usize> = plan.reshares[p]
            .iter()
            .copied()
            .filter(|a| !first.contains(&(p, *a)))
            .collect();
        let posts = &posts;
        async move {
            for a in authors {
                reshare(node, posts[a]).await?;
            }
            Ok(())
        }
    })
    .await?;
    let expected: HashMap<usize, usize> = (0..plan.people())
        .map(|a| (a, plan.resharers(a)))
        .filter(|(_, n)| *n > 0)
        .collect();
    wait_heard(&w, &posts, &expected).await?;
    info!("every reshare heard of");
    let hunters: Vec<usize> = (0..plan.people())
        .filter(|p| plan.role[*p] == Role::Hunter)
        .collect();
    let mut credits_due: HashMap<usize, usize> = HashMap::new();
    for (_, finders) in plan.credited() {
        for f in finders {
            *credits_due.entry(f).or_default() += 1;
        }
    }
    let by = Instant::now() + HEARD_TIMEOUT;
    let mut nudged = Instant::now();
    loop {
        let short = hunters
            .iter()
            .filter(|h| {
                let held = w.nodes[**h].found_early_credits().map_or(0, |c| c.len());
                held < credits_due.get(h).copied().unwrap_or(0)
            })
            .count();
        if short == 0 {
            break;
        }
        ensure!(
            Instant::now() < by,
            "{short} hunters lack their credits in time"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
        nudge(&w, &mut nudged);
    }
    let hunter_credits = hunters
        .iter()
        .map(|h| w.nodes[*h].found_early_credits().map_or(0, |c| c.len()))
        .collect();
    info!("credits held");
    // Followers' nodes hold the reshares in the outboxes they follow.
    sync_everyone(&w.nodes).await?;
    info!("everyone synced again");

    // Repliers gather their sides, as once a day.
    let gatherings: Vec<Gathering> = stream::iter(&plan.replies)
        .map(|r| {
            let node = &w.nodes[r.replier];
            async move {
                let g = node.gather_side_now().await?;
                Ok::<_, anyhow::Error>(Gathering {
                    replier: r.replier,
                    side: g.side,
                    asked: g.asked,
                })
            }
        })
        .buffered_ordered(AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_>>()?;
    info!("sides gathered");

    // Each creator posts; each replier follows them, to read the post.
    let mut open: HashMap<usize, ObjectId> = HashMap::new();
    for &c in &plan.creators {
        open.insert(c, post(&w.nodes[c], "what do you think?").await?);
    }
    let mut readers: HashMap<usize, Vec<usize>> = HashMap::new();
    for r in &plan.replies {
        readers.entry(r.replier).or_default().push(r.creator);
    }
    stream::iter(readers)
        .map(|(p, creators)| {
            let node = &w.nodes[p];
            let tickets = &tickets;
            async move {
                for c in creators {
                    let ticket = tickets[c].clone();
                    call(node, ClientRequest::Follow { ticket }).await?;
                }
                call(node, ClientRequest::Sync).await
            }
        })
        .buffered_unordered(AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let setup_secs = setup.elapsed().as_secs_f64();
    info!(setup_secs, "set up; replying");

    // The replies, a few at once.
    stream::iter(&plan.replies)
        .map(|r| {
            let req = ClientRequest::Reply {
                post: open[&r.creator],
                content: "a reply from further out".into(),
            };
            call(&w.nodes[r.replier], req)
        })
        .buffered_unordered(REPLIES_AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;

    // The operator's.
    let operator = Operator::new(&net, s, &w.nodes[plan.careless]).await?;
    let mut throwaway_answers = Vec::new();
    for &c in &plan.creators {
        let state = operator.reply(&w.nodes[c], open[&c]).await?;
        throwaway_answers.push((c, state));
    }
    info!(?throwaway_answers, "the operator replied");

    // Waits for every reply to be answered.
    let by = Instant::now() + ANSWERED_TIMEOUT;
    let statuses = loop {
        let mut statuses = HashMap::new();
        for r in &plan.replies {
            statuses.insert((r.replier, r.creator), sent_status(&w, r).await?);
        }
        let sending = statuses
            .values()
            .filter(|s| **s == SentStatus::Sending)
            .count();
        if sending == 0 || Instant::now() >= by {
            break statuses;
        }
        info!(sending, "replies not answered yet");
        tokio::time::sleep(Duration::from_secs(5)).await;
    };

    // What the creators' nodes found.
    let mut meetings: HashMap<(usize, usize), Meeting> = HashMap::new();
    for &c in &plan.creators {
        for m in w.nodes[c].meetings() {
            let replier = w.ids.get(&m.replier).copied().unwrap_or(usize::MAX);
            meetings.insert((replier, c), m);
        }
    }
    let mut outcomes: Vec<Outcome> = plan
        .replies
        .iter()
        .map(|r| {
            let status = statuses[&(r.replier, r.creator)];
            Outcome {
                kind: r.kind,
                replier: r.replier,
                creator: r.creator,
                planned_distance: r.distance,
                status: format!("{status:?}"),
                published: status == SentStatus::Published,
                meeting: meetings.get(&(r.replier, r.creator)).map(|m| (*m).into()),
            }
        })
        .collect();
    for &c in &plan.creators {
        let state = throwaway_answers
            .iter()
            .find(|(k, _)| *k == c)
            .map(|(_, s)| *s);
        let meeting = w.nodes[c]
            .meetings()
            .into_iter()
            .find(|m| m.replier == operator.identity.public());
        outcomes.push(Outcome {
            kind: Kind::Throwaway,
            replier: usize::MAX,
            creator: c,
            planned_distance: (c == plan.creators[0]).then_some(5),
            status: format!("{state:?}"),
            published: state == Some(ReplyState::Published),
            meeting: meeting.map(Into::into),
        });
    }

    operator.endpoint.close().await;
    let popular_vouched = (0..plan.people())
        .filter(|p| matches!(plan.role[*p], Role::Popular | Role::Hunter))
        .map(|p| plan.vouched(p))
        .collect();
    let nodes = w.nodes.len();
    for node in w.nodes {
        node.shutdown().await.ok();
    }
    Ok(VouchReport {
        label: s.label.clone(),
        started,
        scenario: s.clone(),
        nodes,
        setup_secs,
        popular_vouched,
        hunter_credits,
        gatherings,
        throwaway_side: operator.side.len(),
        outcomes,
    })
}

impl VouchReport {
    fn of(&self, kind: Kind) -> Vec<&Outcome> {
        self.outcomes.iter().filter(|o| o.kind == kind).collect()
    }

    fn published(&self, kind: Kind) -> usize {
        self.of(kind).iter().filter(|o| o.published).count()
    }

    pub fn max_asked(&self) -> usize {
        self.outcomes
            .iter()
            .filter_map(|o| o.meeting.map(|m| m.asked))
            .max()
            .unwrap_or(0)
    }

    pub fn max_own_work_ms(&self) -> f64 {
        self.outcomes
            .iter()
            .filter_map(|o| o.meeting.map(|m| m.own_work_ms))
            .fold(0.0, f64::max)
    }

    pub fn median_own_work_ms(&self) -> f64 {
        let mut work: Vec<f64> = self
            .outcomes
            .iter()
            .filter_map(|o| o.meeting.map(|m| m.own_work_ms))
            .collect();
        work.sort_by(f64::total_cmp);
        work.get(work.len() / 2).copied().unwrap_or(0.0)
    }

    pub fn max_gather_asked(&self) -> usize {
        self.gatherings.iter().map(|g| g.asked).max().unwrap_or(0)
    }

    /// ADR 0029's marks, each with whether it was met.
    pub fn marks(&self) -> Vec<(String, bool)> {
        let popular = self.of(Kind::Popular).len();
        let published = self.published(Kind::Popular);
        vec![
            (
                format!(
                    "no throwaway reply published: {} of {}",
                    self.published(Kind::Throwaway),
                    self.of(Kind::Throwaway).len()
                ),
                self.published(Kind::Throwaway) == 0,
            ),
            (
                format!(
                    "at least {POPULAR_PUBLISHED_PERCENT}% of popular strangers published: {published} of {popular}"
                ),
                popular > 0 && published * 100 >= popular * POPULAR_PUBLISHED_PERCENT,
            ),
            (
                format!(
                    "at most {MAX_QUESTIONS} questions a meeting: at most {}",
                    self.max_asked()
                ),
                self.max_asked() <= MAX_QUESTIONS,
            ),
            (
                format!(
                    "at most {MAX_OWN_WORK_MS_ON_SBC} ms of work on an SBC-sized node: at most {:.1} ms here (median {:.1}), about {:.0} ms on one",
                    self.max_own_work_ms(),
                    self.median_own_work_ms(),
                    self.max_own_work_ms() * SBC_SLOWER
                ),
                self.max_own_work_ms() * SBC_SLOWER <= MAX_OWN_WORK_MS_ON_SBC,
            ),
            (
                format!(
                    "gathering a side under {MAX_GATHER_REQUESTS_A_DAY} requests a day: at most {}",
                    self.max_gather_asked()
                ),
                self.max_gather_asked() < MAX_GATHER_REQUESTS_A_DAY,
            ),
        ]
    }

    pub fn passed(&self) -> bool {
        self.marks().iter().all(|(_, met)| *met)
    }

    pub fn summary(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} nodes ({}), set up in {:.0} s",
            self.nodes, self.label, self.setup_secs
        );
        if let Some(least) = self.popular_vouched.iter().min() {
            let _ = writeln!(
                out,
                "least vouched-for popular person: {} people in {} communities; hunters' credits {}",
                least.0,
                least.1,
                slashed(&self.hunter_credits)
            );
        }
        let sides: Vec<usize> = self.gatherings.iter().map(|g| g.side).collect();
        let _ = writeln!(
            out,
            "sides gathered: median {} people, nodes asked at most {}; the operator showed {}",
            median(&sides),
            self.max_gather_asked(),
            self.throwaway_side
        );
        let _ = writeln!(
            out,
            "{:<10} {:>8} {:>8} {:>10} {:>9} {:>8} {:>8} {:>8}",
            "", "replies", "let in", "published", "met at", "points", "asked", "work ms"
        );
        for kind in [Kind::Popular, Kind::Ordinary, Kind::Throwaway] {
            let these = self.of(kind);
            let met: Vec<MeetingRecord> = these.iter().filter_map(|o| o.meeting).collect();
            let steps: Vec<usize> = met
                .iter()
                .filter_map(|m| m.steps.map(usize::from))
                .collect();
            let points: Vec<usize> = met.iter().map(|m| m.points).collect();
            let asked: Vec<usize> = met.iter().map(|m| m.asked).collect();
            let work = met.iter().map(|m| m.own_work_ms).fold(0.0, f64::max);
            let _ = writeln!(
                out,
                "{:<10} {:>8} {:>8} {:>10} {:>9} {:>8} {:>8} {:>8.1}",
                format!("{kind:?}"),
                these.len(),
                met.iter().filter(|m| m.let_in).count(),
                self.published(kind),
                format!("~{}", median(&steps)),
                format!("~{}", median(&points)),
                format!("≤{}", asked.iter().max().unwrap_or(&0)),
                work
            );
        }
        let not_met = self
            .of(Kind::Popular)
            .into_iter()
            .filter(|o| !o.published)
            .map(|o| match o.meeting {
                Some(m) => format!(
                    "{}→{} ({:?} steps, {} points through {}, plan {:?})",
                    o.replier, o.creator, m.steps, m.points, m.through, o.planned_distance
                ),
                None => format!("{}→{} ({}, no meeting)", o.replier, o.creator, o.status),
            })
            .collect::<Vec<_>>();
        if !not_met.is_empty() {
            let _ = writeln!(out, "popular not published: {}", not_met.join("; "));
        }
        for t in self.of(Kind::Throwaway) {
            if let Some(m) = t.meeting {
                let _ = writeln!(
                    out,
                    "throwaway reply to {}: {:?} steps, {} points through {}",
                    t.creator, m.steps, m.points, m.through
                );
            }
        }
        let _ = writeln!(
            out,
            "let in by other tables (points at 2-3 / 4 / 5-6 steps):"
        );
        for table in TABLES {
            let by = |kind: Kind| {
                let these = self.of(kind);
                let n = these
                    .iter()
                    .filter(|o| o.meeting.is_some_and(|m| m.let_in_by(table)))
                    .count();
                format!("{n} of {}", these.len())
            };
            let _ = writeln!(
                out,
                "  {}/{}/{}: popular {}, ordinary {}, throwaway {}",
                table.0,
                table.1,
                table.2,
                by(Kind::Popular),
                by(Kind::Ordinary),
                by(Kind::Throwaway)
            );
        }
        for (mark, met) in self.marks() {
            let _ = writeln!(out, "{} {mark}", if met { "met:   " } else { "MISSED:" });
        }
        out
    }
}

/// The operator: a node made by hand, holding the throwaway identities.
struct Operator {
    identity: SecretKey,
    endpoint: Endpoint,
    side: Vec<SignedObject>,
}

impl Operator {
    async fn new(net: &SimNet, s: &VouchScenario, careless: &Node) -> Result<Self> {
        let identity = SecretKey::generate();
        let throwaways: Vec<SecretKey> = (0..s.throwaways).map(|_| SecretKey::generate()).collect();
        let endpoint_key = SecretKey::generate();
        let link = net.link(Link::default());
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Disabled)
            .clear_ip_transports()
            .add_custom_transport(link.transport(endpoint_key.public()))
            .address_lookup(link.lookup())
            .secret_key(endpoint_key)
            .bind()
            .await?;
        // Each throwaway vouches for the operator; the careless person for
        // the first throwaway; whoever vouched for the careless person, as
        // their node tells anyone; and the throwaways for each other.
        let mut side: Vec<SignedObject> = throwaways
            .iter()
            .map(|t| vouch(t, identity.public()))
            .collect::<Result<_>>()?;
        if let Some(t0) = throwaways.first() {
            side.push(careless.sign_reshare_of(t0.public())?);
            let conn = endpoint
                .connect(careless.node_id(), SOCIAL_ALPN)
                .await
                .context("reaching the careless person's node")?;
            match ask(&conn, &SocialRequest::WhoVouched).await? {
                SocialResponse::Vouches { vouches } => {
                    side.extend(vouches.into_iter().map(|v| v.vouch));
                }
                other => bail!("unexpected answer to WhoVouched: {other:?}"),
            }
        }
        for (i, t) in throwaways.iter().enumerate() {
            if side.len() >= MAX_SIDE_VOUCHES {
                break;
            }
            let next = &throwaways[(i + 1) % throwaways.len()];
            side.push(vouch(t, next.public())?);
        }
        side.truncate(MAX_SIDE_VOUCHES);
        Ok(Self {
            identity,
            endpoint,
            side,
        })
    }

    /// Replies to `post` on `creator`'s node, and shows the side when asked.
    async fn reply(&self, creator: &Node, post: ObjectId) -> Result<ReplyState> {
        let conn = self
            .endpoint
            .connect(creator.node_id(), SOCIAL_ALPN)
            .await?;
        let note = Payload::Note(Note {
            actor: self.identity.public(),
            content: "a reply from nowhere in particular".into(),
            published: osp_proto::now_rfc3339(),
            attachment: vec![],
            in_reply_to: Some(post),
        });
        let reply = SignedObject::sign(&self.identity, &note)?;
        let profile = SignedObject::sign(
            &self.identity,
            &Payload::Person(Person {
                actor: self.identity.public(),
                name: "Operator".into(),
                summary: None,
                discoverable: false,
                nodes: vec![self.endpoint.id()],
                media_endpoints: vec![],
                published: osp_proto::now_rfc3339(),
            }),
        )?;
        let req = SocialRequest::SubmitReply {
            reply: reply.clone(),
            profile,
        };
        let (state, show) = match ask(&conn, &req).await? {
            SocialResponse::Reply {
                state,
                show_vouches,
            } => (state, show_vouches),
            other => bail!("unexpected answer to SubmitReply: {other:?}"),
        };
        if !show {
            return Ok(state);
        }
        let req = SocialRequest::ShowVouches {
            reply: reply.id(),
            side: self.side.clone(),
        };
        match ask(&conn, &req).await? {
            SocialResponse::Reply { state, .. } => Ok(state),
            other => bail!("unexpected answer to ShowVouches: {other:?}"),
        }
    }
}

async fn ask(conn: &Connection, req: &SocialRequest) -> Result<SocialResponse> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, req).await?;
    send.finish()?;
    read_frame(&mut recv).await
}

/// `voucher`'s reshare of a post of `of`'s.
fn vouch(voucher: &SecretKey, of: PublicKey) -> Result<SignedObject> {
    SignedObject::sign(
        voucher,
        &Payload::Announce(Announce {
            actor: voucher.public(),
            object: ObjectId::of(of.as_bytes()),
            object_actor: of,
            published: osp_proto::now_rfc3339(),
            hosting: false,
        }),
    )
}

async fn start(home: &Path, name: &str, vouchers: bool, sim: SimSettings) -> Result<Node> {
    let mut config = Config::local_test(name);
    config.sync.interval_secs = SyncConfig::default().interval_secs;
    config.replies.vouchers = vouchers;
    Node::init(home, &config)?;
    Node::spawn_sim(home, sim).await
}

async fn call(node: &Node, req: ClientRequest) -> Result<ClientResponse> {
    match node.handle(req).await {
        ClientResponse::Error { message } => bail!("{message}"),
        resp => Ok(resp),
    }
}

async fn post(node: &Node, text: &str) -> Result<ObjectId> {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
        upload: None,
    };
    match call(node, req).await? {
        ClientResponse::Posted { id } => Ok(id),
        other => bail!("not posted: {other:?}"),
    }
}

async fn reshare(node: &Node, id: ObjectId) -> Result<()> {
    call(node, ClientRequest::Reshare { id, host: false }).await?;
    Ok(())
}

/// Runs `f` for every node, a few at once.
async fn each<'a, F, Fut>(nodes: &'a [Node], f: F) -> Result<()>
where
    F: Fn(usize, &'a Node) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    stream::iter(nodes.iter().enumerate())
        .map(|(p, node)| f(p, node))
        .buffered_unordered(AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}

async fn sync_everyone(nodes: &[Node]) -> Result<()> {
    each(nodes, |_, node| async move {
        call(node, ClientRequest::Sync).await?;
        Ok(())
    })
    .await
}

/// Waits until each author's node counts at least `expected` resharers.
async fn wait_heard(w: &World, posts: &[ObjectId], expected: &HashMap<usize, usize>) -> Result<()> {
    let by = Instant::now() + HEARD_TIMEOUT;
    let mut nudged = Instant::now();
    loop {
        let mut short = 0;
        for (&author, &n) in expected {
            let heard =
                match call(&w.nodes[author], ClientRequest::Show { id: posts[author] }).await? {
                    ClientResponse::Item { item } => item.resharers.unwrap_or(0) as usize,
                    other => bail!("unexpected {other:?}"),
                };
            if heard < n {
                short += 1;
            }
        }
        if short == 0 {
            return Ok(());
        }
        ensure!(
            Instant::now() < by,
            "{short} authors haven't heard of their reshares in time"
        );
        info!(short, "authors still to hear of reshares");
        tokio::time::sleep(Duration::from_secs(5)).await;
        nudge(w, &mut nudged);
    }
}

/// A node tells an author's node of a reshare, or a finder of a credit, at
/// once, and if that fails, on its next pass, which is a day later after its
/// first. Every `NUDGE_EVERY`, every node makes its next pass now.
fn nudge(w: &World, nudged: &mut Instant) {
    if nudged.elapsed() >= NUDGE_EVERY {
        for node in &w.nodes {
            node.tell_vouches_now();
        }
        *nudged = Instant::now();
    }
}

async fn sent_status(w: &World, r: &PlannedReply) -> Result<SentStatus> {
    let creator = w.nodes[r.creator].identity();
    match call(&w.nodes[r.replier], ClientRequest::SentReplies).await? {
        ClientResponse::SentReplies { replies } => Ok(replies
            .iter()
            .find(|x| x.creator == creator)
            .map_or(SentStatus::Sending, |x| x.status)),
        other => bail!("unexpected {other:?}"),
    }
}

/// Communities sit in a ring: the `k`th nearest to `c`, taking turns on
/// either side (next, previous, the one after next, ...).
fn ring(c: usize, k: usize, communities: usize) -> usize {
    let away = k / 2 + 1;
    if k.is_multiple_of(2) {
        (c + away) % communities
    } else {
        (c + communities - away % communities) % communities
    }
}

fn median(values: &[usize]) -> usize {
    let mut v = values.to_vec();
    v.sort();
    v.get(v.len() / 2).copied().unwrap_or(0)
}

fn slashed<T: ToString>(list: &[T]) -> String {
    list.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small(dir: PathBuf) -> VouchScenario {
        VouchScenario {
            dir,
            communities: 4,
            members: 14,
            popular_creators: 1,
            hunters: 1,
            ordinary_repliers: 1,
            follows_in: 4,
            follows_out: 1,
            popular_own: 8,
            popular_out: 3,
            popular_communities: 2,
            ordinary_reshare: 0.3,
            throwaways: 40,
            tor_delay: false,
            seed: 1,
            label: "test".into(),
        }
    }

    #[test]
    fn the_full_plan_meets_the_adrs_setup() {
        let s = VouchScenario {
            communities: 10,
            members: 30,
            popular_creators: 2,
            hunters: 1,
            ordinary_repliers: 5,
            follows_in: 8,
            follows_out: 1,
            popular_own: 18,
            popular_out: 6,
            popular_communities: 2,
            throwaways: 200,
            tor_delay: true,
            ..small(PathBuf::new())
        };
        let plan = Plan::new(&s).unwrap();
        println!("{}", plan.summary());
        let popular: Vec<&PlannedReply> = plan
            .replies
            .iter()
            .filter(|r| r.kind == Kind::Popular)
            .collect();
        assert_eq!(popular.len(), 30);
        assert_eq!(
            plan.replies
                .iter()
                .filter(|r| r.kind == Kind::Ordinary)
                .count(),
            50
        );
        for r in &popular {
            let (people, communities) = plan.vouched(r.replier);
            assert!(
                people >= 30 && communities >= 3,
                "{people} in {communities}"
            );
            assert!(
                r.distance.is_some_and(|d| (4..=6).contains(&d)),
                "{:?}",
                r.distance
            );
        }
        assert_eq!(plan.placed_by(plan.creators[0])[&plan.careless], 3);
    }

    /// A small run without Tor's delays goes through, and no throwaway reply
    /// is let in.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_small_run_goes_through() {
        let dir = tempfile::tempdir().unwrap();
        let report = run(&small(dir.path().join("run"))).await.unwrap();
        println!("{}", report.summary());
        assert_eq!(report.nodes, 56);
        assert!(report.marks()[0].1, "{}", report.summary());
        assert!(
            report
                .outcomes
                .iter()
                .any(|o| o.kind == Kind::Throwaway && o.meeting.is_some())
        );
        // Every popular replier gathered a side; nobody asked more than the
        // node's own limit.
        let popular: HashSet<usize> = report
            .outcomes
            .iter()
            .filter(|o| o.kind == Kind::Popular)
            .map(|o| o.replier)
            .collect();
        assert!(
            report
                .gatherings
                .iter()
                .filter(|g| popular.contains(&g.replier))
                .all(|g| g.side > 0)
        );
        assert!(report.max_gather_asked() <= 80);
        assert!(report.max_asked() <= MAX_QUESTIONS);
    }
}
