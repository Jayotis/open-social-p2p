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
    EarlierKeys, EndpointId, ObjectId, PublicKey,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, DeviceInfo, Direction, FeedItem, PeerRecord, StatusInfo},
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
        /// With --restore: nobody else can have the old node's key (say, its disk died), so
        /// what it signed stays valid. Otherwise you're asked.
        #[arg(long, requires = "restore", conflicts_with = "revoked")]
        retired: bool,
        /// With --restore: someone else may have the old node's key (stolen, malware, a disk
        /// you gave away), so your followers refuse what it signed. Otherwise you're asked.
        #[arg(long, requires = "restore")]
        revoked: bool,
        /// Don't ask for words back to check they were written down (test nodes and scripts)
        #[arg(long, conflicts_with = "restore")]
        skip_word_check: bool,
        /// Share copies of posts from people you follow with other nodes, so popular posts
        /// don't swamp their authors (ADR 0012). Otherwise you're asked
        #[arg(long, conflicts_with = "no_share")]
        share: bool,
        /// Don't share them. Otherwise you're asked
        #[arg(long)]
        no_share: bool,
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
    /// Show what this node measured about the nodes it deals with
    Peers,
    /// Forget a paired phone, by its name or the start of its ID
    Unpair { device: String },
    /// Run the node in the background, starting at boot (systemd user service)
    #[command(subcommand)]
    Service(ServiceCommand),
    /// Back up your posts, follows and followers
    #[command(subcommand)]
    Backup(BackupCommand),
    /// Check your recovery words, or take the identity key off an older node
    #[command(subcommand)]
    Keys(KeysCommand),
}

#[derive(Subcommand)]
enum KeysCommand {
    /// Check that recovery words are this node's: asks for them
    Check,
    /// For a node made before identity keys were kept only as words: shows
    /// the words, checks you wrote them down, and deletes the identity key
    /// from this computer. Stop the node first.
    Protect,
    /// If someone may have copied this node's files: makes a new signing key
    /// that revokes the old one. Asks for your words. Stop the node first.
    Replace,
}

