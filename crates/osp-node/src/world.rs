//! The Universe's world, kept between snapshots (ADR 0034 §2–§4): the tree of
//! groups, their lasting IDs and places, and the walk that opens a region of
//! it within a snapshot's budget.
//!
//! Grouping is the Universe's heaviest job on the node. A world is built from
//! its people and the public lines between them, and kept, stamped with what
//! it was built from. A snapshot made from the same inputs uses it as it is.
//! When the inputs change, a small world is grouped again at once, in a few
//! milliseconds; a large one is grouped in the background while snapshots
//! are answered from the kept world (§3). Only the first world after a start
//! is always grouped at once, as there's none to answer from.
//!
//! The IDs and places are `Places`', which the node saves after each build,
//! so they outlast a restart.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Result;
use iroh::PublicKey;
use osp_proto::universe::{Level, Region};
use tracing::warn;

use crate::{
    cluster::{self, Child, Graph, Tree},
    db::Db,
    places::{Placed, Places, Point},
};

/// A world with at most this many lines is grouped at once, while the device
/// waits: tens of milliseconds, from the million-person run's 28 s for 4.3
/// million lines (docs/testing/universe.md). A larger one is grouped in the
/// background.
const AT_ONCE_LINES: usize = 20_000;

/// Most of the stood-at person's line partners shown beside them.
pub(crate) const MOST_NEIGHBOURS: usize = 50;

/// What a world is built from.
pub(crate) struct Input {
    /// Everyone, sorted, so the same world numbers them the same.
    pub keys: Vec<PublicKey>,
    /// Lines between them, weighted, by their place in `keys`.
    pub lines: Vec<(u32, u32, f64)>,
    pub kim: Option<u32>,
    /// A hash of what the lines were made from. Their weights fade from
    /// day to day, but that alone doesn't group the world again.
    pub stamp: [u8; 32],
}

/// The grouped world.
pub(crate) struct World {
    stamp: [u8; 32],
    keys: Vec<PublicKey>,
    index: HashMap<PublicKey, u32>,
    graph: Graph,
    pub tree: Tree,
    pub placed: Placed,
    by_id: HashMap<u64, u32>,
    /// Old groups gone in the last grouping, and where most of their people
    /// went, so a device asking for one is shown that instead.
    merged: HashMap<u64, u64>,
}

/// What a region opens into.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Opened {
    /// The region as opened: a gone group's successor, or the top if it had
    /// none.
    pub region: Region,
    /// The regions above it, the top first.
    pub path: Vec<Region>,
    pub level: Level,
    /// The groups shown closed, as the tree numbers them.
    pub groups: Vec<u32>,
    /// The people shown one by one, as the world numbers them.
    pub people: Vec<u32>,
}

impl World {
    fn build(input: Input, places: &mut Places<PublicKey>) -> World {
        let graph = Graph::new(input.keys.len(), input.lines);
        let tree = cluster::build(&graph);
        let kim = input.kim.filter(|k| tree.group_of[*k as usize].is_some());
        let placed = places.update(&graph, &tree, &input.keys, kim);
        let by_id = placed
            .ids
            .iter()
            .enumerate()
            .map(|(g, id)| (*id, g as u32))
            .collect();
        let merged = placed.merged.iter().copied().collect();
        let index = input
            .keys
            .iter()
            .enumerate()
            .map(|(i, k)| (*k, i as u32))
            .collect();
        World {
            stamp: input.stamp,
            keys: input.keys,
            index,
            graph,
            tree,
            placed,
            by_id,
            merged,
        }
    }

    pub fn key(&self, person: u32) -> &PublicKey {
        &self.keys[person as usize]
    }

    /// A group's lasting ID.
    pub fn id(&self, group: u32) -> u64 {
        self.placed.ids[group as usize]
    }

    /// Where someone is placed, if their lines place them, with their
    /// number here.
    pub fn placed(&self, key: &PublicKey) -> Option<(u32, Point)> {
        let i = *self.index.get(key)?;
        Some((i, self.placed.person_at[i as usize]?))
    }

    /// The group a lasting ID names, or the one it merged into.
    fn group(&self, id: u64) -> Option<u32> {
        let mut id = id;
        // Merges are kept for one grouping, so a chain is short; the bound
        // only guards against a loop.
        for _ in 0..8 {
            if let Some(g) = self.by_id.get(&id) {
                return Some(*g);
            }
            id = *self.merged.get(&id)?;
        }
        None
    }

