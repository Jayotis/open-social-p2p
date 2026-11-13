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
    Announce, FoundEarly, Note, ObjectId, Payload, Person, PublicKey, SOCIAL_ALPN, SignedObject,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, ReplyInfo, SentStatus},
    social::{PlacedAt, ReplyState, SocialRequest, SocialResponse},
};

async fn start(root: &Path, name: &str) -> Node {
    start_with(root, name, false).await
}

/// Starts a node, with `vouchers` on or off (ADR 0029).
async fn start_with(root: &Path, name: &str, vouchers: bool) -> Node {
    let home = root.join(name.to_lowercase());
    let mut config = Config::local_test(name);
    config.replies.vouchers = vouchers;
    Node::init(&home, &config).unwrap();
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

    /// Follows the node it's connected to.
    async fn subscribe(&self) {
        let req = SocialRequest::Subscribe {
            addr: self.endpoint.addr(),
            shares_prefetched: false,
            media: None,
            direct: None,
        };
        ok(self.ask(&req).await);
    }

    /// Sends `identity`'s reply to `post`, and gives it and the answer.
    async fn reply(
        &self,
        identity: &SecretKey,
        post: ObjectId,
        text: &str,
    ) -> (SignedObject, SocialResponse) {
        let note = Payload::Note(Note {
            actor: identity.public(),
            content: text.into(),
            published: osp_proto::now_rfc3339(),
            attachment: vec![],
            in_reply_to: Some(post),
        });
        let reply = SignedObject::sign(identity, &note).unwrap();
        let req = SocialRequest::SubmitReply {
            reply: reply.clone(),
            profile: self.profile(identity),
        };
        let resp = self.ask(&req).await;
        (reply, resp)
    }

    /// Shows who vouched for the replier of `reply`.
    async fn show(&self, reply: &SignedObject, side: Vec<SignedObject>) -> SocialResponse {
        let req = SocialRequest::ShowVouches {
            reply: reply.id(),
            side,
        };
        self.ask(&req).await
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

/// The answer to a reply: its state, and whether its side is asked for.
fn answered(resp: SocialResponse) -> (ReplyState, bool) {
    match resp {
        SocialResponse::Reply {
            state,
            show_vouches,
        } => (state, show_vouches),
        other => panic!("unexpected {other:?}"),
    }
}

async fn requests(node: &Node) -> Vec<ReplyInfo> {
    match call(node, ClientRequest::ReplyRequests).await {
        ClientResponse::ReplyRequests { requests } => requests,
        other => panic!("unexpected {other:?}"),
    }
}

/// `voucher`'s reshare of a post of `of`'s: a vouch for them.
fn vouch(voucher: &SecretKey, of: PublicKey) -> SignedObject {
    announce(
        voucher,
        ObjectId::of(b"a post of theirs"),
        of,
        &osp_proto::now_rfc3339(),
    )
}

/// Someone `kim`'s owner trusts, on a node made by hand that follows hers.
async fn trusted_follower(kim: &Node) -> (SecretKey, Peer) {
    let peer = Peer::new(&kim.addr().await).await;
    peer.subscribe().await;
    let who = SecretKey::generate();
    let req = SocialRequest::AskTrust {
        profile: peer.profile(&who),
        note: "a friend".into(),
    };
    ok(peer.ask(&req).await);
    let node = peer.endpoint.id();
    call(kim, ClientRequest::AnswerTrust { node, allow: true }).await;
    (who, peer)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_strangers_side_is_asked_for_and_each_vouch_checked() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start_with(dir.path(), "Kim", true).await;
    let open = post(&kim, "anyone may reply").await;
    let mut circle = Vec::new();
    for _ in 0..3 {
        circle.push(trusted_follower(&kim).await);
    }

    // Someone in Kim's circle is published without being asked.
    let (alice, alice_node) = &circle[0];
    let (_, resp) = alice_node.reply(alice, open, "from the circle").await;
    assert_eq!(answered(resp), (ReplyState::Published, false));

    // Jimmy, a stranger, waits, and is asked who vouched for him: again if
    // his node sends the reply again.
    let jimmy = SecretKey::generate();
    let jimmys_node = Peer::new(&kim.addr().await).await;
    let (first, resp) = jimmys_node.reply(&jimmy, open, "hello Kim").await;
    assert_eq!(answered(resp), (ReplyState::AwaitingApproval, true));
    let req = SocialRequest::SubmitReply {
        reply: first.clone(),
        profile: jimmys_node.profile(&jimmy),
    };
    assert_eq!(
        answered(jimmys_node.ask(&req).await),
        (ReplyState::AwaitingApproval, true)
    );

    // Another node can't show it for him.
    let other = Peer::new(&kim.addr().await).await;
    let message = refused(other.show(&first, vec![]).await);
    assert!(message.contains("from your node"), "{message}");

    // Two of Kim's circle vouched for him. A third vouch doesn't verify,
    // a fourth is for someone else: neither counts.
    let mut tampered = serde_json::to_value(vouch(&circle[2].0, jimmy.public())).unwrap();
    tampered["signer"] = serde_json::to_value(circle[1].0.public()).unwrap();
    let side = vec![
        vouch(&circle[0].0, jimmy.public()),
        vouch(&circle[1].0, jimmy.public()),
        serde_json::from_value(tampered).unwrap(),
        vouch(&circle[2].0, SecretKey::generate().public()),
    ];
    assert_eq!(
        answered(jimmys_node.show(&first, side).await),
        (ReplyState::AwaitingApproval, false)
    );
    // Shown once only.
    let message = refused(jimmys_node.show(&first, vec![]).await);
    assert!(message.contains("didn't ask"), "{message}");
    let waiting = requests(&kim).await;
    assert_eq!(waiting.len(), 1);
    assert_eq!(
        waiting[0].why.as_deref(),
        Some("2 steps away; 2 people in your circle's world vouched for them")
    );

    // Three of Kim's circle: published.
    let (second, resp) = jimmys_node.reply(&jimmy, open, "hello again").await;
    assert_eq!(answered(resp), (ReplyState::AwaitingApproval, true));
    let side = circle
        .iter()
        .map(|(who, _)| vouch(who, jimmy.public()))
        .collect();
    assert_eq!(
        answered(jimmys_node.show(&second, side).await),
        (ReplyState::Published, false)
    );

    kim.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn with_vouchers_off_no_side_is_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let open = post(&kim, "anyone may reply").await;
    let alice = trusted_follower(&kim).await;
    let jimmy = SecretKey::generate();
    let jimmys_node = Peer::new(&kim.addr().await).await;
    let (reply, resp) = jimmys_node.reply(&jimmy, open, "hello Kim").await;
    assert_eq!(answered(resp), (ReplyState::AwaitingApproval, false));
    let side = vec![vouch(&alice.0, jimmy.public())];
    let message = refused(jimmys_node.show(&reply, side).await);
    assert!(message.contains("didn't ask"), "{message}");
    assert_eq!(
        requests(&kim).await[0].why.as_deref(),
        Some("you don't follow them")
    );

    kim.shutdown().await.unwrap();
}

/// Whom `peer` hears `identities` placed at, from the node it's connected to.
async fn placed(peer: &Peer, identities: &[PublicKey]) -> SocialResponse {
    let req = SocialRequest::PlaceWithin {
        identities: identities.to_vec(),
    };
    peer.ask(&req).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_places_for_its_followers_and_those_followed_within_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    // Kim follows Alice, who reshared Bob's post.
    let bobs = post(&bob, "by Bob").await;
    follow(&alice, &bob).await;
    call(&alice, ClientRequest::Sync).await;
    reshare(&alice, bobs).await;
    follow(&kim, &alice).await;
    call(&kim, ClientRequest::Sync).await;

    let asked = [
        kim.identity(),
        alice.identity(),
        bob.identity(),
        SecretKey::generate().public(),
    ];
    // A stranger isn't answered.
    let peer = Peer::new(&kim.addr().await).await;
    let message = refused(placed(&peer, &asked).await);
    assert!(message.contains("answers only the nodes"), "{message}");

    // A follower is, from what Kim's node holds, 60 times an hour.
    peer.subscribe().await;
    let SocialResponse::Placed { placed: answer } = placed(&peer, &asked).await else {
        panic!("not answered")
    };
    let at = |actor: PublicKey, steps: u8| PlacedAt { actor, steps };
    assert_eq!(
        answer,
        [
            at(kim.identity(), 0),
            at(alice.identity(), 1),
            at(bob.identity(), 2)
        ]
    );
    for _ in 1..60 {
        assert!(matches!(
            placed(&peer, &asked).await,
            SocialResponse::Placed { .. }
        ));
    }
    let message = refused(placed(&peer, &asked).await);
    assert!(message.contains("no more of your questions"), "{message}");

    for node in [kim, alice, bob] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_is_let_in_where_the_sides_meet_through_the_circle() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start_with(dir.path(), "Kim", true).await;
    let alice = start(dir.path(), "Alice").await;
    let bob = start(dir.path(), "Bob").await;
    let dan = start(dir.path(), "Dan").await;
    let jimmy = start(dir.path(), "Jimmy").await;

    // Kim follows Alice and Bob; Alice follows Dan. Syncing, Kim's node
    // subscribes to Alice's, which then answers its questions.
    follow(&kim, &alice).await;
    follow(&kim, &bob).await;
    follow(&alice, &dan).await;
    call(&kim, ClientRequest::Sync).await;

    // Alice, Bob and Dan reshare Jimmy's post, and Jimmy's node gathers
    // his side: those three, 1 step back.
    let jimmys = post(&jimmy, "by Jimmy").await;
    for node in [&alice, &bob, &dan] {
        follow(node, &jimmy).await;
        call(node, ClientRequest::Sync).await;
        reshare(node, jimmys).await;
        // So Jimmy's node can reach theirs, with DNS lookup off.
        follow(&jimmy, node).await;
    }
    wait_for("Jimmy's node to hear of the reshares", || async {
        resharers(&jimmy, jimmys).await == Some(3)
    })
    .await;
    jimmy.gather_side_now().await.unwrap();
    assert_eq!(jimmy.side().unwrap().len(), 3);

    // Jimmy replies to Kim's post. Kim's node places Alice and Bob itself,
    // 2 steps from Kim through them: 2 meeting points of the 3 needed. Then
    // Alice's node places Dan, whom she follows: 3 steps. Published.
    let kims = post(&kim, "by Kim").await;
    follow(&jimmy, &kim).await;
    call(&jimmy, ClientRequest::Sync).await;
    let req = ClientRequest::Reply {
        post: kims,
        content: "hello Kim".into(),
    };
    let ClientResponse::ReplySent { reply } = call(&jimmy, req).await else {
        panic!("not sent")
    };
    assert_eq!(reply.status, SentStatus::Published, "{reply:?}");
    assert!(requests(&kim).await.is_empty());

    for node in [kim, alice, bob, dan, jimmy] {
        node.shutdown().await.unwrap();
    }
}

/// The case that showed Kim's node mustn't count as a place of its own. Kim
/// follows Alice; Alice follows X and reshared X's post, and trusts four
/// people. Jimmy's side holds X and those four, 2 steps back from him, each
/// through a different person who vouched for him. Kim's
/// node places X through Alice's reshare, and Alice's node places all five:
/// five meeting points at 4 steps, every one through Alice. That stays a
/// request. Once Dave, also followed by Kim, follows X, Dave's node places X
/// too: a second root, and the next reply is published.
#[tokio::test(flavor = "multi_thread")]
async fn four_steps_through_alice_alone_waits_and_a_second_root_lets_it_in() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start_with(dir.path(), "Kim", true).await;
    let alice = start(dir.path(), "Alice").await;
    let dave = start(dir.path(), "Dave").await;
    let x = start(dir.path(), "X").await;

    let xs = post(&x, "by X").await;
    follow(&alice, &x).await;
    call(&alice, ClientRequest::Sync).await;
    reshare(&alice, xs).await;
    let mut trusted = Vec::new();
    for _ in 0..4 {
        trusted.push(trusted_follower(&alice).await.0);
    }
    follow(&kim, &alice).await;
    follow(&kim, &dave).await;
    call(&kim, ClientRequest::Sync).await;

    // Jimmy's side: five people vouched for him; X vouched for the first,
    // and each of Alice's four for one of the others.
    let jimmy = SecretKey::generate();
    let between: Vec<SecretKey> = (0..5).map(|_| SecretKey::generate()).collect();
    let mut side: Vec<SignedObject> = between.iter().map(|s| vouch(s, jimmy.public())).collect();
    side.push(x.sign_reshare_of(between[0].public()).unwrap());
    side.extend(
        trusted
            .iter()
            .zip(&between[1..])
            .map(|(t, s)| vouch(t, s.public())),
    );

    let open = post(&kim, "anyone may reply").await;
    let jimmys_node = Peer::new(&kim.addr().await).await;
    let (first, resp) = jimmys_node.reply(&jimmy, open, "hello Kim").await;
    assert_eq!(answered(resp), (ReplyState::AwaitingApproval, true));
    assert_eq!(
        answered(jimmys_node.show(&first, side.clone()).await),
        (ReplyState::AwaitingApproval, false)
    );
    assert_eq!(
        requests(&kim).await[0].why.as_deref(),
        Some("4 steps away; 5 people in your circle's world vouched for them")
    );

    // Dave follows X: Dave's node places X, through Dave.
    follow(&dave, &x).await;
    let (second, resp) = jimmys_node.reply(&jimmy, open, "hello again").await;
    assert_eq!(answered(resp), (ReplyState::AwaitingApproval, true));
    assert_eq!(
        answered(jimmys_node.show(&second, side).await),
        (ReplyState::Published, false)
    );

    for node in [kim, alice, dave, x] {
        node.shutdown().await.unwrap();
    }
}
