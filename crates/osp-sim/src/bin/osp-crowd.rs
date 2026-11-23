//! `osp-crowd`: one node holding a large made-up world, for looking at the
//! Universe on a phone at the size a real node can reach (P11 part 7: a
//! node keeps at most 5,000 people found through directories).
//!
//! The people aren't nodes. This program makes each one an identity, and
//! signs their profiles, posts and reshares itself. A made-up directory
//! lists them, on the simulated network, and the owner's node reads it
//! there under a machine name, as a real node reads a real directory over
//! Tor: every entry goes through the node's own checks, and nothing is
//! written to its database from outside.
//!
//! Then the owner's node stops, and starts again on the real network, with
//! n0's relays and address lookup, so a phone can pair with it. What it
//! found stays. Its pairing code is printed; it runs until stopped. Run it
//! again later on the same `--dir` to start the same node without making
//! the world again.
//!
//! The world's shape is the clustering test's (`cluster.rs`): friend groups
//! of 20, groups gathered into regions of 10, most reshares within a group,
//! some within a region, a few anywhere. The owner reshares a few people in
//! three groups, so the world has a near side.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use iroh::{
    Endpoint, SecretKey,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler, Router},
};
use osp_node::{
    Config, Home, KnownDirectory, Node, RelayChoice, SimSettings,
    simnet::{Link, SimNet},
};
use osp_proto::{
    Announce, MACHINE_ALPN, Note, ObjectId, Payload, Person, SignedObject,
    framing::{read_frame_max, write_frame},
    machine::{
        ListedReshare, ListingEntry, ListingItem, MAX_LISTING_PAGE, MAX_LISTING_PAGE_BYTES,
        MachineRecord, MachineRequest, MachineResponse, OFFERS_DIRECTORY,
    },
    rpc::{ClientRequest, ClientResponse},
    universe::Layer,
};

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The owner's node home: empty to make the world, or one this made
    /// before to start it again.
    #[arg(long)]
    dir: PathBuf,
    /// How many made-up people.
    #[arg(long, default_value_t = 5_000)]
    people: usize,
    /// Seeds the world, so the same seed makes the same people.
    #[arg(long, default_value_t = 1)]
    seed: u64,
}

/// People in a friend group, and groups in a region, as in `cluster.rs`.
const GROUP: usize = 20;
const REGION: usize = 10;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Whose posts the owner reshares: a few in each of three groups.
const OWNER_RESHARES: [usize; 10] = [0, 1, 2, 3, 20, 21, 22, 400, 401, 402];

/// A small, fixed random number source, so a seed gives the same world.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn key(&mut self) -> SecretKey {
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_mut(8) {
            chunk.copy_from_slice(&self.next().to_le_bytes());
        }
        SecretKey::from_bytes(&bytes)
    }
}

const SOUNDS: [&str; 24] = [
    "ka", "lo", "mi", "ra", "su", "te", "no", "vi", "da", "el", "or", "an", "be", "ri", "ta", "mo",
    "ze", "lu", "ha", "ni", "so", "ve", "ji", "pa",
];

const WORDS: [&str; 24] = [
    "garden", "river", "bread", "morning", "train", "music", "rain", "market", "bike", "coffee",
    "mountain", "book", "harbour", "kitchen", "lamp", "field", "bridge", "song", "winter",
    "festival", "street", "boat", "forest", "window",
];

fn word(rng: &mut Rng, sounds: usize) -> String {
    let w: String = (0..sounds)
        .map(|_| SOUNDS[rng.below(SOUNDS.len())])
        .collect();
    let mut chars = w.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => w,
    }
}

fn date(ms: u64) -> String {
    osp_proto::rfc3339_from_ms(ms)
}

/// One made-up person, as a directory would list them.
struct Made {
    key: SecretKey,
    profile: SignedObject,
    /// Newest first.
    posts: Vec<SignedObject>,
}

