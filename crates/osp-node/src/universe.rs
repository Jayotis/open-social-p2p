//! What the Universe may show (ADR 0033): the snapshot built for the owner's
//! paired devices, on the node alone.
//!
//! It reads what the node already knows and copies across only the fields
//! of `osp_proto::universe`, which are ADR 0033's allowlist. Nothing here
//! sends anything to another node: the Universe never asks the network for
//! more, just so there's something to draw.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use iroh::PublicKey;
use osp_proto::{
    MediaKind, Payload, SignedObject,
    rpc::ClientResponse,
    share::{
        CardPoint, MAX_CARD_LINES, MAX_CARD_NAMES, MAX_CARD_POINTS, ShareCard, on_grid, two_figures,
    },
    universe::{
        ALL_LAYERS, ActivityBand, GroupPoint, LINE_DAYS, LIT_DAYS, Layer, Level, Line, LineKind,
        MAX_LINES, MAX_POINTS, MAX_SNAPSHOT_BYTES, MachinePoint, MachineState, NetworkTotals,
        PersonPoint, Region, SkyCell, Snapshot,
    },
};

use crate::{
    admission::{Side, counted_vouches},
    db::now_ms,
    machine,
    meet::{Seen, SeenState},
    node::Core,
    sky::{self, Cell, Light, Sky},
    world::{Input, Opened},
};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// At the top, the sky's share of the points while it has people: the
/// placed world keeps the rest.
const SKY_SHARE: usize = 60;

impl Core {
    /// Answers `Universe`.
    pub(crate) fn universe(
        &self,
        region: Option<&str>,
        layers: &[Layer],
        since: Option<&str>,
    ) -> Result<ClientResponse> {
        let layers = match layers.is_empty() {
            true => &ALL_LAYERS[..],
            false => layers,
        };
        let Some(region) = Region::parse(region) else {
            bail!("no such region: {}", region.unwrap_or_default());
        };
        if layers.iter().filter(|l| l.lights()).count() > 1 {
            bail!("one layer lights up at a time");
        }
        let snapshot = self.snapshot(layers, region, now_ms())?;
        Ok(match since == Some(snapshot.version.as_str()) {
            true => ClientResponse::UniverseUnchanged,
            false => ClientResponse::Universe {
                snapshot: Box::new(snapshot),
            },
        })
    }

