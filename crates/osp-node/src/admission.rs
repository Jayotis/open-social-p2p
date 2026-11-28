//! Letting a stranger's reply in through who vouched for them (ADR 0029), on
//! the creator's side.
//!
//! Jimmy replies to Kim's post. He isn't in Kim's circle, so under the post's
//! `network` policy his reply waits among Kim's requests, and with `vouchers`
//! on Kim's node answers "show who vouched for you". Jimmy's node sends his
//! side, gathered in the background (`vouches.rs`): who vouched for him, who
//! vouched for those, and so on, 3 steps back. Kim's node checks each vouch
//! and works out the steps itself.
//!
//! Then it looks for where the two sides meet. On its own it places Kim's
//! circle at 1 step and whoever her circle reshared in the last 90 days at 2.
//! If that isn't enough, it asks up to 16 of her circle's nodes which of
//! Jimmy's side they place within 2 steps of their owners: through Alice's
//! node, a meeting is at 1 + Alice's steps. Each different person met is one
//! meeting point, and the farther out, the more are needed (`meet`).
//!
//! Every placing has a root: the person in Kim's circle it comes through.
//! Alice herself, someone Alice reshared, and whatever Alice's node answers
//! are all rooted at Alice, whether Kim's node found them or Alice's did, so
//! one careless friend never counts as two places. From 4 steps out, the
//! meeting points must come through at least 2 different roots. At the mark
//! the reply is published; below it, it stays a request with what was found.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{EndpointAddr, EndpointId, PublicKey};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_proto::{
    ObjectId, SignedObject,
    connected::{Hop, How, Path},
    social::{MAX_SIDE_VOUCHES, PlacedAt, ReplyPolicy, ReplyState, SocialRequest, SocialResponse},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::{
    db::{Db, Held, now_ms},
    node::Core,
    vouches::vouch_parts,
};

/// How far back from the replier their side reaches.
const SIDE_STEPS: u8 = 3;
/// How far back the circle's reshares place someone at 2 steps.
pub(crate) const RESHARES_PLACE_FOR: Duration = Duration::from_secs(90 * 24 * 60 * 60);
/// Circle nodes asked about one reply, at most.
pub(crate) const MAX_CIRCLE_ASKED: usize = 16;
/// "Which can you place" answered for one node an hour.
pub(crate) const MAX_PLACE_QUESTIONS_PER_HOUR: u32 = 60;
/// How long the circle's nodes get to answer, all together: the replier's
/// node waits for the answer to its `ShowVouches`.
pub(crate) const CIRCLE_WAIT: Duration = Duration::from_secs(30);

/// One person on Kim's side: `person`, `steps` steps from Kim, through
/// `root`, the person in her circle it comes through, whichever node found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placing {
    pub root: PublicKey,
    pub person: PublicKey,
    pub steps: u8,
    pub via: Via,
}

/// How a placing was found, which is how its path is drawn (ADR 0037 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Via {
    /// `person` is `root`, in Kim's circle.
    Circle,
    /// Kim's node holds `root`'s reshare of `person`.
    Reshared,
    /// `root`'s node placed `person` within `steps` of its owner.
    Asked { steps: u8 },
}

/// One separate chain of vouches, from a meeting point to the replier: each
/// hop to whom the one before vouched for, the last to the replier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chain {
    pub point: PublicKey,
    pub hops: Vec<Hop>,
}

/// What a meeting found, and the chains it counted.
#[derive(Debug, Clone)]
pub(crate) struct Met {
    pub found: Found,
    pub chains: Vec<Chain>,
}

/// One meeting of a replier's side with the owner's world, as the simulator
/// reads it back (`Node::meetings`).
#[cfg(feature = "sim")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meeting {
    pub replier: PublicKey,
    /// People on the side that counted.
    pub side: usize,
    /// Circle nodes asked, whether or not they answered.
    pub asked: usize,
    /// Of them, those that answered in time.
    pub answered: usize,
    /// The node's own work: checking the side and meeting it, without
    /// waiting on the circle's answers.
    pub own_work: Duration,
    pub steps: Option<u8>,
    pub points: usize,
    pub through: usize,
    pub let_in: bool,
    /// For each distance from Kim, 0 to 6: the separate meeting points
    /// within it, and the people in her circle they come through. So the
    /// simulator can see what other tables would have let in.
    pub within: [(usize, usize); 7],
    /// The paths kept for "How are we connected?" (ADR 0037).
    pub paths: Vec<Path>,
    /// The circle's answers: (whose node, whom it placed, steps from its
    /// owner).
    pub answers: Vec<(PublicKey, PublicKey, u8)>,
    /// The vouches on the side that counted: (voucher, vouched for, kind).
    pub vouches: Vec<(PublicKey, PublicKey, How)>,
}

/// What the meeting of a replier's side with the owner's world found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct Found {
    /// The fewest steps from the owner to the replier through a meeting
    /// point, if there's one.
    pub steps: Option<u8>,
    /// Different meeting points, at any distance.
    pub points: usize,
    /// Different people in the owner's circle the meeting points come
    /// through.
    pub through: usize,
    /// Whether that's enough to let the reply in.
    pub let_in: bool,
}

impl Found {
    /// What the owner is told of a request with it.
    pub(crate) fn describe(&self) -> String {
        match (self.steps, self.points) {
            (Some(steps), 1) => {
                format!("{steps} steps away; 1 person in your circle's world vouched for them")
            }
            (Some(steps), points) => format!(
                "{steps} steps away; {points} people in your circle's world vouched for them"
            ),
            (None, _) => "nobody in your circle's world vouched for them".to_string(),
        }
    }
}

/// Whether `points` meeting points, through `roots` different people in
/// Kim's circle, are enough at `distance` steps. ADR 0029's table.
fn enough(distance: u8, points: usize, roots: usize) -> bool {
    match distance {
        0..=3 => points >= 3,
        4 => points >= 4 && roots >= 2,
        _ => points >= 6 && roots >= 2,
    }
}