/// Makes the people, their posts, and who reshares whom, and returns each
/// one's listing, in order.
fn world(people: usize, seed: u64) -> Result<Vec<ListingItem>> {
    let mut rng = Rng(seed);
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_millis() as u64;
    let groups = people.div_ceil(GROUP);

    let mut made = Vec::with_capacity(people);
    for _ in 0..people {
        let key = rng.key();
        let actor = key.public();
        let name = format!("{} {}", word(&mut rng, 2), word(&mut rng, 3));
        // Some stopped posting a while ago, so not everyone is a creator.
        let quiet = rng.below(5) == 0;
        let newest_days = match quiet {
            true => 100 + rng.below(100),
            false => rng.below(30),
        };
        let mut posts = Vec::new();
        for k in 0..1 + rng.below(5) {
            let words: Vec<&str> = (0..4 + rng.below(8))
                .map(|_| WORDS[rng.below(WORDS.len())])
                .collect();
            let note = Note {
                actor,
                content: words.join(" "),
                published: date(now - (newest_days + 3 * k) as u64 * DAY_MS),
                attachment: vec![],
                in_reply_to: None,
            };
            posts.push(SignedObject::sign(&key, &Payload::Note(note))?);
        }
        let person = Person {
            actor,
            name,
            summary: None,
            discoverable: true,
            nodes: vec![rng.key().public()],
            media_endpoints: vec![],
            icon: None,
            attachment: vec![],
            published: date(now - (200 + rng.below(100)) as u64 * DAY_MS),
        };
        let profile = SignedObject::sign(&key, &Payload::Person(person))?;
        made.push(Made {
            key,
            profile,
            posts,
        });
    }

    let mut items = Vec::with_capacity(people);
    for p in 0..people {
        let g = p / GROUP;
        let mut whom = Vec::new();
        for _ in 0..3 {
            whom.push((g * GROUP + rng.below(GROUP)).min(people - 1));
        }
        if rng.below(3) == 0 {
            let region = g / REGION;
            let other = (region * REGION + rng.below(REGION)).min(groups - 1);
            whom.push((other * GROUP + rng.below(GROUP)).min(people - 1));
        }
        if rng.below(50) == 0 {
            whom.push(rng.below(people));
        }
        whom.sort_unstable();
        whom.dedup();
        let mut reshares = Vec::new();
        for q in whom.into_iter().filter(|q| *q != p) {
            let post = made[q].posts[0].clone();
            let announce = Announce {
                actor: made[p].key.public(),
                object: post.id(),
                object_actor: made[q].key.public(),
                published: date(now - rng.below(80) as u64 * DAY_MS),
                hosting: false,
            };
            reshares.push(ListedReshare {
                announce: SignedObject::sign(&made[p].key, &Payload::Announce(announce))?,
                post,
                hosting: false,
            });
        }
        items.push(ListingItem::Person {
            profile: made[p].profile.clone(),
            posts: made[p].posts.clone(),
            reshares,
        });
    }
    Ok(items)
}

/// The made-up directory: it answers a greeting and pages of its lists, as
/// a real one does, and nothing else.
#[derive(Debug, Clone)]
struct Directory {
    key: SecretKey,
    /// Each entry with its encoded length.
    entries: Arc<Vec<(ListingEntry, usize)>>,
}

impl Directory {
    fn answer(&self, request: MachineRequest) -> Result<MachineResponse> {
        Ok(match request {
            MachineRequest::Hello { .. } => MachineResponse::Hello {
                record: MachineRecord::new(
                    &self.key,
                    vec![OFFERS_DIRECTORY.to_string()],
                    osp_proto::now_rfc3339(),
                )?,
            },
            MachineRequest::Listing { since, limit } => {
                let limit = limit.min(MAX_LISTING_PAGE) as usize;
                let mut bytes = 0;
                let entries = self
                    .entries
                    .iter()
                    .filter(|(e, _)| e.seq > since)
                    .take(limit)
                    .take_while(|(_, len)| {
                        bytes += len;
                        bytes <= MAX_LISTING_PAGE_BYTES
                    })
                    .map(|(e, _)| e.clone())
                    .collect();
                MachineResponse::Listing {
                    entries,
                    latest: self.entries.len() as u64,
                }
            }
            _ => MachineResponse::Error {
                message: "a made-up directory answers only greetings and listings".into(),
            },
        })
    }
}

impl ProtocolHandler for Directory {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let served = async {
                let request: MachineRequest = read_frame_max(&mut recv, 64 * 1024).await?;
                write_frame(&mut send, &self.answer(request)?).await?;
                send.finish()?;
                anyhow::Ok(())
            }
            .await;
            if let Err(e) = served {
                eprintln!("a directory request failed: {e:#}");
            }
        }
        Ok(())
    }
}

async fn call(node: &Node, req: ClientRequest) -> Result<ClientResponse> {
    match node.handle(req).await {
        ClientResponse::Error { message } => bail!("{message}"),
        resp => Ok(resp),
    }
}

async fn universe(node: &Node, layers: Vec<Layer>) -> Result<osp_proto::universe::Snapshot> {
    ask_universe(node, None, layers).await
}

/// The snapshot of a region, people layer only.
async fn universe_of(node: &Node, region: Option<String>) -> Result<osp_proto::universe::Snapshot> {
    ask_universe(node, region, vec![Layer::People]).await
}

async fn ask_universe(
    node: &Node,
    region: Option<String>,
    layers: Vec<Layer>,
) -> Result<osp_proto::universe::Snapshot> {
    let req = ClientRequest::Universe {
        level: Default::default(),
        region,
        layers,
        since: None,
    };
    let ClientResponse::Universe { snapshot } = call(node, req).await? else {
        bail!("no snapshot")
    };
    Ok(*snapshot)
}

