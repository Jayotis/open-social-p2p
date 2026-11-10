//! Directories (ADR 0015, build step 2): who may be listed, what a directory
//! holds, and a node finding a stranger through one with no word leaving it.
//!
//! These run on the simulated network, which has no Tor.

use std::{
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
    BlobClass, Config, Home, KnownDirectory, Node, SimSettings,
    simnet::{self, Link, SimNet},
};
use osp_proto::{
    Announce, BlobHash, CertifiedKey, EarlierKeys, MACHINE_ALPN, Note, ObjectId, Payload, Person,
    SOCIAL_ALPN, SignedObject,
    framing::{read_frame, write_frame},
    machine::{ListingEntry, ListingItem, MachineRecord, MachineRequest, MachineResponse},
    rpc::{ClientRequest, ClientResponse, DirectoryStatus, FeedItem, PersonInfo, StatusInfo},
    social::{OutboxEntry, SocialRequest, SocialResponse},
};

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
            .with_env_filter("osp_node=info,osp_tor=info,iroh=warn")
            .with_ansi(false)
            .with_writer(|| Capture)
            .init();
    });
}

/// Fails if the logs name `machine`: by its ID, even shortened, or its onion
/// address.
fn assert_logs_dont_name(machine: &EndpointId) {
    let logs = LOGS.lock().unwrap().clone();
    for name in [
        machine.to_string(),
        machine.fmt_short().to_string(),
        osp_tor::onion::service_id(machine),
    ] {
        assert!(!logs.contains(&name), "the log names the machine: {logs}");
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
    start_sim(root, name, config, sim(net)).await
}

async fn start_sim(root: &Path, name: &str, config: Config, settings: SimSettings) -> Node {
    let home = root.join(name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn_sim(&home, settings).await.unwrap()
}

/// A directory, reading those it lists as often as `read_every`.
async fn start_directory(root: &Path, net: &SimNet, read_every: Duration) -> Node {
    let mut config = Config::local_test("The directory");
    config.directory.serve = true;
    let settings = SimSettings {
        listed_read_every: read_every,
        ..sim(net)
    };
    start_sim(root, "directory", config, settings).await
}

/// Someone who knows `directory`, and may be listed if `list_me`.
fn config_knowing(name: &str, directory: &Node, list_me: bool) -> Config {
    let mut config = Config::local_test(name);
    config.directory.list_me = list_me;
    config.directory.known = vec![KnownDirectory {
        machine: directory.machine_id().unwrap(),
        node: directory.node_id(),
    }];
    config
}

/// Someone on the network with nothing to do with any node here.
async fn stranger(net: &SimNet) -> Endpoint {
    stranger_with_key(net).await.0
}

async fn stranger_with_key(net: &SimNet) -> (Endpoint, SecretKey) {
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

async fn post(node: &Node, text: &str) -> ObjectId {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
        upload: None,
    };
    match call(node, req).await {
        ClientResponse::Posted { id } => id,
        other => panic!("unexpected {other:?}"),
    }
}

/// The signed profile a node gives anyone who asks.
async fn signed_profile_of(node: &Node, net: &SimNet) -> SignedObject {
    let me = stranger(net).await;
    let conn = me
        .connect(sim_addr(node.node_id()), SOCIAL_ALPN)
        .await
        .unwrap();
    let reply = ask_social(&conn, &SocialRequest::GetProfile).await;
    let SocialResponse::Profile { profile } = reply else {
        panic!("unexpected {reply:?}");
    };
    profile
}

fn person_in(profile: &SignedObject) -> Person {
    let Ok(Payload::Person(person)) = profile.verify() else {
        panic!("not a profile");
    };
    person
}

async fn profile_of(node: &Node, net: &SimNet) -> Person {
    person_in(&signed_profile_of(node, net).await)
}

fn text_of(post: &SignedObject) -> String {
    let Ok(Payload::Note(note)) = post.verify() else {
        panic!("not a post");
    };
    note.content
}

/// Asks a machine, as any stranger can.
async fn ask_machine(
    net: &SimNet,
    machine: EndpointId,
    request: &MachineRequest,
) -> MachineResponse {
    let me = stranger(net).await;
    let conn = me.connect(sim_addr(machine), MACHINE_ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, request).await.unwrap();
    send.finish().unwrap();
    let reply = read_frame(&mut recv).await.unwrap();
    // A machine serves a few at a time, and this one's done. Closing the
    // endpoint sees that it's told so.
    conn.close(0u32.into(), b"done");
    me.close().await;
    reply
}

/// Greets a machine from a new name that serves nothing, and so can't answer
/// when greeted back. Returns its record.
async fn greet(net: &SimNet, machine: EndpointId) -> MachineRecord {
    let (me, key) = stranger_with_key(net).await;
    let conn = me.connect(sim_addr(machine), MACHINE_ALPN).await.unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let record = MachineRecord::new(&key, vec![], osp_proto::now_rfc3339()).unwrap();
    write_frame(&mut send, &MachineRequest::Hello { record })
        .await
        .unwrap();
    send.finish().unwrap();
    match read_frame(&mut recv).await.unwrap() {
        MachineResponse::Hello { record } => record,
        other => panic!("unexpected {other:?}"),
    }
}

/// A directory's lists after `since`, all of them, and the newest number.
async fn read_lists(net: &SimNet, directory: &Node, since: u64) -> (Vec<ListingEntry>, u64) {
    let machine = directory.machine_id().unwrap();
    let request = MachineRequest::Listing { since, limit: 100 };
    match ask_machine(net, machine, &request).await {
        MachineResponse::Listing { entries, latest } => (entries, latest),
        other => panic!("unexpected {other:?}"),
    }
}

/// Reads the lists until `found` finds something in them.
async fn wait_for_entry<T>(
    net: &SimNet,
    directory: &Node,
    since: u64,
    found: impl Fn(&ListingEntry) -> Option<T>,
) -> T {
    let started = Instant::now();
    loop {
        let (entries, _) = read_lists(net, directory, since).await;
        if let Some(it) = entries.iter().find_map(&found) {
            return it;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "never appeared in {entries:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// What the lists say of someone: their entry's number, profile and posts.
fn listing_of(actor: iroh::PublicKey) -> impl Fn(&ListingEntry) -> Option<Listing> {
    move |entry| match &entry.item {
        ListingItem::Person { profile, posts, .. } if profile.signer() == actor => Some(Listing {
            seq: entry.seq,
            texts: posts.iter().map(text_of).collect(),
        }),
        _ => None,
    }
}

#[derive(Debug)]
struct Listing {
    seq: u64,
    texts: Vec<String>,
}

fn gone(actor: iroh::PublicKey) -> impl Fn(&ListingEntry) -> Option<u64> {
    move |entry| match &entry.item {
        ListingItem::Gone { actor: it } if *it == actor => Some(entry.seq),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_says_what_its_owner_set_and_a_later_one_takes_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(20);
    let home = dir.path().join("alice");
    let mut config = Config::local_test("Alice");
    config.summary = Some("  Builds oak tables  ".into());
    config.directory.list_me = true;
    let alice = start_with(dir.path(), "Alice", &net, config.clone()).await;

    let first = profile_of(&alice, &net).await;
    assert_eq!(first.summary.as_deref(), Some("Builds oak tables"));
    assert!(first.discoverable);

    // Nothing changed, so nothing is signed again.
    alice.shutdown().await.unwrap();
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    assert_eq!(profile_of(&alice, &net).await, first);
    alice.shutdown().await.unwrap();

    // New words alone sign a new one.
    config.summary = Some("Builds ash chairs".into());
    config.save(&Home::new(&home).config()).unwrap();
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    let reworded = profile_of(&alice, &net).await;
    assert_eq!(reworded.summary.as_deref(), Some("Builds ash chairs"));
    assert!(reworded.discoverable);
    alice.shutdown().await.unwrap();

    // Turned off, a later profile says so, and followers take the later one.
    config.summary = Some(" ".into());
    config.directory.list_me = false;
    config.save(&Home::new(&home).config()).unwrap();
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    let later = profile_of(&alice, &net).await;
    assert_eq!(later.summary, None);
    assert!(!later.discoverable);
    assert!(
        osp_proto::parse_rfc3339_ms(&later.published)
            > osp_proto::parse_rfc3339_ms(&first.published)
    );
    alice.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_with_too_long_a_summary_stops_the_node_saying_why() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(21);
    let home = dir.path().join("alice");
    let mut config = Config::local_test("Alice");
    config.summary = Some("x".repeat(osp_proto::types::MAX_SUMMARY_BYTES + 1));
    Node::init(&home, &config).unwrap();
    let Err(why) = Node::spawn_sim(&home, sim(&net)).await else {
        panic!("it started");
    };
    assert!(format!("{why:#}").contains("summary"), "{why:#}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restored_node_goes_on_saying_what_its_profile_said() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(22);
    let home = dir.path().join("alice");
    let mut config = Config::local_test("Alice");
    config.summary = Some("Builds oak tables".into());
    config.directory.list_me = true;
    let words = Node::init(&home, &config).unwrap().join(" ");
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    let file = dir.path().join("alice.ospbackup");
    let req = ClientRequest::ExportBackup {
        path: file.clone(),
        media: false,
    };
    call(&alice, req).await;
    alice.shutdown().await.unwrap();

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
    let alice = Node::spawn_sim(&restored, sim(&net)).await.unwrap();
    let person = profile_of(&alice, &net).await;
    assert_eq!(person.name, "Alice");
    assert_eq!(person.summary.as_deref(), Some("Builds oak tables"));
    assert!(person.discoverable);
    alice.shutdown().await.unwrap();
}

const AN_HOUR: Duration = Duration::from_secs(60 * 60);
const SOON: Duration = Duration::from_millis(100);

#[tokio::test(flavor = "multi_thread")]
async fn a_directory_gives_out_those_who_asked_and_nobody_else() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(23);
    let directory = start_directory(dir.path(), &net, AN_HOUR).await;
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    let bob = config_knowing("Bob", &directory, false);
    let bob = start_with(dir.path(), "Bob", &net, bob).await;
    post(&alice, "A table of green oak").await;
    post(&bob, "Bob would rather not be found").await;

    // Its record says what it is.
    let record = greet(&net, directory.machine_id().unwrap()).await;
    assert_eq!(record.verify().unwrap().offers, ["directory"]);

    assert!(alice.ask_to_be_listed().await);
    assert!(bob.ask_to_be_listed().await);
    let listing = wait_for_entry(&net, &directory, 0, listing_of(alice.identity())).await;
    assert_eq!(listing.texts, ["A table of green oak"]);
    let (entries, _) = read_lists(&net, &directory, 0).await;
    let bobs = listing_of(bob.identity());
    assert!(entries.iter().all(|e| bobs(e).is_none()), "{entries:?}");
    let shown = status(&directory).await.directory;
    assert_eq!(
        shown,
        Some(DirectoryStatus {
            people: 1,
            machines: 0
        })
    );

    // Nothing of theirs reaches its owner: it keeps them apart.
    let ClientResponse::Feed { items } = call(&directory, ClientRequest::Feed { limit: 50 }).await
    else {
        panic!("no feed");
    };
    assert!(items.is_empty(), "{items:?}");
    let s = status(&directory).await;
    assert_eq!(s.follows, 0);
    let req = ClientRequest::Search {
        words: "oak".into(),
        limit: 50,
    };
    let ClientResponse::Found { people, posts } = call(&directory, req).await else {
        panic!("no answer");
    };
    assert!(
        people.is_empty() && posts.is_empty(),
        "{people:?} {posts:?}"
    );

    // A node that's no directory says so, on either endpoint.
    let reply = ask_machine(
        &net,
        bob.machine_id().unwrap(),
        &MachineRequest::Listing {
            since: 0,
            limit: 10,
        },
    )
    .await;
    let MachineResponse::Error { message } = reply else {
        panic!("unexpected {reply:?}");
    };
    assert!(message.contains("isn't a directory"), "{message}");
    let me = stranger(&net).await;
    let conn = me
        .connect(sim_addr(bob.node_id()), SOCIAL_ALPN)
        .await
        .unwrap();
    let profile = signed_profile_of(&alice, &net).await;
    let reply = ask_social(&conn, &SocialRequest::AskListing { profile }).await;
    let SocialResponse::Error { message } = reply else {
        panic!("unexpected {reply:?}");
    };
    assert!(message.contains("isn't a directory"), "{message}");

    for node in [alice, bob, directory] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_can_list_or_drop_someone_else() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(24);
    let directory = start_directory(dir.path(), &net, AN_HOUR).await;
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    let profile = signed_profile_of(&alice, &net).await;

    // Her profile, valid as it is, from a node it doesn't list.
    let me = stranger(&net).await;
    let conn = me
        .connect(sim_addr(directory.node_id()), SOCIAL_ALPN)
        .await
        .unwrap();
    let reply = ask_social(&conn, &SocialRequest::AskListing { profile }).await;
    let SocialResponse::Error { message } = reply else {
        panic!("unexpected {reply:?}");
    };
    assert!(message.contains("doesn't list your node"), "{message}");
    let shown = status(&directory).await.directory.unwrap();
    assert_eq!(shown.people, 0);

    // A profile saying she's on this stranger's node, with a signature for
    // something else.
    let (me, key) = stranger_with_key(&net).await;
    let mut forged = serde_json::to_value(signed_profile_of(&alice, &net).await).unwrap();
    let person = Person {
        nodes: vec![key.public()],
        media_endpoints: vec![],
        ..profile_of(&alice, &net).await
    };
    let bytes = serde_json::to_vec(&Payload::Person(person)).unwrap();
    forged["payload"] = serde_json::json!(data_encoding::BASE64URL_NOPAD.encode(&bytes));
    let forged: SignedObject = serde_json::from_value(forged).unwrap();
    let conn = me
        .connect(sim_addr(directory.node_id()), SOCIAL_ALPN)
        .await
        .unwrap();
    let reply = ask_social(&conn, &SocialRequest::AskListing { profile: forged }).await;
    assert!(matches!(reply, SocialResponse::Error { .. }), "{reply:?}");
    assert_eq!(status(&directory).await.directory.unwrap().people, 0);

    // Listed, she can't be dropped by a forged profile saying she'd rather not.
    assert!(alice.ask_to_be_listed().await);
    wait_for_entry(&net, &directory, 0, listing_of(alice.identity())).await;
    let person = Person {
        discoverable: false,
        nodes: vec![key.public()],
        media_endpoints: vec![],
        ..profile_of(&alice, &net).await
    };
    let bytes = serde_json::to_vec(&Payload::Person(person)).unwrap();
    let mut forged = serde_json::to_value(signed_profile_of(&alice, &net).await).unwrap();
    forged["payload"] = serde_json::json!(data_encoding::BASE64URL_NOPAD.encode(&bytes));
    let forged: SignedObject = serde_json::from_value(forged).unwrap();
    let reply = ask_social(&conn, &SocialRequest::AskListing { profile: forged }).await;
    assert!(matches!(reply, SocialResponse::Error { .. }), "{reply:?}");
    assert_eq!(status(&directory).await.directory.unwrap().people, 1);

    // Her post isn't a profile.
    let id = post(&alice, "Not a profile").await;
    let conn = me
        .connect(sim_addr(alice.node_id()), SOCIAL_ALPN)
        .await
        .unwrap();
    let reply = ask_social(&conn, &SocialRequest::GetObjects { ids: vec![id] }).await;
    let SocialResponse::Objects { objects } = reply else {
        panic!("unexpected {reply:?}");
    };
    let conn = me
        .connect(sim_addr(directory.node_id()), SOCIAL_ALPN)
        .await
        .unwrap();
    let reply = ask_social(
        &conn,
        &SocialRequest::AskListing {
            profile: objects[0].clone(),
        },
    )
    .await;
    assert!(matches!(reply, SocialResponse::Error { .. }), "{reply:?}");

    for node in [alice, directory] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listing_has_the_latest_posts_and_a_reader_gets_only_what_changed() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(25);
    let directory = start_directory(dir.path(), &net, SOON).await;
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    for i in 0..7 {
        post(&alice, &format!("Post {i}")).await;
    }
    assert!(alice.ask_to_be_listed().await);
    let first = wait_for_entry(&net, &directory, 0, listing_of(alice.identity())).await;
    assert_eq!(
        first.texts,
        ["Post 6", "Post 5", "Post 4", "Post 3", "Post 2"]
    );

    // Read again and again with nothing new, it stays as it is.
    tokio::time::sleep(SOON * 5).await;
    let (entries, latest) = read_lists(&net, &directory, first.seq).await;
    assert!(entries.is_empty(), "{entries:?}");
    assert_eq!(latest, first.seq);

    // A new post gives it a new number, and it's still one entry.
    post(&alice, "Post 7").await;
    let second = wait_for_entry(&net, &directory, first.seq, listing_of(alice.identity())).await;
    assert_eq!(
        second.texts,
        ["Post 7", "Post 6", "Post 5", "Post 4", "Post 3"]
    );
    let (entries, _) = read_lists(&net, &directory, 0).await;
    assert_eq!(entries.len(), 1, "{entries:?}");

    // Asking for none says where the end is, and a page holds what's asked.
    let reply = ask_machine(
        &net,
        directory.machine_id().unwrap(),
        &MachineRequest::Listing { since: 0, limit: 0 },
    )
    .await;
    let MachineResponse::Listing { entries, latest } = reply else {
        panic!("unexpected {reply:?}");
    };
    assert!(entries.is_empty());
    assert_eq!(latest, second.seq);

    for node in [alice, directory] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn someone_who_takes_it_back_is_gone_when_they_ask_or_when_next_read() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(26);
    // Asking: this directory won't read her again for an hour.
    let slow = start_directory(dir.path(), &net, AN_HOUR).await;
    let config = config_knowing("Alice", &slow, true);
    let alice = start_with(dir.path(), "Alice", &net, config.clone()).await;
    let home = dir.path().join("alice");
    let actor = alice.identity();
    post(&alice, "A table of green oak").await;
    assert!(alice.ask_to_be_listed().await);
    let listing = wait_for_entry(&net, &slow, 0, listing_of(actor)).await;
    alice.shutdown().await.unwrap();

    let mut off = config.clone();
    off.directory.list_me = false;
    off.save(&Home::new(&home).config()).unwrap();
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    assert!(alice.ask_to_be_listed().await);
    let (entries, _) = read_lists(&net, &slow, listing.seq).await;
    assert!(
        entries.iter().any(|e| gone(actor)(e).is_some()),
        "{entries:?}"
    );
    // Nothing of what was listed is left, for a reader from the start either.
    let (entries, _) = read_lists(&net, &slow, 0).await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(status(&slow).await.directory.unwrap().people, 0);
    slow.shutdown().await.unwrap();
    alice.shutdown().await.unwrap();

    // Not asking: this one sees it when it next reads her node.
    let quick = start_directory(&dir.path().join("second"), &net, SOON).await;
    let mut on = config_knowing("Alice", &quick, true);
    on.save(&Home::new(&home).config()).unwrap();
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    assert!(alice.ask_to_be_listed().await);
    wait_for_entry(&net, &quick, 0, listing_of(actor)).await;
    alice.shutdown().await.unwrap();
    on.directory.list_me = false;
    on.save(&Home::new(&home).config()).unwrap();
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    wait_for_entry(&net, &quick, 0, gone(actor)).await;

    // Allowed again, they come back.
    alice.shutdown().await.unwrap();
    on.directory.list_me = true;
    on.save(&Home::new(&home).config()).unwrap();
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    assert!(alice.ask_to_be_listed().await);
    let listing = wait_for_entry(&net, &quick, 0, listing_of(actor)).await;
    assert_eq!(listing.texts, ["A table of green oak"]);

    for node in [alice, quick] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_directory_takes_a_few_new_listings_an_hour() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(27);
    let mut config = Config::local_test("The directory");
    config.directory.serve = true;
    let settings = SimSettings {
        new_listings_per_hour: 1,
        ..sim(&net)
    };
    let directory = start_sim(dir.path(), "directory", config, settings).await;
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    let bob = config_knowing("Bob", &directory, true);
    let bob = start_with(dir.path(), "Bob", &net, bob).await;

    assert!(alice.ask_to_be_listed().await);
    assert!(!bob.ask_to_be_listed().await);
    // Someone listed can ask again: it takes no new place.
    assert!(alice.ask_to_be_listed().await);
    assert_eq!(status(&directory).await.directory.unwrap().people, 1);

    for node in [alice, bob, directory] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_map_holds_machines_that_answered_and_says_nothing_of_people() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(28);
    let directory = start_directory(dir.path(), &net, AN_HOUR).await;
    let its_machine = directory.machine_id().unwrap();
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    let her_machine = alice.machine_id().unwrap();
    post(&alice, "A table of green oak").await;
    assert!(alice.ask_to_be_listed().await);
    wait_for_entry(&net, &directory, 0, listing_of(alice.identity())).await;

    // Greeting from a machine that won't answer back leaves no mark.
    greet(&net, its_machine).await;

    let hello = ClientRequest::MachineHello {
        machine: its_machine,
    };
    call(&alice, hello).await;
    let on_the_map = |entry: &ListingEntry| match &entry.item {
        ListingItem::Machine { record } => Some(record.verify().unwrap()),
        _ => None,
    };
    let info = wait_for_entry(&net, &directory, 0, on_the_map).await;
    assert_eq!(info.machine, her_machine);
    let (entries, _) = read_lists(&net, &directory, 0).await;
    assert_eq!(
        entries
            .iter()
            .filter(|entry| on_the_map(entry).is_some())
            .count(),
        1,
        "{entries:?}"
    );
    assert_eq!(status(&directory).await.directory.unwrap().machines, 1);

    // Neither list ties her machine to her.
    for entry in &entries {
        let shown = match &entry.item {
            ListingItem::Machine { record } => format!("{:?}", record.verify().unwrap()),
            ListingItem::Person { profile, posts, .. } => {
                let posts: Vec<_> = posts.iter().map(text_of).collect();
                format!("{:?} {posts:?}", person_in(profile))
            }
            other => panic!("unexpected {other:?}"),
        };
        let whose = match &entry.item {
            ListingItem::Machine { .. } => {
                [alice.identity().to_string(), alice.node_id().to_string()]
            }
            _ => [her_machine.to_string(), her_machine.fmt_short().to_string()],
        };
        for name in whose {
            assert!(!shown.contains(&name), "{shown} names {name}");
        }
    }

    // Greeted back once, it isn't dialled again for each greeting.
    let hello = ClientRequest::MachineHello {
        machine: its_machine,
    };
    call(&alice, hello).await;
    tokio::time::sleep(SOON).await;
    let (again, _) = read_lists(&net, &directory, 0).await;
    let seqs = |entries: &[ListingEntry]| entries.iter().map(|e| e.seq).collect::<Vec<_>>();
    assert_eq!(seqs(&again), seqs(&entries));

    for node in [alice, directory] {
        node.shutdown().await.unwrap();
    }
}

async fn search(node: &Node, words: &str) -> (Vec<PersonInfo>, Vec<FeedItem>) {
    let req = ClientRequest::Search {
        words: words.into(),
        limit: 50,
    };
    match call(node, req).await {
        ClientResponse::Found { people, posts } => (people, posts),
        other => panic!("unexpected {other:?}"),
    }
}

async fn found_posts(node: &Node, words: &str) -> Vec<String> {
    let posts = search(node, words).await.1;
    posts.into_iter().map(|item| item.content).collect()
}

async fn found_people(node: &Node, words: &str) -> Vec<String> {
    let people = search(node, words).await.0;
    people.into_iter().map(|person| person.name).collect()
}

/// Reads the directories a node knows, and says how many people it now holds
/// from them.
async fn read_directories(node: &Node) -> u64 {
    match call(node, ClientRequest::ReadDirectories).await {
        ClientResponse::DirectoriesRead { found } => found,
        other => panic!("unexpected {other:?}"),
    }
}

async fn feed(node: &Node) -> Vec<FeedItem> {
    match call(node, ClientRequest::Feed { limit: 50 }).await {
        ClientResponse::Feed { items } => items,
        other => panic!("unexpected {other:?}"),
    }
}

/// A date `secs` from now.
fn at(secs: i64) -> String {
    let now = SystemTime::now();
    let then = if secs < 0 {
        now - Duration::from_secs(-secs as u64)
    } else {
        now + Duration::from_secs(secs as u64)
    };
    humantime::format_rfc3339_millis(then).to_string()
}

/// A stranger's signed profile, as a directory would pass it on.
fn signed_person(key: &SecretKey, name: &str, summary: &str, discoverable: bool) -> SignedObject {
    let person = Person {
        actor: key.public(),
        name: name.into(),
        summary: Some(summary.into()),
        discoverable,
        nodes: vec![SecretKey::generate().public()],
        media_endpoints: vec![],
        published: at(-60),
    };
    SignedObject::sign(key, &Payload::Person(person)).unwrap()
}

fn note(key: &SecretKey, text: &str) -> Payload {
    Payload::Note(Note {
        actor: key.public(),
        content: text.into(),
        published: at(-60),
        attachment: vec![],
        in_reply_to: None,
    })
}

fn signed_note(key: &SecretKey, text: &str) -> SignedObject {
    SignedObject::sign(key, &note(key, text)).unwrap()
}

fn person_entry(seq: u64, profile: SignedObject, posts: Vec<SignedObject>) -> ListingEntry {
    ListingEntry {
        seq,
        item: ListingItem::Person {
            profile,
            posts,
            reshares: vec![],
        },
    }
}

/// What a directory run by a test says. It says what it's told to, however
/// wrong, and notes every request it's sent as it came.
#[derive(Default)]
struct FakeLists {
    entries: Vec<ListingEntry>,
    latest: u64,
    /// Entries in one page, unless it's told to give them all at once.
    page: usize,
    all_at_once: bool,
    heard: Vec<String>,
}

#[derive(Clone)]
struct FakeDirectory {
    key: SecretKey,
    offers: Vec<String>,
    lists: Arc<Mutex<FakeLists>>,
}

impl std::fmt::Debug for FakeDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FakeDirectory")
    }
}

impl FakeDirectory {
    fn answer(&self, request: serde_json::Value) -> MachineResponse {
        let mut lists = self.lists.lock().unwrap();
        lists.heard.push(request.to_string());
        match serde_json::from_value(request).unwrap() {
            MachineRequest::Hello { .. } => MachineResponse::Hello {
                record: MachineRecord::new(
                    &self.key,
                    self.offers.clone(),
                    osp_proto::now_rfc3339(),
                )
                .unwrap(),
            },
            MachineRequest::Listing { since, limit } => {
                let page = match lists.page {
                    0 => limit as usize,
                    page => page,
                };
                let entries = match lists.all_at_once {
                    true => lists.entries.clone(),
                    false => lists
                        .entries
                        .iter()
                        .filter(|entry| entry.seq > since)
                        .take(page)
                        .cloned()
                        .collect(),
                };
                MachineResponse::Listing {
                    entries,
                    latest: lists.latest,
                }
            }
            MachineRequest::Known => MachineResponse::Known { records: vec![] },
            MachineRequest::Introduce { .. } => MachineResponse::Introduce { passed_on: false },
            MachineRequest::Introduction { .. } => MachineResponse::Introduction,
            MachineRequest::Unknown => panic!("a request it doesn't know"),
        }
    }
}

impl ProtocolHandler for FakeDirectory {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let request: serde_json::Value = read_frame(&mut recv).await.unwrap();
            let reply = self.answer(request);
            write_frame(&mut send, &reply).await.unwrap();
            send.finish().unwrap();
        }
        Ok(())
    }
}

/// Starts a fake directory saying it's one, and a node knowing it and nothing
/// else. The router has to stay alive.
async fn start_with_fake(
    root: &Path,
    net: &SimNet,
    settings: SimSettings,
) -> (Node, Arc<Mutex<FakeLists>>, Router) {
    let (endpoint, key) = stranger_with_key(net).await;
    let lists = Arc::new(Mutex::new(FakeLists::default()));
    let fake = FakeDirectory {
        key: key.clone(),
        offers: vec!["directory".into()],
        lists: lists.clone(),
    };
    let router = Router::builder(endpoint).accept(MACHINE_ALPN, fake).spawn();
    let mut config = Config::local_test("Carol");
    config.directory.known = vec![KnownDirectory {
        machine: key.public(),
        node: SecretKey::generate().public(),
    }];
    let carol = start_sim(root, "Carol", config, settings).await;
    (carol, lists, router)
}

/// The gate's first half: nothing but a directory, and search finds a stranger.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_with_no_follows_finds_a_stranger_and_searches_with_nobody_there() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(30);
    let directory = start_directory(dir.path(), &net, AN_HOUR).await;
    let mut alice = config_knowing("Alice", &directory, true);
    alice.summary = Some("Builds oak tables".into());
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    post(&alice, "A table of green walnut").await;
    assert!(alice.ask_to_be_listed().await);
    wait_for_entry(&net, &directory, 0, listing_of(alice.identity())).await;

    let carol = config_knowing("Carol", &directory, false);
    let carol = start_with(dir.path(), "Carol", &net, carol).await;
    assert_eq!(read_directories(&carol).await, 1);
    assert_eq!(status(&carol).await.found_people, 1);

    // Reading greeted the directory, and it greets a machine back. Alice's
    // machine may be on the map too, and sooner, so look for Carol's.
    let carols = carol.machine_id().unwrap();
    let carol_on_the_map = |entry: &ListingEntry| match &entry.item {
        ListingItem::Machine { record } if record.verify().unwrap().machine == carols => Some(()),
        _ => None,
    };
    wait_for_entry(&net, &directory, 0, carol_on_the_map).await;

    // With the others gone, it's all still here.
    let alices_identity = alice.identity();
    let ClientResponse::MachineName {
        words: directory_words,
        ..
    } = call(&directory, ClientRequest::MachineName).await
    else {
        panic!("no name");
    };
    alice.shutdown().await.unwrap();
    directory.shutdown().await.unwrap();

    let (people, posts) = search(&carol, "oak").await;
    assert_eq!(people.len(), 1);
    assert_eq!(people[0].actor, alices_identity);
    assert_eq!(people[0].summary.as_deref(), Some("Builds oak tables"));
    assert!(!people[0].followed);
    assert!(posts.is_empty());
    let (people, posts) = search(&carol, "walnut").await;
    assert!(people.is_empty());
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].actor_name.as_deref(), Some("Alice"));

    // They say where they came from, by the words of its machine name.
    assert_eq!(
        posts[0].found_through.as_deref(),
        Some(directory_words.as_str())
    );
    let (people, _) = search(&carol, "alice").await;
    assert_eq!(
        people[0].found_through.as_deref(),
        Some(directory_words.as_str())
    );

    // But it's no follow, and none of it is in the feed.
    assert!(feed(&carol).await.is_empty());
    assert_eq!(status(&carol).await.follows, 0);
    carol.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn left_alone_a_node_asks_to_be_listed_and_another_finds_it() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(31);
    let directory = start_directory(dir.path(), &net, SOON).await;
    let quick = || SimSettings {
        ask_listing_wait: SOON,
        first_read_wait: SOON,
        read_every: SOON * 2,
        ..sim(&net)
    };
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_sim(dir.path(), "Alice", alice, quick()).await;
    post(&alice, "A table of green oak").await;
    let carol = config_knowing("Carol", &directory, false);
    let carol = start_sim(dir.path(), "Carol", carol, quick()).await;

    let started = Instant::now();
    while found_posts(&carol, "oak").await.is_empty() {
        assert!(started.elapsed() < Duration::from_secs(30), "never found");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for node in [alice, carol, directory] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listing_replaces_what_it_brought_and_someone_gone_drops_out() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(32);
    let directory = start_directory(dir.path(), &net, SOON).await;
    let config = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, config.clone()).await;
    let home = dir.path().join("alice");
    post(&alice, "The first post of hers").await;
    assert!(alice.ask_to_be_listed().await);
    let first = wait_for_entry(&net, &directory, 0, listing_of(alice.identity())).await;

    let carol = config_knowing("Carol", &directory, false);
    let carol = start_with(dir.path(), "Carol", &net, carol).await;
    assert_eq!(read_directories(&carol).await, 1);
    assert_eq!(found_posts(&carol, "first").await.len(), 1);

    // Five more push the first out of her listing, and so off this node.
    for i in 0..5 {
        post(&alice, &format!("A later post, number {i}")).await;
    }
    let want = |entry: &ListingEntry| {
        let listing = listing_of(alice.identity())(entry)?;
        (listing.texts[0] == "A later post, number 4").then_some(listing)
    };
    wait_for_entry(&net, &directory, first.seq, want).await;
    read_directories(&carol).await;
    assert!(found_posts(&carol, "first").await.is_empty());
    assert_eq!(found_posts(&carol, "later").await.len(), 5);

    // She takes it back: everything goes, her profile too.
    alice.shutdown().await.unwrap();
    let mut off = config.clone();
    off.directory.list_me = false;
    off.save(&Home::new(&home).config()).unwrap();
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    wait_for_entry(&net, &directory, 0, gone(alice.identity())).await;
    assert_eq!(read_directories(&carol).await, 0);
    assert!(found_posts(&carol, "later").await.is_empty());
    assert!(found_people(&carol, "alice").await.is_empty());

    for node in [alice, carol, directory] {
        node.shutdown().await.unwrap();
    }
}

