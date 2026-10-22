//! `osp`: run and control an Open Social P2P home node.
//!
//! `osp daemon` runs the node; most other commands talk to the running
//! daemon over the Unix socket in the node's home directory.

mod service;

use std::{
    io::IsTerminal,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use osp_node::{Config, Home, Node, RelayChoice};
use osp_proto::{
    EndpointId, ObjectId, PublicKey,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, DeviceInfo, Direction, FeedItem, StatusInfo},
};
use tokio::net::UnixStream;

#[derive(Parser)]
#[command(
    name = "osp",
    version,
    about = "Run and control an Open Social P2P home node"
)]
struct Cli {
    /// Node home directory [default: ~/.local/share/osp]
    #[arg(long, env = "OSP_HOME", global = true)]
    home: Option<PathBuf>,

    /// Print the daemon's response as JSON
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new node: identity, keys and config
    Init {
        /// Display name for your profile [default: Anonymous, or the name in the backup]
        #[arg(long)]
        name: Option<String>,
        /// Relays: "default" (n0's public relays), "disabled", or your own relay URL
        #[arg(long, default_value = "default")]
        relay: String,
        /// Bind to this address only, e.g. 127.0.0.1:0
        #[arg(long)]
        bind: Option<SocketAddr>,
        /// Don't publish or look up node addresses via n0's DNS service
        #[arg(long)]
        no_address_lookup: bool,
        /// Bring back an existing identity: asks for its 24 recovery words
        #[arg(long)]
        restore: bool,
        /// With --restore: also restore posts, follows, followers and media from this backup file
        #[arg(long, requires = "restore")]
        backup: Option<PathBuf>,
    },
    /// Run the node in the foreground (Ctrl-C to stop)
    Daemon,
    /// Show identity, network, connections and storage
    Status,
    /// Print a ticket others can use to follow you
    Invite {
        /// Don't draw a QR code
        #[arg(long)]
        no_qr: bool,
    },
    /// Follow someone by their ticket
    Follow { ticket: String },
    /// Stop following someone
    Unfollow { actor: PublicKey },
    /// List who you follow
    Following,
    /// Publish a post, optionally with photos or videos
    Post {
        text: String,
        /// Image or video to attach (repeatable). Transcoded to 720p before publishing.
        #[arg(long, short)]
        attach: Vec<PathBuf>,
    },
    /// Show your feed, newest first
    Feed {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Show one post or reshare
    Show { id: ObjectId },
    /// Reshare a post
    Reshare {
        id: ObjectId,
        /// Also keep a copy of its media and serve it to others
        #[arg(long)]
        host: bool,
    },
    /// Download a post's media, optionally saving it to a directory
    Fetch {
        id: ObjectId,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Sync everyone you follow now
    Sync,
    /// Pair your phone: shows a QR code to scan with the app
    Pair {
        /// Don't draw a QR code
        #[arg(long)]
        no_qr: bool,
    },
    /// List paired phones
    Devices,
    /// Forget a paired phone, by its name or the start of its ID
    Unpair { device: String },
    /// Run the node in the background, starting at boot (systemd user service)
    #[command(subcommand)]
    Service(ServiceCommand),
    /// Back up your identity
    #[command(subcommand)]
    Backup(BackupCommand),
}

#[derive(Subcommand)]
enum BackupCommand {
    /// Show your 24 recovery words again
    Words,
    /// Write an encrypted backup of your posts, follows, followers and media
    Export {
        /// Where to write it. Keep it somewhere other than this computer.
        file: PathBuf,
        /// Leave out the photos and videos of your posts, for a small file
        #[arg(long)]
        no_media: bool,
    },
}

#[derive(Subcommand)]
enum ServiceCommand {
    /// Install, enable and start the service
    Install,
    /// Stop, disable and remove the service (keeps the node's data)
    Uninstall,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = cli.home.clone().unwrap_or_else(default_home);
    let request = match cli.command {
        Command::Init {
            name,
            relay,
            bind,
            no_address_lookup,
            restore,
            backup,
        } => {
            let lookup = !no_address_lookup;
            return init(
                &home,
                name,
                &relay,
                bind,
                lookup,
                restore,
                backup.as_deref(),
            );
        }
        Command::Daemon => return daemon(&home).await,
        // Read from the key file, not asked of the daemon: only someone who
        // can read the node's files sees them, never a paired phone.
        Command::Backup(BackupCommand::Words) => {
            print_words(&osp_node::backup::recovery_words(&home)?);
            return Ok(());
        }
        // The daemon writes the file, so send an absolute path.
        Command::Backup(BackupCommand::Export { file, no_media }) => ClientRequest::ExportBackup {
            path: std::path::absolute(&file)?,
            media: !no_media,
        },
        Command::Invite { no_qr } => {
            let resp = send(&home, ClientRequest::Invite).await?;
            if let (false, ClientResponse::Ticket { ticket }) = (cli.json || no_qr, &resp) {
                qr2term::print_qr(ticket).context("drawing QR code")?;
            }
            return print(resp, cli.json);
        }
        Command::Pair { no_qr } => return pair(&home, no_qr, cli.json).await,
        Command::Unpair { device } => {
            let id = find_device(&home, &device).await?;
            ClientRequest::Unpair { device: id }
        }
        Command::Service(ServiceCommand::Install) => return service::install(&home).await,
        Command::Service(ServiceCommand::Uninstall) => return service::uninstall(),
        Command::Status => ClientRequest::Status,
        Command::Follow { ticket } => ClientRequest::Follow { ticket },
        Command::Unfollow { actor } => ClientRequest::Unfollow { actor },
        Command::Following => ClientRequest::Following,
        Command::Devices => ClientRequest::Devices,
        Command::Post { text, attach } => ClientRequest::Post {
            content: text,
            // The daemon has its own working directory, so send absolute paths.
            attachments: attach
                .iter()
                .map(|p| p.canonicalize().with_context(|| format!("{}", p.display())))
                .collect::<Result<_>>()?,
            uploads: Vec::new(),
        },
        Command::Feed { limit } => ClientRequest::Feed { limit },
        Command::Show { id } => ClientRequest::Show { id },
        Command::Reshare { id, host } => ClientRequest::Reshare { id, host },
        Command::Fetch { id, out } => ClientRequest::Fetch {
            id,
            out_dir: out.map(|p| std::path::absolute(&p)).transpose()?,
        },
        Command::Sync => ClientRequest::Sync,
    };
    let resp = send(&home, request).await?;
    print(resp, cli.json)
}

fn default_home() -> PathBuf {
    directories::ProjectDirs::from("", "", "osp")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("osp-home"))
}

fn init(
    home: &Path,
    name: Option<String>,
    relay: &str,
    bind: Option<SocketAddr>,
    address_lookup: bool,
    restore: bool,
    backup: Option<&Path>,
) -> Result<()> {
    let mut config = Config::default();
    if let Some(name) = name {
        config.name = name;
    } else if backup.is_some() {
        config.name = String::new(); // Take the name in the backup.
    }
    config.network.relay = match relay {
        "default" => RelayChoice::Default,
        "disabled" => RelayChoice::Disabled,
        url => RelayChoice::Custom(vec![url.to_string()]),
    };
    config.network.bind = bind;
    config.network.address_lookup = address_lookup;
    if restore {
        let restored = Node::restore(home, &config, &read_words()?, backup)?;
        println!("Restored identity {}", restored.identity);
        println!("on a new node in {}.", home.display());
        if let Some(info) = &restored.from_file {
            println!(
                "From the backup: {} posts, {} follows, {} followers and {} media files.",
                info.posts, info.follows, info.followers, info.media_files
            );
        }
        println!();
        println!("This is a new node, so:");
        if restored.from_file.is_some() {
            println!("- when it starts, it tells your followers that you moved here;");
        } else {
            println!("- your followers find it once you give them a new code (`osp invite`);");
        }
        println!("- your phone pairs again (`osp pair`);");
        println!("- never start the old node again, or your followers get two nodes for you.");
    } else {
        Node::init(home, &config)?;
        println!("Created a node in {}", home.display());
        print_words(&osp_node::backup::recovery_words(home)?);
    }
    println!();
    println!("Start it with: osp --home {} daemon", home.display());
    Ok(())
}

/// Prints recovery words numbered in rows of four, so reading them left to
/// right, line by line, gives the right order.
fn print_words(words: &[&str]) {
    println!();
    println!("Your recovery words. Write them down, in order, and keep them somewhere safe:");
    println!();
    for (row, chunk) in words.chunks(4).enumerate() {
        let line: Vec<String> = chunk
            .iter()
            .enumerate()
            .map(|(i, word)| format!("{:>2}. {word:<8}", row * 4 + i + 1))
            .collect();
        println!("  {}", line.join("   ").trim_end());
    }
    println!();
    println!("These words are your identity. If this computer is lost, `osp init --restore`");
    println!("brings it back on another one. Anyone who has them can post as you, so never");
    println!("type them into a website or send them to anyone.");
    println!("`osp backup words` shows them again.");
}

/// Reads recovery words from standard input, over as many lines as it takes.
/// Not from the command line, so they don't end up in the shell's history.
fn read_words() -> Result<String> {
    let count = osp_node::backup::WORD_COUNT;
    if std::io::stdin().is_terminal() {
        println!("Type your {count} recovery words, in order, separated by spaces or new lines:");
    }
    let mut words = Vec::new();
    for line in std::io::stdin().lines() {
        words.extend(line?.split_whitespace().map(str::to_string));
        if words.len() >= count {
            break;
        }
    }
    Ok(words.join(" "))
}

async fn daemon(home: &Path) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // iroh warns on every network check (about every 25 s) while
                // the NAT maps addresses per destination; the node logs it once.
                .unwrap_or_else(|_| {
                    "osp_node=info,iroh=warn,iroh::net_report::report=error,iroh_blobs=warn".into()
                }),
        )
        // No colour codes in the journal when running as a service.
        .with_ansi(std::io::stdout().is_terminal())
        .init();
    let mut node = Node::spawn(home).await?;
    node.serve_local_socket().await?;
    println!("Identity: {}", node.identity());
    println!("Node:     {}", node.node_id());
    println!("Home:     {}", home.display());
    println!("Running. Press Ctrl-C to stop.");

    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
    println!("Stopping...");
    node.shutdown().await
}

