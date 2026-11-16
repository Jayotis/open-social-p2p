//! Opened posts (ADR 0039 §4): Jimmy sends Kim a code for Alice's post,
//! and Kim doesn't follow Alice. Kim's node fetches the post and Alice's
//! profile from a throwaway key and keeps them as an opened post while she
//! looks: shown, with its media, but in no list, and not to be liked or
//! replied to; nothing of it goes from Kim's own endpoints. Leaving drops
//! it at once, a code for a post Alice deleted says so, and following Alice
//! or resharing it makes it an ordinary post.
//!
//! These run on the simulated network, which has no Tor.

use std::{collections::HashSet, path::Path};

use iroh::{EndpointAddr, EndpointId};
use osp_node::{
    BlobClass, Config, Node, SimSettings,
    simnet::{Link, SimNet},
};
use osp_proto::{
    BlobHash, FollowTicket, ObjectId,
    rpc::{ClientRequest, ClientResponse, FeedItem},
};

fn sim(net: &SimNet) -> SimSettings {
    SimSettings {
        network: Some(net.link(Link::default())),
        ..SimSettings::default()
    }
}

async fn start(root: &Path, net: &SimNet, name: &str) -> Node {
    let home = root.join(name.to_lowercase());
    let mut config = Config::local_test(name);
    config.endpoints.enabled = true;
    Node::init(&home, &config).unwrap();
    Node::spawn_sim(&home, sim(net)).await.unwrap()
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

/// Posts a photo made by ffmpeg, captioned `text`. Returns the post and
/// the photo.
async fn post_photo(node: &Node, dir: &Path, text: &str) -> (ObjectId, BlobHash) {
    let photo = dir.join(format!("{}.png", node.identity()));
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240",
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
    let item = show(node, id).await.unwrap();
    (id, item.attachments[0].attachment.blob)
}

async fn show(node: &Node, id: ObjectId) -> Result<FeedItem, String> {
    match node.handle(ClientRequest::Show { id }).await {
        ClientResponse::Item { item } => Ok(item),
        ClientResponse::Error { message } => Err(message),
        other => panic!("unexpected {other:?}"),
    }
}

/// Alice's ticket as an OSP code carries it: her node by its ID alone.
async fn code_ticket(alice: &Node) -> FollowTicket {
    let ClientResponse::Ticket { ticket } = call(alice, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    let mut ticket: FollowTicket = ticket.parse().unwrap();
    ticket.node = EndpointAddr::new(ticket.node.id);
    ticket
}

async fn open(kim: &Node, id: ObjectId, ticket: &FollowTicket) -> FeedItem {
    let req = ClientRequest::OpenPost {
        id,
        ticket: ticket.to_string(),
    };
    let ClientResponse::Item { item } = call(kim, req).await else {
        panic!("not an item")
    };
    item
}

/// The people Kim's node lists, by identity.
async fn people(kim: &Node) -> HashSet<iroh::PublicKey> {
    let ClientResponse::People { people } = call(kim, ClientRequest::People { limit: 100 }).await
    else {
        panic!("not a list of people")
    };
    people.iter().map(|p| p.actor).collect()
}

/// What a search finds: people and posts.
async fn search(kim: &Node, words: &str) -> (usize, usize) {
    let req = ClientRequest::Search {
        words: words.into(),
        limit: 20,
    };
    match call(kim, req).await {
        ClientResponse::Found { people, posts } => (people.len(), posts.len()),
        other => panic!("unexpected {other:?}"),
    }
}

async fn feed_ids(kim: &Node) -> Vec<ObjectId> {
    match call(kim, ClientRequest::Feed { limit: 50 }).await {
        ClientResponse::Feed { items } => items.iter().map(|i| i.id).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

/// Kim's public, media and direct endpoints.
fn own_endpoints(kim: &Node) -> HashSet<EndpointId> {
    let ids: HashSet<EndpointId> = kim.endpoint_ids().into_iter().collect();
    assert_eq!(ids.len(), 3, "public, media and direct");
    ids
}

#[tokio::test(flavor = "multi_thread")]
async fn an_opened_post_shows_with_its_media_from_a_throwaway_key_and_in_no_list() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(70);
    let alice = start(dir.path(), &net, "Alice").await;
    let kim = start(dir.path(), &net, "Kim").await;
    let (id, photo) = post_photo(&alice, dir.path(), "Alice's walnut bench").await;
    let ticket = code_ticket(&alice).await;
    let own = own_endpoints(&kim);

    net.record_senders();
    let item = open(&kim, id, &ticket).await;
    assert_eq!(kim.throwaways_made(), 1);
    assert!(item.opened);
    assert_eq!(item.actor_name.as_deref(), Some("Alice"));
    assert_eq!(item.content, "Alice's walnut bench");

    // Its media comes the way a found post's does, kept for Kim alone.
    kim.open_blob(&photo).await.unwrap();
    assert_eq!(kim.blob_class(&photo).unwrap(), Some(BlobClass::Cache));
    let from_own: Vec<_> = net
        .senders()
        .into_iter()
        .filter(|(from, _)| own.contains(from))
        .collect();
    assert!(
        from_own.is_empty(),
        "Kim's own endpoints sent: {from_own:?}"
    );

    // In no list: not People, not search, not the feed.
    assert!(!people(&kim).await.contains(&alice.identity()));
    assert_eq!(search(&kim, "walnut").await, (0, 0));
    assert_eq!(search(&kim, "Alice").await, (0, 0));
    assert!(!feed_ids(&kim).await.contains(&id));

    // Opening it again while it's here shows it as opened still.
    assert!(open(&kim, id, &ticket).await.opened);
    // Liking or replying needs a follow: it's gone when Kim leaves.
    let like = ClientRequest::Like {
        post: id,
        liked: true,
    };
    assert!(refused(&kim, like).await.contains("follow its author"));
    let reply = ClientRequest::Reply {
        post: id,
        content: "Nice".into(),
    };
    assert!(refused(&kim, reply).await.contains("follow its author"));

    // Kim leaves it: the post and its media go now, and Alice's profile
    // with them.
    call(&kim, ClientRequest::ForgetOpened { id }).await;
    assert!(show(&kim, id).await.is_err());
    assert_eq!(kim.blob_class(&photo).unwrap(), None);
    let for_alice = ClientRequest::InviteFor {
        actor: alice.identity(),
    };
    assert!(refused(&kim, for_alice).await.contains("holds no profile"));
    // Only an opened post can be removed this way.
    let (own_post, _) = post_photo(&kim, dir.path(), "Kim's own").await;
    let message = refused(&kim, ClientRequest::ForgetOpened { id: own_post }).await;
    assert!(message.contains("isn't an opened post"), "{message}");

    for node in [kim, alice] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn alices_delete_takes_an_opened_post_away_when_kim_opens_it_again() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(71);
    let alice = start(dir.path(), &net, "Alice").await;
    let kim = start(dir.path(), &net, "Kim").await;
    let (id, _) = post_photo(&alice, dir.path(), "Gone soon").await;
    let ticket = code_ticket(&alice).await;
    assert!(open(&kim, id, &ticket).await.opened);

    // Kim leaves it, and Alice deletes it.
    call(&kim, ClientRequest::ForgetOpened { id }).await;
    call(&alice, ClientRequest::Delete { id }).await;
    // Opening the code again asks Alice's node afresh, which says so.
    let req = ClientRequest::OpenPost {
        id,
        ticket: ticket.to_string(),
    };
    let message = refused(&kim, req).await;
    assert!(message.contains("deleted"), "{message}");

    for node in [kim, alice] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn following_alice_makes_an_opened_post_an_ordinary_one() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(72);
    let alice = start(dir.path(), &net, "Alice").await;
    let kim = start(dir.path(), &net, "Kim").await;
    let (id, _) = post_photo(&alice, dir.path(), "Kept for good").await;
    let ticket = code_ticket(&alice).await;
    assert!(open(&kim, id, &ticket).await.opened);

    let ClientResponse::Ticket { ticket: full } = call(&alice, ClientRequest::Invite).await else {
        panic!("no ticket")
    };
    call(&kim, ClientRequest::Follow { ticket: full }).await;
    // The marker goes; the post and Alice's profile stay, now in the lists.
    let item = show(&kim, id).await.unwrap();
    assert!(!item.opened);
    assert_eq!(item.actor_name.as_deref(), Some("Alice"));
    assert!(people(&kim).await.contains(&alice.identity()));
    assert_eq!(search(&kim, "good").await.1, 1);
    // Nothing left to remove as opened.
    let message = refused(&kim, ClientRequest::ForgetOpened { id }).await;
    assert!(message.contains("isn't an opened post"), "{message}");

    for node in [kim, alice] {
        node.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn resharing_an_opened_post_makes_it_an_ordinary_one() {
    let dir = tempfile::tempdir().unwrap();
    let net = SimNet::new(73);
    let alice = start(dir.path(), &net, "Alice").await;
    let kim = start(dir.path(), &net, "Kim").await;
    let (id, photo) = post_photo(&alice, dir.path(), "Worth passing on").await;
    let ticket = code_ticket(&alice).await;
    assert!(open(&kim, id, &ticket).await.opened);
    kim.open_blob(&photo).await.unwrap();

    let req = ClientRequest::Reshare { id, host: false };
    let ClientResponse::Posted { id: reshare } = call(&kim, req).await else {
        panic!("not reshared")
    };
    // The marker goes; the post, its media and Alice's profile stay.
    let item = show(&kim, id).await.unwrap();
    assert!(!item.opened);
    assert_eq!(item.actor_name.as_deref(), Some("Alice"));
    assert!(show(&kim, reshare).await.is_ok());
    assert!(kim.blob_class(&photo).unwrap().is_some());
    assert!(people(&kim).await.contains(&alice.identity()));
    let message = refused(&kim, ClientRequest::ForgetOpened { id }).await;
    assert!(message.contains("isn't an opened post"), "{message}");

    for node in [kim, alice] {
        node.shutdown().await.unwrap();
    }
}
