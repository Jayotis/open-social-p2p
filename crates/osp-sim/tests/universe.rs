//! ADR 0033's gate, on the simulated network: a small world with machine
//! names, a directory, reshares, a vouch and a gathered side.
//!
//! Kim follows Jimmy, who reshared Alice. Bob reshared Kim, which vouches
//! for her, and Kim's node gathered her side. Fred is only in the
//! directory's list. Two other machines have greeted Kim's. Kim's Universe then shows people, lines and machines,
//! and:
//!
//! - no full endpoint ID (any node's public, media, direct or machine
//!   endpoint, as text or hex), onion address, IP address or relay URL
//!   appears anywhere in its bytes;
//! - opening it, again and again, at every level and with every layer, sends
//!   nothing new from any of Kim's endpoints, and makes no viewing endpoint.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::{Duration, Instant},
};

use osp_node::{
    Config, KnownDirectory, MeetingTimes, Node, SimSettings,
    simnet::{Link, SimNet},
};
use osp_proto::{
    EndpointId, ObjectId,
    connected::{self, Hop, How, SideFrom},
    rpc::{ClientRequest, ClientResponse, ProfileItemKind, StatusInfo},
    share::{CARD_GRID, ShareCard, two_figures},
    universe::{Layer, Level, MAX_SNAPSHOT_BYTES, SKY_INNER, Snapshot},
};

fn sim(net: &SimNet) -> SimSettings {
    SimSettings {
        network: Some(net.link(Link::default())),
        machine_names: true,
        // Machines meet in the background, with a first round anywhere in
        // the first ten minutes, which once fell in the window where Kim's
        // sends are counted. Opening the Universe is what's measured, so
        // the rounds come long after these tests end.
        meeting: MeetingTimes {
            round: Duration::from_secs(1_000 * 24 * 60 * 60),
            ..MeetingTimes::default()
        },
        // The same for a node asking to be listed and its first read of its
        // directories, also at times of their own: an ask from Kim's public
        // endpoint to the directory's fell in the window once (2026-11-19).
        // The tests ask and read when they need to.
        ask_listing_wait: Duration::from_secs(1_000 * 24 * 60 * 60),
        first_read_wait: Duration::from_secs(1_000 * 24 * 60 * 60),
        ..SimSettings::default()
    }
}