/// Shows a pairing QR code, then waits until a new device appears.
async fn pair(home: &Path, no_qr: bool, json: bool) -> Result<()> {
    let before: Vec<EndpointId> = devices(home).await?.iter().map(|d| d.id).collect();
    let resp = send(home, ClientRequest::NewPairing).await?;
    let ClientResponse::Pairing {
        ticket,
        expires_in_secs,
    } = &resp
    else {
        return print(resp, json);
    };
    if json {
        return print(resp, json);
    }
    if !no_qr {
        qr2term::print_qr(ticket).context("drawing QR code")?;
    }
    println!("{ticket}\n");
    println!(
        "Scan this with the Open Social P2P app on your phone. It works once, within {} minutes.",
        expires_in_secs / 60
    );
    println!("Whoever uses it first becomes a paired device, so don't share it.");
    println!("Waiting for your phone... (Ctrl-C stops waiting; the code stays valid)");

    let deadline = Instant::now() + Duration::from_secs(*expires_in_secs);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Some(d) = devices(home)
            .await?
            .into_iter()
            .find(|d| !before.contains(&d.id))
        {
            println!(
                "Paired with {} ({}).",
                d.name.as_deref().unwrap_or("a device"),
                d.id.fmt_short()
            );
            return Ok(());
        }
    }
    bail!("the pairing code expired; run `osp pair` again")
}

