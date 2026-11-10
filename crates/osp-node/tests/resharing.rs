//! Shared posts found and hosted (ADR 0025, build steps 1 and 2): directories
//! list discoverable people's reshares, check a hosting resharer's copy before
//! saying so, check again, and drop reshares of a deleted post. Jimmy follows
//! nobody, finds Kim's photo, and shares it with hosting; Alice finds it later
//! and fetches Jimmy's copy without asking Kim. Kim's node, told by Jimmy's,
//! names him to anyone who asks, and answers for a deleted post with its
//! `Delete`.
//!
//! These run on the simulated network, which has no Tor.

use std::{
    collections::HashSet,
    io::Write,
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use iroh::{
    EndpointAddr, EndpointId, PublicKey, RelayMode, SecretKey, TransportAddr,
    endpoint::{Connection, Endpoint, presets},
};
use osp_node::{
    Config, KnownDirectory, Node, SimSettings,
    simnet::{self, Link, SimNet},
};
use osp_proto::{
    BlobHash, MACHINE_ALPN, ObjectId, SOCIAL_ALPN,
    framing::{read_frame, write_frame},
    machine::{ListingItem, MachineRequest, MachineResponse},
    rpc::{ClientRequest, ClientResponse},
    social::{
        MEDIA_HOSTS_ALPN, MediaHostsRequest, MediaHostsResponse, SocialRequest, SocialResponse,
    },
};

static LOGS: Mutex<String> = Mutex::new(String::new());

#[derive(Clone)]
struct Capture;

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        LOGS.lock().unwrap().push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_logs() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        tracing_subscriber::fmt()
            .with_env_filter("osp_node=info,iroh=warn")
            .with_ansi(false)
            .with_writer(|| Capture)
            .init();
    });
}

const SOON: Duration = Duration::from_millis(200);

fn sim(net: &SimNet) -> SimSettings {
    SimSettings {
        network: Some(net.link(Link::default())),
        machine_names: true,
        ..SimSettings::default()
    }
}

async fn start_sim(root: &Path, config: Config, settings: SimSettings) -> Node {
    let home = root.join(config.name.to_lowercase());
    Node::init(&home, &config).unwrap();
    Node::spawn_sim(&home, settings).await.unwrap()
}

/// A directory that reads those it lists often, and checks resharers' copies
/// again every `recheck`, listing at most `most` hosting reshares of a post.
async fn start_directory(root: &Path, net: &SimNet, recheck: Duration, most: usize) -> Node {
    let mut config = Config::local_test("The directory");
    config.endpoints.enabled = true;
    config.directory.serve = true;
    let settings = SimSettings {
        listed_read_every: SOON,
        reshare_recheck_every: recheck,
        max_hosting_reshares: most,
        ..sim(net)
    };
    start_sim(root, config, settings).await
}

