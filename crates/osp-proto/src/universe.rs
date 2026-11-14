//! What the Universe may show (ADR 0033): the snapshot a home node gives its
//! own paired devices for `ClientRequest::Universe`.
//!
//! Every field here is a line of ADR 0033's tables, and nothing else is.
//! [`ALLOWLIST`] names them all as paths into the JSON, and a test fails if a
//! snapshot holds a field that isn't on it. These types never hold a
//! `Person` or a machine's key: profiles carry node endpoint IDs, and a
//! machine's name is one, so both are copied field by field.

use serde::{Deserialize, Serialize};

use crate::{BlobHash, PublicKey};

/// Most people in one snapshot (ADR 0034's starting values).
pub const MAX_POINTS: usize = 300;
/// Most public lines in one snapshot.
pub const MAX_LINES: usize = 600;
/// Largest snapshot, as JSON, in bytes.
pub const MAX_SNAPSHOT_BYTES: usize = 128 * 1024;
/// A line fades over this many days, as vouches do (ADR 0029).
pub const LINE_DAYS: u64 = 90;

/// How close the camera is (ADR 0034's zoom bands).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Universe,
    Galaxy,
    Cluster,
    #[default]
    Local,
    Close,
}

/// What a snapshot draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// People and the public lines between them.
    People,
    /// The machines this node's machine name knows, apart from people.
    Machines,
    /// This node's own totals.
    Network,
}

/// Every layer, for a request that names none.
pub const ALL_LAYERS: [Layer; 3] = [Layer::People, Layer::Machines, Layer::Network];

/// The node's answer to `Universe`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Changes whenever anything in the snapshot does. A request whose
    /// `since` is this gets `UniverseUnchanged`.
    pub version: String,
    /// The band this snapshot is for. Until the tree (ADR 0034) exists,
    /// always `Local`: the whole world the node knows, within the limits.
    pub level: Level,
    /// The owner first, then the nearest and most active.
    pub people: Vec<PersonPoint>,
    /// Public acts between people in `people`, newest first.
    pub lines: Vec<Line>,
    /// People the node knows who didn't fit.
    pub more_people: u64,
    pub machines: Vec<MachinePoint>,
    pub network: Option<NetworkTotals>,
}

/// One person (ADR 0033's "People").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonPoint {
    pub actor: PublicKey,
    pub name: String,
    /// Their profile picture's file, which a device reads with `ReadBlob`.
    pub picture: Option<BlobHash>,
    /// Steps from the owner: 0 the owner, 1 their circle (follows, people
    /// they know), 2 people their circle reshared, 3 their gathered side.
    /// None for anyone else, such as a person a directory brought.
    pub step: Option<u8>,
    pub followed: bool,
    /// Among the people the owner knows (trusted, ADR 0013).
    pub known: bool,
    /// The directory they came from, in words, if they're here only
    /// because of one.
    pub found_through: Option<String>,
    /// Their posts, posts with media, and reshares this node holds.
    pub posts: u64,
    pub media: u64,
    pub reshares: u64,
    /// The day of their newest post or reshare here (`YYYY-MM-DD`, UTC).
    pub last_post: Option<String>,
}

/// A public, signed act between two people, each named by their place in
/// the snapshot's `people`, which keeps a line small: a key is 64
/// characters, and there can be 600 lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub kind: LineKind,
    pub from: u32,
    pub to: u32,
    /// How many such acts there are from `from` to `to`, within 90 days.
    pub count: u32,
    /// The day of the newest (`YYYY-MM-DD`, UTC).
    pub last: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    /// `from` reshared one of `to`'s posts.
    Reshared,
    /// `from`, a post's author, credited `to` with finding it early.
    CreditedEarly,
}

