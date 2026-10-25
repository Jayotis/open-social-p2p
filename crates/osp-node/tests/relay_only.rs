//! Relay-only mode (ADR 0007, phase 1), with a real relay on loopback: the
//! node binds no IP sockets, other nodes reach it only through the relay,
//! its tickets carry no IP address, and addresses that name other relays
//! aren't used.

use std::{
    net::Ipv4Addr,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh::{EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use iroh_relay::server::{
    Access, AccessControl, ClientRequest as RelayRequest, RelayConfig, Server, ServerConfig,
};
use osp_node::{Config, Node, RelayChoice};
use osp_proto::{
    FollowTicket, PairingTicket,
    rpc::{ClientRequest, ClientResponse, FollowInfo, StatusInfo},
};

/// Records which nodes connect to a relay.
#[derive(Debug, Clone, Default)]
struct Visitors(Arc<Mutex<Vec<EndpointId>>>);

impl AccessControl for Visitors {
    async fn on_connect(&self, request: &RelayRequest) -> Access {
        self.0.lock().unwrap().push(request.endpoint_id());
        Access::Allow
    }
}

impl Visitors {
    fn saw(&self, node: &Node) -> bool {
        self.0.lock().unwrap().contains(&node.node_id())
    }
}

/// A plain-http relay on loopback. Keep the server; dropping it stops it.
async fn local_relay() -> (Server, RelayUrl) {
    let (server, url, _) = recording_relay().await;
    (server, url)
}

/// A local relay that records who connects to it.
async fn recording_relay() -> (Server, RelayUrl, Visitors) {
    let visitors = Visitors::default();
    let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.access = Arc::new(visitors.clone());
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config).await.unwrap();
    let url = format!("http://{}", server.http_addr().unwrap())
        .parse()
        .unwrap();
    (server, url, visitors)
}

/// A node that uses the local relay, relay-only or not.
fn config(name: &str, relay: &RelayUrl, relay_only: bool) -> Config {
    let mut config = Config::local_test(name);
    config.network.relay = RelayChoice::Custom(vec![relay.to_string()]);
    if relay_only {
        config.network.relay_only = true;
        config.network.bind = None;
    }
    config
}

async fn start(root: &Path, config: Config) -> Node {
    let home = root.join(config.name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn(&home).await.unwrap()
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

async fn invite(node: &Node) -> FollowTicket {
    match call(node, ClientRequest::Invite).await {
        ClientResponse::Ticket { ticket } => ticket.parse().unwrap(),
        other => panic!("unexpected {other:?}"),
    }
}

async fn following(node: &Node) -> Vec<FollowInfo> {
    match call(node, ClientRequest::Following).await {
        ClientResponse::Following { follows } => follows,
        other => panic!("unexpected {other:?}"),
    }
}

/// The path `node` uses to reach `peer`, once there's a connection.
async fn path_to(node: &Node, peer: &Node) -> String {
    let peers = status(node).await.peers;
    let info = peers.iter().find(|p| p.node == peer.node_id());
    info.map(|p| p.path.clone()).unwrap_or_default()
}

fn only_relay(addr: &EndpointAddr, url: &RelayUrl) -> bool {
    addr.addrs.iter().eq([&TransportAddr::Relay(url.clone())])
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_only_node_has_no_ip_sockets_and_is_reached_through_its_relay() {
    let (_relay, url) = local_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let pia = start(dir.path(), config("Pia", &url, true)).await;
    let quin = start(dir.path(), config("Quin", &url, false)).await;

    assert!(status(&pia).await.local_addrs.is_empty(), "no IP sockets");
    // What it hands out names the relay and nothing else.
    let ticket = invite(&pia).await;
    assert!(only_relay(&ticket.node, &url), "{:?}", ticket.node);
    let ClientResponse::Pairing {
        ticket: pairing, ..
    } = call(&pia, ClientRequest::NewPairing).await
    else {
        panic!("no pairing ticket");
    };
    let pairing: PairingTicket = pairing.parse().unwrap();
    assert!(only_relay(&pairing.node, &url), "{:?}", pairing.node);

    // A node in direct mode follows it, and gets its posts through the relay.
    call(
        &pia,
        ClientRequest::Post {
            content: "only through the relay".into(),
            attachments: vec![],
            uploads: vec![],
            upload: None,
        },
    )
    .await;
    call(
        &quin,
        ClientRequest::Follow {
            ticket: ticket.to_string(),
        },
    )
    .await;
    let synced = tokio::time::timeout(Duration::from_secs(60), call(&quin, ClientRequest::Sync))
        .await
        .expect("sync finishes");
    assert!(
        matches!(synced, ClientResponse::Synced { .. }),
        "{synced:?}"
    );
    let ClientResponse::Feed { items } = call(&quin, ClientRequest::Feed { limit: 10 }).await
    else {
        panic!("no feed");
    };
    assert!(items.iter().any(|i| i.content == "only through the relay"));
    assert_eq!(path_to(&quin, &pia).await, "relay");
    assert_eq!(path_to(&pia, &quin).await, "relay");

    pia.shutdown().await.unwrap();
    quin.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn relays_named_by_others_are_used_only_if_the_policy_allows() {
    let (_relay, url) = local_relay().await;
    // Another relay, on loopback over plain http: neither node may use it.
    let (_other, other_url, visitors) = recording_relay().await;
    let dir = tempfile::tempdir().unwrap();
    let pia = start(dir.path(), config("Pia", &url, true)).await;
    let quin = start(dir.path(), config("Quin", &url, false)).await;
    // A node that can only be reached through the other relay.
    let tam = start(dir.path(), config("Tam", &other_url, false)).await;
    let ticket = invite(&tam).await;
    assert!(only_relay(&ticket.node, &other_url), "{:?}", ticket.node);

    for (node, why) in [(&pia, "relay-only"), (&quin, "public https")] {
        call(
            node,
            ClientRequest::Follow {
                ticket: ticket.to_string(),
            },
        )
        .await;
        tokio::time::timeout(Duration::from_secs(60), node.handle(ClientRequest::Sync))
            .await
            .expect("sync finishes");
        let error = following(node).await[0]
            .last_error
            .clone()
            .unwrap_or_default();
        assert!(
            error.contains("relays this node won't use")
                && error.contains(other_url.as_str())
                && error.contains(why),
            "{error}"
        );
        assert!(!visitors.saw(node), "never connects to the other relay");
    }
    assert!(visitors.saw(&tam), "the other relay records its visitors");

    pia.shutdown().await.unwrap();
    quin.shutdown().await.unwrap();
    tam.shutdown().await.unwrap();
}

#[tokio::test]
async fn relay_only_needs_relays_and_no_bind_address() {
    let dir = tempfile::tempdir().unwrap();
    let url: RelayUrl = "https://relay.mine.example/".parse().unwrap();

    let mut no_relays = config("Rae", &url, true);
    no_relays.network.relay = RelayChoice::Disabled;
    let mut bound = config("Sol", &url, true);
    bound.network.bind = Some("127.0.0.1:0".parse().unwrap());
    for (config, expected) in [
        (no_relays, "needs relays"),
        (bound, "can't be used together"),
    ] {
        let home = dir.path().join(config.name.to_lowercase());
        Node::init(&home, &config).unwrap();
        let err = Node::spawn(&home).await.err().expect("refused");
        assert!(format!("{err:#}").contains(expected), "{err:#}");
    }
}
