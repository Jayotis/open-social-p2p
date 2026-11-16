//! Tickets for OSP codes (ADR 0039 §4): `InviteFor` gives the owner's own
//! ticket, or one for someone whose profile the node holds, naming their
//! first node by its ID alone; and refuses someone it holds nothing of.
//!
//! All nodes run in this process on loopback with relays and DNS lookup off.

use std::path::Path;

use osp_node::{Config, Node};
use osp_proto::{
    FollowTicket, OspCode,
    rpc::{ClientRequest, ClientResponse},
};

async fn start(root: &Path, name: &str) -> Node {
    let home = root.join(name.to_lowercase());
    Node::init(&home, &Config::local_test(name)).unwrap();
    Node::spawn(&home).await.unwrap()
}

async fn ticket(node: &Node, req: ClientRequest) -> Result<FollowTicket, String> {
    match node.handle(req).await {
        ClientResponse::Ticket { ticket } => Ok(ticket.parse().unwrap()),
        ClientResponse::Error { message } => Err(message),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn invite_for_gives_the_owners_ticket_or_a_held_profiles_node() {
    let dir = tempfile::tempdir().unwrap();
    let kim = start(dir.path(), "Kim").await;
    let jimmy = start(dir.path(), "Jimmy").await;
    let alice = start(dir.path(), "Alice").await;

    // Kim's own: the same ticket as `Invite`.
    let own = ticket(&kim, ClientRequest::Invite).await.unwrap();
    let for_self = ClientRequest::InviteFor {
        actor: kim.identity(),
    };
    assert_eq!(ticket(&kim, for_self).await.unwrap(), own);

    // Jimmy follows Kim, so holds her profile, and can make a code for her.
    let follow = ClientRequest::Follow {
        ticket: own.to_string(),
    };
    assert!(matches!(jimmy.handle(follow).await, ClientResponse::Ok));
    jimmy.handle(ClientRequest::Sync).await;
    let for_kim = ClientRequest::InviteFor {
        actor: kim.identity(),
    };
    let held = ticket(&jimmy, for_kim).await.unwrap();
    assert_eq!(held.actor, kim.identity());
    assert_eq!(held.node.id, kim.node_id());
    // Only the node's ID: no address of Kim's travels in Jimmy's code.
    assert_eq!(held.node.addrs.len(), 0, "{:?}", held.node);
    let code = OspCode::Person(held.clone()).to_string();
    assert_eq!(code.parse::<OspCode>().unwrap().ticket(), &held);

    // Jimmy holds nothing of Alice.
    let for_alice = ClientRequest::InviteFor {
        actor: alice.identity(),
    };
    let refused = ticket(&jimmy, for_alice).await.unwrap_err();
    assert!(refused.contains("holds no profile"), "{refused}");

    for node in [kim, jimmy, alice] {
        node.shutdown().await.unwrap();
    }
}
