//! What the Universe may show (ADR 0033): the snapshot built for the owner's
//! paired devices, on the node alone.
//!
//! It reads what the node already knows and copies across only the fields
//! of `osp_proto::universe`, which are ADR 0033's allowlist. Nothing here
//! sends anything to another node: the Universe never asks the network for
//! more, just so there's something to draw.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use iroh::PublicKey;
use osp_proto::{
    Payload, SignedObject,
    rpc::ClientResponse,
    universe::{
        ALL_LAYERS, ActivityBand, LINE_DAYS, Layer, Level, Line, LineKind, MAX_LINES, MAX_POINTS,
        MAX_SNAPSHOT_BYTES, MachinePoint, MachineState, NetworkTotals, PersonPoint, Snapshot,
    },
};

use crate::{
    db::now_ms,
    machine,
    meet::{Seen, SeenState},
    node::Core,
};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

impl Core {
    /// Answers `Universe`.
    pub(crate) fn universe(
        &self,
        _level: Level,
        layers: &[Layer],
        since: Option<&str>,
    ) -> Result<ClientResponse> {
        let layers = match layers.is_empty() {
            true => &ALL_LAYERS[..],
            false => layers,
        };
        let snapshot = self.snapshot(layers, now_ms())?;
        Ok(match since == Some(snapshot.version.as_str()) {
            true => ClientResponse::UniverseUnchanged,
            false => ClientResponse::Universe {
                snapshot: Box::new(snapshot),
            },
        })
    }

    fn snapshot(&self, layers: &[Layer], now: i64) -> Result<Snapshot> {
        let (people, lines, more_people) = match layers.contains(&Layer::People) {
            true => self.people_layer(now)?,
            false => (vec![], vec![], 0),
        };
        let machines = match (layers.contains(&Layer::Machines), &self.machine) {
            (true, Some(side)) => machine_points(side.acquaintances.seen()),
            _ => vec![],
        };
        let network = match layers.contains(&Layer::Network) {
            true => {
                let (posts_hosted, bytes_served) = self.db.shared_served()?;
                Some(NetworkTotals {
                    posts_hosted,
                    bytes_served,
                    people_found: self.db.found_count()?,
                })
            }
            false => None,
        };
        fit(Snapshot {
            version: String::new(),
            level: Level::Local,
            people,
            lines,
            more_people,
            machines,
            network,
        })
    }

    /// The people the node knows, nearest and most active first, within the
    /// limits, and the public lines between them.
    fn people_layer(&self, now: i64) -> Result<(Vec<PersonPoint>, Vec<Line>, u64)> {
        let me = self.signer.identity();
        let followed: HashSet<PublicKey> = self.db.follows()?.iter().map(|f| f.actor).collect();
        let known: HashSet<PublicKey> = self.db.trusted()?.iter().map(|t| t.actor).collect();
        let found = self.db.found_directories()?;

        let lines = self.lines(now)?;
        // Step 1: the owner's circle. Step 2: whom the circle, or the owner,
        // reshared. Step 3: the owner's gathered side.
        let circle: HashSet<PublicKey> = followed.union(&known).copied().collect();
        let mut steps: HashMap<PublicKey, u8> = circle.iter().map(|a| (*a, 1)).collect();
        steps.insert(me, 0);
        for line in &lines {
            if line.kind == LineKind::Reshared && (line.from == me || circle.contains(&line.from)) {
                steps.entry(line.to).or_insert(2);
            }
        }
        for side in self.db.side()? {
            steps.entry(side.voucher).or_insert(3);
        }

        let mut people = Vec::new();
        let (posts, media, reshares, newest) = self.db.own_counts(&me)?;
        if let Payload::Person(own) = self.profile().verify()? {
            people.push((
                PersonPoint {
                    actor: me,
                    name: own.name,
                    picture: own.icon.map(|icon| icon.blob),
                    step: Some(0),
                    followed: false,
                    known: false,
                    found_through: None,
                    posts,
                    media,
                    reshares,
                    last_post: newest.map(day),
                },
                newest,
            ));
        }
        for held in self.db.held_people(&me)? {
            let Ok(Payload::Person(person)) = held.profile.verify() else {
                continue;
            };
            let actor = person.actor;
            people.push((
                PersonPoint {
                    actor,
                    name: person.name,
                    picture: person.icon.map(|icon| icon.blob),
                    step: steps.get(&actor).copied(),
                    followed: followed.contains(&actor),
                    known: known.contains(&actor),
                    found_through: found.get(&actor).map(machine::words),
                    posts: held.posts,
                    media: held.media,
                    reshares: held.reshares,
                    last_post: held.newest_ms.map(day),
                },
                held.newest_ms,
            ));
        }
        // Nearest first, those with no step last; then the newest posters.
        people.sort_by(|(a, a_new), (b, b_new)| {
            let step = |p: &PersonPoint| p.step.unwrap_or(u8::MAX);
            step(a)
                .cmp(&step(b))
                .then(b_new.cmp(a_new))
                .then(a.name.cmp(&b.name))
        });
        let more = people.len().saturating_sub(MAX_POINTS) as u64;
        let people: Vec<PersonPoint> = people
            .into_iter()
            .take(MAX_POINTS)
            .map(|(p, _)| p)
            .collect();
        let kept: HashSet<_> = people.iter().map(|p| p.actor).collect();
        let mut lines: Vec<Line> = lines
            .into_iter()
            .filter(|l| kept.contains(&l.from) && kept.contains(&l.to))
            .collect();
        lines.truncate(MAX_LINES);
        Ok((people, lines, more))
    }

