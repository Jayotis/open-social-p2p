//! The public sky (ADR 0034 §1, §6): people who asked to be found, each at
//! a place hashed from their key, drawn at scale as cells.
//!
//! A person's place comes from their key alone, so it never moves and says
//! nothing about who listed them or whom they know. Being listed by the
//! same directory groups no one (§1). The places are spread evenly over a
//! unit square, and the square is cut into cells as a quadtree: a cell at
//! level `n` is one of 4ⁿ equal squares. A cell is only for drawing at
//! scale. It isn't a social cluster, and its people aren't related.
//!
//! Opening a region splits its fullest cells into their quarters until
//! the snapshot's budget of points is used (§3), so zooming into a cell
//! leads, a few levels down, to each person in it.
//!
//! Pure: the caller passes the keys and says who's a creator (recent
//! public posts, §6). Machines get a sky of their own, apart from people
//! (ADR 0033 §4).

use std::hash::{Hash, Hasher};

/// The deepest cell: the place's full precision.
pub const DEEPEST: u8 = 32;

/// A place in the sky: x and y across the unit square, as 32-bit fractions.
pub type Place = (u32, u32);

/// Where `key` is in the sky. `kind` keeps people's and machines' skies
/// apart.
pub fn place(kind: &str, key: &impl Hash) -> Place {
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
    "osp sky".hash(&mut h);
    kind.hash(&mut h);
    key.hash(&mut h);
    let out = h.0.finalize();
    let b = out.as_bytes();
    (
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
    )
}

/// One of the 4ⁿ squares at level `n`, numbered across (`x`) and down
/// (`y`) from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cell {
    pub level: u8,
    pub x: u32,
    pub y: u32,
}

impl Cell {
    /// The whole sky.
    pub const ALL: Cell = Cell {
        level: 0,
        x: 0,
        y: 0,
    };

    /// The cell at `level` holding `place`.
    pub fn of(place: Place, level: u8) -> Cell {
        let shift = DEEPEST - level;
        let cut = |v: u32| v.checked_shr(shift as u32).unwrap_or(0);
        Cell {
            level,
            x: cut(place.0),
            y: cut(place.1),
        }
    }

    pub fn contains(&self, place: Place) -> bool {
        Cell::of(place, self.level) == *self
    }

    /// Its four quarters, or none at the deepest level.
    pub fn quarters(&self) -> Vec<Cell> {
        if self.level >= DEEPEST {
            return Vec::new();
        }
        let level = self.level + 1;
        [(0, 0), (1, 0), (0, 1), (1, 1)]
            .into_iter()
            .map(|(dx, dy)| Cell {
                level,
                x: 2 * self.x + dx,
                y: 2 * self.y + dy,
            })
            .collect()
    }

    /// Its centre, across the unit square.
    pub fn centre(&self) -> [f64; 2] {
        let side = 1.0 / (1u64 << self.level) as f64;
        [(self.x as f64 + 0.5) * side, (self.y as f64 + 0.5) * side]
    }

    /// The range of Z-order codes inside it.
    fn span(&self) -> (u64, u64) {
        let bits = 2 * (DEEPEST - self.level) as u32;
        let start = z_order(self.x, self.y).checked_shl(bits).unwrap_or(0);
        let end = start.saturating_add(1u64.checked_shl(bits).unwrap_or(0).wrapping_sub(1));
        (start, end)
    }
}

/// What a snapshot shows of the sky.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Light {
    /// Many people, drawn as one light: brighter the more it holds.
    Cell {
        cell: Cell,
        people: u32,
        creators: u32,
    },
    /// One person, by the index the caller gave.
    Star(u32),
}

/// The listed people, sorted along a Z-order curve, so each cell's people
/// are one run.
pub struct Sky {
    /// (Z-order code, the caller's index).
    stars: Vec<(u64, u32)>,
    /// How many creators come before each place in `stars`.
    creators_before: Vec<u32>,
}

impl Sky {
    /// `people`: each one's place and whether they're a creator, indexed
    /// as the caller wants `Light::Star` to name them.
    pub fn new(people: impl IntoIterator<Item = (Place, bool)>) -> Sky {
        let mut stars: Vec<(u64, u32, bool)> = people
            .into_iter()
            .enumerate()
            .map(|(i, ((x, y), creator))| (z_order(x, y), i as u32, creator))
            .collect();
        stars.sort_unstable();
        let mut creators_before = Vec::with_capacity(stars.len() + 1);
        let mut count = 0;
        creators_before.push(0);
        for s in &stars {
            count += s.2 as u32;
            creators_before.push(count);
        }
        Sky {
            stars: stars.into_iter().map(|(z, i, _)| (z, i)).collect(),
            creators_before,
        }
    }