/// The gate's second half. A reader says only that it's here and how far it has
/// read, whatever is searched for.
#[tokio::test(flavor = "multi_thread")]
async fn no_word_anyone_types_leaves_the_node() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(33);
    let (carol, lists, _router) = start_with_fake(dir.path(), &net, sim(&net)).await;
    let key = SecretKey::generate();
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![person_entry(
            1,
            signed_person(&key, "Alice", "Builds tables", true),
            vec![signed_note(&key, "A table of green oak")],
        )];
        lists.latest = 1;
    }
    read_directories(&carol).await;

    for words in ["xylophonist", "oak", "alice"] {
        search(&carol, words).await;
    }
    assert_eq!(found_posts(&carol, "oak").await.len(), 1);
    read_directories(&carol).await;
    search(&carol, "xylophonist").await;
    read_directories(&carol).await;

    let heard = lists.lock().unwrap().heard.clone();
    assert!(heard.len() >= 6, "{heard:?}");
    for request in &heard {
        let parsed: serde_json::Value = serde_json::from_str(request).unwrap();
        let op = parsed["op"].as_str().unwrap();
        assert!(["hello", "listing"].contains(&op), "{request}");
        if op == "listing" {
            let fields: Vec<_> = parsed.as_object().unwrap().keys().collect();
            assert_eq!(fields, ["limit", "op", "since"], "{request}");
        }
        for words in ["xylophonist", "oak", "alice"] {
            assert!(!request.contains(words), "{request}");
        }
    }

    carol.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hostile_directory_costs_only_its_own_entries() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(34);
    let (carol, lists, _router) = start_with_fake(dir.path(), &net, sim(&net)).await;
    let honest = SecretKey::generate();
    let wrong = SecretKey::generate();
    let someone = SecretKey::generate;

    let mut bad_signature = serde_json::to_value(signed_note(&wrong, "A forged oak")).unwrap();
    bad_signature["signer"] = serde_json::to_value(wrong.public()).unwrap();
    bad_signature["sig"] = serde_json::to_value("A".repeat(86)).unwrap();
    let bad_signature: SignedObject = serde_json::from_value(bad_signature).unwrap();

    let dated_ahead = someone();
    let ahead = Person {
        actor: dated_ahead.public(),
        name: "Dated ahead".into(),
        summary: None,
        discoverable: true,
        nodes: vec![],
        media_endpoints: vec![],
        published: at(30 * 24 * 60 * 60),
    };
    let ahead = SignedObject::sign(&dated_ahead, &Payload::Person(ahead)).unwrap();
    let (no_consent, too_many, resharer, not_a_profile) =
        (someone(), someone(), someone(), someone());
    let reshare = Payload::Announce(Announce {
        actor: resharer.public(),
        object: signed_note(&honest, "x").id(),
        object_actor: honest.public(),
        published: at(-60),
        hosting: false,
    });
    let bad = [
        // A post by someone other than whose entry it is.
        person_entry(
            1,
            signed_person(&someone(), "Wrong post", "words", true),
            vec![signed_note(&honest, "An oak by someone else")],
        ),
        // A signature that doesn't check out.
        person_entry(
            2,
            signed_person(&wrong, "Forged post", "words", true),
            vec![bad_signature],
        ),
        // One who never said they may be listed.
        person_entry(
            3,
            signed_person(&no_consent, "No consent", "words", false),
            vec![signed_note(&no_consent, "An oak without consent")],
        ),
        // More posts than a listing has.
        person_entry(
            4,
            signed_person(&too_many, "Too many", "words", true),
            (0..6)
                .map(|i| signed_note(&too_many, &format!("Too many oaks {i}")))
                .collect(),
        ),
        // A profile dated ahead, and something other than a post.
        person_entry(5, ahead, vec![]),
        person_entry(
            6,
            signed_person(&resharer, "Not a post", "words", true),
            vec![SignedObject::sign(&resharer, &reshare).unwrap()],
        ),
        // A post in place of a profile.
        person_entry(7, signed_note(&not_a_profile, "Not a profile"), vec![]),
        // From a later version.
        ListingEntry {
            seq: 8,
            item: ListingItem::Unknown,
        },
    ];
    let good = person_entry(
        9,
        signed_person(&honest, "Honest", "Builds tables", true),
        vec![signed_note(&honest, "A table of green oak")],
    );
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = bad.into_iter().chain([good]).collect();
        lists.latest = 9;
    }
    assert_eq!(read_directories(&carol).await, 1);
    assert_eq!(found_posts(&carol, "oak").await, ["A table of green oak"]);
    assert_eq!(found_people(&carol, "honest").await, ["Honest"]);
    assert!(found_people(&carol, "words").await.is_empty());

    // Its numbers must rise, or it could keep a reader going round.
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![
            person_entry(
                10,
                signed_person(&someone(), "Good so far", "words", true),
                vec![],
            ),
            person_entry(
                10,
                signed_person(&wrong, "Out of order", "words", true),
                vec![],
            ),
        ];
        lists.latest = 10;
        lists.all_at_once = true;
    }
    let why = match carol.handle(ClientRequest::ReadDirectories).await {
        ClientResponse::Error { message } => message,
        other => panic!("unexpected {other:?}"),
    };
    assert!(why.contains("out of order"), "{why}");
    assert!(found_people(&carol, "order").await.is_empty());

    // What's numbered past what it says is its newest is the same.
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![person_entry(
            11,
            signed_person(&wrong, "Past the end", "words", true),
            vec![],
        )];
        lists.latest = 10;
    }
    let why = match carol.handle(ClientRequest::ReadDirectories).await {
        ClientResponse::Error { message } => message,
        other => panic!("unexpected {other:?}"),
    };
    assert!(why.contains("out of order"), "{why}");

    // A machine that doesn't say it's a directory isn't read as one.
    carol.shutdown().await.unwrap();
    let (endpoint, key) = stranger_with_key(&net).await;
    let not_one = FakeDirectory {
        key: key.clone(),
        offers: vec![],
        lists: Arc::new(Mutex::new(FakeLists::default())),
    };
    let _router = Router::builder(endpoint)
        .accept(MACHINE_ALPN, not_one)
        .spawn();
    let mut config = Config::local_test("Dave");
    config.directory.known = vec![KnownDirectory {
        machine: key.public(),
        node: SecretKey::generate().public(),
    }];
    let dave = start_with(dir.path(), "Dave", &net, config).await;
    let why = match dave.handle(ClientRequest::ReadDirectories).await {
        ClientResponse::Error { message } => message,
        other => panic!("unexpected {other:?}"),
    };
    assert!(why.contains("doesn't say it's a directory"), "{why}");
    dave.shutdown().await.unwrap();
}