    /// Everyone in the group a lasting ID names, at any depth; none if it
    /// names no group.
    pub fn members(&self, id: u64) -> Option<Vec<PublicKey>> {
        let g = self.group(id)?;
        let mut members = Vec::new();
        for (i, key) in self.keys.iter().enumerate() {
            let mut at = self.tree.group_of[i];
            while let Some(here) = at {
                if here == g {
                    members.push(*key);
                    break;
                }
                at = self.tree.groups[here as usize].parent;
            }
        }
        Some(members)
    }

    /// `group` and the groups above it, the top first.
    fn chain(&self, group: u32) -> Vec<u32> {
        let mut chain = vec![group];
        while let Some(p) = self.tree.groups[*chain.last().unwrap() as usize].parent {
            chain.push(p);
        }
        chain.reverse();
        chain
    }

    /// The groups `key` is in, the top first.
    pub fn chain_of(&self, key: &PublicKey) -> Vec<u32> {
        let leaf = self
            .index
            .get(key)
            .and_then(|i| self.tree.group_of[*i as usize]);
        leaf.map(|g| self.chain(g)).unwrap_or_default()
    }

    /// Opens `region` into at most `budget` points: groups shown closed and
    /// people. `kim` is the owner, whose way down opens first at the top.
    /// None if it stands at someone the world doesn't place: the caller
    /// shows them in the sky.
    pub fn open(&self, region: Region, kim: &PublicKey, budget: usize) -> Option<Opened> {
        let kims_way: HashSet<u32> = self.chain_of(kim).into_iter().collect();
        let groups_path = |chain: &[u32]| -> Vec<Region> {
            std::iter::once(Region::Top)
                .chain(chain.iter().map(|g| Region::Group(self.id(*g))))
                .collect()
        };
        let (region, path, start, toward, extra, level) = match region {
            Region::Group(id) if self.group(id).is_some() => {
                let g = self.group(id).unwrap();
                let chain = self.chain(g);
                let start = self.tree.groups[g as usize].children.clone();
                let path = groups_path(&chain[..chain.len() - 1]);
                (
                    Region::Group(self.id(g)),
                    path,
                    start,
                    kims_way,
                    vec![],
                    None,
                )
            }
            Region::Person(key) => {
                let (i, _) = self.placed(&key)?;
                let chain = self.chain_of(&key);
                let leaf = *chain.last()?;
                // Their line partners beside them, the strongest first.
                let mut partners: Vec<(u32, f64)> = self.graph.neighbours(i).to_vec();
                partners.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                partners.truncate(MOST_NEIGHBOURS.min(budget / 2));
                let extra: Vec<u32> = partners.into_iter().map(|(p, _)| p).collect();
                let start = self.tree.groups[leaf as usize].children.clone();
                let path = groups_path(&chain);
                let level = Some(Level::Close);
                (region, path, start, HashSet::new(), extra, level)
            }
            // The top, a group that's gone with nowhere it went, or the sky,
            // which the caller opens.
            _ => {
                let mut roots = self.tree.roots.clone();
                roots.sort_by_key(|r| {
                    let g = &self.tree.groups[*r as usize];
                    (!kims_way.contains(r), std::cmp::Reverse(g.people), *r)
                });
                let start = roots.into_iter().map(Child::Group).collect();
                (Region::Top, vec![], start, kims_way, vec![], None)
            }
        };
        let budget = budget.saturating_sub(extra.len());
        let mut shown = self.open_from(start, &toward, budget);
        for p in extra {
            if !shown.contains(&Child::Person(p)) {
                shown.push(Child::Person(p));
            }
        }
        let (mut groups, mut people) = (Vec::new(), Vec::new());
        for c in shown {
            match c {
                Child::Group(g) => groups.push(g),
                Child::Person(p) => people.push(p),
            }
        }
        let level = level.unwrap_or(match (groups.is_empty(), region) {
            (true, _) => Level::Local,
            (false, Region::Top) => Level::Universe,
            (false, _) => Level::Cluster,
        });
        Some(Opened {
            region,
            path,
            level,
            groups,
            people,
        })
    }