/// The meetings of `side` with the owner's world as `placings` place it:
/// each person met, their distance from the owner through it, and its root.
fn meetings(side: &Side, placings: &[Placing]) -> Vec<(PublicKey, u8, PublicKey)> {
    placings
        .iter()
        .filter_map(|p| {
            side.steps
                .get(&p.person)
                .map(|b| (p.person, p.steps + b, p.root))
        })
        .collect()
}

/// The separate chains from meeting points, and the roots they come
/// through, among the meetings within `distance`.
fn count_within(
    side: &Side,
    meetings: &[(PublicKey, u8, PublicKey)],
    distance: u8,
) -> (Vec<Chain>, usize) {
    let within: Vec<_> = meetings.iter().filter(|m| m.1 <= distance).collect();
    let people: HashSet<PublicKey> = within.iter().map(|m| m.0).collect();
    let roots = within.iter().map(|m| m.2).collect::<HashSet<_>>().len();
    (side.separate_chains(&people), roots)
}

/// `count_within` at each distance from 0 to 6, counted.
#[cfg(feature = "sim")]
fn within(side: &Side, placings: &[Placing]) -> [(usize, usize); 7] {
    let meetings = meetings(side, placings);
    std::array::from_fn(|d| {
        let (chains, roots) = count_within(side, &meetings, d as u8);
        (chains.len(), roots)
    })
}

/// Where the replier's side meets the owner's world as `placings` place it.
/// Meeting points count only as far as their chains of vouches to the
/// replier are separate, sharing nobody but the replier
/// (`Side::separate_chains`): throwaway identities behind one careless
/// reshare, and whoever vouched for the careless person, all reach the
/// replier through that one person, so they count as one. The chains
/// counted for `points` come back with it, so what's shown is what counted.
pub(crate) fn meet(side: &Side, placings: &[Placing]) -> Met {
    let meetings = meetings(side, placings);
    let (chains, through) = count_within(side, &meetings, u8::MAX);
    let mut found = Found {
        steps: meetings.iter().map(|m| m.1).min(),
        points: chains.len(),
        through,
        let_in: false,
    };
    let farthest = meetings.iter().map(|m| m.1).max().unwrap_or(0);
    for distance in found.steps.unwrap_or(u8::MAX)..=farthest {
        let (within, roots) = count_within(side, &meetings, distance);
        if enough(distance, within.len(), roots) {
            found.let_in = true;
            break;
        }
    }
    Met { found, chains }
}

/// The paths from the owner along `chains` (ADR 0037): for each, the
/// nearest placing of its meeting point, the owner's own evidence first
/// where it's as near, then the chain. `known` are the people the owner
/// knows; the rest of the circle they follow. Nearest paths first.
pub(crate) fn paths(
    chains: &[Chain],
    placings: &[Placing],
    known: &HashSet<PublicKey>,
) -> Vec<Path> {
    let mut paths: Vec<Path> = chains
        .iter()
        .filter_map(|chain| {
            let placing = placings
                .iter()
                .filter(|p| p.person == chain.point)
                .min_by_key(|p| (p.steps, p.via))?;
            let mut hops = owner_hops(placing, known);
            hops.extend(chain.hops.iter().copied());
            Some(Path { hops })
        })
        .collect();
    paths.sort_by_key(|p| {
        (
            p.steps(),
            p.hops.iter().map(|h| h.to.to_string()).collect::<Vec<_>>(),
        )
    });
    paths
}

/// The hops from the owner to a placing's person.
pub(crate) fn owner_hops(placing: &Placing, known: &HashSet<PublicKey>) -> Vec<Hop> {
    let first = Hop {
        to: placing.root,
        how: match known.contains(&placing.root) {
            true => How::Knows,
            false => How::Follows,
        },
    };
    let rest = match placing.via {
        Via::Circle | Via::Asked { steps: 0 } => None,
        Via::Reshared => Some(How::Reshared),
        Via::Asked { steps } => Some(How::Gap { steps }),
    };
    std::iter::once(first)
        .chain(rest.map(|how| Hop {
            to: placing.person,
            how,
        }))
        .collect()
}

/// The vouches of a carried side that count, each as (voucher, vouched
/// for, kind): those that verify and aren't signed by a key this node knows
/// was revoked. Whether each is for whom the side needs is `Side::new`'s
/// part.
pub(crate) fn counted_vouches(
    db: &Db,
    side: &[SignedObject],
) -> Result<Vec<(PublicKey, PublicKey, How)>> {
    let mut edges = Vec::new();
    for signed in side {
        match vouch_parts(signed) {
            Ok(edge) if !db.signed_by_revoked_key(signed)? => edges.push(edge),
            Ok(_) => debug!("a carried vouch was signed by a revoked key"),
            Err(e) => debug!("a carried vouch doesn't check out: {e:#}"),
        }
    }
    Ok(edges)
}

/// A replier's side as carried, once checked.
#[derive(Debug, Clone)]
pub(crate) struct Side {
    pub replier: PublicKey,
    /// The people it places, each with their steps back from the replier.
    pub steps: HashMap<PublicKey, u8>,
    /// Its vouches among them, each as (voucher, vouched for, kind): one
    /// for each pair, the first given.
    edges: Vec<(PublicKey, PublicKey, How)>,
}

impl Side {
    /// The people `edges` place, each with their steps back from `replier`:
    /// those whose vouch is for the replier are 1, those whose vouch is for
    /// one of those 2, and so on to `SIDE_STEPS`. Vouches that don't chain
    /// to the replier place nobody. `edges` are (voucher, vouched for, kind).
    pub(crate) fn new(replier: PublicKey, edges: &[(PublicKey, PublicKey, How)]) -> Self {
        let mut steps = HashMap::new();
        let mut frontier = HashSet::from([replier]);
        for step in 1..=SIDE_STEPS {
            let mut next = HashSet::new();
            for (voucher, subject, _) in edges {
                if frontier.contains(subject) && *voucher != replier && !steps.contains_key(voucher)
                {
                    next.insert(*voucher);
                }
            }
            for voucher in &next {
                steps.insert(*voucher, step);
            }
            frontier = next;
        }
        let mut pairs = HashSet::new();
        let edges = edges
            .iter()
            .filter(|(v, s, _)| steps.contains_key(v) && (*s == replier || steps.contains_key(s)))
            .filter(|(v, s, _)| pairs.insert((*v, *s)))
            .copied()
            .collect();
        Self {
            replier,
            steps,
            edges,
        }
    }