/// Once this node holds a later profile saying no, as from having followed them,
/// an older one saying yes doesn't bring them back. It holds no such profile for
/// someone only ever found, as that goes with the rest of them: with more than
/// one directory, one behind could list them again (ADR 0014's step 6).
#[tokio::test(flavor = "multi_thread")]
async fn a_later_profile_this_node_holds_isnt_undone_by_a_directory_behind() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(35);
    let (carol, lists, _router) = start_with_fake(dir.path(), &net, sim(&net)).await;
    let mut config = Config::local_test("Alice");
    config.directory.list_me = true;
    let alice = start_with(dir.path(), "Alice", &net, config.clone()).await;
    let home = dir.path().join("alice");
    let actor = alice.identity();
    let older = signed_profile_of(&alice, &net).await;
    post(&alice, "A table of green oak").await;
    alice.shutdown().await.unwrap();
    config.directory.list_me = false;
    config.save(&Home::new(&home).config()).unwrap();
    let alice = Node::spawn_sim(&home, sim(&net)).await.unwrap();

    // Followed, then not: her later profile is still here, as what's received is.
    let ClientResponse::Ticket { ticket } = call(&alice, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(&carol, ClientRequest::Follow { ticket }).await;
    call(&carol, ClientRequest::Sync).await;
    call(&carol, ClientRequest::Unfollow { actor }).await;
    assert_eq!(found_posts(&carol, "oak").await.len(), 1);

    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![person_entry(1, older, vec![])];
        lists.latest = 1;
    }
    assert_eq!(read_directories(&carol).await, 0);
    let (people, _) = search(&carol, "alice").await;
    assert_eq!(people[0].found_through, None);

    for node in [alice, carol] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_large_directory_is_read_a_part_at_a_time_and_one_starting_over_from_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(36);
    let (carol, lists, _router) = start_with_fake(dir.path(), &net, sim(&net)).await;
    let people: Vec<_> = (0..70)
        .map(|i| {
            let key = SecretKey::generate();
            person_entry(i + 1, signed_person(&key, "Someone", "words", true), vec![])
        })
        .collect();
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = people.clone();
        lists.latest = 70;
        lists.page = 1;
    }
    // At most 64 pages in one conversation, and it goes on from there.
    assert_eq!(read_directories(&carol).await, 64);
    assert_eq!(read_directories(&carol).await, 70);
    assert_eq!(read_directories(&carol).await, 70);

    // Its numbers went down, so it isn't the lists this node read.
    let key = SecretKey::generate();
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![person_entry(
            1,
            signed_person(&key, "After starting over", "words", true),
            vec![],
        )];
        lists.latest = 1;
    }
    assert_eq!(read_directories(&carol).await, 1);
    assert_eq!(
        found_people(&carol, "starting").await,
        ["After starting over"]
    );
    carol.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_signed_by_a_key_since_revoked_drops_out_and_stays_out() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(37);
    let (carol, lists, _router) = start_with_fake(dir.path(), &net, sim(&net)).await;
    let identity = SecretKey::generate();
    let certify = |secs, earlier| {
        CertifiedKey::certify(&identity, SecretKey::generate(), at(secs), earlier).unwrap()
    };
    let (first, second) = (
        certify(-3600, EarlierKeys::Retired),
        certify(-60, EarlierKeys::Revoked),
    );
    let person = Person {
        actor: identity.public(),
        name: "Alice".into(),
        summary: None,
        discoverable: true,
        nodes: vec![],
        media_endpoints: vec![],
        published: at(-120),
    };
    let profile = |key: &CertifiedKey| {
        SignedObject::sign_certified(key, &Payload::Person(person.clone())).unwrap()
    };
    let hers = note(&identity, "A table of green oak");
    let thiefs = note(&identity, "A table of stolen ash");

    // Both signed with the first key, one by someone who copied it.
    let old_entry = |seq| {
        person_entry(
            seq,
            profile(&first),
            vec![
                SignedObject::sign_certified(&first, &hers).unwrap(),
                SignedObject::sign_certified(&first, &thiefs).unwrap(),
            ],
        )
    };
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![old_entry(1)];
        lists.latest = 1;
    }
    assert_eq!(read_directories(&carol).await, 1);
    assert_eq!(found_posts(&carol, "ash").await.len(), 1);

    // She revokes it. The certificate on her profile alone drops what it signed.
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![person_entry(2, profile(&second), vec![])];
        lists.latest = 2;
    }
    assert_eq!(read_directories(&carol).await, 1);
    assert!(found_posts(&carol, "ash").await.is_empty());

    // And from it alone, this node knows to refuse the old key from then on.
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![old_entry(3)];
        lists.latest = 3;
    }
    read_directories(&carol).await;
    assert!(found_posts(&carol, "ash").await.is_empty());

    // Her own post comes back signed by the new key.
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![person_entry(
            4,
            profile(&second),
            vec![SignedObject::sign_certified(&second, &hers).unwrap()],
        )];
        lists.latest = 4;
    }
    assert_eq!(read_directories(&carol).await, 1);
    assert_eq!(found_posts(&carol, "oak").await.len(), 1);

    // A directory behind, or lying, can't bring the old one back, whether it's
    // all old or only the post is.
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![
            old_entry(5),
            person_entry(
                6,
                profile(&second),
                vec![SignedObject::sign_certified(&first, &thiefs).unwrap()],
            ),
        ];
        lists.latest = 6;
    }
    read_directories(&carol).await;
    assert!(found_posts(&carol, "ash").await.is_empty());
    assert_eq!(found_posts(&carol, "oak").await.len(), 1);
    carol.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn those_found_go_first_when_the_disk_is_nearly_full() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(38);
    let settings = SimSettings {
        found_needs_free: u64::MAX,
        ..sim(&net)
    };
    let (carol, lists, _router) = start_with_fake(dir.path(), &net, sim(&net)).await;
    let key = SecretKey::generate();
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![person_entry(
            1,
            signed_person(&key, "Alice", "words", true),
            vec![signed_note(&key, "A table of green oak")],
        )];
        lists.latest = 1;
    }
    assert_eq!(read_directories(&carol).await, 1);
    assert!(lists.lock().unwrap().heard.len() >= 2);
    post(&carol, "A post of her own, of oak").await;
    carol.shutdown().await.unwrap();

    // The same node, with too little room: they go, and nothing's asked for.
    lists.lock().unwrap().heard.clear();
    let home = dir.path().join("carol");
    let carol = Node::spawn_sim(&home, settings).await.unwrap();
    assert_eq!(read_directories(&carol).await, 0);
    assert_eq!(
        found_posts(&carol, "oak").await,
        ["A post of her own, of oak"]
    );
    assert!(lists.lock().unwrap().heard.is_empty());
    carol.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_reads_no_directory_without_a_machine_name_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(39);
    let mut config = Config::local_test("Carol");
    config.machine.enabled = false;
    config.directory.known = vec![KnownDirectory {
        machine: SecretKey::generate().public(),
        node: SecretKey::generate().public(),
    }];
    let carol = start_with(dir.path(), "Carol", &net, config).await;
    let why = match carol.handle(ClientRequest::ReadDirectories).await {
        ClientResponse::Error { message } => message,
        other => panic!("unexpected {other:?}"),
    };
    assert!(why.contains("no machine name"), "{why}");
    carol.shutdown().await.unwrap();

    let dave = Config::local_test("Dave");
    let dave = start_with(dir.path(), "Dave", &net, dave).await;
    let why = match dave.handle(ClientRequest::ReadDirectories).await {
        ClientResponse::Error { message } => message,
        other => panic!("unexpected {other:?}"),
    };
    assert!(why.contains("knows no directories"), "{why}");
    dave.shutdown().await.unwrap();

    // A directory needs one too.
    let mut config = Config::local_test("Erin");
    config.directory.serve = true;
    config.machine.enabled = false;
    let home = dir.path().join("erin");
    Node::init(&home, &config).unwrap();
    let Err(why) = Node::spawn_sim(&home, sim(&net)).await else {
        panic!("it started");
    };
    assert!(
        format!("{why:#}").contains("needs a machine name"),
        "{why:#}"
    );
}

