//! ADR 0036's events, on the simulated network: each kind comes from a
//! real action, an idle world makes none, and none holds a field outside
//! the allowlist or any node's endpoint ID.
//!
//! Kim follows Alice and Jimmy; Jimmy follows Alice. Alice posts a photo:
//! Kim's node fetches it, and Alice's serves it. Jimmy reshares it, which
//! Kim's node sees. Alice posts another, Jimmy keeps a copy, and Alice's
//! node goes: Jimmy reshares it with hosting, and Kim's node has it from
//! his copy, "kept available by a copy". Fred is listed in a directory,
//! and Kim's read of it brings him. Jimmy's machine greets Kim's, which
//! meets it, and then sees it fade once Jimmy's node is gone.

use std::{
    collections::BTreeSet,
    path::Path,
    time::{Duration, Instant},
};

use osp_node::{
    Config, KnownDirectory, MeetingTimes, Node, SimSettings,
    simnet::{Link, SimNet},
};
use osp_proto::{
    BlobHash, ObjectId,
    rpc::{ClientRequest, ClientResponse},
    universe::{EVENT_ALLOWLIST, Event, EventWhat, Snapshot, field_paths},
};

/// How long a machine counts as answering, here: short, so one can fade.
const STILL_ANSWERING: Duration = Duration::from_secs(3);

fn sim(net: &SimNet, round: Duration) -> SimSettings {
    SimSettings {
        network: Some(net.link(Link::default())),
        machine_names: true,
        meeting: MeetingTimes {
            round,
            still_answering: STILL_ANSWERING,
            ..MeetingTimes::default()
        },
        ..SimSettings::default()
    }
}

/// No round in the background: the test runs them (`meet_round_now`).
const NO_ROUNDS: Duration = Duration::from_secs(1_000 * 24 * 60 * 60);

