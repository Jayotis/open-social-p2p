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
    rpc::{ClientRequest, ClientResponse, StatusInfo},
    universe::{Layer, Level, MAX_SNAPSHOT_BYTES, Snapshot},
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
    assert!(snapshot.lines.len() >= 2, "{:?}", snapshot.lines);
    assert!(!snapshot.machines.is_empty(), "Kim's machine knows none");
    assert!(text.len() <= MAX_SNAPSHOT_BYTES);

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
            ] {
                let req = universe_request(level, layers, since.clone());
                if let ClientResponse::Universe { snapshot } = call(&kim, req).await {
                    since = Some(snapshot.version);
                }
            }
        }
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

    for node in [directory, kim, jimmy, alice, bob, fred] {
        node.shutdown().await.unwrap();
    }
}