fn config_knowing(name: &str, directory: &Node, list_me: bool) -> Config {
    let mut config = Config::local_test(name);
    config.endpoints.enabled = true;
    config.directory.list_me = list_me;
    config.directory.known = vec![KnownDirectory {
        machine: directory.machine_id().unwrap(),
        node: directory.node_id(),
    }];
    config
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

async fn read_directories(node: &Node) {
    match call(node, ClientRequest::ReadDirectories).await {
        ClientResponse::DirectoriesRead { .. } => {}
        other => panic!("unexpected {other:?}"),
    }
}

/// Posts a photo made by ffmpeg, captioned `text`. Returns the post and the
/// photo.
async fn post_photo(node: &Node, dir: &Path, text: &str) -> (ObjectId, BlobHash) {
    let photo = dir.join(format!("{}.png", node.identity()));
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i"])
        .arg(format!("testsrc=size=320x{}", 200 + text.len()))
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let req = ClientRequest::Post {
        content: text.into(),
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

async fn reshare(node: &Node, id: ObjectId, host: bool) {
    call(node, ClientRequest::Reshare { id, host }).await;
}

fn sim_addr(id: EndpointId) -> EndpointAddr {
    EndpointAddr::from_parts(id, [TransportAddr::Custom(simnet::addr(id))])
}

/// What the directory lists of `actor` now, as any stranger reads it: None if
/// they aren't listed, else each reshare's post and hosting mark.
async fn listed_reshares(
    net: &SimNet,
    directory: &Node,
    actor: PublicKey,
) -> Option<Vec<(ObjectId, bool)>> {
    let key = SecretKey::generate();
    let me = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .add_custom_transport(net.link(Link::default()).transport(key.public()))
        .secret_key(key)
        .bind()
        .await
        .unwrap();
    let conn = me
        .connect(sim_addr(directory.machine_id().unwrap()), MACHINE_ALPN)
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let request = MachineRequest::Listing {
        since: 0,
        limit: 100,
    };
    write_frame(&mut send, &request).await.unwrap();
    send.finish().unwrap();
    let reply: MachineResponse = read_frame(&mut recv).await.unwrap();
    conn.close(0u32.into(), b"done");
    me.close().await;
    let MachineResponse::Listing { entries, .. } = reply else {
        panic!("unexpected {reply:?}");
    };
    entries.into_iter().find_map(|entry| match entry.item {
        ListingItem::Person {
            profile, reshares, ..
        } if profile.signer() == actor => {
            Some(reshares.iter().map(|r| (r.post.id(), r.hosting)).collect())
        }
        _ => None,
    })
}

/// Reads the lists until what they say of `actor` is `wanted`.
async fn wait_for_listing(
    net: &SimNet,
    directory: &Node,
    actor: PublicKey,
    wanted: Option<Vec<(ObjectId, bool)>>,
) {
    let started = Instant::now();
    loop {
        let listed = listed_reshares(net, directory, actor).await;
        if listed == wanted {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "listed {listed:?}, waiting for {wanted:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Kim, listed, with a photo; and the directory.
async fn kim_and_directory(
    root: &Path,
    net: &SimNet,
    recheck: Duration,
    most: usize,
) -> (Node, Node, ObjectId, BlobHash) {
    let directory = start_directory(root, net, recheck, most).await;
    let kim = start_sim(root, config_knowing("Kim", &directory, true), sim(net)).await;
    let (post, photo) = post_photo(&kim, root, "Kim's oak table").await;
    assert!(kim.ask_to_be_listed().await);
    wait_for_listing(net, &directory, kim.identity(), Some(vec![])).await;
    (directory, kim, post, photo)
}

/// Someone who follows nobody, finds Kim's post, and shares it.
async fn resharer(
    root: &Path,
    net: &SimNet,
    directory: &Node,
    name: &str,
    post: ObjectId,
    host: bool,
) -> Node {
    let node = start_sim(root, config_knowing(name, directory, true), sim(net)).await;
    read_directories(&node).await;
    reshare(&node, post, host).await;
    node
}

fn own_endpoints(node: &Node) -> HashSet<EndpointId> {
    let mut ids: HashSet<EndpointId> = node.endpoint_ids().into_iter().collect();
    if let Some(machine) = node.machine_id() {
        ids.remove(&machine);
    }
    ids
}

/// A newcomer finds Jimmy's hosting reshare and fetches his copy through a
/// viewing endpoint, with Kim's node offline throughout: the listing is all
/// it needs.
#[tokio::test(flavor = "multi_thread")]
async fn a_hosting_reshare_is_listed_and_serves_a_newcomer_with_its_author_offline() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(70);
    let (directory, kim, post, photo) =
        kim_and_directory(dir.path(), &net, Duration::from_secs(3600), 16).await;
    let jimmy = resharer(dir.path(), &net, &directory, "Jimmy", post, true).await;
    // Watching then sharing came through a viewing endpoint, not Jimmy's own.
    assert_eq!(jimmy.viewing_endpoints_made(), 1);
    assert!(jimmy.ask_to_be_listed().await);
    wait_for_listing(&net, &directory, jimmy.identity(), Some(vec![(post, true)])).await;

    assert!(net.set_online(kim.node_id(), false));
    let alice = start_sim(
        dir.path(),
        config_knowing("Alice", &directory, false),
        sim(&net),
    )
    .await;
    let own = own_endpoints(&alice);
    read_directories(&alice).await;
    net.record_senders();
    alice.open_blob(&photo).await.unwrap();

    // The directory reads Jimmy all along, and Kim's node checks his copy;
    // anyone else sending to him is Alice.
    let jimmys: HashSet<EndpointId> = jimmy.endpoint_ids().into_iter().collect();
    let checkers: HashSet<EndpointId> = [&directory, &kim]
        .iter()
        .flat_map(|n| n.endpoint_ids())
        .collect();
    let senders = net.senders();
    let to_jimmy: HashSet<EndpointId> = senders
        .iter()
        .filter(|(from, to)| jimmys.contains(to) && !checkers.contains(from))
        .map(|(from, _)| *from)
        .collect();
    assert_eq!(to_jimmy.len(), 1, "{to_jimmy:?}");
    assert!(own.is_disjoint(&to_jimmy));
    let from_own: Vec<_> = senders
        .iter()
        .filter(|(from, _)| own.contains(from))
        .collect();
    assert!(
        from_own.is_empty(),
        "Alice's own endpoints sent: {from_own:?}"
    );

    for node in [alice, jimmy, kim, directory] {
        node.shutdown().await.unwrap();
    }
}

/// A plain reshare is listed, never as hosting. Someone who didn't ask to be
/// listed isn't, reshares and all.
#[tokio::test(flavor = "multi_thread")]
async fn a_plain_reshare_is_never_marked_hosting_and_an_unlisted_resharer_isnt_listed() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(71);
    let (directory, kim, post, _) =
        kim_and_directory(dir.path(), &net, Duration::from_secs(3600), 16).await;
    let jimmy = resharer(dir.path(), &net, &directory, "Jimmy", post, false).await;
    assert!(jimmy.ask_to_be_listed().await);
    wait_for_listing(
        &net,
        &directory,
        jimmy.identity(),
        Some(vec![(post, false)]),
    )
    .await;

    let carol = start_sim(
        dir.path(),
        config_knowing("Carol", &directory, false),
        sim(&net),
    )
    .await;
    read_directories(&carol).await;
    reshare(&carol, post, true).await;
    // She asks, but her profile says she may not be listed.
    carol.ask_to_be_listed().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        listed_reshares(&net, &directory, carol.identity()).await,
        None
    );

    for node in [carol, jimmy, kim, directory] {
        node.shutdown().await.unwrap();
    }
}

/// A copy that fails its first check is listed plain. One that passed loses
/// the mark only after 2 failed rechecks in a row.
#[tokio::test(flavor = "multi_thread")]
async fn failed_checks_list_a_hosting_reshare_as_plain() {
    capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(72);
    let (directory, kim, post, photo) =
        kim_and_directory(dir.path(), &net, Duration::from_millis(500), 16).await;

    // Carol's copy is gone before the directory first checks it.
    let carol = resharer(dir.path(), &net, &directory, "Carol", post, true).await;
    carol.let_go_of_blob(&photo).await.unwrap();
    assert!(carol.ask_to_be_listed().await);
    wait_for_listing(
        &net,
        &directory,
        carol.identity(),
        Some(vec![(post, false)]),
    )
    .await;

    // Jimmy's passes, and then is lost.
    let jimmy = resharer(dir.path(), &net, &directory, "Jimmy", post, true).await;
    assert!(jimmy.ask_to_be_listed().await);
    wait_for_listing(&net, &directory, jimmy.identity(), Some(vec![(post, true)])).await;
    jimmy.let_go_of_blob(&photo).await.unwrap();
    let lost = Instant::now();
    wait_for_listing(
        &net,
        &directory,
        jimmy.identity(),
        Some(vec![(post, false)]),
    )
    .await;
    // Two rechecks, half a second apart at the soonest.
    assert!(
        lost.elapsed() >= Duration::from_millis(500),
        "{:?}",
        lost.elapsed()
    );

    for node in [jimmy, carol, kim, directory] {
        node.shutdown().await.unwrap();
    }
}

/// Past the most hosting reshares of one post, another is left out of the
/// listing, and the directory says so in its log.
#[tokio::test(flavor = "multi_thread")]
async fn past_the_most_hosting_reshares_of_a_post_another_is_left_out() {
    capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(73);
    let (directory, kim, post, _) =
        kim_and_directory(dir.path(), &net, Duration::from_secs(3600), 1).await;
    let jimmy = resharer(dir.path(), &net, &directory, "Jimmy", post, true).await;
    assert!(jimmy.ask_to_be_listed().await);
    wait_for_listing(&net, &directory, jimmy.identity(), Some(vec![(post, true)])).await;
    let carol = resharer(dir.path(), &net, &directory, "Carol", post, true).await;
    assert!(carol.ask_to_be_listed().await);
    wait_for_listing(&net, &directory, carol.identity(), Some(vec![])).await;
    assert!(
        LOGS.lock()
            .unwrap()
            .contains("a post has more than 1 hosting reshares; leaving this one out"),
        "not logged"
    );
    // Jimmy keeps his place.
    wait_for_listing(&net, &directory, jimmy.identity(), Some(vec![(post, true)])).await;

    for node in [carol, jimmy, kim, directory] {
        node.shutdown().await.unwrap();
    }
}

/// Kim deletes her post: the directory keeps a tombstone, drops Jimmy's
/// reshare of it, and doesn't list it again.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_posts_reshares_are_dropped_and_not_listed_again() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(74);
    let (directory, kim, post, _) =
        kim_and_directory(dir.path(), &net, Duration::from_secs(3600), 16).await;
    let jimmy = resharer(dir.path(), &net, &directory, "Jimmy", post, true).await;
    assert!(jimmy.ask_to_be_listed().await);
    wait_for_listing(&net, &directory, jimmy.identity(), Some(vec![(post, true)])).await;

    call(&kim, ClientRequest::Delete { id: post }).await;
    wait_for_listing(&net, &directory, jimmy.identity(), Some(vec![])).await;
    assert!(directory.knows_deleted(&post).unwrap());
    // Jimmy's node hasn't heard, and still has the reshare in its outbox:
    // the directory reads it again and still leaves it out.
    tokio::time::sleep(Duration::from_secs(1)).await;
    wait_for_listing(&net, &directory, jimmy.identity(), Some(vec![])).await;

    for node in [jimmy, kim, directory] {
        node.shutdown().await.unwrap();
    }
}

// ---- Step 2: the author's node names hosting resharers ----

/// A stranger's endpoint on the simulated network, connected to `to` with
/// `alpn`.
async fn stranger_conn(net: &SimNet, to: EndpointId, alpn: &[u8]) -> (Endpoint, Connection) {
    let key = SecretKey::generate();
    let me = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .add_custom_transport(net.link(Link::default()).transport(key.public()))
        .secret_key(key)
        .bind()
        .await
        .unwrap();
    let conn = me.connect(sim_addr(to), alpn).await.unwrap();
    (me, conn)
}

/// Whose media endpoints `kim`'s node names to a stranger asking who hosts
/// `blob` over its public ID, as `WhoHosts` goes over Tor.
async fn who_hosts_as_stranger(net: &SimNet, kim: &Node, blob: BlobHash) -> HashSet<EndpointId> {
    let (me, conn) = stranger_conn(net, kim.node_id(), SOCIAL_ALPN).await;
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &SocialRequest::WhoHosts { blob })
        .await
        .unwrap();
    send.finish().unwrap();
    let reply: SocialResponse = read_frame(&mut recv).await.unwrap();
    conn.close(0u32.into(), b"done");
    me.close().await;
    let SocialResponse::BoundHosts { hosts } = reply else {
        panic!("unexpected {reply:?}");
    };
    hosts.iter().map(|h| h.binding.public).collect()
}

/// What `kim`'s media endpoint answers a stranger asking who hosts `blob`
/// of `post`, as a viewing endpoint asks.
async fn media_hosts_as_stranger(
    net: &SimNet,
    kim: &Node,
    post: ObjectId,
    blob: BlobHash,
) -> MediaHostsResponse {
    let media = kim.media_addr().await.unwrap().id;
    let (me, conn) = stranger_conn(net, media, MEDIA_HOSTS_ALPN).await;
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &MediaHostsRequest { post, blob })
        .await
        .unwrap();
    send.finish().unwrap();
    let reply: MediaHostsResponse = read_frame(&mut recv).await.unwrap();
    conn.close(0u32.into(), b"done");
    me.close().await;
    reply
}