    /// Where `cell`'s people are in `stars`.
    fn run(&self, cell: Cell) -> std::ops::Range<usize> {
        let (start, end) = cell.span();
        let from = self.stars.partition_point(|(z, _)| *z < start);
        let to = self.stars.partition_point(|(z, _)| *z <= end);
        from..to
    }

    pub fn people_in(&self, cell: Cell) -> u32 {
        self.run(cell).len() as u32
    }

    fn light(&self, cell: Cell) -> Light {
        let run = self.run(cell);
        match run.len() {
            1 => Light::Star(self.stars[run.start].1),
            n => Light::Cell {
                cell,
                people: n as u32,
                creators: self.creators_before[run.end] - self.creators_before[run.start],
            },
        }
    }

    /// `region` opened into at most `budget` lights: its fullest cell split
    /// into its quarters, again and again, while they fit. Empty cells
    /// aren't shown.
    pub fn open(&self, region: Cell, budget: usize) -> Vec<Light> {
        let mut lights: Vec<Light> = match self.people_in(region) {
            0 => Vec::new(),
            _ => vec![self.light(region)],
        };
        loop {
            // The fullest cell, the shallowest and first of equals.
            let fullest = lights
                .iter()
                .enumerate()
                .filter_map(|(i, l)| match l {
                    Light::Cell { cell, people, .. } if cell.level < DEEPEST => {
                        Some((i, *people, *cell))
                    }
                    _ => None,
                })
                .min_by_key(|(_, people, cell)| (std::cmp::Reverse(*people), *cell));
            let Some((i, _, cell)) = fullest else {
                break;
            };
            let parts: Vec<Light> = cell
                .quarters()
                .into_iter()
                .filter(|q| self.people_in(*q) > 0)
                .map(|q| self.light(q))
                .collect();
            if lights.len() - 1 + parts.len() > budget {
                break;
            }
            lights.splice(i..=i, parts);
        }
        lights
    }
}

