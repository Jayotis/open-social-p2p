//! Watching a found post without showing who you are (ADR 0024, build step
//! 2): Alice follows nobody, finds Kim's post through a directory, and opens
//! its media. Every packet on the simulated network is noted by sender and
//! receiver, so the tests can say that none came from Alice's public, media
//! or direct endpoint. Her public endpoint is the one that would carry Tor
//! requests, so none of those either.
//!
//! These run on the simulated network, which has no Tor.

use std::{
    collections::HashSet,
    path::Path,
    time::{Duration, Instant},
};

use iroh::{
    EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr,
    endpoint::{Endpoint, presets},
};
use osp_node::{
    BlobClass, Config, KnownDirectory, Node, SimSettings,
    simnet::{self, Link, SimNet},
};
use osp_proto::{
    BlobHash, MACHINE_ALPN,
    framing::{read_frame, write_frame},
    machine::{ListingEntry, ListingItem, MachineRequest, MachineResponse},
    rpc::{ClientRequest, ClientResponse},
};

const AN_HOUR: Duration = Duration::from_secs(60 * 60);

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

async fn start_directory(root: &Path, net: &SimNet) -> Node {
    let mut config = Config::local_test("The directory");
    config.directory.serve = true;
    let settings = SimSettings {
        listed_read_every: AN_HOUR,
        ..sim(net)
    };
    start_sim(root, config, settings).await
}

/// Someone with media and direct endpoints who knows `directory`.
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

/// A creator who posted a photo, listed by `directory`. Returns the photo
/// and its thumbnail.
async fn listed_creator(
    root: &Path,
    net: &SimNet,
    directory: &Node,
    name: &str,
) -> (Node, BlobHash, BlobHash) {
    let node = start_sim(root, config_knowing(name, directory, true), sim(net)).await;
    let (photo, thumbnail) = post_photo(&node, root, &format!("{name}'s oak table")).await;
    assert!(node.ask_to_be_listed().await);
    wait_until_listed(net, directory, node.identity()).await;
    (node, photo, thumbnail)
}

/// Alice: follows nobody, and has found everyone `directory` lists.
async fn alice(root: &Path, directory: &Node, settings: SimSettings) -> Node {
    let alice = start_sim(root, config_knowing("Alice", directory, false), settings).await;
    match call(&alice, ClientRequest::ReadDirectories).await {
        ClientResponse::DirectoriesRead { found, .. } => assert!(found >= 1),
        other => panic!("unexpected {other:?}"),
    }
    alice
}

/// Posts a photo made by ffmpeg, captioned `text`. Returns the photo and its
/// thumbnail.
async fn post_photo(node: &Node, dir: &Path, text: &str) -> (BlobHash, BlobHash) {
    let photo = dir.join(format!("{}.png", node.identity()));
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            // A size of its own, so two people's photos aren't one file.
            &format!("testsrc=size=320x{}", 200 + text.len()),
            "-frames:v",
            "1",
        ])
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
    let attachment = &item.attachments[0].attachment;
    (attachment.blob, attachment.thumbnail.as_ref().unwrap().blob)
}

fn sim_addr(id: EndpointId) -> EndpointAddr {
    EndpointAddr::from_parts(id, [TransportAddr::Custom(simnet::addr(id))])
}