    /// Public acts between people within the last 90 days, one line per kind
    /// and pair, newest first.
    fn lines(&self, now: i64) -> Result<Vec<Line>> {
        let since = now - LINE_DAYS as i64 * DAY_MS;
        let mut lines: HashMap<(LineKind, PublicKey, PublicKey), (u32, i64)> = HashMap::new();
        let mut seen = HashSet::new();
        for obj in self.db.public_acts(since)? {
            // The same act can be kept in more than one table.
            if !seen.insert(obj.id()) {
                continue;
            }
            let Some((kind, from, to, at)) = public_act(&obj) else {
                continue;
            };
            if from == to || at < since {
                continue;
            }
            let line = lines.entry((kind, from, to)).or_insert((0, at));
            line.0 += 1;
            line.1 = line.1.max(at);
        }
        let mut lines: Vec<(Line, i64)> = lines
            .into_iter()
            .map(|((kind, from, to), (count, at))| {
                (
                    Line {
                        kind,
                        from,
                        to,
                        count,
                        last: day(at),
                    },
                    at,
                )
            })
            .collect();
        lines.sort_by(|(a, a_at), (b, b_at)| {
            b_at.cmp(a_at)
                .then(a.from.as_bytes().cmp(b.from.as_bytes()))
                .then(a.to.as_bytes().cmp(b.to.as_bytes()))
        });
        Ok(lines.into_iter().map(|(l, _)| l).collect())
    }
}

/// Makes a snapshot fit in `MAX_SNAPSHOT_BYTES`, and gives it its version.
/// The least near people go first, with their lines, a tenth at a time. A
/// full machine table fits beside a person or two (a test checks), so
/// machines go only as a last resort, should that table ever grow.
fn fit(mut snapshot: Snapshot) -> Result<Snapshot> {
    // Room for the version, added once it fits.
    const ROOM: usize = 64;
    loop {
        let len = serde_json::to_vec(&snapshot)?.len();
        if len <= MAX_SNAPSHOT_BYTES - ROOM {
            break;
        }
        if snapshot.people.len() > 1 {
            let drop = (snapshot.people.len() / 10).max(1);
            snapshot.people.truncate(snapshot.people.len() - drop);
            snapshot.more_people += drop as u64;
            let kept: HashSet<_> = snapshot.people.iter().map(|p| p.actor).collect();
            snapshot
                .lines
                .retain(|l| kept.contains(&l.from) && kept.contains(&l.to));
        } else if !snapshot.machines.is_empty() {
            let drop = (snapshot.machines.len() / 10).max(1);
            snapshot.machines.truncate(snapshot.machines.len() - drop);
        } else {
            // One person, no machines: only a line list this long could
            // be left, and lines need two people.
            snapshot.lines.clear();
            break;
        }
    }
    snapshot.version = blake3::hash(&serde_json::to_vec(&snapshot)?).to_hex()[..16].to_string();
    Ok(snapshot)
}

/// A reshare or found-early credit, checked: its kind, from whom to whom,
/// and its date.
fn public_act(obj: &SignedObject) -> Option<(LineKind, PublicKey, PublicKey, i64)> {
    let (kind, from, to, published) = match obj.verify().ok()? {
        Payload::Announce(a) => (LineKind::Reshared, a.actor, a.object_actor, a.published),
        Payload::FoundEarly(f) => (LineKind::CreditedEarly, f.actor, f.finder, f.published),
        _ => return None,
    };
    (obj.signer() == from).then_some(())?;
    Some((kind, from, to, osp_proto::parse_rfc3339_ms(&published)?))
}

fn machine_points(seen: Vec<Seen>) -> Vec<MachinePoint> {
    let mut points: Vec<MachinePoint> = seen
        .into_iter()
        .map(|s| MachinePoint {
            words: machine::words(&s.machine),
            state: match s.state {
                SeenState::HeardOf => MachineState::HeardOf,
                SeenState::Met => MachineState::Met,
                SeenState::AnsweredLately => MachineState::AnsweredLately,
                SeenState::Faded => MachineState::Faded,
            },
            directory: s.directory,
            activity: band(s.activity),
        })
        .collect();
    points.sort_by(|a, b| a.words.cmp(&b.words));
    points
}

