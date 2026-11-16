//! Activity (ADR 0038 §3). Kim posts; Jimmy and Alice follow her, like it,
//! reply and reshare. Kim sees all of it in one place, likes grouped, and
//! what's read is kept on her node, so every device of hers agrees.

use std::{path::Path, time::Duration};

use osp_node::{
    Config, Node, SimSettings,
    simnet::{Link, SimNet},
};
use osp_proto::{
    ObjectId,
    rpc::{
        ActivityEntry, ActivityWhat, ClientRequest, ClientResponse, ReplyAnswer, SentReplyInfo,
        SentStatus,
    },
};

fn sim(net: &SimNet) -> SimSettings {
    SimSettings {
        network: Some(net.link(Link::default())),
        ..SimSettings::default()
    }
}

async fn start(root: &Path, net: &SimNet, name: &str) -> Node {
    let mut config = Config::local_test(name);
    config.endpoints.enabled = true;
    let home = root.join(name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn_sim(&home, sim(net)).await.unwrap()
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

async fn follow(follower: &Node, creator: &Node) {
    let ClientResponse::Ticket { ticket } = call(creator, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(follower, ClientRequest::Follow { ticket }).await;
    call(follower, ClientRequest::Sync).await;
}

async fn like(node: &Node, post: ObjectId, liked: bool) {
    call(node, ClientRequest::Like { post, liked }).await;
    call(node, ClientRequest::Sync).await;
}

async fn reply(node: &Node, post: ObjectId, content: &str) -> SentReplyInfo {
    let req = ClientRequest::Reply {
        post,
        content: content.into(),
    };
    match call(node, req).await {
        ClientResponse::ReplySent { reply } => reply,
        other => panic!("unexpected {other:?}"),
    }
}

async fn activity(node: &Node) -> (Vec<ActivityEntry>, u64) {
    let req = ClientRequest::Activity {
        before: None,
        limit: None,
    };
    match call(node, req).await {
        ClientResponse::Activity {
            entries, unread, ..
        } => (entries, unread),
        other => panic!("unexpected {other:?}"),
    }
}

/// Waits until `check` passes on `node`'s Activity.
async fn wait_activity(
    node: &Node,
    check: impl Fn(&[ActivityEntry]) -> bool,
) -> (Vec<ActivityEntry>, u64) {
    let mut last = (Vec::new(), 0);
    for _ in 0..60 {
        last = activity(node).await;
        if check(&last.0) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("the Activity stayed {:#?}", last.0);
}

fn liked(entries: &[ActivityEntry]) -> Option<(Vec<&str>, u64)> {
    entries.iter().find_map(|e| match &e.what {
        ActivityWhat::Liked { people, count, .. } => {
            Some((people.iter().map(|p| p.name.as_str()).collect(), *count))
        }
        _ => None,
    })
}

fn kinds(entries: &[ActivityEntry]) -> Vec<&str> {
    entries
        .iter()
        .map(|e| e.key.split_once(':').unwrap().0)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn kim_sees_followers_likes_replies_and_reshares_in_one_place() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(100);
    let kim = start(dir.path(), &net, "Kim").await;
    let jimmy = start(dir.path(), &net, "Jimmy").await;
    let alice = start(dir.path(), &net, "Alice").await;
    let kims = post(&kim, "Kim's post\nand more").await;
    assert_eq!(activity(&kim).await, (vec![], 0));

    // Jimmy follows. Kim's node has only his node's key, so it doesn't
    // say whose.
    follow(&jimmy, &kim).await;
    let (entries, unread) = activity(&kim).await;
    assert_eq!(unread, 1);
    assert!(matches!(
        entries[0].what,
        ActivityWhat::Followed { person: None, .. }
    ));

    // Jimmy and then Alice like the post: one entry, Alice first.
    like(&jimmy, kims, true).await;
    follow(&alice, &kim).await;
    like(&alice, kims, true).await;
    let (entries, _) = wait_activity(&kim, |e| liked(e).is_some_and(|(_, n)| n == 2)).await;
    assert_eq!(liked(&entries).unwrap().0, ["Alice", "Jimmy"]);
    assert_eq!(kinds(&entries), ["liked", "followed", "followed"]);
    let ActivityWhat::Liked { post_text, .. } = &entries[0].what else {
        unreachable!()
    };
    assert_eq!(post_text, "Kim's post\nand more");

    // Kim's own reply isn't news to her. Jimmy's waits for her.
    reply(&kim, kims, "Kim's own reply").await;
    let jimmys = reply(&jimmy, kims, "Jimmy's reply").await;
    assert_eq!(jimmys.status, SentStatus::AwaitingApproval);
    let (entries, _) = activity(&kim).await;
    assert_eq!(kinds(&entries), ["reply", "liked", "followed", "followed"]);
    assert!(matches!(
        &entries[0].what,
        ActivityWhat::Reply { waiting: true, name, content, .. }
            if name == "Jimmy" && content == "Jimmy's reply"
    ));

    // Jimmy says he knows Kim: her node now names his node as a follower.
    let ask = ClientRequest::AskTrust {
        actor: kim.identity(),
        note: "from the climbing gym".into(),
    };
    call(&jimmy, ask).await;
    let (entries, _) = activity(&kim).await;
    assert!(matches!(
        &entries[0].what,
        ActivityWhat::KnowsYou { person, note, .. }
            if person.name == "Jimmy" && note == "from the climbing gym"
    ));
    let jimmy_node = entries.iter().find_map(|e| match &e.what {
        ActivityWhat::Followed {
            person: Some(p), ..
        } => Some(p.name.clone()),
        _ => None,
    });
    assert_eq!(jimmy_node.as_deref(), Some("Jimmy"));

    // Alice reshares it.
    let reshare = ClientRequest::Reshare {
        id: kims,
        host: false,
    };
    call(&alice, reshare).await;
    call(&alice, ClientRequest::Sync).await;
    let (entries, _) = wait_activity(&kim, |e| kinds(e).first() == Some(&"reshared")).await;
    assert!(matches!(
        &entries[0].what,
        ActivityWhat::Reshared { people, count: 1, .. } if people[0].name == "Alice"
    ));

    // Kim lets Jimmy's reply in: the same entry, no longer waiting. Jimmy's
    // node hears when he reads the thread, and tells him: it had waited.
    let publish = ClientRequest::AnswerReply {
        reply: jimmys.id,
        answer: ReplyAnswer::Publish,
    };
    call(&kim, publish).await;
    let (entries, _) = activity(&kim).await;
    assert!(entries.iter().any(|e| matches!(
        &e.what,
        ActivityWhat::Reply { waiting: false, reply, .. } if *reply == jimmys.id
    )));
    call(
        &jimmy,
        ClientRequest::Thread {
            post: kims,
            after: 0,
        },
    )
    .await;
    let (theirs, unread) = activity(&jimmy).await;
    assert_eq!(unread, 1);
    assert!(matches!(
        &theirs[0].what,
        ActivityWhat::ReplyPublished { creator_name: Some(n), content, .. }
            if n == "Kim" && content == "Jimmy's reply"
    ));

    // Kim reads her Activity on one phone, through the newest entry; her
    // other phone sees it read too.
    let (entries, unread) = activity(&kim).await;
    assert_eq!(unread as usize, entries.len());
    let read = ClientRequest::ActivityRead {
        keys: vec![],
        through: Some(entries[0].seq),
    };
    call(&kim, read).await;
    let (entries, unread) = activity(&kim).await;
    assert_eq!(unread, 0);
    assert!(entries.iter().all(|e| e.read));

    // Jimmy takes his like back: the entry stays, for Alice, still read.
    // Alice takes hers back too, and the entry goes.
    like(&jimmy, kims, false).await;
    let (entries, _) = wait_activity(&kim, |e| liked(e).is_some_and(|(_, n)| n == 1)).await;
    assert_eq!(liked(&entries).unwrap().0, ["Alice"]);
    assert_eq!(activity(&kim).await.1, 0);
    like(&alice, kims, false).await;
    wait_activity(&kim, |e| liked(e).is_none()).await;

    // Jimmy likes it again: a new liker moves the entry up, unread.
    like(&jimmy, kims, true).await;
    let (entries, unread) = wait_activity(&kim, |e| liked(e).is_some()).await;
    assert_eq!(kinds(&entries)[0], "liked");
    assert!(!entries[0].read);
    assert_eq!(unread, 1);

    for node in [alice, jimmy, kim] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn activity_pages_read_on_and_a_deleted_post_takes_its_entries() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(101);
    let kim = start(dir.path(), &net, "Kim").await;
    let jimmy = start(dir.path(), &net, "Jimmy").await;
    follow(&jimmy, &kim).await;
    let mut posts = Vec::new();
    for i in 0..3 {
        let p = post(&kim, &format!("post {i}")).await;
        follow(&jimmy, &kim).await;
        like(&jimmy, p, true).await;
        posts.push(p);
    }
    wait_activity(&kim, |e| e.len() == 4).await;

    // Two at a time, then the rest.
    let page = |before| ClientRequest::Activity {
        before,
        limit: Some(2),
    };
    let ClientResponse::Activity { entries, more, .. } = call(&kim, page(None)).await else {
        panic!()
    };
    assert!(more);
    assert_eq!(kinds(&entries), ["liked", "liked"]);
    let ClientResponse::Activity {
        entries: rest,
        more,
        ..
    } = call(&kim, page(Some(entries[1].seq))).await
    else {
        panic!()
    };
    assert!(!more);
    assert_eq!(kinds(&rest), ["liked", "followed"]);

    // Kim deletes a post: its likes' entry goes with it.
    call(&kim, ClientRequest::Delete { id: posts[2] }).await;
    let (entries, unread) = activity(&kim).await;
    assert_eq!(kinds(&entries), ["liked", "liked", "followed"]);
    assert_eq!(unread, 3);

    for node in [jimmy, kim] {
        node.shutdown().await.unwrap();
    }
}
