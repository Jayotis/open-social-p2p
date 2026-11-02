//! The node's machine name (ADR 0015, build step 1): a second identity tied to
//! no person, which answers its own protocol and nothing else, and which
//! nothing the node says under its public ID gives away.
//!
//! These run on the simulated network, which has no Tor. An ignored test does
//! it over real Tor.

use std::{
    io::Write,
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr,
    endpoint::{Connection, ConnectionError, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use osp_node::{
    Config, Home, Node, SimSettings,
    simnet::{self, Link, SimNet},
};
use osp_proto::{
    CLIENT_ALPN, EarlierKeys, FollowTicket, MACHINE_ALPN, SOCIAL_ALPN,
    framing::{read_frame, write_frame},
    machine::{MachineRecord, MachineRequest, MachineResponse},
    rpc::{ClientRequest, ClientResponse, StatusInfo},
    social::{SocialRequest, SocialResponse},
};
use tokio::sync::mpsc;

/// What the daemon logs, as it does unless told otherwise.
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

fn capture_logs() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        tracing_subscriber::fmt()
            .with_env_filter(
                "osp_node=info,osp_tor=info,iroh=warn,iroh::net_report::report=error,iroh_blobs=warn",
            )
            .with_ansi(false)
            .with_writer(|| Capture)
            .init();
    });
}

/// Fails if `text` names `machine`: by its ID, even shortened, or its onion
/// address.
fn assert_doesnt_name(text: &str, machine: &EndpointId, where_: &str) {
    for name in [
        machine.to_string(),
        machine.fmt_short().to_string(),
        osp_tor::onion::service_id(machine),
    ] {
        assert!(!text.contains(&name), "{where_} names the machine: {text}");
    }
}

fn sim(net: &SimNet) -> SimSettings {
    SimSettings {
        network: Some(net.link(Link::default())),
        machine_names: true,
        ..SimSettings::default()
    }
}

async fn start_with(root: &Path, name: &str, net: &SimNet, config: Config) -> Node {
    capture_logs();
    let home = root.join(name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn_sim(&home, sim(net)).await.unwrap()
}

async fn start(root: &Path, name: &str, net: &SimNet) -> Node {
    start_with(root, name, net, Config::local_test(name)).await
}

/// Someone on the network with nothing to do with any node here.
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

fn record(key: &SecretKey) -> MachineRecord {
    MachineRecord::new(key, vec![], osp_proto::now_rfc3339()).unwrap()
}

async fn ask(conn: &Connection, request: &MachineRequest) -> MachineResponse {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, request).await.unwrap();
    send.finish().unwrap();
    read_frame(&mut recv).await.unwrap()
}

async fn ask_social(conn: &Connection, request: &SocialRequest) -> SocialResponse {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, request).await.unwrap();
    send.finish().unwrap();
    read_frame(&mut recv).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn status(node: &Node) -> StatusInfo {
    match call(node, ClientRequest::Status).await {
        ClientResponse::Status(status) => status,
        other => panic!("unexpected {other:?}"),
    }
}

