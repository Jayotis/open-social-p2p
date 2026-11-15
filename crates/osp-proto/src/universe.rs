//! What the Universe may show (ADR 0033): the snapshot a home node gives its
//! own paired devices for `ClientRequest::Universe`.
//!
//! Every field here is a line of ADR 0033's tables, and nothing else is.
//! [`ALLOWLIST`] names them all as paths into the JSON, and a test fails if a
//! snapshot holds a field that isn't on it. These types never hold a
//! `Person` or a machine's key: profiles carry node endpoint IDs, and a
//! machine's name is one, so both are copied field by field.

use serde::{Deserialize, Serialize};

use crate::{BlobHash, ObjectId, PublicKey};

/// Most people in one snapshot (ADR 0034's starting values).
pub const MAX_POINTS: usize = 300;
/// Most public lines in one snapshot.
pub const MAX_LINES: usize = 600;
/// Largest snapshot, as JSON, in bytes.
pub const MAX_SNAPSHOT_BYTES: usize = 128 * 1024;
/// A line fades over this many days, as vouches do (ADR 0029).
pub const LINE_DAYS: u64 = 90;

/// The public sky (ADR 0034 §6) is a ring around the placed world, in the
/// same units as its places (a person is about one unit across): from this
/// far from the centre...
pub const SKY_INNER: f32 = 64.0;
/// ...to this far. It never moves, so its people keep their places
/// however the placed world grows.
pub const SKY_OUTER: f32 = 1024.0;

/// What a snapshot opens (ADR 0034 §2, §6): a request's `region`, as text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Region {
    /// No region: the whole world around the owner, and the public sky.
    #[default]
    Top,
    /// A group of the tree, by its lasting ID: `g:42`.
    Group(u64),
    /// Standing at a person: `p:` and their key.
    Person(PublicKey),
    /// A cell of the public sky: `sky:level/x/y`, one of the 4^level
    /// squares the sky's places are cut into, numbered across and down.
    Sky { level: u8, x: u32, y: u32 },
}

/// The deepest sky cell: a place's full precision.
pub const SKY_DEEPEST: u8 = 32;

impl Region {
    /// The region `text` names, or none if it names nothing. No text, or
    /// empty text, is the top.
    pub fn parse(text: Option<&str>) -> Option<Region> {
        let text = match text {
            None | Some("") => return Some(Region::Top),
            Some(t) => t,
        };
        if let Some(id) = text.strip_prefix("g:") {
            return id.parse().ok().map(Region::Group);
        }
        if let Some(key) = text.strip_prefix("p:") {
            return key.parse().ok().map(Region::Person);
        }
        let mut parts = text.strip_prefix("sky:")?.split('/');
        let level: u8 = parts.next()?.parse().ok()?;
        let x: u32 = parts.next()?.parse().ok()?;
        let y: u32 = parts.next()?.parse().ok()?;
        let side = 1u64 << level.min(SKY_DEEPEST);
        let fits = level <= SKY_DEEPEST && (x as u64) < side && (y as u64) < side;
        (parts.next().is_none() && fits).then_some(Region::Sky { level, x, y })
    }
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Region::Top => Ok(()),
            Region::Group(id) => write!(f, "g:{id}"),
            Region::Person(key) => write!(f, "p:{key}"),
            Region::Sky { level, x, y } => write!(f, "sky:{level}/{x}/{y}"),
        }
    }
}

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
    /// Lights up where recent public posts are (ADR 0035 §1): each
    /// person's, group's and sky cell's `lit` counts them. With `People`.
    Posts,
    /// Lights up where recent public video is: posts with a video.
    Video,
    /// Lights up where reshares are spreading: recent reshares.
    Shared,
}

impl Layer {
    /// Whether it lights up the people layer, rather than adding to it.
    pub fn lights(self) -> bool {
        matches!(self, Layer::Posts | Layer::Video | Layer::Shared)
    }
}

/// Every layer, for a request that names none. A layer that lights up
/// (`Posts`, `Video`, `Shared`) is asked for by name, one at a time.
pub const ALL_LAYERS: [Layer; 3] = [Layer::People, Layer::Machines, Layer::Network];

