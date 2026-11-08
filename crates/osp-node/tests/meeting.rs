//! Machines meeting machines (ADR 0015's step 3): the stages, `Known`, the
//! limits, and nothing the owner does reaching what a machine says.
//!
//! These run on the simulated network, which has no Tor, with the times among
//! machines made short.

use std::{
    collections::HashSet,
    io::Write,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use osp_node::{
    Acquaintance, Config, IntroductionsConfig, KnownDirectory, MeetingTimes, Node, SimSettings,
    Stage,
    simnet::{self, Link, SimNet},
};
use osp_proto::{
    MACHINE_ALPN,
    framing::{read_frame, write_frame},
    machine::{
        IntroduceRequest, ListingEntry, ListingItem, MAX_KNOWN, MachineRecord, MachineRequest,
        MachineResponse,
    },
    rpc::{ClientRequest, ClientResponse, PersonInfo},
};

static LOGS: Mutex<String> = Mutex::new(String::new());

#[derive(Clone)]
struct Capture;

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        LOGS.lock().unwrap().push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What the daemon logs, at its most, so a test can see none of it names a
/// machine.
fn capture_logs() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        tracing_subscriber::fmt()
            .with_env_filter("osp_node=debug,iroh=warn")
            .with_ansi(false)
            .with_writer(|| Capture)
            .init();
    });
}

fn assert_logs_dont_name(machine: &EndpointId) {
    let logs = LOGS.lock().unwrap().clone();
    for name in [machine.to_string(), machine.fmt_short().to_string()] {
        assert!(!logs.contains(&name), "the log names a machine: {logs}");
    }
}

/// Times among machines, made short: an hour is a second.
fn quick() -> MeetingTimes {
    MeetingTimes {
        hour: Duration::from_secs(1),
        round: Duration::from_millis(100),
        try_again_after: Duration::from_millis(300),
        check_in_every: Duration::from_secs(2),
        met_lately: Duration::from_secs(60),
        still_answering: Duration::from_secs(30),
        forget_silent: Duration::from_secs(120),
        known_after: Duration::from_secs(2),
        half_life: Duration::from_secs(60),
        introducer_after: Duration::from_secs(1),
    }
}

fn sim(net: &SimNet) -> SimSettings {
    SimSettings {
        network: Some(net.link(Link::default())),
        machine_names: true,
        meeting: quick(),
        first_read_wait: Duration::from_millis(200),
        read_every: Duration::from_secs(1),
        ..SimSettings::default()
    }
}

