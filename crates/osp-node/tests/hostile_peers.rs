//! Peers that misbehave on purpose: oversized requests, floods of requests
//! that never finish, and unknown devices. The node must turn them away
//! quickly and keep serving everyone else.
//!
//! Each stranger is a fresh iroh endpoint with a new key, as an attacker's
//! would be. Everything runs in this process on loopback.

use std::{net::SocketAddr, path::Path, time::Duration};

use iroh::{
    Endpoint, EndpointAddr, RelayMode, SecretKey,
    address_lookup::MemoryLookup,
    endpoint::{Connection, RecvStream, SendStream, presets},
};
use osp_node::{Config, Node};
use osp_proto::{
    CLIENT_ALPN, SOCIAL_ALPN,
    framing::{read_frame, write_frame},
    rpc::{self, ClientRequest, ClientResponse},
    social::{self, SocialRequest, SocialResponse},
};

/// Matches `MAX_REQUESTS` and `MAX_REQUESTS_PER_CONN` in `social.rs`.
const SOCIAL_LIMIT: usize = 32;
const SOCIAL_LIMIT_PER_CONN: usize = 4;
/// Matches `MAX_UNKNOWN_REQUESTS` in `client.rs`.
const UNKNOWN_LIMIT: usize = 8;
/// Matches `MAX_NEW_FOLLOWERS_PER_HOUR` in `social.rs`.
const NEW_FOLLOWERS_PER_HOUR: usize = 100;

/// Long enough for loopback, far shorter than any of the node's timeouts.
const PROMPTLY: Duration = Duration::from_secs(5);

async fn start(root: &Path, name: &str) -> Node {
    let home = root.join(name.to_lowercase());
    Node::init(&home, &Config::local_test(name)).unwrap();
    Node::spawn(&home).await.unwrap()
}

/// A peer nobody has seen before, connected to the node. Holds its endpoint,
/// since dropping that closes the connection. The endpoint never accepts
/// connections, so the node dialling it back waits until it gives up.
struct Stranger {
    endpoint: Endpoint,
    conn: Connection,
}

impl Stranger {
    /// Its own address, as it would give it when subscribing.
    fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }
}

impl std::ops::Deref for Stranger {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

async fn stranger(node: &EndpointAddr, alpn: &[u8]) -> Stranger {
    let lookup = MemoryLookup::new();
    lookup.add_endpoint_info(node.clone());
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .address_lookup(lookup)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let conn = endpoint.connect(node.clone(), alpn).await.unwrap();
    Stranger { endpoint, conn }
}

/// Runs a local request that must succeed.
async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn follower_count(node: &Node) -> usize {
    match call(node, ClientRequest::Status).await {
        ClientResponse::Status(status) => status.followers,
        other => panic!("unexpected {other:?}"),
    }
}

async fn subscribe(peer: &Stranger) -> SocialResponse {
    ask(peer, &SocialRequest::Subscribe { addr: peer.addr() }).await
}

/// Starts a request that never finishes: announces 100 bytes and sends 10.
/// Keep the streams; dropping them ends the request.
async fn stall(conn: &Connection) -> (SendStream, RecvStream) {
    let (mut send, recv) = conn.open_bi().await.unwrap();
    send.write_all(&100u32.to_be_bytes()).await.unwrap();
    send.write_all(&[b' '; 10]).await.unwrap();
    (send, recv)
}

/// Sends one complete social request and waits briefly for the reply.
async fn ask(conn: &Connection, req: &SocialRequest) -> SocialResponse {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, req).await.unwrap();
    send.finish().unwrap();
    tokio::time::timeout(PROMPTLY, read_frame(&mut recv))
        .await
        .expect("a prompt reply")
        .unwrap()
}

fn is_busy(resp: &SocialResponse) -> bool {
    matches!(resp, SocialResponse::Error { message } if message.contains("busy"))
}

