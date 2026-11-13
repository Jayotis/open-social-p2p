//! `osp`: run and control an Open Social P2P home node.
//!
//! `osp daemon` runs the node; most other commands talk to the running
//! daemon over the Unix socket in the node's home directory.

/// Stands in for the standard `println!` in this crate (it's defined first),
/// so that output nobody reads any more, as in `osp feed | head`, ends the
/// program as a closed pipe ends others, instead of with a panic.
macro_rules! println {
    () => {
        $crate::write_out(format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::write_out(format_args!("{}\n", format_args!($($arg)*)))
    };
}

/// The same for `print!`.
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::write_out(format_args!($($arg)*))
    };
}

mod service;

use std::{
    fmt::Write as _,
    io::IsTerminal,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use osp_node::{Config, Home, Node, RelayChoice, plain};
use osp_proto::{
    EarlierKeys, EndpointId, ObjectId, PublicKey,
    framing::{read_frame, write_frame},
    machine::MAX_LISTED_POSTS,
    rpc::{
        ClientRequest, ClientResponse, DeviceInfo, Direction, FeedItem, FollowerInfo, PeerRecord,
        Reached, ReplierChoice, ReplyAnswer, ReplyInfo, ReplyStatus, StatusInfo, TrustInfo,
    },
    social::ReplyPolicy,
};
use tokio::net::UnixStream;

#[derive(Parser)]
#[command(
    name = "osp",
    version = osp_node::VERSION,
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
        /// Leave Tor off, for tests on one machine or a home network. The node then
        /// reaches only nodes with it off too, and anyone who connects learns its address
        #[arg(long)]
        no_tor: bool,
        /// Ask the directories this node knows to list your profile and latest posts, so
        /// people can find you (ADR 0015). Otherwise you're asked
        #[arg(long, conflicts_with_all = ["no_list_me", "no_tor"])]
        list_me: bool,
        /// Don't ask to be listed. Otherwise you're asked, unless Tor is off
        #[arg(long)]
        no_list_me: bool,
        /// A directory this node reads, by its machine name and public ID, written under
        /// [[directory.known]] in config.toml. Repeat it for more than one. There's no default:
        /// an installer passes the ones to use
        #[arg(long, num_args = 2, value_names = ["MACHINE", "NODE"], conflicts_with = "no_tor")]
        directory: Vec<EndpointId>,
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
    /// Follow someone by their ticket, or by the identity `osp search` showed
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
    /// Look through the posts and profiles your node has received. Nothing leaves your node
    Search {
        /// Every word has to start a word in the post or profile
        #[arg(required = true)]
        words: Vec<String>,
        /// How many posts, and how many people, at most
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Reshare a post
    Reshare {
        id: ObjectId,
        /// Also keep a copy of its media and serve it to others
        #[arg(long)]
        host: bool,
    },
    /// Delete one of your posts or reshares, everywhere the software reaches.
    /// It can't be undone, and copies people saved stay
    Delete { id: ObjectId },
    /// Show the replies to one of your posts, and who may reply to it
    Replies { post: ObjectId },
    /// List the replies waiting for you to look at them, newest first
    ReplyRequests,
    /// Publish a reply waiting among your requests
    ReplyPublish {
        reply: ObjectId,
        /// Also let its replier in from now on, without asking you
        #[arg(long)]
        allow: bool,
    },
    /// Decline a reply waiting among your requests. Its replier isn't told
    ReplyDecline {
        reply: ObjectId,
        /// Also block its replier: their replies on your posts are hidden, and new
        /// ones refused
        #[arg(long)]
        block: bool,
    },
    /// Hide a published reply from its thread, or with --show, show it again. It
    /// isn't deleted: the words are its replier's
    ReplyHide {
        reply: ObjectId,
        #[arg(long)]
        show: bool,
    },
    /// Set who may reply to one of your posts: off (nobody), circle (people you
    /// follow, your trusted followers and those you allowed) or network (your
    /// circle at once, anyone else to your requests)
    ReplyPolicy { post: ObjectId, policy: ReplyPolicy },
    /// Let someone reply to your posts without asking you, or block them, by
    /// their identity
    #[command(subcommand)]
    Repliers(RepliersCommand),
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
    /// Trusted followers: answer requests, trust people you follow, or ask
    #[command(subcommand)]
    Trust(TrustCommand),
    /// Check messages between nodes over Tor, being built (ADR 0014)
    #[command(subcommand)]
    Tor(TorCommand),
    /// Who may connect to this node directly, under the rule `approved` under
    /// [endpoints] in config.toml, being built (ADR 0014)
    #[command(subcommand)]
    Direct(DirectCommand),
    /// Your followers' nodes: list them, or remove one
    #[command(subcommand)]
    Followers(FollowersCommand),
    /// This node's machine name, tied to no person, under which it gets to know
    /// other nodes, being built (ADR 0015)
    #[command(subcommand)]
    Machine(MachineCommand),
    /// The directories this node reads, to find people it doesn't follow yet,
    /// being built (ADR 0015)
    #[command(subcommand)]
    Directory(DirectoryCommand),
}

#[derive(Subcommand)]
enum RepliersCommand {
    /// Publish their replies on your posts without asking you
    Allow { actor: PublicKey },
    /// Hide their replies on your posts and refuse new ones. They aren't told
    Block { actor: PublicKey },
    /// Forget that you allowed or blocked them
    Forget { actor: PublicKey },
}

#[derive(Subcommand)]
enum DirectoryCommand {
    /// Read the directories under [directory] in config.toml now, rather than
    /// waiting for the next read in the background
    Read,
}

/// A follower is given by name, where your node knows whose node it is, or by
/// the start of its node's ID (`osp followers list`).
#[derive(Subcommand)]
enum FollowersCommand {
    /// List your followers' nodes, and the nodes you removed
    List,
    /// Stop a node being your follower, and refuse it until you allow it again.
    /// Your posts stay public: it can still read them, as anyone can
    Remove { who: String },
    /// Let a node you removed follow you again
    Allow { who: String },
}

/// Nodes are given by their full public node ID.
#[derive(Subcommand)]
enum DirectCommand {
    /// List the nodes you allowed a direct connection
    List,
    /// Allow a node a direct connection, if its own rule allows it too. Once
    /// connected, it knows your IP address, and that can't be taken back.
    Allow { node: EndpointId },
    /// Stop allowing a node a direct connection, and close any it has
    Remove { node: EndpointId },
}

#[derive(Subcommand)]
enum MachineCommand {
    /// Show this node's machine name. Anyone with it and your node ID can tell
    /// the two are one node, so keep them apart
    Show,
    /// Greet another machine by its ID, over Tor. Needs `enabled = true` under
    /// [tor] in config.toml, on both nodes
    Hello { machine: EndpointId },
    /// Forget this node's machine name. It makes a new one when it next starts,
    /// and other machines know it as a stranger. The node must be stopped
    New,
}

#[derive(Subcommand)]
enum TorCommand {
    /// Reach a node over Tor by its node ID, and ask for its profile. Needs
    /// `enabled = true` under [tor] in config.toml, on both nodes.
    Ping { node: EndpointId },
}

/// People are given by their name (ignoring case), or by the start of their
/// identity or node ID.
#[derive(Subcommand)]
enum TrustCommand {
    /// List the requests to be a trusted follower waiting for your answer
    Requests,
    /// Trust the follower whose request this is. Your node then names it to
    /// your other followers as a host more often, so more of them can see
    /// that this person follows you.
    Allow { who: String },
    /// Decline a request. The follower isn't told; its requests are ignored
    /// for 30 days.
    Decline { who: String },
    /// Trust someone you follow, without a request: their nodes that follow you
    Add { who: String },
    /// Stop trusting someone, on every node they were trusted on
    Remove { who: String },
    /// List who you trust
    List,
    /// Ask someone you follow to trust you. You won't be told their answer.
    Ask {
        who: String,
        /// Who you are, so they can tell, in up to 200 characters
        #[arg(long, default_value = "")]
        note: String,
    },
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
    /// Write an encrypted backup of your posts, follows, followers and media, and whom
    /// you trust and let connect directly
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
            no_tor,
            list_me,
            no_list_me,
            directory,
        } => {
            let lookup = !no_address_lookup;
            let ask_name = name.is_none();
            let mut config = init_config(name, &relay, bind, lookup, backup.is_some(), !no_tor);
            config.directory.known = known_directories(&directory);
            let answers = Answers {
                share: answered(share, no_share),
                list_me: answered(list_me, no_list_me),
                ask_name,
            };
            if restore {
                let old_key = match (retired, revoked) {
                    (true, _) => Some(EarlierKeys::Retired),
                    (_, true) => Some(EarlierKeys::Revoked),
                    _ => None,
                };
                return restore_node(&home, config, backup.as_deref(), old_key, answers).await;
            }
            return init(&home, config, !skip_word_check, answers).await;
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
        Command::Machine(MachineCommand::New) => {
            Node::forget_machine_name(&home)?;
            println!("This node's machine name is forgotten. It makes a new one when it next");
            println!("starts, and other machines know it as a stranger.");
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
        Command::Trust(TrustCommand::Requests) => {
            let (requests, _) = trust_list(&home).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&requests)?);
            } else {
                print_trust_requests(&requests);
            }
            return Ok(());
        }
        Command::Trust(TrustCommand::List) => {
            let (_, trusted) = trust_list(&home).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&trusted)?);
            } else {
                print_trusted(&trusted);
            }
            return Ok(());
        }
        Command::Trust(TrustCommand::Allow { who }) => ClientRequest::AnswerTrust {
            node: find_request(&home, &who).await?,
            allow: true,
        },
        Command::Trust(TrustCommand::Decline { who }) => ClientRequest::AnswerTrust {
            node: find_request(&home, &who).await?,
            allow: false,
        },
        Command::Trust(TrustCommand::Add { who }) => ClientRequest::TrustFollowed {
            actor: find_followed(&home, &who).await?,
        },
        Command::Trust(TrustCommand::Remove { who }) => {
            let (_, trusted) = trust_list(&home).await?;
            let actors: Vec<PublicKey> = trusted
                .iter()
                .filter(|t| matches_person(&who, &t.name, &t.actor, Some(&t.node)))
                .map(|t| t.actor)
                .collect();
            ClientRequest::Untrust {
                actor: only_one(&who, actors, "trusted person", "osp trust list")?,
            }
        }
        Command::Delete { id } => {
            let resp = send(&home, ClientRequest::Delete { id }).await?;
            if let (false, ClientResponse::Ok) = (cli.json, &resp) {
                println!("Deleted. Nodes running this software drop it as they hear, within");
                println!("minutes for those online; copies people saved stay.");
                return Ok(());
            }
            return print(resp, cli.json);
        }
        Command::Trust(TrustCommand::Ask { who, note }) => {
            let ask = ClientRequest::AskTrust {
                actor: find_followed(&home, &who).await?,
                note,
            };
            let resp = send(&home, ask).await?;
            if let (false, ClientResponse::Ok) = (cli.json, &resp) {
                println!("Asked. Their node doesn't say whether they trust you.");
                return Ok(());
            }
            return print(resp, cli.json);
        }
        Command::Service(ServiceCommand::Install) => return service::install(&home).await,
        Command::Service(ServiceCommand::Uninstall) => return service::uninstall(),
        Command::Status => ClientRequest::Status,
        Command::Tor(TorCommand::Ping { node }) => ClientRequest::TorPing { node },
        Command::Machine(MachineCommand::Show) => ClientRequest::MachineName,
        Command::Machine(MachineCommand::Hello { machine }) => {
            ClientRequest::MachineHello { machine }
        }
        Command::Directory(DirectoryCommand::Read) => ClientRequest::ReadDirectories,
        Command::Followers(FollowersCommand::List) => ClientRequest::Followers,
        Command::Followers(FollowersCommand::Remove { who }) => ClientRequest::RemoveFollower {
            node: find_follower(&home, &who, false).await?,
        },
        Command::Followers(FollowersCommand::Allow { who }) => ClientRequest::AllowFollower {
            node: find_follower(&home, &who, true).await?,
        },
        Command::Direct(DirectCommand::List) => ClientRequest::DirectContacts,
        Command::Direct(DirectCommand::Allow { node }) => ClientRequest::AllowDirect { node },
        Command::Direct(DirectCommand::Remove { node }) => ClientRequest::RemoveDirect { node },
        Command::Follow { ticket } => match ticket.parse() {
            Ok(actor) => ClientRequest::FollowPerson { actor },
            Err(_) => ClientRequest::Follow { ticket },
        },
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
        Command::Search { words, limit } => ClientRequest::Search {
            words: words.join(" "),
            limit,
        },
        Command::Reshare { id, host } => ClientRequest::Reshare { id, host },
        Command::Fetch { id, out } => ClientRequest::Fetch {
            id,
            out_dir: out.map(|p| std::path::absolute(&p)).transpose()?,
        },
        Command::Sync => ClientRequest::Sync,
        Command::Replies { post } => ClientRequest::Thread { post },
        Command::ReplyRequests => ClientRequest::ReplyRequests,
        Command::ReplyPublish { reply, allow } => ClientRequest::AnswerReply {
            reply,
            answer: match allow {
                true => ReplyAnswer::PublishAndAllow,
                false => ReplyAnswer::Publish,
            },
        },
        Command::ReplyDecline { reply, block } => ClientRequest::AnswerReply {
            reply,
            answer: match block {
                true => ReplyAnswer::Block,
                false => ReplyAnswer::Decline,
            },
        },
        Command::ReplyHide { reply, show } => ClientRequest::HideReply {
            reply,
            hidden: !show,
        },
        Command::ReplyPolicy { post, policy } => ClientRequest::SetReplyPolicy { post, policy },
        Command::Repliers(command) => {
            let (actor, choice) = match command {
                RepliersCommand::Allow { actor } => (actor, Some(ReplierChoice::Allow)),
                RepliersCommand::Block { actor } => (actor, Some(ReplierChoice::Block)),
                RepliersCommand::Forget { actor } => (actor, None),
            };
            ClientRequest::SetReplier { actor, choice }
        }
    };
    let resp = send(&home, request).await?;
    print(resp, cli.json)
}