async fn devices(home: &Path) -> Result<Vec<DeviceInfo>> {
    match send(home, ClientRequest::Devices).await? {
        ClientResponse::Devices { devices } => Ok(devices),
        ClientResponse::Error { message } => bail!(message),
        other => bail!("unexpected reply {other:?}"),
    }
}

/// A paired device by name (ignoring case) or by the start of its ID.
async fn find_device(home: &Path, query: &str) -> Result<EndpointId> {
    let query = query.trim().to_lowercase();
    let found: Vec<EndpointId> = devices(home)
        .await?
        .into_iter()
        .filter(|d| {
            d.id.to_string().starts_with(&query)
                || d.name.as_deref().is_some_and(|n| n.to_lowercase() == query)
        })
        .map(|d| d.id)
        .collect();
    match found[..] {
        [id] => Ok(id),
        [] => bail!("no paired device matches {query:?} (see `osp devices`)"),
        _ => bail!("{query:?} matches several devices; give more of the ID"),
    }
}

async fn send(home: &Path, req: ClientRequest) -> Result<ClientResponse> {
    let socket = Home::new(home).socket();
    let stream = UnixStream::connect(&socket).await.with_context(|| {
        format!(
            "no node running in {} (start one with `osp --home {} daemon`)",
            home.display(),
            home.display()
        )
    })?;
    let (mut reader, mut writer) = stream.into_split();
    write_frame(&mut writer, &req).await?;
    read_frame(&mut reader).await
}