/// The gate: a node with no follows finds a stranger and follows them, and no
/// follow code changed hands.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_with_no_follows_finds_a_stranger_and_follows_them() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(40);
    let directory = start_directory(dir.path(), &net, AN_HOUR).await;
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    for i in 0..7 {
        post(&alice, &format!("Oak table number {i}")).await;
    }
    assert!(alice.ask_to_be_listed().await);
    wait_for_entry(&net, &directory, 0, listing_of(alice.identity())).await;

    let carol = config_knowing("Carol", &directory, false);
    let carol = start_with(dir.path(), "Carol", &net, carol).await;
    read_directories(&carol).await;
    let (people, posts) = search(&carol, "alice").await;
    assert_eq!(people.len(), 1);
    assert!(posts.is_empty());
    assert!(feed(&carol).await.is_empty());

    call(
        &carol,
        ClientRequest::FollowPerson {
            actor: people[0].actor,
        },
    )
    .await;
    call(&carol, ClientRequest::Sync).await;

    // She's followed now, like anyone: all she posted, not only what was listed.
    assert_eq!(feed(&carol).await.len(), 7);
    let (people, posts) = search(&carol, "alice").await;
    assert!(people[0].followed);
    assert_eq!(people[0].found_through, None);
    assert!(posts.is_empty());
    let (_, posts) = search(&carol, "oak").await;
    assert_eq!(posts.len(), 7);
    assert!(posts.iter().all(|post| post.found_through.is_none()));
    assert_eq!(status(&carol).await.follows, 1);
    assert_eq!(status(&carol).await.found_people, 0);

    // New posts reach her directly, with the directory gone.
    let identity = alice.identity();
    directory.shutdown().await.unwrap();
    post(&alice, "Oak table number 7").await;
    call(&carol, ClientRequest::Sync).await;
    assert_eq!(feed(&carol).await.len(), 8);

    // The directory coming back around doesn't make her a stranger.
    let ClientResponse::Following { follows } = call(&carol, ClientRequest::Following).await else {
        panic!("no follows");
    };
    assert_eq!(follows[0].actor, identity);
    assert_eq!(follows[0].last_error, None);

    for node in [alice, carol] {
        node.shutdown().await.unwrap();
    }
}