    /// The vouches that count, as (voucher, vouched for, kind).
    #[cfg(feature = "sim")]
    pub(crate) fn edges(&self) -> &[(PublicKey, PublicKey, How)] {
        &self.edges
    }

    /// The most chains of vouches to the replier, each from one of `people`,
    /// that can be separate, sharing nobody but the replier: the most paths
    /// that pass through no one twice. Found as a maximum flow with each
    /// person able to carry one path; each chain is then read off the flow.
    pub(crate) fn separate_chains(&self, people: &HashSet<PublicKey>) -> Vec<Chain> {
        let order: Vec<PublicKey> = self.steps.keys().copied().collect();
        let index: HashMap<PublicKey, usize> =
            order.iter().enumerate().map(|(i, p)| (*p, i)).collect();
        let n = index.len();
        // Each person i is two nodes, 2i in and 2i + 1 out, joined by one
        // path's room. Then the source, and the replier as the sink.
        let (source, sink) = (2 * n, 2 * n + 1);
        let mut flow = Flow::new(2 * n + 2);
        for (person, &i) in &index {
            flow.add(2 * i, 2 * i + 1, None);
            if people.contains(person) {
                flow.add(source, 2 * i, None);
            }
        }
        for (voucher, subject, how) in &self.edges {
            let from = 2 * index[voucher] + 1;
            match index.get(subject) {
                Some(&s) => flow.add(from, 2 * s, Some(*how)),
                None => flow.add(from, sink, Some(*how)),
            }
        }
        flow.max(source, sink);
        // Each path leaves the source for a meeting point's in-node, crosses
        // to its out-node, and goes on by the one link it used, to the next
        // person's in-node or to the replier.
        let mut chains = Vec::new();
        for start in flow.used(source) {
            let point = order[start / 2];
            let mut hops = Vec::new();
            let mut at = start + 1;
            while let Some((to, how)) = flow.used_from(at) {
                let how = how.expect("a vouch's link");
                if to == sink {
                    hops.push(Hop {
                        to: self.replier,
                        how,
                    });
                    break;
                }
                hops.push(Hop {
                    to: order[to / 2],
                    how,
                });
                at = to + 1;
            }
            chains.push(Chain { point, hops });
        }
        chains
    }
}

/// One link of a flow network: where to, room left, the reverse link's
/// index, and, for a vouch's link, its kind.
#[derive(Clone)]
struct Link {
    to: usize,
    room: u8,
    back: usize,
    forward: bool,
    how: Option<How>,
}

/// A flow network where every link carries one path.
struct Flow {
    links: Vec<Vec<Link>>,
}

impl Flow {
    fn new(nodes: usize) -> Self {
        Self {
            links: vec![Vec::new(); nodes],
        }
    }

    fn add(&mut self, from: usize, to: usize, how: Option<How>) {
        let (back, forth) = (self.links[to].len(), self.links[from].len());
        self.links[from].push(Link {
            to,
            room: 1,
            back,
            forward: true,
            how,
        });
        self.links[to].push(Link {
            to: from,
            room: 0,
            back: forth,
            forward: false,
            how: None,
        });
    }

    /// The nodes the links out of `at` that carry a path lead to.
    fn used(&self, at: usize) -> Vec<usize> {
        self.links[at]
            .iter()
            .filter(|l| l.forward && l.room == 0)
            .map(|l| l.to)
            .collect()
    }

    /// The one link out of `at` that carries a path, where `at` carries at
    /// most one: where it leads, and its kind.
    fn used_from(&self, at: usize) -> Option<(usize, Option<How>)> {
        self.links[at]
            .iter()
            .find(|l| l.forward && l.room == 0)
            .map(|l| (l.to, l.how))
    }

    /// Paths found one at a time, each the shortest with room left.
    fn max(&mut self, source: usize, sink: usize) -> usize {
        let mut paths = 0;
        loop {
            let mut came: Vec<Option<(usize, usize)>> = vec![None; self.links.len()];
            let mut queue = std::collections::VecDeque::from([source]);
            while let Some(at) = queue.pop_front() {
                if at == sink {
                    break;
                }
                for (k, link) in self.links[at].iter().enumerate() {
                    if link.room > 0 && link.to != source && came[link.to].is_none() {
                        came[link.to] = Some((at, k));
                        queue.push_back(link.to);
                    }
                }
            }
            if came[sink].is_none() {
                return paths;
            }
            let mut at = sink;
            while let Some((from, k)) = came[at] {
                let back = self.links[from][k].back;
                self.links[from][k].room -= 1;
                self.links[at][back].room += 1;
                at = from;
            }
            paths += 1;
        }
    }
}

impl Core {
    /// Whether a stranger's request may be let in through who vouched for
    /// them: with `vouchers` on, under the `network` policy.
    pub(crate) fn asks_for_side(&self, policy: ReplyPolicy) -> bool {
        self.config.replies.vouchers && policy == ReplyPolicy::Network
    }