/// What a lit layer counts: items from this many days back.
pub const LIT_DAYS: u64 = 90;

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The node's answer to `Universe`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Changes whenever anything in the snapshot does. A request whose
    /// `since` is this gets `UniverseUnchanged`.
    pub version: String,
    /// The band of what's shown: `Universe` at the top while it shows
    /// groups, `Cluster` inside a group that shows groups, `Local` when
    /// it shows only people, `Close` standing at someone.
    pub level: Level,
    /// What this snapshot opens, as a request's `region` names it: empty
    /// for the top. A group that's gone since the device asked for it is
    /// answered by the group most of its people went to, named here.
    #[serde(default)]
    pub region: String,
    /// The regions above it, the top (empty) first, so a device zooming out
    /// asks for each in turn and stays where it is (ADR 0034 §6).
    #[serde(default)]
    pub path: Vec<String>,
    /// The groups shown closed, each one light (ADR 0034 §2).
    #[serde(default)]
    pub groups: Vec<GroupPoint>,
    /// The owner first, then the one stood at, then the nearest and most
    /// active.
    pub people: Vec<PersonPoint>,
    /// Public acts between people in `people`, newest first.
    pub lines: Vec<Line>,
    /// People the node knows who didn't fit.
    pub more_people: u64,
    pub machines: Vec<MachinePoint>,
    pub network: Option<NetworkTotals>,
    /// The public sky's cells shown, each many people drawn as one light.
    /// Its people shown one by one are in `people`.
    #[serde(default)]
    pub sky: Vec<SkyCell>,
}

/// A group of the tree, shown as one light (ADR 0034 §2). Its people are
/// grouped only by the public lines between them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupPoint {
    /// Lasting while more than half its people stay in it (ADR 0034 §4).
    pub id: u64,
    pub parent: Option<u64>,
    /// For a group new since the last grouping: the group most of its
    /// people came from, so a device can draw the split.
    pub from: Option<u64>,
    /// Everyone in it, at any depth.
    pub people: u32,
    /// 0 to 100 (ADR 0034 §5).
    pub bright: u8,
    pub at: [f32; 2],
    /// With a lit layer asked for: its people's recent items of that kind
    /// held here, at any depth. Left out when none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lit: u32,
}

/// Many people of the public sky, drawn as one light. Only for drawing at
/// scale: it isn't a group, and its people aren't related (ADR 0034 §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkyCell {
    pub level: u8,
    pub x: u32,
    pub y: u32,
    pub people: u32,
    /// Those of them with public posts lately.
    pub creators: u32,
    /// Its centre, in the placed world's units.
    pub at: [f32; 2],
    /// As a group's `lit`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lit: u32,
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
    /// Where they are: placed by their lines, or in the public sky at a
    /// place hashed from their key. The owner is at the centre.
    #[serde(default)]
    pub at: [f32; 2],
    /// The group they're directly in; none in the sky.
    #[serde(default)]
    pub group: Option<u64>,
    /// 0 to 100 (ADR 0034 §5).
    #[serde(default)]
    pub bright: u8,
    /// As a group's `lit`: their own recent items of the layer's kind. It
    /// never moves them.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lit: u32,
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
    "region",
    "path",
    "groups",
    "groups[].id",
    "groups[].parent",
    "groups[].from",
    "groups[].people",
    "groups[].bright",
    "groups[].at",
    "groups[].lit",
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
    "people[].at",
    "people[].group",
    "people[].bright",
    "people[].lit",
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
    "sky",
    "sky[].level",
    "sky[].x",
    "sky[].y",
    "sky[].people",
    "sky[].creators",
    "sky[].at",
    "sky[].lit",
];

/// How long a node keeps its events (ADR 0036 §1)...
pub const EVENT_HOURS: u64 = 24;
/// ...and at most how many.
pub const MAX_EVENTS: usize = 500;
/// Events of one kind about one thing within this long of the first are
/// summed into one, drawn once: "served 14 copies in the last hour".
pub const SUM_MINUTES: u64 = 60;

/// Something the node did or saw (ADR 0036 §1), for `UniverseEvents`. Only
/// its owner's devices get it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Rises with each new event, and each one summed into: a device asks
    /// for those since the highest it holds.
    pub seq: u64,
    /// When it first and last happened, in seconds since 1970, to the
    /// minute.
    pub first: u64,
    pub last: u64,
    /// How many times it happened in that while.
    pub count: u32,
    #[serde(flatten)]
    pub what: EventWhat,
}

