//! Stable places for the Universe's tree (ADR 0034 §4): group IDs that last
//! from one tree to the next, and positions that stay put.
//!
//! The tree (`cluster`) is built from scratch each time, and its group
//! numbers mean nothing from one build to the next. `Places` is what the
//! node keeps between builds: each group's lasting ID, and where each group
//! and person sits. `update` matches a new tree against it and moves as
//! little as it can.
//!
//! **IDs.** A group keeps its ID while more than half its people are in one
//! new group; the closest such match wins. A new group without an old ID
//! notes the group most of its people came from (a split, for the phone to
//! animate), and an old group that's gone notes where most of its people
//! went (a merge).
//!
//! **Positions.** Each group and person has an offset from the centre of the
//! group it's in (a top-level group, from the world's centre), in the same
//! units at every level: a person is about one unit across, a group of n
//! people about √n. A group whose children are the ones it had last time
//! isn't touched, so nothing moves where nothing changed. Where something
//! did, a child that was there keeps its place, moved at most a little; one
//! that came from elsewhere starts where it was; a new one starts by the
//! children it has lines to, or where a hash of its ID puts it. Kim, and
//! each group she's in, sits at the centre of the group above, so she's at
//! the centre of the map.
//!
//! Pure, like `cluster`: the caller passes each person's lasting key (their
//! public key, on the node) and keeps the `Places` between builds.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::cluster::{Child, Graph, Tree};

/// A place on the map, in the units above.
pub type Point = [f32; 2];

/// Furthest a child that was already placed moves in one update, as a share
/// of its group's size, and never less than one person's width.
const MOST_MOVE: f32 = 0.02;

/// Relaxation steps for a group that changed, and for one laid out new.
const STEPS: usize = 20;
const FRESH_STEPS: usize = 60;

/// What the node keeps between trees.
#[derive(Debug, Clone)]
pub struct Places<K> {
    groups: HashMap<u64, KeptGroup>,
    people: HashMap<K, KeptPerson>,
    next: u64,
}

#[derive(Debug, Clone, Copy)]
struct KeptGroup {
    parent: Option<u64>,
    people: u32,
    at: Point,
}

#[derive(Debug, Clone, Copy)]
struct KeptPerson {
    group: u64,
    at: Point,
}

/// A tree with lasting IDs and places. Indexed as the tree is: by its
/// group numbers and by person.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub ids: Vec<u64>,
    /// For a group with a new ID: the old group most of its people were in.
    pub from: Vec<Option<u64>>,
    /// Old groups that are gone, each with the group most of its people are
    /// in now.
    pub merged: Vec<(u64, u64)>,
    /// Each group's centre.
    pub group_at: Vec<Point>,
    /// Each placed person; the unplaced field keeps its hashed places
    /// elsewhere (§1).
    pub person_at: Vec<Option<Point>>,
}

impl<K> Default for Places<K> {
    fn default() -> Self {
        Places {
            groups: HashMap::new(),
            people: HashMap::new(),
            next: 1,
        }
    }
}

impl<K: Hash + Eq + Clone> Places<K> {
    /// Gives `tree`, built from `graph`, lasting IDs and places, and keeps
    /// them for next time. `keys` are the people's lasting keys, by index;
    /// `kim` is the node's owner, if she's placed.
    pub fn update(&mut self, graph: &Graph, tree: &Tree, keys: &[K], kim: Option<u32>) -> Placed {
        let (ids, from, merged) = self.match_ids(tree, keys);
        let was = self.absolute();
        let mut layout = Layout::new(self, tree, keys, &ids, &was);
        layout.pin(kim);
        layout.weigh(graph);
        let (group_at, person_at, kept) = layout.run();
        *self = kept;
        Placed {
            ids,
            from,
            merged,
            group_at,
            person_at,
        }
    }

