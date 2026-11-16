//! Likes (ADR 0038). Kim posts; Jimmy follows her and likes it. Kim's node
//! shows her who; anyone reading the thread sees how many. A like changes
//! nothing else.
//!
//! The first tests run two nodes on the simulated network, so a node that
//! restarts is reached by its ID as before. The rest send likes from nodes
//! made by hand, on loopback, so each can sign as anyone.

use std::{net::SocketAddr, path::Path, time::Duration};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey,
    address_lookup::MemoryLookup,
    endpoint::{Connection, presets},
};
use osp_node::{
    Config, Node, SimSettings,
    simnet::{Link, SimNet},
};
use osp_proto::{
    Like, Note, ObjectId, Payload, Person, SOCIAL_ALPN, SignedObject, Undo,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, LikeInfo, ReplierChoice},
    social::{SocialRequest, SocialResponse},
};

fn sim(net: &SimNet) -> SimSettings {
    SimSettings {
        network: Some(net.link(Link::default())),
        ..SimSettings::default()
    }
}

fn home(root: &Path, name: &str) -> std::path::PathBuf {
    root.join(name.to_lowercase())
}

/// Someone on the simulated network, with media and direct endpoints.
async fn start(root: &Path, net: &SimNet, name: &str) -> Node {
    let mut config = Config::local_test(name);
    config.endpoints.enabled = true;
    Node::init(&home(root, name), &config).unwrap();
    restart(root, net, name).await
}

async fn restart(root: &Path, net: &SimNet, name: &str) -> Node {
    Node::spawn_sim(&home(root, name), sim(net)).await.unwrap()
}

