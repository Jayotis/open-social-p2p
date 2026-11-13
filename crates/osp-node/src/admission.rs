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
}

/// One meeting of a replier's side with the owner's world, as the simulator
/// reads it back (`Node::meetings`).
#[cfg(feature = "sim")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Separate meeting points, and the roots they come through, among the
/// meetings within `distance`.
fn count_within(
    side: &Side,
    meetings: &[(PublicKey, u8, PublicKey)],
    distance: u8,
) -> (usize, usize) {
    let within: Vec<_> = meetings.iter().filter(|m| m.1 <= distance).collect();
    let people: HashSet<PublicKey> = within.iter().map(|m| m.0).collect();
    let roots = within.iter().map(|m| m.2).collect::<HashSet<_>>().len();
    (side.separate_chains(&people), roots)
}

/// `count_within` at each distance from 0 to 6.
#[cfg(feature = "sim")]
fn within(side: &Side, placings: &[Placing]) -> [(usize, usize); 7] {
    let meetings = meetings(side, placings);
    std::array::from_fn(|d| count_within(side, &meetings, d as u8))
}

/// Where the replier's side meets the owner's world as `placings` place it.
/// Meeting points count only as far as their chains of vouches to the
/// replier are separate, sharing nobody but the replier
/// (`Side::separate_chains`): throwaway identities behind one careless
/// reshare, and whoever vouched for the careless person, all reach the
/// replier through that one person, so they count as one.
pub(crate) fn meet(side: &Side, placings: &[Placing]) -> Found {
    let meetings = meetings(side, placings);
    let (points, through) = count_within(side, &meetings, u8::MAX);
    let mut found = Found {
        steps: meetings.iter().map(|m| m.1).min(),
        points,
        through,
        let_in: false,
    };
    let farthest = meetings.iter().map(|m| m.1).max().unwrap_or(0);
    for distance in found.steps.unwrap_or(u8::MAX)..=farthest {
        let (points, roots) = count_within(side, &meetings, distance);
        if enough(distance, points, roots) {
            found.let_in = true;
            break;
        }
    }
    found
}

/// The vouches of a carried side that count, each as (voucher, vouched
/// for): those that verify and aren't signed by a key this node knows was
/// revoked. Whether each is for whom the side needs is `Side::new`'s part.
pub(crate) fn counted_vouches(
    db: &Db,
    side: &[SignedObject],
) -> Result<Vec<(PublicKey, PublicKey)>> {
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
    /// The people it places, each with their steps back from the replier.
    pub steps: HashMap<PublicKey, u8>,
    /// Its vouches among them, each as (voucher, vouched for).
    edges: Vec<(PublicKey, PublicKey)>,
}