    /// Each new group's ID, the old group a new ID came from, and the old
    /// groups that merged.
    #[allow(clippy::type_complexity)]
    fn match_ids(
        &mut self,
        tree: &Tree,
        keys: &[K],
    ) -> (Vec<u64>, Vec<Option<u64>>, Vec<(u64, u64)>) {
        // How many people each new group shares with each old one, through
        // every level of both.
        let mut shared: HashMap<(u32, u64), u32> = HashMap::new();
        let mut old_chain = Vec::new();
        for (i, key) in keys.iter().enumerate() {
            let (Some(leaf), Some(was)) = (tree.group_of[i], self.people.get(key)) else {
                continue;
            };
            old_chain.clear();
            let mut at = Some(was.group);
            while let Some(id) = at {
                old_chain.push(id);
                at = self.groups.get(&id).and_then(|g| g.parent);
            }
            let mut at = Some(leaf);
            while let Some(g) = at {
                for id in &old_chain {
                    *shared.entry((g, *id)).or_default() += 1;
                }
                at = tree.groups[g as usize].parent;
            }
        }
        let closeness = |(g, id): (u32, u64), n: u32| {
            let new = tree.groups[g as usize].people as f64;
            let old = self.groups.get(&id).map_or(0, |o| o.people) as f64;
            n as f64 / (new + old - n as f64)
        };
        let mut pairs: Vec<((u32, u64), u32, f64)> = shared
            .iter()
            .map(|(k, n)| (*k, *n, closeness(*k, *n)))
            .collect();
        pairs.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));

        let count = tree.groups.len();
        let mut ids: Vec<Option<u64>> = vec![None; count];
        let mut taken: HashMap<u64, u32> = HashMap::new();
        let mut from: Vec<Option<u64>> = vec![None; count];
        let mut went: HashMap<u64, u32> = HashMap::new();
        for ((g, id), n, _) in &pairs {
            // The closest pair first, so these are each one's best.
            from[*g as usize].get_or_insert(*id);
            went.entry(*id).or_insert(*g);
            let old = self.groups.get(id).map_or(0, |o| o.people);
            if 2 * n > old && ids[*g as usize].is_none() && !taken.contains_key(id) {
                ids[*g as usize] = Some(*id);
                taken.insert(*id, *g);
            }
        }
        let ids: Vec<u64> = ids
            .into_iter()
            .map(|id| {
                id.unwrap_or_else(|| {
                    self.next += 1;
                    self.next - 1
                })
            })
            .collect();
        for (g, f) in from.iter_mut().enumerate() {
            if taken.get(&ids[g]) == Some(&(g as u32)) {
                *f = None;
            }
        }
        let mut merged: Vec<(u64, u64)> = went
            .into_iter()
            .filter(|(id, _)| !taken.contains_key(id))
            .map(|(id, g)| (id, ids[g as usize]))
            .collect();
        merged.sort_unstable();
        (ids, from, merged)
    }

    /// Where each kept group's centre was.
    fn absolute(&self) -> HashMap<u64, Point> {
        let mut at: HashMap<u64, Point> = HashMap::with_capacity(self.groups.len());
        let mut chain = Vec::new();
        for id in self.groups.keys() {
            chain.clear();
            let mut cur = Some(*id);
            let mut base = [0.0, 0.0];
            while let Some(c) = cur {
                if let Some(p) = at.get(&c) {
                    base = *p;
                    break;
                }
                chain.push(c);
                cur = self.groups.get(&c).and_then(|g| g.parent);
            }
            for c in chain.iter().rev() {
                let off = self.groups.get(c).map_or([0.0, 0.0], |g| g.at);
                base = add(base, off);
                at.insert(*c, base);
            }
        }
        at
    }
}

/// One child in one group, while it's laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Slot {
    Group(u32),
    Person(u32),
}

impl From<Child> for Slot {
    fn from(c: Child) -> Self {
        match c {
            Child::Group(g) => Slot::Group(g),
            Child::Person(p) => Slot::Person(p),
        }
    }
}

/// A frame is a group's inside, or the top (`TOP`), where the roots sit.
const TOP: u32 = 0;

fn frame_of(group: u32) -> u32 {
    group + 1
}

struct Layout<'a, K> {
    old: &'a Places<K>,
    tree: &'a Tree,
    keys: &'a [K],
    ids: &'a [u64],
    was: &'a HashMap<u64, Point>,
    /// Where each new group's people were on average, for a group that
    /// has no place of its own yet.
    came_from: Vec<Option<Point>>,
    /// How many children each old group had, to see that one left.
    had: HashMap<Option<u64>, u32>,
    /// The child pinned at the centre of each frame: Kim's way down.
    pinned: HashMap<u32, Slot>,
    /// Lines between siblings, by frame, sorted.
    lines: Vec<(u32, Slot, Slot, f32)>,
}