/// Writes to standard output. If its reader has gone, the program ends with
/// the code a shell gives one killed by a closed pipe (128 + SIGPIPE).
fn write_out(args: std::fmt::Arguments) {
    use std::io::Write as _;
    if let Err(e) = std::io::stdout().write_fmt(args) {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(141);
        }
        panic!("failed printing to stdout: {e}");
    }
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
    tor: bool,
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
    // On for new nodes only: a config without them keeps them off, as a node
    // on Tor reaches only nodes on Tor (ADR 0014).
    config.tor.enabled = tor;
    config.endpoints.enabled = tor;
    config
}

/// The directories given by `--directory MACHINE NODE`, which clap hands over
/// as one flat list of pairs.
fn known_directories(ids: &[EndpointId]) -> Vec<osp_node::KnownDirectory> {
    ids.as_chunks::<2>()
        .0
        .iter()
        .map(|&[machine, node]| osp_node::KnownDirectory { machine, node })
        .collect()
}

/// What `osp init`'s flags already answer.
struct Answers {
    share: Option<bool>,
    list_me: Option<bool>,
    /// No `--name` was given, so a new node asks for one.
    ask_name: bool,
}

/// A yes-or-no flag pair: `None` if neither was given, so it's asked.
fn answered(yes: bool, no: bool) -> Option<bool> {
    match (yes, no) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        _ => None,
    }
}

