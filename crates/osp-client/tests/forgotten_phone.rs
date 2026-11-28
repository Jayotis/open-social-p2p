//! A phone its node forgot (`osp unpair`) is told so, in the words the app
//! looks for to offer pairing again, whichever way the node's direct endpoint
//! turns it away (ADR 0014): refused at the first packet through a relay, or
//! closed as not allowed just after the handshake when it came straight to
//! the node's address. A used code is turned down in plain words, and pairing
//! again with a new code then works.
//!
//! The node runs with its media and direct endpoints on, as nodes do, and the
//! phone on loopback.

use std::{net::Ipv4Addr, path::Path};

use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use osp_client::{Client, ClientOptions};
use osp_node::{Config, Node, RelayChoice};
use osp_proto::{
    CODE_TURNED_DOWN, NOT_PAIRED,
    rpc::{ClientRequest, ClientResponse},
};

async fn start(root: &Path, mut config: Config) -> Node {
    config.endpoints.enabled = true;
    let home = root.join("kim");
    Node::init(&home, &config).unwrap();
    Node::spawn(&home).await.unwrap()
}

async fn new_pairing(node: &Node) -> String {
    match node.handle(ClientRequest::NewPairing).await {
        ClientResponse::Pairing { ticket, .. } => ticket,
        other => panic!("unexpected {other:?}"),
    }
}

async fn assert_reaches_node(phone: &Client) {
    match phone.request(&ClientRequest::Status).await.unwrap() {
        ClientResponse::Status(status) => assert_eq!(status.name, "Kim"),
        other => panic!("unexpected {other:?}"),
    }
}

/// Pairs `phone`, has the node forget it, checks what the phone is told,
/// then pairs it again.
async fn forget_and_pair_again(node: &Node, phone: &Client, path: &str) {
    let first = new_pairing(node).await;
    phone.pair(&first, "Kim's phone").await.unwrap();
    assert_reaches_node(phone).await;
    assert_eq!(phone.connection_info().await.unwrap().path, path);

    let resp = node
        .handle(ClientRequest::Unpair {
            device: phone.device_id(),
        })
        .await;
    assert!(matches!(resp, ClientResponse::Ok), "{resp:?}");
    // A request already on its way may see the connection close as
    // unpaired. Each after that dials again and is turned away.
    let _ = phone.request(&ClientRequest::Status).await;
    for _ in 0..2 {
        let err = phone.request(&ClientRequest::Status).await.unwrap_err();
        assert_eq!(err.to_string(), NOT_PAIRED, "{err:#}");
    }
    assert!(phone.pairing().is_some(), "kept until a new one works");

    // The first code was used: the node turns it down, and says so.
    let err = phone.pair(&first, "Kim's phone").await.unwrap_err();
    assert_eq!(err.to_string(), CODE_TURNED_DOWN, "{err:#}");

    phone
        .pair(&new_pairing(node).await, "Kim's phone")
        .await
        .unwrap();
    assert_reaches_node(phone).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forgotten_phone_is_told_so_through_a_relay() {
    let mut relay = ServerConfig::default();
    relay.relay = Some(RelayConfig::new((Ipv4Addr::LOCALHOST, 0)));
    let relay = Server::spawn(relay).await.unwrap();
    let url = format!("http://{}", relay.http_addr().unwrap());

    // Relay-only, with a token so the pairing code carries the relay: the
    // relay is the only way to the node.
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::local_test("Kim");
    config.network.relay = RelayChoice::Custom(vec![url]);
    config.network.relay_token = Some("a token".into());
    config.network.relay_only = true;
    config.network.bind = None;
    let node = start(dir.path(), config).await;

    let options = ClientOptions {
        address_lookup: false,
        ..ClientOptions::default()
    };
    let phone = Client::open(&dir.path().join("phone"), options)
        .await
        .unwrap();
    forget_and_pair_again(&node, &phone, "relay").await;

    phone.shutdown().await;
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forgotten_phone_is_told_so_at_the_nodes_address() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), Config::local_test("Kim")).await;
    let phone = Client::open(&dir.path().join("phone"), ClientOptions::local_test())
        .await
        .unwrap();
    forget_and_pair_again(&node, &phone, "direct").await;

    phone.shutdown().await;
    node.shutdown().await.unwrap();
}
