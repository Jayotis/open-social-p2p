//! Directories (ADR 0015, build step 2): who may be listed, what a directory
//! holds, and a node finding a stranger through one with no word leaving it.
//!
//! These run on the simulated network, which has no Tor.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr,
    endpoint::{Connection, presets},
};
use osp_node::{
    Config, Home, KnownDirectory, Node, SimSettings,
    simnet::{self, Link, SimNet},
};
use osp_proto::{
    EarlierKeys, MACHINE_ALPN, ObjectId, Payload, Person, SOCIAL_ALPN, SignedObject,
    framing::{read_frame, write_frame},
    machine::{ListingEntry, ListingItem, MachineRecord, MachineRequest, MachineResponse},
    rpc::{ClientRequest, ClientResponse, DirectoryStatus, StatusInfo},
    social::{SocialRequest, SocialResponse},
};

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
    read_frame(&mut recv).await.unwrap()
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
        ListingItem::Person { profile, posts } if profile.signer() == actor => Some(Listing {
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
async fn nobody_can_list_someone_else() {
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
            ListingItem::Person { profile, posts } => {
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