impl Side {
    /// The people `edges` place, each with their steps back from `replier`:
    /// those whose vouch is for the replier are 1, those whose vouch is for
    /// one of those 2, and so on to `SIDE_STEPS`. Vouches that don't chain
    /// to the replier place nobody. `edges` are (voucher, vouched for).
    pub(crate) fn new(replier: PublicKey, edges: &[(PublicKey, PublicKey)]) -> Self {
        let mut steps = HashMap::new();
        let mut frontier = HashSet::from([replier]);
        for step in 1..=SIDE_STEPS {
            let mut next = HashSet::new();
            for (voucher, subject) in edges {
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
        let edges = edges
            .iter()
            .filter(|(v, s)| steps.contains_key(v) && (*s == replier || steps.contains_key(s)))
            .copied()
            .collect();
        Self { steps, edges }
    }

    /// The most of `people` whose chains of vouches to the replier can be
    /// separate, sharing nobody but the replier: the most paths, each from
    /// one of them, that pass through no one twice. Found as a maximum flow
    /// with each person able to carry one path.
    pub(crate) fn separate_chains(&self, people: &HashSet<PublicKey>) -> usize {
        let index: HashMap<PublicKey, usize> = self
            .steps
            .keys()
            .enumerate()
            .map(|(i, p)| (*p, i))
            .collect();
        let n = index.len();
        // Each person i is two nodes, 2i in and 2i + 1 out, joined by one
        // path's room. Then the source, and the replier as the sink.
        let (source, sink) = (2 * n, 2 * n + 1);
        let mut flow = Flow::new(2 * n + 2);
        for (person, &i) in &index {
            flow.add(2 * i, 2 * i + 1);
            if people.contains(person) {
                flow.add(source, 2 * i);
            }
        }
        for (voucher, subject) in &self.edges {
            let from = 2 * index[voucher] + 1;
            match index.get(subject) {
                Some(&s) => flow.add(from, 2 * s),
                None => flow.add(from, sink),
            }
        }
        flow.max(source, sink)
    }
}

/// A flow network where every link carries one path.
struct Flow {
    /// Each node's links: where to, room left, and the reverse link's index.
    links: Vec<Vec<(usize, u8, usize)>>,
}

impl Flow {
    fn new(nodes: usize) -> Self {
        Self {
            links: vec![Vec::new(); nodes],
        }
    }

    fn add(&mut self, from: usize, to: usize) {
        let (back, forth) = (self.links[to].len(), self.links[from].len());
        self.links[from].push((to, 1, back));
        self.links[to].push((from, 0, forth));
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
                for (k, &(to, room, _)) in self.links[at].iter().enumerate() {
                    if room > 0 && to != source && came[to].is_none() {
                        came[to] = Some((at, k));
                        queue.push_back(to);
                    }
                }
            }
            if came[sink].is_none() {
                return paths;
            }
            let mut at = sink;
            while let Some((from, k)) = came[at] {
                let back = self.links[from][k].2;
                self.links[from][k].1 -= 1;
                self.links[at][back].1 += 1;
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
        let mut found = meet(&carried, &placings);
        let mut own_work = started.elapsed();
        let (mut asked, mut answered) = (0, 0);
        if !found.let_in && !people.is_empty() {
            let answers;
            (asked, answers) = self.ask_circle(&people).await?;
            answered = answers.len();
            let again = std::time::Instant::now();
            for (root, placed) in answers {
                placings.extend(placed.into_iter().map(|p| Placing {
                    root,
                    person: p.actor,
                    steps: 1 + p.steps,
                }));
            }
            found = meet(&carried, &placings);
            own_work += again.elapsed();
        }
        self.db.set_found(reply, &serde_json::to_string(&found)?)?;
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
                });
            }
        }
        Ok(placings)
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
    use osp_proto::{Announce, EarlierKeys, Payload, object::CertifiedKey};

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
        Side::new(replier, &chains(replier, points, back))
    }

    fn placed(root: PublicKey, points: &[PublicKey], steps: u8) -> Vec<Placing> {
        points
            .iter()
            .map(|p| Placing {
                root,
                person: *p,
                steps,
            })
            .collect()
    }

    fn key() -> PublicKey {
        SecretKey::generate().public()
    }

    #[test]
    fn at_each_distance_one_short_of_the_mark_isnt_let_in() {
        let (alice, bob) = (key(), key());
        // 2 or 3 steps: 3 points, through anyone.
        for (kim_steps, back) in [(1, 1), (2, 1), (1, 2)] {
            let p = people(3);
            let side = side_of(&p, back);
            let found = meet(&side, &placed(alice, &p, kim_steps));
            assert!(found.let_in, "{kim_steps} + {back}");
            assert_eq!(found.steps, Some(kim_steps + back));
            let found = meet(&side, &placed(alice, &p[..2], kim_steps));
            assert!(!found.let_in, "{kim_steps} + {back}");
        }
        // 4 steps: 4 points, through 2 people in Kim's circle.
        let p = people(4);
        let side = side_of(&p, 2);
        let mut both = placed(alice, &p[..2], 2);
        both.extend(placed(bob, &p[2..], 2));
        assert!(meet(&side, &both).let_in);
        assert!(!meet(&side, &both[1..]).let_in);
        // 5 or 6 steps: 6 points, through 2 people in her circle.
        for (kim_steps, back) in [(2, 3), (3, 3)] {
            let p = people(6);
            let side = side_of(&p, back);
            let mut both = placed(alice, &p[..3], kim_steps);
            both.extend(placed(bob, &p[3..], kim_steps));
            let found = meet(&side, &both);
            assert!(found.let_in, "{kim_steps} + {back}");
            assert_eq!((found.points, found.through), (6, 2));
            assert!(!meet(&side, &both[1..]).let_in, "{kim_steps} + {back}");
        }
    }

    #[test]
    fn through_only_one_person_in_the_circle_four_steps_and_more_arent_let_in() {
        let alice = key();
        for (kim_steps, back) in [(2, 2), (3, 2), (3, 3)] {
            let p = people(20);
            let found = meet(&side_of(&p, back), &placed(alice, &p, kim_steps));
            assert!(!found.let_in, "{kim_steps} + {back}");
            assert_eq!((found.points, found.through), (20, 1));
        }
        // From 3 steps, one is enough.
        let p = people(3);
        assert!(meet(&side_of(&p, 2), &placed(alice, &p, 1)).let_in);
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
        let side = Side::new(jimmy, &edges);
        // Kim's node: Alice at 1 (not on the side), X reshared by Alice at 2.
        let mut placings = placed(alice, &[alice], 1);
        placings.extend(placed(alice, &[x], 2));
        // Alice's node: X and the others at 1 from her, 2 from Kim.
        placings.extend(placed(alice, &[x], 2));
        placings.extend(placed(alice, &others, 2));
        let found = meet(&side, &placings);
        assert_eq!((found.steps, found.points, found.through), (Some(4), 5, 1));
        assert!(!found.let_in);
        // Dave's node places one of them too: a second root, and it's in.
        placings.extend(placed(dave, &[others[0]], 2));
        let found = meet(&side, &placings);
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
        let side = Side::new(jimmy, &edges);
        let mut placings = placed(alice, &near, 1);
        placings.extend(placed(alice, &far[..1], 2));
        placings.extend(placed(bob, &far[1..], 2));
        let found = meet(&side, &placings);
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
        let side = Side::new(jimmy, &edges);
        assert_eq!(side.steps.len(), 201);
        assert_eq!(side.steps[&careless], 2);
        // Kim's side holds the careless one, through everyone in her circle.
        let mut placings = Vec::new();
        for _ in 0..16 {
            placings.extend(placed(key(), &[careless], 2));
        }
        let found = meet(&side, &placings);
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
        let side = Side::new(operator, &edges);
        assert!(behind.iter().all(|b| side.steps[b] == 3));
        let mut placings = placed(key(), &[careless], 2);
        for b in &behind {
            placings.extend(placed(key(), &[*b], 2));
        }
        let found = meet(&side, &placings);
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
        let side = Side::new(jimmy, &chains(jimmy, &met, 3));
        let mut placings = placed(alice, &met[..5], 2);
        placings.extend(placed(dave, &met[5..], 2));
        let found = meet(&side, &placings);
        assert_eq!((found.steps, found.points), (Some(5), 10));
        assert!(found.let_in);

        let bob = key();
        let mut edges = vec![(bob, jimmy)];
        edges.extend(met.iter().map(|m| (*m, bob)));
        let found = meet(&Side::new(jimmy, &edges), &placings);
        assert_eq!((found.steps, found.points), (Some(4), 1));
        assert!(!found.let_in);
        // Bob placed himself, as well: he and those behind him share Bob.
        placings.extend(placed(alice, &[bob], 2));
        assert_eq!(meet(&Side::new(jimmy, &edges), &placings).points, 1);
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
        let side = Side::new(jimmy, &edges).steps;
        assert_eq!(
            side,
            HashMap::from([(bob, 1), (carol, 1), (dan, 2), (eve, 3)])
        );
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
        let reshare = |of: PublicKey, by: PublicKey| {
            Payload::Announce(Announce {
                actor: by,
                object: ObjectId::of(b"a post"),
                object_actor: of,
                published: osp_proto::now_rfc3339(),
                hosting: false,
            })
        };
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
            [(bob.public(), jimmy), (carol.public(), jimmy)]
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
            [(carol.public(), jimmy)]
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
                let a = Payload::Announce(Announce {
                    actor: v.public(),
                    object: ObjectId::of(b"a post"),
                    object_actor: jimmy,
                    published: osp_proto::now_rfc3339(),
                    hosting: false,
                });
                SignedObject::sign(&v, &a).unwrap()
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
            })
            .collect();
        let found = meet(&steps, &placings);
        println!("Side::new and meet: {:?} ({found:?})", t.elapsed());
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