async fn start_sim(root: &Path, name: &str, config: Config, settings: SimSettings) -> Node {
    let home = root.join(name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn_sim(&home, settings).await.unwrap()
}

/// A node knowing these directories.
async fn start_knowing(root: &Path, name: &str, net: &SimNet, directories: &[EndpointId]) -> Node {
    start_sim(root, name, config_knowing(name, directories), sim(net)).await
}

/// The same, with introductions on, as they're off by default.
async fn start_introducing(
    root: &Path,
    name: &str,
    net: &SimNet,
    directories: &[EndpointId],
) -> Node {
    start_sim(root, name, introducing(name, directories), sim(net)).await
}

fn introducing(name: &str, directories: &[EndpointId]) -> Config {
    let mut config = config_knowing(name, directories);
    config.machine.introductions = IntroductionsConfig::ON;
    config
}

fn config_knowing(name: &str, directories: &[EndpointId]) -> Config {
    let mut config = Config::local_test(name);
    config.directory.known = directories
        .iter()
        .map(|machine| KnownDirectory {
            machine: *machine,
            node: SecretKey::generate().public(),
        })
        .collect();
    config
}

async fn start_directory(root: &Path, net: &SimNet) -> Node {
    let mut config = Config::local_test("The directory");
    config.directory.serve = true;
    start_sim(root, "directory", config, sim(net)).await
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

fn machine_of(node: &Node) -> EndpointId {
    node.machine_id().unwrap()
}

fn stage(node: &Node, machine: EndpointId) -> Option<Stage> {
    node.acquaintances()
        .into_iter()
        .find(|a| a.machine == machine)
        .map(|a| a.stage)
}

fn met(node: &Node, machine: EndpointId) -> bool {
    stage(node, machine).is_some_and(|stage| stage >= Stage::Met)
}

/// Waits until `done`, failing after `limit`.
async fn wait_until(limit: Duration, what: &str, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < limit, "never: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn stranger(net: &SimNet) -> (Endpoint, SecretKey) {
    let key = SecretKey::generate();
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .add_custom_transport(net.link(Link::default()).transport(key.public()))
        .secret_key(key.clone())
        .bind()
        .await
        .unwrap();
    (endpoint, key)
}

fn sim_addr(id: EndpointId) -> EndpointAddr {
    EndpointAddr::from_parts(id, [TransportAddr::Custom(simnet::addr(id))])
}

/// Greets a machine as a stranger, leaving the conversation open.
async fn greet_as(
    endpoint: &Endpoint,
    key: &SecretKey,
    machine: EndpointId,
) -> (Connection, MachineRecord) {
    let conn = endpoint
        .connect(sim_addr(machine), MACHINE_ALPN)
        .await
        .unwrap();
    let record = MachineRecord::new(key, vec![], osp_proto::now_rfc3339()).unwrap();
    let MachineResponse::Hello { record } = ask_on(&conn, &MachineRequest::Hello { record }).await
    else {
        panic!("no greeting");
    };
    (conn, record)
}

/// Asks a machine as a stranger: first a greeting, then `request`.
async fn ask_as(
    endpoint: &Endpoint,
    key: &SecretKey,
    machine: EndpointId,
    request: &MachineRequest,
) -> MachineResponse {
    let (conn, _) = greet_as(endpoint, key, machine).await;
    let reply = ask_on(&conn, request).await;
    conn.close(0u32.into(), b"done");
    reply
}

async fn ask_on(conn: &Connection, request: &MachineRequest) -> MachineResponse {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, request).await.unwrap();
    send.finish().unwrap();
    read_frame(&mut recv).await.unwrap()
}

/// A machine's own record, got by greeting it from a name that serves nothing,
/// and asking it nothing more, so a fake's first `Known` stays for the node.
async fn record_of(net: &SimNet, machine: EndpointId) -> MachineRecord {
    let (endpoint, key) = stranger(net).await;
    let (conn, record) = greet_as(&endpoint, &key, machine).await;
    conn.close(0u32.into(), b"done");
    endpoint.close().await;
    record
}

/// The machines a `Known` reply names, asked as a stranger.
async fn known_by(net: &SimNet, machine: EndpointId) -> HashSet<EndpointId> {
    let (endpoint, key) = stranger(net).await;
    let reply = ask_as(&endpoint, &key, machine, &MachineRequest::Known).await;
    endpoint.close().await;
    let MachineResponse::Known { records } = reply else {
        panic!("unexpected {reply:?}");
    };
    assert!(records.len() <= MAX_KNOWN);
    records
        .iter()
        .map(|record| record.verify().unwrap().machine)
        .collect()
}

#[derive(Clone)]
enum Kind {
    /// A directory whose map holds these records.
    Directory(Vec<MachineRecord>),
    /// A machine from before `Known`.
    Old,
    /// One that answers only this machine.
    OldFor(EndpointId),
    /// A machine whose every `Known` reply passes on 20 new names that serve
    /// nothing.
    Hostile,
    /// One whose first `Known` reply passes on 40 such names, more than it may.
    TooMany,
}

/// A machine in the test's hands, which notes what it was asked.
#[derive(Clone)]
struct FakeMachine {
    key: SecretKey,
    kind: Kind,
    asked: Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for FakeMachine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FakeMachine")
    }
}

