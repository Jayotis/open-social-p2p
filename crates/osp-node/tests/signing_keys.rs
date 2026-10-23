//! Signing keys (ADR 0010): how a node keeps its keys, objects signed by
//! certified keys, and what readers do when an identity retires or revokes
//! a key.
//!
//! Where a test needs objects a real node wouldn't sign, the identities a
//! node follows are played by the test, with small nodes that serve
//! whatever the test gives them. Everything runs in this process on
//! loopback with relays and DNS lookup off.

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
use osp_node::{Config, Home, Node, backup};
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

async fn post(node: &Node, text: &str) {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
    };
    call(node, req).await;
}

async fn follow_node(follower: &Node, followed: &Node) {
    let ClientResponse::Ticket { ticket } = call(followed, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    call(follower, ClientRequest::Follow { ticket }).await;
}

async fn problems(node: &Node) -> Vec<String> {
    match call(node, ClientRequest::Status).await {
        ClientResponse::Status(status) => status.problems,
        other => panic!("unexpected {other:?}"),
    }
}

/// Writes a backup file, without media. Returns its format version.
async fn export(node: &Node, path: &Path) -> u8 {
    let req = ClientRequest::ExportBackup {
        path: path.into(),
        media: false,
    };
    call(node, req).await;
    // After the 9 bytes of "OSPBACKUP".
    std::fs::read(path).unwrap()[9]
}

/// Asks a real node something over `osp/social/1`, as a node it doesn't know.
async fn ask_node(node: &Node, req: &SocialRequest) -> SocialResponse {
    let addr = node.addr().await;
    let stranger = serve(Author::default(), &addr).await;
    ask(&stranger, &addr, req).await
}

async fn profile_of(node: &Node) -> SignedObject {
    match ask_node(node, &SocialRequest::GetProfile).await {
        SocialResponse::Profile { profile } => profile,
        other => panic!("unexpected {other:?}"),
    }
}

/// The objects in a node's outbox, oldest first.
async fn outbox(node: &Node) -> Vec<SignedObject> {
    let req = SocialRequest::GetOutbox {
        since: 0,
        limit: 100,
    };
    match ask_node(node, &req).await {
        SocialResponse::Outbox { entries, .. } => entries.into_iter().map(|e| e.object).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

/// Turns a newly made home into one made before ADR 0010: the identity key
/// in `identity.key`, and no signing key.
fn make_old_home(home: &Path, words: &str) {
    let files = Home::new(home);
    for file in [
        files.signing_key(),
        files.signing_cert(),
        files.backup_key(),
    ] {
        std::fs::remove_file(file).unwrap();
    }
    let identity = backup::identity_from_words(words).unwrap();
    let hex: String = identity
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(files.identity_key(), hex).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_node_keeps_a_certified_signing_key_and_not_the_identity_key() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("alice");
    let words = Node::init(&home, &Config::local_test("Alice"))
        .unwrap()
        .join(" ");
    let identity = backup::identity_from_words(&words).unwrap().public();

    // The words are the only copy of the identity key.
    let files = Home::new(&home);
    assert!(!files.identity_key().exists());
    for file in [
        files.signing_key(),
        files.signing_cert(),
        files.backup_key(),
    ] {
        assert!(file.exists(), "no {}", file.display());
    }
    let err = backup::recovery_words(&home).unwrap_err().to_string();
    assert!(err.contains("only where you wrote them down"), "{err}");
    assert!(backup::check_words(&home, &words).unwrap());
    let others = backup::new_identity_words().join(" ");
    assert!(!backup::check_words(&home, &others).unwrap());

    // Its profile and posts carry the certificate for its signing key, and
    // a follower takes them.
    let alice = Node::spawn(&home).await.unwrap();
    assert_eq!(alice.identity(), identity);
    post(&alice, "hello").await;
    for obj in [profile_of(&alice).await, outbox(&alice).await.remove(0)] {
        let (_, cert) = obj.verify_with_key().unwrap();
        assert_eq!(cert.expect("signed by a certified key").actor, identity);
    }
    assert!(
        !problems(&alice)
            .await
            .iter()
            .any(|p| p.contains("identity key"))
    );
    let bob_home = dir.path().join("bob");
    Node::init(&bob_home, &Config::local_test("Bob")).unwrap();
    let bob = Node::spawn(&bob_home).await.unwrap();
    follow_node(&bob, &alice).await;
    assert_eq!(feed_texts(&bob).await, ["hello"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_older_node_works_until_protect_takes_its_identity_key_off() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("alice");
    let words = Node::init(&home, &Config::local_test("Alice"))
        .unwrap()
        .join(" ");
    make_old_home(&home, &words);
    assert_eq!(backup::recovery_words(&home).unwrap().join(" "), words);

    // It signs with the identity key itself, and says it shouldn't keep it.
    let alice = Node::spawn(&home).await.unwrap();
    post(&alice, "before").await;
    let before = outbox(&alice).await;
    assert!(before[0].certificate().is_none());
    assert!(
        problems(&alice)
            .await
            .iter()
            .any(|p| p.contains("identity key"))
    );
    let bob_home = dir.path().join("bob");
    Node::init(&bob_home, &Config::local_test("Bob")).unwrap();
    let bob = Node::spawn(&bob_home).await.unwrap();
    follow_node(&bob, &alice).await;
    assert_eq!(feed_texts(&bob).await, ["before"]);
    let old_file = dir.path().join("old.ospbackup");
    assert_eq!(export(&alice, &old_file).await, 1);

    // Not while the node runs.
    let err = Node::protect_keys(&home, |_| Ok(()))
        .unwrap_err()
        .to_string();
    assert!(err.contains("already running"), "{err}");
    alice.shutdown().await.unwrap();
    // Nothing changes until the owner shows they have the words.
    let files = Home::new(&home);
    let shown = std::cell::RefCell::new(String::new());
    let not_written = Node::protect_keys(&home, |words| {
        *shown.borrow_mut() = words.join(" ");
        anyhow::bail!("not written down")
    });
    assert!(not_written.is_err());
    assert_eq!(*shown.borrow(), words);
    assert!(files.identity_key().exists());
    assert!(!files.signing_key().exists());
    Node::protect_keys(&home, |_| Ok(())).unwrap();
    assert!(!files.identity_key().exists());
    for file in [
        files.signing_key(),
        files.signing_cert(),
        files.backup_key(),
    ] {
        assert!(file.exists(), "no {}", file.display());
    }
    let err = Node::protect_keys(&home, |_| Ok(()))
        .unwrap_err()
        .to_string();
    assert!(err.contains("nothing to protect"), "{err}");
    assert!(backup::check_words(&home, &words).unwrap());

    // Started again, it signs with the new key. What it signed before is
    // unchanged, and its profile now carries the certificate.
    let alice = Node::spawn(&home).await.unwrap();
    assert!(
        !problems(&alice)
            .await
            .iter()
            .any(|p| p.contains("identity key"))
    );
    post(&alice, "after").await;
    let after = outbox(&alice).await;
    assert_eq!(after[0], before[0]);
    assert!(after[1].certificate().is_some());
    assert!(profile_of(&alice).await.certificate().is_some());
    let mut feed = feed_texts(&bob).await;
    feed.sort();
    assert_eq!(feed, ["after", "before"]);

    // Backup files from before and after both open with the words.
    let new_file = dir.path().join("new.ospbackup");
    assert_eq!(export(&alice, &new_file).await, 2);
    alice.shutdown().await.unwrap();
    for (name, file) in [("from-old", &old_file), ("from-new", &new_file)] {
        let restored = Node::restore(
            &dir.path().join(name),
            &Config::local_test("Alice"),
            &words,
            Some(file),
            EarlierKeys::Retired,
        )
        .unwrap();
        assert_eq!(
            restored.from_file.unwrap().posts,
            if name == "from-old" { 1 } else { 2 }
        );
    }
}

/// Checks `ready` every 100 ms, for up to 20 s.
async fn wait_until(what: &str, mut ready: impl AsyncFnMut() -> bool) {
    for _ in 0..200 {
        if ready().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("gave up waiting for {what}");
}

/// The signing key in a home's files, as someone who copied them has it.
fn copied_signing_key(home: &Path) -> CertifiedKey {
    let files = Home::new(home);
    let hex = std::fs::read_to_string(files.signing_key()).unwrap();
    let bytes: Vec<u8> = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let secret = SecretKey::from_bytes(&bytes.try_into().unwrap());
    let cert = std::fs::read_to_string(files.signing_cert()).unwrap();
    CertifiedKey::new(secret, serde_json::from_str(&cert).unwrap()).unwrap()
}

/// What the certificate in a home's files says.
fn certificate_in(home: &Path) -> osp_proto::Key {
    let cert = std::fs::read_to_string(Home::new(home).signing_cert()).unwrap();
    let cert: SignedObject = serde_json::from_str(&cert).unwrap();
    match cert.verify().unwrap() {
        Payload::Key(key) => key,
        other => panic!("not a certificate: {other:?}"),
    }
}

/// Gives a home's signing key a new certificate dated `secs` from now, as
/// a node whose clock was ahead would have made.
fn redate_certificate(home: &Path, words: &str, secs: i64) {
    let identity = backup::identity_from_words(words).unwrap();
    let copied = copied_signing_key(home);
    let hex = std::fs::read_to_string(Home::new(home).signing_key()).unwrap();
    let bytes: Vec<u8> = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let secret = SecretKey::from_bytes(&bytes.try_into().unwrap());
    assert_eq!(secret.public(), copied.public());
    let key = CertifiedKey::certify(&identity, secret, at(secs), EarlierKeys::Retired).unwrap();
    let json = serde_json::to_vec(key.certificate()).unwrap();
    std::fs::write(Home::new(home).signing_cert(), json).unwrap();
}

/// Copies a home's files, as someone who copied the computer's disk has
/// them, into `to` with a node key of its own.
fn copy_home(from: &Path, to: &Path) {
    std::fs::create_dir(to).unwrap();
    for name in [
        "config.toml",
        "signing.key",
        "signing.cert",
        "backup.key",
        "osp.db",
        "osp.db-wal",
        "osp.db-shm",
    ] {
        if from.join(name).exists() {
            std::fs::copy(from.join(name), to.join(name)).unwrap();
        }
    }
    let hex: String = SecretKey::generate()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(to.join("node.key"), hex).unwrap();
}

/// Sends `Moved` to `to` as someone holding `key`, from a node of their own.
async fn move_as(key: &CertifiedKey, to: &Node) -> SocialResponse {
    let addr = to.addr().await;
    let node = serve(Author::default(), &addr).await;
    let profile = Payload::Person(Person {
        actor: key.identity(),
        name: "Alice".into(),
        summary: None,
        nodes: vec![node.endpoint().id()],
        published: at(60),
    });
    let req = SocialRequest::Moved {
        profile: SignedObject::sign_certified(key, &profile).unwrap(),
        addr: node.endpoint().addr(),
    };
    ask(&node, &addr, &req).await
}

async fn feed_has(node: &Node, text: &str) -> bool {
    feed_texts(node).await.contains(&text.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn replacing_a_copied_key_keeps_the_owners_posts_and_shuts_the_copy_out() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("alice");
    let words = Node::init(&home, &Config::local_test("Alice"))
        .unwrap()
        .join(" ");
    // Her clock was four minutes ahead when the certificate was made, so the
    // new one must be dated later than that, not just now.
    redate_certificate(&home, &words, 4 * 60);
    let alice = Node::spawn(&home).await.unwrap();
    post(&alice, "before").await;
    let before = outbox(&alice).await;
    let bob_home = dir.path().join("bob");
    Node::init(&bob_home, &Config::local_test("Bob")).unwrap();
    let bob = Node::spawn(&bob_home).await.unwrap();
    follow_node(&bob, &alice).await;
    assert_eq!(feed_texts(&bob).await, ["before"]);

    // Someone copies her node's files and runs them as a node of their own,
    // which moves Bob's follow there.
    let thief_home = dir.path().join("thief");
    copy_home(&home, &thief_home);
    let copied = copied_signing_key(&home);
    let thief = Node::spawn(&thief_home).await.unwrap();
    post(&thief, "posted by the thief").await;
    wait_until("Bob to follow the copy", async || {
        feed_has(&bob, "posted by the thief").await
    })
    .await;

    // Not while her node runs, and not with someone else's words.
    let mut asked = false;
    let err = Node::replace_keys(&home, || {
        asked = true;
        Ok(words.clone())
    })
    .unwrap_err();
    assert!(err.to_string().contains("already running"), "{err}");
    assert!(
        !asked,
        "asked for the words before checking the node was stopped"
    );
    alice.shutdown().await.unwrap();
    let others = backup::new_identity_words().join(" ");
    let err = Node::replace_keys(&home, || Ok(others)).unwrap_err();
    assert!(err.to_string().contains("another identity"), "{err}");

    // She replaces her key. The new certificate revokes the old one.
    let old_cert = certificate_in(&home);
    Node::replace_keys(&home, || Ok(words.clone())).unwrap();
    let new_cert = certificate_in(&home);
    assert_eq!(new_cert.earlier_keys, EarlierKeys::Revoked);
    let (old_ms, new_ms) = (
        osp_proto::parse_rfc3339_ms(&old_cert.published).unwrap(),
        osp_proto::parse_rfc3339_ms(&new_cert.published).unwrap(),
    );
    assert!(new_ms > old_ms, "the new certificate isn't newer");

    // Started again, her node signs its posts again with the new key,
    // keeping their IDs, and tells Bob, who comes back to it.
    let alice = Node::spawn(&home).await.unwrap();
    let again = outbox(&alice).await;
    assert_eq!(again[0].id(), before[0].id());
    let (_, cert) = again[0].verify_with_key().unwrap();
    assert_eq!(cert.unwrap().key, new_cert.key);
    post(&alice, "after").await;
    wait_until("Bob to come back to Alice's node", async || {
        feed_has(&bob, "after").await
    })
    .await;

    // Whoever holds the copied key can't move Bob again.
    refused(move_as(&copied, &bob).await, "revoked");
    thief.shutdown().await.unwrap();

    // A node that still keeps its identity key protects it first.
    let old_home = dir.path().join("old");
    let old_words = Node::init(&old_home, &Config::local_test("Old"))
        .unwrap()
        .join(" ");
    make_old_home(&old_home, &old_words);
    let err = Node::replace_keys(&old_home, || Ok(old_words)).unwrap_err();
    assert!(err.to_string().contains("protect"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restore_that_revokes_the_old_key_signs_the_backup_again() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("osp_node=debug,warn")
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("alice");
    let words = Node::init(&home, &Config::local_test("Alice"))
        .unwrap()
        .join(" ");
    redate_certificate(&home, &words, 4 * 60);
    let alice = Node::spawn(&home).await.unwrap();
    post(&alice, "before").await;
    let before = outbox(&alice).await;
    let bob_home = dir.path().join("bob");
    Node::init(&bob_home, &Config::local_test("Bob")).unwrap();
    let bob = Node::spawn(&bob_home).await.unwrap();
    follow_node(&bob, &alice).await;
    assert_eq!(feed_texts(&bob).await, ["before"]);
    let file = dir.path().join("alice.ospbackup");
    export(&alice, &file).await;

    // Her computer is stolen: whoever has it can run her node.
    let copied = copied_signing_key(&home);
    let old_cert = certificate_in(&home);
    alice.shutdown().await.unwrap();

    // She restores on a new computer, revoking the old key.
    let new_home = dir.path().join("alice-new");
    let config = Config::local_test("Alice");
    Node::restore(
        &new_home,
        &config,
        &words,
        Some(&file),
        EarlierKeys::Revoked,
    )
    .unwrap();
    let new_cert = certificate_in(&new_home);
    assert_eq!(new_cert.earlier_keys, EarlierKeys::Revoked);
    let (old_ms, new_ms) = (
        osp_proto::parse_rfc3339_ms(&old_cert.published).unwrap(),
        osp_proto::parse_rfc3339_ms(&new_cert.published).unwrap(),
    );
    assert!(
        new_ms > old_ms,
        "not dated after the certificate in the backup"
    );
    let alice = Node::spawn(&new_home).await.unwrap();
    let again = outbox(&alice).await;
    assert_eq!(again[0].id(), before[0].id());
    assert_eq!(
        again[0].verify_with_key().unwrap().1.unwrap().key,
        new_cert.key
    );
    post(&alice, "on the new computer").await;
    wait_until("Bob to follow the new node", async || {
        feed_has(&bob, "on the new computer").await
    })
    .await;
    refused(move_as(&copied, &bob).await, "revoked");

    // When nobody else can have the old key, a restore retires it instead.
    let retired = dir.path().join("alice-retired");
    Node::restore(&retired, &config, &words, Some(&file), EarlierKeys::Retired).unwrap();
    assert_eq!(certificate_in(&retired).earlier_keys, EarlierKeys::Retired);
}
