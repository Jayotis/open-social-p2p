//! ADR 0044 step 5: a node with a relay token passes its relays and the
//! token to its phone in the pairing code, and the phone reaches it through
//! them, also after the app restarts.
//!
//! The relay runs on loopback and lets in only those with the token. The
//! node is relay-only, so the relay is the only way to it.

use std::{
    net::Ipv4Addr,
    sync::{Arc, Mutex},
};

use iroh::EndpointId;
use iroh_relay::server::{
    Access, AccessControl, ClientRequest as RelayRequest, RelayConfig, Server, ServerConfig,
};
use osp_client::{Client, ClientOptions};
use osp_node::{Config, Node, RelayChoice};
use osp_proto::{
    PairingTicket,
    rpc::{ClientRequest, ClientResponse},
};

const TOKEN: &str = "the project's token";

/// Lets in only those that bring the token, and records the rest.
#[derive(Debug, Clone, Default)]
struct TokenGate(Arc<Mutex<Vec<EndpointId>>>);

impl AccessControl for TokenGate {
    async fn on_connect(&self, request: &RelayRequest) -> Access {
        if request.auth_token().as_deref() == Some(TOKEN) {
            Access::Allow
        } else {
            self.0.lock().unwrap().push(request.endpoint_id());
            Access::Deny { reason: None }
        }
    }
}

async fn new_pairing(node: &Node) -> String {
    match node.handle(ClientRequest::NewPairing).await {
        ClientResponse::Pairing { ticket, .. } => ticket,
        other => panic!("unexpected {other:?}"),
    }
}

/// Relays on, n0's lookups off.
fn phone_options() -> ClientOptions {
    ClientOptions {
        address_lookup: false,
        ..ClientOptions::default()
    }
}

async fn assert_reaches_node(phone: &Client) {
    match phone.request(&ClientRequest::Status).await.unwrap() {
        ClientResponse::Status(status) => assert_eq!(status.name, "Kim"),
        other => panic!("unexpected {other:?}"),
    }
    let path = phone.connection_info().await.unwrap().path;
    assert_eq!(path, "relay", "the node is relay-only");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_uses_its_nodes_relays_and_token_from_the_pairing_code() {
    let refused = TokenGate::default();
    let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.access = Arc::new(refused.clone());
    let mut server = ServerConfig::default();
    server.relay = Some(relay);
    let _relay = Server::spawn(server).await.unwrap();
    let url = format!("http://{}", _relay.http_addr().unwrap());

    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::local_test("Kim");
    config.network.relay = RelayChoice::Custom(vec![url.clone()]);
    config.network.relay_token = Some(TOKEN.into());
    config.network.relay_only = true;
    config.network.bind = None;
    let home = dir.path().join("kim");
    Node::init(&home, &config).unwrap();
    let node = Node::spawn(&home).await.unwrap();

    let ticket = new_pairing(&node).await;
    assert!(ticket.starts_with("OSPPAIR2"), "{ticket}");
    let parsed: PairingTicket = ticket.parse().unwrap();
    let relays = parsed.relays.expect("the ticket carries the relays");
    assert_eq!(relays.urls, vec![url]);
    assert_eq!(relays.token, TOKEN);
    assert!(!relays.fallback, "the node has no fallback");

    let phone_dir = dir.path().join("phone");
    let phone = Client::open(&phone_dir, phone_options()).await.unwrap();
    let pairing = phone.pair(&ticket, "Kim's phone").await.unwrap();
    assert_eq!(
        pairing.relays.as_ref().map(|r| r.token.as_str()),
        Some(TOKEN)
    );
    assert_reaches_node(&phone).await;

    // After a restart the phone starts on the node's relays, from pairing.json.
    let device = phone.device_id();
    phone.shutdown().await;
    drop(phone);
    let phone = Client::open(&phone_dir, phone_options()).await.unwrap();
    assert_eq!(phone.device_id(), device);
    assert_reaches_node(&phone).await;

    assert!(
        !refused.0.lock().unwrap().contains(&device),
        "the phone always brought the token"
    );
    phone.shutdown().await;
}

/// A node without a token still gives out `OSPPAIR1`, so an older app can
/// pair with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_without_a_token_gives_out_the_old_pairing_code() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("kim");
    Node::init(&home, &Config::local_test("Kim")).unwrap();
    let node = Node::spawn(&home).await.unwrap();
    let ticket = new_pairing(&node).await;
    assert!(ticket.starts_with("OSPPAIR1"), "{ticket}");
    assert_eq!(ticket.parse::<PairingTicket>().unwrap().relays, None);
}
