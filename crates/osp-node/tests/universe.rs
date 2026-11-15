//! What the Universe may show (ADR 0033): one snapshot from one allowlist,
//! built on the node alone.
//!
//! Kim follows Jimmy, and Jimmy reshares a post of Alice's. Kim's Universe
//! shows Kim, Jimmy one step away, Alice two, and the line between them, and
//! nothing outside the allowlist: no full endpoint ID, address or relay URL.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.

use std::{collections::BTreeSet, path::Path};

use osp_node::{Config, Node};
use osp_proto::{
    ObjectId,
    rpc::{ClientRequest, ClientResponse, StatusInfo},
    universe::{ALLOWLIST, Layer, Level, LineKind, MAX_SNAPSHOT_BYTES, Snapshot, field_paths},
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

async fn follow(follower: &Node, followed: &Node) {
    let ClientResponse::Ticket { ticket } = call(followed, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    call(follower, ClientRequest::Follow { ticket }).await;
}

async fn status(node: &Node) -> StatusInfo {
    match call(node, ClientRequest::Status).await {
        ClientResponse::Status(status) => status,
        other => panic!("unexpected {other:?}"),
    }
}

fn universe_request(since: Option<String>) -> ClientRequest {
    ClientRequest::Universe {
        level: Level::Universe,
        region: None,
        layers: vec![],
        since,
    }
}

async fn universe(node: &Node) -> (Snapshot, Vec<u8>) {
    let resp = call(node, universe_request(None)).await;
    let bytes = serde_json::to_vec(&resp).unwrap();
    match resp {
        ClientResponse::Universe { snapshot } => (*snapshot, bytes),
        other => panic!("unexpected {other:?}"),
    }
}

/// Requests this node has sent its peers, as its peer record counts them.
async fn requests_sent(node: &Node) -> f64 {
    match call(node, ClientRequest::Peers).await {
        ClientResponse::Peers { peers } => peers.iter().map(|p| p.counts.requests).sum(),
        other => panic!("unexpected {other:?}"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_universe_shows_steps_and_public_lines_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let jimmy = start(dir.path(), "Jimmy").await;
    let alice = start(dir.path(), "Alice").await;
    follow(&jimmy, &alice).await;
    let id = post(&alice, "A birch stool").await;
    call(&jimmy, ClientRequest::Sync).await;
    call(&jimmy, ClientRequest::Reshare { id, host: false }).await;
    post(&jimmy, "Jimmy's own").await;
    follow(&kim, &jimmy).await;
    call(&kim, ClientRequest::Sync).await;

    let before = requests_sent(&kim).await;
    let (snapshot, bytes) = universe(&kim).await;
    let step = |name: &str| {
        let person = snapshot.people.iter().find(|p| p.name == name);
        person.unwrap_or_else(|| panic!("{name} isn't shown")).step
    };
    assert_eq!(snapshot.people[0].name, "Kim");
    assert_eq!(step("Kim"), Some(0));
    assert_eq!(step("Jimmy"), Some(1));
    assert_eq!(step("Alice"), Some(2));
    assert_eq!(snapshot.level, Level::Local);
    let jimmy_id = status(&jimmy).await.identity;
    let alice_id = status(&alice).await.identity;
    let actor = |place: u32| snapshot.people[place as usize].actor;
    assert!(snapshot.lines.iter().any(|l| l.kind == LineKind::Reshared
        && actor(l.from) == jimmy_id
        && actor(l.to) == alice_id
        && l.count == 1));
    let jimmy_point = snapshot.people.iter().find(|p| p.name == "Jimmy").unwrap();
    assert!(jimmy_point.followed);
    assert_eq!((jimmy_point.posts, jimmy_point.reshares), (1, 1));
    assert!(snapshot.network.is_some());

    // Every field is on the allowlist.
    let fields = field_paths(&serde_json::to_value(&snapshot).unwrap());
    let allowed: BTreeSet<String> = ALLOWLIST.iter().map(|s| s.to_string()).collect();
    let outside: Vec<_> = fields.difference(&allowed).collect();
    assert!(
        outside.is_empty(),
        "fields outside the allowlist: {outside:?}"
    );

    // Within its limit (ADR 0034).
    assert!(bytes.len() <= MAX_SNAPSHOT_BYTES, "{} bytes", bytes.len());

    // No full endpoint ID, address or relay URL appears in the snapshot.
    // (A machine's words come from 33 bits of its key, as ADR 0033 says.)
    let text = String::from_utf8(bytes).unwrap();
    for node in [&kim, &jimmy, &alice] {
        let s = status(node).await;
        let mut forbidden = vec![s.node.to_string(), hex(s.node.as_bytes())];
        forbidden.extend(s.local_addrs);
        forbidden.extend(s.relay);
        for f in forbidden {
            assert!(!text.contains(&f), "the snapshot holds {f}");
        }
    }

    // Opening it again sends nothing to any node, and says it's unchanged.
    let again = call(&kim, universe_request(Some(snapshot.version.clone()))).await;
    assert!(
        matches!(again, ClientResponse::UniverseUnchanged),
        "{again:?}"
    );
    assert!(requests_sent(&kim).await <= before);

    // A request for one layer gets only that one.
    let req = ClientRequest::Universe {
        level: Level::Local,
        region: None,
        layers: vec![Layer::Network],
        since: None,
    };
    let ClientResponse::Universe { snapshot: only } = call(&kim, req).await else {
        panic!("no snapshot")
    };
    assert!(only.people.is_empty() && only.lines.is_empty() && only.network.is_some());

    // Their lines place all three, Kim at the centre, in one group: a
    // world this small opens at Local, with no group drawn closed.
    assert_eq!(snapshot.people[0].at, [0.0, 0.0]);
    assert!(snapshot.groups.is_empty() && snapshot.sky.is_empty());
    assert!(snapshot.people.iter().all(|p| p.group.is_some()));
    assert_eq!((snapshot.region.as_str(), snapshot.path.len()), ("", 0));

    // Standing at Jimmy shows him and Alice, whom he reshared, and the way
    // back up; each step up opens, and none of it asks anyone anything.
    let at = |region: String| ClientRequest::Universe {
        level: Level::Close,
        region: Some(region),
        layers: vec![Layer::People],
        since: None,
    };
    let ClientResponse::Universe { snapshot: close } =
        call(&kim, at(format!("p:{jimmy_id}"))).await
    else {
        panic!("no snapshot at Jimmy")
    };
    assert_eq!(close.level, Level::Close);
    assert_eq!(close.region, format!("p:{jimmy_id}"));
    assert_eq!(close.people[0].name, "Kim");
    assert_eq!(close.people[1].name, "Jimmy");
    assert!(close.people.iter().any(|p| p.name == "Alice"));
    assert_eq!(close.path.first().map(String::as_str), Some(""));
    for up in &close.path {
        let resp = call(&kim, at(up.clone())).await;
        assert!(
            matches!(resp, ClientResponse::Universe { .. }),
            "{up}: {resp:?}"
        );
    }
    for bad in ["g:x", "sky:1/2/2"] {
        let resp = kim.handle(at(bad.into())).await;
        assert!(matches!(resp, ClientResponse::Error { .. }), "{resp:?}");
    }
    assert!(requests_sent(&kim).await <= before);

    for node in [kim, jimmy, alice] {
        node.shutdown().await.unwrap();
    }
}
