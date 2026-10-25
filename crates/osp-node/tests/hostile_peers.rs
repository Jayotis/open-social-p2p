//! Peers that misbehave on purpose: oversized requests, floods of requests
//! that never finish, unknown devices, floods of subscribers, followed
//! nodes whose outboxes never move on or never end, followed nodes whose
//! media would overfill the disk, and crowds downloading our media. The
//! node must turn them away quickly and keep serving everyone else.
//!
//! Each stranger is a fresh iroh endpoint with a new key, as an attacker's
//! would be. Everything runs in this process on loopback.

use std::{
    net::SocketAddr,
    path::Path,
    time::{Duration, Instant},
};

use iroh::{
    Endpoint, EndpointAddr, RelayMode, SecretKey,
    address_lookup::MemoryLookup,
    endpoint::{Connection, RecvStream, SendStream, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_blobs::{
    BlobsProtocol, Hash, get,
    protocol::{ChunkRanges, ChunkRangesExt, GetRequest},
    store::mem::MemStore,
};
use osp_node::{BlobClass, Config, Node, SimSettings};
use osp_proto::{
    Attachment, BlobHash, CLIENT_ALPN, FollowTicket, MediaKind, Note, ObjectId, Payload, Person,
    SOCIAL_ALPN, SignedObject,
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
/// Match the transfer limits in `store.rs`.
const TRANSFER_LIMIT: usize = 8;
const STRANGER_TRANSFER_LIMIT: usize = 4;
const TRANSFER_LIMIT_PER_NODE: usize = 2;
const BUSY_WAIT: Duration = Duration::from_secs(10);
/// Matches `STALL_TIMEOUT` in `store.rs`.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

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
    let req = SocialRequest::Subscribe {
        addr: peer.addr(),
        shares_prefetched: false,
    };
    ask(peer, &req).await
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
    // Over the limit the node doesn't even read the request. The node takes
    // a connection's requests in the order they were opened, so once the
    // prober's next one is turned away, its stalled ones hold every place.
    assert_unread(&prober).await;
    // The limit is shared: another device is turned away too.
    let other = stranger(&addr, CLIENT_ALPN).await;
    assert_unread(&other).await;

    node.shutdown().await.unwrap();
}

/// Sends a complete client request. The node must end the stream promptly
/// without replying.
async fn assert_unread(conn: &Connection) {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &ClientRequest::Status)
        .await
        .unwrap();
    send.finish().unwrap();
    let reply = tokio::time::timeout(PROMPTLY, read_frame::<_, ClientResponse>(&mut recv))
        .await
        .expect("the node should end the stream promptly");
    assert!(reply.is_err(), "no reply is expected, got {reply:?}");
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
    let req = SocialRequest::Subscribe {
        addr: padded,
        shares_prefetched: false,
    };
    let resp = ask(&padder, &req).await;
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
            upload: None,
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
            upload: None,
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
        .filter(|b| reader.blob_class(b).unwrap() == Some(BlobClass::Prefetch))
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
    // Prefetched media counts against the cache quota.
    let cached: u64 = status
        .storage
        .iter()
        .filter(|s| s.class == "cache" || s.class == "prefetch")
        .map(|s| s.bytes)
        .sum();
    assert!(cached <= 1 << 20, "{:?}", status.storage);

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

/// Posts a photo of noise, which compresses badly: a 2048 px WebP of about
/// 3 MB, more than a QUIC stream buffers (1.25 MB). Returns the post and
/// the photo's blob and size.
async fn post_noise_photo(node: &Node, dir: &Path) -> (ObjectId, BlobHash, u64) {
    let photo = dir.join("noise.png");
    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg("nullsrc=s=2048x2048,geq=random(1)*255:random(1)*255:random(1)*255")
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let post = ClientRequest::Post {
        content: "noise".into(),
        attachments: vec![photo],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, post).await else {
        panic!("not posted");
    };
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    let photo = &items[0].attachments[0].attachment;
    (id, photo.blob, photo.size)
}

/// Asks for a blob without reading the answer. Once the stream's buffer is
/// full the transfer can't finish, so it keeps its place until dropped.
async fn hold(conn: &Connection, blob: BlobHash) -> get::fsm::ConnectedNext {
    let request = GetRequest::blob(Hash::from_bytes(*blob.as_bytes()));
    let connected = get::fsm::start(conn.clone(), request, Default::default())
        .next()
        .await
        .unwrap();
    let held = connected.next().await.unwrap();
    // Gives the node time to grant it a place.
    tokio::time::sleep(Duration::from_millis(200)).await;
    held
}

/// Downloads a blob in full. Returns whether it arrived, and how long that
/// took, or how long until the node refused.
async fn fetch(conn: &Connection, blob: BlobHash) -> (bool, Duration) {
    let started = Instant::now();
    let got = MemStore::new()
        .remote()
        .fetch(conn.clone(), Hash::from_bytes(*blob.as_bytes()))
        .await;
    (got.is_ok(), started.elapsed())
}

/// A stranger that subscribed to the node, so the node counts it as a
/// follower, connected for downloading media.
async fn follower_downloading(addr: &EndpointAddr) -> (Stranger, Connection) {
    let follower = stranger(addr, SOCIAL_ALPN).await;
    assert!(matches!(subscribe(&follower).await, SocialResponse::Ok));
    let conn = follower
        .endpoint
        .connect(addr.clone(), iroh_blobs::ALPN)
        .await
        .unwrap();
    (follower, conn)
}

#[tokio::test(flavor = "multi_thread")]
async fn media_transfers_take_turns_and_strangers_get_only_some_places() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), "Mona").await;
    let addr = node.addr().await;
    let (_, blob, _) = post_noise_photo(&node, dir.path()).await;

    // One node's third transfer waits until one of its first two ends.
    let first = stranger(&addr, iroh_blobs::ALPN).await;
    let mut held = Vec::new();
    for _ in 0..TRANSFER_LIMIT_PER_NODE {
        held.push(hold(&first, blob).await);
    }
    let conn = first.conn.clone();
    let third = tokio::spawn(async move { fetch(&conn, blob).await });
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !third.is_finished(),
        "a node's third transfer waits its turn"
    );
    held.pop();
    let (ok, took) = third.await.unwrap();
    assert!(ok && took < BUSY_WAIT, "served once a place was free");

    // Strangers share half the places between them.
    held.push(hold(&first, blob).await);
    let second = stranger(&addr, iroh_blobs::ALPN).await;
    for _ in 0..STRANGER_TRANSFER_LIMIT - TRANSFER_LIMIT_PER_NODE {
        held.push(hold(&second, blob).await);
    }
    let latecomer = stranger(&addr, iroh_blobs::ALPN).await;
    let conn = latecomer.conn.clone();
    let refused = tokio::spawn(async move { fetch(&conn, blob).await });

    // A follower still gets a place straight away.
    let (_follower, conn) = follower_downloading(&addr).await;
    let (ok, took) = fetch(&conn, blob).await;
    assert!(ok && took < BUSY_WAIT, "the follower waited {took:?}");

    // The stranger is turned away as busy, but only after waiting.
    let (ok, took) = refused.await.unwrap();
    assert!(!ok, "a stranger over the limit is refused");
    assert!(
        took >= BUSY_WAIT - Duration::from_secs(1),
        "refused after {took:?}"
    );

    drop(held);
    node.shutdown().await.unwrap();
}

