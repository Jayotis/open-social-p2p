//! Trusted followers (ADR 0013): a follower asks, the owner allows or
//! declines, or trusts someone they follow; trusted followers are named as
//! hosts more often, until trust is taken back. Every valid request gets
//! the same reply, so the asking node never learns the answer.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.
//! Requires ffmpeg.

use std::{
    net::SocketAddr,
    path::Path,
    time::{Duration, Instant},
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey,
    address_lookup::MemoryLookup,
    endpoint::{Connection, presets},
    protocol::Router,
};
use iroh_blobs::{BlobsProtocol, Hash, store::mem::MemStore};
use osp_node::{BlobClass, Config, Node, SimSettings};
use osp_proto::{
    Attachment, BlobHash, Payload, Person, SOCIAL_ALPN, SignedObject,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, TrustInfo},
    social::{SocialRequest, SocialResponse},
};

/// Matches `MAX_NEW_REQUESTS_PER_HOUR` in `trust.rs`.
const NEW_REQUESTS_PER_HOUR: usize = 10;
/// Long enough for anything on loopback.
const PROMPTLY: Duration = Duration::from_secs(30);

/// Starts a node that shares what it prefetches, at once rather than
/// spread out over minutes.
async fn start(root: &Path, name: &str) -> Node {
    let mut config = Config::local_test(name);
    config.swarm.share_prefetched = true;
    let sim = SimSettings {
        prefetch_spread: Duration::ZERO,
        ..SimSettings::default()
    };
    let home = root.join(name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn_sim(&home, sim).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn follow(node: &Node, author: &Node) {
    let ClientResponse::Ticket { ticket } = call(author, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(node, ClientRequest::Follow { ticket }).await;
    call(node, ClientRequest::Sync).await;
}

/// Requests waiting on `node`, and the nodes it trusts.
async fn trust(node: &Node) -> (Vec<TrustInfo>, Vec<TrustInfo>) {
    match call(node, ClientRequest::TrustRequests).await {
        ClientResponse::Trust { requests, trusted } => (requests, trusted),
        other => panic!("unexpected {other:?}"),
    }
}

async fn waiting(node: &Node) -> usize {
    match call(node, ClientRequest::Status).await {
        ClientResponse::Status(status) => status.trust_requests,
        other => panic!("unexpected {other:?}"),
    }
}

/// Posts a photo of noise, about 3 MB: large enough for followers to fetch
/// through the follower swarm.
async fn post_photo(node: &Node, dir: &Path) -> Attachment {
    let photo = dir.join("photo.png");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i"])
        .arg("nullsrc=s=2048x2048,geq=random(1)*255:random(1)*255:random(1)*255")
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let post = ClientRequest::Post {
        content: "a photo".into(),
        attachments: vec![photo],
        uploads: vec![],
        upload: None,
    };
    call(node, post).await;
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    items[0].attachments[0].attachment.clone()
}

/// A peer node made by hand, connected to a node over `osp/social/1`.
/// Holds its endpoint, since dropping that closes the connection.
struct Peer {
    endpoint: Endpoint,
    conn: Connection,
    /// The identity it speaks for.
    identity: SecretKey,
    _serving: Option<Router>,
}

impl Peer {
    async fn new(node: &EndpointAddr) -> Self {
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
        let conn = endpoint.connect(node.clone(), SOCIAL_ALPN).await.unwrap();
        Self {
            endpoint,
            conn,
            identity: SecretKey::generate(),
            _serving: None,
        }
    }

    /// A peer that follows the node, sharing what it prefetches.
    async fn follower(node: &EndpointAddr) -> Self {
        let peer = Self::new(node).await;
        let req = SocialRequest::Subscribe {
            addr: peer.endpoint.addr(),
            shares_prefetched: true,
            media: None,
            direct: None,
        };
        assert!(matches!(peer.ask(&req).await, SocialResponse::Ok));
        peer
    }

    /// A follower that holds a copy of `blob`, fetched from `node`, serves
    /// it to anyone, and has told the node so.
    async fn holding(node: &EndpointAddr, blob: BlobHash) -> Self {
        let mut peer = Self::follower(node).await;
        let media = MemStore::new();
        let conn = peer
            .endpoint
            .connect(node.clone(), iroh_blobs::ALPN)
            .await
            .unwrap();
        let hash = Hash::from_bytes(*blob.as_bytes());
        media.remote().fetch(conn, hash).await.unwrap();
        let router = Router::builder(peer.endpoint.clone())
            .accept(iroh_blobs::ALPN, BlobsProtocol::new(&media, None))
            .spawn();
        peer._serving = Some(router);
        let resp = peer.ask(&SocialRequest::Hosting { blob }).await;
        assert!(matches!(resp, SocialResponse::Ok), "{resp:?}");
        peer
    }

    async fn ask(&self, req: &SocialRequest) -> SocialResponse {
        let (mut send, mut recv) = self.conn.open_bi().await.unwrap();
        write_frame(&mut send, req).await.unwrap();
        send.finish().unwrap();
        read_frame(&mut recv).await.unwrap()
    }

    /// Asks to be trusted, as `name`, with a profile listing `nodes`.
    async fn ask_trust(&self, name: &str, nodes: Vec<EndpointId>, note: &str) -> SocialResponse {
        let profile = Payload::Person(Person {
            actor: self.identity.public(),
            name: name.to_string(),
            summary: None,
            nodes,
            published: osp_proto::now_rfc3339(),
        });
        let req = SocialRequest::AskTrust {
            profile: SignedObject::sign(&self.identity, &profile).unwrap(),
            note: note.to_string(),
        };
        self.ask(&req).await
    }

    /// Asks as itself: a profile listing its node.
    async fn ask_trust_as(&self, name: &str) -> SocialResponse {
        self.ask_trust(name, vec![self.endpoint.id()], "").await
    }

    /// The followers the node names as hosts of `blob`.
    async fn who_hosts(&self, blob: BlobHash) -> Vec<EndpointId> {
        match self.ask(&SocialRequest::WhoHosts { blob }).await {
            SocialResponse::Hosts { nodes } => nodes.into_iter().map(|a| a.id).collect(),
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_follower_asks_is_allowed_and_named_until_trust_is_taken_back() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let addr = alice.addr().await;
    let photo = post_photo(&alice, dir.path()).await;

    // Bob follows Alice, fetches her photo and tells her node, then asks.
    let bob = start(dir.path(), "Bob").await;
    follow(&bob, &alice).await;
    let started = Instant::now();
    while bob.blob_class(&photo.blob).unwrap() != Some(BlobClass::Prefetch) {
        assert!(started.elapsed() < PROMPTLY, "not prefetched");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let ask = ClientRequest::AskTrust {
        actor: alice.identity(),
        note: "Bob, from\u{1b}[2J the climbing club".into(),
    };
    call(&bob, ask).await;

    // Alice sees who says he's asking, that she doesn't follow him, and
    // that his node follows hers and holds a copy.
    assert_eq!(waiting(&alice).await, 1);
    let (requests, trusted) = trust(&alice).await;
    assert!(trusted.is_empty());
    let [request] = &requests[..] else {
        panic!("{requests:?}");
    };
    assert_eq!(request.node, bob.node_id());
    assert_eq!(request.actor, bob.identity());
    assert_eq!(request.name, "Bob");
    assert_eq!(request.note, "Bob, from [2J the climbing club");
    assert!(!request.followed && !request.trusted_elsewhere);
    assert!(request.follower_secs.is_some());
    assert!(request.reached.is_some());
    let started = Instant::now();
    while trust(&alice).await.0[0].copies == 0 {
        assert!(started.elapsed() < PROMPTLY, "Bob's copy wasn't noted");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // More followers with a copy than one answer names.
    let mut others = Vec::new();
    for _ in 0..12 {
        others.push(Peer::holding(&addr, photo.blob).await);
    }
    let follower = Peer::follower(&addr).await;
    let named = async || -> usize {
        let mut named = 0;
        for _ in 0..20 {
            let hosts = follower.who_hosts(photo.blob).await;
            assert_eq!(hosts.len(), 7, "Alice's node and six followers: {hosts:?}");
            named += usize::from(hosts.contains(&bob.node_id()));
        }
        named
    };

    // Allowed, Bob is named in every answer.
    let allow = ClientRequest::AnswerTrust {
        node: bob.node_id(),
        allow: true,
    };
    call(&alice, allow).await;
    assert_eq!(waiting(&alice).await, 0);
    let (requests, trusted) = trust(&alice).await;
    assert!(requests.is_empty());
    assert_eq!(trusted.len(), 1);
    assert_eq!(trusted[0].node, bob.node_id());
    assert_eq!(named().await, 20);

    // Trust taken back, he's named like the others: 6 places among 13.
    let untrust = ClientRequest::Untrust {
        actor: bob.identity(),
    };
    call(&alice, untrust).await;
    assert!(trust(&alice).await.1.is_empty());
    assert!(named().await < 20);

    for node in [alice, bob] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_trusts_someone_they_follow_without_a_request() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    let trust_bob = ClientRequest::TrustFollowed {
        actor: bob.identity(),
    };

    // Only someone she follows, and only nodes of his that follow her.
    let resp = alice.handle(trust_bob.clone()).await;
    assert!(matches!(&resp, ClientResponse::Error { message } if message.contains("follow")));
    follow(&alice, &bob).await;
    let resp = alice.handle(trust_bob.clone()).await;
    assert!(
        matches!(&resp, ClientResponse::Error { message } if message.contains("doesn't follow you")),
        "{resp:?}"
    );
    follow(&bob, &alice).await;
    call(&alice, trust_bob).await;

    let (requests, trusted) = trust(&alice).await;
    assert!(requests.is_empty());
    let [t] = &trusted[..] else {
        panic!("{trusted:?}");
    };
    assert_eq!((t.node, t.actor), (bob.node_id(), bob.identity()));
    assert_eq!(t.name, "Bob");
    assert!(t.followed);
    assert!(t.note.is_empty());

    // Asking now changes nothing, and Bob isn't told.
    let ask = ClientRequest::AskTrust {
        actor: alice.identity(),
        note: String::new(),
    };
    call(&bob, ask).await;
    assert_eq!(waiting(&alice).await, 0);

    for node in [alice, bob] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_valid_request_gets_the_same_reply_and_others_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice").await;
    let addr = alice.addr().await;

    // Only followers may ask, with their owner's profile listing their node.
    let stranger = Peer::new(&addr).await;
    let resp = stranger.ask_trust_as("Mallory").await;
    assert!(
        matches!(&resp, SocialResponse::Error { message } if message.contains("follower")),
        "{resp:?}"
    );
    let sam = Peer::follower(&addr).await;
    let elsewhere = SecretKey::generate().public();
    let resp = sam.ask_trust("Sam", vec![elsewhere], "").await;
    assert!(
        matches!(&resp, SocialResponse::Error { message } if message.contains("doesn't list")),
        "{resp:?}"
    );
    let resp = sam
        .ask_trust("Sam", vec![sam.endpoint.id()], &"x".repeat(201))
        .await;
    assert!(matches!(resp, SocialResponse::Error { .. }), "{resp:?}");
    assert_eq!(waiting(&alice).await, 0);

    // A new request, a repeat, and requests past the hour's limit all get
    // the same reply.
    let ok = |resp: SocialResponse| assert!(matches!(resp, SocialResponse::Ok), "{resp:?}");
    ok(sam.ask_trust("Sam", vec![sam.endpoint.id()], "first").await);
    ok(sam
        .ask_trust("Sam", vec![sam.endpoint.id()], "second")
        .await);
    let mut crowd = Vec::new();
    for i in 0..NEW_REQUESTS_PER_HOUR {
        let peer = Peer::follower(&addr).await;
        ok(peer.ask_trust_as(&format!("Fake {i}")).await);
        crowd.push(peer);
    }
    let (requests, _) = trust(&alice).await;
    assert_eq!(requests.len(), NEW_REQUESTS_PER_HOUR);
    assert!(requests.iter().all(|r| r.node != crowd[9].endpoint.id()));
    let from_sam = requests.iter().find(|r| r.node == sam.endpoint.id());
    assert_eq!(from_sam.unwrap().note, "second");

    // Declined, Sam asking again gets the same reply, and isn't listed.
    let decline = ClientRequest::AnswerTrust {
        node: sam.endpoint.id(),
        allow: false,
    };
    call(&alice, decline).await;
    ok(sam
        .ask_trust("Sam", vec![sam.endpoint.id()], "please")
        .await);
    let (requests, trusted) = trust(&alice).await;
    assert!(requests.iter().all(|r| r.node != sam.endpoint.id()));
    assert!(trusted.is_empty());
    // Allowed, a follower asking again gets it too.
    let allow = ClientRequest::AnswerTrust {
        node: crowd[0].endpoint.id(),
        allow: true,
    };
    call(&alice, allow).await;
    ok(crowd[0].ask_trust_as("Fake 0").await);
    assert_eq!(trust(&alice).await.1.len(), 1);

    alice.shutdown().await.unwrap();
}