impl<'a, K: Hash + Eq + Clone> Layout<'a, K> {
    fn new(
        old: &'a Places<K>,
        tree: &'a Tree,
        keys: &'a [K],
        ids: &'a [u64],
        was: &'a HashMap<u64, Point>,
    ) -> Self {
        let mut sums = vec![([0.0f64, 0.0f64], 0u32); tree.groups.len()];
        for (i, key) in keys.iter().enumerate() {
            let (Some(leaf), Some(p)) = (tree.group_of[i], old.people.get(key)) else {
                continue;
            };
            let Some(base) = was.get(&p.group) else {
                continue;
            };
            let at = add(*base, p.at);
            let mut g = Some(leaf);
            while let Some(x) = g {
                let s = &mut sums[x as usize];
                s.0[0] += at[0] as f64;
                s.0[1] += at[1] as f64;
                s.1 += 1;
                g = tree.groups[x as usize].parent;
            }
        }
        let came_from = sums
            .into_iter()
            .map(|(s, n)| (n > 0).then(|| [(s[0] / n as f64) as f32, (s[1] / n as f64) as f32]))
            .collect();
        let mut had: HashMap<Option<u64>, u32> = HashMap::new();
        for g in old.groups.values() {
            *had.entry(g.parent).or_default() += 1;
        }
        for p in old.people.values() {
            *had.entry(Some(p.group)).or_default() += 1;
        }
        Layout {
            old,
            tree,
            keys,
            ids,
            was,
            came_from,
            had,
            pinned: HashMap::new(),
            lines: Vec::new(),
        }
    }

    fn parent_frame(&self, g: u32) -> u32 {
        self.tree.groups[g as usize].parent.map_or(TOP, frame_of)
    }

    fn pin(&mut self, kim: Option<u32>) {
        let Some(kim) = kim else { return };
        let Some(leaf) = self.tree.group_of[kim as usize] else {
            return;
        };
        self.pinned.insert(frame_of(leaf), Slot::Person(kim));
        let mut g = leaf;
        loop {
            self.pinned.insert(self.parent_frame(g), Slot::Group(g));
            match self.tree.groups[g as usize].parent {
                Some(p) => g = p,
                None => break,
            }
        }
    }

    /// Each line, as a pull between the two siblings it runs between: the
    /// children of the lowest group that holds both ends.
    fn weigh(&mut self, graph: &Graph) {
        let groups = &self.tree.groups;
        let mut depth = vec![0u32; groups.len()];
        // A parent is always numbered before its children.
        for g in 0..groups.len() {
            if let Some(p) = groups[g].parent {
                depth[g] = depth[p as usize] + 1;
            }
        }
        for (a, b, w) in graph.lines() {
            let (Some(mut x), Some(mut y)) = (
                self.tree.group_of[a as usize],
                self.tree.group_of[b as usize],
            ) else {
                continue;
            };
            let (mut sa, mut sb) = (Slot::Person(a), Slot::Person(b));
            while depth[x as usize] > depth[y as usize] {
                sa = Slot::Group(x);
                x = groups[x as usize].parent.expect("deeper than a root");
            }
            while depth[y as usize] > depth[x as usize] {
                sb = Slot::Group(y);
                y = groups[y as usize].parent.expect("deeper than a root");
            }
            let mut frame = frame_of(x);
            while x != y {
                sa = Slot::Group(x);
                sb = Slot::Group(y);
                match (groups[x as usize].parent, groups[y as usize].parent) {
                    (Some(px), Some(py)) => {
                        x = px;
                        y = py;
                        frame = frame_of(x);
                    }
                    _ => {
                        frame = TOP;
                        break;
                    }
                }
            }
            self.lines.push((frame, sa, sb, w as f32));
        }
        self.lines.sort_unstable_by_key(|l| (l.0, l.1, l.2));
    }

