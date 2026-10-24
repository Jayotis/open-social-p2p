//! The follower swarm (doc 13): what a follower's node fetched ahead of time
//! is served to anyone only if its owner chose to share it; the author's
//! node names followers holding a copy, to its followers only; and a node
//! in the swarm spreads out its fetches and tries other followers first.
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
};
use osp_node::{BlobClass, Config, Home, Node, SimSettings};
use osp_proto::{
    Attachment, BlobHash, SOCIAL_ALPN,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, PeerRecord},
    social::{SocialRequest, SocialResponse},
};

/// Matches `MAX_SWARM_SAMPLES_PER_HOUR` in `social.rs`.
const SWARM_SAMPLES_PER_HOUR: usize = 60;
/// Long enough for anything on loopback.
const PROMPTLY: Duration = Duration::from_secs(30);

/// Starts a node. One that shares fetches large files at once, not spread
/// out over minutes.
async fn start(root: &Path, name: &str, share_prefetched: bool) -> Node {
    start_spread(root, name, share_prefetched, Duration::ZERO).await
}

async fn start_spread(root: &Path, name: &str, share_prefetched: bool, spread: Duration) -> Node {
    let home = root.join(name.to_lowercase());
    let mut config = Config::local_test(name);
    config.swarm.share_prefetched = share_prefetched;
    Node::init(&home, &config).unwrap();
    let sim = SimSettings {
        prefetch_spread: spread,
        ..SimSettings::default()
    };
    Node::spawn_sim(&home, sim).await.unwrap()
}

/// Stops a node, changes whether it shares, and starts it again.
async fn restart_sharing(node: Node, root: &Path, name: &str, share_prefetched: bool) -> Node {
    node.shutdown().await.unwrap();
    let home = Home::new(root.join(name.to_lowercase()));
    let mut config = Config::load(&home.config()).unwrap();
    config.swarm.share_prefetched = share_prefetched;
    config.save(&home.config()).unwrap();
    let sim = SimSettings {
        prefetch_spread: Duration::ZERO,
        ..SimSettings::default()
    };
    Node::spawn_sim(home.root(), sim).await.unwrap()
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

/// Posts a photo. A `small` one is under 1 MiB; otherwise it's noise, which
/// compresses badly, about 3 MB. Returns its attachment.
async fn post_photo(node: &Node, dir: &Path, small: bool) -> Attachment {
    let photo = dir.join("photo.png");
    let source = if small {
        "testsrc=size=640x360"
    } else {
        "nullsrc=s=2048x2048,geq=random(1)*255:random(1)*255:random(1)*255"
    };
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", source])
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
    };
    call(node, post).await;
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    let photo = items[0].attachments[0].attachment.clone();
    assert_eq!(photo.size <= 1 << 20, small, "{} bytes", photo.size);
    photo
}

