//! Who vouched for a node's owner (ADR 0029). Alice reshares Kim's post, and
//! Alice's node tells Kim's, which keeps it as a vouch for Kim. Once the post
//! has 10 resharers, Kim's node credits the first 3 it heard of with finding
//! it early, and Alice's node keeps her credit as a vouch for Alice.
//!
//! Other resharers are peers made by hand, so a test can sign as any
//! identity. All nodes run in this process on loopback with relays and DNS
//! lookup off.

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
use osp_node::{Config, Node};
use osp_proto::{
    Announce, FoundEarly, ObjectId, Payload, Person, PublicKey, SOCIAL_ALPN, SignedObject,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse},
    social::{SocialRequest, SocialResponse},
};

async fn start(root: &Path, name: &str) -> Node {
    let home = root.join(name.to_lowercase());
    Node::init(&home, &Config::local_test(name)).unwrap();
    Node::spawn(&home).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn follow(follower: &Node, followed: &Node) {
    let ClientResponse::Ticket { ticket } = call(followed, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    call(follower, ClientRequest::Follow { ticket }).await;
}

async fn post(node: &Node, text: &str) -> ObjectId {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, req).await else {
        panic!("not posted")
    };
    id
}

async fn reshare(node: &Node, id: ObjectId) {
    call(node, ClientRequest::Reshare { id, host: false }).await;
}

/// How many resharers the author's node counts for its post.
async fn resharers(node: &Node, id: ObjectId) -> Option<u32> {
    match call(node, ClientRequest::Show { id }).await {
        ClientResponse::Item { item } => item.resharers,
        other => panic!("unexpected {other:?}"),
    }
}

/// Waits up to 20 seconds for `check` to hold.
async fn wait_for<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check().await {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn profile_listing(identity: &SecretKey, nodes: Vec<EndpointId>) -> SignedObject {
    let person = Payload::Person(Person {
        actor: identity.public(),
        name: "Someone".into(),
        summary: None,
        discoverable: false,
        nodes,
        media_endpoints: vec![],
        published: osp_proto::now_rfc3339(),
    });
    SignedObject::sign(identity, &person).unwrap()
}

/// `identity`'s reshare of `author`'s `post`, claiming to be from `published`.
fn announce(
    identity: &SecretKey,
    post: ObjectId,
    author: PublicKey,
    published: &str,
) -> SignedObject {
    let a = Payload::Announce(Announce {
        actor: identity.public(),
        object: post,
        object_actor: author,
        published: published.into(),
        hosting: false,
    });
    SignedObject::sign(identity, &a).unwrap()
}

/// A node made by hand, connected to another. Holds its endpoint, since
/// dropping that closes the connection.
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

    async fn ask(&self, req: &SocialRequest) -> SocialResponse {
        let (mut send, mut recv) = self.conn.open_bi().await.unwrap();
        write_frame(&mut send, req).await.unwrap();
        send.finish().unwrap();
        read_frame(&mut recv).await.unwrap()
    }

    /// `identity`'s profile, listing this node.
    fn profile(&self, identity: &SecretKey) -> SignedObject {
        profile_listing(identity, vec![self.endpoint.id()])
    }

    /// Tells of `identity`'s reshare of `author`'s `post`.
    async fn reshared(
        &self,
        identity: &SecretKey,
        post: ObjectId,
        author: PublicKey,
        published: &str,
    ) -> SocialResponse {
        let req = SocialRequest::Reshared {
            announce: announce(identity, post, author, published),
            profile: self.profile(identity),
        };
        self.ask(&req).await
    }
}

fn ok(resp: SocialResponse) {
    assert!(matches!(resp, SocialResponse::Ok), "unexpected {resp:?}");
}

fn refused(resp: SocialResponse) -> String {
    match resp {
        SocialResponse::Error { message } => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reshare_tells_its_authors_node_once_per_resharer() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let alice = start(dir.path(), "Alice").await;
    let shared = post(&kim, "worth passing on").await;
    let other = post(&kim, "not reshared").await;
    assert_eq!(resharers(&kim, shared).await, Some(0));

    // Alice reshares it; her node tells Kim's.
    follow(&alice, &kim).await;
    call(&alice, ClientRequest::Sync).await;
    reshare(&alice, shared).await;
    wait_for("Kim's node to hear of Alice's reshare", || async {
        resharers(&kim, shared).await == Some(1)
    })
    .await;
    assert_eq!(resharers(&kim, other).await, Some(0));
    // Not Kim's post, not counted on Alice's node.
    assert_eq!(resharers(&alice, shared).await, None);

    // Bob tells of his twice: one vouch.
    let peer = Peer::new(&kim.addr().await).await;
    let bob = SecretKey::generate();
    let me = kim.identity();
    ok(peer
        .reshared(&bob, shared, me, &osp_proto::now_rfc3339())
        .await);
    ok(peer
        .reshared(&bob, shared, me, &osp_proto::now_rfc3339())
        .await);
    assert_eq!(resharers(&kim, shared).await, Some(2));

    // What doesn't check out is refused.
    let message = refused(
        peer.reshared(&bob, ObjectId::of(b"nothing"), me, "2026-01-01T00:00:00Z")
            .await,
    );
    assert!(
        message.contains("isn't one of this node's posts"),
        "{message}"
    );
    let message = refused(
        peer.reshared(&bob, shared, bob.public(), "2026-01-01T00:00:00Z")
            .await,
    );
    assert!(message.contains("isn't of this node's posts"), "{message}");
    let carol = SecretKey::generate();
    let req = SocialRequest::Reshared {
        announce: announce(&carol, shared, me, &osp_proto::now_rfc3339()),
        profile: profile_listing(&carol, vec![SecretKey::generate().public()]),
    };
    let message = refused(peer.ask(&req).await);
    assert!(message.contains("doesn't list your node"), "{message}");
    let req = SocialRequest::Reshared {
        announce: announce(&carol, shared, me, &osp_proto::now_rfc3339()),
        profile: peer.profile(&bob),
    };
    let message = refused(peer.ask(&req).await);
    assert!(message.contains("isn't the sender's owner's"), "{message}");
    assert_eq!(resharers(&kim, shared).await, Some(2));

    // A deleted post takes no more.
    call(&kim, ClientRequest::Delete { id: shared }).await;
    let message = refused(
        peer.reshared(&bob, shared, me, &osp_proto::now_rfc3339())
            .await,
    );
    assert!(
        message.contains("isn't one of this node's posts"),
        "{message}"
    );

    for node in [kim, alice] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_three_heard_of_are_credited_once_ten_reshare() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let alice = start(dir.path(), "Alice").await;
    let took_off = post(&kim, "this one takes off").await;
    follow(&alice, &kim).await;
    // So Kim's node can reach Alice's.
    follow(&kim, &alice).await;
    call(&alice, ClientRequest::Sync).await;

    // Alice is heard of first.
    reshare(&alice, took_off).await;
    wait_for("Kim's node to hear of Alice's reshare", || async {
        resharers(&kim, took_off).await == Some(1)
    })
    .await;

    // Eight more, each claiming to have reshared it a year ago: the order is
    // the one Kim's node heard them in.
    let peer = Peer::new(&kim.addr().await).await;
    let me = kim.identity();
    let others: Vec<SecretKey> = (0..9).map(|_| SecretKey::generate()).collect();
    for other in &others[..8] {
        ok(peer
            .reshared(other, took_off, me, "2025-11-13T00:00:00Z")
            .await);
    }
    assert_eq!(resharers(&kim, took_off).await, Some(9));
    assert!(kim.credited_finders(&took_off).unwrap().is_empty());

    // The tenth: Alice and the next two are credited.
    ok(peer
        .reshared(&others[8], took_off, me, "2025-11-13T00:00:00Z")
        .await);
    assert_eq!(
        kim.credited_finders(&took_off).unwrap(),
        [alice.identity(), others[0].public(), others[1].public()]
    );
    wait_for("Alice's node to keep her credit", || async {
        alice.found_early_credits().unwrap() == [took_off]
    })
    .await;

    // Credited once only.
    ok(peer
        .reshared(
            &SecretKey::generate(),
            took_off,
            me,
            &osp_proto::now_rfc3339(),
        )
        .await);
    assert_eq!(kim.credited_finders(&took_off).unwrap().len(), 3);

    for node in [kim, alice] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn credits_that_dont_check_out_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let alice = start(dir.path(), "Alice").await;
    let reshared = post(&kim, "Alice reshares this").await;
    let not_reshared = post(&kim, "but not this").await;
    follow(&alice, &kim).await;
    call(&alice, ClientRequest::Sync).await;
    reshare(&alice, reshared).await;

    let peer = Peer::new(&alice.addr().await).await;
    let dan = SecretKey::generate();
    let credit = |post: ObjectId, finder: PublicKey| {
        let f = Payload::FoundEarly(FoundEarly {
            actor: dan.public(),
            post,
            finder,
            published: osp_proto::now_rfc3339(),
        });
        SocialRequest::FoundEarly {
            credit: SignedObject::sign(&dan, &f).unwrap(),
            profile: peer.profile(&dan),
        }
    };
    // Dan isn't the author of what Alice reshared.
    let message = refused(peer.ask(&credit(reshared, alice.identity())).await);
    assert!(
        message.contains("isn't from the post's author"),
        "{message}"
    );
    // Alice didn't reshare it.
    let message = refused(peer.ask(&credit(not_reshared, alice.identity())).await);
    assert!(message.contains("didn't reshare"), "{message}");
    // It's for someone else.
    let message = refused(peer.ask(&credit(reshared, dan.public())).await);
    assert!(message.contains("isn't for this node's owner"), "{message}");
    assert!(alice.found_early_credits().unwrap().is_empty());

    for node in [kim, alice] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flood_of_new_resharers_stops_at_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let shared = post(&kim, "flooded").await;
    let peer = Peer::new(&kim.addr().await).await;
    let me = kim.identity();
    let now = osp_proto::now_rfc3339();
    let first = SecretKey::generate();
    ok(peer.reshared(&first, shared, me, &now).await);
    for _ in 1..200 {
        ok(peer
            .reshared(&SecretKey::generate(), shared, me, &now)
            .await);
    }
    let message = refused(
        peer.reshared(&SecretKey::generate(), shared, me, &now)
            .await,
    );
    assert!(
        message.contains("no more new resharers this hour"),
        "{message}"
    );
    // Someone known before isn't new.
    ok(peer.reshared(&first, shared, me, &now).await);
    assert_eq!(resharers(&kim, shared).await, Some(200));

    kim.shutdown().await.unwrap();
}

/// The vouchers in an answer to `WhoVouched`, checking each comes with its
/// voucher's profile.
fn vouchers(resp: SocialResponse) -> Vec<PublicKey> {
    let SocialResponse::Vouches { vouches } = resp else {
        panic!("unexpected {resp:?}")
    };
    vouches
        .iter()
        .map(|carried| {
            let voucher = carried.vouch.signer();
            assert_eq!(carried.profile.signer(), voucher);
            voucher
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn who_vouched_is_answered_newest_first_one_per_voucher() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let first = post(&kim, "first").await;
    let second = post(&kim, "second").await;
    let peer = Peer::new(&kim.addr().await).await;
    let me = kim.identity();
    let now = osp_proto::now_rfc3339();
    let resharers: Vec<SecretKey> = (0..40).map(|_| SecretKey::generate()).collect();
    for resharer in &resharers {
        ok(peer.reshared(resharer, first, me, &now).await);
        // Heard of one after another, so the order is plain.
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    // The first resharer reshares the second post too: newest, but once.
    ok(peer.reshared(&resharers[0], second, me, &now).await);

    let answered = vouchers(peer.ask(&SocialRequest::WhoVouched).await);
    let mut expected = vec![resharers[0].public()];
    expected.extend(resharers[9..].iter().rev().map(|k| k.public()));
    assert_eq!(answered, expected);

    // Ten questions an hour from one node.
    for _ in 1..10 {
        vouchers(peer.ask(&SocialRequest::WhoVouched).await);
    }
    let message = refused(peer.ask(&SocialRequest::WhoVouched).await);
    assert!(message.contains("no more of your questions"), "{message}");

    kim.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_gathers_its_owners_side_three_steps_back() {
    let dir = tempfile::tempdir().unwrap();
    let jimmy = start(dir.path(), "Jimmy").await;
    let bob = start(dir.path(), "Bob").await;
    let carol = start(dir.path(), "Carol").await;

    // Nobody vouched for Jimmy yet: nothing to gather, nobody asked.
    let gathered = jimmy.gather_side_now().await.unwrap();
    assert_eq!((gathered.side, gathered.asked), (0, 0));

    // Bob reshares Jimmy's post; Carol reshares Bob's.
    let jimmys = post(&jimmy, "by Jimmy").await;
    follow(&bob, &jimmy).await;
    call(&bob, ClientRequest::Sync).await;
    reshare(&bob, jimmys).await;
    let bobs = post(&bob, "by Bob").await;
    follow(&carol, &bob).await;
    call(&carol, ClientRequest::Sync).await;
    reshare(&carol, bobs).await;
    wait_for(
        "Jimmy's and Bob's nodes to hear of the reshares",
        || async {
            resharers(&jimmy, jimmys).await == Some(1) && resharers(&bob, bobs).await == Some(1)
        },
    )
    .await;

    // Dan, on a node made by hand, reshares Carol's.
    let carols = post(&carol, "by Carol").await;
    let peer = Peer::new(&carol.addr().await).await;
    let dan = SecretKey::generate();
    ok(peer
        .reshared(&dan, carols, carol.identity(), &osp_proto::now_rfc3339())
        .await);

    // So Jimmy's node can reach Bob's and Carol's, with DNS lookup off.
    follow(&jimmy, &bob).await;
    follow(&jimmy, &carol).await;

    let gathered = jimmy.gather_side_now().await.unwrap();
    assert_eq!(
        jimmy.side().unwrap(),
        [
            (bob.identity(), 1),
            (carol.identity(), 2),
            (dan.public(), 3)
        ]
    );
    // Bob's node and Carol's: Dan's, 3 steps back, isn't asked.
    assert_eq!((gathered.side, gathered.asked), (3, 2));

    for node in [jimmy, bob, carol] {
        node.shutdown().await.unwrap();
    }
}