impl FakeMachine {
    fn answer(&self, request: serde_json::Value) -> MachineResponse {
        self.asked.lock().unwrap().push(request.to_string());
        let offers = match self.kind {
            Kind::Directory(_) => vec!["directory".to_string()],
            _ => vec![],
        };
        let now = osp_proto::now_rfc3339();
        match (serde_json::from_value(request).unwrap(), &self.kind) {
            (MachineRequest::Hello { .. }, _) => MachineResponse::Hello {
                record: MachineRecord::new(&self.key, offers, now).unwrap(),
            },
            (MachineRequest::Listing { since, .. }, Kind::Directory(map)) => {
                let entries: Vec<ListingEntry> = (1..)
                    .zip(map)
                    .filter(|(seq, _)| *seq > since)
                    .map(|(seq, record)| ListingEntry {
                        seq,
                        item: ListingItem::Machine {
                            record: record.clone(),
                        },
                    })
                    .collect();
                MachineResponse::Listing {
                    entries,
                    latest: map.len() as u64,
                }
            }
            (MachineRequest::Known, Kind::Hostile) => MachineResponse::Known {
                records: (0..MAX_KNOWN)
                    .map(|_| {
                        let fake = SecretKey::generate();
                        MachineRecord::new(&fake, vec![], now.clone()).unwrap()
                    })
                    .collect(),
            },
            (MachineRequest::Known, Kind::TooMany) => {
                let first = self.asked("known") == 1;
                MachineResponse::Known {
                    records: (0..if first { 2 * MAX_KNOWN } else { 0 })
                        .map(|_| {
                            let fake = SecretKey::generate();
                            MachineRecord::new(&fake, vec![], now.clone()).unwrap()
                        })
                        .collect(),
                }
            }
            (MachineRequest::Known, Kind::Directory(_)) => {
                MachineResponse::Known { records: vec![] }
            }
            _ => MachineResponse::Error {
                message: "this machine doesn't know that request".into(),
            },
        }
    }

    fn asked(&self, what: &str) -> usize {
        let asked = self.asked.lock().unwrap();
        asked
            .iter()
            .filter(|request| request.contains(what))
            .count()
    }
}

impl ProtocolHandler for FakeMachine {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        if let Kind::OldFor(only) = &self.kind
            && conn.remote_id() != *only
        {
            conn.close(1u32.into(), b"not you");
            return Ok(());
        }
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let request: serde_json::Value = read_frame(&mut recv).await.unwrap();
            let reply = self.answer(request);
            write_frame(&mut send, &reply).await.unwrap();
            send.finish().unwrap();
        }
        Ok(())
    }
}

/// Starts a fake machine. The router has to stay alive.
async fn fake(net: &SimNet, kind: Kind) -> (FakeMachine, Router) {
    let (endpoint, key) = stranger(net).await;
    let machine = FakeMachine {
        key,
        kind,
        asked: Arc::default(),
    };
    let router = Router::builder(endpoint)
        .accept(MACHINE_ALPN, machine.clone())
        .spawn();
    (machine, router)
}

/// Kim and Alice, each having met the other long enough ago for Alice's
/// introductions to count with Kim.
async fn kim_and_alice(root: &Path, net: &SimNet) -> (Node, Node, Vec<Router>) {
    let alice = start_introducing(root, "Alice", net, &[]).await;
    let a = machine_of(&alice);
    // Alice's record, from a machine that stays to answer when she greets it
    // back, as one gone would hold up her rounds.
    let (helper, helpers_router) = fake(net, Kind::Old).await;
    let (conn, record) = greet_as(helpers_router.endpoint(), &helper.key, a).await;
    conn.close(0u32.into(), b"done");
    // Kim hears of Alice from her directory and meets her, so Alice doesn't
    // count among those who greet Kim and may introduce; Alice, greeted,
    // meets Kim back.
    let (kims, router) = fake(net, Kind::Directory(vec![record])).await;
    let kim = start_introducing(root, "Kim", net, &[kims.key.public()]).await;
    let k = machine_of(&kim);
    wait_until(Duration::from_secs(30), "Kim and Alice met", || {
        met(&alice, k) && met(&kim, a)
    })
    .await;
    tokio::time::sleep(quick().introducer_after * 2).await;
    (kim, alice, vec![helpers_router, router])
}

fn request_from(asker: &SecretKey, via: EndpointId, target: EndpointId) -> IntroduceRequest {
    let expires = humantime::format_rfc3339_seconds(SystemTime::now() + Duration::from_secs(600));
    let record = MachineRecord::new(asker, vec![], osp_proto::now_rfc3339()).unwrap();
    let mut id = [0; 16];
    id[..8].copy_from_slice(&rand_u64().to_le_bytes());
    IntroduceRequest::new(asker, id, via, target, expires.to_string(), record).unwrap()
}