    /// Lays out every frame, top down, so each group's centre is known
    /// before its children are placed around it.
    fn run(self) -> (Vec<Point>, Vec<Option<Point>>, Places<K>) {
        let tree = self.tree;
        let mut group_at = vec![[0.0, 0.0]; tree.groups.len()];
        let mut person_at = vec![None; self.keys.len()];
        let mut kept = Places {
            groups: HashMap::with_capacity(tree.groups.len()),
            people: HashMap::with_capacity(self.keys.len()),
            next: self.old.next,
        };
        let placed: f32 = tree
            .roots
            .iter()
            .map(|r| tree.groups[*r as usize].people as f32)
            .sum();
        let mut line_at = 0;
        for frame in 0..=tree.groups.len() as u32 {
            let (centre, radius, children, id): (Point, f32, Vec<Slot>, Option<u64>) = match frame {
                TOP => (
                    [0.0, 0.0],
                    1.2 * placed.sqrt(),
                    tree.roots.iter().map(|r| Slot::Group(*r)).collect(),
                    None,
                ),
                f => {
                    let g = &tree.groups[f as usize - 1];
                    (
                        group_at[f as usize - 1],
                        (g.people as f32).sqrt(),
                        g.children.iter().map(|c| Slot::from(*c)).collect(),
                        Some(self.ids[f as usize - 1]),
                    )
                }
            };
            let start = line_at;
            while line_at < self.lines.len() && self.lines[line_at].0 == frame {
                line_at += 1;
            }
            let lines = &self.lines[start..line_at];
            let offsets = self.frame(frame, id, centre, radius, &children, lines);
            for (slot, off) in children.iter().zip(offsets) {
                let abs = add(centre, off);
                match *slot {
                    Slot::Group(g) => {
                        group_at[g as usize] = abs;
                        kept.groups.insert(
                            self.ids[g as usize],
                            KeptGroup {
                                parent: id,
                                people: tree.groups[g as usize].people,
                                at: off,
                            },
                        );
                    }
                    Slot::Person(p) => {
                        person_at[p as usize] = Some(abs);
                        kept.people.insert(
                            self.keys[p as usize].clone(),
                            KeptPerson {
                                group: id.expect("people are in groups"),
                                at: off,
                            },
                        );
                    }
                }
            }
        }
        (group_at, person_at, kept)
    }