    /// The replier's node of the request `reply` shows who vouched for its
    /// owner: their `side`, asked for when the reply came. Checks each
    /// vouch, looks for where the side meets the owner's world, and
    /// publishes the reply if that's enough. Taken once per reply.
    pub(crate) async fn shown_vouches(
        self: &Arc<Self>,
        remote: EndpointId,
        reply: &ObjectId,
        side: &[SignedObject],
    ) -> Result<ReplyState> {
        ensure!(
            side.len() <= MAX_SIDE_VOUCHES,
            "{} vouches; at most {MAX_SIDE_VOUCHES}",
            side.len()
        );
        let row = self
            .db
            .received_reply(reply)?
            .filter(|row| row.reply.node == remote)
            .with_context(|| {
                format!("no reply {} from your node is kept here", reply.fmt_short())
            })?;
        if row.held != Held::Request {
            return Ok(match row.held {
                Held::Published => ReplyState::Published,
                _ => ReplyState::NotPublished,
            });
        }
        let policy = self.reply_policy(&row.reply.post)?;
        ensure!(
            self.asks_for_side(policy) && self.db.take_side(reply)?,
            "this node didn't ask who vouched for that reply's replier"
        );
        let replier = row.reply.actor;
        let started = std::time::Instant::now();
        let carried = Side::new(replier, &counted_vouches(&self.db, side)?);
        let people: HashSet<PublicKey> = carried.steps.keys().copied().collect();
        let mut placings = self.rooted_near_owner(&people)?;
        let mut met = meet(&carried, &placings);
        let mut own_work = started.elapsed();
        let (mut asked, mut answered) = (0, 0);
        if !met.found.let_in && !people.is_empty() {
            let answers;
            (asked, answers) = self.ask_circle(&people).await?;
            answered = answers.len();
            let again = std::time::Instant::now();
            for (root, placed) in answers {
                placings.extend(placed.into_iter().map(|p| Placing {
                    root,
                    person: p.actor,
                    steps: 1 + p.steps,
                    via: Via::Asked { steps: p.steps },
                }));
            }
            met = meet(&carried, &placings);
            own_work += again.elapsed();
        }
        let found = met.found;
        let paths = paths(&met.chains, &placings, &self.known()?);
        self.db.set_found(
            reply,
            &serde_json::to_string(&found)?,
            &serde_json::to_string(&paths)?,
        )?;
        info!(
            replier = %replier.fmt_short(),
            side = carried.steps.len(),
            ?found,
            asked,
            answered,
            own_work_us = own_work.as_micros() as u64,
            "met a replier's side"
        );
        #[cfg(feature = "sim")]
        self.meetings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Meeting {
                replier,
                side: carried.steps.len(),
                asked,
                answered,
                own_work,
                steps: found.steps,
                points: found.points,
                through: found.through,
                let_in: found.let_in,
                within: within(&carried, &placings),
                paths,
                answers: placings
                    .iter()
                    .filter_map(|p| match p.via {
                        Via::Asked { steps } => Some((p.root, p.person, steps)),
                        _ => None,
                    })
                    .collect(),
                vouches: carried.edges().to_vec(),
            });
        if !found.let_in {
            return Ok(ReplyState::AwaitingApproval);
        }
        // The owner may have answered meanwhile.
        match self.db.received_reply(reply)?.map(|row| row.held) {
            Some(Held::Request) => {}
            Some(Held::Published) => return Ok(ReplyState::Published),
            _ => return Ok(ReplyState::NotPublished),
        }
        match self.db.move_reply(reply, Held::Published, now_ms()) {
            Ok(_) => Ok(ReplyState::Published),
            // A full thread: it waits as a request.
            Err(e) => {
                debug!("couldn't publish a reply let in by its vouchers: {e:#}");
                Ok(ReplyState::AwaitingApproval)
            }
        }
    }

    /// Those of `identities` this node places near its owner, each with
    /// every root it comes through: those they follow or trust at 1 step,
    /// rooted at themselves, and those whom someone they follow reshared in
    /// the last 90 days at 2, rooted at that resharer. The owner's own
    /// reshares place nobody.
    pub(crate) fn rooted_near_owner(
        &self,
        identities: &HashSet<PublicKey>,
    ) -> Result<Vec<Placing>> {
        let me = self.signer.identity();
        let mut placings = Vec::new();
        let circle: HashSet<PublicKey> = self
            .db
            .follows()?
            .into_iter()
            .map(|f| f.actor)
            .chain(self.db.trusted()?.into_iter().map(|t| t.actor))
            .collect();
        for actor in &circle {
            if identities.contains(actor) {
                placings.push(Placing {
                    root: *actor,
                    person: *actor,
                    steps: 1,
                    via: Via::Circle,
                });
            }
        }
        let since = now_ms() - RESHARES_PLACE_FOR.as_millis() as i64;
        let reshares: HashSet<(PublicKey, PublicKey)> = self
            .db
            .followed_reshared_since(since)?
            .into_iter()
            .collect();
        for (resharer, author) in reshares {
            if identities.contains(&author) && author != me {
                placings.push(Placing {
                    root: resharer,
                    person: author,
                    steps: 2,
                    via: Via::Reshared,
                });
            }
        }
        Ok(placings)
    }

    /// The people the owner knows (trusted, ADR 0013).
    pub(crate) fn known(&self) -> Result<HashSet<PublicKey>> {
        Ok(self.db.trusted()?.into_iter().map(|t| t.actor).collect())
    }

    /// Those of `identities` this node places within 2 steps of its owner,
    /// each at its nearest: the owner at 0, then as `rooted_near_owner`.
    pub(crate) fn place_near_owner(
        &self,
        identities: &HashSet<PublicKey>,
    ) -> Result<Vec<PlacedAt>> {
        let me = self.signer.identity();
        let mut placed: HashMap<PublicKey, u8> = HashMap::new();
        if identities.contains(&me) {
            placed.insert(me, 0);
        }
        for p in self.rooted_near_owner(identities)? {
            let steps = placed.entry(p.person).or_insert(p.steps);
            *steps = (*steps).min(p.steps);
        }
        let mut placed: Vec<PlacedAt> = placed
            .into_iter()
            .map(|(actor, steps)| PlacedAt { actor, steps })
            .collect();
        placed.sort_by_key(|p| (p.steps, p.actor.to_string()));
        Ok(placed)
    }

    /// The node `remote` asks which of `identities` this node places within
    /// 2 steps of its owner. Answered only for the node of someone the owner
    /// follows, or of a follower, at most `MAX_PLACE_QUESTIONS_PER_HOUR`;
    /// never asked of anyone else.
    pub(crate) fn asked_to_place(
        &self,
        remote: EndpointId,
        identities: &[PublicKey],
    ) -> Result<Vec<PlacedAt>> {
        ensure!(
            identities.len() <= MAX_SIDE_VOUCHES,
            "asked about {} identities; at most {MAX_SIDE_VOUCHES}",
            identities.len()
        );
        let followed = self
            .db
            .follows()?
            .iter()
            .any(|f| f.addr.id == remote || self.nodes_of(&f.actor).contains(&remote));
        ensure!(
            followed || self.db.is_follower(&remote)?,
            "this node answers only the nodes of those its owner follows, or who follow them"
        );
        ensure!(
            self.place_questions
                .allow(remote, MAX_PLACE_QUESTIONS_PER_HOUR),
            "this node answers no more of your questions this hour; try again later"
        );
        let identities: HashSet<PublicKey> = identities.iter().copied().collect();
        self.place_near_owner(&identities)
    }

    /// Asks up to `MAX_CIRCLE_ASKED` nodes of people in the owner's circle,
    /// trusted followers first, then those followed longest, which of
    /// `people` they place near their owners. Each person once; those that
    /// don't answer within `CIRCLE_WAIT` are passed over. Gives how many
    /// were asked, and the answers.
    async fn ask_circle(
        self: &Arc<Self>,
        people: &HashSet<PublicKey>,
    ) -> Result<(usize, Vec<(PublicKey, Vec<PlacedAt>)>)> {
        let mut ask: Vec<(PublicKey, EndpointId)> = Vec::new();
        let candidates = self
            .db
            .trusted()?
            .into_iter()
            .map(|t| (t.actor, t.node))
            .chain(self.db.followed_longest()?);
        for (person, node) in candidates {
            if ask.len() >= MAX_CIRCLE_ASKED {
                break;
            }
            if !ask.iter().any(|(p, _)| *p == person) {
                ask.push((person, node));
            }
        }
        let identities: Vec<PublicKey> = people.iter().copied().collect();
        let asked = ask.len();
        let mut asks = stream::iter(ask)
            .map(|(person, node)| {
                let core = self.clone();
                let identities = identities.clone();
                async move {
                    let answer = core.ask_place_within(node, identities).await;
                    (person, node, answer)
                }
            })
            .buffered_unordered(MAX_CIRCLE_ASKED);
        // Each answer as it comes: one node that never answers, say over
        // Tor for longer than `CIRCLE_WAIT`, loses only its own.
        let deadline = tokio::time::Instant::now() + CIRCLE_WAIT;
        let mut answers = Vec::new();
        while let Ok(Some(answer)) = tokio::time::timeout_at(deadline, asks.next()).await {
            answers.push(answer);
        }
        let mut placed = Vec::new();
        for (person, node, answer) in answers {
            match answer {
                Ok(answer) => {
                    // Only those asked about, and within 2 steps.
                    let answer: Vec<PlacedAt> = answer
                        .into_iter()
                        .filter(|p| p.steps <= 2 && people.contains(&p.actor))
                        .collect();
                    placed.push((person, answer));
                }
                Err(e) => debug!(
                    node = %node.fmt_short(),
                    "a circle node didn't say whom it places: {e:#}"
                ),
            }
        }
        Ok((asked, placed))
    }

    async fn ask_place_within(
        &self,
        node: EndpointId,
        identities: Vec<PublicKey>,
    ) -> Result<Vec<PlacedAt>> {
        let req = SocialRequest::PlaceWithin { identities };
        match self.social_request(&EndpointAddr::new(node), &req).await? {
            SocialResponse::Placed { placed } => Ok(placed),
            other => bail!("unexpected reply to PlaceWithin: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;
    use osp_proto::{EarlierKeys, Payload, Vouch, VouchKind, object::CertifiedKey};

    use super::*;

    fn people(n: usize) -> Vec<PublicKey> {
        (0..n).map(|_| SecretKey::generate().public()).collect()
    }

    /// Vouches putting each of `points` `back` steps back from `replier`,
    /// each through a chain of their own.
    fn chains(replier: PublicKey, points: &[PublicKey], back: u8) -> Vec<(PublicKey, PublicKey)> {
        let mut edges = Vec::new();
        for p in points {
            let mut at = *p;
            for _ in 1..back {
                let next = key();
                edges.push((at, next));
                at = next;
            }
            edges.push((at, replier));
        }
        edges
    }

    /// A side of `points` people, each `back` steps back from the replier
    /// through a chain of their own.
    fn side_of(points: &[PublicKey], back: u8) -> Side {
        let replier = key();
        side_from(replier, &chains(replier, points, back))
    }

    fn placed(root: PublicKey, points: &[PublicKey], steps: u8) -> Vec<Placing> {
        points
            .iter()
            .map(|p| Placing {
                root,
                person: *p,
                steps,
                via: Via::Asked { steps: steps - 1 },
            })
            .collect()
    }

    fn key() -> PublicKey {
        SecretKey::generate().public()
    }

    /// A side whose vouches are all reshares, as (voucher, vouched for).
    fn side_from(replier: PublicKey, pairs: &[(PublicKey, PublicKey)]) -> Side {
        let edges: Vec<_> = pairs.iter().map(|(v, s)| (*v, *s, How::Reshared)).collect();
        Side::new(replier, &edges)
    }

    fn found_of(side: &Side, placings: &[Placing]) -> Found {
        meet(side, placings).found
    }

    #[test]
    fn at_each_distance_one_short_of_the_mark_isnt_let_in() {
        let (alice, bob) = (key(), key());
        // 2 or 3 steps: 3 points, through anyone.
        for (kim_steps, back) in [(1, 1), (2, 1), (1, 2)] {
            let p = people(3);
            let side = side_of(&p, back);
            let found = found_of(&side, &placed(alice, &p, kim_steps));
            assert!(found.let_in, "{kim_steps} + {back}");
            assert_eq!(found.steps, Some(kim_steps + back));
            let found = found_of(&side, &placed(alice, &p[..2], kim_steps));
            assert!(!found.let_in, "{kim_steps} + {back}");
        }
        // 4 steps: 4 points, through 2 people in Kim's circle.
        let p = people(4);
        let side = side_of(&p, 2);
        let mut both = placed(alice, &p[..2], 2);
        both.extend(placed(bob, &p[2..], 2));
        assert!(found_of(&side, &both).let_in);
        assert!(!found_of(&side, &both[1..]).let_in);
        // 5 or 6 steps: 6 points, through 2 people in her circle.
        for (kim_steps, back) in [(2, 3), (3, 3)] {
            let p = people(6);
            let side = side_of(&p, back);
            let mut both = placed(alice, &p[..3], kim_steps);
            both.extend(placed(bob, &p[3..], kim_steps));
            let found = found_of(&side, &both);
            assert!(found.let_in, "{kim_steps} + {back}");
            assert_eq!((found.points, found.through), (6, 2));
            assert!(!found_of(&side, &both[1..]).let_in, "{kim_steps} + {back}");
        }
    }

    #[test]
    fn through_only_one_person_in_the_circle_four_steps_and_more_arent_let_in() {
        let alice = key();
        for (kim_steps, back) in [(2, 2), (3, 2), (3, 3)] {
            let p = people(20);
            let found = found_of(&side_of(&p, back), &placed(alice, &p, kim_steps));
            assert!(!found.let_in, "{kim_steps} + {back}");
            assert_eq!((found.points, found.through), (20, 1));
        }
        // From 3 steps, one is enough.
        let p = people(3);
        assert!(found_of(&side_of(&p, 2), &placed(alice, &p, 1)).let_in);
    }

    /// Kim follows Alice, who reshared X. Kim's node places X at 2 steps
    /// through Alice, and Alice's node places X and four more at 1 step from
    /// her: five meeting points at 4 steps, every one rooted at Alice. Kim's
    /// node finding some of them itself doesn't make a second root.
    #[test]
    fn what_kims_node_finds_through_alice_and_what_alices_node_answers_are_one_root() {
        let (alice, dave, x) = (key(), key(), key());
        let others = people(4);
        let jimmy = key();
        let mut edges = chains(jimmy, &others, 2);
        edges.extend(chains(jimmy, &[x], 2));
        let side = side_from(jimmy, &edges);
        // Kim's node: Alice at 1 (not on the side), X reshared by Alice at 2.
        let mut placings = placed(alice, &[alice], 1);
        placings.extend(placed(alice, &[x], 2));
        // Alice's node: X and the others at 1 from her, 2 from Kim.
        placings.extend(placed(alice, &[x], 2));
        placings.extend(placed(alice, &others, 2));
        let found = found_of(&side, &placings);
        assert_eq!((found.steps, found.points, found.through), (Some(4), 5, 1));
        assert!(!found.let_in);
        // Dave's node places one of them too: a second root, and it's in.
        placings.extend(placed(dave, &[others[0]], 2));
        let found = found_of(&side, &placings);
        assert_eq!((found.points, found.through), (5, 2));
        assert!(found.let_in);
    }

    #[test]
    fn nearer_meetings_count_toward_a_farther_mark() {
        let (alice, bob) = (key(), key());
        // 2 at 2 steps and 1 at 4 through Alice, 2 at 4 steps through Bob.
        let near = people(2);
        let far = people(3);
        let jimmy = key();
        let mut edges = chains(jimmy, &near, 1);
        edges.extend(chains(jimmy, &far, 2));
        let side = side_from(jimmy, &edges);
        let mut placings = placed(alice, &near, 1);
        placings.extend(placed(alice, &far[..1], 2));
        placings.extend(placed(bob, &far[1..], 2));
        let found = found_of(&side, &placings);
        assert!(found.let_in);
        assert_eq!((found.steps, found.points), (Some(2), 5));
    }

    #[test]
    fn many_vouches_behind_one_careless_reshare_meet_at_one_point() {
        let jimmy = key();
        let careless = key();
        let throwaways = people(200);
        // The throwaways vouch for each other and for Jimmy; the careless
        // one reshared one of them.
        let mut edges: Vec<(PublicKey, PublicKey)> = Vec::new();
        for (i, t) in throwaways.iter().enumerate() {
            edges.push((*t, jimmy));
            edges.push((*t, throwaways[(i + 1) % throwaways.len()]));
        }
        edges.push((careless, throwaways[0]));
        let side = side_from(jimmy, &edges);
        assert_eq!(side.steps.len(), 201);
        assert_eq!(side.steps[&careless], 2);
        // Kim's side holds the careless one, through everyone in her circle.
        let mut placings = Vec::new();
        for _ in 0..16 {
            placings.extend(placed(key(), &[careless], 2));
        }
        let found = found_of(&side, &placings);
        assert_eq!(found.points, 1);
        assert!(!found.let_in);
    }

    /// The careless person is well vouched-for near Kim. Those who vouched
    /// for them are on the operator's side too, 3 steps back, and Kim's
    /// world places them all, through many people in her circle; but every
    /// chain from them to the operator goes through the careless person.
    #[test]
    fn whoever_vouched_for_the_careless_person_meets_at_that_one_point() {
        let operator = key();
        let careless = key();
        let throwaways = people(200);
        let behind = people(30);
        let mut edges: Vec<(PublicKey, PublicKey)> = Vec::new();
        for (i, t) in throwaways.iter().enumerate() {
            edges.push((*t, operator));
            edges.push((*t, throwaways[(i + 1) % throwaways.len()]));
        }
        edges.push((careless, throwaways[0]));
        edges.extend(behind.iter().map(|b| (*b, careless)));
        let side = side_from(operator, &edges);
        assert!(behind.iter().all(|b| side.steps[b] == 3));
        let mut placings = placed(key(), &[careless], 2);
        for b in &behind {
            placings.extend(placed(key(), &[*b], 2));
        }
        let found = found_of(&side, &placings);
        assert_eq!((found.steps, found.points), (Some(4), 1));
        assert!(found.through > 2);
        assert!(!found.let_in);
    }

    /// Popular Jimmy: ten people vouched for him, and for each of them
    /// someone Kim's world places. Ten separate chains, ten points. If those
    /// ten had all vouched only for Bob, who vouched for Jimmy, it'd be one.
    #[test]
    fn meeting_points_count_as_far_as_their_chains_are_separate() {
        let (alice, dave) = (key(), key());
        let jimmy = key();
        let met = people(10);
        let side = side_from(jimmy, &chains(jimmy, &met, 3));
        let mut placings = placed(alice, &met[..5], 2);
        placings.extend(placed(dave, &met[5..], 2));
        let found = found_of(&side, &placings);
        assert_eq!((found.steps, found.points), (Some(5), 10));
        assert!(found.let_in);

        let bob = key();
        let mut edges = vec![(bob, jimmy)];
        edges.extend(met.iter().map(|m| (*m, bob)));
        let found = found_of(&side_from(jimmy, &edges), &placings);
        assert_eq!((found.steps, found.points), (Some(4), 1));
        assert!(!found.let_in);
        // Bob placed himself, as well: he and those behind him share Bob.
        placings.extend(placed(alice, &[bob], 2));
        assert_eq!(found_of(&side_from(jimmy, &edges), &placings).points, 1);
    }

    #[test]
    fn a_side_places_only_those_who_chain_to_the_replier_nearest_first() {
        let [jimmy, bob, carol, dan, eve, frank, stray] = people(7).try_into().unwrap();
        let edges = [
            (bob, jimmy),
            (carol, bob),
            (dan, carol),
            (eve, dan),
            // Four back: past the side.
            (frank, eve),
            // Also vouched for Jimmy: 1, not 2.
            (carol, jimmy),
            // For someone not on the side.
            (stray, SecretKey::generate().public()),
            // Jimmy vouching for his own vouchers places nobody.
            (jimmy, bob),
        ];
        let side = side_from(jimmy, &edges).steps;
        assert_eq!(
            side,
            HashMap::from([(bob, 1), (carol, 1), (dan, 2), (eve, 3)])
        );
    }

    /// `by`'s vouch for `of`, valid a day.
    fn vouch(by: PublicKey, of: PublicKey) -> Payload {
        Payload::Vouch(Vouch {
            actor: by,
            subject: of,
            kind: VouchKind::Reshare,
            valid_until: osp_proto::rfc3339_from_ms(
                (crate::db::now_ms() + 24 * 60 * 60 * 1000) as u64,
            ),
            published: osp_proto::now_rfc3339(),
        })
    }

    #[test]
    fn a_vouch_signed_by_a_revoked_key_or_that_doesnt_verify_doesnt_count() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (jimmy, bob, carol) = (
            SecretKey::generate().public(),
            SecretKey::generate(),
            SecretKey::generate(),
        );
        let reshare = |of: PublicKey, by: PublicKey| vouch(by, of);
        // Bob's vouch is signed by a key he later revokes.
        let old_key = CertifiedKey::certify(
            &bob,
            SecretKey::generate(),
            "2026-01-01T00:00:00Z".into(),
            EarlierKeys::Retired,
        )
        .unwrap();
        let by_old_key =
            SignedObject::sign_certified(&old_key, &reshare(jimmy, bob.public())).unwrap();
        let good = SignedObject::sign(&carol, &reshare(jimmy, carol.public())).unwrap();
        let mut forged = serde_json::to_value(&good).unwrap();
        forged["signer"] = serde_json::to_value(bob.public()).unwrap();
        let forged: SignedObject = serde_json::from_value(forged).unwrap();
        let side = [by_old_key, good, forged];
        assert_eq!(
            counted_vouches(&db, &side).unwrap(),
            [
                (bob.public(), jimmy, How::Reshared),
                (carol.public(), jimmy, How::Reshared)
            ]
        );
        let new_key = CertifiedKey::certify(
            &bob,
            SecretKey::generate(),
            "2026-06-01T00:00:00Z".into(),
            EarlierKeys::Revoked,
        )
        .unwrap();
        let Payload::Key(key) = new_key.certificate().verify().unwrap() else {
            unreachable!()
        };
        let cert_ms = osp_proto::parse_rfc3339_ms(&key.published).unwrap();
        assert!(db.check_key(new_key.certificate(), &key, cert_ms).unwrap());
        assert_eq!(
            counted_vouches(&db, &side).unwrap(),
            [(carol.public(), jimmy, How::Reshared)]
        );
    }

    /// Where a meeting's own work goes, for a side of 256. Run by hand, in
    /// release: `cargo test --release -p osp-node --lib timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_a_full_side() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let jimmy = key();
        let side: Vec<SignedObject> = (0..256)
            .map(|_| {
                let v = SecretKey::generate();
                SignedObject::sign(&v, &vouch(v.public(), jimmy)).unwrap()
            })
            .collect();
        let t = std::time::Instant::now();
        for s in &side {
            s.verify().unwrap();
        }
        println!("verify: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        for s in &side {
            db.signed_by_revoked_key(s).unwrap();
        }
        println!("revoked checks: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let edges = counted_vouches(&db, &side).unwrap();
        println!("counted_vouches: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let steps = Side::new(jimmy, &edges);
        let placings: Vec<Placing> = steps
            .steps
            .keys()
            .map(|p| Placing {
                root: jimmy,
                person: *p,
                steps: 2,
                via: Via::Reshared,
            })
            .collect();
        let found = found_of(&steps, &placings);
        println!("Side::new and meet: {:?} ({found:?})", t.elapsed());
    }

    fn hop(to: PublicKey, how: How) -> Hop {
        Hop { to, how }
    }

    /// ADR 0037's paths, and one through a gap: each written hop by hop
    /// from what Kim's node holds, the side, and what a circle node said.
    #[test]
    fn the_paths_are_the_adrs_written_out() {
        let [
            jimmy,
            alice,
            carol,
            bob,
            david,
            sarah,
            emma,
            frank,
            dana,
            gus,
        ] = people(10).try_into().unwrap();
        let side = Side::new(
            jimmy,
            &[
                (carol, jimmy, How::Reshared),
                (david, jimmy, How::CreditedEarly),
                (sarah, emma, How::Reshared),
                (emma, frank, How::Reshared),
                (frank, jimmy, How::Reshared),
                (gus, jimmy, How::Reshared),
            ],
        );
        let placing = |root, person, steps, via| Placing {
            root,
            person,
            steps,
            via,
        };
        let placings = [
            // Kim's node holds Alice's reshare of Carol, and Bob's of David.
            placing(alice, carol, 2, Via::Reshared),
            placing(bob, david, 2, Via::Reshared),
            // Kim knows Sarah.
            placing(sarah, sarah, 1, Via::Circle),
            // Dana's node places Gus within 2 of Dana.
            placing(dana, gus, 3, Via::Asked { steps: 2 }),
        ];
        let met = meet(&side, &placings);
        assert_eq!(met.found.points, 4);
        let got: HashSet<Path> = paths(&met.chains, &placings, &HashSet::from([sarah]))
            .into_iter()
            .collect();
        let want = HashSet::from([
            Path {
                hops: vec![
                    hop(alice, How::Follows),
                    hop(carol, How::Reshared),
                    hop(jimmy, How::Reshared),
                ],
            },
            Path {
                hops: vec![
                    hop(bob, How::Follows),
                    hop(david, How::Reshared),
                    hop(jimmy, How::CreditedEarly),
                ],
            },
            Path {
                hops: vec![
                    hop(sarah, How::Knows),
                    hop(emma, How::Reshared),
                    hop(frank, How::Reshared),
                    hop(jimmy, How::Reshared),
                ],
            },
            Path {
                hops: vec![
                    hop(dana, How::Follows),
                    hop(gus, How::Gap { steps: 2 }),
                    hop(jimmy, How::Reshared),
                ],
            },
        ]);
        assert_eq!(got, want);
    }

    /// Where a meeting point is placed both by Kim's own node and through a
    /// circle node's answer, as near, her own evidence is drawn.
    #[test]
    fn a_path_takes_the_nearest_placing_and_the_owners_own_evidence_first() {
        let [jimmy, alice, carol] = people(3).try_into().unwrap();
        let side = Side::new(jimmy, &[(carol, jimmy, How::Reshared)]);
        let asked = Placing {
            root: alice,
            person: carol,
            steps: 2,
            via: Via::Asked { steps: 1 },
        };
        let own = Placing {
            via: Via::Reshared,
            ..asked
        };
        let farther = Placing {
            steps: 3,
            via: Via::Asked { steps: 2 },
            ..asked
        };
        let placings = [farther, asked, own];
        let met = meet(&side, &placings);
        let paths = paths(&met.chains, &placings, &HashSet::new());
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].hops[1], hop(carol, How::Reshared));
    }

    /// A small xorshift, so the worlds below are the same each run.
    struct Rand(u64);

    impl Rand {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    /// On random sides and placings, the chains handed back are as many as
    /// the meeting points counted, each starts at a placed person and runs
    /// along the side's own vouches, kind and all, to the replier, and no
    /// one but the replier is on two of them.
    #[test]
    fn the_chains_counted_are_separate_vouches_of_the_side() {
        let mut rand = Rand(0x9e37_79b9_7f4a_7c15);
        for world in 0..300 {
            let n = 3 + rand.below(30);
            let everyone = people(n);
            let jimmy = everyone[0];
            let mut edges = Vec::new();
            for _ in 0..rand.below(4 * n) {
                let voucher = everyone[1 + rand.below(n - 1)];
                let subject = everyone[rand.below(n)];
                let how = match rand.below(3) {
                    0 => How::CreditedEarly,
                    _ => How::Reshared,
                };
                if voucher != subject {
                    edges.push((voucher, subject, how));
                }
            }
            let side = Side::new(jimmy, &edges);
            let roots = people(3);
            let placings: Vec<Placing> = (0..rand.below(n))
                .map(|_| Placing {
                    root: roots[rand.below(3)],
                    person: everyone[1 + rand.below(n - 1)],
                    steps: 1 + rand.below(3) as u8,
                    via: Via::Reshared,
                })
                .collect();
            let met = meet(&side, &placings);
            assert_eq!(met.chains.len(), met.found.points, "world {world}");
            let kinds: HashMap<(PublicKey, PublicKey), How> =
                side.edges.iter().map(|(v, s, h)| ((*v, *s), *h)).collect();
            let mut seen = HashSet::new();
            for chain in &met.chains {
                assert!(placings.iter().any(|p| p.person == chain.point));
                assert_eq!(chain.hops.last().unwrap().to, jimmy, "world {world}");
                let mut at = chain.point;
                assert!(seen.insert(at), "world {world}: {} twice", at.fmt_short());
                for hop in &chain.hops {
                    assert_eq!(kinds.get(&(at, hop.to)), Some(&hop.how), "world {world}");
                    at = hop.to;
                    if at != jimmy {
                        assert!(seen.insert(at), "world {world}: {} twice", at.fmt_short());
                    }
                }
            }
            let known = HashSet::from([roots[0]]);
            let paths = paths(&met.chains, &placings, &known);
            assert_eq!(paths.len(), met.chains.len());
            for path in &paths {
                assert_eq!(path.end(), Some(jimmy));
                assert!(path.hops[0].how.is_known());
                assert!(path.hops[1..].iter().all(|h| h.how.is_signed()));
            }
        }
    }

    #[test]
    fn what_was_found_reads_plainly() {
        let found = Found {
            steps: Some(4),
            points: 3,
            through: 2,
            let_in: false,
        };
        assert_eq!(
            found.describe(),
            "4 steps away; 3 people in your circle's world vouched for them"
        );
        assert_eq!(
            Found::default().describe(),
            "nobody in your circle's world vouched for them"
        );
    }
}