fn rand_u64() -> u64 {
    u64::from_le_bytes(SecretKey::generate().to_bytes()[..8].try_into().unwrap())
}

/// ADR 0016's introductions: Jimmy, which met Alice, asks her to introduce
/// it to Kim, whose record she passed on. She passes it on, and Kim, who has
/// known Alice a while, dials Jimmy in the turn for those who greet, counting
/// it there. Jimmy never greeted Kim, and the log names no machine.
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_introduced_is_dialled_by_the_one_it_was_introduced_to() {
    capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(5);
    let (kim, alice, _router) = kim_and_alice(dir.path(), &net).await;
    let (k, a) = (machine_of(&kim), machine_of(&alice));

    // Jimmy, in the test's hands, answers Kim's greeting and nothing more.
    // Alice can't meet it, so can't pass it on to Kim before the introduction
    // arrives.
    let (jimmy, jimmys_router) = fake(&net, Kind::OldFor(k)).await;
    let j = jimmy.key.public();
    let (conn, _) = greet_as(jimmys_router.endpoint(), &jimmy.key, a).await;
    let MachineResponse::Known { records } = ask_on(&conn, &MachineRequest::Known).await else {
        panic!("no Known");
    };
    assert!(records.iter().any(|r| r.verify().unwrap().machine == k));
    let introduce = |request| MachineRequest::Introduce { request };
    // Not Jimmy's own request, or naming another machine to pass it on.
    let other = SecretKey::generate();
    for request in [request_from(&other, a, k), request_from(&jimmy.key, k, k)] {
        let reply = ask_on(&conn, &introduce(request)).await;
        assert!(matches!(
            reply,
            MachineResponse::Introduce { passed_on: false }
        ));
    }
    let reply = ask_on(&conn, &introduce(request_from(&jimmy.key, a, k))).await;
    assert!(matches!(
        reply,
        MachineResponse::Introduce { passed_on: true }
    ));
    conn.close(0u32.into(), b"done");

    wait_until(Duration::from_secs(30), "Kim met Jimmy", || met(&kim, j)).await;
    let jimmy_to_kim = kim
        .acquaintances()
        .into_iter()
        .find(|a| a.machine == j)
        .unwrap();
    assert!(jimmy_to_kim.introduced && jimmy_to_kim.greeters_share);
    assert_eq!(jimmy_to_kim.source, Some(a), "Alice introduced it");
    assert_eq!(kim.introductions().accepted, 1);
    assert!(alice.introductions().passed_on >= 1);
    assert!(
        jimmy.asked("known") > 0,
        "Kim dialled Jimmy as she would any machine"
    );
    for machine in [j, k, a] {
        assert_logs_dont_name(&machine);
    }
    for node in [kim, alice] {
        node.shutdown().await.unwrap();
    }
}

/// A node asks on its own while few have met it, and each part can be
/// turned off: asked by a node that won't pass it on, nothing is.
#[tokio::test(flavor = "multi_thread")]
async fn nodes_ask_to_be_introduced_on_their_own_and_each_part_can_be_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(6);
    let (kim, alice, _router) = kim_and_alice(dir.path(), &net).await;
    let a = machine_of(&alice);
    let (jimmys, _jimmys_router) =
        fake(&net, Kind::Directory(vec![record_of(&net, a).await])).await;
    let jimmy = start_introducing(dir.path(), "Jimmy", &net, &[jimmys.key.public()]).await;
    wait_until(
        Duration::from_secs(30),
        "Jimmy asked and Alice passed it on",
        || {
            let asked = jimmy.introductions();
            asked.asked > 0 && asked.asked_passed_on > 0 && alice.introductions().passed_on > 0
        },
    )
    .await;

    // Lee won't ask.
    let mut config = introducing("Lee", &[jimmys.key.public()]);
    config.machine.introductions.ask = false;
    let lee = start_sim(dir.path(), "Lee", config, sim(&net)).await;
    wait_until(Duration::from_secs(30), "Lee met Alice", || met(&lee, a)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(lee.introductions().asked, 0);

    // A machine that won't pass them on says so to every request.
    let mut config = introducing("Max", &[]);
    config.machine.introductions.pass_on = false;
    let max = start_sim(dir.path(), "Max", config, sim(&net)).await;
    let (endpoint, key) = stranger(&net).await;
    let m = machine_of(&max);
    let request = request_from(&key, m, a);
    let reply = ask_as(&endpoint, &key, m, &MachineRequest::Introduce { request }).await;
    assert!(matches!(
        reply,
        MachineResponse::Introduce { passed_on: false }
    ));
    for node in [kim, alice, jimmy, lee, max] {
        node.shutdown().await.unwrap();
    }
}