    fn snapshot(&self, layers: &[Layer], region: Region, now: i64) -> Result<Snapshot> {
        // A layer that lights up lights the people layer, so it brings it.
        let light = layers.iter().copied().find(|l| l.lights());
        let shown = match layers.contains(&Layer::People) || light.is_some() {
            true => {
                let lit = match light {
                    Some(layer) => self.lit(layer, now)?,
                    None => HashMap::new(),
                };
                self.people_layer(region, &lit, now)?
            }
            false => Shown {
                region,
                ..Shown::default()
            },
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
            level: shown.level,
            region: shown.region.to_string(),
            path: shown.path.iter().map(Region::to_string).collect(),
            groups: shown.groups,
            people: shown.people,
            lines: shown.lines,
            more_people: shown.more_people,
            machines,
            network,
            sky: shown.sky,
        })
    }

    /// Answers `ShareCard` (ADR 0039): the card to share outside OSP, from
    /// its own allowlist. Its drawing is the top of the placed world only:
    /// the public sky is left out, as its people are placed by their keys,
    /// so a point there would say who it is. Only people in `named` the
    /// drawing shows are named, and nothing else on it names anyone.
    pub(crate) fn share_card(&self, named: &[PublicKey]) -> Result<ShareCard> {
        let me = self.signer.identity();
        let now = now_ms();
        let acts = self.lines(now)?;
        let everyone = self.everyone(&acts)?;
        let world = self
            .atlas
            .world(&self.db, world_input(&everyone, &acts, &me, now))?;

        // Kim at the centre, then the groups and people the top opens to.
        let mut points = vec![CardPoint {
            at: [0.0, 0.0],
            people: 1,
            you: true,
            name: None,
        }];
        let mut drawn: HashMap<PublicKey, u32> = HashMap::from([(me, 0)]);
        if let Some(opened) = world.open(Region::Top, &me, MAX_CARD_POINTS - 1) {
            for g in &opened.groups {
                points.push(CardPoint {
                    at: on_grid(world.placed.group_at[*g as usize]),
                    people: two_figures(world.tree.groups[*g as usize].people as u64) as u32,
                    you: false,
                    name: None,
                });
            }
            let names: HashMap<PublicKey, &str> = everyone
                .iter()
                .map(|(p, _)| (p.actor, p.name.as_str()))
                .collect();
            let mut naming = named
                .iter()
                .filter(|a| **a != me)
                .take(MAX_CARD_NAMES)
                .collect::<HashSet<_>>();
            for p in &opened.people {
                let key = *world.key(*p);
                let Some((_, at)) = world.placed(&key) else {
                    continue;
                };
                if key == me || points.len() >= MAX_CARD_POINTS {
                    continue;
                }
                let name = match naming.remove(&key) {
                    true => names
                        .get(&key)
                        .filter(|n| !n.is_empty())
                        .map(|n| n.to_string()),
                    false => None,
                };
                drawn.insert(key, points.len() as u32);
                points.push(CardPoint {
                    at: on_grid(at),
                    people: 1,
                    you: false,
                    name,
                });
            }
        }
        if let Some((_, at)) = world.placed(&me) {
            points[0].at = on_grid(at);
        }
        let mut pairs = HashSet::new();
        let lines = acts
            .iter()
            .filter_map(|act| {
                let (a, b) = (*drawn.get(&act.from)?, *drawn.get(&act.to)?);
                pairs.insert((a.min(b), a.max(b))).then_some([a, b])
            })
            .take(MAX_CARD_LINES)
            .collect();

        let followed: HashSet<PublicKey> = self.db.follows()?.iter().map(|f| f.actor).collect();
        let known: HashSet<PublicKey> = self.db.trusted()?.iter().map(|t| t.actor).collect();
        let direct = followed.union(&known).filter(|a| **a != me).count() as u64;
        let people = everyone.iter().filter(|(p, _)| p.actor != me).count() as u64;
        let (_, bytes_served) = self.db.shared_served()?;
        Ok(ShareCard {
            people: two_figures(people),
            direct: two_figures(direct),
            paths: two_figures(self.side_paths()?),
            bytes_served: two_figures(bytes_served),
            points,
            lines,
        })
    }

    /// Separate chains from the owner's side into the wider network (ADR
    /// 0037): from the farthest people on the side as last gathered, to
    /// the owner, sharing nobody but her. The same count admission makes.
    fn side_paths(&self) -> Result<u64> {
        let signed: Vec<SignedObject> = self.db.side()?.into_iter().map(|v| v.signed).collect();
        let side = Side::new(self.signer.identity(), &counted_vouches(&self.db, &signed)?);
        let Some(far) = side.steps.values().max().copied() else {
            return Ok(0);
        };
        let farthest: HashSet<PublicKey> = side
            .steps
            .iter()
            .filter(|(_, s)| **s == far)
            .map(|(p, _)| *p)
            .collect();
        Ok(side.separate_chains(&farthest).len() as u64)
    }

    /// Everyone the node holds a profile of, the owner first, each with the
    /// date of their newest post here.
    fn everyone(&self, lines: &[Act]) -> Result<Vec<(PersonPoint, Option<i64>)>> {
        let me = self.signer.identity();
        let followed: HashSet<PublicKey> = self.db.follows()?.iter().map(|f| f.actor).collect();
        let known: HashSet<PublicKey> = self.db.trusted()?.iter().map(|t| t.actor).collect();
        let found = self.db.found_directories()?;

        // Step 1: the owner's circle. Step 2: whom the circle, or the owner,
        // reshared. Step 3: the owner's gathered side.
        let circle: HashSet<PublicKey> = followed.union(&known).copied().collect();
        let mut steps: HashMap<PublicKey, u8> = circle.iter().map(|a| (*a, 1)).collect();
        steps.insert(me, 0);
        for line in lines {
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
                    at: [0.0, 0.0],
                    group: None,
                    bright: 0,
                    lit: 0,
                },
                newest,
            ));
        }
        for held in self.db.held_people(&me)? {
            let Ok(Payload::Person(person)) = held.profile.stored_payload() else {
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
                    at: [0.0, 0.0],
                    group: None,
                    bright: 0,
                    lit: 0,
                },
                held.newest_ms,
            ));
        }
        Ok(people)
    }

    /// Where each of `actors` is on the map (ADR 0035 §3), as
    /// `FeedItem::regions` says: the top, then a placed person's groups,
    /// widest first, or a sky person's deepest cell. Reads the same world
    /// a snapshot does, kept while nothing in it changed.
    pub(crate) fn regions_of(
        &self,
        actors: impl IntoIterator<Item = PublicKey>,
    ) -> Result<HashMap<PublicKey, Vec<String>>> {
        let me = self.signer.identity();
        let now = now_ms();
        let acts = self.lines(now)?;
        let everyone = self.everyone(&acts)?;
        let world = self
            .atlas
            .world(&self.db, world_input(&everyone, &acts, &me, now))?;
        let mut out = HashMap::new();
        for actor in actors {
            let mut regions = vec![Region::Top];
            if world.placed(&actor).is_some() {
                let chain = world.chain_of(&actor);
                regions.extend(chain.iter().map(|g| Region::Group(world.id(*g))));
            } else if actor != me {
                let (x, y) = sky::place(SKY_PEOPLE, &actor);
                regions.push(Region::Sky {
                    level: sky::DEEPEST,
                    x,
                    y,
                });
            }
            out.insert(actor, regions.iter().map(Region::to_string).collect());
        }
        Ok(out)
    }

    /// Everyone in a group (`g:`), at any depth, or in a cell of the public
    /// sky (`sky:`), for the sheet a tap opens (ADR 0035 §2).
    pub(crate) fn region_people(&self, region: &str) -> Result<Vec<PublicKey>> {
        let me = self.signer.identity();
        let now = now_ms();
        let acts = self.lines(now)?;
        let everyone = self.everyone(&acts)?;
        let world = self
            .atlas
            .world(&self.db, world_input(&everyone, &acts, &me, now))?;
        Ok(match Region::parse(Some(region)) {
            Some(Region::Group(id)) => match world.members(id) {
                Some(members) => members,
                None => bail!("no such group: {region}"),
            },
            Some(Region::Sky { level, x, y }) => {
                let cell = Cell { level, x, y };
                everyone
                    .iter()
                    .map(|(p, _)| p.actor)
                    .filter(|a| *a != me && world.placed(a).is_none())
                    .filter(|a| cell.contains(sky::place(SKY_PEOPLE, a)))
                    .collect()
            }
            _ => bail!("a sheet opens a group or a sky cell, not {region:?}"),
        })
    }

    /// What a layer that lights up counts for each person (ADR 0035 §1):
    /// their public posts, posts with a video, or reshares from the last
    /// `LIT_DAYS`, as this node holds them. Only people with some.
    fn lit(&self, layer: Layer, now: i64) -> Result<HashMap<PublicKey, u32>> {
        let since = now - LIT_DAYS as i64 * DAY_MS;
        let counts = match layer {
            Layer::Posts => self.db.recent_counts("Note", since)?,
            Layer::Shared => self.db.recent_counts("Announce", since)?,
            Layer::Video => {
                let mut counts: HashMap<PublicKey, u64> = HashMap::new();
                for obj in self.db.recent_media_posts(since)? {
                    let Ok(Payload::Note(note)) = obj.stored_payload() else {
                        continue;
                    };
                    if note.attachment.iter().any(|a| a.kind == MediaKind::Video) {
                        *counts.entry(note.actor).or_default() += 1;
                    }
                }
                counts.into_iter().collect()
            }
            _ => vec![],
        };
        Ok(counts
            .into_iter()
            .filter(|(_, n)| *n > 0)
            .map(|(actor, n)| (actor, n.min(u32::MAX as u64) as u32))
            .collect())
    }

    /// The people layer of `region`: groups, people and sky cells within
    /// the limits, and the public lines between the people shown. `lit` is
    /// a lit layer's count for each person, if one was asked for.
    fn people_layer(
        &self,
        region: Region,
        lit: &HashMap<PublicKey, u32>,
        now: i64,
    ) -> Result<Shown> {
        let lit_of = |actor: &PublicKey| lit.get(actor).copied().unwrap_or(0);
        let me = self.signer.identity();
        let acts = self.lines(now)?;
        let everyone = self.everyone(&acts)?;
        let at: HashMap<PublicKey, usize> = everyone
            .iter()
            .enumerate()
            .map(|(i, (p, _))| (p.actor, i))
            .collect();
        if let Region::Person(key) = region
            && !at.contains_key(&key)
        {
            bail!("this node holds no profile of {key}");
        }

        let world = self
            .atlas
            .world(&self.db, world_input(&everyone, &acts, &me, now))?;

        // The unplaced are the public sky (ADR 0034 §1, §6), the owner aside.
        let unplaced: Vec<usize> = (0..everyone.len())
            .filter(|i| everyone[*i].0.actor != me && world.placed(&everyone[*i].0.actor).is_none())
            .collect();
        let recent = now - LINE_DAYS as i64 * DAY_MS;
        let sky = Sky::new(unplaced.iter().map(|i| {
            let (p, newest) = &everyone[*i];
            let creator = p.posts > 0 && newest.is_some_and(|n| n >= recent);
            (sky::place(SKY_PEOPLE, &p.actor), creator, lit_of(&p.actor))
        }));

        // What's shown: by the world's numbers, then the sky's.
        let mut shown_people: Vec<usize> = Vec::new();
        let mut lights = Vec::new();
        let opened = match region {
            Region::Sky { level, x, y } => {
                let cell = Cell { level, x, y };
                lights = sky.open(cell, MAX_POINTS - 1);
                let level = match lights.iter().any(|l| matches!(l, Light::Cell { .. })) {
                    true => Level::Universe,
                    false => Level::Local,
                };
                let path = std::iter::once(Region::Top)
                    .chain((1..cell.level).map(|l| {
                        let up = (cell.level - l) as u32;
                        Region::Sky {
                            level: l,
                            x: cell.x >> up,
                            y: cell.y >> up,
                        }
                    }))
                    .collect();
                Opened {
                    region,
                    path,
                    level,
                    groups: vec![],
                    people: vec![],
                }
            }
            _ => {
                let sky_share = match region {
                    Region::Top => SKY_SHARE.min(unplaced.len()),
                    _ => 0,
                };
                let budget = MAX_POINTS - 1 - sky_share;
                let opened = world.open(region, &me, budget).unwrap_or(Opened {
                    // Someone unplaced: alone in the sky, with the owner.
                    region,
                    path: vec![Region::Top],
                    level: Level::Close,
                    groups: vec![],
                    people: vec![],
                });
                if region == Region::Top {
                    let used = opened.groups.len() + opened.people.len();
                    lights = sky.open(Cell::ALL, MAX_POINTS - 1 - used);
                }
                opened
            }
        };
        shown_people.extend(at.get(&me));
        if let Region::Person(key) = opened.region {
            shown_people.extend(at.get(&key));
        }
        let mut rest: Vec<usize> = opened
            .people
            .iter()
            .filter_map(|p| at.get(world.key(*p)).copied())
            .collect();
        let mut cells = Vec::new();
        for light in &lights {
            match light {
                Light::Star(i) => rest.push(unplaced[*i as usize]),
                Light::Cell {
                    cell,
                    people,
                    creators,
                    lit,
                } => cells.push(SkyCell {
                    level: cell.level,
                    x: cell.x,
                    y: cell.y,
                    people: *people,
                    creators: *creators,
                    at: round(sky::ring(cell.centre())),
                    lit: *lit,
                }),
            }
        }
        // Nearest first, those with no step last; then the newest posters.
        rest.sort_by(|a, b| {
            let ((a, a_new), (b, b_new)) = (&everyone[*a], &everyone[*b]);
            let step = |p: &PersonPoint| p.step.unwrap_or(u8::MAX);
            step(a)
                .cmp(&step(b))
                .then(b_new.cmp(a_new))
                .then(a.name.cmp(&b.name))
        });
        for i in rest {
            if !shown_people.contains(&i) {
                shown_people.push(i);
            }
        }

        // Each group's posts and reshares, for its brightness, and its lit
        // layer's count.
        let tree = &world.tree;
        let mut sums = vec![(0u64, 0u64, 0u64); tree.groups.len()];
        for (p, _) in &everyone {
            let Some((i, _)) = world.placed(&p.actor) else {
                continue;
            };
            let lit = lit_of(&p.actor) as u64;
            let mut g = tree.group_of[i as usize];
            while let Some(at) = g {
                sums[at as usize].0 += p.posts;
                sums[at as usize].1 += p.reshares;
                sums[at as usize].2 += lit;
                g = tree.groups[at as usize].parent;
            }
        }
        let groups: Vec<GroupPoint> = opened
            .groups
            .iter()
            .map(|g| {
                let group = &tree.groups[*g as usize];
                let (posts, reshares, lit) = sums[*g as usize];
                GroupPoint {
                    id: world.id(*g),
                    parent: group.parent.map(|p| world.id(p)),
                    from: world.placed.from[*g as usize],
                    people: group.people,
                    bright: bright(group.people, posts, reshares),
                    at: round(world.placed.group_at[*g as usize]),
                    lit: lit.min(u32::MAX as u64) as u32,
                }
            })
            .collect();

        let in_lights: u64 = groups.iter().map(|g| g.people as u64).sum::<u64>()
            + cells.iter().map(|c| c.people as u64).sum::<u64>();
        let more_people = (everyone.len() as u64)
            .saturating_sub(shown_people.len() as u64)
            .saturating_sub(in_lights);
        let people: Vec<PersonPoint> = shown_people
            .iter()
            .map(|i| {
                let mut p = everyone[*i].0.clone();
                match world.placed(&p.actor) {
                    Some((w, point)) => {
                        p.at = round(point);
                        p.group = tree.group_of[w as usize].map(|g| world.id(g));
                    }
                    None if p.actor == me => p.at = [0.0, 0.0],
                    None => p.at = round(sky::ring(sky::unit(sky::place(SKY_PEOPLE, &p.actor)))),
                }
                p.bright = bright(1, p.posts, p.reshares);
                p.lit = lit_of(&p.actor);
                p
            })
            .collect();

        // A line names its two people by their place in `people`.
        let place: HashMap<PublicKey, u32> = people
            .iter()
            .enumerate()
            .map(|(i, p)| (p.actor, i as u32))
            .collect();
        let lines: Vec<Line> = acts
            .into_iter()
            .filter_map(|act| {
                Some(Line {
                    kind: act.kind,
                    from: *place.get(&act.from)?,
                    to: *place.get(&act.to)?,
                    count: act.count,
                    last: day(act.at),
                })
            })
            .take(MAX_LINES)
            .collect();
        Ok(Shown {
            region: opened.region,
            path: opened.path,
            level: opened.level,
            groups,
            people,
            lines,
            more_people,
            sky: cells,
        })
    }

    /// Public acts between people within the last 90 days, one line per kind
    /// and pair, newest first.
    fn lines(&self, now: i64) -> Result<Vec<Act>> {
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
        let mut lines: Vec<Act> = lines
            .into_iter()
            .map(|((kind, from, to), (count, at))| Act {
                kind,
                from,
                to,
                count,
                at,
            })
            .collect();
        lines.sort_by(|a, b| {
            b.at.cmp(&a.at)
                .then(a.from.as_bytes().cmp(b.from.as_bytes()))
                .then(a.to.as_bytes().cmp(b.to.as_bytes()))
        });
        Ok(lines)
    }
}

