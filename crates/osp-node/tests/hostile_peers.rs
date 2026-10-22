//! Peers that misbehave on purpose: oversized requests, floods of requests
//! that never finish, unknown devices, floods of subscribers, followed
//! nodes whose outboxes never move on or never end, and followed nodes
//! whose media would overfill the disk. The node must turn them away
//! quickly and keep serving everyone else.
//!
//! Each stranger is a fresh iroh endpoint with a new key, as an attacker's
//! would be. Everything runs in this process on loopback.

use std::{net::SocketAddr, path::Path, time::Duration};

use iroh::{
    Endpoint, EndpointAddr, RelayMode, SecretKey,
    address_lookup::MemoryLookup,
    endpoint::{Connection, RecvStream, SendStream, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_blobs::{BlobsProtocol, store::mem::MemStore};
use osp_node::{BlobClass, Config, Node};
use osp_proto::{
    Attachment, BlobHash, CLIENT_ALPN, FollowTicket, MediaKind, Note, Payload, Person, SOCIAL_ALPN,
    SignedObject,
    framing::{read_frame, write_frame},
    rpc::{self, ClientRequest, ClientResponse, FollowInfo},
    social::{self, OutboxEntry, SocialRequest, SocialResponse},
};

/// Matches `MAX_REQUESTS` and `MAX_REQUESTS_PER_CONN` in `social.rs`.
const SOCIAL_LIMIT: usize = 32;
const SOCIAL_LIMIT_PER_CONN: usize = 4;
/// Matches `MAX_UNKNOWN_REQUESTS` in `client.rs`.
const UNKNOWN_LIMIT: usize = 8;
/// Matches `MAX_NEW_FOLLOWERS_PER_HOUR` in `social.rs`.
const NEW_FOLLOWERS_PER_HOUR: usize = 100;
/// Matches `MAX_NEW_PER_DAY` in `sync.rs`.
const MAX_NEW_PER_DAY: u64 = 500;

/// Long enough for loopback, far shorter than any of the node's timeouts.
const PROMPTLY: Duration = Duration::from_secs(5);
/// Steps of 100 ms to wait for a background sync. A node syncs at most
/// every 5 s, however often it's woken (`MIN_SYNC_GAP` in `sync.rs`).
const SYNC_WAIT_STEPS: usize = 150;

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
    for _ in 0..SYNC_WAIT_STEPS {
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
    // can bring the post this soon. One at a time, it would take 150 s.
    let mut arrived = false;
    for _ in 0..SYNC_WAIT_STEPS {
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

/// What a hostile followed node does when asked for its outbox.
#[derive(Debug, Clone, Copy)]
enum Outbox {
    /// Sends an old entry every time, and claims there's always more.
    Stuck,
    /// Makes up a full page of new posts every time, claims there's
    /// always more, and slips in a post nobody reshared.
    Endless,
    /// Serves its `posts` in order, as an honest node would. What the
    /// posts say may still be untrue.
    Posts,
}

/// A followed node that signs its profile and posts properly, but whose
/// outbox never moves on, or never ends, or whose posts lie.
#[derive(Clone)]
struct HostileAuthor {
    key: SecretKey,
    outbox: Outbox,
    /// Someone else's post, sent along without any reshare pointing at it.
    stray: SignedObject,
    /// What `Outbox::Posts` serves.
    posts: Vec<SignedObject>,
}

impl std::fmt::Debug for HostileAuthor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HostileAuthor({:?})", self.outbox)
    }
}

impl ProtocolHandler for HostileAuthor {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
            let Ok(req) = read_frame::<_, SocialRequest>(&mut recv).await else {
                continue;
            };
            write_frame(&mut send, &self.answer(req)).await.ok();
            send.finish().ok();
        }
        Ok(())
    }
}