async fn refused(node: &Node, req: ClientRequest) -> String {
    match node.handle(req).await {
        ClientResponse::Error { message } => message,
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_name_answers_its_own_protocol_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(1);
    let mut config = Config::local_test("Alice");
    config.endpoints.enabled = true;
    let alice = start_with(dir.path(), "Alice", &net, config).await;
    let machine = alice.machine_id().unwrap();
    let (me, key) = stranger(&net).await;

    // Nothing but its own protocol on the machine endpoint.
    for alpn in [SOCIAL_ALPN, CLIENT_ALPN, iroh_blobs::ALPN] {
        let why = me.connect(sim_addr(machine), alpn).await;
        assert!(
            why.is_err(),
            "it answered {}",
            String::from_utf8_lossy(alpn)
        );
    }
    // And its own protocol nowhere else.
    let others: Vec<_> = alice
        .endpoint_ids()
        .into_iter()
        .filter(|id| *id != machine)
        .collect();
    assert_eq!(others.len(), 3, "the public, media and direct endpoints");
    for other in others {
        let why = me.connect(sim_addr(other), MACHINE_ALPN).await;
        assert!(why.is_err(), "{} answered as a machine", other.fmt_short());
    }

    let conn = me.connect(sim_addr(machine), MACHINE_ALPN).await.unwrap();
    let reply = ask(
        &conn,
        &MachineRequest::Hello {
            record: record(&key),
        },
    )
    .await;
    let MachineResponse::Hello { record: theirs } = reply else {
        panic!("unexpected {reply:?}");
    };
    let info = theirs.verify().unwrap();
    assert_eq!(info.machine, machine);
    assert!(info.offers.is_empty());
    alice.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_refuses_a_record_that_isnt_the_senders() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(2);
    let alice = start(dir.path(), "Alice", &net).await;
    let machine = alice.machine_id().unwrap();
    let (me, key) = stranger(&net).await;
    let conn = me.connect(sim_addr(machine), MACHINE_ALPN).await.unwrap();

    // Another machine's record, valid as it is.
    let someone = SecretKey::generate();
    let reply = ask(
        &conn,
        &MachineRequest::Hello {
            record: record(&someone),
        },
    )
    .await;
    let MachineResponse::Error { message } = reply else {
        panic!("unexpected {reply:?}");
    };
    assert!(message.contains("another machine"), "{message}");

    // Dated far ahead.
    let ahead = humantime::format_rfc3339_seconds(
        std::time::SystemTime::now() + Duration::from_secs(30 * 24 * 60 * 60),
    );
    let far = MachineRecord::new(&key, vec![], ahead.to_string()).unwrap();
    let reply = ask(&conn, &MachineRequest::Hello { record: far }).await;
    assert!(matches!(reply, MachineResponse::Error { .. }), "{reply:?}");

    // A request from a later version gets an error, and the connection carries on.
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(
        &mut send,
        &serde_json::json!({"op": "something_later", "x": 1}),
    )
    .await
    .unwrap();
    send.finish().unwrap();
    let reply: MachineResponse = read_frame(&mut recv).await.unwrap();
    let MachineResponse::Error { message } = reply else {
        panic!("unexpected {reply:?}");
    };
    assert!(message.contains("doesn't know"), "{message}");
    let reply = ask(
        &conn,
        &MachineRequest::Hello {
            record: record(&key),
        },
    )
    .await;
    assert!(matches!(reply, MachineResponse::Hello { .. }), "{reply:?}");

    // A frame too large is refused without being read, and the machine goes on.
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(&(1u32 << 20).to_be_bytes()).await.unwrap();
    let mut rest = [0u8; 1];
    let answered = tokio::time::timeout(Duration::from_secs(5), recv.read(&mut rest))
        .await
        .expect("it kept waiting for the rest of the frame");
    assert!(!matches!(answered, Ok(Some(_))), "{answered:?}");
    let conn = me.connect(sim_addr(machine), MACHINE_ALPN).await.unwrap();
    let reply = ask(
        &conn,
        &MachineRequest::Hello {
            record: record(&key),
        },
    )
    .await;
    assert!(matches!(reply, MachineResponse::Hello { .. }), "{reply:?}");
    alice.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn two_machines_greet_each_other_and_their_nodes_dont_notice() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(3);
    let alice = start(dir.path(), "Alice", &net).await;
    let bob = start(dir.path(), "Bob", &net).await;
    let (their_machine, my_machine) = (bob.machine_id().unwrap(), alice.machine_id().unwrap());
    assert_ne!(their_machine, my_machine);

    let ClientResponse::MachineName { machine, words } =
        call(&bob, ClientRequest::MachineName).await
    else {
        panic!("no name");
    };
    assert_eq!(machine, their_machine);
    assert_eq!(words.split('-').count(), 3, "{words}");

    let hello = ClientRequest::MachineHello {
        machine: their_machine,
    };
    let ClientResponse::MachineGreeted {
        words: greeted,
        offers,
        ..
    } = call(&alice, hello).await
    else {
        panic!("no greeting");
    };
    assert_eq!(greeted, words);
    assert!(offers.is_empty());

    // Machines aren't peers, follows or followers: nothing the owner sees of
    // the node's dealings holds them, and neither node learned the other's ID.
    for node in [&alice, &bob] {
        let ClientResponse::Peers { peers } = call(node, ClientRequest::Peers).await else {
            panic!("no peers");
        };
        assert!(peers.is_empty(), "{peers:?}");
        let s = status(node).await;
        assert!(s.peers.is_empty(), "{:?}", s.peers);
        assert_eq!((s.follows, s.followers), (0, 0));
    }

    // Following someone changes nothing a machine says.
    let ClientResponse::Ticket { ticket } = call(&bob, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(&alice, ClientRequest::Follow { ticket }).await;
    let (me, key) = stranger(&net).await;
    let conn = me
        .connect(sim_addr(my_machine), MACHINE_ALPN)
        .await
        .unwrap();
    let reply = ask(
        &conn,
        &MachineRequest::Hello {
            record: record(&key),
        },
    )
    .await;
    let MachineResponse::Hello { record: theirs } = reply else {
        panic!("unexpected {reply:?}");
    };
    let info = theirs.verify().unwrap();
    assert_eq!(info.machine, my_machine);
    assert!(info.offers.is_empty());

    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
}

/// A machine that answers a greeting with `record`, and says when the other
/// side closes the connection.
#[derive(Debug, Clone)]
struct FakeMachine {
    record: MachineRecord,
    closed: mpsc::UnboundedSender<()>,
}

impl ProtocolHandler for FakeMachine {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        if let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let _: MachineRequest = read_frame(&mut recv).await.unwrap();
            let reply = MachineResponse::Hello {
                record: self.record.clone(),
            };
            write_frame(&mut send, &reply).await.unwrap();
            send.finish().unwrap();
        }
        conn.closed().await;
        let _ = self.closed.send(());
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_greeting_checks_the_answer_and_closes_the_connection() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(7);
    let alice = start(dir.path(), "Alice", &net).await;

    // An honest machine: the conversation ends when the greeting does.
    let (endpoint, key) = stranger(&net).await;
    let (closed, mut heard) = mpsc::unbounded_channel();
    let honest = FakeMachine {
        record: record(&key),
        closed,
    };
    let _router = Router::builder(endpoint)
        .accept(MACHINE_ALPN, honest)
        .spawn();
    let hello = ClientRequest::MachineHello {
        machine: key.public(),
    };
    call(&alice, hello).await;
    tokio::time::timeout(Duration::from_secs(5), heard.recv())
        .await
        .expect("the connection stayed open");

    // One answering with another machine's record, valid as it is.
    let (endpoint, key) = stranger(&net).await;
    let (closed, _heard) = mpsc::unbounded_channel();
    let false_one = FakeMachine {
        record: record(&SecretKey::generate()),
        closed,
    };
    let _router = Router::builder(endpoint)
        .accept(MACHINE_ALPN, false_one)
        .spawn();
    let hello = ClientRequest::MachineHello {
        machine: key.public(),
    };
    let why = refused(&alice, hello).await;
    assert!(why.contains("another machine"), "{why}");
    alice.shutdown().await.unwrap();
}

/// Anyone can make a name for free, so it gets a short turn among a few.
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_serves_a_few_at_a_time_for_a_short_while() {
    const PLACES: usize = 16;
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(8);
    let alice = start(dir.path(), "Alice", &net).await;
    let machine = alice.machine_id().unwrap();
    let (me, key) = stranger(&net).await;
    let hello = MachineRequest::Hello {
        record: record(&key),
    };

    let mut held = Vec::new();
    for _ in 0..PLACES {
        let conn = me.connect(sim_addr(machine), MACHINE_ALPN).await.unwrap();
        assert!(matches!(
            ask(&conn, &hello).await,
            MachineResponse::Hello { .. }
        ));
        held.push(conn);
    }
    // The next is told to come back later, at once.
    let busy = me.connect(sim_addr(machine), MACHINE_ALPN).await.unwrap();
    let why = tokio::time::timeout(Duration::from_secs(5), busy.closed())
        .await
        .expect("it wasn't told");
    assert!(
        matches!(why, ConnectionError::ApplicationClosed(_)),
        "{why:?}"
    );

    // Those holding places have them for a minute, however quiet they are.
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(90), held[0].closed())
        .await
        .expect("it kept its place");
    assert!(
        started.elapsed() > Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let conn = me.connect(sim_addr(machine), MACHINE_ALPN).await.unwrap();
    assert!(matches!(
        ask(&conn, &hello).await,
        MachineResponse::Hello { .. }
    ));
    alice.shutdown().await.unwrap();
}

/// The gate's leak test: nothing public, and no log, ties a machine name to
/// its node.
#[tokio::test(flavor = "multi_thread")]
async fn nothing_the_node_says_or_logs_ties_its_machine_name_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(4);
    let mut config = Config::local_test("Alice");
    config.endpoints.enabled = true;
    let alice = start_with(dir.path(), "Alice", &net, config).await;
    let machine = alice.machine_id().unwrap();
    let bob = start(dir.path(), "Bob", &net).await;
    let ClientResponse::Posted { id } = call(
        &alice,
        ClientRequest::Post {
            content: "a post".into(),
            attachments: vec![],
            uploads: vec![],
            upload: None,
        },
    )
    .await
    else {
        panic!("no post");
    };
    let hello = ClientRequest::MachineHello {
        machine: bob.machine_id().unwrap(),
    };
    call(&alice, hello).await;
    let hello = ClientRequest::MachineHello { machine };
    call(&bob, hello).await;

    // What it tells its owner and phones in `Status`, and its follow code.
    let s = status(&alice).await;
    assert!(s.machine);
    let json = serde_json::to_string(&s).unwrap();
    assert_doesnt_name(&json, &machine, "the status");
    let ClientResponse::Ticket { ticket } = call(&alice, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    let parsed: FollowTicket = ticket.parse().unwrap();
    assert_ne!(parsed.node.id, machine);
    assert_doesnt_name(&format!("{parsed:?}"), &machine, "the follow code");
    for req in [
        ClientRequest::Peers,
        ClientRequest::Following,
        ClientRequest::Devices,
        ClientRequest::DirectContacts,
        ClientRequest::TrustRequests,
        ClientRequest::Feed { limit: 50 },
    ] {
        let json = serde_json::to_string(&call(&alice, req.clone()).await).unwrap();
        assert_doesnt_name(&json, &machine, &format!("{req:?}"));
    }

    // What it tells anyone who asks under its public ID.
    let (me, _) = stranger(&net).await;
    let conn = me
        .connect(sim_addr(alice.node_id()), SOCIAL_ALPN)
        .await
        .unwrap();
    for req in [
        SocialRequest::GetProfile,
        SocialRequest::GetOutbox {
            since: 0,
            limit: 50,
        },
        SocialRequest::GetObjects { ids: vec![id] },
        SocialRequest::Endpoints { direct: None },
    ] {
        let json = serde_json::to_string(&ask_social(&conn, &req).await).unwrap();
        assert!(!json.contains(r#""result":"error""#), "{req:?}: {json}");
        assert_doesnt_name(&json, &machine, &format!("{req:?}"));
    }

    // It's in nothing it published under its key or put in the home but its key.
    let home = Home::new(dir.path().join("alice"));
    let config_text = std::fs::read_to_string(home.config()).unwrap();
    assert_doesnt_name(&config_text, &machine, "config.toml");

    alice.shutdown().await.unwrap();
    bob.shutdown().await.unwrap();
    let logs = LOGS.lock().unwrap().clone();
    assert!(logs.contains("the machine name is on"), "{logs}");
    assert_doesnt_name(&logs, &machine, "the log");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_name_stays_until_forgotten_and_a_restore_gets_a_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(5);
    let home = dir.path().join("alice");
    let words = Node::init(&home, &Config::local_test("Alice"))
        .unwrap()
        .join(" ");
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    let first = alice.machine_id().unwrap();
    assert!(!alice.endpoint_ids().is_empty());
    assert_ne!(first, alice.node_id());

    // Its key is private, and has nothing to do with the node's other keys.
    let keys = Home::new(&home);
    let mode = std::os::unix::fs::PermissionsExt::mode(
        &std::fs::metadata(keys.machine_key()).unwrap().permissions(),
    );
    assert_eq!(mode & 0o777, 0o600);
    let machine_key = std::fs::read(keys.machine_key()).unwrap();
    for other in [keys.node_key(), keys.signing_key()] {
        assert_ne!(std::fs::read(other).unwrap(), machine_key);
    }

    let file = dir.path().join("alice.ospbackup");
    call(
        &alice,
        ClientRequest::ExportBackup {
            path: file.clone(),
            media: false,
        },
    )
    .await;
    // It can't be forgotten while the node runs.
    assert!(Node::forget_machine_name(&home).is_err());
    alice.shutdown().await.unwrap();

    // The same after a restart.
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    assert_eq!(alice.machine_id(), Some(first));
    alice.shutdown().await.unwrap();

    // A restore starts again as a stranger: the backup doesn't hold it.
    let restored = dir.path().join("alice-new");
    let mut config = Config::local_test("");
    config.name = String::new();
    Node::restore(
        &restored,
        &config,
        &words,
        Some(&file),
        EarlierKeys::Retired,
    )
    .unwrap();
    let again = Node::spawn_sim(&restored, sim(&net)).await.unwrap();
    assert_ne!(again.machine_id(), Some(first));
    again.shutdown().await.unwrap();

    // Forgotten, it comes back different.
    Node::forget_machine_name(&home).unwrap();
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    let second = alice.machine_id().unwrap();
    assert_ne!(second, first);
    alice.shutdown().await.unwrap();
    // Forgetting one already forgotten is fine; a home that isn't a node isn't.
    Node::forget_machine_name(&home).unwrap();
    Node::forget_machine_name(&home).unwrap();
    assert!(Node::forget_machine_name(&dir.path().join("nobody")).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_can_do_without_a_machine_name() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(6);

    // Turned off in its config, and an older config means on.
    assert!(Config::default().machine.enabled);
    let mut config = Config::local_test("Alice");
    config.machine.enabled = false;
    let alice = start_with(dir.path(), "Alice", &net, config).await;
    assert_eq!(alice.machine_id(), None);
    assert!(!Home::new(dir.path().join("alice")).machine_key().exists());
    assert!(!status(&alice).await.machine);
    let why = refused(&alice, ClientRequest::MachineName).await;
    assert!(why.contains("no machine name"), "{why}");
    let hello = ClientRequest::MachineHello {
        machine: SecretKey::generate().public(),
    };
    let why = refused(&alice, hello).await;
    assert!(why.contains("no machine name"), "{why}");
    alice.shutdown().await.unwrap();

    // Without Tor there's nothing to put it on.
    let home = dir.path().join("bob");
    Node::init(&home, &Config::local_test("Bob")).unwrap();
    let bob = Node::spawn(&home).await.unwrap();
    assert_eq!(bob.machine_id(), None);
    assert!(!Home::new(&home).machine_key().exists());
    bob.shutdown().await.unwrap();
}

/// Waits until the node's Tor is ready and has published its onion services.
async fn wait_for_tor(node: &Node) {
    let started = Instant::now();
    loop {
        let s = status(node).await;
        if s.tor.as_ref().is_some_and(|t| t.ready && t.published) {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "Tor stayed {:?}",
            s.tor
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Greets `machine` until it answers.
async fn greet(node: &Node, machine: EndpointId) -> (u64, u64, f64) {
    let started = Instant::now();
    loop {
        match node.handle(ClientRequest::MachineHello { machine }).await {
            ClientResponse::MachineGreeted {
                connect_ms,
                round_trip_ms,
                ..
            } => return (connect_ms, round_trip_ms, started.elapsed().as_secs_f64()),
            other => println!("not yet: {other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(420),
            "never answered"
        );
    }
}

/// The gate over real Tor. Needs Tor:
/// `OSP_TOR=/usr/bin/tor cargo test -p osp-node --test machine -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs tor and the Tor network; takes minutes"]
async fn machines_greet_each_other_over_tor_and_after_it_restarts() {
    capture_logs();
    let binary = std::env::var("OSP_TOR").unwrap_or_else(|_| "tor".into());
    let dir = tempfile::tempdir().unwrap();
    let mut nodes = Vec::new();
    for name in ["A", "B"] {
        let mut config = Config::local_test(name);
        config.tor.enabled = true;
        config.tor.binary = binary.clone();
        config.endpoints.enabled = true;
        let home = dir.path().join(name.to_lowercase());
        Node::init(&home, &config).unwrap();
        nodes.push(Node::spawn(&home).await.unwrap());
    }
    let (a, b) = (&nodes[0], &nodes[1]);
    for node in [a, b] {
        wait_for_tor(node).await;
    }
    let (machine_a, machine_b) = (a.machine_id().unwrap(), b.machine_id().unwrap());
    assert_ne!(machine_a, a.node_id());
    assert_ne!(
        osp_tor::onion::service_id(&machine_a),
        osp_tor::onion::service_id(&a.node_id())
    );

    let (connect, asked, all) = greet(a, machine_b).await;
    println!("a greets b: connected in {connect} ms, asked in {asked} ms, {all:.1} s in all");
    let (connect, asked, all) = greet(b, machine_a).await;
    println!("b greets a: connected in {connect} ms, asked in {asked} ms, {all:.1} s in all");
    for (node, other) in [(a, machine_b), (b, machine_a)] {
        let s = status(node).await;
        assert!(s.peers.iter().all(|p| p.node != other), "{:?}", s.peers);
        assert_doesnt_name(&serde_json::to_string(&s).unwrap(), &other, "the status");
    }

    // The machine endpoint on Tor answers nothing else, and neither does the
    // public one answer as a machine.
    let why = a
        .handle(ClientRequest::MachineHello {
            machine: b.node_id(),
        })
        .await;
    assert!(matches!(why, ClientResponse::Error { .. }), "{why:?}");

    // B's Tor is killed: the node starts it again, and sets both of its onion
    // services up again. A machine name doesn't lose its place.
    let pattern = format!("{}/tor/[t]orrc", dir.path().join("b").display());
    let killed = std::process::Command::new("pkill")
        .args(["-f", &pattern])
        .status()
        .unwrap();
    assert!(killed.success(), "nothing to kill: {pattern}");
    let started = Instant::now();
    loop {
        let s = status(b).await.tor.unwrap();
        if s.restarts > 0 && s.ready && s.published {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(300), "{s:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!(
        "b's Tor back and published: {:.1} s",
        started.elapsed().as_secs_f64()
    );
    assert_eq!(b.machine_id(), Some(machine_b));
    let (connect, asked, all) = greet(a, machine_b).await;
    println!(
        "a greets b after its Tor restarted: connected in {connect} ms, asked in {asked} ms, {all:.1} s in all"
    );

    for node in nodes {
        node.shutdown().await.unwrap();
    }
    let logs = LOGS.lock().unwrap().clone();
    assert!(logs.contains("Tor published an onion service"), "{logs}");
    assert_doesnt_name(&logs, &machine_a, "the log");
    assert_doesnt_name(&logs, &machine_b, "the log");
}
