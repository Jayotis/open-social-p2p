//! The follower swarm (doc 13): what a follower's node fetched ahead of time
//! is served to anyone only if its owner chose to share it.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.
//! Requires ffmpeg.

use std::{net::SocketAddr, path::Path};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, address_lookup::MemoryLookup,
    endpoint::presets,
};
use osp_node::{BlobClass, Config, Home, Node};
use osp_proto::{
    Attachment, SOCIAL_ALPN,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse},
    social::{SocialRequest, SocialResponse},
};

async fn start(root: &Path, name: &str, share_prefetched: bool) -> Node {
    let home = root.join(name.to_lowercase());
    let mut config = Config::local_test(name);
    config.swarm.share_prefetched = share_prefetched;
    Node::init(&home, &config).unwrap();
    Node::spawn(&home).await.unwrap()
}

/// Stops a node, changes whether it shares, and starts it again.
async fn restart_sharing(node: Node, root: &Path, name: &str, share_prefetched: bool) -> Node {
    node.shutdown().await.unwrap();
    let home = Home::new(root.join(name.to_lowercase()));
    let mut config = Config::load(&home.config()).unwrap();
    config.swarm.share_prefetched = share_prefetched;
    config.save(&home.config()).unwrap();
    Node::spawn(home.root()).await.unwrap()
}

async fn call(node: &Node, req: ClientRequest) -> ClientResponse {
    match node.handle(req).await {
        ClientResponse::Error { message } => panic!("request failed: {message}"),
        resp => resp,
    }
}

/// Posts a small photo. Returns its attachment.
async fn post_photo(node: &Node, dir: &Path) -> Attachment {
    let photo = dir.join("photo.png");
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc=size=640x360"])
        .args(["-frames:v", "1"])
        .arg(&photo)
        .status()
        .await
        .expect("ffmpeg must be installed");
    assert!(status.success());
    let post = ClientRequest::Post {
        content: "a photo".into(),
        attachments: vec![photo],
        uploads: vec![],
    };
    call(node, post).await;
    let ClientResponse::Feed { items } = call(node, ClientRequest::Feed { limit: 1 }).await else {
        panic!("no feed");
    };
    items[0].attachments[0].attachment.clone()
}

/// Asks `node`, as a stranger would, which nodes host `blob`.
async fn who_hosts(node: &EndpointAddr, blob: osp_proto::BlobHash) -> Vec<EndpointId> {
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
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &SocialRequest::WhoHosts { blob })
        .await
        .unwrap();
    send.finish().unwrap();
    match read_frame(&mut recv).await.unwrap() {
        SocialResponse::Hosts { nodes } => nodes.into_iter().map(|a| a.id).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_followers_that_share_serve_what_they_prefetched() {
    let dir = tempfile::tempdir().unwrap();
    let alice = start(dir.path(), "Alice", false).await;
    let bob = start(dir.path(), "Bob", true).await;
    let carol = start(dir.path(), "Carol", false).await;
    let photo = post_photo(&alice, dir.path()).await;

    // Bob and Carol follow Alice and fetch her photo ahead of time.
    let ClientResponse::Ticket { ticket } = call(&alice, ClientRequest::Invite).await else {
        panic!("no ticket");
    };
    for node in [&bob, &carol] {
        let ticket = ticket.clone();
        call(node, ClientRequest::Follow { ticket }).await;
        call(node, ClientRequest::Sync).await;
        assert_eq!(
            node.blob_class(&photo.blob).unwrap(),
            Some(BlobClass::Prefetch)
        );
    }

    // A stranger may download Bob's copy, but not Carol's.
    let dave = start(dir.path(), "Dave", false).await;
    let from = async |node: &Node| {
        dave.download_blob_from(&photo.blob, photo.size, &[node.addr().await])
            .await
    };
    assert!(from(&carol).await.is_err(), "Carol doesn't share");
    from(&bob).await.expect("Bob shares");

    // Each names itself as a host only if it serves the photo to anyone.
    let hosts = who_hosts(&bob.addr().await, photo.blob).await;
    assert!(hosts.contains(&bob.node_id()), "{hosts:?}");
    assert!(hosts.contains(&alice.node_id()), "{hosts:?}");
    let hosts = who_hosts(&carol.addr().await, photo.blob).await;
    assert!(!hosts.contains(&carol.node_id()), "{hosts:?}");
    assert!(hosts.contains(&alice.node_id()), "{hosts:?}");

    // Sharing can be turned off: the copy stays, served only to Bob's devices.
    let bob = restart_sharing(bob, dir.path(), "Bob", false).await;
    assert_eq!(
        bob.blob_class(&photo.blob).unwrap(),
        Some(BlobClass::Prefetch)
    );
    let erin = start(dir.path(), "Erin", false).await;
    let got = erin
        .download_blob_from(&photo.blob, photo.size, &[bob.addr().await])
        .await;
    assert!(got.is_err(), "Bob stopped sharing");
    let hosts = who_hosts(&bob.addr().await, photo.blob).await;
    assert!(!hosts.contains(&bob.node_id()), "{hosts:?}");

    for node in [alice, bob, carol, dave, erin] {
        node.shutdown().await.unwrap();
    }
}
