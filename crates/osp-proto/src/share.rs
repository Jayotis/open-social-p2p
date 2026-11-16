//! "Share my Universe" (ADR 0039): the card a home node makes for its owner
//! to share outside OSP.
//!
//! Kim's card leaves OSP for good, so it is built from its own allowlist,
//! stricter than the Universe's (ADR 0033): rounded totals, and a drawing
//! of her part of the Universe in which nobody is named unless she ticked
//! them. These types are that allowlist. There are no keys, addresses,
//! machine names or times in them, so none can be shared.

use serde::{Deserialize, Serialize};

/// Most points a card draws, Kim included.
pub const MAX_CARD_POINTS: usize = 120;

/// Most lines a card draws.
pub const MAX_CARD_LINES: usize = 240;

/// The grid a card's points are rounded to, in the Universe's units (about
/// ±1024 across): coarse enough that a point says only roughly where.
pub const CARD_GRID: f32 = 16.0;

/// Most people a card names.
pub const MAX_CARD_NAMES: usize = 12;

/// What a shared card holds (ADR 0039 §1, §2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShareCard {
    /// People the node holds a profile of, Kim aside.
    pub people: u64,
    /// The people Kim follows and the people she knows, as one number.
    pub direct: u64,
    /// Separate chains from Kim's side into the wider network (ADR 0037):
    /// chains of vouches from the farthest people on her side to her that
    /// share nobody but her.
    pub paths: u64,
    /// Bytes her node served of posts it reshared with hosting.
    pub bytes_served: u64,
    /// The drawing: Kim first, at the centre.
    pub points: Vec<CardPoint>,
    /// Lines between two points, by their place in `points`: public acts
    /// between them, with no kind, count or time.
    pub lines: Vec<[u32; 2]>,
}

/// One point of a card's drawing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardPoint {
    /// Where, rounded to `CARD_GRID`.
    pub at: [f32; 2],
    /// How many people it stands for: 1 for a person, more for a group,
    /// rounded to two figures.
    pub people: u32,
    /// Whether it's Kim.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub you: bool,
    /// A person's name, only if Kim ticked them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `n` rounded to two significant figures: 1834 is 1800, 68 is 68.
pub fn two_figures(n: u64) -> u64 {
    let mut scale = 1;
    while n / scale >= 100 {
        scale *= 10;
    }
    // Rounded half up, without overflowing near u64::MAX.
    let down = n / scale * scale;
    match n - down >= scale / 2 && scale > 1 {
        true => down.saturating_add(scale),
        false => down,
    }
}

/// A position rounded to `CARD_GRID`.
pub fn on_grid([x, y]: [f32; 2]) -> [f32; 2] {
    [
        (x / CARD_GRID).round() * CARD_GRID,
        (y / CARD_GRID).round() * CARD_GRID,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_keep_two_figures() {
        assert_eq!(two_figures(0), 0);
        assert_eq!(two_figures(7), 7);
        assert_eq!(two_figures(68), 68);
        assert_eq!(two_figures(99), 99);
        assert_eq!(two_figures(149), 150);
        assert_eq!(two_figures(1834), 1800);
        assert_eq!(two_figures(1_850_000_000), 1_900_000_000);
        assert_eq!(two_figures(u64::MAX), 18_000_000_000_000_000_000);
    }

    #[test]
    fn points_land_on_the_grid() {
        assert_eq!(on_grid([7.9, -8.1]), [0.0, -16.0]);
        assert_eq!(on_grid([-1023.99, 1000.0]), [-1024.0, 1008.0]);
    }
}