    /// The offsets of one frame's children from its centre.
    fn frame(
        &self,
        frame: u32,
        id: Option<u64>,
        centre: Point,
        radius: f32,
        children: &[Slot],
        lines: &[(u32, Slot, Slot, f32)],
    ) -> Vec<Point> {
        #[derive(Clone, Copy, PartialEq)]
        enum Start {
            Kept,
            Moved,
            New,
        }
        let pinned = self.pinned.get(&frame).copied();
        let mut at: Vec<Point> = Vec::with_capacity(children.len());
        let mut start: Vec<Start> = Vec::with_capacity(children.len());
        for slot in children {
            let (s, p) = match *slot {
                Slot::Group(g) => {
                    let gid = self.ids[g as usize];
                    match self.old.groups.get(&gid) {
                        Some(k) if k.parent == id => (Start::Kept, k.at),
                        _ => match self.was.get(&gid).or(self.came_from[g as usize].as_ref()) {
                            Some(w) => (Start::Moved, sub(*w, centre)),
                            None => (Start::New, [0.0, 0.0]),
                        },
                    }
                }
                Slot::Person(p) => match self.old.people.get(&self.keys[p as usize]) {
                    Some(k) if Some(k.group) == id => (Start::Kept, k.at),
                    Some(k) => match self.was.get(&k.group) {
                        Some(w) => (Start::Moved, sub(add(*w, k.at), centre)),
                        None => (Start::New, [0.0, 0.0]),
                    },
                    None => (Start::New, [0.0, 0.0]),
                },
            };
            start.push(s);
            at.push(p);
        }
        let kept = start.iter().filter(|s| **s == Start::Kept).count() as u32;
        let pin = pinned.and_then(|p| children.iter().position(|c| *c == p));
        let pin_moved = pin.is_some_and(|i| at[i] != [0.0, 0.0]);
        let unchanged = kept as usize == children.len()
            && self.had.get(&id).copied().unwrap_or(0) == kept
            && !pin_moved;
        if unchanged {
            return at;
        }

        let index: HashMap<Slot, usize> =
            children.iter().enumerate().map(|(i, c)| (*c, i)).collect();
        let edges: Vec<(usize, usize, f32)> = lines
            .iter()
            .filter_map(|(_, a, b, w)| Some((*index.get(a)?, *index.get(b)?, *w)))
            .collect();
        let mut pull = vec![0.0f32; children.len()];
        for (a, b, w) in &edges {
            pull[*a] += w;
            pull[*b] += w;
        }
        let size: Vec<f32> = children
            .iter()
            .map(|c| match *c {
                Slot::Group(g) => 0.7 * (self.tree.groups[g as usize].people as f32).sqrt(),
                Slot::Person(_) => 0.5,
            })
            .collect();

        // New children: by the placed ones they have lines to, else hashed.
        let mut placed: Vec<bool> = start.iter().map(|s| *s != Start::New).collect();
        for i in 0..children.len() {
            if placed[i] {
                continue;
            }
            let seed = match children[i] {
                Slot::Group(g) => hash_point(&("group", self.ids[g as usize])),
                Slot::Person(p) => hash_point(&("person", &self.keys[p as usize])),
            };
            let (mut sum, mut weight) = ([0.0f32, 0.0f32], 0.0f32);
            for (a, b, w) in &edges {
                let other = match (*a == i, *b == i) {
                    (true, _) => *b,
                    (_, true) => *a,
                    _ => continue,
                };
                if placed[other] {
                    sum = add(sum, scale(at[other], *w));
                    weight += w;
                }
            }
            at[i] = match weight > 0.0 {
                true => add(scale(sum, 1.0 / weight), scale(seed, 2.0 * size[i])),
                false => scale(seed, 0.8 * radius),
            };
            placed[i] = true;
        }
        if let Some(p) = pin {
            at[p] = [0.0, 0.0];
        }

        let fresh = kept == 0;
        let steps = if fresh { FRESH_STEPS } else { STEPS };
        let most = (MOST_MOVE * radius).max(1.0);
        let first = at.clone();
        let mut push = vec![[0.0f32, 0.0f32]; children.len()];
        let mut order: Vec<usize> = (0..children.len()).collect();
        for _ in 0..steps {
            push.iter_mut().for_each(|p| *p = [0.0, 0.0]);
            // Siblings that overlap push apart: swept along x.
            order.sort_by(|a, b| {
                (at[*a][0] - size[*a])
                    .total_cmp(&(at[*b][0] - size[*b]))
                    .then(a.cmp(b))
            });
            let mut open: Vec<usize> = Vec::new();
            for &i in &order {
                let left = at[i][0] - size[i];
                open.retain(|j| at[*j][0] + size[*j] > left);
                for &j in &open {
                    let d = sub(at[i], at[j]);
                    let len = norm(d);
                    let gap = size[i] + size[j] - len;
                    if gap <= 0.0 {
                        continue;
                    }
                    let dir = match len > 1e-4 {
                        true => scale(d, 1.0 / len),
                        false => apart(i, j),
                    };
                    push[i] = add(push[i], scale(dir, gap * 0.25));
                    push[j] = sub(push[j], scale(dir, gap * 0.25));
                }
                open.push(i);
            }
            // Lines pull siblings together, until they touch.
            for (a, b, w) in &edges {
                let d = sub(at[*b], at[*a]);
                let len = norm(d);
                let gap = len - size[*a] - size[*b];
                if gap <= 0.0 {
                    continue;
                }
                let dir = scale(d, 1.0 / len);
                push[*a] = add(push[*a], scale(dir, gap * 0.1 * w / pull[*a]));
                push[*b] = sub(push[*b], scale(dir, gap * 0.1 * w / pull[*b]));
            }
            // And everyone stays inside the group.
            for i in 0..children.len() {
                let len = norm(at[i]);
                let over = len + size[i] - radius;
                if over > 0.0 && len > 1e-4 {
                    push[i] = sub(push[i], scale(at[i], 0.5 * over / len));
                }
            }
            for i in 0..children.len() {
                let mut next = add(at[i], clamp(push[i], 0.1 * radius));
                if start[i] != Start::New {
                    next = add(first[i], clamp(sub(next, first[i]), most));
                }
                at[i] = next;
            }
            if let Some(p) = pin {
                at[p] = [0.0, 0.0];
            }
        }
        at
    }
}

/// A point evenly spread over the unit disc, from a hash of `what`.
fn hash_point(what: &impl Hash) -> Point {
    struct Blake(blake3::Hasher);
    impl Hasher for Blake {
        fn write(&mut self, bytes: &[u8]) {
            self.0.update(bytes);
        }
        fn finish(&self) -> u64 {
            0
        }
    }
    let mut h = Blake(blake3::Hasher::new());
    what.hash(&mut h);
    let bytes = h.0.finalize();
    let b = bytes.as_bytes();
    let u = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64 / (u32::MAX as f64 + 1.0);
    let v = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as f64 / (u32::MAX as f64 + 1.0);
    let (r, a) = (u.sqrt(), v * std::f64::consts::TAU);
    [(r * a.cos()) as f32, (r * a.sin()) as f32]
}