/// The people's sky, apart from the machines' (ADR 0033 §4).
const SKY_PEOPLE: &str = "person";

/// The people layer of a snapshot.
#[derive(Default)]
struct Shown {
    region: Region,
    path: Vec<Region>,
    level: Level,
    groups: Vec<GroupPoint>,
    people: Vec<PersonPoint>,
    lines: Vec<Line>,
    more_people: u64,
    sky: Vec<SkyCell>,
}

/// What the world is grouped from: everyone held, and the public lines
/// between them, each fading over 90 days as vouches do; and the owner's
/// own follows and the people she knows, as lines from her (ADR 0034 §1).
fn world_input(
    everyone: &[(PersonPoint, Option<i64>)],
    acts: &[Act],
    me: &PublicKey,
    now: i64,
) -> Input {
    let mut keys: Vec<PublicKey> = everyone.iter().map(|(p, _)| p.actor).collect();
    keys.sort();
    let at = |k: &PublicKey| keys.binary_search(k).ok().map(|i| i as u32);
    let mut stamp = blake3::Hasher::new();
    for k in &keys {
        stamp.update(k.as_bytes());
    }
    let mut lines = Vec::new();
    for act in acts {
        let (Some(from), Some(to)) = (at(&act.from), at(&act.to)) else {
            continue;
        };
        let age = (now - act.at).max(0) as f64 / (LINE_DAYS as i64 * DAY_MS) as f64;
        lines.push((from, to, act.count as f64 * (1.0 - age).clamp(0.1, 1.0)));
        stamp.update(&[act.kind as u8]);
        stamp.update(act.from.as_bytes());
        stamp.update(act.to.as_bytes());
        stamp.update(&act.count.to_le_bytes());
        stamp.update(&act.at.to_le_bytes());
    }
    let kim = at(me);
    let mut circle: Vec<u32> = everyone
        .iter()
        .filter(|(p, _)| p.followed || p.known)
        .filter_map(|(p, _)| at(&p.actor))
        .collect();
    circle.sort_unstable();
    if let Some(kim) = kim {
        for c in circle {
            lines.push((kim, c, 1.0));
            stamp.update(&c.to_le_bytes());
        }
    }
    Input {
        keys,
        lines,
        kim,
        stamp: *stamp.finalize().as_bytes(),
    }
}

