//! Replying to someone else's post, and reading a thread (ADR 0028). Kim
//! posts. Jimmy follows her, but she doesn't follow him: his reply waits for
//! her to look. Alice reads the thread without showing who she is.
//!
//! These run on the simulated network, so a node that restarts is reached by
//! its ID as before, and every packet is noted by sender and receiver.

use std::{collections::HashSet, path::Path};

use iroh::EndpointId;
use osp_node::{
    Config, Node, SimSettings,
    simnet::{Link, SimNet},
};
use osp_proto::{
    ObjectId,
    rpc::{ClientRequest, ClientResponse, ReplyAnswer, ReplyInfo, SentReplyInfo, SentStatus},
    social::ReplyPolicy,
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

/// Someone with media and direct endpoints.
async fn start(root: &Path, net: &SimNet, name: &str) -> Node {
    let mut config = Config::local_test(name);
    config.endpoints.enabled = true;
    Node::init(&home(root, name), &config).unwrap();
    restart(root, net, name).await
}

/// Someone whose node polls those it follows every 5 s, not hourly.
async fn start_polling(root: &Path, net: &SimNet, name: &str) -> Node {
    let mut config = Config::local_test(name);
    config.endpoints.enabled = true;
    config.sync.interval_secs = 5;
    Node::init(&home(root, name), &config).unwrap();
    restart(root, net, name).await
}

async fn restart(root: &Path, net: &SimNet, name: &str) -> Node {
    Node::spawn_sim(&home(root, name), sim(net)).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
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

async fn reply(node: &Node, post: ObjectId, text: &str) -> SentReplyInfo {
    let req = ClientRequest::Reply {
        post,
        content: text.into(),
    };
    match call(node, req).await {
        ClientResponse::ReplySent { reply } => reply,
        other => panic!("unexpected {other:?}"),
    }
}

async fn sent(node: &Node) -> Vec<SentReplyInfo> {
    match call(node, ClientRequest::SentReplies).await {
        ClientResponse::SentReplies { replies } => replies,
        other => panic!("unexpected {other:?}"),
    }
}

async fn thread(node: &Node, post: ObjectId, after: u64) -> (Vec<ReplyInfo>, bool) {
    match call(node, ClientRequest::Thread { post, after }).await {
        ClientResponse::Thread { replies, more, .. } => (replies, more),
        other => panic!("unexpected {other:?}"),
    }
}

async fn requests(node: &Node) -> Vec<ReplyInfo> {
    match call(node, ClientRequest::ReplyRequests).await {
        ClientResponse::ReplyRequests { requests } => requests,
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_to_a_creator_whos_off_is_kept_through_a_restart_and_arrives_once() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(80);
    let kim = start(dir.path(), &net, "Kim").await;
    let jimmy = start(dir.path(), &net, "Jimmy").await;
    let kims = post(&kim, "Kim's post").await;
    follow(&jimmy, &kim).await;

    // Kim's node is off: Jimmy's reply is kept, to be tried again.
    kim.shutdown().await.unwrap();
    let first = reply(&jimmy, kims, "Jimmy's reply").await;
    assert_eq!(first.status, SentStatus::Sending);
    assert!(first.next_try_secs.is_some());
    assert_eq!(first.post, kims);

    // It's still there after Jimmy's node restarts.
    jimmy.shutdown().await.unwrap();
    let jimmy = restart(dir.path(), &net, "Jimmy").await;
    let kept = sent(&jimmy).await;
    assert_eq!(kept.len(), 1);
    assert_eq!(
        (kept[0].id, kept[0].status),
        (first.id, SentStatus::Sending)
    );
    assert_eq!(kept[0].content, "Jimmy's reply");

    // Kim's node is back, and `osp sync` tries at once. It arrives once,
    // however often it's tried.
    let kim = restart(dir.path(), &net, "Kim").await;
    call(&jimmy, ClientRequest::Sync).await;
    let kept = sent(&jimmy).await;
    assert_eq!(kept[0].status, SentStatus::AwaitingApproval);
    assert_eq!(kept[0].next_try_secs, None);
    call(&jimmy, ClientRequest::Sync).await;
    let waiting = requests(&kim).await;
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].id, first.id);

    // Kim publishes it. Jimmy sees it in the thread, so it's published for
    // him too.
    let publish = ClientRequest::AnswerReply {
        reply: first.id,
        answer: ReplyAnswer::Publish,
    };
    call(&kim, publish).await;
    let (replies, more) = thread(&jimmy, kims, 0).await;
    assert!(!more);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].content, "Jimmy's reply");
    assert_eq!(sent(&jimmy).await[0].status, SentStatus::Published);

    for node in [jimmy, kim] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_creator_reached_again_gets_waiting_replies_and_a_deleted_post_ends_them() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(82);
    let kim = start(dir.path(), &net, "Kim").await;
    let jimmy = start_polling(dir.path(), &net, "Jimmy").await;
    let kims = post(&kim, "Kim's post").await;
    follow(&jimmy, &kim).await;

    // Kim's node is off: Jimmy's reply waits a minute for its next try.
    kim.shutdown().await.unwrap();
    let first = reply(&jimmy, kims, "Jimmy's reply").await;
    assert_eq!(first.status, SentStatus::Sending);

    // Kim's node is back. Jimmy's node syncs her on its own, with no
    // `osp sync`, and sends the reply then, not when the minute is up.
    let kim = restart(dir.path(), &net, "Kim").await;
    let back = std::time::Instant::now();
    let status = loop {
        let status = sent(&jimmy).await[0].status;
        if status != SentStatus::Sending || back.elapsed().as_secs() >= 45 {
            break status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    };
    assert_eq!(status, SentStatus::AwaitingApproval);
    assert_eq!(requests(&kim).await.len(), 1);

    // Kim deletes the post. Once Jimmy's node hears, his reply stands as
    // the post deleted, with nothing more to try.
    call(&kim, ClientRequest::Delete { id: kims }).await;
    call(&jimmy, ClientRequest::Sync).await;
    let kept = sent(&jimmy).await;
    assert_eq!(
        (kept[0].status, kept[0].next_try_secs),
        (SentStatus::Deleted, None)
    );

    for node in [jimmy, kim] {
        node.shutdown().await.unwrap();
    }
}

