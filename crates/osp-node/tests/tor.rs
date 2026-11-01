//! The node's own Tor (ADR 0014, build step 1). Without Tor installed, a
//! node with it turned on still starts and says why Tor isn't running.
//! With Tor (an ignored test), two nodes reach each other through it.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use osp_node::{Config, Node};
use osp_proto::rpc::{ClientRequest, ClientResponse, StatusInfo};

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

#[tokio::test]
async fn a_node_whose_tor_wont_start_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::local_test("A");
    config.tor.enabled = true;
    config.tor.binary = "/nonexistent/tor".to_string();
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