async fn start(root: &Path, config: Config, settings: SimSettings) -> Node {
    let home = root.join(config.name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn_sim(&home, settings).await.unwrap()
}

async fn start_directory(root: &Path, net: &SimNet, round: Duration) -> Node {
    let mut config = Config::local_test("The directory");
    config.endpoints.enabled = true;
    config.directory.serve = true;
    let settings = SimSettings {
        listed_read_every: Duration::from_millis(200),
        ..sim(net, round)
    };
    start(root, config, settings).await
}

async fn start_knowing(
    root: &Path,
    net: &SimNet,
    name: &str,
    directory: &Node,
    list_me: bool,
    round: Duration,
) -> Node {
    let mut config = Config::local_test(name);
    config.endpoints.enabled = true;
    config.directory.list_me = list_me;
    config.directory.known = vec![KnownDirectory {
        machine: directory.machine_id().unwrap(),
        node: directory.node_id(),
    }];
    start(root, config, sim(net, round)).await
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
    call(follower, ClientRequest::Sync).await;
}

/// Posts a photo made with ffmpeg, `width` wide so each is its own.
/// Returns the post and the photo.
async fn post_photo(node: &Node, dir: &Path, width: u32) -> (ObjectId, BlobHash) {
    let photo = dir.join(format!("{}-{width}.png", node.identity()));
    let made = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i"])
        .arg(format!("testsrc=size={width}x240"))
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(made.success());
    let req = ClientRequest::Post {
        content: "a photo".into(),
        attachments: vec![photo],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, req).await else {
        panic!("not posted")
    };
    let ClientResponse::Item { item } = call(node, ClientRequest::Show { id }).await else {
        panic!("not shown")
    };
    (id, item.attachments[0].attachment.blob)
}

async fn events(node: &Node) -> (Vec<Event>, String) {
    let resp = call(node, ClientRequest::UniverseEvents { since: None }).await;
    let text = serde_json::to_string(&resp).unwrap();
    let ClientResponse::UniverseEvents { events } = resp else {
        panic!("no events")
    };
    (events, text)
}

/// Waits up to a minute for an event of `node`'s that `check` picks, and
/// returns it.
async fn event_when(node: &Node, what: &str, check: impl Fn(&EventWhat) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let (all, text) = events(node).await;
        if let Some(e) = all.into_iter().find(|e| check(&e.what)) {
            return e;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {text}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn holds(node: &Node, blob: &BlobHash, who: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while node.blob_class(blob).unwrap().is_none() {
        assert!(Instant::now() < deadline, "{who} lacks the photo");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Every field is on the allowlist, and no endpoint ID of any node is
/// anywhere in the bytes.
fn check_fields(text: &str, world: &[&Node]) {
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let allowed: BTreeSet<String> = EVENT_ALLOWLIST.iter().map(|s| s.to_string()).collect();
    // The reply's own tag, then its events.
    let mut fields = field_paths(&value);
    fields.retain(|f| f != "result" && !allowed.contains(f));
    assert!(
        fields.is_empty(),
        "fields outside the allowlist: {fields:?}"
    );
    for node in world {
        for id in node.endpoint_ids() {
            assert!(!text.contains(&id.to_string()), "an endpoint ID in {text}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_idle_world_makes_no_events() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(37);
    // Machines meet in the background, every second or so.
    let round = Duration::from_secs(1);
    let directory = start_directory(dir.path(), &net, round).await;
    let mut world = vec![directory];
    for name in ["Kim", "Jimmy", "Alice"] {
        let node = start_knowing(dir.path(), &net, name, &world[0], false, round).await;
        world.push(node);
    }
    tokio::time::sleep(Duration::from_secs(15)).await;
    for node in &world {
        let (events, text) = events(node).await;
        assert!(events.is_empty(), "{text}");
    }
    for node in world {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_event_comes_from_a_real_action() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(38);
    let directory = start_directory(dir.path(), &net, NO_ROUNDS).await;
    let knowing = |name: &'static str, list_me: bool| {
        start_knowing(dir.path(), &net, name, &directory, list_me, NO_ROUNDS)
    };
    let kim = knowing("Kim", false).await;
    let jimmy = knowing("Jimmy", false).await;
    let alice = knowing("Alice", false).await;
    let fred = knowing("Fred", true).await;
    let alice_id = alice.identity();
    let jimmy_id = jimmy.identity();

    follow(&jimmy, &alice).await;
    follow(&kim, &alice).await;
    follow(&kim, &jimmy).await;
    // Following and syncing make none.
    for node in [&directory, &kim, &jimmy, &alice, &fred] {
        let (events, text) = events(node).await;
        assert!(events.is_empty(), "{text}");
    }

    // Alice posts a photo: Kim's node fetches it ahead of time, from her,
    // and hers serves it.
    let (first, first_photo) = post_photo(&alice, dir.path(), 320).await;
    call(&kim, ClientRequest::Sync).await;
    holds(&kim, &first_photo, "Kim").await;
    let fetched = event_when(
        &kim,
        "Kim's fetch",
        |w| matches!(w, EventWhat::Fetched { post: Some(p), .. } if *p == first),
    )
    .await;
    let EventWhat::Fetched {
        author,
        bytes,
        host,
        kept_by_copy,
        from_author,
        ..
    } = fetched.what
    else {
        unreachable!()
    };
    assert_eq!(author, Some(alice_id));
    assert!(bytes > 0);
    assert_eq!(host, None, "Alice sent it herself");
    assert!(from_author, "Alice's node sent it");
    assert!(!kept_by_copy);
    let served = event_when(
        &alice,
        "Alice serving",
        |w| matches!(w, EventWhat::Served { post: Some(p), .. } if *p == first),
    )
    .await;
    assert!(
        matches!(served.what, EventWhat::Served { author: Some(a), bytes, .. } if a == alice_id && bytes > 0)
    );

    // Jimmy reshares it, and Kim's node sees that.
    call(&jimmy, ClientRequest::Sync).await;
    call(
        &jimmy,
        ClientRequest::Reshare {
            id: first,
            host: true,
        },
    )
    .await;
    call(&kim, ClientRequest::Sync).await;
    let reshared = event_when(&kim, "Kim seeing Jimmy's reshare", |w| {
        matches!(w, EventWhat::Reshared { .. })
    })
    .await;
    assert_eq!(
        reshared.what,
        EventWhat::Reshared {
            from: jimmy_id,
            author: alice_id,
            post: first,
        }
    );

    // Alice posts another photo, Jimmy's node keeps it, and hers goes.
    let (second, second_photo) = post_photo(&alice, dir.path(), 160).await;
    call(&jimmy, ClientRequest::Sync).await;
    holds(&jimmy, &second_photo, "Jimmy").await;
    alice.shutdown().await.unwrap();
    // Kim's node can't reach hers now.
    call(&kim, ClientRequest::Sync).await;
    assert!(kim.blob_class(&second_photo).unwrap().is_none());
    call(
        &jimmy,
        ClientRequest::Reshare {
            id: second,
            host: true,
        },
    )
    .await;
    call(&kim, ClientRequest::Sync).await;
    holds(&kim, &second_photo, "Kim").await;
    let kept = event_when(
        &kim,
        "Kim's fetch from Jimmy's copy",
        |w| matches!(w, EventWhat::Fetched { post: Some(p), .. } if *p == second),
    )
    .await;
    let EventWhat::Fetched {
        host,
        kept_by_copy,
        from_author,
        ..
    } = kept.what
    else {
        unreachable!()
    };
    assert!(kept_by_copy, "{kept:?}");
    assert!(!from_author, "Alice's node was away");
    // Named only as the resharer hosting it, if at all.
    assert!(host.is_none() || host == Some(jimmy_id), "{host:?}");

    // Fred is listed, and Kim's read brings him.
    assert!(fred.ask_to_be_listed().await);
    call(&kim, ClientRequest::ReadDirectories).await;
    let found = event_when(&kim, "Kim finding Fred", |w| {
        matches!(w, EventWhat::Found { .. })
    })
    .await;
    let req = ClientRequest::Universe {
        level: Default::default(),
        region: None,
        layers: vec![],
        since: None,
    };
    let ClientResponse::Universe { snapshot } = call(&kim, req).await else {
        panic!("no snapshot")
    };
    let snapshot: Snapshot = *snapshot;
    let fred_point = snapshot.people.iter().find(|p| p.actor == fred.identity());
    let through = fred_point.and_then(|p| p.found_through.clone());
    assert!(
        matches!(&found.what, EventWhat::Found { directory, people } if Some(directory) == through.as_ref() && *people >= 1),
        "{found:?} {through:?}"
    );

    // Jimmy's machine greets Kim's, which meets it in its next round, and
    // sees it fade once Jimmy's node is gone.
    let machine = kim.machine_id().unwrap();
    call(&jimmy, ClientRequest::MachineHello { machine }).await;
    kim.meet_round_now().await.unwrap();
    let met = event_when(&kim, "Kim meeting Jimmy's machine", |w| {
        matches!(w, EventWhat::Machine { met: true, .. })
    })
    .await;
    let EventWhat::Machine { words, .. } = met.what else {
        unreachable!()
    };
    jimmy.shutdown().await.unwrap();
    tokio::time::sleep(STILL_ANSWERING + Duration::from_secs(1)).await;
    kim.meet_round_now().await.unwrap();
    event_when(
        &kim,
        "Jimmy's machine fading",
        |w| matches!(w, EventWhat::Machine { words: w, met: false } if *w == words),
    )
    .await;

    // Nothing outside the allowlist, and no endpoint ID.
    for node in [&directory, &kim, &fred] {
        let (_, text) = events(node).await;
        check_fields(&text, &[&directory, &kim, &fred]);
    }
    for node in [directory, kim, fred] {
        node.shutdown().await.unwrap();
    }
}