/// Announces a frame one byte over `limit` and sends nothing more. The
/// node must end the stream at once instead of waiting for the bytes.
async fn assert_oversized_refused(conn: &Connection, limit: u32) {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(&(limit + 1).to_be_bytes()).await.unwrap();
    let reply = tokio::time::timeout(PROMPTLY, read_frame::<_, SocialResponse>(&mut recv))
        .await
        .expect("the node should end the stream promptly");
    assert!(reply.is_err(), "no reply is expected, got {reply:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_requests_are_refused_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), "Gina").await;
    let addr = node.addr().await;

    let peer = stranger(&addr, SOCIAL_ALPN).await;
    assert_oversized_refused(&peer, social::MAX_REQUEST_LEN).await;
    // The connection still works for proper requests.
    assert!(matches!(
        ask(&peer, &SocialRequest::GetProfile).await,
        SocialResponse::Profile { .. }
    ));

    // An unknown device may only pair, so it gets the much smaller limit.
    let device = stranger(&addr, CLIENT_ALPN).await;
    assert_oversized_refused(&device, rpc::MAX_PAIR_REQUEST_LEN).await;
    let (mut send, mut recv) = device.open_bi().await.unwrap();
    write_frame(&mut send, &ClientRequest::Status)
        .await
        .unwrap();
    send.finish().unwrap();
    let reply: ClientResponse = tokio::time::timeout(PROMPTLY, read_frame(&mut recv))
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(&reply, ClientResponse::Error { message } if message.contains("isn't paired")),
        "{reply:?}"
    );

    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flood_of_stalled_requests_gets_busy_replies_and_then_clears() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), "Hana").await;
    let addr = node.addr().await;

    // One connection can't hold more than its share.
    let first = stranger(&addr, SOCIAL_ALPN).await;
    let mut stalled = Vec::new();
    for _ in 0..SOCIAL_LIMIT_PER_CONN {
        stalled.push(stall(&first).await);
    }
    assert!(is_busy(&ask(&first, &SocialRequest::GetProfile).await));

    // Fill the rest of the node's places from other new identities.
    let mut flood = vec![first];
    for _ in 1..SOCIAL_LIMIT / SOCIAL_LIMIT_PER_CONN {
        let conn = stranger(&addr, SOCIAL_ALPN).await;
        for _ in 0..SOCIAL_LIMIT_PER_CONN {
            stalled.push(stall(&conn).await);
        }
        flood.push(conn);
    }
    let latecomer = stranger(&addr, SOCIAL_ALPN).await;
    assert!(is_busy(&ask(&latecomer, &SocialRequest::GetProfile).await));

    // The owner's own requests don't go through those limits.
    assert!(matches!(
        node.handle(ClientRequest::Status).await,
        ClientResponse::Status(_)
    ));

    // Once the flood goes away, its places are free again.
    drop(stalled);
    for conn in &flood {
        conn.close(0u32.into(), b"done");
    }
    let mut served = false;
    for _ in 0..50 {
        if matches!(
            ask(&latecomer, &SocialRequest::GetProfile).await,
            SocialResponse::Profile { .. }
        ) {
            served = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(served, "the node should serve again once the flood is gone");

    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_devices_get_only_a_few_places() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), "Iris").await;
    let addr = node.addr().await;

    let prober = stranger(&addr, CLIENT_ALPN).await;
    let mut stalled = Vec::new();
    for _ in 0..UNKNOWN_LIMIT {
        stalled.push(stall(&prober).await);
    }
    // Over the limit the node doesn't even read the request.
    let other = stranger(&addr, CLIENT_ALPN).await;
    let (mut send, mut recv) = other.open_bi().await.unwrap();
    write_frame(&mut send, &ClientRequest::Status)
        .await
        .unwrap();
    send.finish().unwrap();
    let reply = tokio::time::timeout(PROMPTLY, read_frame::<_, ClientResponse>(&mut recv))
        .await
        .expect("the node should end the stream promptly");
    assert!(reply.is_err(), "no reply is expected, got {reply:?}");

    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn subscribers_need_a_short_address_and_arrive_at_a_limited_rate() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), "Jade").await;
    let addr = node.addr().await;

    // An address padded with IP addresses of the subscriber's choosing.
    let padder = stranger(&addr, SOCIAL_ALPN).await;
    let mut padded = padder.addr();
    for port in 1..=20 {
        padded = padded.with_ip_addr(SocketAddr::from(([10, 0, 0, 1], port)));
    }
    let resp = ask(&padder, &SocialRequest::Subscribe { addr: padded }).await;
    assert!(
        matches!(&resp, SocialResponse::Error { message } if message.contains("at most")),
        "{resp:?}"
    );

    // New identities, one after another, up to the hourly limit.
    let first = stranger(&addr, SOCIAL_ALPN).await;
    assert!(matches!(subscribe(&first).await, SocialResponse::Ok));
    for _ in 1..NEW_FOLLOWERS_PER_HOUR {
        let peer = stranger(&addr, SOCIAL_ALPN).await;
        let resp = subscribe(&peer).await;
        assert!(matches!(resp, SocialResponse::Ok), "{resp:?}");
    }
    let late = stranger(&addr, SOCIAL_ALPN).await;
    assert!(is_busy(&subscribe(&late).await));
    assert_eq!(follower_count(&node).await, NEW_FOLLOWERS_PER_HOUR);
    // A follower the node already has may still subscribe again.
    assert!(matches!(subscribe(&first).await, SocialResponse::Ok));

    // Notifications are only welcome from nodes this one follows.
    let notify = SocialRequest::Notify {
        actor: SecretKey::generate().public(),
        seq: 1,
    };
    let resp = ask(&first, &notify).await;
    assert!(
        matches!(&resp, SocialResponse::Error { message } if message.contains("not following")),
        "{resp:?}"
    );

    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn followers_that_never_answer_dont_delay_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let author = start(dir.path(), "Kim").await;
    let addr = author.addr().await;

    // Followers whose nodes never accept a connection: notifying each one
    // waits until it times out.
    let mut silent = Vec::new();
    for _ in 0..10 {
        let peer = stranger(&addr, SOCIAL_ALPN).await;
        assert!(matches!(subscribe(&peer).await, SocialResponse::Ok));
        silent.push(peer);
    }
    // A real follower, subscribed last, so one-by-one it would be told last.
    let reader = start(dir.path(), "Lee").await;
    let ClientResponse::Ticket { ticket } = call(&author, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(&reader, ClientRequest::Follow { ticket }).await;
    let mut subscribed = false;
    for _ in 0..50 {
        if follower_count(&author).await == silent.len() + 1 {
            subscribed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(subscribed, "following should subscribe");

    call(
        &author,
        ClientRequest::Post {
            content: "hello, followers".into(),
            attachments: vec![],
            uploads: vec![],
        },
    )
    .await;
    // The reader's node polls only hourly in tests, so only the notification
    // can bring the post this soon.
    let mut arrived = false;
    for _ in 0..50 {
        let ClientResponse::Feed { items } = call(&reader, ClientRequest::Feed { limit: 10 }).await
        else {
            panic!("no feed");
        };
        if items.iter().any(|i| i.content == "hello, followers") {
            arrived = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(arrived, "the live follower should be notified at once");

    author.shutdown().await.unwrap();
    reader.shutdown().await.unwrap();
}