/// The gate's first half: nodes that know only a directory meet one another,
/// with nobody doing anything.
#[tokio::test(flavor = "multi_thread")]
async fn left_alone_machines_meet_through_a_directory_with_no_human_act() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(1);
    let directory = start_directory(dir.path(), &net).await;
    let d = machine_of(&directory);
    let mut nodes = Vec::new();
    for name in ["Alice", "Bob", "Carol"] {
        nodes.push(start_knowing(dir.path(), name, &net, &[d]).await);
    }
    let machines: Vec<EndpointId> = nodes.iter().map(machine_of).collect();
    wait_until(Duration::from_secs(60), "everyone met everyone", || {
        nodes.iter().all(|node| {
            machines
                .iter()
                .filter(|m| **m != machine_of(node))
                .all(|m| met(node, *m))
        })
    })
    .await;
    // A directory set in config.toml is kept apart, and never in the table.
    for node in &nodes {
        assert_eq!(stage(node, d), None);
    }
    // And with time, they come to know one another.
    wait_until(Duration::from_secs(30), "known", || {
        nodes
            .iter()
            .all(|node| node.acquaintances().iter().all(|a| a.stage == Stage::Known))
    })
    .await;

    for node in nodes.into_iter().chain([directory]) {
        node.shutdown().await.unwrap();
    }
}

/// The gate's second half: a node meets a machine no directory gave it, with
/// the directories gone, through a machine it met.
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_meets_one_it_heard_of_only_from_another_with_the_directories_off() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(2);
    let carol = start_knowing(dir.path(), "Carol", &net, &[]).await;
    let c = machine_of(&carol);

    // Bob's directory lists only Carol, and Alice's only Bob.
    let (bobs, bobs_router) = fake(&net, Kind::Directory(vec![record_of(&net, c).await])).await;
    let bob = start_knowing(dir.path(), "Bob", &net, &[bobs.key.public()]).await;
    let b = machine_of(&bob);
    wait_until(Duration::from_secs(30), "Bob met Carol", || met(&bob, c)).await;
    bobs_router.shutdown().await.unwrap();

    let (alices, alices_router) = fake(&net, Kind::Directory(vec![record_of(&net, b).await])).await;
    let alice = start_knowing(dir.path(), "Alice", &net, &[alices.key.public()]).await;
    wait_until(Duration::from_secs(30), "Alice read her directory", || {
        alices.asked("listing") > 0
    })
    .await;
    alices_router.shutdown().await.unwrap();

    wait_until(Duration::from_secs(30), "Alice met Carol", || {
        met(&alice, c)
    })
    .await;
    let carol_to_alice = alice
        .acquaintances()
        .into_iter()
        .find(|a| a.machine == c)
        .unwrap();
    assert_eq!(
        carol_to_alice.source,
        Some(b),
        "she heard of Carol from Bob"
    );

    for node in [alice, bob, carol] {
        node.shutdown().await.unwrap();
    }
}