async fn init(home: &Path, mut config: Config, check_words: bool, answers: Answers) -> Result<()> {
    // Asked first, so words aren't shown for a node that isn't made.
    if answers.ask_name {
        config.name = ask_name()?;
    }
    config.swarm.share_prefetched = ask_share(answers.share)?;
    config.directory.list_me = ask_list_me(answers.list_me, config.tor.enabled)?;
    let words = osp_node::backup::new_identity_words();
    print_words(&words);
    if check_words {
        check_written_down(&words)?;
    }
    Node::init_with_words(home, &config, &words.join(" "))?;
    println!();
    println!("Created a node in {}", home.display());
    if config.tor.enabled {
        println!();
        println!("With Tor on, this node also gets to know other nodes in the background, under a");
        println!(
            "machine name tied to no profile. It isn't anonymous: it shows that a machine runs"
        );
        println!(
            "this software and when it's on, and someone reaching it and your node could match"
        );
        println!(
            "them over time. To turn it off, set enabled = false under [machine] in config.toml."
        );
    }
    say_missing(&config).await;
    println!();
    println!("Start it with: osp --home {} daemon", home.display());
    Ok(())
}

/// Says what this machine still needs for the node to work, and how to get it.
async fn say_missing(config: &Config) {
    let mut missing = Vec::new();
    if config.tor.enabled && !runs(&config.tor.binary, "--version") {
        missing.push("Tor, to reach other nodes: sudo apt install tor".to_string());
    }
    if !osp_node::media::tools_available(&config.media).await {
        missing.push("ffmpeg, for photos and videos: sudo apt install ffmpeg".to_string());
    } else if !osp_node::media::heic_tool_available(&config.media).await {
        missing.push(
            "heif-convert, for photos from iPhones: sudo apt install libheif-examples".to_string(),
        );
    }
    if missing.is_empty() {
        return;
    }
    println!();
    println!("This machine still needs (the commands are for Debian and Ubuntu):");
    for what in &missing {
        println!("- {what}");
    }
    println!("The node starts anyway, and `osp status` says what's missing.");
}