fn print(resp: ClientResponse, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        return match resp {
            ClientResponse::Error { .. } => std::process::exit(1),
            _ => Ok(()),
        };
    }
    match resp {
        ClientResponse::Error { message } => bail!(message),
        ClientResponse::Ok => println!("Done."),
        ClientResponse::Status(status) => print_status(&status),
        ClientResponse::Ticket { ticket } => println!("{ticket}"),
        ClientResponse::Posted { id } => println!("Published {id}"),
        ClientResponse::Following { follows } => {
            if follows.is_empty() {
                println!("You don't follow anyone yet.");
            }
            for f in follows {
                println!(
                    "{}  {}",
                    f.name.as_deref().unwrap_or("(name unknown)"),
                    f.actor
                );
                match (f.last_sync, f.last_error) {
                    (_, Some(err)) => println!("    last sync failed: {err}"),
                    (Some(at), None) => println!("    synced {at}, up to #{}", f.cursor),
                    (None, None) => println!("    not synced yet"),
                }
            }
        }
        ClientResponse::Feed { items } => {
            if items.is_empty() {
                println!("Nothing here yet. Follow someone with `osp follow <ticket>`.");
            }
            for item in items {
                print_item(&item);
                println!();
            }
        }
        ClientResponse::Item { item } => print_item(&item),
        ClientResponse::Fetched { files } => {
            for f in files {
                match f.path {
                    Some(path) => println!("{} ({}) -> {}", f.blob, size(f.size), path.display()),
                    None => println!("{} ({}) stored", f.blob, size(f.size)),
                }
            }
        }
        ClientResponse::Synced { new_objects: 0 } => println!("Up to date."),
        ClientResponse::Synced { new_objects } => println!("{new_objects} new objects"),
        ClientResponse::Pairing { ticket, .. } => println!("{ticket}"),
        ClientResponse::Devices { devices } => {
            if devices.is_empty() {
                println!("No paired devices. Pair your phone with `osp pair`.");
            }
            for d in devices {
                let connected = if d.connected { ", connected now" } else { "" };
                println!("{}  {}", d.name.as_deref().unwrap_or("(no name)"), d.id);
                println!("    paired {}{connected}", d.paired);
            }
        }
        ClientResponse::Paired { name, .. } => println!("Paired with {name}'s node."),
        ClientResponse::Blob { size: bytes } => println!("{}", size(bytes)),
        ClientResponse::BackedUp { path, info } => {
            println!("Wrote {} ({}):", path.display(), size(info.bytes));
            println!(
                "{} posts, {} follows, {} followers and {} media files.",
                info.posts, info.follows, info.followers, info.media_files
            );
            if info.media_missing > 0 {
                println!(
                    "{} media files of your posts are missing from this node, so the backup lacks them.",
                    info.media_missing
                );
            }
            println!("It is encrypted: only your recovery words open it. Keep it somewhere other");
            println!("than this computer, and make a new one now and then.");
        }
    }
    Ok(())
}

fn print_status(s: &StatusInfo) {
    println!("Name:      {}", s.name);
    println!("Identity:  {}", s.identity);
    println!("Node:      {}", s.node);
    println!("Version:   {}, up {}", s.version, uptime(s.uptime_secs));
    println!("Relay:     {}", s.relay.as_deref().unwrap_or("none"));
    println!("Bound:     {}", s.local_addrs.join(", "));
    if let Some(nat) = s.nat {
        println!("NAT:       {}", nat.describe());
    }
    println!(
        "Follows:   {}   Followers: {}   Devices: {}",
        s.follows, s.followers, s.devices
    );
    for st in &s.storage {
        println!(
            "Storage:   {:<8} {} blobs, {}",
            st.class,
            st.blobs,
            size(st.bytes)
        );
    }
    if let Some(free) = s.disk_free_bytes {
        println!("Disk:      {} free", size(free));
    }
    for p in &s.peers {
        let rtt = p.rtt_ms.map(|r| format!("{r} ms")).unwrap_or_default();
        let direction = match p.direction {
            Direction::Outgoing => "outgoing",
            Direction::Incoming => "incoming",
            Direction::Both => "both",
        };
        let device = if p.device { "  (your device)" } else { "" };
        println!(
            "Peer:      {}  {direction:<8}  {} {}{device}",
            p.node.fmt_short(),
            p.path,
            rtt
        );
    }
    if s.problems.is_empty() {
        println!("Health:    no problems");
    }
    for problem in &s.problems {
        println!("Problem:   {problem}");
    }
}

fn uptime(secs: u64) -> String {
    match secs {
        s if s >= 86_400 => format!("{}d {}h", s / 86_400, s % 86_400 / 3600),
        s if s >= 3600 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        s if s >= 60 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn print_item(item: &FeedItem) {
    let author = item
        .actor_name
        .clone()
        .unwrap_or_else(|| item.actor.fmt_short().to_string());
    println!("{author} · {}", item.published);
    if let Some(by) = &item.reshared_by {
        let name = by
            .name
            .clone()
            .unwrap_or_else(|| by.actor.fmt_short().to_string());
        let hosting = if by.hosting { " (hosting)" } else { "" };
        println!("  reshared by {name}{hosting}");
    }
    if !item.content.is_empty() {
        println!("  {}", item.content);
    }
    for a in &item.attachments {
        let m = &a.attachment;
        let duration = m
            .duration_ms
            .map(|ms| format!(" {:.1}s", ms as f64 / 1000.0))
            .unwrap_or_default();
        let local = if a.local { "stored" } else { "not downloaded" };
        println!(
            "  [{:?} {}x{}{} {}, {local}]",
            m.kind,
            m.width,
            m.height,
            duration,
            size(m.size)
        );
    }
    println!("  id: {}", item.id);
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}