/// Posts found through a directory are here before their author is
/// followed, without their media. The follow's first sync fetches it ahead
/// of time, as it does for the posts it brings.
#[tokio::test(flavor = "multi_thread")]
async fn following_someone_found_fetches_their_found_posts_media_ahead() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(50);
    let directory = start_directory(dir.path(), &net, AN_HOUR).await;
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    let photo = post_photo(&alice, dir.path(), "oak table").await;
    assert!(alice.ask_to_be_listed().await);
    wait_for_entry(&net, &directory, 0, listing_of(alice.identity())).await;

    let carol = config_knowing("Carol", &directory, false);
    let carol = start_with(dir.path(), "Carol", &net, carol).await;
    read_directories(&carol).await;
    let (_, posts) = search(&carol, "oak").await;
    assert_eq!(posts.len(), 1);
    assert_eq!(carol.blob_class(&photo).unwrap(), None);

    let follow = ClientRequest::FollowPerson {
        actor: alice.identity(),
    };
    call(&carol, follow).await;
    call(&carol, ClientRequest::Sync).await;
    assert_eq!(carol.blob_class(&photo).unwrap(), Some(BlobClass::Prefetch));

    for node in [alice, carol, directory] {
        node.shutdown().await.unwrap();
    }
}

/// Posts a photo made by ffmpeg, captioned `text`, and returns its file.
async fn post_photo(node: &Node, dir: &Path, text: &str) -> BlobHash {
    let photo = dir.join(format!("{}.png", node.identity()));
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240",
            "-frames:v",
            "1",
        ])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![photo],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, req).await else {
        panic!("not posted")
    };
    let ClientResponse::Item { item } = call(node, ClientRequest::Show { id }).await else {
        panic!("not shown")
    };
    item.attachments[0].attachment.blob
}