/// The public IDs of the hosts a media-hosts reply names.
fn named_by(reply: &MediaHostsResponse) -> HashSet<EndpointId> {
    let MediaHostsResponse::Hosts { hosts } = reply else {
        panic!("unexpected {reply:?}");
    };
    hosts.iter().map(|h| h.binding.public).collect()
}

/// Waits until `check` holds.
async fn until(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let started = Instant::now();
    while !check().await {
        assert!(started.elapsed() < Duration::from_secs(30), "{what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Posts a photo of noise, about 3 MB, large enough that a follower that
/// shares fetches it through the follower swarm. Returns the post and photo.
async fn post_large_photo(node: &Node, dir: &Path) -> (ObjectId, BlobHash) {
    let photo = dir.join("noise.png");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i"])
        .arg("nullsrc=s=2048x2048,geq=random(1)*255:random(1)*255:random(1)*255")
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let req = ClientRequest::Post {
        content: "Kim's noise".into(),
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

async fn follow(node: &Node, author: &Node) {
    let ClientResponse::Ticket { ticket } = call(author, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    call(node, ClientRequest::Follow { ticket }).await;
    call(node, ClientRequest::Sync).await;
}

/// Kim's node names Jimmy, who may be listed and hosts her post, to a
/// stranger, over Tor and through her media endpoint alike; never Carol, who
/// hosts it but may not be listed, nor Fred, a follower with a copy. Alice,
/// who follows nobody, then gets the post from Jimmy.
#[tokio::test(flavor = "multi_thread")]
async fn the_author_names_listed_hosting_resharers_to_anyone_never_a_followers_copy() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(75);
    let (directory, kim, _, _) =
        kim_and_directory(dir.path(), &net, Duration::from_secs(3600), 16).await;
    let (post, photo) = post_large_photo(&kim, dir.path()).await;

    let mut config = config_knowing("Fred", &directory, false);
    config.swarm.share_prefetched = true;
    let settings = SimSettings {
        prefetch_spread: Duration::ZERO,
        ..sim(&net)
    };
    let fred = start_sim(dir.path(), config, settings).await;
    follow(&fred, &kim).await;
    until("Fred's copy isn't noted", async || {
        kim.swarm_copies(&fred.node_id()).unwrap() > 0
    })
    .await;

    let jimmy = resharer(dir.path(), &net, &directory, "Jimmy", post, true).await;
    let carol = start_sim(
        dir.path(),
        config_knowing("Carol", &directory, false),
        sim(&net),
    )
    .await;
    read_directories(&carol).await;
    reshare(&carol, post, true).await;
    for node in [&jimmy, &carol] {
        until("a resharer isn't noted", async || {
            kim.notes_resharer(&post, &node.node_id()).unwrap()
        })
        .await;
    }

    let wanted = HashSet::from([kim.node_id(), jimmy.node_id()]);
    assert_eq!(who_hosts_as_stranger(&net, &kim, photo).await, wanted);
    let reply = media_hosts_as_stranger(&net, &kim, post, photo).await;
    assert_eq!(named_by(&reply), wanted);

    // Alice follows nobody, and isn't told of Jimmy by any directory.
    let alice = start_sim(
        dir.path(),
        config_knowing("Alice", &directory, false),
        sim(&net),
    )
    .await;
    let own = own_endpoints(&alice);
    read_directories(&alice).await;
    net.record_senders();
    alice.open_blob(&photo).await.unwrap();
    let jimmys: HashSet<EndpointId> = jimmy.endpoint_ids().into_iter().collect();
    let others: HashSet<EndpointId> = [&directory, &kim]
        .iter()
        .flat_map(|n| n.endpoint_ids())
        .collect();
    let to_jimmy: HashSet<EndpointId> = net
        .senders()
        .iter()
        .filter(|(from, to)| jimmys.contains(to) && !others.contains(from))
        .map(|(from, _)| *from)
        .collect();
    assert_eq!(to_jimmy.len(), 1, "{to_jimmy:?}");
    assert!(own.is_disjoint(&to_jimmy));

    for node in [alice, carol, jimmy, fred, kim, directory] {
        node.shutdown().await.unwrap();
    }
}

/// Kim deletes her post: her media endpoint answers with her `Delete`, and
/// Alice, who found it earlier, keeps it and doesn't get the media.
#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_posts_media_endpoint_answers_with_its_delete() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(76);
    let (directory, kim, post, photo) =
        kim_and_directory(dir.path(), &net, Duration::from_secs(3600), 16).await;
    let alice = start_sim(
        dir.path(),
        config_knowing("Alice", &directory, false),
        sim(&net),
    )
    .await;
    read_directories(&alice).await;

    call(&kim, ClientRequest::Delete { id: post }).await;
    let reply = media_hosts_as_stranger(&net, &kim, post, photo).await;
    let MediaHostsResponse::Deleted { delete } = reply else {
        panic!("unexpected {reply:?}");
    };
    assert_eq!(delete.signer(), kim.identity());

    let opened = alice.open_blob(&photo).await;
    assert!(opened.is_err(), "{opened:?}");
    assert!(alice.knows_deleted(&post).unwrap());

    for node in [alice, kim, directory] {
        node.shutdown().await.unwrap();
    }
}

/// Jimmy's copy is lost: after 2 failed checks in a row, Kim's node stops
/// naming him.
#[tokio::test(flavor = "multi_thread")]
async fn a_resharer_failing_two_checks_in_a_row_stops_being_named() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(77);
    let directory = start_directory(dir.path(), &net, Duration::from_secs(3600), 16).await;
    let settings = SimSettings {
        reshare_recheck_every: Duration::from_millis(500),
        ..sim(&net)
    };
    let kim = start_sim(
        dir.path(),
        config_knowing("Kim", &directory, true),
        settings,
    )
    .await;
    let (post, photo) = post_photo(&kim, dir.path(), "Kim's oak table").await;
    assert!(kim.ask_to_be_listed().await);
    wait_for_listing(&net, &directory, kim.identity(), Some(vec![])).await;

    let jimmy = resharer(dir.path(), &net, &directory, "Jimmy", post, true).await;
    until("Jimmy isn't named", async || {
        named_by(&media_hosts_as_stranger(&net, &kim, post, photo).await).contains(&jimmy.node_id())
    })
    .await;
    jimmy.let_go_of_blob(&photo).await.unwrap();
    let lost = Instant::now();
    until("Jimmy is still named", async || {
        !named_by(&media_hosts_as_stranger(&net, &kim, post, photo).await)
            .contains(&jimmy.node_id())
    })
    .await;
    assert!(
        lost.elapsed() >= Duration::from_millis(500),
        "{:?}",
        lost.elapsed()
    );
    // Still noted: a check that passes names him again.
    assert!(kim.notes_resharer(&post, &jimmy.node_id()).unwrap());

    for node in [jimmy, kim, directory] {
        node.shutdown().await.unwrap();
    }
}

/// The posts a directory lists of `actor`, by ID.
async fn listed_posts(net: &SimNet, directory: &Node, actor: PublicKey) -> Vec<ObjectId> {
    let (me, conn) = stranger_conn(net, directory.machine_id().unwrap(), MACHINE_ALPN).await;
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let request = MachineRequest::Listing {
        since: 0,
        limit: 100,
    };
    write_frame(&mut send, &request).await.unwrap();
    send.finish().unwrap();
    let reply: MachineResponse = read_frame(&mut recv).await.unwrap();
    conn.close(0u32.into(), b"done");
    me.close().await;
    let MachineResponse::Listing { entries, .. } = reply else {
        panic!("unexpected {reply:?}");
    };
    entries
        .into_iter()
        .find_map(|entry| match entry.item {
            ListingItem::Person { profile, posts, .. } if profile.signer() == actor => {
                Some(posts.iter().map(|p| p.id()).collect())
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// A directory that follows Kim hears of her `Delete` through its follow, and
/// drops the post and Jimmy's reshare of it from its lists at once, not at its
/// next read of Kim an hour later. Found on real nodes, 2026-11-10.
#[tokio::test(flavor = "multi_thread")]
async fn a_directory_that_follows_the_author_drops_a_deleted_post_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(78);
    let mut config = Config::local_test("The directory");
    config.endpoints.enabled = true;
    config.directory.serve = true;
    let settings = SimSettings {
        listed_read_every: Duration::from_secs(3600),
        ..sim(&net)
    };
    let directory = start_sim(dir.path(), config, settings).await;
    let kim = start_sim(
        dir.path(),
        config_knowing("Kim", &directory, true),
        sim(&net),
    )
    .await;
    let (post, _) = post_photo(&kim, dir.path(), "Kim's oak table").await;
    assert!(kim.ask_to_be_listed().await);
    until("Kim's post isn't listed", async || {
        listed_posts(&net, &directory, kim.identity()).await == vec![post]
    })
    .await;
    let jimmy = resharer(dir.path(), &net, &directory, "Jimmy", post, true).await;
    assert!(jimmy.ask_to_be_listed().await);
    wait_for_listing(&net, &directory, jimmy.identity(), Some(vec![(post, true)])).await;
    follow(&directory, &kim).await;

    call(&kim, ClientRequest::Delete { id: post }).await;
    wait_for_listing(&net, &directory, jimmy.identity(), Some(vec![])).await;
    assert!(
        listed_posts(&net, &directory, kim.identity())
            .await
            .is_empty()
    );
    assert!(directory.knows_deleted(&post).unwrap());

    for node in [jimmy, kim, directory] {
        node.shutdown().await.unwrap();
    }
}