    /// `start` with groups opened into their children while they fit in
    /// `budget`: those in `toward` first, then the largest. So at one zoom a
    /// dense area opens deeper than a sparse one beside it (ADR 0034 §2).
    fn open_from(&self, mut shown: Vec<Child>, toward: &HashSet<u32>, budget: usize) -> Vec<Child> {
        shown.truncate(budget);
        loop {
            // The group to open: on the way first, then the largest, then
            // the first numbered.
            let best = shown
                .iter()
                .enumerate()
                .filter_map(|(at, c)| match c {
                    Child::Group(g) => Some((at, *g)),
                    Child::Person(_) => None,
                })
                .filter(|(_, g)| {
                    shown.len() - 1 + self.tree.groups[*g as usize].children.len() <= budget
                })
                .max_by_key(|(_, g)| {
                    let people = self.tree.groups[*g as usize].people;
                    (toward.contains(g), people, std::cmp::Reverse(*g))
                });
            let Some((at, g)) = best else {
                break;
            };
            let children = self.tree.groups[g as usize].children.clone();
            shown.splice(at..=at, children);
        }
        shown
    }
}

/// The world the node keeps, and the places it saves.
#[derive(Default)]
pub(crate) struct Atlas {
    kept: Arc<Kept>,
}

#[derive(Default)]
struct Kept {
    world: Mutex<Option<Arc<World>>>,
    /// Read from the database at the first build.
    places: Mutex<Option<Places<PublicKey>>>,
    building: AtomicBool,
}

impl Atlas {
    /// The world for `input`: the kept one if it was built from the same,
    /// or else one grouped again, now or in the background (above).
    pub fn world(&self, db: &Arc<Db>, input: Input) -> Result<Arc<World>> {
        let kept = self.kept.world.lock().unwrap().clone();
        match kept {
            Some(world) if world.stamp == input.stamp => Ok(world),
            Some(world) if input.lines.len() > AT_ONCE_LINES => {
                if !self.kept.building.swap(true, Ordering::SeqCst) {
                    let kept = self.kept.clone();
                    let db = db.clone();
                    std::thread::Builder::new()
                        .name("universe".into())
                        .spawn(move || {
                            // Cleared even if grouping panics.
                            struct Done(Arc<Kept>);
                            impl Drop for Done {
                                fn drop(&mut self) {
                                    self.0.building.store(false, Ordering::SeqCst);
                                }
                            }
                            let done = Done(kept);
                            if let Err(e) = done.0.build(&db, input) {
                                warn!("grouping the Universe: {e:#}");
                            }
                        })?;
                }
                Ok(world)
            }
            _ => self.kept.build(db, input),
        }
    }

    /// Whether a grouping is running in the background.
    #[cfg(test)]
    pub fn building(&self) -> bool {
        self.kept.building.load(Ordering::SeqCst)
    }
}

impl Kept {
    fn build(&self, db: &Db, input: Input) -> Result<Arc<World>> {
        let mut places = self.places.lock().unwrap();
        let places = places.get_or_insert_with(|| {
            let saved = db.universe_places().unwrap_or_else(|e| {
                warn!("reading the Universe's places: {e:#}");
                None
            });
            match saved.map(|json| Places::from_json(&json)) {
                Some(Ok(p)) => p,
                Some(Err(e)) => {
                    warn!("the Universe's saved places don't read, so they start again: {e}");
                    Places::default()
                }
                None => Places::default(),
            }
        });
        let world = Arc::new(World::build(input, places));
        db.set_universe_places(&places.to_json()?)?;
        *self.world.lock().unwrap() = Some(world.clone());
        Ok(world)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osp_proto::SecretKey;

    fn key(i: u32) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[..4].copy_from_slice(&i.to_le_bytes());
        seed[31] = 1;
        SecretKey::from_bytes(&seed).public()
    }