#[tokio::test(flavor = "multi_thread")]
async fn someone_followed_is_left_alone_when_a_directory_lists_them_again() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(41);
    let directory = start_directory(dir.path(), &net, SOON).await;
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    post(&alice, "Oak table number 0").await;
    assert!(alice.ask_to_be_listed().await);
    let first = wait_for_entry(&net, &directory, 0, listing_of(alice.identity())).await;

    let carol = config_knowing("Carol", &directory, false);
    let carol = start_with(dir.path(), "Carol", &net, carol).await;
    read_directories(&carol).await;
    call(
        &carol,
        ClientRequest::FollowPerson {
            actor: alice.identity(),
        },
    )
    .await;
    call(&carol, ClientRequest::Sync).await;

    post(&alice, "Oak table number 1").await;
    wait_for_entry(&net, &directory, first.seq, listing_of(alice.identity())).await;
    read_directories(&carol).await;
    assert_eq!(status(&carol).await.found_people, 0);
    let (_, posts) = search(&carol, "number").await;
    assert!(posts.iter().all(|post| post.found_through.is_none()));

    // Nor does its saying she's gone take away what following her brought.
    directory.shutdown().await.unwrap();
    for node in [alice, carol] {
        node.shutdown().await.unwrap();
    }
}

/// A directory's copy may be stale or wrong. Following asks their own node, and
/// the node named has to speak for them.
#[tokio::test(flavor = "multi_thread")]
async fn following_someone_found_asks_their_own_node_first() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(42);
    let (carol, lists, _router) = start_with_fake(dir.path(), &net, sim(&net)).await;
    let bob = Config::local_test("Bob");
    let bob = start_with(dir.path(), "Bob", &net, bob).await;
    post(&bob, "Bob's own post").await;

    // Someone real says Bob's node is theirs. It's signed; it just isn't so.
    let key = SecretKey::generate();
    let person = Person {
        actor: key.public(),
        name: "Mallory".into(),
        summary: None,
        discoverable: true,
        nodes: vec![bob.node_id()],
        media_endpoints: vec![],
        published: at(-60),
    };
    let profile = SignedObject::sign(&key, &Payload::Person(person)).unwrap();
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![person_entry(1, profile, vec![])];
        lists.latest = 1;
    }
    read_directories(&carol).await;
    call(
        &carol,
        ClientRequest::FollowPerson {
            actor: key.public(),
        },
    )
    .await;
    call(&carol, ClientRequest::Sync).await;
    assert!(feed(&carol).await.is_empty());
    let ClientResponse::Following { follows } = call(&carol, ClientRequest::Following).await else {
        panic!("no follows");
    };
    let why = follows[0].last_error.as_deref().unwrap_or("no error");
    assert!(why.contains("different identity"), "{why}");

    // A person this node holds no profile of can't be followed this way.
    let actor = SecretKey::generate().public();
    let why = match carol.handle(ClientRequest::FollowPerson { actor }).await {
        ClientResponse::Error { message } => message,
        other => panic!("unexpected {other:?}"),
    };
    assert!(why.contains("holds no profile"), "{why}");
    // Nor oneself.
    let actor = carol.identity();
    let why = match carol.handle(ClientRequest::FollowPerson { actor }).await {
        ClientResponse::Error { message } => message,
        other => panic!("unexpected {other:?}"),
    };
    assert!(why.contains("can't follow yourself"), "{why}");

    for node in [bob, carol] {
        node.shutdown().await.unwrap();
    }
}