/// A node that read its directory to the end before it took machines from it,
/// as one did before step 3, reads it again from the start and meets those on
/// the map, instead of never hearing of them.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_reads_again_what_an_earlier_version_passed_over() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(7);
    let carol = start_knowing(dir.path(), "Carol", &net, &[]).await;
    let c = machine_of(&carol);
    let (directory, _router) = fake(&net, Kind::Directory(vec![record_of(&net, c).await])).await;
    let d = directory.key.public();

    let home = dir.path().join("bob");
    Node::init(&home, &config_knowing("Bob", &[d])).unwrap();
    let db = rusqlite::Connection::open(home.join("osp.db")).unwrap();
    db.execute(
        "INSERT INTO directories_read (directory, cursor, reader) VALUES (?1, 1, 0)",
        [d.to_string()],
    )
    .unwrap();
    drop(db);
    let bob = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    wait_until(Duration::from_secs(30), "Bob met Carol", || met(&bob, c)).await;
    // Having read it as this version, he goes on from there next time.
    wait_until(
        Duration::from_secs(30),
        "Bob read on from where he got",
        || {
            let asked = directory.asked.lock().unwrap();
            asked
                .iter()
                .any(|request| request.contains("listing") && request.contains(r#""since":1"#))
        },
    )
    .await;

    for node in [bob, carol] {
        node.shutdown().await.unwrap();
    }
}

async fn found(node: &Node, words: &str) -> Vec<PersonInfo> {
    let req = ClientRequest::Search {
        words: words.into(),
        limit: 50,
    };
    match call(node, req).await {
        ClientResponse::Found { people, .. } => people,
        other => panic!("unexpected {other:?}"),
    }
}

/// The leak test's part for `Known`: a reply names only machines met, and
/// nothing its owner does changes it. Nothing logged names a machine.
#[tokio::test(flavor = "multi_thread")]
async fn known_names_only_machines_met_and_follows_and_searches_change_nothing() {
    capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(3);
    let directory = start_directory(dir.path(), &net).await;
    let d = machine_of(&directory);
    let mut alice = config_knowing("Alice", &[d]);
    alice.directory.known[0].node = directory.node_id();
    alice.directory.list_me = true;
    let alice = start_sim(dir.path(), "Alice", alice, sim(&net)).await;
    let bob = start_knowing(dir.path(), "Bob", &net, &[d]).await;
    let mut carol = config_knowing("Carol", &[d]);
    carol.directory.known[0].node = directory.node_id();
    let carol = start_sim(dir.path(), "Carol", carol, sim(&net)).await;
    let (a, b, c) = (machine_of(&alice), machine_of(&bob), machine_of(&carol));
    wait_until(Duration::from_secs(60), "Carol met the others", || {
        met(&carol, a) && met(&carol, b)
    })
    .await;

    let before = known_by(&net, c).await;
    assert_eq!(before, HashSet::from([a, b]));

    // Carol finds Alice, follows her, and searches.
    assert!(alice.ask_to_be_listed().await);
    let mut people = Vec::new();
    let started = Instant::now();
    while people.is_empty() {
        assert!(started.elapsed() < Duration::from_secs(30), "never found");
        call(&carol, ClientRequest::ReadDirectories).await;
        people = found(&carol, "alice").await;
    }
    let actor = people[0].actor;
    call(&carol, ClientRequest::FollowPerson { actor }).await;
    call(&carol, ClientRequest::Sync).await;
    found(&carol, "bob").await;
    found(&carol, "photography").await;
    assert_eq!(known_by(&net, c).await, before);

    // What a machine says names no person, and nothing names a machine.
    let (endpoint, key) = stranger(&net).await;
    let reply = ask_as(&endpoint, &key, c, &MachineRequest::Known).await;
    let text = serde_json::to_string(&reply).unwrap();
    for node in [&alice, &bob, &carol] {
        assert!(!text.contains(&node.identity().to_string()));
        assert!(!text.contains(&node.node_id().to_string()));
    }
    for machine in [a, b, c, d] {
        assert_logs_dont_name(&machine);
    }

    for node in [alice, bob, carol, directory] {
        node.shutdown().await.unwrap();
    }
}

/// Greetings from new names, however many, fill only the share of those who
/// greet unasked, 32 places, and the 8 they take in turn.
#[tokio::test(flavor = "multi_thread")]
async fn a_crowd_of_names_greeting_fills_only_one_share_and_the_turns() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(4);
    // An hour as long as a real one, so few are dialled back, and none forgotten.
    let settings = SimSettings {
        meeting: MeetingTimes {
            hour: Duration::from_secs(3600),
            ..quick()
        },
        ..sim(&net)
    };
    let alice = start_sim(dir.path(), "Alice", config_knowing("Alice", &[]), settings).await;
    let a = machine_of(&alice);
    let greeted = |node: &Node| {
        node.acquaintances()
            .iter()
            .filter(|a| a.source.is_none())
            .count()
    };
    for _ in 0..50 {
        record_of(&net, a).await;
        assert!(greeted(&alice) <= 40);
    }
    assert_eq!(greeted(&alice), 40);
    alice.shutdown().await.unwrap();
}