/// What happened, by kind. Each is a line in ADR 0033's tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventWhat {
    /// A directory read brought `people` this node didn't hold before.
    Found {
        /// The directory's machine name, in words, as `found_through`.
        directory: String,
        people: u32,
    },
    /// `from` reshared `post`, by `author`, which this node holds: a public
    /// signed act, made in the last day. Summed by `from` and `author`;
    /// `post` is the newest.
    Reshared {
        from: PublicKey,
        author: PublicKey,
        post: ObjectId,
    },
    /// The node fetched media in the background, to have it ready or to
    /// keep a copy: never media its owner opened (ADR 0033 §4).
    Fetched {
        /// Whose media it is, and its post, when the node can tell.
        author: Option<PublicKey>,
        post: Option<ObjectId>,
        bytes: u64,
        /// The resharer who sent most of it, only when it's listed as
        /// hosting that post (ADR 0025). Otherwise it was "another node".
        host: Option<PublicKey>,
        /// The author's node couldn't be reached, and a copy sent it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        kept_by_copy: bool,
    },
    /// The node sent media to another node: never which one.
    Served {
        author: Option<PublicKey>,
        post: Option<ObjectId>,
        bytes: u64,
    },
    /// A machine answered, first or after it had faded (`met`), or
    /// stopped answering. By its words, never its key.
    Machine { words: String, met: bool },
}

/// Every field an event may hold, as [`ALLOWLIST`] writes them: `[]` for
/// the reply's list.
pub const EVENT_ALLOWLIST: &[&str] = &[
    "events",
    "events[].seq",
    "events[].first",
    "events[].last",
    "events[].count",
    "events[].kind",
    "events[].directory",
    "events[].people",
    "events[].from",
    "events[].author",
    "events[].post",
    "events[].bytes",
    "events[].host",
    "events[].kept_by_copy",
    "events[].words",
    "events[].met",
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
            region: "g:1".into(),
            path: vec![String::new()],
            groups: vec![GroupPoint {
                id: 1,
                parent: Some(2),
                from: Some(3),
                people: 2,
                bright: 10,
                at: [1.0, 2.0],
                lit: 4,
            }],
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
                at: [0.0, 0.0],
                group: Some(1),
                bright: 5,
                lit: 1,
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
            sky: vec![SkyCell {
                level: 1,
                x: 0,
                y: 1,
                people: 9,
                creators: 2,
                at: [100.0, -100.0],
                lit: 3,
            }],
        }
    }

    #[test]
    fn events_hold_exactly_the_fields_on_their_allowlist() {
        let post = ObjectId::from_bytes([4; 32]);
        let whats = [
            EventWhat::Found {
                directory: "able-baker-charlie".into(),
                people: 3,
            },
            EventWhat::Reshared {
                from: key(1),
                author: key(2),
                post,
            },
            EventWhat::Fetched {
                author: Some(key(2)),
                post: Some(post),
                bytes: 10,
                host: Some(key(1)),
                kept_by_copy: true,
            },
            EventWhat::Served {
                author: Some(key(2)),
                post: Some(post),
                bytes: 10,
            },
            EventWhat::Machine {
                words: "able-baker-charlie".into(),
                met: true,
            },
        ];
        let events: Vec<Event> = whats
            .into_iter()
            .enumerate()
            .map(|(n, what)| Event {
                seq: n as u64,
                first: 60,
                last: 120,
                count: 1,
                what,
            })
            .collect();
        let json = serde_json::json!({ "events": events });
        let allowed: std::collections::BTreeSet<String> =
            EVENT_ALLOWLIST.iter().map(|s| s.to_string()).collect();
        assert_eq!(field_paths(&json), allowed);
        let back: Vec<Event> = serde_json::from_value(json["events"].clone()).unwrap();
        assert_eq!(back, events);
    }

    #[test]
    fn regions_are_read_back_as_written() {
        for region in [
            Region::Top,
            Region::Group(42),
            Region::Person(key(3)),
            Region::Sky {
                level: 3,
                x: 7,
                y: 0,
            },
            Region::Sky {
                level: SKY_DEEPEST,
                x: u32::MAX,
                y: u32::MAX,
            },
        ] {
            assert_eq!(Region::parse(Some(&region.to_string())), Some(region));
        }
        assert_eq!(Region::parse(None), Some(Region::Top));
        for bad in [
            "g:",
            "g:x",
            "p:nope",
            "sky:1/2/0",
            "sky:33/0/0",
            "sky:1/0",
            "x",
        ] {
            assert_eq!(Region::parse(Some(bad)), None, "{bad}");
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