/// The leak test of ADR 0015, with the directory added: nothing a directory
/// holds or gives out, and nothing a node logs, ties a machine name to a person.
#[tokio::test(flavor = "multi_thread")]
async fn being_listed_ties_a_machine_to_nobody() {
    capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(43);
    let directory = start_directory(dir.path(), &net, SOON).await;
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    let her_machine = alice.machine_id().unwrap();
    post(&alice, "A table of green oak").await;
    assert!(alice.ask_to_be_listed().await);
    read_directories(&alice).await;
    let on_the_map = |entry: &ListingEntry| match &entry.item {
        ListingItem::Machine { record } => Some(record.verify().unwrap().machine),
        _ => None,
    };
    assert_eq!(
        wait_for_entry(&net, &directory, 0, on_the_map).await,
        her_machine
    );
    wait_for_entry(&net, &directory, 0, listing_of(alice.identity())).await;

    // She reads her own listing, and has better than a directory's copy of it.
    assert_eq!(read_directories(&alice).await, 0);

    // What she sent under her public ID doesn't name her machine.
    let profile = signed_profile_of(&alice, &net).await;
    let json = format!(
        "{} {}",
        serde_json::to_string(&profile).unwrap(),
        String::from_utf8_lossy(profile.payload_bytes())
    );
    assert!(!json.contains(&her_machine.to_string()), "{json}");

    // A reader sees the two in separate entries, and nothing joining them.
    let (entries, _) = read_lists(&net, &directory, 0).await;
    assert!(entries.len() >= 2, "{entries:?}");
    for entry in &entries {
        match &entry.item {
            ListingItem::Machine { record } => {
                let info = record.verify().unwrap();
                assert_ne!(info.machine, alice.node_id());
                let shown = serde_json::to_string(&info).unwrap();
                assert!(!shown.contains(&alice.identity().to_string()), "{shown}");
                assert!(!shown.contains(&alice.node_id().to_string()), "{shown}");
            }
            ListingItem::Person { profile, posts, .. } => {
                let person = person_in(profile);
                assert!(!person.nodes.contains(&her_machine));
                let shown: Vec<_> = std::iter::once(profile)
                    .chain(posts)
                    .map(|object| String::from_utf8_lossy(object.payload_bytes()).to_string())
                    .collect();
                assert!(
                    shown.iter().all(|s| !s.contains(&her_machine.to_string())),
                    "{shown:?}"
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // Neither did reading a directory and being listed by it put it in a log,
    // nor the directory's own.
    let its_machine = directory.machine_id().unwrap();
    for node in [alice, directory] {
        node.shutdown().await.unwrap();
    }
    assert_logs_dont_name(&her_machine);
    assert_logs_dont_name(&its_machine);
}

/// Fills a directory's home before it starts, as if it had been running a while.
fn fill_directory(root: &Path, listed: usize, entries: usize) {
    let home = Home::new(root.join("directory"));
    let db = rusqlite::Connection::open(home.db()).unwrap();
    db.execute_batch("BEGIN").unwrap();
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    for _ in 0..listed {
        db.execute(
            "INSERT INTO listed (actor, node, asked_ms, tried_ms) VALUES (?1, ?1, ?2, ?3)",
            rusqlite::params![
                SecretKey::generate().public().to_string(),
                // Asked long enough ago not to count as new, and not silent yet.
                now - 2 * 60 * 60 * 1000,
                now
            ],
        )
        .unwrap();
    }
    for i in 0..entries {
        let item = ListingItem::Gone {
            actor: SecretKey::generate().public(),
        };
        db.execute(
            "INSERT INTO listing (key, item) VALUES (?1, ?2)",
            rusqlite::params![format!("filler:{i}"), serde_json::to_string(&item).unwrap()],
        )
        .unwrap();
    }
    db.execute_batch("COMMIT").unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_directory_refuses_more_and_a_page_has_a_limit() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(44);
    let home = dir.path().join("directory");
    let mut config = Config::local_test("The directory");
    config.directory.serve = true;
    Node::init(&home, &config).unwrap();
    fill_directory(dir.path(), 5_000, 150);
    let directory = Node::spawn_sim(&home, sim(&net)).await.unwrap();
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    assert!(!alice.ask_to_be_listed().await);

    // However many it's asked for.
    let request = MachineRequest::Listing {
        since: 0,
        limit: 1_000_000,
    };
    let reply = ask_machine(&net, directory.machine_id().unwrap(), &request).await;
    let MachineResponse::Listing { entries, latest } = reply else {
        panic!("unexpected {reply:?}");
    };
    assert_eq!(entries.len(), 100);
    assert_eq!(latest, 150);

    for node in [alice, directory] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn asking_again_doesnt_make_a_directory_read_any_sooner() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(45);
    let directory = start_directory(dir.path(), &net, AN_HOUR).await;
    let alice = config_knowing("Alice", &directory, true);
    let alice = start_with(dir.path(), "Alice", &net, alice).await;
    post(&alice, "The first").await;
    assert!(alice.ask_to_be_listed().await);
    let first = wait_for_entry(&net, &directory, 0, listing_of(alice.identity())).await;

    post(&alice, "The second").await;
    assert!(alice.ask_to_be_listed().await);
    tokio::time::sleep(SOON * 5).await;
    let (entries, _) = read_lists(&net, &directory, first.seq).await;
    assert!(entries.is_empty(), "{entries:?}");

    for node in [alice, directory] {
        node.shutdown().await.unwrap();
    }
}

/// What a listed node run by a test says of itself.
#[derive(Clone)]
struct FakeNode {
    profile: SignedObject,
    outbox: Vec<SignedObject>,
}

impl std::fmt::Debug for FakeNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FakeNode")
    }
}

impl ProtocolHandler for FakeNode {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let request: SocialRequest = read_frame(&mut recv).await.unwrap();
            let reply = match request {
                SocialRequest::GetProfile => SocialResponse::Profile {
                    profile: self.profile.clone(),
                },
                SocialRequest::GetOutbox { limit: 0, .. } => SocialResponse::Outbox {
                    entries: vec![],
                    referenced: vec![],
                    latest: self.outbox.len() as u64,
                },
                SocialRequest::GetOutbox { .. } => SocialResponse::Outbox {
                    entries: self
                        .outbox
                        .iter()
                        .enumerate()
                        .map(|(i, object)| OutboxEntry {
                            seq: i as u64 + 1,
                            object: object.clone(),
                        })
                        .collect(),
                    referenced: vec![],
                    latest: self.outbox.len() as u64,
                },
                _ => SocialResponse::Ok,
            };
            write_frame(&mut send, &reply).await.unwrap();
            send.finish().unwrap();
        }
        Ok(())
    }
}

/// Signed by `key`, naming `node` as theirs.
fn profile_naming(key: &SecretKey, node: EndpointId, discoverable: bool) -> SignedObject {
    let person = Person {
        actor: key.public(),
        name: "Someone".into(),
        summary: None,
        discoverable,
        nodes: vec![node],
        media_endpoints: vec![],
        published: at(-60),
    };
    SignedObject::sign(key, &Payload::Person(person)).unwrap()
}