async fn start(root: &Path, config: Config, settings: SimSettings) -> Node {
    let home = root.join(config.name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn_sim(&home, settings).await.unwrap()
}

async fn start_directory(root: &Path, net: &SimNet) -> Node {
    let mut config = Config::local_test("The directory");
    config.endpoints.enabled = true;
    config.directory.serve = true;
    let settings = SimSettings {
        listed_read_every: Duration::from_millis(200),
        ..sim(net)
    };
    start(root, config, settings).await
}

async fn start_knowing(root: &Path, net: &SimNet, name: &str, directory: &Node) -> Node {
    let mut config = Config::local_test(name);
    config.endpoints.enabled = true;
    config.directory.list_me = true;
    config.directory.known = vec![KnownDirectory {
        machine: directory.machine_id().unwrap(),
        node: directory.node_id(),
    }];
    start(root, config, sim(net)).await
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn follow(follower: &Node, followed: &Node) {
    let ClientResponse::Ticket { ticket } = call(followed, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    call(follower, ClientRequest::Follow { ticket }).await;
}

async fn post(node: &Node, text: &str) -> ObjectId {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, req).await else {
        panic!("not posted")
    };
    id
}

async fn reshare(node: &Node, id: ObjectId) {
    call(node, ClientRequest::Reshare { id, host: false }).await;
}

async fn status(node: &Node) -> StatusInfo {
    let ClientResponse::Status(status) = call(node, ClientRequest::Status).await else {
        panic!("no status")
    };
    status
}

/// Waits up to a minute for `check` to hold.
async fn wait_for<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(60);
    while !check().await {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn universe_request(level: Level, layers: Vec<Layer>, since: Option<String>) -> ClientRequest {
    ClientRequest::Universe {
        level,
        region: None,
        layers,
        since,
    }
}

async fn universe(node: &Node) -> (Snapshot, String) {
    let resp = call(node, universe_request(Level::Local, vec![], None)).await;
    let text = serde_json::to_string(&resp).unwrap();
    let ClientResponse::Universe { snapshot } = resp else {
        panic!("no snapshot")
    };
    (*snapshot, text)
}

async fn share_card(node: &Node, named: Vec<iroh::PublicKey>) -> (ShareCard, String) {
    let resp = call(node, ClientRequest::ShareCard { named }).await;
    let text = serde_json::to_string(&resp).unwrap();
    let ClientResponse::ShareCard { card } = resp else {
        panic!("no card")
    };
    (card, text)
}

/// Whether `text` holds a date, as 2026-11-16 or 20261116.
fn holds_a_date(text: &str) -> bool {
    let b = text.as_bytes();
    let digits = |from: usize, n: usize| b[from..from + n].iter().all(u8::is_ascii_digit);
    (0..b.len().saturating_sub(9)).any(|i| {
        (digits(i, 4) && b[i + 4] == b'-' && digits(i + 5, 2) && b[i + 7] == b'-')
            || (b[i..i + 2] == *b"20" && digits(i, 8))
    })
}

/// "Share my Universe" (ADR 0039's gate), on the world of the test below:
/// with nobody ticked, the card names nobody; ticked, Jimmy is named, and
/// Fred, in the public sky, isn't drawn, so isn't. No machine, ID or time;
/// rounded totals; points on the card's grid. Gives the card as sent, for
/// the address checks.
async fn check_share_card(kim: &Node, snapshot: &Snapshot) -> String {
    let id_of = |name: &str| {
        snapshot
            .people
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .actor
    };
    let (plain, plain_text) = share_card(kim, vec![]).await;
    let (card, card_text) = share_card(kim, vec![id_of("Jimmy"), id_of("Fred")]).await;
    for name in ["Kim", "Jimmy", "Alice", "Bob", "Fred", "directory"] {
        assert!(!plain_text.contains(name), "an unticked card names {name}");
    }
    let named: Vec<&str> = card
        .points
        .iter()
        .filter_map(|p| p.name.as_deref())
        .collect();
    assert_eq!(named, ["Jimmy"], "{card:?}");
    for name in ["Kim", "Alice", "Bob", "Fred", "directory"] {
        assert!(!card_text.contains(name), "the card names {name}");
    }
    for m in &snapshot.machines {
        assert!(!card_text.contains(&m.words), "the card holds {}", m.words);
    }
    for p in &snapshot.people {
        assert!(!card_text.contains(&p.actor.to_string()), "an ID");
        assert!(!card_text.contains(&hex(p.actor.as_bytes())), "an ID");
        // Nobody in the sky is drawn where the sky would put them.
        if p.found_through.is_some() {
            assert!(card.points.iter().all(|c| c.at != p.at), "{:?}", p.at);
        }
    }
    assert!(!holds_a_date(&card_text), "a time: {card_text}");
    assert!(card.points[0].you && card.points.iter().filter(|p| p.you).count() == 1);
    assert_eq!((card.direct, card.paths), (1, 1), "{card:?}");
    assert!(card.people >= 4, "{card:?}");
    for n in [card.people, card.direct, card.paths, card.bytes_served] {
        assert_eq!(n, two_figures(n));
    }
    for p in &card.points {
        for c in p.at {
            assert_eq!(c % CARD_GRID, 0.0, "{:?}", p.at);
        }
    }
    let drawn = card.points.len() as u32;
    assert!(card.lines.iter().all(|[a, b]| a < &drawn && b < &drawn));
    assert_eq!(plain.points.len(), card.points.len());

    card_text
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The requests and tries `node`'s peer record counts, over every peer
/// (ADR 0011). They fade, so they never grow by themselves.
async fn asked(node: &Node) -> (f64, f64) {
    let ClientResponse::Peers { peers } = call(node, ClientRequest::Peers).await else {
        panic!("no peers")
    };
    peers.iter().fold((0.0, 0.0), |(requests, tries), p| {
        (requests + p.counts.requests, tries + p.counts.tries)
    })
}

/// What `asked` says once it has stayed the same for longer than a node
/// puts off a sync it was told of (`MIN_SYNC_GAP`, 5 seconds): the setup's
/// last requests have been made. Waits up to a minute.
async fn settled(node: &Node) -> (f64, f64) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last = asked(node).await;
    loop {
        tokio::time::sleep(Duration::from_secs(6)).await;
        let now = asked(node).await;
        // Fading alone moves them a little, and only down.
        if now.0 <= last.0 + 0.5 && now.1 <= last.1 + 0.5 {
            return now;
        }
        assert!(Instant::now() < deadline, "Kim's node never settled");
        last = now;
    }
}

/// Who sent packets to whom from `own` over `window`, while `during` runs.
async fn sent_from(
    net: &SimNet,
    own: &HashSet<EndpointId>,
    window: Duration,
    during: impl std::future::Future<Output = ()>,
) -> HashSet<(EndpointId, EndpointId)> {
    net.record_senders();
    let ((), ()) = tokio::join!(during, tokio::time::sleep(window));
    net.senders()
        .into_iter()
        .filter(|(from, _)| own.contains(from))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_universe_holds_no_address_and_opening_it_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(90);
    let directory = start_directory(dir.path(), &net).await;
    let kim = start_knowing(dir.path(), &net, "Kim", &directory).await;
    let jimmy = start_knowing(dir.path(), &net, "Jimmy", &directory).await;
    let alice = start_knowing(dir.path(), &net, "Alice", &directory).await;
    let bob = start_knowing(dir.path(), &net, "Bob", &directory).await;
    let fred = start_knowing(dir.path(), &net, "Fred", &directory).await;

    // Jimmy reshares Alice; Kim follows Jimmy.
    follow(&jimmy, &alice).await;
    let alices = post(&alice, "A birch stool").await;
    call(&jimmy, ClientRequest::Sync).await;
    reshare(&jimmy, alices).await;
    follow(&kim, &jimmy).await;
    call(&kim, ClientRequest::Sync).await;

    // Bob reshares Kim, which her node keeps as a vouch; then it gathers
    // her side.
    follow(&bob, &kim).await;
    let kims = post(&kim, "Kim's chair").await;
    call(&bob, ClientRequest::Sync).await;
    reshare(&bob, kims).await;
    let vouched = || async {
        let resp = call(&kim, ClientRequest::Vouches { gather: false }).await;
        matches!(resp, ClientResponse::Vouches { vouchers, .. } if !vouchers.is_empty())
    };
    wait_for("Bob's vouch", vouched).await;
    call(&kim, ClientRequest::Vouches { gather: true }).await;
    let gathered = || async {
        let resp = call(&kim, ClientRequest::Vouches { gather: false }).await;
        matches!(resp, ClientResponse::Vouches { side, gathering: false, .. } if !side.is_empty())
    };
    wait_for("Kim's side", gathered).await;

    // Fred is only in the directory's list.
    post(&fred, "Fred's table").await;
    assert!(fred.ask_to_be_listed().await);
    let found = || async {
        call(&kim, ClientRequest::ReadDirectories).await;
        status(&kim).await.found_people > 0
    };
    wait_for("Kim finding Fred", found).await;

    // Two other machines greet Kim's, so it knows them.
    let machine = kim.machine_id().unwrap();
    for other in [&jimmy, &alice] {
        call(other, ClientRequest::MachineHello { machine }).await;
    }

    let (snapshot, text) = universe(&kim).await;
    let step = |name: &str| {
        let person = snapshot.people.iter().find(|p| p.name == name);
        person.unwrap_or_else(|| panic!("{name} isn't shown")).step
    };
    assert_eq!(step("Kim"), Some(0));
    assert_eq!(step("Jimmy"), Some(1));
    assert_eq!(step("Alice"), Some(2));
    assert_eq!(step("Bob"), Some(3));
    let fred_point = snapshot.people.iter().find(|p| p.name == "Fred");
    assert!(fred_point.is_some_and(|p| p.found_through.is_some()));
    // Only a directory lists Fred, so he's in the public sky, ungrouped,
    // on the ring around the placed world (ADR 0034 §1, §6).
    let fred_point = fred_point.unwrap();
    let [x, y] = fred_point.at;
    assert!(fred_point.group.is_none());
    assert!(
        (x * x + y * y).sqrt() >= SKY_INNER - 0.01,
        "{:?}",
        fred_point.at
    );
    // Searching for him says he's in the sky, at one cell, and opening it
    // shows him (ADR 0035 §3).
    let req = ClientRequest::Search {
        words: "Fred".into(),
        limit: 20,
    };
    let ClientResponse::Found { people, posts } = call(&kim, req).await else {
        panic!("no results")
    };
    let fred_id = fred_point.actor;
    let regions: Vec<&Vec<String>> = people
        .iter()
        .filter(|p| p.actor == fred_id)
        .map(|p| &p.regions)
        .chain(
            posts
                .iter()
                .filter(|p| p.actor == fred_id)
                .map(|p| &p.regions),
        )
        .collect();
    assert_eq!(regions.len(), 2, "Fred and his table: {people:?} {posts:?}");
    for r in regions {
        assert!(
            r.len() == 2 && r[0].is_empty() && r[1].starts_with("sky:32/"),
            "{r:?}"
        );
        let req = ClientRequest::Universe {
            level: Level::Close,
            region: Some(r[1].clone()),
            layers: vec![Layer::People],
            since: None,
        };
        let ClientResponse::Universe { snapshot: cell } = call(&kim, req).await else {
            panic!("no snapshot of {r:?}")
        };
        assert!(cell.people.iter().any(|p| p.actor == fred_id), "{r:?}");
    }
    // The whole sky, tapped, opens as a sheet of him and his post
    // (ADR 0035 §2).
    let req = ClientRequest::RegionItems {
        region: "sky:0/0/0".into(),
        limit: 20,
    };
    let ClientResponse::Found { people, posts } = call(&kim, req).await else {
        panic!("no sheet of the sky")
    };
    assert!(people.iter().any(|p| p.actor == fred_id), "{people:?}");
    assert!(
        posts.iter().any(|p| p.content == "Fred's table"),
        "{posts:?}"
    );
    assert!(
        people.iter().all(|p| p.found_through.is_some()),
        "only the sky's"
    );
    assert!(snapshot.lines.len() >= 2, "{:?}", snapshot.lines);
    assert!(!snapshot.machines.is_empty(), "Kim's machine knows none");
    assert!(text.len() <= MAX_SNAPSHOT_BYTES);

    let card_text = Box::pin(check_share_card(&kim, &snapshot)).await;

    // No full endpoint ID, onion address, IP address or relay URL.
    let world = [&directory, &kim, &jimmy, &alice, &bob, &fred];
    for node in world {
        let mut forbidden = Vec::new();
        for id in node.endpoint_ids() {
            forbidden.push(id.to_string());
            forbidden.push(hex(id.as_bytes()));
            forbidden.push(osp_tor::onion::service_id(&id));
        }
        let s = status(node).await;
        forbidden.extend(s.local_addrs);
        forbidden.extend(s.relay);
        for f in forbidden {
            assert!(!text.contains(&f), "the snapshot holds {f}");
            assert!(!card_text.contains(&f), "the card holds {f}");
        }
    }

    // Opening it sends nothing new. Kim's node keeps connections open with
    // those it synced with, which carry keep-alives now and then whatever
    // she does. So: while she opens it over and over, at every level and
    // with every layer, her node sends only on connections it already had,
    // and only from its public endpoint; asks nobody anything (its peer
    // record counts each request and try); and makes no viewing endpoint.
    let public = kim.node_id();
    let own: HashSet<EndpointId> = kim.endpoint_ids().into_iter().collect();
    let connected: HashSet<EndpointId> = status(&kim).await.peers.iter().map(|p| p.node).collect();
    let asked_before = settled(&kim).await;
    let viewing_before = kim.viewing_endpoints_made();
    let everyone = [&directory, &kim, &jimmy, &alice, &bob, &fred];
    let who_vouched = || everyone.map(|n| n.who_vouched_askers().len());
    let who_vouched_before = who_vouched();
    let sent = sent_from(&net, &own, Duration::from_secs(3), async {
        let mut since = None;
        for level in [
            Level::Universe,
            Level::Galaxy,
            Level::Cluster,
            Level::Local,
            Level::Close,
        ] {
            for layers in [
                vec![],
                vec![Layer::People],
                vec![Layer::Machines],
                vec![Layer::Network],
                vec![Layer::Posts],
                vec![Layer::Video],
                vec![Layer::Shared],
            ] {
                let req = universe_request(level, layers, since.clone());
                if let ClientResponse::Universe { snapshot } = call(&kim, req).await {
                    since = Some(snapshot.version);
                }
            }
        }
        // Moving around (ADR 0034 §6): standing at each person, zooming out
        // from them a step at a time, flying home, and opening the sky.
        let mut regions: Vec<String> = snapshot
            .people
            .iter()
            .map(|p| format!("p:{}", p.actor))
            .collect();
        regions.extend(["sky:0/0/0", "sky:1/0/0", "sky:1/1/1", "sky:3/5/2"].map(String::from));
        let mut asked = HashSet::new();
        while let Some(region) = regions.pop() {
            if !asked.insert(region.clone()) {
                continue;
            }
            let req = ClientRequest::Universe {
                level: Level::Close,
                region: Some(region.clone()),
                layers: vec![],
                since: None,
            };
            let ClientResponse::Universe { snapshot } = call(&kim, req).await else {
                panic!("no snapshot of {region}");
            };
            regions.extend(snapshot.path.iter().cloned());
            regions.extend(snapshot.groups.iter().map(|g| format!("g:{}", g.id)));
        }
        assert!(asked.contains(""), "no way home from {asked:?}");
        // Selecting each person and opening their profile, as a tap on
        // them does, short of asking how she's connected (ADR 0037 §2).
        // Some have no profile here, which is an answer too.
        for person in &snapshot.people {
            kim.handle(ClientRequest::Profile {
                actor: person.actor,
            })
            .await;
            for kind in [
                ProfileItemKind::Posts,
                ProfileItemKind::Media,
                ProfileItemKind::Reshares,
            ] {
                let req = ClientRequest::ProfileItems {
                    actor: person.actor,
                    kind,
                    before: None,
                    limit: 20,
                };
                kim.handle(req).await;
            }
        }
        call(&kim, universe_request(Level::Universe, vec![], None)).await;
    })
    .await;
    let new: Vec<_> = sent
        .iter()
        .filter(|(from, to)| *from != public || !connected.contains(to))
        .collect();
    let mut names: HashMap<EndpointId, String> = HashMap::new();
    for (node, name) in [
        (&directory, "the directory"),
        (&kim, "Kim"),
        (&jimmy, "Jimmy"),
        (&alice, "Alice"),
        (&bob, "Bob"),
        (&fred, "Fred"),
    ] {
        let ids = node.endpoint_ids();
        for (i, id) in ids.iter().enumerate() {
            let what = match (i, Some(*id) == node.machine_id()) {
                (0, _) => "public",
                (_, true) => "machine",
                _ => "other",
            };
            names.insert(*id, format!("{name}'s {what} endpoint"));
        }
    }
    let named: Vec<_> = new
        .iter()
        .map(|(from, to)| {
            let name = |id: &EndpointId| {
                names
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| id.fmt_short().to_string())
            };
            format!("{} to {}", name(from), name(to))
        })
        .collect();
    assert!(new.is_empty(), "opening the Universe sent: {named:?}");
    let asked_after = asked(&kim).await;
    assert!(
        asked_after.0 <= asked_before.0 && asked_after.1 <= asked_before.1,
        "requests and tries: {asked_before:?} before, {asked_after:?} after"
    );
    assert_eq!(kim.viewing_endpoints_made(), viewing_before);
    // Nor did it ask anyone who vouched for them (ADR 0037 §2).
    assert_eq!(who_vouched(), who_vouched_before);
    assert_eq!(kim.throwaways_made(), 0);
    // And the check sees a send it doesn't expect: the same ask to be listed,
    // made in a window of its own, is caught.
    let ask = sent_from(&net, &own, Duration::from_secs(3), async {
        assert!(kim.ask_to_be_listed().await);
    })
    .await;
    let to_directory = (public, directory.node_id());
    assert!(
        ask.contains(&to_directory) && !connected.contains(&to_directory.1),
        "the check missed an ask to be listed: {ask:?}"
    );

    // Kim asks how she's connected to Alice. Her node holds Jimmy's reshare
    // of Alice, and asks Alice's node who vouched for her, once, from a
    // throwaway key: Jimmy, whose reshare her node told Alice's of.
    let jimmy_vouched = || async {
        let resp = call(&alice, ClientRequest::Vouches { gather: false }).await;
        matches!(resp, ClientResponse::Vouches { vouchers, .. } if !vouchers.is_empty())
    };
    wait_for("Jimmy's vouch for Alice", jimmy_vouched).await;
    let alice_before = alice.who_vouched_askers();
    let ask = ClientRequest::HowConnected {
        actor: alice.identity(),
    };
    let ClientResponse::Connected { connection } = call(&kim, ask).await else {
        panic!("no connection")
    };
    assert_eq!(connection.side, SideFrom::Asked { vouches: 1 });
    // Her own node's line and the one through Alice's side are the same
    // path, shown once.
    let path = connected::Path {
        hops: vec![
            Hop {
                to: jimmy.identity(),
                how: How::Follows,
            },
            Hop {
                to: alice.identity(),
                how: How::Reshared,
            },
        ],
    };
    assert_eq!(connection.paths, [path]);
    let names: Vec<_> = connection
        .people
        .iter()
        .map(|p| p.name.as_deref())
        .collect();
    assert_eq!(names, [Some("Alice"), Some("Jimmy")]);
    assert!(connection.people.iter().all(|p| !p.regions.is_empty()));
    let askers = alice.who_vouched_askers();
    assert_eq!(askers.len(), alice_before.len() + 1);
    let asker = askers.last().unwrap();
    assert!(
        !kim.endpoint_ids().contains(asker),
        "Alice's node was asked from one of Kim's endpoints"
    );
    assert_eq!(kim.throwaways_made(), 1);
    // Fred: nobody vouched for him, so no path, and the answer says so.
    let ask = ClientRequest::HowConnected {
        actor: fred.identity(),
    };
    let ClientResponse::Connected { connection } = call(&kim, ask).await else {
        panic!("no connection")
    };
    assert_eq!(connection.side, SideFrom::Asked { vouches: 0 });
    assert!(connection.paths.is_empty());

    for node in [directory, kim, jimmy, alice, bob, fred] {
        node.shutdown().await.unwrap();
    }
}