impl HostileAuthor {
    fn answer(&self, req: SocialRequest) -> SocialResponse {
        match req {
            SocialRequest::GetProfile => SocialResponse::Profile {
                profile: SignedObject::sign(
                    &self.key,
                    &Payload::Person(Person {
                        actor: self.key.public(),
                        name: "Mallory".into(),
                        summary: None,
                        nodes: vec![],
                        published: osp_proto::now_rfc3339(),
                    }),
                )
                .unwrap(),
            },
            SocialRequest::GetOutbox { since, limit } => {
                let (entries, referenced) = match (self.outbox, limit) {
                    (_, 0) => (vec![], vec![]),
                    (Outbox::Stuck, _) => (vec![self.entry(1)], vec![]),
                    (Outbox::Endless, _) => (
                        (since + 1..=since + u64::from(limit))
                            .map(|seq| self.entry(seq))
                            .collect(),
                        vec![self.stray.clone()],
                    ),
                    (Outbox::Posts, _) => (
                        (1..)
                            .zip(&self.posts)
                            .map(|(seq, object)| OutboxEntry {
                                seq,
                                object: object.clone(),
                            })
                            .filter(|e| e.seq > since)
                            .take(limit as usize)
                            .collect(),
                        vec![],
                    ),
                };
                let latest = match self.outbox {
                    Outbox::Posts => self.posts.len() as u64,
                    _ => since + 1_000_000,
                };
                SocialResponse::Outbox {
                    entries,
                    referenced,
                    latest,
                }
            }
            _ => SocialResponse::Ok,
        }
    }

    fn entry(&self, seq: u64) -> OutboxEntry {
        OutboxEntry {
            seq,
            object: note(&self.key, &format!("flood {seq}")),
        }
    }
}

fn note(key: &SecretKey, content: &str) -> SignedObject {
    note_with(key, content, vec![])
}

fn note_with(key: &SecretKey, content: &str, attachment: Vec<Attachment>) -> SignedObject {
    let payload = Payload::Note(Note {
        actor: key.public(),
        content: content.into(),
        published: osp_proto::now_rfc3339(),
        attachment,
        in_reply_to: None,
    });
    SignedObject::sign(key, &payload).unwrap()
}

/// A video attachment that declares `size` bytes for `blob`.
fn video(blob: BlobHash, size: u64) -> Attachment {
    Attachment {
        kind: MediaKind::Video,
        media_type: "video/mp4".into(),
        blob,
        size,
        width: 1280,
        height: 720,
        duration_ms: Some(1000),
        name: None,
        thumbnail: None,
        profile: "v1-720p".into(),
    }
}

/// Runs a hostile author on loopback. Returns its router (keep it) and a
/// ticket to follow it.
async fn hostile_author(outbox: Outbox) -> (Router, String, SignedObject) {
    let stray = note(&SecretKey::generate(), "nobody reshared this");
    let author = HostileAuthor {
        key: SecretKey::generate(),
        outbox,
        stray: stray.clone(),
        posts: vec![],
    };
    let (router, ticket) = serve(author, None).await;
    (router, ticket, stray)
}

/// Runs an author on loopback, serving `media` too if given. Returns its
/// router (keep it) and a ticket to follow it.
async fn serve(author: HostileAuthor, media: Option<&MemStore>) -> (Router, String) {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let ticket = FollowTicket {
        actor: author.key.public(),
        node: endpoint.addr(),
    }
    .to_string();
    let mut router = Router::builder(endpoint).accept(SOCIAL_ALPN, author);
    if let Some(media) = media {
        router = router.accept(iroh_blobs::ALPN, BlobsProtocol::new(media, None));
    }
    (router.spawn(), ticket)
}

async fn sync_within(node: &Node, limit: Duration) {
    let synced = tokio::time::timeout(limit, call(node, ClientRequest::Sync))
        .await
        .expect("sync should finish, whatever followed nodes do");
    assert!(
        matches!(synced, ClientResponse::Synced { .. }),
        "{synced:?}"
    );
}

