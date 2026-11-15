//! The Universe's tree (ADR 0034 §2): people grouped by the public lines
//! between them, and groups grouped again, until each group has few enough
//! children to draw at once.
//!
//! Pure: people are indices, lines are weighted pairs, and nothing here reads
//! the database or the network. The grouping is Louvain's (local moves, then
//! the groups as one point each, again), run top-down: a group too large to
//! open at once is split by clustering its own people, and a group with too
//! many parts gets a level between, by clustering the parts. People with no
//! lines aren't in the tree: they're the unplaced field (§1).
//!
//! The order is fixed by the indices, so the same world gives the same tree.
//! The caller numbers people in a stable order (by key), for that reason.

use std::collections::HashMap;

/// Most children a group has: people, groups, or both. ADR 0034's examples
/// open into 70 to 120.
pub const MAX_CHILDREN: usize = 100;

/// How hard the grouping tries to split a group it can't otherwise split:
/// each try doubles the resolution.
const SPLIT_TRIES: usize = 24;

/// A world's people and the weighted lines between them, undirected.
pub struct Graph {
    /// Each person's neighbours and the weight of their lines, summed over
    /// both directions and every kind.
    adj: Vec<Vec<(u32, f64)>>,
}

impl Graph {
    /// `people` people, numbered from 0, and `lines` between them. A line
    /// from someone to themselves, or with no weight, is left out.
    pub fn new(people: usize, lines: impl IntoIterator<Item = (u32, u32, f64)>) -> Self {
        let mut pairs: HashMap<(u32, u32), f64> = HashMap::new();
        for (a, b, w) in lines {
            if a == b || w <= 0.0 || !w.is_finite() {
                continue;
            }
            let key = (a.min(b), a.max(b));
            *pairs.entry(key).or_default() += w;
        }
        let mut adj = vec![Vec::new(); people];
        let mut pairs: Vec<_> = pairs.into_iter().collect();
        pairs.sort_by_key(|(k, _)| *k);
        for ((a, b), w) in pairs {
            adj[a as usize].push((b, w));
            adj[b as usize].push((a, w));
        }
        Graph { adj }
    }

    pub fn people(&self) -> usize {
        self.adj.len()
    }

    /// Each line once, as (lower, higher, weight), in order.
    pub fn lines(&self) -> impl Iterator<Item = (u32, u32, f64)> + '_ {
        self.adj.iter().enumerate().flat_map(|(a, n)| {
            n.iter()
                .filter(move |(b, _)| *b as usize > a)
                .map(move |(b, w)| (a as u32, *b, *w))
        })
    }
}

/// One child of a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Child {
    Person(u32),
    Group(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub parent: Option<u32>,
    pub children: Vec<Child>,
    /// Everyone under it, at any depth.
    pub people: u32,
}

/// The grouped world.
#[derive(Debug, Clone, PartialEq)]
pub struct Tree {
    /// Every group; a group's number is its place here, and changes from one
    /// tree to the next. The IDs that stay are `places`' (ADR 0034 §4).
    pub groups: Vec<Group>,
    /// The top: one group for each separate part of the world. Parts with
    /// no line between them are never grouped together.
    pub roots: Vec<u32>,
    /// People with no lines.
    pub unplaced: Vec<u32>,
    /// The group each placed person is directly in.
    pub group_of: Vec<Option<u32>>,
}

impl Tree {
    /// How many levels the deepest person is below the top, counting their
    /// own group as 1.
    pub fn depth(&self) -> usize {
        let mut deepest = 0;
        for g in self.group_of.iter().flatten() {
            let mut depth = 1;
            let mut at = *g;
            while let Some(p) = self.groups[at as usize].parent {
                depth += 1;
                at = p;
            }
            deepest = deepest.max(depth);
        }
        deepest
    }
}

/// Groups a world.
pub fn build(graph: &Graph) -> Tree {
    let n = graph.people();
    let mut tree = Tree {
        groups: Vec::new(),
        roots: Vec::new(),
        unplaced: Vec::new(),
        group_of: vec![None; n],
    };
    for part in components(graph) {
        if part.len() == 1 {
            tree.unplaced.push(part[0]);
            continue;
        }
        let root = group(graph, part, None, &mut tree);
        tree.roots.push(root);
    }
    tree
}