/// Whether `program` runs and says it's fine.
fn runs(program: &str, version_flag: &str) -> bool {
    std::process::Command::new(program)
        .arg(version_flag)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn restore_node(
    home: &Path,
    mut config: Config,
    backup: Option<&Path>,
    old_key: Option<EarlierKeys>,
    answers: Answers,
) -> Result<()> {
    let words = read_words()?;
    let old_key = match old_key {
        Some(choice) => choice,
        None => ask_old_key(backup.is_some())?,
    };
    // A backup doesn't keep the settings, so these are asked again.
    config.swarm.share_prefetched = ask_share(answers.share)?;
    config.directory.list_me = ask_list_me(answers.list_me, config.tor.enabled)?;
    let restored = Node::restore(home, &config, &words, backup, old_key)?;
    println!("Restored identity {}", restored.identity);
    println!("on a new node in {}.", home.display());
    if let Some(info) = &restored.from_file {
        println!(
            "From the backup: {} posts, {} follows, {} followers and {} media files.",
            info.posts, info.follows, info.followers, info.media_files
        );
        if let Some(line) = trust_line(info) {
            println!("{line} (`osp trust list`, `osp direct list`)");
        }
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
    say_missing(&config).await;
    println!();
    println!("Start it with: osp --home {} daemon", home.display());
    Ok(())
}

/// What a backup holds of who this node trusts and lets connect directly,
/// if anything.
fn trust_line(info: &osp_proto::rpc::BackupInfo) -> Option<String> {
    (info.trusted > 0 || info.direct_contacts > 0).then(|| {
        format!(
            "Trusted followers' nodes: {}. Contacts for direct connections: {}.",
            info.trusted, info.direct_contacts
        )
    })
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

/// Asks for the name others see on your profile. With no answer at all, as
/// from a script, or an empty one, it stays Anonymous.
fn ask_name() -> Result<String> {
    println!();
    println!("The name others see on your posts and profile. You can change it later:");
    println!("`name` in config.toml, then start the node again.");
    loop {
        print!("Your name (Enter for Anonymous): ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            return Ok(Config::default().name);
        }
        let name = line.trim();
        if name.is_empty() {
            return Ok(Config::default().name);
        }
        if name.len() > osp_proto::types::MAX_NAME_BYTES || name.chars().any(char::is_control) {
            println!(
                "Please keep it to one line of up to {} bytes.",
                osp_proto::types::MAX_NAME_BYTES
            );
            continue;
        }
        return Ok(name.to_string());
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
    ask_yes_no(
        "Share copies of posts from people you follow?",
        "--share or --no-share",
    )
}

/// Asks whether to ask directories to list this node's profile and latest
/// posts (ADR 0015), unless `choice` already says. With Tor off it isn't
/// asked, as directories are reached only through Tor. There's no default:
/// being listed shows your profile and posts to anyone, so it needs a yes.
fn ask_list_me(choice: Option<bool>, tor: bool) -> Result<bool> {
    if !tor {
        return Ok(false);
    }
    if let Some(list_me) = choice {
        return Ok(list_me);
    }
    println!();
    println!("Directories let people find each other without swapping follow codes. A");
    println!("directory is a node someone runs for this; the ones yours reads go under");
    println!("[[directory.known]] in config.toml.");
    println!();
    println!("If you ask them to list you:");
    println!(
        "- your profile and latest {MAX_LISTED_POSTS} posts go into them, where anyone who reads one"
    );
    println!("  can see them and pass them on;");
    println!("- they reach your node through Tor, so they don't learn your internet address.");
    println!("If not, people find you only through a follow code you give them (`osp invite`).");
    println!();
    println!("You can change it later: `list_me` under [directory] in config.toml.");
    ask_yes_no("Ask directories to list you?", "--list-me or --no-list-me")
}

/// Asks `question` until the answer is yes or no. With no answer at all, nothing
/// is made, and the error names the `flags` that answer it.
fn ask_yes_no(question: &str, flags: &str) -> Result<bool> {
    loop {
        print!("{question} (yes / no): ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            bail!("no answer; nothing was created (or pass {flags})");
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
                    "osp_node=info,osp_tor=info,iroh=warn,iroh::net_report::report=error,iroh_blobs=warn"
                        .into()
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

async fn trust_list(home: &Path) -> Result<(Vec<TrustInfo>, Vec<TrustInfo>)> {
    match send(home, ClientRequest::TrustRequests).await? {
        ClientResponse::Trust { requests, trusted } => Ok((requests, trusted)),
        ClientResponse::Error { message } => bail!(message),
        other => bail!("unexpected reply {other:?}"),
    }
}

/// The node of a request waiting, by its person's name, or the start of
/// their identity or the node's ID.
async fn find_request(home: &Path, who: &str) -> Result<EndpointId> {
    let (requests, _) = trust_list(home).await?;
    let nodes = requests
        .iter()
        .filter(|r| matches_person(who, &r.name, &r.actor, Some(&r.node)))
        .map(|r| r.node)
        .collect();
    only_one(who, nodes, "request", "osp trust requests")
}

/// Someone this node follows, by name or the start of their identity.
async fn find_followed(home: &Path, who: &str) -> Result<PublicKey> {
    let follows = match send(home, ClientRequest::Following).await? {
        ClientResponse::Following { follows } => follows,
        ClientResponse::Error { message } => bail!(message),
        other => bail!("unexpected reply {other:?}"),
    };
    let actors = follows
        .iter()
        .filter(|f| matches_person(who, f.name.as_deref().unwrap_or(""), &f.actor, None))
        .map(|f| f.actor)
        .collect();
    only_one(who, actors, "person you follow", "osp following")
}

/// Whether `who` is this name (ignoring case), or the start of this
/// identity or node ID.
fn matches_person(who: &str, name: &str, actor: &PublicKey, node: Option<&EndpointId>) -> bool {
    let who = who.trim().to_lowercase();
    !who.is_empty()
        && (name.to_lowercase() == who
            || actor.to_string().starts_with(&who)
            || node.is_some_and(|n| n.to_string().starts_with(&who)))
}

/// A follower's node by name or the start of its ID, or, with `removed`, a
/// removed one's. A full node ID is taken as it is, so a node can be removed
/// before it follows.
async fn find_follower(home: &Path, who: &str, removed: bool) -> Result<EndpointId> {
    if let Ok(node) = who.trim().parse::<EndpointId>() {
        return Ok(node);
    }
    let (followers, gone) = match send(home, ClientRequest::Followers).await? {
        ClientResponse::Followers { followers, removed } => (followers, removed),
        ClientResponse::Error { message } => bail!(message),
        other => bail!("unexpected reply {other:?}"),
    };
    let nodes = if removed {
        gone.into_iter()
            .filter(|n| matches_follower(who, None, None, n))
            .collect()
    } else {
        followers
            .iter()
            .filter(|f| matches_follower(who, f.name.as_deref(), f.actor.as_ref(), &f.node))
            .map(|f| f.node)
            .collect()
    };
    let what = if removed { "removed node" } else { "follower" };
    only_one(who, nodes, what, "osp followers list")
}

/// Whether `who` is a follower's name, or the start of its identity or node.
fn matches_follower(
    who: &str,
    name: Option<&str>,
    actor: Option<&PublicKey>,
    node: &EndpointId,
) -> bool {
    let who = who.trim().to_lowercase();
    !who.is_empty()
        && (name.is_some_and(|n| !n.is_empty() && n.to_lowercase() == who)
            || actor.is_some_and(|a| a.to_string().starts_with(&who))
            || node.to_string().starts_with(&who))
}

/// The one thing found, once duplicates are dropped.
fn only_one<T: PartialEq + Copy>(who: &str, found: Vec<T>, what: &str, see: &str) -> Result<T> {
    let mut unique = Vec::new();
    for one in found {
        if !unique.contains(&one) {
            unique.push(one);
        }
    }
    match unique[..] {
        [one] => Ok(one),
        [] => bail!("no {what} matches {who:?} (see `{see}`)"),
        _ => bail!("{who:?} matches several; give more of the ID (see `{see}`)"),
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
        ClientResponse::Thread { policy, replies } => {
            let who = match policy {
                ReplyPolicy::Off => "nobody",
                ReplyPolicy::Circle => "your circle",
                ReplyPolicy::Network => "your circle, and anyone else through your requests",
            };
            println!("Who may reply: {who} ({}).", policy.as_str());
            if replies.is_empty() {
                println!("No replies yet.");
            }
            replies.iter().for_each(print_reply);
        }
        ClientResponse::ReplyRequests { requests } => {
            if requests.is_empty() {
                println!("No replies are waiting for you.");
            } else {
                println!(
                    "Replies waiting for you, newest first. Each name is what that person says."
                );
                requests.iter().for_each(print_reply);
                println!();
                println!("`osp reply-publish <reply>` (--allow to let them in from now on), or");
                println!("`osp reply-decline <reply>` (--block to block them); they aren't told.");
            }
        }
        ClientResponse::Status(status) => print_status(&status),
        ClientResponse::Followers { followers, removed } => print_followers(&followers, &removed),
        ClientResponse::FollowerRemoved { was_following } => {
            if was_following {
                println!("Removed. That node isn't told of your new posts or named to others any");
                println!("more, and isn't trusted. Its subscriptions are refused until");
                println!("`osp followers allow`. Your posts stay public: it can still read them,");
                println!("as anyone can.");
            } else {
                println!(
                    "That node wasn't following you. Its subscriptions are refused from now on."
                );
            }
        }
        ClientResponse::FollowerAllowed { was_removed } => {
            if was_removed {
                println!("Allowed: that node can follow you again, from its next subscription.");
            } else {
                println!("That node wasn't removed.");
            }
        }
        ClientResponse::DirectContacts { nodes } => {
            if nodes.is_empty() {
                println!("No nodes allowed a direct connection.");
            }
            for node in nodes {
                println!("{node}");
            }
        }
        ClientResponse::DirectoriesRead { found, failed } => {
            for directory in &failed {
                println!("Didn't answer: {directory}");
            }
            let people = if found == 1 { "person" } else { "people" };
            let read = if failed.is_empty() {
                "Read"
            } else {
                "Read the others"
            };
            println!("{read}. This node holds {found} {people} it found through directories.");
            if !failed.is_empty() {
                println!(
                    "A new node's first connections over Tor can take a few minutes: try again soon."
                );
            }
        }
        ClientResponse::MachineName {
            machine,
            words,
            heard_of,
            met,
            known,
        } => {
            println!("{words}");
            println!("{machine}");
            println!("It knows {known} machines, has met {met} more and heard of {heard_of}.");
        }
        ClientResponse::MachineGreeted {
            words,
            offers,
            connect_ms,
            round_trip_ms,
        } => {
            println!(
                "Greeted {words}: connected in {}, asked in {}",
                secs(connect_ms),
                secs(round_trip_ms)
            );
            if !offers.is_empty() {
                println!("It offers: {}", offers.join(", "));
            }
        }
        ClientResponse::TorPinged {
            connect_ms,
            round_trip_ms,
        } => match connect_ms {
            Some(ms) => println!(
                "Reached over Tor: connected in {}, asked in {}",
                secs(ms),
                secs(round_trip_ms)
            ),
            None => println!(
                "Reached over Tor, on the connection already open: asked in {}",
                secs(round_trip_ms)
            ),
        },
        ClientResponse::Ticket { ticket } => println!("{ticket}"),
        ClientResponse::Posted { id } => println!("Published {id}"),
        ClientResponse::Following { follows } => {
            if follows.is_empty() {
                println!("You don't follow anyone yet.");
            }
            for f in follows {
                println!(
                    "{}  {}",
                    plain(f.name.as_deref().unwrap_or("(name unknown)")),
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
        ClientResponse::Found { people, posts } => {
            if people.is_empty() && posts.is_empty() {
                println!("Nothing on this node matches.");
            }
            for person in people {
                let followed = if person.followed { " (following)" } else { "" };
                println!("{}{followed}", plain(&person.name));
                if let Some(summary) = person.summary {
                    println!("  {}", plain(&summary));
                }
                println!("  identity: {}", person.actor);
                if let Some(directory) = person.found_through {
                    println!("  found through directory {directory}");
                }
                println!();
            }
            for item in posts {
                print_item(&item);
                println!();
            }
        }
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
        ClientResponse::Trust { requests, trusted } => {
            print_trust_requests(&requests);
            println!();
            print_trusted(&trusted);
        }
        ClientResponse::Paired { name, .. } => println!("Paired with {name}'s node."),
        ClientResponse::Blob { size: bytes } => println!("{}", size(bytes)),
        ClientResponse::Resume { received } => println!("{} received", size(received)),
        ClientResponse::BackedUp { path, info } => {
            println!("Wrote {} ({}):", path.display(), size(info.bytes));
            println!(
                "{} posts, {} follows, {} followers and {} media files.",
                info.posts, info.follows, info.followers, info.media_files
            );
            if let Some(line) = trust_line(&info) {
                println!("{line}");
            }
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

/// Milliseconds as seconds, to a tenth.
fn secs(ms: u64) -> String {
    format!("{:.1} s", ms as f64 / 1000.0)
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
    // An older node doesn't say.
    if s.transfer_places > 0 {
        let measured = match s.upload_measured_mbps {
            Some(mbps) => format!(", for an upload measured at {mbps:.1} Mbit/s"),
            None => String::new(),
        };
        println!(
            "Serving:   {} media transfers at once{measured}",
            s.transfer_places
        );
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
    if let Some(tor) = &s.tor {
        let published = match (tor.ready, tor.published) {
            (true, true) => ", onion service published",
            (true, false) => ", onion service not published yet",
            _ => "",
        };
        let restarts = match tor.restarts {
            0 => String::new(),
            1 => ", restarted once".to_string(),
            n => format!(", restarted {n} times"),
        };
        let streams = match tor.streams {
            1 => "1 stream".to_string(),
            n => format!("{n} streams"),
        };
        println!("Tor:       {}{published}, {streams}{restarts}", tor.state);
    }
    if s.machine {
        println!("Machine:   on (`osp machine show` gives its name)");
    }
    if let Some(directory) = &s.directory {
        println!(
            "Directory: {} people, {} machines",
            directory.people, directory.machines
        );
    }
    if s.found_people > 0 {
        println!("Found:     {} people through directories", s.found_people);
    }
    if s.shared.posts > 0 {
        println!(
            "Shared:    {} post(s) with hosting, {} served to others (counted here only)",
            s.shared.posts,
            size(s.shared.served_bytes)
        );
    }
    if let Some(own) = &s.endpoints {
        let media = match own.media {
            Some(media) => media.fmt_short().to_string(),
            None => "none, as relays are off".to_string(),
        };
        println!(
            "Endpoints: direct {} (allows {}), media {media}",
            own.direct.fmt_short(),
            own.rule
        );
    }
    match s.trust_requests {
        0 => {}
        1 => {
            println!("Trust:     a request to be a trusted follower waiting (`osp trust requests`)")
        }
        n => println!(
            "Trust:     {n} requests to be a trusted follower waiting (`osp trust requests`)"
        ),
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
        if p.trusted {
            what.push("trusted");
        }
        if p.asks_trust {
            what.push("asks to be trusted");
        }
        println!();
        println!(
            "{}  {}  {}",
            plain(p.name.as_deref().unwrap_or("(unknown)")),
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

/// A reply as shown in a terminal, its words made plain.
fn print_reply(r: &ReplyInfo) {
    let hidden = match r.state {
        ReplyStatus::Hidden => "  [hidden]",
        _ => "",
    };
    println!();
    println!(
        "{} ({}) · {}{hidden}",
        shown(&plain(&r.name)),
        r.actor.fmt_short(),
        plain(&r.published)
    );
    if let Some(why) = &r.why {
        println!("  {why}");
    }
    for line in r.content.lines() {
        println!("  {}", plain(line));
    }
    println!("  reply {}", r.id);
}

fn print_trust_requests(requests: &[TrustInfo]) {
    if requests.is_empty() {
        println!("No requests to be a trusted follower are waiting.");
        return;
    }
    println!("Requests to be a trusted follower, newest first. Each name and note is what");
    println!("that person says: only following them yourself shows it's them.");
    for r in requests {
        println!();
        println!("{}  node {}", shown(&r.name), r.node.fmt_short());
        if !r.note.is_empty() {
            println!("  says: {}", r.note);
        }
        println!("  asked {} ago", uptime(r.secs_ago));
        if r.followed {
            println!("  you follow them, so this identity is the one you added");
        } else {
            println!("  you don't follow them: to be sure it's them, get their follow code in");
            println!("  person or another way you trust, follow them, then allow");
        }
        if r.trusted_elsewhere {
            println!("  already trusted on another node");
        }
        println!("  {}", follower_line(r));
    }
    println!();
    println!("Allowing one names it to your other followers as a host more often, so more");
    println!("of them can see that this person follows you. `osp trust allow <name>` or");
    println!("`osp trust decline <name>`; they aren't told either way.");
}

fn print_trusted(trusted: &[TrustInfo]) {
    if trusted.is_empty() {
        println!("You don't trust anyone yet. `osp trust add <name>` trusts someone you follow;");
        println!("followers can also ask (`osp trust requests`).");
        return;
    }
    println!("Trusted followers, most recently trusted first:");
    for t in trusted {
        println!();
        let followed = if t.followed { ", you follow them" } else { "" };
        println!(
            "{}  node {}  trusted {} ago{followed}",
            shown(&t.name),
            t.node.fmt_short(),
            uptime(t.secs_ago)
        );
        println!("  {}", follower_line(t));
    }
}

fn print_followers(followers: &[FollowerInfo], removed: &[EndpointId]) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let ago = |ms: i64| uptime((now - ms).max(0) as u64 / 1000);
    if followers.is_empty() {
        println!("Nobody follows you yet.");
    } else {
        println!("Followers' nodes, oldest first:");
    }
    for f in followers {
        // A name comes from someone else's profile, so it's made plain first.
        let name = f.name.as_deref().map(plain).unwrap_or_default();
        let who = if f.actor.is_none() {
            "(whose node: not known)".to_string()
        } else {
            shown(&name).to_string()
        };
        let mut notes = Vec::new();
        if f.shares {
            notes.push("shares copies");
        }
        if f.trusted {
            notes.push("trusted");
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!("  ({})", notes.join(", "))
        };
        println!(
            "{who}  node {}  since {} ago, heard from {} ago{notes}",
            f.node.fmt_short(),
            ago(f.since_ms),
            ago(f.seen_ms)
        );
    }
    if !removed.is_empty() {
        println!();
        println!("Removed, refused until `osp followers allow`:");
        for node in removed {
            println!("{node}");
        }
    }
}

fn shown(name: &str) -> &str {
    if name.is_empty() { "(no name)" } else { name }
}

/// How long a node has followed, how it reached this one lately, and the
/// copies it holds.
fn follower_line(t: &TrustInfo) -> String {
    let following = match t.follower_secs {
        Some(secs) => format!("follows you for {}", uptime(secs)),
        None => "no longer follows you".to_string(),
    };
    let reached = match t.reached {
        Some(Reached::Direct) => "reached you directly lately",
        Some(Reached::Relay) => "reached you through a relay lately",
        Some(Reached::Tor) => "reached you over Tor lately",
        None => "not heard from lately",
    };
    format!(
        "{following}; {reached}; holds copies of {} of your files",
        t.copies
    )
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
    print!("{}", item_text(item));
}

/// A post as shown in a terminal. Names and words come from other people, so
/// they're made plain first: a directory brings words from strangers.
fn item_text(item: &FeedItem) -> String {
    let mut out = String::new();
    let author = item
        .actor_name
        .clone()
        .unwrap_or_else(|| item.actor.fmt_short().to_string());
    let _ = writeln!(out, "{} · {}", plain(&author), plain(&item.published));
    if let Some(by) = &item.reshared_by {
        let name = by
            .name
            .clone()
            .unwrap_or_else(|| by.actor.fmt_short().to_string());
        let hosting = if by.hosting { " (hosting)" } else { "" };
        let _ = writeln!(out, "  reshared by {}{hosting}", plain(&name));
    }
    if let Some(directory) = &item.found_through {
        let _ = writeln!(out, "  found through directory {directory}");
    }
    if !item.found_reshares.is_empty() {
        let names: Vec<String> = item
            .found_reshares
            .iter()
            .map(|by| {
                let name = by
                    .name
                    .clone()
                    .unwrap_or_else(|| by.actor.fmt_short().to_string());
                let hosting = if by.hosting { " (hosting)" } else { "" };
                format!("{}{hosting}", plain(&name))
            })
            .collect();
        let _ = writeln!(out, "  reshared by {}", names.join(", "));
    }
    for line in item.content.lines() {
        let _ = writeln!(out, "  {}", plain(line));
    }
    for a in &item.attachments {
        let m = &a.attachment;
        let duration = m
            .duration_ms
            .map(|ms| format!(" {:.1}s", ms as f64 / 1000.0))
            .unwrap_or_default();
        let local = if a.local { "stored" } else { "not downloaded" };
        let _ = writeln!(
            out,
            "  [{:?} {}x{}{} {}, {local}]",
            m.kind,
            m.width,
            m.height,
            duration,
            size(m.size)
        );
    }
    let _ = writeln!(out, "  id: {}", item.id);
    // Your reshare has an ID of its own, which is what deleting it takes.
    if let Some(yours) = item.deletable.filter(|id| *id != item.id) {
        let _ = writeln!(out, "  your reshare: {yours}");
    }
    out
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use osp_proto::rpc::ResharedBy;

    use super::*;

    #[test]
    fn init_takes_directories_as_machine_and_node_pairs() {
        let id = || osp_proto::SecretKey::generate().public().to_string();
        let (m1, n1, m2, n2) = (id(), id(), id(), id());
        let cli = Cli::try_parse_from([
            "osp",
            "init",
            "--directory",
            &m1,
            &n1,
            "--directory",
            &m2,
            &n2,
        ])
        .unwrap();
        let Command::Init { directory, .. } = cli.command else {
            panic!("not init");
        };
        let known = known_directories(&directory);
        assert_eq!(known.len(), 2);
        assert_eq!(known[0].machine.to_string(), m1);
        assert_eq!(known[0].node.to_string(), n1);
        assert_eq!(known[1].machine.to_string(), m2);
        assert_eq!(known[1].node.to_string(), n2);
        // Half a pair, or one with Tor off, isn't taken.
        assert!(Cli::try_parse_from(["osp", "init", "--directory", &m1]).is_err());
        assert!(Cli::try_parse_from(["osp", "init", "--no-tor", "--directory", &m1, &n1]).is_err());
    }

    #[test]
    fn nothing_from_another_person_can_move_the_terminals_cursor() {
        let actor: PublicKey = "ae".repeat(32).parse().unwrap();
        let escape = "\u{1b}[2J\u{1b}]0;owned\u{7}";
        let item = FeedItem {
            id: osp_proto::ObjectId::of(b"post"),
            actor,
            actor_name: Some(format!("Mallory{escape}")),
            published: format!("2026-11-01{escape}"),
            content: format!("First line{escape}\nSecond line{escape}"),
            attachments: vec![],
            reshared_by: Some(ResharedBy {
                actor,
                name: Some(format!("Mallory{escape}")),
                announce: osp_proto::ObjectId::of(b"reshare"),
                published: "2026-11-01".into(),
                hosting: false,
            }),
            found_through: Some("oak-ash-elm".into()),
            deletable: None,
            found_reshares: vec![ResharedBy {
                actor,
                name: Some(format!("Jimmy{escape}")),
                announce: osp_proto::ObjectId::of(b"found reshare"),
                published: "2026-11-02".into(),
                hosting: true,
            }],
        };
        let shown = item_text(&item);
        let bad: Vec<_> = shown
            .chars()
            .filter(|c| c.is_control() && *c != '\n')
            .collect();
        assert!(bad.is_empty(), "{bad:?} in {shown:?}");
        // Lines stay lines, and it says where it came from.
        assert!(shown.contains("First line"), "{shown}");
        assert!(shown.contains("\n  Second line"), "{shown}");
        assert!(
            shown.contains("found through directory oak-ash-elm"),
            "{shown}"
        );
        assert!(shown.contains("(hosting)"), "{shown}");
    }
}