/// A point's brightness, 0 to 100 (ADR 0034 §5): how many people it holds,
/// their posts and their reshares, each as a logarithm with a cap, so one
/// large creator can't outshine every small group.
fn bright(people: u32, posts: u64, reshares: u64) -> u8 {
    let part = |v: f64, cap: f64| (v.ln_1p() / cap.ln_1p()).min(1.0);
    let b = 0.5 * part(people as f64, 10_000.0)
        + 0.3 * part(posts as f64, 1_000.0)
        + 0.2 * part(reshares as f64, 1_000.0);
    (b * 100.0).round() as u8
}

/// A place to a hundredth of a person's width, which keeps it short in JSON.
fn round([x, y]: [f32; 2]) -> [f32; 2] {
    [(x * 100.0).round() / 100.0, (y * 100.0).round() / 100.0]
}

/// Public acts of one kind from one person to another, before they're
/// placed in a snapshot.
struct Act {
    kind: LineKind,
    from: PublicKey,
    to: PublicKey,
    count: u32,
    /// When the newest was published.
    at: i64,
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
            // The people kept keep their places, so their lines still
            // name them.
            let kept = snapshot.people.len() as u32;
            snapshot.lines.retain(|l| l.from < kept && l.to < kept);
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

/// A reshare or found-early credit, as checked when it was stored: its
/// kind, from whom to whom, and its date.
fn public_act(obj: &SignedObject) -> Option<(LineKind, PublicKey, PublicKey, i64)> {
    let (kind, from, to, published) = match obj.stored_payload().ok()? {
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
                at: [-1023.99, -1023.99],
                group: Some(u64::MAX),
                bright: 100,
                lit: u32::MAX,
            })
            .collect();
        let lines = (0..MAX_LINES)
            .map(|i| Line {
                kind: LineKind::CreditedEarly,
                from: (MAX_POINTS - 1 - i % MAX_POINTS) as u32,
                to: ((i * 7 + 1) % MAX_POINTS) as u32,
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
        // The longest path: a sky cell at the deepest level.
        let deep = |level: u8| Region::Sky {
            level,
            x: ((1u64 << level) - 1) as u32,
            y: ((1u64 << level) - 1) as u32,
        };
        Snapshot {
            version: String::new(),
            level: Level::Local,
            region: deep(32).to_string(),
            path: (0..32).map(|l| deep(l).to_string()).collect(),
            groups: vec![],
            people,
            lines,
            more_people: u64::MAX / 2,
            machines,
            network: Some(NetworkTotals {
                posts_hosted: u64::MAX,
                bytes_served: u64::MAX,
                people_found: u64::MAX,
            }),
            sky: vec![],
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
            let kept = fitted.people.len() as u32;
            assert!(fitted.lines.iter().all(|l| l.from < kept && l.to < kept));
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

    /// Points, groups and sky cells share the 300: a snapshot of them all
    /// as groups and cells, at their largest, fits too.
    #[test]
    fn the_largest_lights_fit_their_limit() {
        let mut lights = worst_case("Kim");
        lights.people.truncate(1);
        lights.lines.clear();
        lights.groups = (0..MAX_POINTS / 2)
            .map(|_| GroupPoint {
                id: u64::MAX,
                parent: Some(u64::MAX),
                from: Some(u64::MAX),
                people: u32::MAX,
                bright: 100,
                at: [-1023.99, -1023.99],
                lit: u32::MAX,
            })
            .collect();
        lights.sky = (0..MAX_POINTS / 2)
            .map(|_| SkyCell {
                level: 32,
                x: u32::MAX,
                y: u32::MAX,
                people: u32::MAX,
                creators: u32::MAX,
                at: [-1023.99, -1023.99],
                lit: u32::MAX,
            })
            .collect();
        let fitted = fit(lights.clone()).unwrap();
        assert_eq!(fitted.groups, lights.groups);
        assert_eq!(fitted.sky, lights.sky);
        assert_eq!(fitted.machines.len(), MAX_MACHINES);
    }

    #[test]
    fn brightness_grows_slowly_and_is_capped() {
        assert!(bright(1, 0, 0) < bright(20, 0, 0));
        assert!(bright(20, 0, 0) < bright(20, 10, 0));
        assert_eq!(bright(u32::MAX, u64::MAX, u64::MAX), 100);
        // One very large creator is no brighter than a busy group.
        assert!(bright(1, 1_000_000, 1_000_000) <= bright(50, 200, 100));
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