/// Starts a listed node run by a test, asks `directory` to list it, and returns
/// when the directory has read it, or tried.
async fn listed_fake(
    net: &SimNet,
    directory: &Node,
    key: &SecretKey,
    answer: impl FnOnce(EndpointId) -> FakeNode,
) -> Router {
    let (endpoint, node_key) = stranger_with_key(net).await;
    let node = node_key.public();
    let conn = endpoint
        .connect(sim_addr(directory.node_id()), SOCIAL_ALPN)
        .await
        .unwrap();
    let router = Router::builder(endpoint.clone())
        .accept(SOCIAL_ALPN, answer(node))
        .spawn();
    let request = SocialRequest::AskListing {
        profile: profile_naming(key, node, true),
    };
    let reply = ask_social(&conn, &request).await;
    assert!(matches!(reply, SocialResponse::Ok), "{reply:?}");
    router
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listed_node_can_speak_only_for_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(46);
    let directory = start_directory(dir.path(), &net, SOON).await;

    // Its outbox has someone else's posts among its own, and something that
    // isn't a post.
    let (key, other) = (SecretKey::generate(), SecretKey::generate());
    let reshare = Payload::Announce(Announce {
        actor: key.public(),
        object: signed_note(&other, "x").id(),
        object_actor: other.public(),
        published: at(-60),
        hosting: false,
    });
    let outbox = vec![
        signed_note(&key, "Its own post"),
        signed_note(&other, "Someone else's post"),
        SignedObject::sign(&key, &reshare).unwrap(),
    ];
    let profile_key = key.clone();
    let _honest = listed_fake(&net, &directory, &key, move |node| FakeNode {
        profile: profile_naming(&profile_key, node, true),
        outbox,
    })
    .await;
    let listing = wait_for_entry(&net, &directory, 0, listing_of(key.public())).await;
    assert_eq!(listing.texts, ["Its own post"]);

    // Too many entries for what was asked: nothing is taken from it.
    let flooded = SecretKey::generate();
    let outbox = (0..21)
        .map(|i| signed_note(&flooded, &format!("Flood {i}")))
        .collect();
    let profile_key = flooded.clone();
    let _flood = listed_fake(&net, &directory, &flooded, move |node| FakeNode {
        profile: profile_naming(&profile_key, node, true),
        outbox,
    })
    .await;

    // One that asks with its own profile, and then gives another's.
    let liar = SecretKey::generate();
    let victim = other.clone();
    let _liar = listed_fake(&net, &directory, &liar, move |node| FakeNode {
        profile: profile_naming(&victim, node, true),
        outbox: vec![],
    })
    .await;

    // One whose profile stops naming the node it's read through.
    let moved = SecretKey::generate();
    let somewhere_else = SecretKey::generate().public();
    let profile_key = moved.clone();
    let _moved = listed_fake(&net, &directory, &moved, move |_| FakeNode {
        profile: profile_naming(&profile_key, somewhere_else, true),
        outbox: vec![],
    })
    .await;
    wait_for_entry(&net, &directory, 0, gone(moved.public())).await;

    // By now it has read them all, and listed none of those.
    tokio::time::sleep(SOON * 5).await;
    let (entries, _) = read_lists(&net, &directory, 0).await;
    for actor in [flooded.public(), liar.public(), other.public()] {
        let theirs = listing_of(actor);
        assert!(entries.iter().all(|e| theirs(e).is_none()), "{entries:?}");
    }
    directory.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_directory_greets_a_machine_back_once_and_a_few_an_hour() {
    /// A machine that counts how often it's greeted.
    #[derive(Clone)]
    struct Counting {
        key: SecretKey,
        greeted: Arc<Mutex<u32>>,
    }
    impl std::fmt::Debug for Counting {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("Counting")
        }
    }
    impl ProtocolHandler for Counting {
        async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
            if let Ok((mut send, mut recv)) = conn.accept_bi().await {
                let _: MachineRequest = read_frame(&mut recv).await.unwrap();
                *self.greeted.lock().unwrap() += 1;
                let record =
                    MachineRecord::new(&self.key, vec![], osp_proto::now_rfc3339()).unwrap();
                write_frame(&mut send, &MachineResponse::Hello { record })
                    .await
                    .unwrap();
                send.finish().unwrap();
            }
            conn.closed().await;
            Ok(())
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(47);
    let mut config = Config::local_test("The directory");
    config.directory.serve = true;
    let settings = SimSettings {
        greets_back_per_hour: 3,
        ..sim(&net)
    };
    let directory = start_sim(dir.path(), "directory", config, settings).await;
    let its_machine = directory.machine_id().unwrap();

    let mut machines = Vec::new();
    for _ in 0..4 {
        let (endpoint, key) = stranger_with_key(&net).await;
        let greeted = Arc::new(Mutex::new(0));
        let counting = Counting {
            key: key.clone(),
            greeted: greeted.clone(),
        };
        let router = Router::builder(endpoint.clone())
            .accept(MACHINE_ALPN, counting)
            .spawn();
        machines.push((endpoint, key, greeted, router));
    }
    let say_hello = |endpoint: Endpoint, key: SecretKey| async move {
        let conn = endpoint
            .connect(sim_addr(its_machine), MACHINE_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let record = MachineRecord::new(&key, vec![], osp_proto::now_rfc3339()).unwrap();
        write_frame(&mut send, &MachineRequest::Hello { record })
            .await
            .unwrap();
        send.finish().unwrap();
        let _: MachineResponse = read_frame(&mut recv).await.unwrap();
    };

    // One whose answer doesn't check out stays off the map, and however often
    // it says hello, it's dialled once.
    let (endpoint, key) = stranger_with_key(&net).await;
    let greeted = Arc::new(Mutex::new(0));
    let false_one = Counting {
        key: SecretKey::generate(),
        greeted: greeted.clone(),
    };
    let _router = Router::builder(endpoint.clone())
        .accept(MACHINE_ALPN, false_one)
        .spawn();
    for _ in 0..3 {
        say_hello(endpoint.clone(), key.clone()).await;
        tokio::time::sleep(SOON).await;
    }
    assert_eq!(*greeted.lock().unwrap(), 1);
    assert_eq!(status(&directory).await.directory.unwrap().machines, 0);

    // However often an honest one says hello, it's greeted back once.
    let (endpoint, key, greeted, _) = &machines[0];
    for _ in 0..3 {
        say_hello(endpoint.clone(), key.clone()).await;
        tokio::time::sleep(SOON).await;
    }
    assert_eq!(*greeted.lock().unwrap(), 1);
    assert_eq!(status(&directory).await.directory.unwrap().machines, 1);

    // And only so many are greeted back, whoever they say they are.
    for (endpoint, key, _, _) in &machines[1..] {
        say_hello(endpoint.clone(), key.clone()).await;
        tokio::time::sleep(SOON).await;
    }
    assert_eq!(status(&directory).await.directory.unwrap().machines, 2);
    directory.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_keeps_only_so_many_from_directories_and_refuses_too_large_a_reply() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(48);
    let settings = SimSettings {
        max_found_people: 2,
        ..sim(&net)
    };
    let (carol, lists, _router) = start_with_fake(dir.path(), &net, settings).await;
    let entry = |seq, name: &str| {
        person_entry(
            seq,
            signed_person(&SecretKey::generate(), name, "words", true),
            vec![],
        )
    };
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![entry(1, "One"), entry(2, "Two"), entry(3, "Three")];
        lists.latest = 3;
        lists.page = 1;
    }
    assert_eq!(read_directories(&carol).await, 2);
    assert!(found_people(&carol, "one").await.is_empty());
    assert_eq!(found_people(&carol, "three").await, ["Three"]);

    // More entries in a page than it asked for.
    let many: Vec<_> = (4..=104).map(|seq| entry(seq, "Many")).collect();
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = many.clone();
        lists.latest = 104;
        lists.page = 200;
        lists.all_at_once = true;
    }
    let why = match carol.handle(ClientRequest::ReadDirectories).await {
        ClientResponse::Error { message } => message,
        other => panic!("unexpected {other:?}"),
    };
    assert!(why.contains("at most 100"), "{why}");

    // A reply larger than any honest page isn't read at all.
    let big = |seq| {
        person_entry(
            seq,
            signed_person(&SecretKey::generate(), "Big", "words", true),
            vec![signed_note(&SecretKey::generate(), &"\u{1}".repeat(10_000)); 5],
        )
    };
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![big(4), big(5)];
        lists.latest = 5;
        lists.all_at_once = false;
        lists.page = 200;
    }
    let reply = carol.handle(ClientRequest::ReadDirectories).await;
    assert!(matches!(reply, ClientResponse::Error { .. }), "{reply:?}");
    carol.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_the_owner_reshares_stays_when_its_author_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(49);
    let (carol, lists, _router) = start_with_fake(dir.path(), &net, sim(&net)).await;
    let key = SecretKey::generate();
    let (kept, dropped) = (
        signed_note(&key, "Oak to keep"),
        signed_note(&key, "Oak to lose"),
    );
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![person_entry(
            1,
            signed_person(&key, "Alice", "words", true),
            vec![kept.clone(), dropped],
        )];
        lists.latest = 1;
    }
    read_directories(&carol).await;
    let req = ClientRequest::Reshare {
        id: kept.id(),
        host: false,
    };
    call(&carol, req).await;
    {
        let mut lists = lists.lock().unwrap();
        lists.entries = vec![ListingEntry {
            seq: 2,
            item: ListingItem::Gone {
                actor: key.public(),
            },
        }];
        lists.latest = 2;
    }
    assert_eq!(read_directories(&carol).await, 0);
    assert_eq!(found_posts(&carol, "oak").await, ["Oak to keep"]);
    // And it's in the feed as what it is: her reshare.
    let feed = feed(&carol).await;
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].content, "Oak to keep");
    assert!(feed[0].reshared_by.is_some());
    carol.shutdown().await.unwrap();
}

/// Waits until a node's Tor is ready and has published its onion services.
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

/// The gate over real Tor: a node with no follows finds a stranger through a
/// directory and follows them, on nothing but their IDs. Needs Tor:
/// `OSP_TOR=/usr/bin/tor cargo test -p osp-node --test directory -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs tor and the Tor network; takes minutes"]
async fn a_stranger_is_found_and_followed_over_tor() {
    capture_logs();
    let binary = std::env::var("OSP_TOR").unwrap_or_else(|_| "tor".into());
    let dir = tempfile::tempdir().unwrap();
    let on_tor = |mut config: Config| {
        config.tor.enabled = true;
        config.tor.binary = binary.clone();
        config.endpoints.enabled = true;
        config
    };
    let start = |name: &str, config: Config| {
        let home = dir.path().join(name);
        async move {
            Node::init(&home, &config).unwrap();
            Node::spawn(&home).await.unwrap()
        }
    };
    let started = Instant::now();
    let mut config = on_tor(Config::local_test("The directory"));
    config.directory.serve = true;
    let directory = start("directory", config).await;
    let mut alice = on_tor(config_knowing("Alice", &directory, true));
    alice.summary = Some("Builds oak tables".into());
    let alice = start("alice", alice).await;
    let carol = on_tor(config_knowing("Carol", &directory, false));
    let carol = start("carol", carol).await;
    for node in [&directory, &alice, &carol] {
        wait_for_tor(node).await;
    }
    println!(
        "Tor up on all three: {:.1} s",
        started.elapsed().as_secs_f64()
    );
    post(&alice, "A table of green walnut").await;

    // A first contact takes as long as it takes, and asking again is harmless.
    let started = Instant::now();
    while !alice.ask_to_be_listed().await {
        assert!(started.elapsed() < Duration::from_secs(600), "never asked");
    }
    println!(
        "asked to be listed: {:.1} s",
        started.elapsed().as_secs_f64()
    );

    let started = Instant::now();
    loop {
        if let ClientResponse::DirectoriesRead { found: 1 } =
            carol.handle(ClientRequest::ReadDirectories).await
        {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(600), "never found");
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    println!(
        "found through the directory: {:.1} s",
        started.elapsed().as_secs_f64()
    );

    // With the directory and Alice both away, Carol can still find her.
    let (people, _) = search(&carol, "oak").await;
    assert_eq!(people.len(), 1);
    assert_eq!(people[0].summary.as_deref(), Some("Builds oak tables"));
    assert!(people[0].found_through.is_some());
    assert_eq!(found_posts(&carol, "walnut").await.len(), 1);
    assert!(feed(&carol).await.is_empty());

    let started = Instant::now();
    let actor = people[0].actor;
    call(&carol, ClientRequest::FollowPerson { actor }).await;
    loop {
        let reply = carol.handle(ClientRequest::Sync).await;
        if !feed(&carol).await.is_empty() {
            break;
        }
        println!("not yet: {reply:?}");
        assert!(
            started.elapsed() < Duration::from_secs(600),
            "never followed"
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    println!("followed: {:.1} s", started.elapsed().as_secs_f64());
    assert_eq!(status(&carol).await.found_people, 0);
    assert_eq!(status(&carol).await.follows, 1);

    // The directory greets back the machine that read it, over Tor.
    let started = Instant::now();
    let its_status = loop {
        let shown = status(&directory).await.directory.unwrap();
        if shown.machines > 0 {
            break shown;
        }
        assert!(started.elapsed() < Duration::from_secs(600), "never mapped");
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    println!(
        "the directory holds {} people and {} machines: {:.1} s",
        its_status.people,
        its_status.machines,
        started.elapsed().as_secs_f64()
    );

    let machines = [
        directory.machine_id().unwrap(),
        alice.machine_id().unwrap(),
        carol.machine_id().unwrap(),
    ];
    for node in [directory, alice, carol] {
        node.shutdown().await.unwrap();
    }
    for machine in &machines {
        assert_logs_dont_name(machine);
    }
}