/// A group of `members`, with its own groups below it if it's too large.
fn group(graph: &Graph, members: Vec<u32>, parent: Option<u32>, tree: &mut Tree) -> u32 {
    if members.len() <= MAX_CHILDREN {
        let id = new_group(tree, parent, members.len());
        for &p in &members {
            tree.group_of[p as usize] = Some(id);
        }
        tree.groups[id as usize].children = members.into_iter().map(Child::Person).collect();
        return id;
    }
    let parts = split(graph, &members);
    group_of_parts(graph, parts, parent, tree)
}

/// A group whose children are `parts`, with a level between if there are
/// too many of them.
fn group_of_parts(
    graph: &Graph,
    parts: Vec<Vec<u32>>,
    parent: Option<u32>,
    tree: &mut Tree,
) -> u32 {
    let total = parts.iter().map(Vec::len).sum();
    let id = new_group(tree, parent, total);
    let children = match parts.len() <= MAX_CHILDREN {
        true => parts
            .into_iter()
            .map(|part| match part[..] {
                [one] => {
                    tree.group_of[one as usize] = Some(id);
                    Child::Person(one)
                }
                _ => Child::Group(group(graph, part, Some(id), tree)),
            })
            .collect(),
        false => split_parts(graph, parts)
            .into_iter()
            .map(|sub| match sub.len() {
                1 => {
                    let part = sub.into_iter().next().expect("one part");
                    Child::Group(group(graph, part, Some(id), tree))
                }
                _ => Child::Group(group_of_parts(graph, sub, Some(id), tree)),
            })
            .collect(),
    };
    tree.groups[id as usize].children = children;
    id
}

fn new_group(tree: &mut Tree, parent: Option<u32>, people: usize) -> u32 {
    tree.groups.push(Group {
        parent,
        children: Vec::new(),
        people: people as u32,
    });
    (tree.groups.len() - 1) as u32
}

/// Splits `members` into at least two parts by the lines between them.
fn split(graph: &Graph, members: &[u32]) -> Vec<Vec<u32>> {
    let place: HashMap<u32, u32> = members
        .iter()
        .enumerate()
        .map(|(i, p)| (*p, i as u32))
        .collect();
    let sub = Weighted {
        adj: members
            .iter()
            .map(|p| {
                graph.adj[*p as usize]
                    .iter()
                    .filter_map(|(q, w)| Some((*place.get(q)?, *w)))
                    .collect()
            })
            .collect(),
        own: vec![0.0; members.len()],
    };
    let of = split_weighted(&sub);
    collect(&of, members)
}

/// Splits more than `MAX_CHILDREN` parts into at least two sets of parts,
/// by the lines between the parts.
fn split_parts(graph: &Graph, parts: Vec<Vec<u32>>) -> Vec<Vec<Vec<u32>>> {
    let mut part_of: HashMap<u32, u32> = HashMap::new();
    for (i, part) in parts.iter().enumerate() {
        for p in part {
            part_of.insert(*p, i as u32);
        }
    }
    let mut own = vec![0.0; parts.len()];
    let mut between: Vec<HashMap<u32, f64>> = vec![HashMap::new(); parts.len()];
    for (i, part) in parts.iter().enumerate() {
        for p in part {
            for (q, w) in &graph.adj[*p as usize] {
                match part_of.get(q) {
                    Some(&j) if j as usize == i => own[i] += w / 2.0,
                    Some(&j) => *between[i].entry(j).or_default() += w,
                    None => {}
                }
            }
        }
    }
    let quotient = Weighted {
        adj: between
            .into_iter()
            .map(|m| {
                let mut v: Vec<_> = m.into_iter().collect();
                v.sort_by_key(|(j, _)| *j);
                v
            })
            .collect(),
        own,
    };
    let of = split_weighted(&quotient);
    let mut sets: Vec<Vec<Vec<u32>>> = Vec::new();
    let mut index: HashMap<u32, usize> = HashMap::new();
    for (part, c) in parts.into_iter().zip(of) {
        let at = *index.entry(c).or_insert_with(|| {
            sets.push(Vec::new());
            sets.len() - 1
        });
        sets[at].push(part);
    }
    sets
}