/// Makes the world and has the owner's node read it, on the simulated
/// network.
async fn make(home: &Path, people: usize, seed: u64) -> Result<()> {
    let started = Instant::now();
    let items = world(people, seed)?;
    let owner_reshares: Vec<ObjectId> = OWNER_RESHARES
        .iter()
        .filter_map(|p| match items.get(*p) {
            Some(ListingItem::Person { posts, .. }) => posts.first().map(SignedObject::id),
            _ => None,
        })
        .collect();
    let entries: Vec<(ListingEntry, usize)> = items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let entry = ListingEntry {
                seq: i as u64 + 1,
                item,
            };
            let len = serde_json::to_vec(&entry).map_or(0, |v| v.len()) + 1;
            (entry, len)
        })
        .collect();
    println!(
        "{people} people made in {:.1} s",
        started.elapsed().as_secs_f64()
    );

    let net = SimNet::new(seed);
    let mut rng = Rng(seed ^ 0x5eed);
    let (machine, node) = (rng.key(), rng.key());
    let link = net.link(Link::default());
    let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .relay_mode(iroh::RelayMode::Disabled)
        .clear_ip_transports()
        .add_custom_transport(link.transport(machine.public()))
        .address_lookup(link.lookup())
        .secret_key(machine.clone())
        .bind()
        .await
        .context("starting the directory's endpoint")?;
    let directory = Directory {
        key: machine.clone(),
        entries: Arc::new(entries),
    };
    let router = Router::builder(endpoint)
        .accept(MACHINE_ALPN, directory)
        .spawn();

    let mut config = Config::local_test("Rae");
    config.summary = Some("The owner of a large made-up world".into());
    config.directory.known = vec![KnownDirectory {
        machine: machine.public(),
        node: node.public(),
    }];
    Node::init(home, &config)?;
    let sim = SimSettings {
        network: Some(net.link(Link::default())),
        machine_names: true,
        first_read_wait: Duration::ZERO,
        read_every: Duration::from_secs(2),
        ..SimSettings::default()
    };
    let owner = Node::spawn_sim(home, sim).await?;

    let reading = Instant::now();
    let mut last = 0;
    loop {
        let found = universe(&owner, vec![Layer::Network])
            .await?
            .network
            .map_or(0, |n| n.people_found);
        if found != last {
            println!("{found} found");
            last = found;
        }
        if found as usize >= people {
            break;
        }
        if reading.elapsed() > Duration::from_secs(30 * 60) {
            bail!("only {found} of {people} found after 30 minutes");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    println!(
        "read the directory in {:.1} s",
        reading.elapsed().as_secs_f64()
    );

    let mut reshared = 0;
    for id in owner_reshares {
        match call(&owner, ClientRequest::Reshare { id, host: false }).await {
            Ok(_) => reshared += 1,
            Err(e) => eprintln!("the owner's reshare failed: {e:#}"),
        }
    }
    println!("the owner reshared {reshared} posts");

    owner.shutdown().await?;
    router.shutdown().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.dir)?;
    let home = Home::new(&cli.dir);
    if !home.is_initialized() {
        if std::fs::read_dir(&cli.dir)?.next().is_some() {
            bail!("{} isn't empty, nor a node's home", cli.dir.display());
        }
        make(&cli.dir, cli.people, cli.seed).await?;

        // From now on an ordinary node, which a phone can reach past a
        // firewall, with no directory to read.
        let mut config = Config::load(&home.config())?;
        config.network.relay = RelayChoice::Default;
        config.network.address_lookup = true;
        config.network.bind = None;
        config.directory.known.clear();
        config.save(&home.config())?;
    }

    let started = Instant::now();
    let owner = Node::spawn(&cli.dir).await?;
    let snapshot = universe(&owner, vec![]).await?;
    println!(
        "Rae's Universe, in {:.1} s: {} people, {} groups, {} lines, {} sky cells, {} more",
        started.elapsed().as_secs_f64(),
        snapshot.people.len(),
        snapshot.groups.len(),
        snapshot.lines.len(),
        snapshot.sky.len(),
        snapshot.more_people,
    );
    // What each later answer costs the node, as a phone moving about asks.
    let started = Instant::now();
    universe(&owner, vec![]).await?;
    let top = started.elapsed();
    let group = snapshot
        .groups
        .first()
        .map(|g| osp_proto::universe::Region::Group(g.id).to_string());
    let started = Instant::now();
    universe_of(&owner, group).await?;
    println!(
        "again: the top in {} ms, a group in {} ms",
        top.as_millis(),
        started.elapsed().as_millis()
    );

    let ClientResponse::Pairing { ticket, .. } = call(&owner, ClientRequest::NewPairing).await?
    else {
        bail!("no pairing code")
    };
    println!("pairing code: {ticket}");
    println!("running until stopped");
    std::future::pending::<()>().await;
    Ok(())
}