async fn following(node: &Node) -> Vec<FollowInfo> {
    match call(node, ClientRequest::Following).await {
        ClientResponse::Following { follows } => follows,
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_followed_node_that_never_moves_on_cant_freeze_syncing() {
    let dir = tempfile::tempdir().unwrap();
    let reader = start(dir.path(), "Reed").await;
    let honest = start(dir.path(), "Ned").await;
    call(
        &honest,
        ClientRequest::Post {
            content: "an honest post".into(),
            attachments: vec![],
            uploads: vec![],
        },
    )
    .await;
    let ClientResponse::Ticket { ticket } = call(&honest, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    let (_mallory, mallory_ticket, _) = hostile_author(Outbox::Stuck).await;
    call(
        &reader,
        ClientRequest::Follow {
            ticket: mallory_ticket,
        },
    )
    .await;
    call(&reader, ClientRequest::Follow { ticket }).await;

    sync_within(&reader, Duration::from_secs(60)).await;
    let ClientResponse::Feed { items } = call(&reader, ClientRequest::Feed { limit: 10 }).await
    else {
        panic!("no feed");
    };
    assert!(items.iter().any(|i| i.content == "an honest post"));
    let mallory = following(&reader)
        .await
        .into_iter()
        .find(|f| f.name.as_deref() == Some("Mallory"))
        .expect("following Mallory");
    let error = mallory.last_error.unwrap_or_default();
    assert!(error.contains("out of order"), "{error}");

    reader.shutdown().await.unwrap();
    honest.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_followed_node_cant_flood_us_with_posts() {
    let dir = tempfile::tempdir().unwrap();
    let reader = start(dir.path(), "Rita").await;
    let (_mallory, ticket, stray) = hostile_author(Outbox::Endless).await;
    call(&reader, ClientRequest::Follow { ticket }).await;

    // A new follow starts 100 entries before the claimed end, and takes
    // at most 500 new posts a day.
    let start_cursor = 1_000_000 - 100;
    sync_within(&reader, Duration::from_secs(60)).await;
    let cursor = following(&reader).await[0].cursor;
    assert_eq!(cursor, start_cursor + MAX_NEW_PER_DAY);
    sync_within(&reader, Duration::from_secs(60)).await;
    assert_eq!(
        following(&reader).await[0].cursor,
        cursor,
        "today's limit holds"
    );

    // Objects nobody reshared aren't kept.
    let shown = reader.handle(ClientRequest::Show { id: stray.id() }).await;
    assert!(matches!(shown, ClientResponse::Error { .. }), "{shown:?}");

    reader.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_followed_nodes_media_cant_overfill_the_cache_or_be_bigger_than_declared() {
    const KB: u64 = 1 << 10;
    let dir = tempfile::tempdir().unwrap();
    // A reader with 1 MB for cached media and no room for hosting.
    let mut config = Config::local_test("Una");
    config.storage.cache_quota_mb = 1;
    config.storage.replica_quota_mb = 0;
    let home = dir.path().join("una");
    Node::init(&home, &config).unwrap();
    let reader = Node::spawn(&home).await.unwrap();

    // Three honest 400 KB videos, which don't all fit, and one five
    // times bigger than its post says.
    let media = MemStore::new();
    let key = SecretKey::generate();
    let mut posts = Vec::new();
    let mut honest = Vec::new();
    for fill in 1..=3u8 {
        let blob = add_blob(&media, vec![fill; 400 * KB as usize]).await;
        honest.push(blob);
        posts.push(note_with(&key, "a video", vec![video(blob, 400 * KB)]));
    }
    let liar = add_blob(&media, vec![9; 500 * KB as usize]).await;
    posts.push(note_with(
        &key,
        "a small video",
        vec![video(liar, 100 * KB)],
    ));
    let author = HostileAuthor {
        key,
        outbox: Outbox::Posts,
        stray: note(&SecretKey::generate(), "unused"),
        posts,
    };
    let (_mallory, ticket) = serve(author, Some(&media)).await;
    call(&reader, ClientRequest::Follow { ticket }).await;

    // A sync from the owner waits for the prefetch to finish.
    sync_within(&reader, Duration::from_secs(60)).await;
    let cached = honest
        .iter()
        .filter(|b| reader.blob_class(b).unwrap() == Some(BlobClass::Cache))
        .count();
    assert_eq!(
        cached, 2,
        "two fit in 1 MB; the third made room by evicting one"
    );
    assert_eq!(
        reader.blob_class(&liar).unwrap(),
        None,
        "stopped and not kept"
    );
    let ClientResponse::Status(status) = call(&reader, ClientRequest::Status).await else {
        panic!("no status");
    };
    let cache = status.storage.iter().find(|s| s.class == "cache").unwrap();
    assert!(cache.bytes <= 1 << 20, "{cache:?}");

    // Hosting needs replica room, and there is none; a plain reshare is fine.
    let ClientResponse::Feed { items } = call(&reader, ClientRequest::Feed { limit: 10 }).await
    else {
        panic!("no feed");
    };
    let id = items[0].id;
    let resp = reader
        .handle(ClientRequest::Reshare { id, host: true })
        .await;
    assert!(
        matches!(&resp, ClientResponse::Error { message } if message.contains("replica quota")),
        "{resp:?}"
    );
    call(&reader, ClientRequest::Reshare { id, host: false }).await;

    reader.shutdown().await.unwrap();
}

async fn add_blob(store: &MemStore, bytes: Vec<u8>) -> BlobHash {
    let tag = store.add_bytes(bytes).await.unwrap();
    BlobHash::from_bytes(*tag.hash.as_bytes())
}
