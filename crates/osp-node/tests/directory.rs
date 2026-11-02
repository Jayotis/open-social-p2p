//! Directories (ADR 0015, build step 2): who may be listed, what a directory
//! holds, and a node finding a stranger through one with no word leaving it.
//!
//! These run on the simulated network, which has no Tor.

use std::path::Path;

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr,
    endpoint::{Connection, presets},
};
use osp_node::{
    Config, Node, SimSettings,
    simnet::{self, Link, SimNet},
};
use osp_proto::{
    EarlierKeys, Payload, Person, SOCIAL_ALPN,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse},
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
    let home = root.join(name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn_sim(&home, sim(net)).await.unwrap()
}

/// Someone on the network with nothing to do with any node here.
async fn stranger(net: &SimNet) -> Endpoint {
    let key = SecretKey::generate();
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .add_custom_transport(net.link(Link::default()).transport(key.public()))
        .secret_key(key)
        .bind()
        .await
        .unwrap()
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

/// The profile a node gives anyone who asks, as signed.
async fn profile_of(node: &Node, net: &SimNet) -> Person {
    let me = stranger(net).await;
    let conn = me
        .connect(sim_addr(node.node_id()), SOCIAL_ALPN)
        .await
        .unwrap();
    let reply = ask_social(&conn, &SocialRequest::GetProfile).await;
    let SocialResponse::Profile { profile } = reply else {
        panic!("unexpected {reply:?}");
    };
    let Ok(Payload::Person(person)) = profile.verify() else {
        panic!("not a profile");
    };
    person
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
    config.save(&osp_node::Home::new(&home).config()).unwrap();
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