/// `node`'s public, media and direct endpoints.
fn own_endpoints(node: &Node) -> HashSet<EndpointId> {
    let ids: HashSet<EndpointId> = node.endpoint_ids().into_iter().collect();
    assert_eq!(ids.len(), 3, "public, media and direct");
    ids
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thread_is_read_without_the_readers_endpoints_and_deletes_reach_it() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(81);
    let kim = start(dir.path(), &net, "Kim").await;
    let jimmy = start(dir.path(), &net, "Jimmy").await;
    let alice = start(dir.path(), &net, "Alice").await;
    let kims = post(&kim, "Kim's post").await;
    follow(&jimmy, &kim).await;
    follow(&alice, &kim).await;

    // Kim answers her own post on her node; Jimmy's waits until she
    // publishes it.
    let own = reply(&kim, kims, "Kim's own reply").await;
    assert_eq!(own.status, SentStatus::Published);
    let jimmys = reply(&jimmy, kims, "Jimmy's reply").await;
    assert_eq!(jimmys.status, SentStatus::AwaitingApproval);
    let publish = ClientRequest::AnswerReply {
        reply: jimmys.id,
        answer: ReplyAnswer::Publish,
    };
    call(&kim, publish).await;

    // Alice reads the thread, and nothing goes from her own endpoints.
    let alices = own_endpoints(&alice);
    net.record_senders();
    let (replies, _) = thread(&alice, kims, 0).await;
    let words: Vec<&str> = replies.iter().map(|r| r.content.as_str()).collect();
    assert_eq!(words, ["Kim's own reply", "Jimmy's reply"]);
    let from_alice: Vec<_> = net
        .senders()
        .into_iter()
        .filter(|(from, _)| alices.contains(from))
        .collect();
    assert!(
        from_alice.is_empty(),
        "Alice's endpoints sent: {from_alice:?}"
    );
    assert!(alice.viewing_endpoints_made() >= 1);
    // The next page starts after the first reply.
    let (rest, _) = thread(&alice, kims, replies[0].seq).await;
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].id, jimmys.id);

    // Jimmy deletes his: it goes from his node and from Kim's thread.
    call(&jimmy, ClientRequest::Delete { id: jimmys.id }).await;
    call(&jimmy, ClientRequest::Sync).await;
    assert!(sent(&jimmy).await.is_empty());
    let (replies, _) = thread(&alice, kims, 0).await;
    assert_eq!(replies.len(), 1);
    // Kim deletes her own the same way.
    call(&kim, ClientRequest::Delete { id: own.id }).await;
    assert!(thread(&alice, kims, 0).await.0.is_empty());

    // Kim deletes the post: a reader gets her `Delete`.
    call(&kim, ClientRequest::Delete { id: kims }).await;
    let gone = alice
        .handle(ClientRequest::Thread {
            post: kims,
            after: 0,
        })
        .await;
    match gone {
        ClientResponse::Error { message } => assert!(message.contains("deleted"), "{message}"),
        other => panic!("unexpected {other:?}"),
    }

    for node in [alice, jimmy, kim] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_ends_the_tries_and_a_reshare_is_answered_at_its_post() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(82);
    let kim = start(dir.path(), &net, "Kim").await;
    let jimmy = start(dir.path(), &net, "Jimmy").await;
    let open = post(&kim, "anyone may answer").await;
    let closed = post(&kim, "no replies").await;
    let off = ClientRequest::SetReplyPolicy {
        post: closed,
        policy: ReplyPolicy::Off,
    };
    call(&kim, off).await;
    follow(&jimmy, &kim).await;

    // Closed is an answer: nothing more is tried.
    let refused = reply(&jimmy, closed, "hello?").await;
    assert_eq!(refused.status, SentStatus::Closed);
    assert_eq!(refused.next_try_secs, None);

    // A reply to Jimmy's reshare answers Kim's post.
    let ClientResponse::Posted { id: reshare } = call(
        &jimmy,
        ClientRequest::Reshare {
            id: open,
            host: false,
        },
    )
    .await
    else {
        panic!("not reshared");
    };
    let answer = reply(&jimmy, reshare, "on the reshare").await;
    assert_eq!(answer.post, open);
    assert_eq!(answer.status, SentStatus::AwaitingApproval);
    assert_eq!(requests(&kim).await[0].post, open);

    // A closed one deleted goes at once: Kim's node never kept it.
    call(&jimmy, ClientRequest::Delete { id: refused.id }).await;
    assert_eq!(sent(&jimmy).await.len(), 1);

    for node in [jimmy, kim] {
        node.shutdown().await.unwrap();
    }
}