#[derive(Subcommand)]
enum BackupCommand {
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
            retired,
            revoked,
            skip_word_check,
            share,
            no_share,
        } => {
            let lookup = !no_address_lookup;
            let config = init_config(name, &relay, bind, lookup, backup.is_some());
            let share = match (share, no_share) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            };
            if restore {
                let old_key = match (retired, revoked) {
                    (true, _) => Some(EarlierKeys::Retired),
                    (_, true) => Some(EarlierKeys::Revoked),
                    _ => None,
                };
                return restore_node(&home, config, backup.as_deref(), old_key, share);
            }
            return init(&home, config, !skip_word_check, share);
        }
        Command::Daemon => return daemon(&home).await,
        // These read the node's files rather than asking the daemon: only
        // someone who can read them may, never a paired phone.
        Command::Keys(KeysCommand::Check) => {
            if osp_node::backup::check_words(&home, &read_words()?)? {
                println!("These are your recovery words: they belong to this node's identity.");
            } else {
                bail!("these words belong to another identity, not this node's");
            }
            return Ok(());
        }
        Command::Keys(KeysCommand::Protect) => return protect(&home),
        Command::Keys(KeysCommand::Replace) => return replace(&home),
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
        Command::Peers => ClientRequest::Peers,
        Command::Post { text, attach } => ClientRequest::Post {
            content: text,
            // The daemon has its own working directory, so send absolute paths.
            attachments: attach
                .iter()
                .map(|p| p.canonicalize().with_context(|| format!("{}", p.display())))
                .collect::<Result<_>>()?,
            uploads: Vec::new(),
            upload: None,
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

/// A new node's config, from the options of `osp init`.
fn init_config(
    name: Option<String>,
    relay: &str,
    bind: Option<SocketAddr>,
    address_lookup: bool,
    from_backup: bool,
) -> Config {
    let mut config = Config::default();
    if let Some(name) = name {
        config.name = name;
    } else if from_backup {
        config.name = String::new(); // Take the name in the backup.
    }
    config.network.relay = match relay {
        "default" => RelayChoice::Default,
        "disabled" => RelayChoice::Disabled,
        url => RelayChoice::Custom(vec![url.to_string()]),
    };
    config.network.bind = bind;
    config.network.address_lookup = address_lookup;
    config
}

fn init(home: &Path, mut config: Config, check_words: bool, share: Option<bool>) -> Result<()> {
    // Asked first, so words aren't shown for a node that isn't made.
    config.swarm.share_prefetched = ask_share(share)?;
    let words = osp_node::backup::new_identity_words();
    print_words(&words);
    if check_words {
        check_written_down(&words)?;
    }
    Node::init_with_words(home, &config, &words.join(" "))?;
    println!();
    println!("Created a node in {}", home.display());
    println!();
    println!("Start it with: osp --home {} daemon", home.display());
    Ok(())
}

fn restore_node(
    home: &Path,
    mut config: Config,
    backup: Option<&Path>,
    old_key: Option<EarlierKeys>,
    share: Option<bool>,
) -> Result<()> {
    let words = read_words()?;
    let old_key = match old_key {
        Some(choice) => choice,
        None => ask_old_key(backup.is_some())?,
    };
    // A backup doesn't keep the settings, so this is asked again.
    config.swarm.share_prefetched = ask_share(share)?;
    let restored = Node::restore(home, &config, &words, backup, old_key)?;
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
    if old_key == EarlierKeys::Revoked {
        println!("- your followers refuse anything the old node's key signs from now on;");
    }
    println!("- your phone pairs again (`osp pair`);");
    println!("- never start the old node again, or your followers get two nodes for you.");
    println!();
    println!("Start it with: osp --home {} daemon", home.display());
    Ok(())
}

/// Asks whether someone else could have the old node's signing key.
fn ask_old_key(from_backup: bool) -> Result<EarlierKeys> {
    println!();
    println!("Could anyone else have a copy of your old node's key? Say yes if the computer");
    println!("was stolen, malware may have run on it, or its disk went somewhere you don't");
    println!("control. Say no if, for example, its disk simply died.");
    println!();
    println!("- yes: the old key is revoked. Your followers refuse anything it signs, including");
    if from_backup {
        println!("  posts someone else made with it. Posts not in your backup file are lost.");
    } else {
        println!("  posts someone else made with it. Without a backup file, so are all your");
        println!("  earlier posts.");
    }
    println!("- no: the old key is retired. Every post it signed stays.");
    loop {
        print!("Could anyone else have it? (yes / no / not sure): ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            bail!("no answer; nothing was created (or pass --retired or --revoked)");
        }
        match line.trim().to_lowercase().as_str() {
            "yes" | "y" | "not sure" | "unsure" => return Ok(EarlierKeys::Revoked),
            "no" | "n" => return Ok(EarlierKeys::Retired),
            _ => println!("Please answer yes, no or not sure."),
        }
    }
}

/// Asks whether to share copies of posts from people this node follows
/// (ADR 0012), unless `choice` already says. There's no default: sharing
/// shows others who you follow, so it needs a yes.
fn ask_share(choice: Option<bool>) -> Result<bool> {
    if let Some(share) = choice {
        return Ok(share);
    }
    println!();
    println!("Your node fetches new posts from people you follow ahead of time, so they open");
    println!("at once on your phone. It can also share those copies with other nodes, so a");
    println!("popular post doesn't swamp its author's home connection.");
    println!();
    println!("If you share:");
    println!("- a node that downloads a copy from yours learns that you follow its author, or");
    println!("  someone who reshared it, and can see your internet address;");
    println!("- the author's node, or the resharer's, may name yours to their other followers");
    println!("  as having a copy;");
    println!("- it uses some of your upload, which `max_upload_mbps` in config.toml can cap.");
    println!("It never shows what you watched: media you open from outside the people you");
    println!("follow is never shared.");
    println!();
    println!("Recommended: yes. You can change it later: `share_prefetched` under [swarm] in");
    println!("config.toml.");
    loop {
        print!("Share copies of posts from people you follow? (yes / no): ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            bail!("no answer; nothing was created (or pass --share or --no-share)");
        }
        match line.trim().to_lowercase().as_str() {
            "yes" | "y" => return Ok(true),
            "no" | "n" => return Ok(false),
            _ => println!("Please answer yes or no."),
        }
    }
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
    println!("These words are your identity. If this computer is lost or stolen,");
    println!("`osp init --restore` brings it back on another one. Anyone who has them can");
    println!("post as you, so keep them on paper, not on this computer, and never type them");
    println!("into a website or send them to anyone.");
    println!();
    println!("This node won't show them again: it doesn't keep them.");
}

/// Asks for three of the words back, so a node is never made from words
/// nobody wrote down. Nothing is created until they're right.
fn check_written_down(words: &[&str]) -> Result<()> {
    let mut picks = Vec::new();
    while picks.len() < 3 {
        let mut byte = [0];
        getrandom::fill(&mut byte).map_err(|e| anyhow::anyhow!("no random numbers: {e}"))?;
        // 240 is the largest multiple of 24 below 256, so each word is equally likely.
        if byte[0] < 240 && !picks.contains(&(byte[0] as usize % words.len())) {
            picks.push(byte[0] as usize % words.len());
        }
    }
    picks.sort();
    println!();
    println!("To check you have them, type these words from your copy.");
    for i in picks {
        loop {
            print!("Word {}: ", i + 1);
            std::io::Write::flush(&mut std::io::stdout())?;
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line)? == 0 {
                bail!("stopped before the words were checked; nothing was created");
            }
            if line.trim().eq_ignore_ascii_case(words[i]) {
                break;
            }
            println!(
                "That isn't word {}. Check your copy against the list above.",
                i + 1
            );
        }
    }
    Ok(())
}