/// Asks for `ranges` of a blob without reading the answer, as `hold` does.
async fn hold_part(
    conn: &Connection,
    blob: BlobHash,
    ranges: ChunkRanges,
) -> get::fsm::ConnectedNext {
    let request = GetRequest::blob_ranges(Hash::from_bytes(*blob.as_bytes()), ranges);
    let connected = get::fsm::start(conn.clone(), request, Default::default())
        .next()
        .await
        .unwrap();
    let held = connected.next().await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    held
}

/// Downloads `ranges` of a blob. Whether they arrived.
async fn fetch_part(conn: &Connection, blob: BlobHash, ranges: ChunkRanges) -> bool {
    let request = GetRequest::blob_ranges(Hash::from_bytes(*blob.as_bytes()), ranges);
    let got = MemStore::new().remote().execute_get(conn.clone(), request);
    got.complete().await.is_ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_fetching_in_parts_keeps_its_place_for_its_next_part() {
    const MB: u64 = 1 << 20;
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), "Mina").await;
    let addr = node.addr().await;
    let (_, blob, size) = post_noise_photo(&node, dir.path()).await;
    assert!(size > 2 * MB + MB / 4, "{size} bytes");

    // Strangers take every place for strangers. The last goes to one
    // fetching in parts, and its part is more than a stream holds, so it
    // can't end until it's read.
    let first = stranger(&addr, iroh_blobs::ALPN).await;
    let second = stranger(&addr, iroh_blobs::ALPN).await;
    let mut held = vec![
        hold(&first, blob).await,
        hold(&first, blob).await,
        hold(&second, blob).await,
    ];
    let parts = stranger(&addr, iroh_blobs::ALPN).await;
    let part = hold_part(&parts, blob, ChunkRanges::bytes(0..2 * MB)).await;
    // Another stranger waits for a place.
    let waiting = stranger(&addr, iroh_blobs::ALPN).await;
    held.push(hold(&waiting, blob).await);

    // The part ends, and the next is asked for at once: it gets the place
    // kept for it, before the one waiting.
    drop(part);
    let next = ChunkRanges::bytes(2 * MB..size);
    let started = Instant::now();
    assert!(
        fetch_part(&parts, blob, next).await,
        "the next part was refused"
    );
    assert!(started.elapsed() < PROMPTLY, "{:?}", started.elapsed());

    // The same when the next part is asked for before the first ends.
    drop(held);
    let ahead = stranger(&addr, iroh_blobs::ALPN).await;
    let mut held = vec![
        hold(&first, blob).await,
        hold(&first, blob).await,
        hold(&second, blob).await,
    ];
    let part = hold_part(&ahead, blob, ChunkRanges::bytes(0..2 * MB)).await;
    held.push(hold(&waiting, blob).await);
    let conn = ahead.conn.clone();
    let next = tokio::spawn(async move {
        let next = ChunkRanges::bytes(2 * MB..size);
        fetch_part(&conn, blob, next).await
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!next.is_finished(), "the next part waits for a place");
    drop(part);
    let started = Instant::now();
    assert!(next.await.unwrap(), "the next part was refused");
    assert!(started.elapsed() < PROMPTLY, "{:?}", started.elapsed());

    // A whole blob's place isn't kept: the one waiting gets it.
    drop(held);
    let late = stranger(&addr, iroh_blobs::ALPN).await;
    let mut held = vec![
        hold(&first, blob).await,
        hold(&first, blob).await,
        hold(&second, blob).await,
        hold(&parts, blob).await,
    ];
    let conn = late.conn.clone();
    let waited = tokio::spawn(async move { fetch(&conn, blob).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    held.pop();
    let freed = Instant::now();
    let (ok, _) = waited.await.unwrap();
    // Not after the 2 seconds a place is kept.
    assert!(ok, "the waiting stranger was refused");
    assert!(
        freed.elapsed() < Duration::from_secs(1),
        "{:?}",
        freed.elapsed()
    );

    drop(held);
    node.shutdown().await.unwrap();
}

/// Strangers and followers take every place, downloading `blob`, until
/// what this returns is dropped. The transfers come first, so they're
/// dropped first: dropping one ends it only while its connection is open.
async fn take_every_place(
    addr: &EndpointAddr,
    blob: BlobHash,
) -> (Vec<get::fsm::ConnectedNext>, Vec<Stranger>) {
    let mut peers = Vec::new();
    let mut held = Vec::new();
    for _ in 0..STRANGER_TRANSFER_LIMIT / TRANSFER_LIMIT_PER_NODE {
        let peer = stranger(addr, iroh_blobs::ALPN).await;
        for _ in 0..TRANSFER_LIMIT_PER_NODE {
            held.push(hold(&peer, blob).await);
        }
        peers.push(peer);
    }
    for _ in 0..(TRANSFER_LIMIT - STRANGER_TRANSFER_LIMIT) / TRANSFER_LIMIT_PER_NODE {
        let (follower, conn) = follower_downloading(addr).await;
        for _ in 0..TRANSFER_LIMIT_PER_NODE {
            held.push(hold(&conn, blob).await);
        }
        peers.push(follower);
    }
    (held, peers)
}

/// The thumbnail of the post `node` made last.
async fn last_thumbnail(node: &Node) -> BlobHash {
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    let thumb = &items[0].attachments[0].attachment.thumbnail;
    thumb.as_ref().expect("a thumbnail").blob
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_with_every_place_taken_sends_downloads_to_another_host() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), "Nora").await;
    let addr = node.addr().await;
    let (id, blob, size) = post_noise_photo(&node, dir.path()).await;

    // Another host with the same photo.
    let out = dir.path().join("out");
    let ClientResponse::Fetched { files } = call(
        &node,
        ClientRequest::Fetch {
            id,
            out_dir: Some(out),
        },
    )
    .await
    else {
        panic!("not fetched");
    };
    let photo = files.iter().find(|f| f.blob == blob).expect("the photo");
    let media = MemStore::new();
    let path = photo.path.as_ref().expect("exported");
    add_blob(&media, std::fs::read(path).unwrap()).await;
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let other_host = endpoint.addr();
    let _other = Router::builder(endpoint)
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&media, None))
        .spawn();

    let held = take_every_place(&addr, blob).await;

    // Even a follower is refused now, after waiting.
    let (_late, conn) = follower_downloading(&addr).await;
    let refused = tokio::spawn(async move { fetch(&conn, blob).await });

    // A thumbnail still goes out at once: small blobs have places of their own.
    let thumb = last_thumbnail(&node).await;
    let peer = stranger(&addr, iroh_blobs::ALPN).await;
    let (ok, took) = fetch(&peer, thumb).await;
    assert!(ok && took < PROMPTLY, "the thumbnail took {took:?}");

    // A node downloading the photo is refused too, and moves on to the
    // other host.
    let reader = start(dir.path(), "Otto").await;
    let started = Instant::now();
    let got = reader
        .download_blob_from(&blob, size, &[addr.clone(), other_host.clone()])
        .await;
    assert_eq!(got.unwrap(), size);
    let took = started.elapsed();
    assert!(
        took >= BUSY_WAIT - Duration::from_secs(1),
        "it asked the busy node first, {took:?}"
    );

    let (ok, took) = refused.await.unwrap();
    assert!(!ok, "a follower over the overall limit is refused");
    assert!(
        took >= BUSY_WAIT - Duration::from_secs(1),
        "refused after {took:?}"
    );

    // Both sides recorded it (ADR 0011). Counts fade a little even within a
    // test, so they're rounded.
    let peers = async |node: &Node| match call(node, ClientRequest::Peers).await {
        ClientResponse::Peers { peers } => peers,
        other => panic!("unexpected {other:?}"),
    };
    let seen = peers(&reader).await;
    let busy = seen
        .iter()
        .find(|p| p.node == addr.id)
        .expect("the busy node");
    assert_eq!(busy.counts.busy.round(), 1.0, "{busy:?}");
    assert_eq!(busy.counts.completed, 0.0, "{busy:?}");
    let other = seen
        .iter()
        .find(|p| p.node == other_host.id)
        .expect("the other host");
    assert!(other.counts.completed.round() >= 1.0, "{other:?}");
    assert!(
        other.counts.received_bytes.round() >= size as f64,
        "{other:?}"
    );
    let refused = peers(&node).await;
    let reader_there = refused
        .iter()
        .find(|p| p.node == reader.node_id())
        .expect("the reader");
    assert!(
        reader_there.counts.refused_busy.round() >= 1.0,
        "{reader_there:?}"
    );

    drop(held);
    reader.shutdown().await.unwrap();
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prefetch_refused_as_busy_is_tried_again_later() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), "Quinn").await;
    let addr = node.addr().await;
    let (_, photo, _) = post_noise_photo(&node, dir.path()).await;
    let thumb = last_thumbnail(&node).await;
    let held = take_every_place(&addr, photo).await;

    // A follower fetching the post ahead of time gets the thumbnail, but
    // the photo is refused as busy.
    let home = dir.path().join("rae");
    Node::init(&home, &Config::local_test("Rae")).unwrap();
    let sim = SimSettings {
        first_retry_wait: Duration::from_millis(500),
        ..SimSettings::default()
    };
    let reader = Node::spawn_sim(&home, sim).await.unwrap();
    let ClientResponse::Ticket { ticket } = call(&node, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(&reader, ClientRequest::Follow { ticket }).await;
    sync_within(&reader, Duration::from_secs(60)).await;
    assert_eq!(
        reader.blob_class(&thumb).unwrap(),
        Some(BlobClass::Prefetch)
    );
    assert_eq!(reader.blob_class(&photo).unwrap(), None, "refused as busy");

    // It is tried again soon, and refused again while every place is
    // still taken: the node asks twice, before and after asking who else
    // hosts it. Once places free up, it is tried again and arrives.
    tokio::time::sleep(BUSY_WAIT * 2 + Duration::from_secs(3)).await;
    assert_eq!(reader.blob_class(&photo).unwrap(), None);
    drop(held);
    let started = Instant::now();
    while reader.blob_class(&photo).unwrap().is_none() {
        assert!(
            started.elapsed() < BUSY_WAIT * 3,
            "not fetched again in time"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        reader.blob_class(&photo).unwrap(),
        Some(BlobClass::Prefetch)
    );

    reader.shutdown().await.unwrap();
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prefetch_that_fails_is_tried_again_later() {
    let dir = tempfile::tempdir().unwrap();
    // An author whose node doesn't have its video yet, so it can't send it.
    let media = MemStore::new();
    let bytes = vec![7u8; 400 << 10];
    let blob = BlobHash::from_bytes(*Hash::new(&bytes).as_bytes());
    let key = SecretKey::generate();
    let author = HostileAuthor {
        key: key.clone(),
        outbox: Outbox::Posts,
        stray: note(&SecretKey::generate(), "unused"),
        posts: vec![note_with(
            &key,
            "a video",
            vec![video(blob, bytes.len() as u64)],
        )],
    };
    let (_author, ticket) = serve(author, Some(&media)).await;

    let home = dir.path().join("xena");
    Node::init(&home, &Config::local_test("Xena")).unwrap();
    let sim = SimSettings {
        first_retry_wait: Duration::from_millis(500),
        ..SimSettings::default()
    };
    let reader = Node::spawn_sim(&home, sim).await.unwrap();
    call(&reader, ClientRequest::Follow { ticket }).await;
    // A sync from the owner waits for the prefetch, which fails.
    sync_within(&reader, Duration::from_secs(60)).await;
    assert_eq!(reader.blob_class(&blob).unwrap(), None);

    // Once the author's node has it, the prefetch is tried again and the
    // video arrives.
    add_blob(&media, bytes).await;
    let started = Instant::now();
    while reader.blob_class(&blob).unwrap().is_none() {
        assert!(
            started.elapsed() < PROMPTLY * 2,
            "not fetched again in time"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(reader.blob_class(&blob).unwrap(), Some(BlobClass::Prefetch));

    reader.shutdown().await.unwrap();
}

/// A host that takes requests for media and never answers them, as one
/// whose connection dropped out looks to a node fetching from it.
#[derive(Debug, Clone)]
struct Silent;

impl ProtocolHandler for Silent {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        // Holds each request's streams, unanswered, until the other side goes.
        let mut held = Vec::new();
        while let Ok(streams) = conn.accept_bi().await {
            held.push(streams);
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_moves_on_from_a_host_that_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), "Vera").await;
    let addr = node.addr().await;
    let (_, photo, size) = post_noise_photo(&node, dir.path()).await;
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let silent = endpoint.addr();
    let _silent = Router::builder(endpoint)
        .accept(iroh_blobs::ALPN, Silent)
        .spawn();

    // The silent host, asked first, is given up on once it has sent
    // nothing for a while, and the node with the photo is asked next.
    let reader = start(dir.path(), "Wes").await;
    let started = Instant::now();
    let got = tokio::time::timeout(
        STALL_TIMEOUT + PROMPTLY,
        reader.download_blob_from(&photo, size, &[silent.clone(), addr.clone()]),
    )
    .await
    .expect("it should move on long before the download's time is up");
    assert_eq!(got.unwrap(), size);
    let took = started.elapsed();
    assert!(
        took >= STALL_TIMEOUT - Duration::from_secs(1),
        "it asked the silent host first, {took:?}"
    );

    // Its record says so (ADR 0011).
    let ClientResponse::Peers { peers } = call(&reader, ClientRequest::Peers).await else {
        panic!("no peers");
    };
    let record = peers
        .iter()
        .find(|p| p.node == silent.id)
        .expect("the silent host");
    assert_eq!(record.counts.failed.round(), 1.0, "{record:?}");
    assert_eq!(record.counts.received_bytes, 0.0, "{record:?}");

    reader.shutdown().await.unwrap();
    node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_follower_refused_as_busy_is_sent_to_a_follower_with_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let node = start(dir.path(), "Sven").await;
    let addr = node.addr().await;
    let (_, photo, size) = post_noise_photo(&node, dir.path()).await;
    let ClientResponse::Ticket { ticket } = call(&node, ClientRequest::Invite).await else {
        panic!("no ticket");
    };

    // A follower that shares what it prefetches gets the photo and says so.
    let home = dir.path().join("tara");
    let mut config = Config::local_test("Tara");
    config.swarm.share_prefetched = true;
    Node::init(&home, &config).unwrap();
    let sim = SimSettings {
        prefetch_spread: Duration::ZERO,
        ..SimSettings::default()
    };
    let sharing = Node::spawn_sim(&home, sim).await.unwrap();
    let follow = ClientRequest::Follow {
        ticket: ticket.clone(),
    };
    call(&sharing, follow).await;
    let (watcher, _) = follower_downloading(&addr).await;
    let started = Instant::now();
    loop {
        let SocialResponse::Hosts { nodes } =
            ask(&watcher, &SocialRequest::WhoHosts { blob: photo }).await
        else {
            panic!("no hosts");
        };
        if nodes.iter().any(|a| a.id == sharing.node_id()) {
            break;
        }
        assert!(started.elapsed() < BUSY_WAIT * 3, "never named");
        // Each reply counts against the watcher's samples of the followers
        // with a copy, 60 an hour: asking faster would use them all up
        // before the follower says it holds one.
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // With every place taken, a follower that doesn't share is refused,
    // asks who else has it, and gets it from the one that does.
    let held = take_every_place(&addr, photo).await;
    let reader = start(dir.path(), "Ulla").await;
    call(&reader, ClientRequest::Follow { ticket }).await;
    sync_within(&reader, Duration::from_secs(60)).await;
    assert_eq!(
        reader.blob_class(&photo).unwrap(),
        Some(BlobClass::Prefetch)
    );
    let ClientResponse::Peers { peers } = call(&reader, ClientRequest::Peers).await else {
        panic!("no peers");
    };
    let from = peers
        .iter()
        .find(|p| p.node == sharing.node_id())
        .expect("the follower with a copy");
    assert!(
        from.counts.received_bytes.round() >= size as f64,
        "{from:?}"
    );
    // It didn't go back to the busy node first.
    let busy = peers.iter().find(|p| p.node == addr.id).expect("the node");
    assert_eq!(busy.counts.busy.round(), 1.0, "{busy:?}");

    // Nor does it name the follower to others: it was named to followers only.
    let other = stranger(&reader.addr().await, SOCIAL_ALPN).await;
    let SocialResponse::Hosts { nodes } =
        ask(&other, &SocialRequest::WhoHosts { blob: photo }).await
    else {
        panic!("no hosts");
    };
    assert!(nodes.iter().all(|a| a.id != sharing.node_id()), "{nodes:?}");

    drop(held);
    for node in [reader, sharing, node] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_upload_cap_slows_sending_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::local_test("Pia");
    config.serving.max_upload_mbps = 8;
    let home = dir.path().join("pia");
    Node::init(&home, &config).unwrap();
    let node = Node::spawn(&home).await.unwrap();
    let addr = node.addr().await;
    let (_, blob, size) = post_noise_photo(&node, dir.path()).await;

    // At 8 Mbit/s, a megabyte a second. On loopback it would take far less.
    let at_cap = Duration::from_secs_f64(size as f64 * 8.0 / 8_000_000.0);
    let peer = stranger(&addr, iroh_blobs::ALPN).await;
    let (ok, took) = fetch(&peer, blob).await;
    assert!(ok);
    assert!(
        took >= at_cap.mul_f64(0.8),
        "{size} bytes took {took:?}; the cap allows no less than {at_cap:?}"
    );

    node.shutdown().await.unwrap();
}