/// Reads the directory's lists, as any stranger can, until they list `actor`.
async fn wait_until_listed(net: &SimNet, directory: &Node, actor: iroh::PublicKey) {
    let started = Instant::now();
    loop {
        let key = SecretKey::generate();
        let me = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .clear_ip_transports()
            .add_custom_transport(net.link(Link::default()).transport(key.public()))
            .secret_key(key)
            .bind()
            .await
            .unwrap();
        let machine = directory.machine_id().unwrap();
        let conn = me.connect(sim_addr(machine), MACHINE_ALPN).await.unwrap();
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
        let listed = |entry: &ListingEntry| matches!(&entry.item, ListingItem::Person { profile, .. } if profile.signer() == actor);
        if entries.iter().any(listed) {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "never listed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Alice's public, media and direct endpoints: none of them may send
/// anything for a found post. Not her machine name, which reads directories.
fn own_endpoints(alice: &Node) -> HashSet<EndpointId> {
    let mut ids: HashSet<EndpointId> = alice.endpoint_ids().into_iter().collect();
    if let Some(machine) = alice.machine_id() {
        ids.remove(&machine);
    }
    assert_eq!(ids.len(), 3, "public, media and direct");
    ids
}

/// Fails if any of `own` sent anything since recording began.
fn assert_sent_nothing_from(net: &SimNet, own: &HashSet<EndpointId>) {
    let from_own: Vec<_> = net
        .senders()
        .into_iter()
        .filter(|(from, _)| own.contains(from))
        .collect();
    assert!(
        from_own.is_empty(),
        "Alice's own endpoints sent: {from_own:?}"
    );
}

/// Who sent to any of `node`'s endpoints since recording began, leaving out
/// the `others` in the test: a directory reading `node`, or another creator's
/// machine name meeting it, runs on its own timers and isn't Alice.
fn senders_to(net: &SimNet, node: &Node, others: &[&Node]) -> HashSet<EndpointId> {
    let ids: HashSet<EndpointId> = node.endpoint_ids().into_iter().collect();
    let others: HashSet<EndpointId> = others.iter().flat_map(|n| n.endpoint_ids()).collect();
    net.senders()
        .into_iter()
        .filter(|(from, to)| ids.contains(to) && !others.contains(from))
        .map(|(from, _)| from)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_found_post_opens_through_a_viewing_endpoint_and_nothing_of_alices_own() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(60);
    let directory = start_directory(dir.path(), &net).await;
    let (kim, photo, thumbnail) = listed_creator(dir.path(), &net, &directory, "Kim").await;
    let alice = alice(dir.path(), &directory, sim(&net)).await;
    let own = own_endpoints(&alice);
    assert_eq!(alice.blob_class(&photo).unwrap(), None);

    net.record_senders();
    alice.open_blob(&thumbnail).await.unwrap();
    alice.open_blob(&photo).await.unwrap();

    // One viewing endpoint for both, a key Kim's node had never seen.
    assert_eq!(alice.viewing_endpoints_made(), 1);
    let to_kim = senders_to(&net, &kim, &[&directory]);
    assert_eq!(to_kim.len(), 1, "{to_kim:?}");
    let viewer = *to_kim.iter().next().unwrap();
    assert!(!alice.endpoint_ids().contains(&viewer));
    assert_sent_nothing_from(&net, &own);
    // It stays private to Alice: watching publishes nothing.
    assert_eq!(alice.blob_class(&photo).unwrap(), Some(BlobClass::Cache));

    // When she follows Kim, her node goes back to its own endpoints.
    let follow = ClientRequest::FollowPerson {
        actor: kim.identity(),
    };
    call(&alice, follow).await;
    call(&alice, ClientRequest::Sync).await;
    assert!(
        senders_to(&net, &kim, &[&directory])
            .iter()
            .any(|from| own.contains(from))
    );
    assert_eq!(alice.viewing_endpoints_made(), 1);

    for node in [alice, kim, directory] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_author_has_their_own_viewing_endpoint_and_a_late_request_a_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(61);
    let directory = start_directory(dir.path(), &net).await;
    let (kim, kims_photo, kims_thumbnail) =
        listed_creator(dir.path(), &net, &directory, "Kim").await;
    let (jimmy, jimmys_photo, _) = listed_creator(dir.path(), &net, &directory, "Jimmy").await;
    let settings = SimSettings {
        viewing_reuse: Duration::from_secs(2),
        ..sim(&net)
    };
    let alice = alice(dir.path(), &directory, settings).await;
    let own = own_endpoints(&alice);
    net.record_senders();

    alice.open_blob(&kims_thumbnail).await.unwrap();
    alice.open_blob(&jimmys_photo).await.unwrap();
    assert_eq!(alice.viewing_endpoints_made(), 2);
    let to_kim = senders_to(&net, &kim, &[&directory, &jimmy]);
    let to_jimmy = senders_to(&net, &jimmy, &[&directory, &kim]);
    assert!(to_kim.is_disjoint(&to_jimmy), "{to_kim:?} {to_jimmy:?}");

    // Past the time a viewing endpoint takes new requests, Kim's photo comes
    // through a new one.
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    alice.open_blob(&kims_photo).await.unwrap();
    assert_eq!(alice.viewing_endpoints_made(), 3);
    assert_eq!(senders_to(&net, &kim, &[&directory, &jimmy]).len(), 2);
    assert_sent_nothing_from(&net, &own);

    for node in [alice, kim, jimmy, directory] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_source_reachable_privately_it_fails_plainly_and_a_retry_works() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(62);
    let directory = start_directory(dir.path(), &net).await;
    let (kim, photo, _) = listed_creator(dir.path(), &net, &directory, "Kim").await;
    let alice = alice(dir.path(), &directory, sim(&net)).await;
    let own = own_endpoints(&alice);
    net.record_senders();

    assert!(net.set_online(kim.node_id(), false));
    let err = alice.open_blob(&photo).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("without showing who you are"),
        "{err:#}"
    );
    assert_sent_nothing_from(&net, &own);
    assert_eq!(alice.blob_class(&photo).unwrap(), None);

    // She tries again once Kim is back, and it's still only a viewing key.
    assert!(net.set_online(kim.node_id(), true));
    alice.open_blob(&photo).await.unwrap();
    assert_sent_nothing_from(&net, &own);

    for node in [alice, kim, directory] {
        node.shutdown().await.unwrap();
    }
}