/// One machine (ADR 0033's "Machines"). Drawn apart from people, with no
/// line to any person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachinePoint {
    /// Its machine name, in words. Never its key.
    pub words: String,
    pub state: MachineState,
    /// Its record says it serves a directory.
    pub directory: bool,
    pub activity: ActivityBand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    /// A record of it arrived; it hasn't answered yet.
    HeardOf,
    /// It answered, lately.
    Met,
    /// It has answered for a month, and lately.
    AnsweredLately,
    /// It answered once, but not lately.
    Faded,
}

/// A machine's recent machine-protocol interaction with this node: greetings
/// it answered and records it passed on that checked out, never as a number.
/// It isn't work for the network, such as bytes carried or hosting: those
/// words are kept for that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityBand {
    None,
    Some,
    ALot,
}

/// The owner's own totals (ADR 0033's "Network").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkTotals {
    /// Posts this node reshared with hosting, and the bytes it has served
    /// of them.
    pub posts_hosted: u64,
    pub bytes_served: u64,
    /// People this node holds because a directory brought them.
    pub people_found: u64,
}

/// Every field a snapshot may hold, as a path into its JSON: `[]` for an
/// array's items. Each is a line in ADR 0033's tables. Adding a field
/// means adding it here and there.
pub const ALLOWLIST: &[&str] = &[
    "version",
    "level",
    "people",
    "people[].actor",
    "people[].name",
    "people[].picture",
    "people[].step",
    "people[].followed",
    "people[].known",
    "people[].found_through",
    "people[].posts",
    "people[].media",
    "people[].reshares",
    "people[].last_post",
    "lines",
    "lines[].kind",
    "lines[].from",
    "lines[].to",
    "lines[].count",
    "lines[].last",
    "more_people",
    "machines",
    "machines[].words",
    "machines[].state",
    "machines[].directory",
    "machines[].activity",
    "network",
    "network.posts_hosted",
    "network.bytes_served",
    "network.people_found",
];

/// Every field path in a JSON value, as [`ALLOWLIST`] writes them.
pub fn field_paths(value: &serde_json::Value) -> std::collections::BTreeSet<String> {
    fn walk(v: &serde_json::Value, at: &str, out: &mut std::collections::BTreeSet<String>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    let path = match at {
                        "" => k.clone(),
                        _ => format!("{at}.{k}"),
                    };
                    out.insert(path.clone());
                    walk(v, &path, out);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, &format!("{at}[]"), out);
                }
            }
            _ => {}
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(value, "", &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretKey;

    fn key(n: u8) -> PublicKey {
        SecretKey::from_bytes(&[n; 32]).public()
    }

    /// A snapshot with every field set, so each one shows in its JSON.
    fn full() -> Snapshot {
        Snapshot {
            version: "v".into(),
            level: Level::Local,
            people: vec![PersonPoint {
                actor: key(1),
                name: "Kim".into(),
                picture: Some(BlobHash::from_bytes([2; 32])),
                step: Some(0),
                followed: false,
                known: false,
                found_through: Some("able-baker-charlie".into()),
                posts: 1,
                media: 1,
                reshares: 1,
                last_post: Some("2026-11-14".into()),
            }],
            lines: vec![Line {
                kind: LineKind::Reshared,
                from: 0,
                to: 0,
                count: 1,
                last: "2026-11-14".into(),
            }],
            more_people: 0,
            machines: vec![MachinePoint {
                words: "able-baker-charlie".into(),
                state: MachineState::Met,
                directory: true,
                activity: ActivityBand::ALot,
            }],
            network: Some(NetworkTotals {
                posts_hosted: 1,
                bytes_served: 1,
                people_found: 1,
            }),
        }
    }

    #[test]
    fn a_snapshot_holds_exactly_the_fields_on_the_allowlist() {
        let json = serde_json::to_value(full()).unwrap();
        let fields = field_paths(&json);
        let allowed: std::collections::BTreeSet<String> =
            ALLOWLIST.iter().map(|s| s.to_string()).collect();
        assert_eq!(fields, allowed);
    }
}