/// Louvain into 2 to `MAX_CHILDREN` communities: a higher resolution when
/// it finds one, a lower when it finds too many, halving the gap between.
fn split_weighted(graph: &Weighted) -> Vec<u32> {
    let n = graph.adj.len();
    let (mut low, mut high): (Option<f64>, Option<f64>) = (None, None);
    let mut resolution = 1.0;
    for _ in 0..SPLIT_TRIES {
        let of = louvain(graph, resolution);
        let count = of.iter().max().map_or(0, |c| *c as usize + 1);
        if (2..=MAX_CHILDREN).contains(&count) {
            return of;
        }
        match count < 2 {
            true => {
                low = Some(resolution);
                resolution = high.map_or(resolution * 2.0, |h| (resolution + h) / 2.0);
            }
            false => {
                high = Some(resolution);
                resolution = low.map_or(resolution / 2.0, |l| (l + resolution) / 2.0);
            }
        }
    }
    // Lines too even to split well: even blocks, in the stable order. Only
    // for drawing at scale, as the sky's cells are (ADR 0034 §6).
    let blocks = n.div_ceil(MAX_CHILDREN).clamp(2, MAX_CHILDREN);
    let size = n.div_ceil(blocks);
    (0..n).map(|i| (i / size) as u32).collect()
}

/// `of` as lists of `members`, in order of first appearance.
fn collect(of: &[u32], members: &[u32]) -> Vec<Vec<u32>> {
    let mut parts: Vec<Vec<u32>> = Vec::new();
    let mut index: HashMap<u32, usize> = HashMap::new();
    for (p, c) in members.iter().zip(of) {
        let at = *index.entry(*c).or_insert_with(|| {
            parts.push(Vec::new());
            parts.len() - 1
        });
        parts[at].push(*p);
    }
    parts
}

/// The separate parts of the world, each in index order, the parts in order
/// of their first person.
fn components(graph: &Graph) -> Vec<Vec<u32>> {
    let n = graph.people();
    let mut seen = vec![false; n];
    let mut parts = Vec::new();
    for start in 0..n {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let mut part = vec![start as u32];
        let mut at = 0;
        while at < part.len() {
            for (q, _) in &graph.adj[part[at] as usize] {
                if !seen[*q as usize] {
                    seen[*q as usize] = true;
                    part.push(*q);
                }
            }
            at += 1;
        }
        part.sort_unstable();
        parts.push(part);
    }
    parts
}

/// A graph for Louvain: points with weighted lines between them, and the
/// weight inside each point (a group's own lines, once it's one point).
struct Weighted {
    adj: Vec<Vec<(u32, f64)>>,
    own: Vec<f64>,
}

impl Weighted {
    fn degree(&self, i: usize) -> f64 {
        self.adj[i].iter().map(|(_, w)| w).sum::<f64>() + 2.0 * self.own[i]
    }
}

/// One full Louvain run: each point's community, numbered from 0.
fn louvain(graph: &Weighted, resolution: f64) -> Vec<u32> {
    let mut of: Vec<u32> = (0..graph.adj.len() as u32).collect();
    let mut level = Weighted {
        adj: graph.adj.clone(),
        own: graph.own.clone(),
    };
    loop {
        let (moved, local) = local_moves(&level, resolution);
        if !moved {
            return renumber(of);
        }
        for c in of.iter_mut() {
            *c = local[*c as usize];
        }
        level = aggregate(&level, &local);
    }
}