/// A fixed direction for two children in the same spot, so they part.
fn apart(i: usize, j: usize) -> Point {
    let a = ((i * 31 + j * 17) % 360) as f32 * std::f32::consts::PI / 180.0;
    [a.cos(), a.sin()]
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale(a: Point, s: f32) -> Point {
    [a[0] * s, a[1] * s]
}

fn norm(a: Point) -> f32 {
    (a[0] * a[0] + a[1] * a[1]).sqrt()
}

fn clamp(a: Point, most: f32) -> Point {
    let len = norm(a);
    match len > most {
        true => scale(a, most / len),
        false => a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::build;

    /// The same small random source as `cluster`'s tests.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// A world by key: people in friend groups of `size`, groups in regions
    /// of `regions_of` groups, as `cluster`'s tests make them. Each person
    /// has a random key, as public keys are, and is numbered by it, as the
    /// node numbers people.
    struct World {
        keys: Vec<u64>,
        /// Each person's friend group, by key.
        group: HashMap<u64, usize>,
        lines: Vec<(u64, u64, f64)>,
        size: usize,
        regions_of: usize,
    }

    impl World {
        fn new(people: usize, size: usize, regions_of: usize, rng: &mut Rng) -> World {
            let mut w = World {
                keys: Vec::new(),
                group: HashMap::new(),
                lines: Vec::new(),
                size,
                regions_of,
            };
            let groups = people.div_ceil(size);
            for p in 0..people {
                let key = rng.next();
                w.keys.push(key);
                w.group.insert(key, p / size);
            }
            let by_group: Vec<Vec<u64>> = (0..groups)
                .map(|g| w.keys[g * size..((g + 1) * size).min(people)].to_vec())
                .collect();
            for p in 0..people {
                let me = w.keys[p];
                w.befriend(me, p / size, &by_group, rng);
            }
            w
        }

        /// Lines from `me`, in friend group `g`: mostly within it, some in
        /// its region, a few anywhere.
        fn befriend(&mut self, me: u64, g: usize, by_group: &[Vec<u64>], rng: &mut Rng) {
            let pick = |list: &[u64], rng: &mut Rng| list[rng.below(list.len())];
            for _ in 0..3 {
                let w = 1.0 + rng.below(3) as f64;
                self.lines.push((me, pick(&by_group[g], rng), w));
            }
            if rng.below(3) == 0 {
                let region = g / self.regions_of;
                let other =
                    (region * self.regions_of + rng.below(self.regions_of)).min(by_group.len() - 1);
                self.lines.push((me, pick(&by_group[other], rng), 1.0));
            }
            if rng.below(50) == 0 {
                let any = rng.below(by_group.len());
                self.lines.push((me, pick(&by_group[any], rng), 1.0));
            }
        }

        /// `n` new people, each joining one of `groups` friend groups, in
        /// turn. A group numbered past the last is a new one, in the first
        /// region: each member also has a line to someone there.
        fn join(&mut self, n: usize, groups: &[usize], rng: &mut Rng) -> Vec<u64> {
            let mut by_group: Vec<Vec<u64>> = Vec::new();
            for k in &self.keys {
                let g = self.group[k];
                if by_group.len() <= g {
                    by_group.resize(g + 1, Vec::new());
                }
                by_group[g].push(*k);
            }
            let existing = by_group.len();
            let mut new = Vec::new();
            for i in 0..n {
                let key = rng.next();
                let g = groups[i % groups.len()];
                if by_group.len() <= g {
                    by_group.resize(g + 1, Vec::new());
                }
                if g >= existing {
                    let near = &by_group[rng.below(self.regions_of)];
                    self.lines.push((key, near[rng.below(near.len())], 1.0));
                }
                if !by_group[g].is_empty() {
                    self.befriend(key, g, &by_group, rng);
                }
                self.keys.push(key);
                self.group.insert(key, g);
                by_group[g].push(key);
                new.push(key);
            }
            new
        }

        /// The graph and keys, numbered in key order.
        fn graph(&self) -> (Graph, Vec<u64>) {
            let mut keys = self.keys.clone();
            keys.sort_unstable();
            let index: HashMap<u64, u32> = keys
                .iter()
                .enumerate()
                .map(|(i, k)| (*k, i as u32))
                .collect();
            let lines = self.lines.iter().map(|(a, b, w)| (index[a], index[b], *w));
            (Graph::new(keys.len(), lines), keys)
        }
    }

    fn place(places: &mut Places<u64>, world: &World, kim: u64) -> (Tree, Placed, Vec<u64>) {
        let (graph, keys) = world.graph();
        let tree = build(&graph);
        let kim = keys.iter().position(|k| *k == kim).map(|i| i as u32);
        let placed = places.update(&graph, &tree, &keys, kim);
        (tree, placed, keys)
    }

    fn distance(a: Point, b: Point) -> f32 {
        norm(sub(a, b))
    }

    #[test]
    fn friends_are_placed_together_and_kim_at_the_centre() {
        let mut rng = Rng(3);
        let world = World::new(3_000, 20, 10, &mut rng);
        let kim = world.keys[0];
        let mut places = Places::default();
        let (tree, placed, keys) = place(&mut places, &world, kim);
        let at = |k: u64| placed.person_at[keys.binary_search(&k).unwrap()].unwrap();
        assert_eq!(at(kim), [0.0, 0.0]);
        assert_eq!(placed.ids.len(), tree.groups.len());

        // Friends are nearer each other than people picked at random.
        let (mut friends, mut strangers) = (0.0f64, 0.0f64);
        for i in 0..world.keys.len() - 1 {
            let (a, b) = (world.keys[i], world.keys[i + 1]);
            let c = world.keys[rng.below(world.keys.len())];
            if world.group[&a] == world.group[&b] {
                friends += distance(at(a), at(b)) as f64;
                strangers += distance(at(a), at(c)) as f64;
            }
        }
        assert!(
            friends * 3.0 < strangers,
            "friends {friends:.0}, strangers {strangers:.0}"
        );
    }

    #[test]
    fn the_same_world_gets_the_same_places() {
        let mut rng = Rng(4);
        let world = World::new(2_000, 20, 10, &mut rng);
        let (mut one, mut two) = (Places::default(), Places::default());
        let a = place(&mut one, &world, world.keys[5]).1;
        let b = place(&mut two, &world, world.keys[5]).1;
        assert_eq!(a, b);
        // And placing it again changes nothing.
        let c = place(&mut one, &world, world.keys[5]).1;
        assert_eq!(a.ids, c.ids);
        assert_eq!(a.group_at, c.group_at);
        assert_eq!(a.person_at, c.person_at);
        assert!(c.from.iter().all(Option::is_none) && c.merged.is_empty());
    }

    #[test]
    fn a_split_keeps_one_id_and_says_where_the_other_came_from() {
        // Friend groups of 50 (Kim's) and 30, joined by many lines, are one
        // group; cut the lines and they're two. Kim's keeps the ID, as it
        // holds more than half the people; an even split would keep none.
        let mut lines = Vec::new();
        for (from, to) in [(0u32, 50u32), (50, 80)] {
            for i in from..to {
                for j in i + 1..to {
                    lines.push((i, j, 1.0));
                }
            }
        }
        let joined: Vec<_> = (0..50)
            .flat_map(|i| (50..80).map(move |j| (i, j, 1.0)))
            .collect();
        let keys: Vec<u64> = (0..80).collect();
        let mut places = Places::default();
        let together = Graph::new(80, lines.iter().copied().chain(joined));
        let tree = build(&together);
        assert_eq!(tree.groups.len(), 1);
        let before = places.update(&together, &tree, &keys, Some(0));

        let apart = Graph::new(80, lines);
        let tree = build(&apart);
        assert_eq!(tree.roots.len(), 2);
        let after = places.update(&apart, &tree, &keys, Some(0));
        let kims = tree.group_of[0].unwrap() as usize;
        let other = tree.group_of[79].unwrap() as usize;
        assert_eq!(after.ids[kims], before.ids[0], "Kim's side keeps the ID");
        assert_ne!(after.ids[other], before.ids[0]);
        assert_eq!(after.from[other], Some(before.ids[0]));
        assert_eq!(after.person_at[0], Some([0.0, 0.0]));
    }

    /// ADR 0034's stability gate: 1% new people, all in one region, half
    /// joining its friend groups and half making five new ones. Every
    /// other region's centre stays within `DRIFT`, and nine groups in ten
    /// keep their IDs.
    #[test]
    fn a_few_new_people_move_no_unrelated_region() {
        const DRIFT: f32 = 2.0;
        let mut rng = Rng(9);
        let mut world = World::new(20_000, 20, 10, &mut rng);
        let kim = world.keys[12_345];
        let mut places = Places::default();
        let (_, before, _) = place(&mut places, &world, kim);
        let old_at: HashMap<u64, Point> = before
            .ids
            .iter()
            .copied()
            .zip(before.group_at.iter().copied())
            .collect();

        // 100 people join the first region's ten friend groups, and 100
        // more make five new ones there.
        let mut new = world.join(100, &(0..10).collect::<Vec<_>>(), &mut rng);
        new.extend(world.join(100, &(1000..1005).collect::<Vec<_>>(), &mut rng));
        let (tree, after, keys) = place(&mut places, &world, kim);
        assert_eq!(
            after.person_at[keys.binary_search(&kim).unwrap()],
            Some([0.0, 0.0])
        );

        let kept = before
            .ids
            .iter()
            .filter(|id| after.ids.contains(id))
            .count();
        let share = kept as f64 / before.ids.len() as f64;
        assert!(share >= 0.9, "only {share:.2} of IDs kept");

        // A region is a child of a root. It's unrelated if none of its
        // people has a line to a newcomer, and isn't one.
        let index = |k: u64| keys.binary_search(&k).unwrap() as u32;
        let new_set: std::collections::HashSet<u64> = new.iter().copied().collect();
        let mut touched = vec![false; tree.groups.len()];
        let region_of = |g: u32| {
            let mut g = g;
            while let Some(p) = tree.groups[g as usize].parent {
                if tree.groups[p as usize].parent.is_none() {
                    return g;
                }
                g = p;
            }
            g
        };
        for (a, b, _) in &world.lines {
            if new_set.contains(a) || new_set.contains(b) {
                for k in [a, b] {
                    if let Some(g) = tree.group_of[index(*k) as usize] {
                        touched[region_of(g) as usize] = true;
                    }
                }
            }
        }
        let (mut checked, mut worst) = (0, 0.0f32);
        for r in &tree.roots {
            for c in &tree.groups[*r as usize].children {
                let Child::Group(g) = *c else { continue };
                let Some(was) = old_at.get(&after.ids[g as usize]) else {
                    continue;
                };
                if touched[g as usize] {
                    continue;
                }
                checked += 1;
                worst = worst.max(distance(*was, after.group_at[g as usize]));
            }
        }
        let fresh = after
            .ids
            .iter()
            .filter(|id| !before.ids.contains(id))
            .count();
        let moved = before
            .ids
            .iter()
            .zip(&before.group_at)
            .filter(|(id, at)| {
                let i = after.ids.iter().position(|x| x == *id);
                i.is_some_and(|i| distance(**at, after.group_at[i]) > 0.0)
            })
            .count();
        println!(
            "{} groups, then {}: IDs kept {share:.3}, {fresh} new, {moved} moved; \
             {checked} unrelated regions, the worst moved {worst:.2}",
            before.ids.len(),
            after.ids.len()
        );
        assert!(checked >= 5, "only {checked} unrelated regions");
        assert!(worst <= DRIFT, "a region moved {worst:.2}");
    }

    /// Placing a million people, then again after 1% more join. Run by
    /// hand, in release:
    /// `cargo test -p osp-node --release --lib a_million_people_are_placed -- --ignored --nocapture`.
    #[test]
    #[ignore = "a minute or more; run by hand"]
    fn a_million_people_are_placed() {
        use std::time::Instant;
        let mut rng = Rng(11);
        let mut world = World::new(1_000_000, 20, 50, &mut rng);
        let kim = world.keys[123_456];
        let mut places = Places::default();
        let (graph, keys) = world.graph();
        let tree = build(&graph);
        let kim_at = keys.binary_search(&kim).ok().map(|i| i as u32);
        let started = Instant::now();
        let before = places.update(&graph, &tree, &keys, kim_at);
        let first = started.elapsed();

        world.join(5_000, &(0..50).collect::<Vec<_>>(), &mut rng);
        world.join(5_000, &(50_000..50_250).collect::<Vec<_>>(), &mut rng);
        let (graph, keys) = world.graph();
        let tree = build(&graph);
        let kim_at = keys.binary_search(&kim).ok().map(|i| i as u32);
        let started = Instant::now();
        let after = places.update(&graph, &tree, &keys, kim_at);
        let again = started.elapsed();
        let old: std::collections::HashSet<u64> = after.ids.iter().copied().collect();
        let kept = before.ids.iter().filter(|id| old.contains(id)).count();
        println!(
            "1,000,000 people: placed in {first:.1?}; 1% more: placed in {again:.1?}, \
             {kept} of {} IDs kept",
            before.ids.len()
        );
    }
}
