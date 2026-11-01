//! The node's own Tor (ADR 0014, build step 1). Without Tor installed, a
//! node with it turned on still starts and says why Tor isn't running.
//! With Tor (an ignored test), two nodes reach each other through it.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use iroh::{EndpointAddr, SecretKey};
use osp_node::{Config, Node, RelayChoice};
use osp_proto::{
    FollowTicket,
    rpc::{ClientRequest, ClientResponse, StatusInfo},
};

async fn start(root: &Path, config: Config) -> Node {
    let home = root.join(config.name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn(&home).await.unwrap()
}

async fn status(node: &Node) -> StatusInfo {
    match node.handle(ClientRequest::Status).await {
        ClientResponse::Status(status) => status,
        other => panic!("unexpected reply: {other:?}"),
    }
}

/// The error a request fails with.
async fn refused(node: &Node, req: ClientRequest) -> String {
    match node.handle(req).await {
        ClientResponse::Error { message } => message,
        other => panic!("expected an error, got {other:?}"),
    }
}

/// Waits until the node's status passes `check`.
async fn wait_for(node: &Node, limit: Duration, check: impl Fn(&StatusInfo) -> bool) -> StatusInfo {
    let started = Instant::now();
    loop {
        let s = status(node).await;
        if check(&s) {
            return s;
        }
        assert!(started.elapsed() < limit, "status stayed {:?}", s.tor);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn tor_is_off_unless_turned_on() {
    let dir = tempfile::tempdir().unwrap();
    let a = start(dir.path(), Config::local_test("A")).await;
    assert!(status(&a).await.tor.is_none());
    let node = a.node_id();
    let why = refused(&a, ClientRequest::TorPing { node }).await;
    assert!(why.contains("Tor is off"), "{why}");
    a.shutdown().await.unwrap();
}

/// Media and phones can't go over Tor, so without other endpoints for them a
/// node with Tor on would be cut off from both.
#[tokio::test]
async fn tor_takes_the_other_endpoints_too() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::local_test("A");
    config.tor.enabled = true;
    let home = dir.path().join("a");
    Node::init(&home, &config).unwrap();
    let Err(why) = Node::spawn(&home).await else {
        panic!("it started");
    };
    assert!(why.to_string().contains("tor needs endpoints"), "{why:#}");
}

#[tokio::test]
async fn a_node_whose_tor_wont_start_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::local_test("A");
    config.tor.enabled = true;
    config.tor.binary = "/nonexistent/tor".to_string();
    config.endpoints.enabled = true;
    let a = start(dir.path(), config).await;
    let s = wait_for(&a, Duration::from_secs(10), |s| {
        s.tor.as_ref().is_some_and(|t| t.state.starts_with("down"))
    })
    .await;
    assert!(
        s.problems
            .iter()
            .any(|p| p.starts_with("Tor isn't running") && p.contains("isn't installed")),
        "{:?}",
        s.problems
    );
    // Said at once, not after a wait to connect.
    let node = a.node_id();
    let why = refused(&a, ClientRequest::TorPing { node }).await;
    assert!(why.contains("Tor isn't ready"), "{why}");
    // Messages wait for Tor, and go nowhere else, however they're asked for.
    let ticket = FollowTicket {
        actor: SecretKey::generate().public(),
        node: EndpointAddr::new(SecretKey::generate().public()),
    };
    let follow = ClientRequest::Follow {
        ticket: ticket.to_string(),
    };
    assert!(matches!(a.handle(follow).await, ClientResponse::Ok));
    a.handle(ClientRequest::Sync).await;
    let ClientResponse::Following { follows } = a.handle(ClientRequest::Following).await else {
        panic!("no follows");
    };
    let why = follows[0].last_error.as_deref().unwrap_or_default();
    assert!(why.contains("Tor isn't ready"), "{why}");
    a.shutdown().await.unwrap();
}

/// Needs Tor:
/// `OSP_TOR=/usr/bin/tor cargo test -p osp-node --test tor -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs tor and the Tor network; takes a minute or more"]
async fn nodes_reach_each_other_over_tor() {
    let binary = std::env::var("OSP_TOR").unwrap_or_else(|_| "tor".into());
    let dir = tempfile::tempdir().unwrap();
    let mut nodes = Vec::new();
    for name in ["A", "B"] {
        let mut config = Config::local_test(name);
        config.tor.enabled = true;
        config.tor.binary = binary.clone();
        config.endpoints.enabled = true;
        nodes.push(start(dir.path(), config).await);
    }
    let (a, b) = (&nodes[0], &nodes[1]);
    for node in [a, b] {
        wait_for(node, Duration::from_secs(300), |s| {
            s.tor.as_ref().is_some_and(|t| t.ready && t.published)
        })
        .await;
    }

    let started = Instant::now();
    let node = b.node_id();
    let first = loop {
        match a.handle(ClientRequest::TorPing { node }).await {
            ClientResponse::TorPinged {
                connect_ms,
                round_trip_ms,
            } => break (connect_ms, round_trip_ms),
            other => println!("not yet: {other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "never reached"
        );
    };
    println!(
        "first ping: connected in {:?} ms, asked in {} ms, {:.1} s in all",
        first.0,
        first.1,
        started.elapsed().as_secs_f64()
    );
    assert!(first.0.is_some());
    match a.handle(ClientRequest::TorPing { node }).await {
        ClientResponse::TorPinged {
            connect_ms: None,
            round_trip_ms,
        } => println!("second ping, same connection: asked in {round_trip_ms} ms"),
        other => panic!("unexpected reply: {other:?}"),
    }
    let s = status(b).await.tor.unwrap();
    assert_eq!(s.streams, 1, "one stream, both ways");
    for node in nodes {
        node.shutdown().await.unwrap();
    }
}

/// Two nodes as they're meant to run: messages over real Tor and nothing else,
/// media through n0's public relays. Needs Tor and the internet:
/// `OSP_TOR=/usr/bin/tor cargo test -p osp-node --test tor -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs tor, the Tor network and public relays; takes minutes"]
async fn following_and_fetching_with_messages_over_tor_and_media_through_relays() {
    let binary = std::env::var("OSP_TOR").unwrap_or_else(|_| "tor".into());
    let dir = tempfile::tempdir().unwrap();
    let mut nodes = Vec::new();
    for name in ["Ada", "Bea"] {
        let mut config = Config::local_test(name);
        config.network.relay = RelayChoice::Default;
        config.network.bind = None;
        config.tor.enabled = true;
        config.tor.binary = binary.clone();
        config.endpoints.enabled = true;
        nodes.push(start(dir.path(), config).await);
    }
    let (ada, bea) = (&nodes[0], &nodes[1]);
    for node in [ada, bea] {
        wait_for(node, Duration::from_secs(300), |s| {
            s.tor.as_ref().is_some_and(|t| t.ready && t.published)
        })
        .await;
    }

    let photo = dir.path().join("photo.png");
    let made = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc=size=640x360"])
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(made.success());
    let post = ClientRequest::Post {
        content: "a photo".into(),
        attachments: vec![photo],
        uploads: vec![],
        upload: None,
    };
    assert!(matches!(
        ada.handle(post).await,
        ClientResponse::Posted { .. }
    ));
    let ClientResponse::Feed { items } = ada.handle(ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    let blob = items[0].attachments[0].attachment.blob;

    // A follow code gives only the node's ID: its address comes from that.
    let ClientResponse::Ticket { ticket } = ada.handle(ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    let parsed: FollowTicket = ticket.parse().unwrap();
    assert_eq!(parsed.node.id, ada.node_id());
    assert!(parsed.node.addrs.is_empty(), "{:?}", parsed.node);

    let started = Instant::now();
    let follow = ClientRequest::Follow { ticket };
    assert!(matches!(bea.handle(follow).await, ClientResponse::Ok));
    loop {
        if bea.blob_class(&blob).unwrap().is_some() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(420),
            "the photo never came"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    println!(
        "from following to holding the photo: {:.1} s",
        started.elapsed().as_secs_f64()
    );

    // Each node's connections are over Tor for messages, and through a relay
    // for media. Nothing goes direct.
    let ClientResponse::Peers { peers } = bea.handle(ClientRequest::Peers).await else {
        panic!("no peers");
    };
    let record = peers.iter().find(|p| p.node == ada.node_id()).unwrap();
    assert!(record.counts.received_bytes > 0.0, "{record:?}");
    assert!(record.counts.requests > 0.0, "{record:?}");
    for node in [ada, bea] {
        let s = status(node).await;
        println!("{}: {:?}", s.name, s.peers);
        assert!(s.peers.iter().all(|p| p.path != "direct"), "{:?}", s.peers);
    }
    let s = status(bea).await;
    assert!(s.peers.iter().any(|p| p.path == "tor"), "{:?}", s.peers);
    assert!(s.peers.iter().any(|p| p.path == "relay"), "{:?}", s.peers);
    for node in nodes {
        node.shutdown().await.unwrap();
    }
}
