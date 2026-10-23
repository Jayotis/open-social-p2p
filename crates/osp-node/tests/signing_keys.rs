//! Objects signed by certified keys, and what readers do when an identity
//! retires or revokes a key (ADR 0010).
//!
//! The reader is a real node. The identities it follows are played by the
//! test, with small nodes that serve whatever the test gives them, since a
//! real node doesn't sign with a certified key yet. Everything runs in this
//! process on loopback with relays and DNS lookup off.

use std::{
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use iroh::{
    Endpoint, EndpointAddr, RelayMode,
    address_lookup::MemoryLookup,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use osp_node::{Config, Node};
use osp_proto::{
    Announce, CertifiedKey, EarlierKeys, FollowTicket, Note, Payload, Person, SOCIAL_ALPN,
    SecretKey, SignedObject,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, FollowInfo},
    social::{OutboxEntry, SocialRequest, SocialResponse},
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

/// Syncs, then returns the texts in the feed, newest first.
async fn feed_texts(node: &Node) -> Vec<String> {
    call(node, ClientRequest::Sync).await;
    match call(node, ClientRequest::Feed { limit: 50 }).await {
        ClientResponse::Feed { items } => items.into_iter().map(|i| i.content).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

async fn following(node: &Node) -> Vec<FollowInfo> {
    match call(node, ClientRequest::Following).await {
        ClientResponse::Following { follows } => follows,
        other => panic!("unexpected {other:?}"),
    }
}

/// `secs` from now (earlier when negative), written as a `published` date.
fn at(secs: i64) -> String {
    let now = SystemTime::now();
    let t = if secs >= 0 {
        now + Duration::from_secs(secs as u64)
    } else {
        now - Duration::from_secs(secs.unsigned_abs())
    };
    humantime::format_rfc3339_millis(t).to_string()
}

/// A new signing key for `identity`, its certificate dated `secs` from now.
fn certify(identity: &SecretKey, secs: i64, earlier_keys: EarlierKeys) -> CertifiedKey {
    CertifiedKey::certify(identity, SecretKey::generate(), at(secs), earlier_keys).unwrap()
}

fn note(identity: &SecretKey, text: &str) -> Payload {
    Payload::Note(Note {
        actor: identity.public(),
        content: text.into(),
        published: osp_proto::now_rfc3339(),
        attachment: vec![],
        in_reply_to: None,
    })
}

fn profile(identity: &SecretKey, nodes: Vec<iroh::EndpointId>, secs: i64) -> Payload {
    Payload::Person(Person {
        actor: identity.public(),
        name: "Alice".into(),
        summary: None,
        nodes,
        published: at(secs),
    })
}

fn signed(key: &CertifiedKey, payload: &Payload) -> SignedObject {
    SignedObject::sign_certified(key, payload).unwrap()
}

/// What a test-run node serves: a profile, and an outbox of posts, each
/// with the objects bundled with it (what a reshare points at).
#[derive(Default)]
struct Served {
    profile: Option<SignedObject>,
    posts: Vec<(SignedObject, Vec<SignedObject>)>,
}

/// A node run by the test for an identity. It serves whatever is in `served`.
#[derive(Clone, Default)]
struct Author {
    served: Arc<Mutex<Served>>,
}

impl std::fmt::Debug for Author {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Author")
    }
}

impl ProtocolHandler for Author {
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

impl Author {
    fn answer(&self, req: SocialRequest) -> SocialResponse {
        let served = self.served.lock().unwrap();
        match req {
            SocialRequest::GetProfile => SocialResponse::Profile {
                profile: served.profile.clone().expect("a profile to serve"),
            },
            SocialRequest::GetOutbox { since, limit } => {
                let page: Vec<_> = (1..)
                    .zip(&served.posts)
                    .filter(|(seq, _)| *seq > since)
                    .take(limit as usize)
                    .collect();
                SocialResponse::Outbox {
                    entries: page
                        .iter()
                        .map(|(seq, (object, _))| OutboxEntry {
                            seq: *seq,
                            object: object.clone(),
                        })
                        .collect(),
                    referenced: page.iter().flat_map(|(_, (_, b))| b.clone()).collect(),
                    latest: served.posts.len() as u64,
                }
            }
            _ => SocialResponse::Ok,
        }
    }

    fn set_profile(&self, profile: SignedObject) {
        self.served.lock().unwrap().profile = Some(profile);
    }

    fn post(&self, post: SignedObject) {
        self.served.lock().unwrap().posts.push((post, vec![]));
    }

    /// Reshares `post`, signed by `resharer` itself, bundling the post.
    fn reshare(&self, resharer: &SecretKey, post: &SignedObject) {
        let announce = Payload::Announce(Announce {
            actor: resharer.public(),
            object: post.id(),
            object_actor: post.signer(),
            published: osp_proto::now_rfc3339(),
            hosting: false,
        });
        let announce = SignedObject::sign(resharer, &announce).unwrap();
        self.served
            .lock()
            .unwrap()
            .posts
            .push((announce, vec![post.clone()]));
    }
}

/// Runs an `Author` on loopback, able to reach `reader`. Keep the router:
/// dropping it stops the node.
async fn serve(author: Author, reader: &EndpointAddr) -> Router {
    let lookup = MemoryLookup::new();
    lookup.add_endpoint_info(reader.clone());
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
    Router::builder(endpoint)
        .accept(SOCIAL_ALPN, author)
        .spawn()
}

async fn ask(from: &Router, to: &EndpointAddr, req: &SocialRequest) -> SocialResponse {
    let conn = from
        .endpoint()
        .connect(to.clone(), SOCIAL_ALPN)
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, req).await.unwrap();
    send.finish().unwrap();
    tokio::time::timeout(Duration::from_secs(5), read_frame(&mut recv))
        .await
        .expect("a prompt reply")
        .unwrap()
}

/// Asserts the reply is an error containing `expected`.
fn refused(resp: SocialResponse, expected: &str) {
    match resp {
        SocialResponse::Error { message } => {
            assert!(message.contains(expected), "{message:?}, not {expected:?}")
        }
        other => panic!("expected a refusal ({expected}), got {other:?}"),
    }
}

async fn follow(reader: &Node, identity: &SecretKey, node: &Router) {
    let ticket = FollowTicket {
        actor: identity.public(),
        node: node.endpoint().addr(),
    }
    .to_string();
    call(reader, ClientRequest::Follow { ticket }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_key_is_refused_and_a_retired_one_isnt() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let bob = start(dir.path(), "Bob").await;
    let alice = SecretKey::generate();
    let author = Author::default();
    let node = serve(author.clone(), &bob.addr().await).await;

    // Alice's first signing key, certified three hours ago.
    let first = certify(&alice, -3 * 3600, EarlierKeys::Retired);
    author.set_profile(signed(
        &first,
        &profile(&alice, vec![node.endpoint().id()], -3 * 3600),
    ));
    author.post(signed(&first, &note(&alice, "first key")));
    // Something signed by the identity key itself, as everything was before ADR 0010.
    author.post(SignedObject::sign(&alice, &note(&alice, "identity key")).unwrap());
    follow(&bob, &alice, &node).await;
    let mut feed = feed_texts(&bob).await;
    feed.sort();
    assert_eq!(feed, ["first key", "identity key"]);

    // She revokes it: her new key's certificate says the earlier ones may
    // have been copied. What the first key signs from now on is refused.
    let second = certify(&alice, -2 * 3600, EarlierKeys::Revoked);
    author.set_profile(signed(
        &second,
        &profile(&alice, vec![node.endpoint().id()], -2 * 3600),
    ));
    author.post(signed(&second, &note(&alice, "second key")));
    author.post(signed(&first, &note(&alice, "a thief with the first key")));
    let feed = feed_texts(&bob).await;
    assert!(feed.contains(&"second key".to_string()), "{feed:?}");
    assert!(!feed.iter().any(|t| t.contains("thief")), "{feed:?}");

    // Later she only retires the second key: what it signs stays valid.
    let third = certify(&alice, -3600, EarlierKeys::Retired);
    author.post(signed(&third, &note(&alice, "third key")));
    author.post(signed(&second, &note(&alice, "second key, later")));
    // The identity key itself is never revoked.
    author.post(SignedObject::sign(&alice, &note(&alice, "identity key, later")).unwrap());
    // The revocation still holds.
    author.post(signed(&first, &note(&alice, "the thief again")));
    // A certificate dated an hour ahead of Bob's clock is refused.
    let ahead = certify(&alice, 3600, EarlierKeys::Retired);
    author.post(signed(&ahead, &note(&alice, "from the future")));
    let feed = feed_texts(&bob).await;
    for text in ["third key", "second key, later", "identity key, later"] {
        assert!(
            feed.contains(&text.to_string()),
            "{text:?} missing from {feed:?}"
        );
    }
    for text in ["the thief again", "from the future"] {
        assert!(!feed.contains(&text.to_string()), "{text:?} in {feed:?}");
    }

    // A profile signed by the revoked key is refused too, so syncing from a
    // node that serves one fails.
    author.set_profile(signed(
        &first,
        &profile(&alice, vec![node.endpoint().id()], 0),
    ));
    call(&bob, ClientRequest::Sync).await;
    let error = following(&bob).await[0]
        .last_error
        .clone()
        .unwrap_or_default();
    assert!(error.contains("revoked"), "{error:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn after_a_revocation_a_thief_cant_move_followers_back() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let bob = start(dir.path(), "Bob").await;
    let bob_addr = bob.addr().await;
    let alice = SecretKey::generate();
    let stolen = certify(&alice, -3600, EarlierKeys::Retired);

    // Bob follows Alice's node.
    let (home, thiefs, new) = (Author::default(), Author::default(), Author::default());
    let home_node = serve(home.clone(), &bob_addr).await;
    let thief_node = serve(thiefs.clone(), &bob_addr).await;
    let new_node = serve(new.clone(), &bob_addr).await;
    home.set_profile(signed(
        &stolen,
        &profile(&alice, vec![home_node.endpoint().id()], -3600),
    ));
    follow(&bob, &alice, &home_node).await;
    feed_texts(&bob).await;

    // Someone copies her signing key and moves Bob's follow to their node.
    let thiefs_profile = signed(
        &stolen,
        &profile(&alice, vec![thief_node.endpoint().id()], -60),
    );
    thiefs.set_profile(thiefs_profile.clone());
    thiefs.post(signed(&stolen, &note(&alice, "posted by the thief")));
    let moved = |profile, node: &Router| SocialRequest::Moved {
        profile,
        addr: node.endpoint().addr(),
    };
    let req = moved(thiefs_profile, &thief_node);
    assert!(matches!(
        ask(&thief_node, &bob_addr, &req).await,
        SocialResponse::Ok
    ));
    assert_eq!(feed_texts(&bob).await, ["posted by the thief"]);

    // Alice takes her identity back with her words: a new key that revokes
    // the stolen one, on a new node.
    let taken_back = certify(&alice, 0, EarlierKeys::Revoked);
    let new_profile = signed(
        &taken_back,
        &profile(&alice, vec![new_node.endpoint().id()], 0),
    );
    new.set_profile(new_profile.clone());
    new.post(signed(&taken_back, &note(&alice, "it's really me")));
    let req = moved(new_profile, &new_node);
    assert!(matches!(
        ask(&new_node, &bob_addr, &req).await,
        SocialResponse::Ok
    ));
    assert!(
        feed_texts(&bob)
            .await
            .contains(&"it's really me".to_string())
    );

    // The thief answers with a newer profile, signed with the stolen key.
    // Bob refuses it and stays with Alice's new node.
    let newer = signed(
        &stolen,
        &profile(&alice, vec![thief_node.endpoint().id()], 60),
    );
    let req = moved(newer, &thief_node);
    refused(ask(&thief_node, &bob_addr, &req).await, "revoked");
    thiefs.post(signed(&stolen, &note(&alice, "the thief again")));
    let feed = feed_texts(&bob).await;
    assert!(!feed.contains(&"the thief again".to_string()), "{feed:?}");
    assert!(following(&bob).await[0].last_error.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn news_of_a_revocation_travels_with_a_reshare() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let bob = start(dir.path(), "Bob").await;
    let bob_addr = bob.addr().await;
    let (alice, carol) = (SecretKey::generate(), SecretKey::generate());
    let (alices, carols) = (Author::default(), Author::default());
    let alice_node = serve(alices.clone(), &bob_addr).await;
    let carol_node = serve(carols.clone(), &bob_addr).await;

    // Bob follows Alice, whose node still signs with a key she has since
    // revoked, and Carol.
    let old = certify(&alice, -3600, EarlierKeys::Retired);
    alices.set_profile(signed(
        &old,
        &profile(&alice, vec![alice_node.endpoint().id()], -3600),
    ));
    let carols_profile = Payload::Person(Person {
        actor: carol.public(),
        name: "Carol".into(),
        summary: None,
        nodes: vec![carol_node.endpoint().id()],
        published: at(-3600),
    });
    carols.set_profile(SignedObject::sign(&carol, &carols_profile).unwrap());
    follow(&bob, &alice, &alice_node).await;
    follow(&bob, &carol, &carol_node).await;
    feed_texts(&bob).await;
    assert!(following(&bob).await.iter().all(|f| f.last_error.is_none()));

    // Carol reshares a post signed with Alice's new key. Bob learns of the
    // revocation from the post bundled with the reshare.
    let new = certify(&alice, 0, EarlierKeys::Revoked);
    carols.reshare(&carol, &signed(&new, &note(&alice, "new key")));
    assert!(feed_texts(&bob).await.contains(&"new key".to_string()));
    // Follows sync side by side, so Alice's may have finished first. On the
    // next sync, her old node's profile is refused.
    call(&bob, ClientRequest::Sync).await;
    let alices_follow = following(&bob)
        .await
        .into_iter()
        .find(|f| f.actor == alice.public())
        .unwrap();
    let error = alices_follow.last_error.unwrap_or_default();
    assert!(error.contains("revoked"), "{error:?}");

    // A post signed with the old key, bundled with a reshare, is dropped.
    carols.reshare(&carol, &signed(&old, &note(&alice, "old key")));
    let feed = feed_texts(&bob).await;
    assert!(!feed.contains(&"old key".to_string()), "{feed:?}");
}
