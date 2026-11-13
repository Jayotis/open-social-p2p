//! Replies, on the creator's node (ADR 0028). Kim posts. Alice is someone Kim
//! follows, Tom a trusted follower, Al someone Kim allowed; Jimmy only
//! follows Kim. Alice's, Tom's and Al's replies are published; Jimmy's waits
//! among Kim's requests, or is refused on a post for her circle only.
//!
//! Repliers here are peers made by hand, so each test can sign replies as any
//! identity. All nodes run in this process on loopback with relays and DNS
//! lookup off.

use std::{
    net::SocketAddr,
    path::Path,
    time::{Duration, SystemTime},
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey,
    address_lookup::MemoryLookup,
    endpoint::{Connection, presets},
};
use osp_node::{Config, Node};
use osp_proto::{
    Attachment, BlobHash, CertifiedKey, Delete, EarlierKeys, FollowTicket, MediaKind, Note,
    ObjectId, Payload, Person, SOCIAL_ALPN, SignedObject,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, ReplierChoice, ReplyAnswer, ReplyInfo, ReplyStatus},
    social::{ReplyPolicy, ReplyState, SocialRequest, SocialResponse},
};

async fn start(root: &Path, name: &str) -> Node {
    let config = Config::local_test(name);
    let home = root.join(name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn(&home).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

/// Posts `text` on `node`, and gives the post's ID.
async fn post(node: &Node, text: &str) -> ObjectId {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
        upload: None,
    };
    call(node, req).await;
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    assert_eq!(items[0].content, text);
    items[0].id
}

async fn thread(node: &Node, post: ObjectId) -> Vec<ReplyInfo> {
    match call(node, ClientRequest::Thread { post }).await {
        ClientResponse::Thread { replies, .. } => replies,
        other => panic!("unexpected {other:?}"),
    }
}

async fn requests(node: &Node) -> Vec<ReplyInfo> {
    match call(node, ClientRequest::ReplyRequests).await {
        ClientResponse::ReplyRequests { requests } => requests,
        other => panic!("unexpected {other:?}"),
    }
}

fn at(secs: i64) -> String {
    let now = SystemTime::now();
    let t = if secs >= 0 {
        now + Duration::from_secs(secs as u64)
    } else {
        now - Duration::from_secs(secs.unsigned_abs())
    };
    humantime::format_rfc3339_millis(t).to_string()
}

fn reply_note(identity: &SecretKey, post: ObjectId, text: &str) -> Payload {
    Payload::Note(Note {
        actor: identity.public(),
        content: text.into(),
        published: osp_proto::now_rfc3339(),
        attachment: vec![],
        in_reply_to: Some(post),
    })
}

/// A node made by hand that sends replies, connected to the creator's node.
/// Holds its endpoint, since dropping that closes the connection.
struct Replier {
    endpoint: Endpoint,
    conn: Connection,
}

impl Replier {
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

    fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    async fn ask(&self, req: &SocialRequest) -> SocialResponse {
        let (mut send, mut recv) = self.conn.open_bi().await.unwrap();
        write_frame(&mut send, req).await.unwrap();
        send.finish().unwrap();
        read_frame(&mut recv).await.unwrap()
    }

    async fn subscribe(&self) {
        let req = SocialRequest::Subscribe {
            addr: self.endpoint.addr(),
            shares_prefetched: false,
            media: None,
            direct: None,
        };
        assert!(matches!(self.ask(&req).await, SocialResponse::Ok));
    }

    /// `identity`'s profile, listing this node.
    fn profile(&self, identity: &SecretKey) -> SignedObject {
        profile_listing(identity, vec![self.id()])
    }

    /// Sends `reply` with `profile`, and gives the answer.
    async fn submit(&self, reply: SignedObject, profile: SignedObject) -> SocialResponse {
        let req = SocialRequest::SubmitReply { reply, profile };
        self.ask(&req).await
    }

    /// Replies to `post` as `identity`, and gives the reply and its state.
    async fn reply(
        &self,
        identity: &SecretKey,
        post: ObjectId,
        text: &str,
    ) -> (SignedObject, ReplyState) {
        let reply = SignedObject::sign(identity, &reply_note(identity, post, text)).unwrap();
        let state = state(self.submit(reply.clone(), self.profile(identity)).await);
        (reply, state)
    }

    async fn delete(&self, identity: &SecretKey, reply: &SignedObject) -> ReplyState {
        let delete = Payload::Delete(Delete {
            actor: identity.public(),
            object: reply.id(),
            published: osp_proto::now_rfc3339(),
        });
        let req = SocialRequest::DeleteReply {
            delete: SignedObject::sign(identity, &delete).unwrap(),
            profile: self.profile(identity),
        };
        state(self.ask(&req).await)
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

fn state(resp: SocialResponse) -> ReplyState {
    match resp {
        SocialResponse::Reply { state } => state,
        other => panic!("unexpected {other:?}"),
    }
}

fn refused(resp: SocialResponse) -> String {
    match resp {
        SocialResponse::Error { message } => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_creators_circle_is_published_and_strangers_wait_for_her() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let addr = kim.addr().await;
    let open = post(&kim, "anyone may answer").await;
    let close = post(&kim, "for my circle").await;
    call(
        &kim,
        ClientRequest::SetReplyPolicy {
            post: close,
            policy: ReplyPolicy::Circle,
        },
    )
    .await;

    // Alice: Kim follows her.
    let (alice, alice_node) = (SecretKey::generate(), Replier::new(&addr).await);
    let ticket = FollowTicket {
        actor: alice.public(),
        node: alice_node.endpoint.addr(),
    };
    call(
        &kim,
        ClientRequest::Follow {
            ticket: ticket.to_string(),
        },
    )
    .await;
    // Tom: a trusted follower.
    let (tom, tom_node) = (SecretKey::generate(), Replier::new(&addr).await);
    tom_node.subscribe().await;
    let ask = SocialRequest::AskTrust {
        profile: tom_node.profile(&tom),
        note: String::new(),
    };
    assert!(matches!(tom_node.ask(&ask).await, SocialResponse::Ok));
    call(
        &kim,
        ClientRequest::AnswerTrust {
            node: tom_node.id(),
            allow: true,
        },
    )
    .await;
    // Al: allowed by hand. Jimmy: only follows Kim. Bob: blocked.
    let (al, al_node) = (SecretKey::generate(), Replier::new(&addr).await);
    call(
        &kim,
        ClientRequest::SetReplier {
            actor: al.public(),
            choice: Some(ReplierChoice::Allow),
        },
    )
    .await;
    let (jimmy, jimmy_node) = (SecretKey::generate(), Replier::new(&addr).await);
    jimmy_node.subscribe().await;
    let (bob, bob_node) = (SecretKey::generate(), Replier::new(&addr).await);
    call(
        &kim,
        ClientRequest::SetReplier {
            actor: bob.public(),
            choice: Some(ReplierChoice::Block),
        },
    )
    .await;

    for (who, node, name) in [
        (&alice, &alice_node, "Alice"),
        (&tom, &tom_node, "Tom"),
        (&al, &al_node, "Al"),
    ] {
        let (_, on_open) = node.reply(who, open, &format!("{name} here")).await;
        assert_eq!(on_open, ReplyState::Published, "{name}");
        let (_, on_close) = node.reply(who, close, &format!("{name} again")).await;
        assert_eq!(on_close, ReplyState::Published, "{name}");
    }
    let (jimmys, on_open) = jimmy_node.reply(&jimmy, open, "Jimmy here").await;
    assert_eq!(on_open, ReplyState::AwaitingApproval);
    let (_, on_close) = jimmy_node.reply(&jimmy, close, "Jimmy again").await;
    assert_eq!(on_close, ReplyState::NotPublished);
    // Blocked gets the same answer as any other refusal.
    let (_, blocked) = bob_node.reply(&bob, open, "Bob here").await;
    assert_eq!(blocked, ReplyState::NotPublished);

    // Sending the same reply again gives its state, and makes no second copy.
    let again = jimmy_node
        .submit(jimmys.clone(), jimmy_node.profile(&jimmy))
        .await;
    assert_eq!(state(again), ReplyState::AwaitingApproval);
    let published: Vec<String> = thread(&kim, open)
        .await
        .into_iter()
        .map(|r| r.content)
        .collect();
    assert_eq!(published, ["Alice here", "Tom here", "Al here"]);
    let [request] = &requests(&kim).await[..] else {
        panic!("one request");
    };
    assert_eq!(request.id, jimmys.id());
    assert_eq!(request.state, ReplyStatus::Request);
    assert_eq!(
        request.why.as_deref(),
        Some("you don't follow them; they follow you")
    );

    // Kim publishes it once: Jimmy's next one waits again. Then publishes and
    // allows him: his next one is published at once.
    call(
        &kim,
        ClientRequest::AnswerReply {
            reply: jimmys.id(),
            answer: ReplyAnswer::Publish,
        },
    )
    .await;
    let again = jimmy_node
        .submit(jimmys.clone(), jimmy_node.profile(&jimmy))
        .await;
    assert_eq!(state(again), ReplyState::Published);
    let (second, waits) = jimmy_node.reply(&jimmy, open, "Jimmy, second").await;
    assert_eq!(waits, ReplyState::AwaitingApproval);
    call(
        &kim,
        ClientRequest::AnswerReply {
            reply: second.id(),
            answer: ReplyAnswer::PublishAndAllow,
        },
    )
    .await;
    let (_, now) = jimmy_node.reply(&jimmy, open, "Jimmy, third").await;
    assert_eq!(now, ReplyState::Published);

    // A declined request stays refused when sent again.
    let (carol, carol_node) = (SecretKey::generate(), Replier::new(&addr).await);
    let (carols, waits) = carol_node.reply(&carol, open, "Carol here").await;
    assert_eq!(waits, ReplyState::AwaitingApproval);
    call(
        &kim,
        ClientRequest::AnswerReply {
            reply: carols.id(),
            answer: ReplyAnswer::Decline,
        },
    )
    .await;
    let again = carol_node
        .submit(carols.clone(), carol_node.profile(&carol))
        .await;
    assert_eq!(state(again), ReplyState::NotPublished);
    assert!(requests(&kim).await.is_empty());

    // Blocking Jimmy hides what he published and refuses what's new.
    call(
        &kim,
        ClientRequest::SetReplier {
            actor: jimmy.public(),
            choice: Some(ReplierChoice::Block),
        },
    )
    .await;
    let shown: Vec<String> = thread(&kim, open)
        .await
        .into_iter()
        .filter(|r| r.state == ReplyStatus::Published)
        .map(|r| r.content)
        .collect();
    assert_eq!(shown, ["Alice here", "Tom here", "Al here"]);
    let (_, refused) = jimmy_node.reply(&jimmy, open, "Jimmy, fourth").await;
    assert_eq!(refused, ReplyState::NotPublished);

    // A post closed to replies says so.
    call(
        &kim,
        ClientRequest::SetReplyPolicy {
            post: open,
            policy: ReplyPolicy::Off,
        },
    )
    .await;
    let (_, closed) = alice_node.reply(&alice, open, "Alice, late").await;
    assert_eq!(closed, ReplyState::Closed);
}

#[tokio::test(flavor = "multi_thread")]
async fn replies_that_fail_their_checks_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let bob = start(dir.path(), "Bob").await;
    let addr = kim.addr().await;
    let kims = post(&kim, "Kim's post").await;
    // Kim reshares one of Bob's posts.
    let bobs = post(&bob, "Bob's post").await;
    let ClientResponse::Ticket { ticket } = call(&bob, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(&kim, ClientRequest::Follow { ticket }).await;
    call(&kim, ClientRequest::Sync).await;
    call(
        &kim,
        ClientRequest::Reshare {
            id: bobs,
            host: false,
        },
    )
    .await;
    let ClientResponse::Feed { items } = call(&kim, ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    let reshare = items[0].reshared_by.as_ref().expect("a reshare").announce;

    let jimmy = SecretKey::generate();
    let node = Replier::new(&addr).await;
    call(
        &kim,
        ClientRequest::SetReplier {
            actor: jimmy.public(),
            choice: Some(ReplierChoice::Allow),
        },
    )
    .await;
    let send = |payload: Payload| SignedObject::sign(&jimmy, &payload).unwrap();
    let reply = |post: ObjectId, text: &str| send(reply_note(&jimmy, post, text));

    // To a reshare, to a post Kim's node doesn't have, to a reply.
    let to_reshare = node.submit(reply(reshare, "hi"), node.profile(&jimmy));
    assert!(refused(to_reshare.await).contains("reshare"));
    let to_bob = node.submit(reply(bobs, "hi"), node.profile(&jimmy));
    assert!(refused(to_bob.await).contains("isn't one of this node's posts"));
    let first = reply(kims, "first");
    assert_eq!(
        state(node.submit(first.clone(), node.profile(&jimmy)).await),
        ReplyState::Published
    );
    let to_reply = node.submit(reply(first.id(), "hi"), node.profile(&jimmy));
    assert!(refused(to_reply.await).contains("isn't one of this node's posts"));

    // Not a reply, empty, too long, with an attachment.
    let mut plain = reply_note(&jimmy, kims, "hi");
    if let Payload::Note(n) = &mut plain {
        n.in_reply_to = None;
    }
    let not_a_reply = node.submit(send(plain), node.profile(&jimmy));
    assert!(refused(not_a_reply.await).contains("isn't a reply"));
    let empty = node.submit(reply(kims, " \n "), node.profile(&jimmy));
    assert!(refused(empty.await).contains("empty"));
    let long = "x".repeat(osp_proto::types::MAX_REPLY_BYTES + 1);
    let too_long = node.submit(reply(kims, &long), node.profile(&jimmy));
    assert!(refused(too_long.await).contains("longer"));
    let mut with_photo = reply_note(&jimmy, kims, "look");
    if let Payload::Note(n) = &mut with_photo {
        n.attachment.push(Attachment {
            kind: MediaKind::Image,
            media_type: "image/webp".into(),
            blob: BlobHash::from_bytes([7; 32]),
            size: 1000,
            width: 10,
            height: 10,
            duration_ms: None,
            name: None,
            thumbnail: None,
            profile: "v1-720p".into(),
        });
    }
    let attached = node.submit(send(with_photo), node.profile(&jimmy));
    assert!(refused(attached.await).contains("attachments"));

    // A profile that doesn't list the sending node, or is someone else's.
    let elsewhere = profile_listing(&jimmy, vec![SecretKey::generate().public()]);
    let unlisted = node.submit(reply(kims, "hi"), elsewhere);
    assert!(refused(unlisted.await).contains("doesn't list your node"));
    let someone = SecretKey::generate();
    let theirs = node.submit(reply(kims, "hi"), node.profile(&someone));
    assert!(refused(theirs.await).contains("isn't the replier's"));

    // Signed by a key Jimmy revoked: once Kim's node knows of the revocation,
    // from a reply it kept, the old key's replies are refused.
    let old = CertifiedKey::certify(
        &jimmy,
        SecretKey::generate(),
        at(-2 * 3600),
        EarlierKeys::Retired,
    )
    .unwrap();
    let new = CertifiedKey::certify(
        &jimmy,
        SecretKey::generate(),
        at(-3600),
        EarlierKeys::Revoked,
    )
    .unwrap();
    let by_new = SignedObject::sign_certified(&new, &reply_note(&jimmy, kims, "new key")).unwrap();
    assert_eq!(
        state(node.submit(by_new.clone(), node.profile(&jimmy)).await),
        ReplyState::Published
    );
    let by_old = SignedObject::sign_certified(&old, &reply_note(&jimmy, kims, "old key")).unwrap();
    let revoked = node.submit(by_old.clone(), node.profile(&jimmy));
    assert!(refused(revoked.await).contains("revoked"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_published_reply_stays_out_of_the_feed_and_search() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let kims = post(&kim, "Kim's post").await;
    let node = Replier::new(&kim.addr().await).await;
    let jimmy = SecretKey::generate();
    call(
        &kim,
        ClientRequest::SetReplier {
            actor: jimmy.public(),
            choice: Some(ReplierChoice::Allow),
        },
    )
    .await;
    let (reply, published) = node.reply(&jimmy, kims, "zebracorn sighting").await;
    assert_eq!(published, ReplyState::Published);
    assert_eq!(thread(&kim, kims).await[0].id, reply.id());

    let ClientResponse::Feed { items } = call(&kim, ClientRequest::Feed { limit: 50 }).await else {
        panic!("no feed");
    };
    assert!(items.iter().all(|i| i.id != reply.id()));
    let search = ClientRequest::Search {
        words: "zebracorn".into(),
        limit: 10,
    };
    match call(&kim, search).await {
        ClientResponse::Found { posts, .. } => assert!(posts.is_empty(), "{posts:?}"),
        other => panic!("unexpected {other:?}"),
    }
    let show = kim.handle(ClientRequest::Show { id: reply.id() }).await;
    assert!(matches!(show, ClientResponse::Error { .. }), "{show:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_deleted_and_closed_by_deleting_the_post() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let kims = post(&kim, "Kim's post").await;
    let node = Replier::new(&kim.addr().await).await;
    let jimmy = SecretKey::generate();
    call(
        &kim,
        ClientRequest::SetReplier {
            actor: jimmy.public(),
            choice: Some(ReplierChoice::Allow),
        },
    )
    .await;
    let (first, _) = node.reply(&jimmy, kims, "first").await;
    let (second, _) = node.reply(&jimmy, kims, "second").await;

    // Hiding takes a reply out of the thread without a `Delete`; it can be
    // shown again, at the end.
    let hide = |reply: ObjectId, hidden: bool| ClientRequest::HideReply { reply, hidden };
    call(&kim, hide(first.id(), true)).await;
    let states: Vec<(String, ReplyStatus)> = thread(&kim, kims)
        .await
        .into_iter()
        .map(|r| (r.content, r.state))
        .collect();
    assert_eq!(
        states,
        [
            ("first".to_string(), ReplyStatus::Hidden),
            ("second".to_string(), ReplyStatus::Published)
        ]
    );
    let resent = node.submit(first.clone(), node.profile(&jimmy)).await;
    assert_eq!(state(resent), ReplyState::NotPublished);
    call(&kim, hide(first.id(), false)).await;
    let order: Vec<String> = thread(&kim, kims)
        .await
        .into_iter()
        .map(|r| r.content)
        .collect();
    assert_eq!(order, ["second", "first"]);

    // Jimmy deletes his second reply: it goes, and sending it again doesn't
    // bring it back.
    assert_eq!(node.delete(&jimmy, &second).await, ReplyState::Deleted);
    let resent = node.submit(second.clone(), node.profile(&jimmy)).await;
    assert_eq!(state(resent), ReplyState::Deleted);
    let left: Vec<ObjectId> = thread(&kim, kims).await.into_iter().map(|r| r.id).collect();
    assert_eq!(left, [first.id()]);
    // Someone else can't delete his first.
    let mallory = SecretKey::generate();
    let forged = Payload::Delete(Delete {
        actor: mallory.public(),
        object: first.id(),
        published: osp_proto::now_rfc3339(),
    });
    let req = SocialRequest::DeleteReply {
        delete: SignedObject::sign(&mallory, &forged).unwrap(),
        profile: node.profile(&mallory),
    };
    assert_eq!(state(node.ask(&req).await), ReplyState::Deleted);
    assert_eq!(thread(&kim, kims).await.len(), 1);

    // Kim deletes her post: its thread is closed, and replies to it are
    // answered as deleted.
    call(&kim, ClientRequest::Delete { id: kims }).await;
    let (_, late) = node.reply(&jimmy, kims, "late").await;
    assert_eq!(late, ReplyState::Deleted);
    let gone = kim.handle(ClientRequest::Thread { post: kims }).await;
    assert!(matches!(gone, ClientResponse::Error { .. }), "{gone:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flood_of_replies_stops_at_the_limits() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let addr = kim.addr().await;
    let kims = post(&kim, "Kim's post").await;

    // One identity has at most 3 requests waiting.
    let node = Replier::new(&addr).await;
    let jimmy = SecretKey::generate();
    for i in 0..3 {
        let (_, waits) = node.reply(&jimmy, kims, &format!("Jimmy {i}")).await;
        assert_eq!(waits, ReplyState::AwaitingApproval);
    }
    let (_, over) = node.reply(&jimmy, kims, "Jimmy 3").await;
    assert_eq!(over, ReplyState::NotPublished);

    // One node sends at most 20 new replies an hour, whoever they're from:
    // 4 sent above, 16 more go through, the next doesn't.
    for i in 0..16 {
        let (_, waits) = node
            .reply(&SecretKey::generate(), kims, &format!("throwaway {i}"))
            .await;
        assert_eq!(waits, ReplyState::AwaitingApproval, "{i}");
    }
    let (_, over) = node.reply(&SecretKey::generate(), kims, "one more").await;
    assert_eq!(over, ReplyState::NotPublished);

    // At most 100 requests wait in all: from more nodes, the 101st is
    // refused, and none waiting is pushed out.
    let mut waiting = 3 + 16;
    while waiting < 100 {
        let node = Replier::new(&addr).await;
        for _ in 0..20.min(100 - waiting) {
            let (_, waits) = node.reply(&SecretKey::generate(), kims, "flood").await;
            assert_eq!(waits, ReplyState::AwaitingApproval, "{waiting}");
            waiting += 1;
        }
    }
    let node = Replier::new(&addr).await;
    let (_, over) = node.reply(&SecretKey::generate(), kims, "flood").await;
    assert_eq!(over, ReplyState::NotPublished);
    let kept = requests(&kim).await;
    assert_eq!(kept.len(), 100);
    assert!(kept.iter().any(|r| r.content == "Jimmy 0"));

    // Kim's circle still gets in.
    let al = SecretKey::generate();
    call(
        &kim,
        ClientRequest::SetReplier {
            actor: al.public(),
            choice: Some(ReplierChoice::Allow),
        },
    )
    .await;
    let (_, published) = node.reply(&al, kims, "Al here").await;
    assert_eq!(published, ReplyState::Published);
}