/// One hostile machine, passing on fakes, fills its share and no more, isn't
/// known, and the honest machine and the old one are still met.
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_passing_fakes_fills_only_its_share_and_others_still_get_met() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(5);
    let (hostile, _hostile_router) = fake(&net, Kind::Hostile).await;
    let (old, _old_router) = fake(&net, Kind::Old).await;
    let (too_many, _too_many_router) = fake(&net, Kind::TooMany).await;
    let bob = start_knowing(dir.path(), "Bob", &net, &[]).await;
    let (h, o, b) = (hostile.key.public(), old.key.public(), machine_of(&bob));
    let t = too_many.key.public();
    let map = vec![
        record_of(&net, h).await,
        record_of(&net, o).await,
        record_of(&net, b).await,
        record_of(&net, t).await,
    ];
    let (directory, _router) = fake(&net, Kind::Directory(map)).await;
    let alice = start_knowing(dir.path(), "Alice", &net, &[directory.key.public()]).await;

    let from_hostile = |node: &Node| -> Vec<Acquaintance> {
        node.acquaintances()
            .into_iter()
            .filter(|a| a.source == Some(h))
            .collect()
    };
    let from_too_many = |node: &Node| {
        node.acquaintances()
            .iter()
            .filter(|a| a.source == Some(t))
            .count()
    };
    let started = Instant::now();
    let mut most_from_too_many = 0;
    while started.elapsed() < Duration::from_secs(8) {
        assert!(from_hostile(&alice).len() <= 32);
        most_from_too_many = most_from_too_many.max(from_too_many(&alice));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(hostile.asked("known") > 1, "it was checked in with");
    assert!(!from_hostile(&alice).is_empty());
    // A machine from before `Known` has still answered.
    assert!(met(&alice, o) && met(&alice, b));
    assert_eq!(stage(&alice, b), Some(Stage::Known));
    assert_eq!(
        stage(&alice, h),
        Some(Stage::Met),
        "its records didn't check out"
    );
    // A reply with more than it may pass on is cut to 20, and counts against it.
    assert_eq!(most_from_too_many, MAX_KNOWN);
    assert_eq!(stage(&alice, t), Some(Stage::Met));

    for node in [alice, bob] {
        node.shutdown().await.unwrap();
    }
}

/// What a machine knows stays after a restart, and goes with a new machine
/// name.
#[tokio::test(flavor = "multi_thread")]
async fn what_a_machine_knows_stays_after_a_restart_and_goes_with_a_new_name() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(6);
    let bob = start_knowing(dir.path(), "Bob", &net, &[]).await;
    let b = machine_of(&bob);
    let (directory, _router) = fake(&net, Kind::Directory(vec![record_of(&net, b).await])).await;
    let config = config_knowing("Alice", &[directory.key.public()]);
    let home = dir.path().join("alice");
    let alice = start_sim(dir.path(), "Alice", config, sim(&net)).await;
    wait_until(Duration::from_secs(30), "Alice met Bob", || met(&alice, b)).await;
    let before = alice.acquaintances();
    alice.shutdown().await.unwrap();

    // Not reading the directory again for a while, so only what's stored shows.
    let settings = SimSettings {
        first_read_wait: Duration::from_secs(3600),
        ..sim(&net)
    };
    let alice = Node::spawn_sim(&home, settings.clone()).await.unwrap();
    let as_set = |known: Vec<Acquaintance>| -> HashSet<(EndpointId, Option<EndpointId>)> {
        known.into_iter().map(|a| (a.machine, a.source)).collect()
    };
    assert_eq!(as_set(alice.acquaintances()), as_set(before));
    assert!(met(&alice, b));
    alice.shutdown().await.unwrap();

    Node::forget_machine_name(&home).unwrap();
    let alice = Node::spawn_sim(&home, settings).await.unwrap();
    assert!(alice.acquaintances().is_empty());
    for node in [alice, bob] {
        node.shutdown().await.unwrap();
    }
}