/// x and y's bits interleaved, x's in the lower place of each pair.
fn z_order(x: u32, y: u32) -> u64 {
    fn spread(v: u32) -> u64 {
        let mut v = v as u64;
        v = (v | (v << 16)) & 0x0000_FFFF_0000_FFFF;
        v = (v | (v << 8)) & 0x00FF_00FF_00FF_00FF;
        v = (v | (v << 4)) & 0x0F0F_0F0F_0F0F_0F0F;
        v = (v | (v << 2)) & 0x3333_3333_3333_3333;
        v = (v | (v << 1)) & 0x5555_5555_5555_5555;
        v
    }
    spread(x) | (spread(y) << 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{Graph, build};
    use osp_proto::universe::MAX_POINTS;

    /// 5,000 people one directory lists, by made-up keys.
    fn listed() -> Vec<[u8; 32]> {
        (0..5_000u32)
            .map(|i| *blake3::hash(&i.to_le_bytes()).as_bytes())
            .collect()
    }

    /// Pearson's χ² of the places over a `side` × `side` grid.
    fn chi_squared(places: &[Place], side: u32) -> f64 {
        let mut counts = vec![0u32; (side * side) as usize];
        let level = side.trailing_zeros() as u8;
        for p in places {
            let c = Cell::of(*p, level);
            counts[(c.y * side + c.x) as usize] += 1;
        }
        let expected = places.len() as f64 / (side * side) as f64;
        counts
            .iter()
            .map(|c| (*c as f64 - expected).powi(2) / expected)
            .sum()
    }

    #[test]
    fn listed_people_with_no_lines_are_in_no_group_and_spread_evenly() {
        let keys = listed();
        // No lines: being listed by one directory groups no one.
        let tree = build(&Graph::new(keys.len(), []));
        assert!(tree.groups.is_empty());
        assert_eq!(tree.unplaced.len(), keys.len());

        // Spread evenly, at two scales. With 255 and 15 degrees of freedom,
        // these limits fail an even spread once in a thousand times.
        let places: Vec<Place> = keys.iter().map(|k| place("person", k)).collect();
        let fine = chi_squared(&places, 16);
        let coarse = chi_squared(&places, 4);
        assert!(fine < 330.5, "χ² {fine:.0} over 16 × 16");
        assert!(coarse < 37.7, "χ² {coarse:.0} over 4 × 4");

        // And no clumps: each person's nearest neighbour is about as far
        // as an even spread puts it (Clark and Evans' ratio, near 1).
        let mut nearest = 0.0;
        let unit = |p: Place| [p.0 as f64 / 2f64.powi(32), p.1 as f64 / 2f64.powi(32)];
        for (i, p) in places.iter().enumerate() {
            let a = unit(*p);
            let d = places
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, q)| {
                    let b = unit(*q);
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
                })
                .fold(f64::MAX, f64::min);
            nearest += d;
        }
        let ratio = (nearest / places.len() as f64) / (0.5 / (places.len() as f64).sqrt());
        println!(
            "χ² {fine:.0} over 16 × 16, {coarse:.1} over 4 × 4; nearest-neighbour ratio {ratio:.3}"
        );
        assert!(
            (0.9..1.1).contains(&ratio),
            "nearest-neighbour ratio {ratio:.2}"
        );
    }

    #[test]
    fn a_public_line_places_a_listed_person_and_their_sky_place_stays() {
        let keys = listed();
        let before = place("person", &keys[7]);
        let tree = build(&Graph::new(keys.len(), [(7, 8, 1.0)]));
        assert_eq!(tree.unplaced.len(), keys.len() - 2);
        assert!(tree.group_of[7].is_some() && tree.group_of[8].is_some());
        assert_eq!(
            place("person", &keys[7]),
            before,
            "a place is the key's alone"
        );
        assert_ne!(place("person", &keys[7]), place("machine", &keys[7]));
    }

    #[test]
    fn every_listed_person_is_reached_by_zooming_within_the_budget() {
        let keys = listed();
        let sky = Sky::new(
            keys.iter()
                .enumerate()
                .map(|(i, k)| (place("person", k), i % 7 == 0)),
        );
        let top = sky.open(Cell::ALL, MAX_POINTS);
        assert!(top.len() <= MAX_POINTS && top.len() > MAX_POINTS - 4);
        let shown: u32 = top
            .iter()
            .map(|l| match l {
                Light::Cell { people, .. } => *people,
                Light::Star(_) => 1,
            })
            .sum();
        assert_eq!(shown, keys.len() as u32, "everyone is in some light");
        let creators: u32 = top
            .iter()
            .map(|l| match l {
                Light::Cell { creators, .. } => *creators,
                Light::Star(i) => (*i % 7 == 0) as u32,
            })
            .sum();
        assert_eq!(creators, keys.len().div_ceil(7) as u32);

        // Each person: zoom into the light that holds them until it's them.
        let mut deepest = 0;
        for (i, k) in keys.iter().enumerate() {
            let at = place("person", k);
            let mut region = Cell::ALL;
            let mut zooms = 0;
            loop {
                let lights = sky.open(region, MAX_POINTS);
                assert!(lights.len() <= MAX_POINTS);
                let holder = lights.iter().find(|l| match l {
                    Light::Cell { cell, .. } => cell.contains(at),
                    Light::Star(s) => *s == i as u32,
                });
                match holder.expect("someone holds them") {
                    Light::Star(_) => break,
                    Light::Cell { cell, .. } => region = *cell,
                }
                zooms += 1;
                assert!(zooms < 10, "person {i} not reached");
            }
            deepest = deepest.max(zooms);
        }
        println!(
            "{} lights at the top; the furthest person {deepest} zoom(s) in",
            top.len()
        );
        assert!(deepest <= 2, "{deepest} zooms to the deepest person");
    }

    #[test]
    fn cells_nest_and_cover_their_quarters() {
        let p = place("person", &[1u8; 32]);
        let mut cell = Cell::of(p, DEEPEST);
        for level in (0..DEEPEST).rev() {
            let parent = Cell::of(p, level);
            assert!(parent.quarters().contains(&cell));
            assert!(parent.contains(p));
            cell = parent;
        }
        assert_eq!(cell, Cell::ALL);
        let sky = Sky::new([(p, false)]);
        assert_eq!(sky.open(Cell::ALL, 10), [Light::Star(0)]);
        assert_eq!(Sky::new([]).open(Cell::ALL, 10), []);
    }
}