/// Waits until `node` holds `blob` as a prefetched copy.
async fn prefetched(node: &Node, blob: &BlobHash) {
    let started = Instant::now();
    while node.blob_class(blob).unwrap() != Some(BlobClass::Prefetch) {
        assert!(started.elapsed() < PROMPTLY, "not prefetched");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A peer node made by hand, connected to a node over `osp/social/1`.
/// Holds its endpoint, since dropping that closes the connection.
struct Peer {
    endpoint: Endpoint,
    conn: Connection,
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
        Self { endpoint, conn }
    }

    /// A peer that follows the node, sharing or not.
    async fn follower(node: &EndpointAddr, shares_prefetched: bool) -> Self {
        let peer = Self::new(node).await;
        let req = SocialRequest::Subscribe {
            addr: peer.endpoint.addr(),
            shares_prefetched,
        };
        assert!(matches!(peer.ask(&req).await, SocialResponse::Ok));
        peer
    }

    async fn ask(&self, req: &SocialRequest) -> SocialResponse {
        let (mut send, mut recv) = self.conn.open_bi().await.unwrap();
        write_frame(&mut send, req).await.unwrap();
        send.finish().unwrap();
        read_frame(&mut recv).await.unwrap()
    }

    /// The nodes the node names as hosts of `blob`.
    async fn who_hosts(&self, blob: BlobHash) -> Vec<EndpointId> {
        match self.ask(&SocialRequest::WhoHosts { blob }).await {
            SocialResponse::Hosts { nodes } => nodes.into_iter().map(|a| a.id).collect(),
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Waits until the node names `host` as a host of `blob`. Slowly
    /// enough to stay within the samples a follower gets an hour.
    async fn wait_for_host(&self, blob: BlobHash, host: EndpointId) {
        let started = Instant::now();
        while !self.who_hosts(blob).await.contains(&host) {
            assert!(started.elapsed() < PROMPTLY, "{host} was never named");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn hosting(&self, blob: BlobHash) -> SocialResponse {
        self.ask(&SocialRequest::Hosting { blob }).await
    }
}

/// What `node` measured about `other` (ADR 0011).
async fn peer(node: &Node, other: EndpointId) -> Option<PeerRecord> {
    match call(node, ClientRequest::Peers).await {
        ClientResponse::Peers { peers } => peers.into_iter().find(|p| p.node == other),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_followers_that_share_serve_what_they_prefetched() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice", false).await;
    let bob = start(dir.path(), "Bob", true).await;
    let carol = start(dir.path(), "Carol", false).await;
    let photo = post_photo(&alice, dir.path(), true).await;

    // Bob and Carol follow Alice and fetch her photo ahead of time.
    for node in [&bob, &carol] {
        follow(node, &alice).await;
        assert_eq!(
            node.blob_class(&photo.blob).unwrap(),
            Some(BlobClass::Prefetch)
        );
    }

    // A stranger may download Bob's copy, but not Carol's.
    let dave = start(dir.path(), "Dave", false).await;
    let from = async |node: &Node| {
        dave.download_blob_from(&photo.blob, photo.size, &[node.addr().await])
            .await
    };
    assert!(from(&carol).await.is_err(), "Carol doesn't share");
    from(&bob).await.expect("Bob shares");

    // Each names itself as a host only if it serves the photo to anyone.
    let hosts = Peer::new(&bob.addr().await)
        .await
        .who_hosts(photo.blob)
        .await;
    assert!(hosts.contains(&bob.node_id()), "{hosts:?}");
    assert!(hosts.contains(&alice.node_id()), "{hosts:?}");
    let hosts = Peer::new(&carol.addr().await)
        .await
        .who_hosts(photo.blob)
        .await;
    assert!(!hosts.contains(&carol.node_id()), "{hosts:?}");
    assert!(hosts.contains(&alice.node_id()), "{hosts:?}");

    // Sharing can be turned off: the copy stays, served only to Bob's devices.
    let bob = restart_sharing(bob, dir.path(), "Bob", false).await;
    assert_eq!(
        bob.blob_class(&photo.blob).unwrap(),
        Some(BlobClass::Prefetch)
    );
    let erin = start(dir.path(), "Erin", false).await;
    let got = erin
        .download_blob_from(&photo.blob, photo.size, &[bob.addr().await])
        .await;
    assert!(got.is_err(), "Bob stopped sharing");
    let hosts = Peer::new(&bob.addr().await)
        .await
        .who_hosts(photo.blob)
        .await;
    assert!(!hosts.contains(&bob.node_id()), "{hosts:?}");

    for node in [alice, bob, carol, dave, erin] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_author_names_followers_with_a_copy_only_to_followers() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice", false).await;
    let addr = alice.addr().await;
    let photo = post_photo(&alice, dir.path(), false).await;

    // Bob shares, and tells Alice's node once he holds a copy. Carol doesn't.
    let bob = start(dir.path(), "Bob", true).await;
    let carol = start(dir.path(), "Carol", false).await;
    follow(&bob, &alice).await;
    follow(&carol, &alice).await;
    prefetched(&bob, &photo.blob).await;
    prefetched(&carol, &photo.blob).await;
    let watcher = Peer::follower(&addr, false).await;
    watcher.wait_for_host(photo.blob, bob.node_id()).await;
    let follower = Peer::follower(&addr, false).await;
    let hosts = follower.who_hosts(photo.blob).await;
    assert_eq!(hosts[0], alice.node_id(), "{hosts:?}");
    assert!(hosts.contains(&bob.node_id()), "{hosts:?}");
    assert!(!hosts.contains(&carol.node_id()), "{hosts:?}");

    // A stranger hears only of Alice's node.
    let stranger = Peer::new(&addr).await;
    assert_eq!(stranger.who_hosts(photo.blob).await, [alice.node_id()]);

    // Only a follower that shares may say it holds a copy, and only of
    // Alice's own media.
    let sharing = Peer::follower(&addr, true).await;
    let other = BlobHash::from_bytes([7; 32]);
    for (peer, blob) in [
        (&stranger, photo.blob),
        (&follower, photo.blob),
        (&sharing, other),
    ] {
        let resp = peer.hosting(blob).await;
        assert!(matches!(resp, SocialResponse::Error { .. }), "{resp:?}");
    }
    assert!(matches!(
        sharing.hosting(photo.blob).await,
        SocialResponse::Ok
    ));
    let hosts = follower.who_hosts(photo.blob).await;
    assert!(hosts.contains(&sharing.endpoint.id()), "{hosts:?}");
    let hosts = sharing.who_hosts(photo.blob).await;
    assert!(!hosts.contains(&sharing.endpoint.id()), "{hosts:?}");

    // A follower gets only so many samples an hour; it has had two.
    for _ in 2..SWARM_SAMPLES_PER_HOUR {
        assert!(follower.who_hosts(photo.blob).await.len() > 1);
    }
    assert_eq!(follower.who_hosts(photo.blob).await, [alice.node_id()]);

    // A follower that stops sharing isn't named any more. Once it has
    // synced, its node has subscribed again, saying so.
    let bob = restart_sharing(bob, dir.path(), "Bob", false).await;
    call(&bob, ClientRequest::Sync).await;
    let hosts = Peer::follower(&addr, false)
        .await
        .who_hosts(photo.blob)
        .await;
    assert!(!hosts.contains(&bob.node_id()), "{hosts:?}");
    assert!(hosts.contains(&sharing.endpoint.id()), "{hosts:?}");

    for node in [alice, bob, carol] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_in_the_swarm_fetches_from_other_followers_first() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice", false).await;
    let photo = post_photo(&alice, dir.path(), false).await;
    let bob = start(dir.path(), "Bob", true).await;
    follow(&bob, &alice).await;
    prefetched(&bob, &photo.blob).await;
    let follower = Peer::follower(&alice.addr().await, false).await;
    follower.wait_for_host(photo.blob, bob.node_id()).await;

    // Dave gets the photo from Bob, not from Alice.
    let dave = start(dir.path(), "Dave", true).await;
    follow(&dave, &alice).await;
    prefetched(&dave, &photo.blob).await;
    let from_bob = peer(&dave, bob.node_id()).await.expect("Bob's node");
    assert!(
        from_bob.counts.received_bytes.round() >= photo.size as f64,
        "{from_bob:?}"
    );
    let from_alice = peer(&dave, alice.node_id()).await.expect("Alice's node");
    assert!(
        from_alice.counts.received_bytes < photo.size as f64,
        "{from_alice:?}"
    );
    // And tells Alice's node it holds a copy too. It doesn't name Bob to
    // others: Alice's node named him to followers only.
    follower.wait_for_host(photo.blob, dave.node_id()).await;
    let hosts = Peer::new(&dave.addr().await)
        .await
        .who_hosts(photo.blob)
        .await;
    assert!(!hosts.contains(&bob.node_id()), "{hosts:?}");

    for node in [alice, bob, dave] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_in_the_swarm_spreads_out_large_fetches() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice", false).await;
    let photo = post_photo(&alice, dir.path(), false).await;

    // With an hour to spread over, the thumbnail comes at once and the
    // photo later. A sync doesn't wait for it.
    let hour = Duration::from_secs(60 * 60);
    let bob = start_spread(dir.path(), "Bob", true, hour).await;
    let started = Instant::now();
    follow(&bob, &alice).await;
    assert!(started.elapsed() < PROMPTLY, "{:?}", started.elapsed());
    let thumb = photo.thumbnail.as_ref().expect("a thumbnail");
    assert_eq!(
        bob.blob_class(&thumb.blob).unwrap(),
        Some(BlobClass::Prefetch)
    );
    // Long enough to fetch it, had it not waited.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(bob.blob_class(&photo.blob).unwrap(), None);

    // A node that doesn't share fetches it at once, as before.
    let carol = start_spread(dir.path(), "Carol", false, hour).await;
    follow(&carol, &alice).await;
    assert_eq!(
        carol.blob_class(&photo.blob).unwrap(),
        Some(BlobClass::Prefetch)
    );

    // So does one that shares, for a reshared post whose author's node it
    // doesn't follow: there's no node to ask who holds a copy.
    let ClientResponse::Feed { items } = call(&alice, ClientRequest::Feed { limit: 1 }).await
    else {
        panic!("no feed");
    };
    let reshare = ClientRequest::Reshare {
        id: items[0].id,
        host: false,
    };
    call(&carol, reshare).await;
    let dave = start_spread(dir.path(), "Dave", true, hour).await;
    follow(&dave, &carol).await;
    assert_eq!(
        dave.blob_class(&photo.blob).unwrap(),
        Some(BlobClass::Prefetch)
    );

    for node in [alice, bob, carol, dave] {
        node.shutdown().await.unwrap();
    }
}