/// A machine's recent activity with this node in bands, never as its score
/// (ADR 0033).
fn band(activity: f64) -> ActivityBand {
    match activity {
        a if a < 1.0 => ActivityBand::None,
        a if a < 20.0 => ActivityBand::Some,
        _ => ActivityBand::ALot,
    }
}

/// The day of a time, as `YYYY-MM-DD` in UTC.
fn day(ms: i64) -> String {
    osp_proto::rfc3339_from_ms(ms.max(0) as u64)[..10].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    use osp_proto::{
        BlobHash, SecretKey,
        types::MAX_NAME_BYTES,
        universe::{ActivityBand, MachinePoint, MachineState},
    };

    use crate::meet::MAX_MACHINES;

    /// The largest snapshot the node could be asked to send: the most
    /// people, each with the longest name JSON can make of a profile's
    /// (control characters, six bytes each), the most lines, and a full
    /// machine table with the longest words.
    fn worst_case(name: &str) -> Snapshot {
        let keys: Vec<PublicKey> = (0..MAX_POINTS)
            .map(|i| {
                let mut seed = [0u8; 32];
                seed[..8].copy_from_slice(&(i as u64).to_le_bytes());
                SecretKey::from_bytes(&seed).public()
            })
            .collect();
        let people = keys
            .iter()
            .map(|actor| PersonPoint {
                actor: *actor,
                name: name.to_string(),
                picture: Some(BlobHash::from_bytes([0xff; 32])),
                step: Some(3),
                followed: true,
                known: true,
                found_through: Some("abstract-abstract-abstract".into()),
                posts: u64::MAX,
                media: u64::MAX,
                reshares: u64::MAX,
                last_post: Some("2026-11-14".into()),
            })
            .collect();
        let lines = (0..MAX_LINES)
            .map(|i| Line {
                kind: LineKind::CreditedEarly,
                from: keys[i % MAX_POINTS],
                to: keys[(i * 7 + 1) % MAX_POINTS],
                count: u32::MAX,
                last: "2026-11-14".into(),
            })
            .collect();
        let machines = (0..MAX_MACHINES)
            .map(|_| MachinePoint {
                words: "abstract-abstract-abstract".into(),
                state: MachineState::AnsweredLately,
                directory: true,
                activity: ActivityBand::ALot,
            })
            .collect();
        Snapshot {
            version: String::new(),
            level: Level::Local,
            people,
            lines,
            more_people: u64::MAX / 2,
            machines,
            network: Some(NetworkTotals {
                posts_hosted: u64::MAX,
                bytes_served: u64::MAX,
                people_found: u64::MAX,
            }),
        }
    }

    /// The 128 KiB contract (ADR 0034) holds however large the world, and
    /// a full machine table always fits: only people and their lines are
    /// trimmed. If the table grows past what fits, this fails, rather than
    /// the Universe quietly dropping machines or growing past its limit.
    #[test]
    fn the_largest_snapshot_fits_its_limit_with_every_machine() {
        for name in [
            "Kim".to_string(),
            "n".repeat(MAX_NAME_BYTES),
            "\u{1}".repeat(MAX_NAME_BYTES),
        ] {
            let fitted = fit(worst_case(&name)).unwrap();
            let len = serde_json::to_vec(&fitted).unwrap().len();
            assert!(len <= MAX_SNAPSHOT_BYTES, "{len} bytes");
            assert_eq!(fitted.machines.len(), MAX_MACHINES);
            assert_eq!(fitted.people[0].actor, worst_case(&name).people[0].actor);
            assert_eq!(
                fitted.people.len() as u64 + fitted.more_people,
                MAX_POINTS as u64 + u64::MAX / 2
            );
            let kept: HashSet<_> = fitted.people.iter().map(|p| p.actor).collect();
            assert!(
                fitted
                    .lines
                    .iter()
                    .all(|l| kept.contains(&l.from) && kept.contains(&l.to))
            );
        }
    }

    /// A small world is sent whole.
    #[test]
    fn a_small_snapshot_is_sent_whole() {
        let mut small = worst_case("Kim");
        small.people.truncate(20);
        small.lines.clear();
        small.machines.truncate(5);
        let fitted = fit(small.clone()).unwrap();
        assert_eq!(fitted.people, small.people);
        assert_eq!(fitted.machines, small.machines);
        assert_eq!(fitted.version.len(), 16);
    }

    #[test]
    fn a_day_is_only_the_date() {
        assert_eq!(day(1_763_078_400_000), "2025-11-14");
    }

    #[test]
    fn activity_is_shown_in_bands() {
        assert_eq!(band(0.0), ActivityBand::None);
        assert_eq!(band(3.0), ActivityBand::Some);
        assert_eq!(band(500.0), ActivityBand::ALot);
    }
}