/// Someone on loopback, for the nodes made by hand to reach.
async fn start_local(root: &Path, name: &str) -> Node {
    let config = Config::local_test(name);
    Node::init(&home(root, name), &config).unwrap();
    Node::spawn(&home(root, name)).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn refused(node: &Node, req: ClientRequest) -> String {
    match node.handle(req).await {
        ClientResponse::Error { message } => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn post(node: &Node, text: &str) -> ObjectId {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
        upload: None,
    };
    match call(node, req).await {
        ClientResponse::Posted { id } => id,
        other => panic!("unexpected {other:?}"),
    }
}

/// `follower` follows `creator`, and has their posts.
async fn follow(follower: &Node, creator: &Node) {
    let ClientResponse::Ticket { ticket } = call(creator, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(follower, ClientRequest::Follow { ticket }).await;
    call(follower, ClientRequest::Sync).await;
}

async fn like(node: &Node, post: ObjectId, liked: bool) {
    call(node, ClientRequest::Like { post, liked }).await;
}

async fn likes(node: &Node, post: ObjectId) -> Vec<LikeInfo> {
    match call(node, ClientRequest::Likes { post }).await {
        ClientResponse::Likes { likes } => likes,
        other => panic!("unexpected {other:?}"),
    }
}

/// How many like `post`, and whether this node's owner does, as its thread
/// says.
async fn counted(node: &Node, post: ObjectId) -> (u64, bool) {
    match call(node, ClientRequest::Thread { post, after: 0 }).await {
        ClientResponse::Thread { likes, liked, .. } => (likes, liked),
        other => panic!("unexpected {other:?}"),
    }
}

/// Waits until `check` passes on Kim's list of who likes `post`.
async fn wait_likes(
    kim: &Node,
    post: ObjectId,
    check: impl Fn(&[LikeInfo]) -> bool,
) -> Vec<LikeInfo> {
    let mut last = Vec::new();
    for _ in 0..60 {
        last = likes(kim, post).await;
        if check(&last) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("Kim's likes stayed {last:?}");
}

async fn feed_ids(node: &Node) -> Vec<ObjectId> {
    match call(node, ClientRequest::Feed { limit: 50 }).await {
        ClientResponse::Feed { items } => items.iter().map(|i| i.id).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_like_and_its_undo_reach_the_creator_and_readers_see_how_many() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(90);
    let kim = start(dir.path(), &net, "Kim").await;
    let jimmy = start(dir.path(), &net, "Jimmy").await;
    let kims = post(&kim, "Kim's post").await;
    follow(&jimmy, &kim).await;
    let kims_feed = feed_ids(&kim).await;
    let jimmys_feed = feed_ids(&jimmy).await;

    // Jimmy likes it. Kim's node names him; Jimmy reads the count, not who.
    like(&jimmy, kims, true).await;
    let liked = wait_likes(&kim, kims, |l| l.len() == 1).await;
    assert_eq!(liked[0].name, "Jimmy");
    assert_eq!(counted(&jimmy, kims).await, (1, true));
    assert_eq!(counted(&kim, kims).await.0, 1);
    // Only Kim's node says who.
    let why = refused(&jimmy, ClientRequest::Likes { post: kims }).await;
    assert!(why.contains("only its creator's node"), "{why}");
    // Nothing else changed: no feed holds a like (ADR 0038 §2).
    assert_eq!(feed_ids(&kim).await, kims_feed);
    assert_eq!(feed_ids(&jimmy).await, jimmys_feed);

    // Liking again sends nothing new; taking it back reaches Kim.
    like(&jimmy, kims, true).await;
    like(&jimmy, kims, false).await;
    wait_likes(&kim, kims, |l| l.is_empty()).await;
    assert_eq!(counted(&jimmy, kims).await, (0, false));

    // Liked again: back.
    like(&jimmy, kims, true).await;
    wait_likes(&kim, kims, |l| l.len() == 1).await;

    // Kim can't like her own post.
    let why = refused(
        &kim,
        ClientRequest::Like {
            post: kims,
            liked: true,
        },
    )
    .await;
    assert!(why.contains("your own post"), "{why}");

    // Kim deletes the post: its likes go with it.
    call(&kim, ClientRequest::Delete { id: kims }).await;
    match kim.handle(ClientRequest::Likes { post: kims }).await {
        ClientResponse::Error { .. } => {}
        ClientResponse::Likes { likes } => assert!(likes.is_empty(), "{likes:?}"),
        other => panic!("unexpected {other:?}"),
    }

    for node in [jimmy, kim] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_like_while_the_creator_is_off_arrives_once_it_is_back() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(91);
    let kim = start(dir.path(), &net, "Kim").await;
    let jimmy = start(dir.path(), &net, "Jimmy").await;
    let kims = post(&kim, "Kim's post").await;
    follow(&jimmy, &kim).await;

    // Kim's node is off: Jimmy's like is kept, through his node's restart.
    kim.shutdown().await.unwrap();
    like(&jimmy, kims, true).await;
    jimmy.shutdown().await.unwrap();
    let jimmy = restart(dir.path(), &net, "Jimmy").await;

    // Kim's node is back, and `osp sync` tries at once.
    let kim = restart(dir.path(), &net, "Kim").await;
    call(&jimmy, ClientRequest::Sync).await;
    wait_likes(&kim, kims, |l| l.len() == 1).await;

    for node in [jimmy, kim] {
        node.shutdown().await.unwrap();
    }
}

/// A node made by hand that sends likes, connected to the creator's node.
/// Holds its endpoint, since dropping that closes the connection.
struct Liker {
    endpoint: Endpoint,
    conn: Connection,
}

impl Liker {
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

    /// `identity`'s profile, listing `nodes`.
    fn profile(&self, identity: &SecretKey, nodes: Vec<EndpointId>) -> SignedObject {
        let person = Payload::Person(Person {
            actor: identity.public(),
            name: "Someone".into(),
            summary: None,
            discoverable: false,
            nodes,
            media_endpoints: vec![],
            published: osp_proto::now_rfc3339(),
            icon: None,
            attachment: Vec::new(),
        });
        SignedObject::sign(identity, &person).unwrap()
    }

    /// Sends `identity`'s like of `post`, dated `published`.
    async fn like_at(
        &self,
        identity: &SecretKey,
        post: ObjectId,
        published: String,
    ) -> (SignedObject, SocialResponse) {
        let like = Payload::Like(Like {
            actor: identity.public(),
            object: post,
            published,
        });
        let like = SignedObject::sign(identity, &like).unwrap();
        let req = SocialRequest::Like {
            like: like.clone(),
            profile: self.profile(identity, vec![self.id()]),
        };
        let answer = self.ask(&req).await;
        (like, answer)
    }

    async fn like(&self, identity: &SecretKey, post: ObjectId) -> SocialResponse {
        self.like_at(identity, post, osp_proto::now_rfc3339())
            .await
            .1
    }

    async fn unlike(&self, identity: &SecretKey, like: &SignedObject) -> SocialResponse {
        let undo = Payload::Undo(Undo {
            actor: identity.public(),
            object: like.id(),
            published: osp_proto::now_rfc3339(),
        });
        let req = SocialRequest::Unlike {
            undo: SignedObject::sign(identity, &undo).unwrap(),
            profile: self.profile(identity, vec![self.id()]),
        };
        self.ask(&req).await
    }
}

fn ago(secs: u64) -> String {
    let t = std::time::SystemTime::now() - Duration::from_secs(secs);
    humantime::format_rfc3339_millis(t).to_string()
}

fn is_ok(resp: &SocialResponse) -> bool {
    matches!(resp, SocialResponse::Ok)
}

fn error(resp: SocialResponse) -> String {
    match resp {
        SocialResponse::Error { message } => message,
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn likes_that_fail_their_checks_are_refused_and_a_newer_one_replaces_an_older() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start_local(dir.path(), "Kim").await;
    let addr = kim.addr().await;
    let kims = post(&kim, "Kim's post").await;
    let node = Liker::new(&addr).await;
    let jimmy = SecretKey::generate();

    // Not one of Kim's posts.
    let why = error(node.like(&jimmy, ObjectId::of(b"elsewhere")).await);
    assert!(why.contains("isn't one of this node's posts"), "{why}");
    // A profile that doesn't list the node it came from.
    let like = Payload::Like(Like {
        actor: jimmy.public(),
        object: kims,
        published: osp_proto::now_rfc3339(),
    });
    let req = SocialRequest::Like {
        like: SignedObject::sign(&jimmy, &like).unwrap(),
        profile: node.profile(&jimmy, vec![]),
    };
    let why = error(node.ask(&req).await);
    assert!(why.contains("doesn't list your node"), "{why}");
    // Someone else's like, with Jimmy's profile.
    let alice = SecretKey::generate();
    let like = Payload::Like(Like {
        actor: alice.public(),
        object: kims,
        published: osp_proto::now_rfc3339(),
    });
    let req = SocialRequest::Like {
        like: SignedObject::sign(&alice, &like).unwrap(),
        profile: node.profile(&jimmy, vec![node.id()]),
    };
    let why = error(node.ask(&req).await);
    assert!(why.contains("isn't the replier's"), "{why}");
    // A post sent as a like.
    let note = Payload::Note(Note {
        actor: jimmy.public(),
        content: "not a like".into(),
        published: osp_proto::now_rfc3339(),
        attachment: vec![],
        in_reply_to: None,
    });
    let req = SocialRequest::Like {
        like: SignedObject::sign(&jimmy, &note).unwrap(),
        profile: node.profile(&jimmy, vec![node.id()]),
    };
    assert!(error(node.ask(&req).await).contains("isn't a like"));
    assert!(likes(&kim, kims).await.is_empty());

    // A like, then a newer one in its place, then an older one, which
    // changes nothing.
    let (first, answer) = node.like_at(&jimmy, kims, ago(60)).await;
    assert!(is_ok(&answer));
    let (newer, answer) = node.like_at(&jimmy, kims, ago(30)).await;
    assert!(is_ok(&answer));
    let (_, answer) = node.like_at(&jimmy, kims, ago(90)).await;
    assert!(is_ok(&answer));
    assert_eq!(likes(&kim, kims).await.len(), 1);
    // Undoing the first, which was replaced, leaves the newer.
    assert!(is_ok(&node.unlike(&jimmy, &first).await));
    assert_eq!(likes(&kim, kims).await.len(), 1);
    assert!(is_ok(&node.unlike(&jimmy, &newer).await));
    assert!(likes(&kim, kims).await.is_empty());
    // An undo of a like never held is answered the same.
    assert!(is_ok(&node.unlike(&jimmy, &newer).await));

    // Someone Kim blocked is answered as anyone, and not counted.
    let bob = SecretKey::generate();
    call(
        &kim,
        ClientRequest::SetReplier {
            actor: bob.public(),
            choice: Some(ReplierChoice::Block),
        },
    )
    .await;
    assert!(is_ok(&node.like(&bob, kims).await));
    assert!(likes(&kim, kims).await.is_empty());
    kim.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flood_of_likes_stops_at_the_limits_and_is_told_to_wait() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start_local(dir.path(), "Kim").await;
    let addr = kim.addr().await;
    let kims = post(&kim, "Kim's post").await;

    // 60 new likes an hour from one node; the next is told to wait.
    let node = Liker::new(&addr).await;
    for _ in 0..60 {
        assert!(is_ok(&node.like(&SecretKey::generate(), kims).await));
    }
    let why = error(node.like(&SecretKey::generate(), kims).await);
    assert!(why.contains("busy"), "{why}");
    // A like already held isn't counted again.
    let held = SecretKey::generate();
    let other = Liker::new(&addr).await;
    let (like, answer) = other.like_at(&held, kims, ago(10)).await;
    assert!(is_ok(&answer));
    let resend = SocialRequest::Like {
        like,
        profile: other.profile(&held, vec![other.id()]),
    };
    for _ in 0..80 {
        assert!(is_ok(&other.ask(&resend).await));
    }

    // 2,000 an hour from everyone together: 60 per node from 32 more
    // nodes, and the rest are told to wait.
    let mut taken = 61;
    let mut told_to_wait = 0;
    for _ in 0..33 {
        let node = Liker::new(&addr).await;
        for _ in 0..60 {
            match node.like(&SecretKey::generate(), kims).await {
                SocialResponse::Ok => taken += 1,
                resp => {
                    assert!(error(resp).contains("busy"));
                    told_to_wait += 1;
                }
            }
        }
    }
    assert_eq!(taken, 2_000);
    assert_eq!(told_to_wait, 33 * 60 - (2_000 - 61));
    assert_eq!(likes(&kim, kims).await.len(), 2_000);
    kim.shutdown().await.unwrap();
}