/// Moves each point to the neighbouring community that gains the most
/// modularity, in index order, until a pass moves none. Returns whether
/// any moved, and each point's community, numbered from 0.
fn local_moves(graph: &Weighted, resolution: f64) -> (bool, Vec<u32>) {
    let n = graph.adj.len();
    let degree: Vec<f64> = (0..n).map(|i| graph.degree(i)).collect();
    let two_m: f64 = degree.iter().sum();
    if two_m == 0.0 {
        return (false, (0..n as u32).collect());
    }
    let mut of: Vec<u32> = (0..n as u32).collect();
    let mut total = degree.clone();
    let mut moved_any = false;
    let mut to: HashMap<u32, f64> = HashMap::new();
    // A pass that moves only a few is still worth another; this bounds it.
    for _ in 0..64 {
        let mut moved = 0;
        for i in 0..n {
            let mine = of[i];
            to.clear();
            for (j, w) in &graph.adj[i] {
                *to.entry(of[*j as usize]).or_default() += w;
            }
            total[mine as usize] -= degree[i];
            let gain = |c: u32, w: f64| w - resolution * total[c as usize] * degree[i] / two_m;
            let mut best = (mine, gain(mine, to.get(&mine).copied().unwrap_or(0.0)));
            let mut options: Vec<(u32, f64)> = to.iter().map(|(c, w)| (*c, *w)).collect();
            options.sort_by_key(|(c, _)| *c);
            for (c, w) in options {
                let g = gain(c, w);
                if g > best.1 + 1e-12 {
                    best = (c, g);
                }
            }
            total[best.0 as usize] += degree[i];
            if best.0 != mine {
                of[i] = best.0;
                moved += 1;
            }
        }
        if moved == 0 {
            break;
        }
        moved_any = true;
    }
    (moved_any, renumber(of))
}

/// Communities numbered from 0, in order of their first point.
fn renumber(of: Vec<u32>) -> Vec<u32> {
    let mut index: HashMap<u32, u32> = HashMap::new();
    of.into_iter()
        .map(|c| {
            let next = index.len() as u32;
            *index.entry(c).or_insert(next)
        })
        .collect()
}