    /// `groups` friend groups of `size`, everyone with lines to the next
    /// two in their group, and one line from each group to the next in a
    /// ring, so the world is one part. Kim is person 0.
    fn input(groups: u32, size: u32, extra: &[(u32, u32)]) -> (Input, Vec<PublicKey>) {
        let people = groups * size;
        let made: Vec<PublicKey> = (0..people).map(key).collect();
        let mut keys = made.clone();
        keys.sort();
        let at: HashMap<PublicKey, u32> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (*k, i as u32))
            .collect();
        let n = |p: u32| at[&made[p as usize]];
        let mut lines = Vec::new();
        for g in 0..groups {
            for i in 0..size {
                for d in 1..=2 {
                    lines.push((n(g * size + i), n(g * size + (i + d) % size), 1.0));
                }
            }
            lines.push((n(g * size), n(((g + 1) % groups) * size + 1), 0.2));
        }
        for (a, b) in extra {
            lines.push((n(*a), n(*b), 1.0));
        }
        let stamp = *blake3::hash(format!("{groups} {size} {extra:?}").as_bytes()).as_bytes();
        let input = Input {
            keys,
            lines,
            kim: Some(n(0)),
            stamp,
        };
        (input, made)
    }

    fn world(groups: u32, size: u32) -> (World, Vec<PublicKey>) {
        let (input, made) = input(groups, size, &[]);
        (World::build(input, &mut Places::default()), made)
    }

    fn points(o: &Opened) -> usize {
        o.groups.len() + o.people.len()
    }

    #[test]
    fn a_small_world_opens_at_local() {
        let (world, made) = world(2, 8);
        let top = world.open(Region::Top, &made[0], 300).unwrap();
        assert_eq!(top.level, Level::Local);
        assert!(top.groups.is_empty());
        assert_eq!(top.people.len(), 16);
        assert!(top.path.is_empty());
    }

    /// Every snapshot within its budget, and every person reachable by
    /// opening groups from the top.
    #[test]
    fn a_large_world_opens_within_its_budget_down_to_everyone() {
        let (world, made) = world(400, 20);
        let top = world.open(Region::Top, &made[0], 300).unwrap();
        assert_eq!(top.level, Level::Universe);
        assert!(points(&top) <= 300);
        let mut reached = HashSet::new();
        let mut open = vec![top];
        while let Some(o) = open.pop() {
            assert!(points(&o) <= 300, "{:?} has {}", o.region, points(&o));
            reached.extend(o.people.iter().copied());
            for g in &o.groups {
                let inside = world
                    .open(Region::Group(world.id(*g)), &made[0], 300)
                    .unwrap();
                assert_eq!(inside.region, Region::Group(world.id(*g)));
                assert_eq!(inside.path.first(), Some(&Region::Top));
                open.push(inside);
            }
        }
        assert_eq!(reached.len(), 8_000);
    }

    /// Kim's own way down opens first, so the top shows her friends one by
    /// one around her while the rest of the world is coarser.
    #[test]
    fn the_top_opens_around_kim() {
        let (world, made) = world(400, 20);
        let top = world.open(Region::Top, &made[0], 300).unwrap();
        let kim = world.placed(&made[0]).unwrap().0;
        assert!(top.people.contains(&kim));
        let friends = (1..20).filter(|p| {
            let i = world.placed(&made[*p]).unwrap().0;
            top.people.contains(&i)
        });
        assert!(friends.count() >= 15);
    }

    /// Dense and sparse regions open to different depths at one zoom: a
    /// group of three small groups opens to people, while a large group
    /// beside it stays closed.
    #[test]
    fn dense_and_sparse_regions_open_to_different_depths() {
        let (world, made) = world(400, 20);
        let top = world.open(Region::Top, &made[0], 300).unwrap();
        // Some people shown one by one, some groups closed.
        assert!(!top.people.is_empty() && !top.groups.is_empty());
        let depth = |g: u32| world.chain(g).len();
        let depths: HashSet<usize> = top.groups.iter().map(|g| depth(*g)).collect();
        let people_depths: HashSet<usize> = top
            .people
            .iter()
            .map(|p| depth(world.tree.group_of[*p as usize].unwrap()) + 1)
            .collect();
        assert!(
            depths.union(&people_depths).count() > 1,
            "everything at one depth: {depths:?} {people_depths:?}"
        );
    }

    /// Standing at someone shows their group, their line partners, and the
    /// way up, so zooming out stays where they are.
    #[test]
    fn standing_at_someone_shows_them_and_the_way_up() {
        let (world, made) = world(400, 20);
        let jimmy = made[4_005];
        let (i, _) = world.placed(&jimmy).unwrap();
        let at = world.open(Region::Person(jimmy), &made[0], 300).unwrap();
        assert_eq!(at.level, Level::Close);
        assert!(at.people.contains(&i));
        for (p, _) in world.graph.neighbours(i) {
            assert!(at.people.contains(p));
        }
        assert!(points(&at) <= 300);
        let chain = world.chain_of(&jimmy);
        assert_eq!(at.path.len(), chain.len() + 1);
        assert_eq!(at.path[0], Region::Top);
        // Each step up holds the one below.
        for pair in at.path.windows(2) {
            let Region::Group(id) = pair[1] else { panic!() };
            let up = world.open(pair[0], &made[0], 300).unwrap();
            let g = world.group(id).unwrap();
            let shown = up.groups.contains(&g) || {
                let mut inside = false;
                for c in &up.groups {
                    inside |= world.chain(g).contains(c);
                }
                inside
                    || up
                        .people
                        .iter()
                        .any(|p| world.chain_of(world.key(*p)).contains(&g))
            };
            assert!(shown, "{:?} doesn't hold {:?}", pair[0], pair[1]);
        }
        // Someone with no lines isn't placed.
        let (input, _) = input(2, 8, &[]);
        let lonely = key(999_999);
        let small = World::build(input, &mut Places::default());
        assert!(small.open(Region::Person(lonely), &made[0], 300).is_none());
    }

    #[test]
    fn a_gone_group_is_answered_by_where_its_people_went() {
        let mut places = Places::default();
        let (before, made) = input(2, 8, &[]);
        let before = World::build(before, &mut places);
        let (after, _) = input(2, 8, &[(0, 8), (1, 9), (2, 10), (3, 11), (4, 12), (5, 13)]);
        let after = World::build(after, &mut places);
        if let Some((gone, into)) = after.placed.merged.first() {
            let opened = after.open(Region::Group(*gone), &made[0], 300).unwrap();
            assert_eq!(opened.region, Region::Group(*into));
        }
        // A group no world had opens the top.
        let opened = before.open(Region::Group(u64::MAX), &made[0], 300).unwrap();
        assert_eq!(opened.region, Region::Top);
    }

    /// ADR 0034's gate: a million people explored down the tree from the
    /// top, every snapshot within its points, and everyone reached. Run by
    /// hand:
    /// `cargo test -p osp-node --release --lib a_million_people_are_explored -- --ignored --nocapture`.
    #[test]
    #[ignore = "a minute or more; run by hand"]
    fn a_million_people_are_explored() {
        let started = std::time::Instant::now();
        let (world, made) = world(50_000, 20);
        let built = started.elapsed();
        let explored = std::time::Instant::now();
        let mut reached = vec![false; made.len()];
        let (mut opens, mut most, mut depths) = (0, 0, HashSet::new());
        let mut open = vec![Region::Top];
        while let Some(region) = open.pop() {
            let o = world.open(region, &made[0], 300).unwrap();
            opens += 1;
            most = most.max(points(&o));
            assert!(points(&o) <= 300);
            for p in &o.people {
                reached[*p as usize] = true;
            }
            if region == Region::Top {
                depths.extend(o.groups.iter().map(|g| world.chain(*g).len()));
            }
            open.extend(o.groups.iter().map(|g| Region::Group(world.id(*g))));
        }
        let explored = explored.elapsed();
        assert!(reached.iter().all(|r| *r));
        println!(
            "1,000,000 people: grouped and placed in {built:.1?}; {opens} snapshots opened in \
             {explored:.1?} ({:.0?} each), at most {most} points; groups at the top at depths {depths:?}",
            explored / opens
        );
    }

    /// The same inputs reuse the kept world; new ones in a small world are
    /// grouped at once; places are saved and read back after a restart.
    #[test]
    fn the_world_is_kept_and_its_places_saved() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open(&dir.path().join("db.sqlite")).unwrap());
        let atlas = Atlas::default();
        let one = atlas.world(&db, input(20, 10, &[]).0).unwrap();
        let again = atlas.world(&db, input(20, 10, &[]).0).unwrap();
        assert!(Arc::ptr_eq(&one, &again));
        let more = atlas.world(&db, input(20, 10, &[(0, 50)]).0).unwrap();
        assert!(!Arc::ptr_eq(&one, &more));
        assert!(!atlas.building());

        // A restarted node keeps the IDs.
        let restarted = Atlas::default();
        let after = restarted.world(&db, input(20, 10, &[(0, 50)]).0).unwrap();
        assert_eq!(after.placed.ids, more.placed.ids);
        assert_eq!(after.placed.group_at, more.placed.group_at);
    }
}
