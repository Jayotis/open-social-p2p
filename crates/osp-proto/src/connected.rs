//! "How are we connected?" (ADR 0037): the paths a home node knows from its
//! owner to someone, for its owner's devices to draw and write out.
//!
//! Kim follows Alice. Alice reshared Carol. Carol reshared Jimmy. A path is
//! that, hop by hop from Kim: each hop names the person it reaches and how.
//! The paths through Jimmy's side are the separate chains ADR 0029 counts,
//! from the same code that lets replies in, so they never share anyone but
//! Jimmy.

use serde::{Deserialize, Serialize};

use crate::PublicKey;

/// One path from the node's owner to someone, nearest hop first.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Path {
    pub hops: Vec<Hop>,
}

impl Path {
    /// The person the path ends at.
    pub fn end(&self) -> Option<PublicKey> {
        self.hops.last().map(|h| h.to)
    }

    /// Steps from the owner along it: a gap counts its length.
    pub fn steps(&self) -> u32 {
        self.hops.iter().map(|h| u32::from(h.how.steps())).sum()
    }
}

/// One hop of a path: from whoever the hop before reached (the owner, for the
/// first) to `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hop {
    pub to: PublicKey,
    #[serde(flatten)]
    pub how: How,
}

/// How a hop is known (ADR 0037 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "how", rename_all = "snake_case")]
pub enum How {
    /// Known: the owner follows `to`.
    Follows,
    /// Known: the owner knows `to` (trusted, ADR 0013).
    Knows,
    /// Signed evidence, checked: the one before reshared one of `to`'s posts.
    Reshared,
    /// Signed evidence, checked: the one before, a post's author, credited
    /// `to` with finding it early.
    CreditedEarly,
    /// A gap: the node of the one before placed `to` within `steps` steps of
    /// its owner (`PlaceWithin`). Nobody inside it is named, as the answer
    /// was given on terms.
    Gap { steps: u8 },
}

impl How {
    /// Steps the hop covers.
    pub fn steps(&self) -> u8 {
        match self {
            How::Gap { steps } => *steps,
            _ => 1,
        }
    }

    /// Whether it's the owner's own follow or someone they know.
    pub fn is_known(&self) -> bool {
        matches!(self, How::Follows | How::Knows)
    }

    /// Whether it's signed evidence the node checked.
    pub fn is_signed(&self) -> bool {
        matches!(self, How::Reshared | How::CreditedEarly)
    }
}

/// The answer to "How are we connected?": the paths to `actor`, nearest
/// first, and who's named on them. No paths means the node hasn't found a
/// connection, not that there is none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub actor: PublicKey,
    pub paths: Vec<Path>,
    /// Everyone a path names, each once.
    pub people: Vec<Named>,
    /// Where the paths through `actor`'s side came from.
    pub side: SideFrom,
}

/// Someone a path names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Named {
    pub actor: PublicKey,
    /// From their profile, if the node has one, or the one their vouch
    /// came with.
    pub name: Option<String>,
    /// Where they are on the Universe's map, as search gives it (ADR 0035
    /// §3); none for someone the node doesn't hold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<String>,
}

/// Where the paths through someone's side came from (ADR 0037 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum SideFrom {
    /// The side their node showed when they replied: the paths that let
    /// the reply in, or would have.
    Replied,
    /// Their node was asked who vouched for them, and named `vouches`
    /// people.
    Asked { vouches: u32 },
    /// Their node was asked and didn't answer.
    NoAnswer,
    /// Their node wasn't asked: the owner's own node, or one asked too often
    /// this hour, or none known. Only the owner's own world counts.
    NotAsked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hop_reads_as_its_kind_in_json() {
        let to = crate::SecretKey::generate().public();
        let gap = Hop {
            to,
            how: How::Gap { steps: 2 },
        };
        let json = serde_json::to_value(gap).unwrap();
        assert_eq!(json["how"], "gap");
        assert_eq!(json["steps"], 2);
        let path = Path {
            hops: vec![
                Hop {
                    to,
                    how: How::Follows,
                },
                gap,
            ],
        };
        assert_eq!((path.steps(), path.end()), (3, Some(to)));
    }
}