/// Each community as one point.
fn aggregate(graph: &Weighted, of: &[u32]) -> Weighted {
    let count = of.iter().max().map_or(0, |c| *c as usize + 1);
    let mut own = vec![0.0; count];
    let mut between: Vec<HashMap<u32, f64>> = vec![HashMap::new(); count];
    for (i, neighbours) in graph.adj.iter().enumerate() {
        let a = of[i];
        own[a as usize] += graph.own[i];
        for (j, w) in neighbours {
            let b = of[*j as usize];
            match a == b {
                // Each line is seen from both ends.
                true => own[a as usize] += w / 2.0,
                false => *between[a as usize].entry(b).or_default() += w,
            }
        }
    }
    Weighted {
        adj: between
            .into_iter()
            .map(|m| {
                let mut v: Vec<_> = m.into_iter().collect();
                v.sort_by_key(|(j, _)| *j);
                v
            })
            .collect(),
        own,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A small, fixed random source, so the worlds are the same each run.
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

    /// A world of `people` in friend groups of about `size`, groups gathered
    /// into regions of about `regions_of` groups: most lines within a group,
    /// some within a region, a few anywhere. Returns the lines and each
    /// person's group.
    fn world(people: usize, size: usize, regions_of: usize, seed: u64) -> (Graph, Vec<usize>) {
        let mut rng = Rng(seed);
        let group = |p: usize| p / size;
        let groups = people.div_ceil(size);
        let mut lines = Vec::new();
        for p in 0..people {
            let g = group(p);
            let mate = |rng: &mut Rng| (g * size + rng.below(size)).min(people - 1);
            for _ in 0..3 {
                lines.push((p as u32, mate(&mut rng) as u32, 1.0 + rng.below(3) as f64));
            }
            if rng.below(3) == 0 {
                let region = g / regions_of;
                let other = (region * regions_of + rng.below(regions_of)).min(groups - 1);
                let q = (other * size + rng.below(size)).min(people - 1);
                lines.push((p as u32, q as u32, 1.0));
            }
            if rng.below(50) == 0 {
                lines.push((p as u32, rng.below(people) as u32, 1.0));
            }
        }
        (Graph::new(people, lines), (0..people).map(group).collect())
    }

    /// Every group within the limit, every placed person in exactly one
    /// group, parents and counts consistent.
    fn check(tree: &Tree, people: usize) {
        let mut seen = vec![0u8; people];
        for (id, g) in tree.groups.iter().enumerate() {
            assert!(!g.children.is_empty(), "group {id} is empty");
            assert!(
                g.children.len() <= MAX_CHILDREN,
                "group {id}: {} children",
                g.children.len()
            );
            let mut under = 0;
            for c in &g.children {
                match *c {
                    Child::Person(p) => {
                        seen[p as usize] += 1;
                        assert_eq!(tree.group_of[p as usize], Some(id as u32));
                        under += 1;
                    }
                    Child::Group(child) => {
                        assert_eq!(tree.groups[child as usize].parent, Some(id as u32));
                        under += tree.groups[child as usize].people;
                    }
                }
            }
            assert_eq!(under, g.people, "group {id}'s count");
        }
        for p in &tree.unplaced {
            seen[*p as usize] += 1;
        }
        assert!(seen.iter().all(|s| *s == 1), "each person once");
        for r in &tree.roots {
            assert_eq!(tree.groups[*r as usize].parent, None);
        }
    }

    #[test]
    fn people_with_no_lines_are_unplaced_and_separate_parts_never_meet() {
        // Kim, Alice and Jimmy reshare each other; Sam and Lee do; Bo has
        // no lines.
        let graph = Graph::new(6, [(0, 1, 1.0), (1, 2, 1.0), (3, 4, 2.0), (0, 0, 5.0)]);
        let tree = build(&graph);
        check(&tree, 6);
        assert_eq!(tree.unplaced, [5]);
        assert_eq!(tree.roots.len(), 2);
        let children = |r: u32| tree.groups[r as usize].children.clone();
        assert_eq!(
            children(tree.roots[0]),
            [Child::Person(0), Child::Person(1), Child::Person(2)]
        );
        assert_eq!(
            children(tree.roots[1]),
            [Child::Person(3), Child::Person(4)]
        );
    }

    #[test]
    fn a_world_that_fits_is_one_group() {
        let (graph, _) = world(80, 10, 4, 1);
        let tree = build(&graph);
        check(&tree, 80);
        assert_eq!(tree.depth(), 1);
    }

    #[test]
    fn friend_groups_are_found_and_the_tree_is_the_same_each_time() {
        let (graph, truth) = world(20_000, 20, 10, 7);
        let tree = build(&graph);
        check(&tree, 20_000);
        assert_eq!(tree, build(&graph), "the same world, the same tree");
        assert!(tree.depth() >= 2, "too large for one level");

        // Most of each friend group shares a bottom group.
        let mut together = 0;
        let mut pairs = 0;
        for p in 0..20_000 - 1 {
            if truth[p] == truth[p + 1] {
                pairs += 1;
                together += (tree.group_of[p] == tree.group_of[p + 1]) as usize;
            }
        }
        let share = together as f64 / pairs as f64;
        assert!(share > 0.8, "only {share:.2} of friends together");
    }

    #[test]
    fn a_group_too_even_to_split_is_still_split() {
        // Everyone reshared everyone: no structure to find.
        let n = 150;
        let mut lines = Vec::new();
        for a in 0..n {
            for b in a + 1..n {
                lines.push((a, b, 1.0));
            }
        }
        let tree = build(&Graph::new(n as usize, lines));
        check(&tree, n as usize);
    }

    #[test]
    fn many_small_parts_get_a_level_between() {
        // One hub who reshared 500 pairs: 1,001 people, one part.
        let mut lines = Vec::new();
        for i in 0..500u32 {
            let (a, b) = (1 + 2 * i, 2 + 2 * i);
            lines.push((a, b, 5.0));
            lines.push((0, a, 0.1));
        }
        let tree = build(&Graph::new(1001, lines));
        check(&tree, 1001);
        assert_eq!(tree.roots.len(), 1);
    }

    /// ADR 0034's gate: a million people. Run by hand, in release:
    /// `cargo test -p osp-node --release a_million -- --ignored --nocapture`.
    #[test]
    #[ignore = "a minute or more; run by hand"]
    fn a_million_people_are_grouped() {
        let started = Instant::now();
        let (graph, _) = world(1_000_000, 20, 50, 11);
        let made = started.elapsed();
        let tree = build(&graph);
        let built = started.elapsed() - made;
        check(&tree, 1_000_000);
        println!(
            "1,000,000 people: world {made:.1?}, tree {built:.1?}, {} groups, {} roots, depth {}",
            tree.groups.len(),
            tree.roots.len(),
            tree.depth()
        );
    }
}
