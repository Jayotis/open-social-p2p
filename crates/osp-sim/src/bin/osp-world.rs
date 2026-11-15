//! `osp-world`: a small made-up world on real nodes, for looking at the
//! Universe on a phone (ADR 0034's gate: "a world of 20 people and 5
//! machines opens at Local and looks right on the test phones").
//!
//! Everyone runs on this computer, on loopback, with no relay and no
//! address lookup, so the people's nodes talk only to each other. The
//! owner's node is like an ordinary one, with n0's relays and lookup, so a
//! phone can reach it past a firewall. Its pairing code is printed; the
//! world runs until stopped.
//!
//! The people: three groups of friends who follow and reshare each other,
//! one reshare between two of the groups, and three the owner follows who
//! have no lines. The owner follows a few in each group.
//!
//! No directory and no machines: on real nodes both need machine names,
//! which need Tor. The lab's nodes show those.

use std::{path::PathBuf, time::Duration};

use anyhow::{Result, bail};
use clap::Parser;
use osp_node::{Config, Node, RelayChoice};
use osp_proto::{
    ObjectId,
    rpc::{ClientRequest, ClientResponse},
};

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// An empty directory for the nodes' homes.
    #[arg(long)]
    dir: PathBuf,
}

const NAMES: [&str; 20] = [
    "Ana", "Ben", "Cleo", "Dev", "Eli", "Fay", "Gus", "Hana", "Ivo", "Jo", "Kai", "Lena", "Milo",
    "Nia", "Omar", "Pia", "Quinn", "Rui", "Sol", "Tao",
];

/// Who reshares whom, by place in `NAMES`. Each resharer follows the other
/// first. Three groups: 0–6, 7–13, 14–16; 2 reshares 7 across.
const RESHARES: [(usize, usize); 17] = [
    (0, 3),
    (0, 4),
    (1, 4),
    (1, 5),
    (2, 6),
    (2, 0),
    (3, 5),
    (2, 7),
    (7, 9),
    (7, 10),
    (8, 11),
    (8, 12),
    (8, 13),
    (9, 13),
    (14, 15),
    (14, 16),
    (15, 16),
];

/// Whom the owner follows.
const FOLLOWED: [usize; 6] = [0, 1, 2, 7, 8, 14];

/// Followed, with no lines.
const ALONE: [usize; 3] = [17, 18, 19];

async fn start(dir: &std::path::Path, config: Config) -> Result<Node> {
    let home = dir.join(config.name.to_lowercase().replace(' ', "-"));
    Node::init(&home, &config)?;
    Node::spawn(&home).await
}

async fn call(node: &Node, req: ClientRequest) -> Result<ClientResponse> {
    match node.handle(req).await {
        ClientResponse::Error { message } => bail!("{message}"),
        resp => Ok(resp),
    }
}

async fn follow(follower: &Node, followed: &Node) -> Result<()> {
    let ClientResponse::Ticket { ticket } = call(followed, ClientRequest::Invite).await? else {
        bail!("no ticket")
    };
    call(follower, ClientRequest::Follow { ticket }).await?;
    Ok(())
}

async fn post(node: &Node, text: &str) -> Result<ObjectId> {
    let req = ClientRequest::Post {
        content: text.into(),
        attachments: vec![],
        uploads: vec![],
        upload: None,
    };
    let ClientResponse::Posted { id } = call(node, req).await? else {
        bail!("not posted")
    };
    Ok(id)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.dir)?;
    if std::fs::read_dir(&cli.dir)?.next().is_some() {
        bail!("{} isn't empty", cli.dir.display());
    }

    let mut people = Vec::new();
    for name in NAMES {
        people.push(start(&cli.dir, Config::local_test(name)).await?);
    }
    let mut config = Config::local_test("Rae");
    config.summary = Some("The owner of a made-up world".into());
    config.network.relay = RelayChoice::Default;
    config.network.address_lookup = true;
    config.network.bind = None;
    let owner = start(&cli.dir, config).await?;
    println!("{} people started", people.len());

    let mut posts = Vec::new();
    for (node, name) in people.iter().zip(NAMES) {
        posts.push(post(node, &format!("{name}'s first post")).await?);
    }
    for (from, to) in RESHARES {
        follow(&people[from], &people[to]).await?;
    }
    for (from, to) in RESHARES {
        call(&people[from], ClientRequest::Sync).await?;
        let id = posts[to];
        call(&people[from], ClientRequest::Reshare { id, host: false }).await?;
    }
    println!("{} reshares", RESHARES.len());

    for i in FOLLOWED.into_iter().chain(ALONE) {
        follow(&owner, &people[i]).await?;
    }
    call(&owner, ClientRequest::Sync).await?;
    // Syncing fetches what the followed reshared in the background.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let ClientResponse::Universe { snapshot } = call(
        &owner,
        ClientRequest::Universe {
            level: Default::default(),
            region: None,
            layers: vec![],
            since: None,
        },
    )
    .await?
    else {
        bail!("no snapshot")
    };
    println!(
        "Rae's Universe: {} people, {} groups, {} lines, {} sky cells, {} machines",
        snapshot.people.len(),
        snapshot.groups.len(),
        snapshot.lines.len(),
        snapshot.sky.len(),
        snapshot.machines.len()
    );

    let ClientResponse::Pairing { ticket, .. } = call(&owner, ClientRequest::NewPairing).await?
    else {
        bail!("no pairing code")
    };
    println!("pairing code: {ticket}");
    println!("running until stopped");
    std::future::pending::<()>().await;
    Ok(())
}