/// Replaces the node's signing key with one that revokes it.
fn replace(home: &Path) -> Result<()> {
    Node::replace_keys(home, || {
        println!("This gives this node a new signing key and revokes the old one: use it if");
        println!("someone may have copied this node's files. Your followers refuse anything the");
        println!("old key signs from then on, and your own posts are signed again with the new");
        println!("key, so none of them are lost.");
        println!();
        read_words()
    })?;
    println!();
    println!("Done. Start the node again (`systemctl --user start osp`, or `osp daemon`):");
    println!("it signs your posts with the new key and tells your followers.");
    Ok(())
}

/// Takes the identity key off a node made before ADR 0010.
fn protect(home: &Path) -> Result<()> {
    Node::protect_keys(home, |words| {
        println!("This node keeps your identity key in a file, so anyone who copies its files");
        println!("can be you for good. From now on it keeps a signing key instead, which your");
        println!("recovery words can replace if it's ever stolen. Your words become the only");
        println!("copy of your identity key, so write them down now.");
        print_words(words);
        check_written_down(words)
    })?;
    println!();
    println!(
        "Done: {} no longer holds your identity key.",
        home.display()
    );
    println!("Start the node again (`systemctl --user start osp`, or `osp daemon`).");
    Ok(())
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
        ClientResponse::Peers { peers } => print_peers(&peers),
        ClientResponse::Paired { name, .. } => println!("Paired with {name}'s node."),
        ClientResponse::Blob { size: bytes } => println!("{}", size(bytes)),
        ClientResponse::Resume { received } => println!("{} received", size(received)),
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

fn print_peers(peers: &[PeerRecord]) {
    if peers.is_empty() {
        println!("No peers yet: this node hasn't dealt with another node.");
        return;
    }
    println!("What this node saw itself. Counts fade by half every 14 days, so they're");
    println!("recent weights, rounded. Nothing here leaves this node.");
    for p in peers {
        let c = &p.counts;
        let mut what = Vec::new();
        if p.followed {
            what.push("you follow them");
        }
        if p.follower {
            what.push("follows you");
        }
        if p.host {
            what.push("hosts media");
        }
        println!();
        println!(
            "{}  {}  {}",
            p.name.as_deref().unwrap_or("(unknown)"),
            p.node.fmt_short(),
            what.join(", ")
        );
        println!(
            "  last contact {} ago, first {} ago",
            uptime(p.last_contact_secs_ago),
            uptime(p.first_contact_secs_ago)
        );
        let mut asked = Vec::new();
        if c.tries > 0.0 {
            asked.push(format!(
                "reached {} of {:.0} tries",
                percent(c.tries - c.unreached, c.tries),
                c.tries
            ));
        }
        if c.requests > 0.0 {
            let mut answers = format!(
                "answered {} of {:.0} requests",
                percent(c.answered, c.requests),
                c.requests
            );
            if c.answered > 0.0 {
                answers += &format!(", in {:.0} ms on average", c.answer_ms / c.answered);
            }
            asked.push(answers);
        }
        if !asked.is_empty() {
            println!("  {}", asked.join("; "));
        }
        let attempts = c.completed + c.busy + c.not_hosting + c.bad_data + c.failed + c.handed_on;
        if attempts > 0.0 || c.received_bytes > 0.0 {
            let mut line = format!(
                "  downloads from it: {:.0} completed, {:.0} busy, {:.0} not hosting, {:.0} bad data, {:.0} failed",
                c.completed, c.busy, c.not_hosting, c.bad_data, c.failed
            );
            if c.handed_on > 0.0 {
                line += &format!(", {:.0} handed on before it sent anything", c.handed_on);
            }
            if c.received_bytes > 0.0 {
                line += &format!("; {}", size(c.received_bytes as u64));
                if c.receive_ms > 0.0 {
                    let per_sec = c.received_bytes / (c.receive_ms / 1000.0);
                    line += &format!(" at {}/s", size(per_sec as u64));
                }
            }
            if c.first_bytes > 0.0 {
                line += &format!(
                    ", first byte after {:.0} ms",
                    c.first_byte_ms / c.first_bytes
                );
            }
            println!("{line}");
        }
        if c.transfers > 0.0 || c.refused_busy > 0.0 {
            println!(
                "  served to it: {:.0} transfers, {}; {:.0} refused as busy",
                c.transfers,
                size(c.sent_bytes as u64),
                c.refused_busy
            );
        }
    }
}

fn percent(part: f64, whole: f64) -> String {
    format!("{:.0}%", 100.0 * part / whole)
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
