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
    BlobHash, EarlierKeys, EndpointId, ObjectId, OspCode, PublicKey,
    connected::{self, Connection, How, SideFrom},
    framing::{read_frame, write_frame},
    machine::MAX_LISTED_POSTS,
    rpc::{
        ActivityEntry, ActivityPerson, ActivityWhat, BlockTarget, ClientRequest, ClientResponse,
        DeviceInfo, Direction, FeedItem, FollowerInfo, IconChange, PeerRecord, ProfileInfo,
        ProfileItemKind, Reached, ReplierChoice, ReplyAnswer, ReplyInfo, ReplyStatus,
        SentReplyInfo, SentStatus, StatusInfo, TrustInfo,
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
        /// Relays: "default" (n0's public relays), "disabled", or your own relay URL.
        /// Repeat it for more than one URL
        #[arg(long, default_value = "default")]
        relay: Vec<String>,
        /// The token your own relays ask for (ADR 0044). Only with relay URLs
        #[arg(long, env = "OSP_RELAY_TOKEN", hide_env_values = true)]
        relay_token: Option<String>,
        /// Relays used only while none of yours lets the node in: "default" (n0's),
        /// "disabled", or a URL. Only with relay URLs
        #[arg(long, default_value = "disabled")]
        relay_fallback: String,
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
        /// Use the project's hide list (ADR 0042), written in config.toml: the worst posts and
        /// files aren't shown, fetched or passed on. It's on without this too
        #[arg(long, conflicts_with = "no_hide_list")]
        hide_list: bool,
        /// Don't use it. `osp hide-list on` turns it on later
        #[arg(long)]
        no_hide_list: bool,
        /// A directory this node reads, by its machine name and public ID, written under
        /// [[directory.known]] in config.toml. Repeat it for more than one. There's no default:
        /// an installer passes the ones to use
        #[arg(long, num_args = 2, value_names = ["MACHINE", "NODE"], conflicts_with = "no_tor")]
        directory: Vec<EndpointId>,
        /// A seed carrier (ADR 0043), by its machine name, written under [[carrier.known]]
        /// in config.toml. Repeat it for more than one. There's no default: an installer
        /// passes the ones to use
        #[arg(long, value_name = "MACHINE", conflicts_with = "no_tor")]
        carrier: Vec<EndpointId>,
        /// Your connection's upload speed in Mbit/s: carrying other nodes' video uses half
        /// of it at most (ADR 0043). Otherwise you're asked
        #[arg(long, value_name = "MBIT/S")]
        upload_mbps: Option<u64>,
        /// Your internet plan's monthly data cap in GB, 0 for unlimited: carrying keeps to
        /// half of it. Otherwise you're asked
        #[arg(long, value_name = "GB")]
        data_cap_gb: Option<u64>,
        /// Let the nodes yours picks to carry its video see its internet address, so its
        /// video goes through them, not the public fallback (ADR 0043 §8). Otherwise you're
        /// asked, if the installer named seed carriers
        #[arg(long, conflicts_with = "no_hops")]
        hops: bool,
        /// Keep this node's own video on the public fallback relays. Otherwise you're asked
        #[arg(long)]
        no_hops: bool,
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
    /// Block a person, a post or a file: your node drops what it holds of
    /// it, and won't fetch, serve or show it again. Nothing is sent; they
    /// aren't told
    #[command(subcommand)]
    Block(BlockCommand),
    /// Undo a block. What it dropped comes back as it's fetched again
    #[command(subcommand)]
    Unblock(BlockCommand),
    /// List what you blocked
    Blocks,
    /// The project's hide list (ADR 0042): what it holds isn't fetched,
    /// served or shown. With no change given, how it stands
    HideList {
        #[command(subcommand)]
        change: Option<HideListCommand>,
    },
    /// Carrying other nodes' encrypted video (ADR 0043): with no change given,
    /// how it goes
    Carrier {
        #[command(subcommand)]
        change: Option<CarrierCommand>,
    },
    /// Whether the nodes yours picks to carry its video may see its internet
    /// address (ADR 0043 §8): yes, its video goes through them; no, through
    /// the public fallback relays
    Hops { answer: YesNo },
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
    /// Everyone your node holds a profile of: those you follow and those directories brought, newest post first. Nothing leaves your node
    People {
        /// How many people at most
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// What the Universe shows: the people your node knows, how near each is
    /// to you and the public lines between them, the machines it knows, and
    /// its totals. Built on your node alone; nothing leaves it
    Universe,
    /// What your node did or saw in the last day, oldest first: people a
    /// directory brought, reshares, media fetched and served, machines met
    /// or faded. Nothing leaves your node
    Events {
        /// Only those after this number, as the last one printed showed
        #[arg(long)]
        since: Option<u64>,
    },
    /// Show someone's profile and how many of their posts your node holds,
    /// or one page of them. With no one named, your own
    Profile {
        /// A name, or the start of an identity, of someone your node found
        who: Option<String>,
        /// List their posts, a page at a time
        #[arg(long, conflicts_with_all = ["media", "reshares"])]
        posts: bool,
        /// List their posts with photos or videos
        #[arg(long, conflicts_with = "reshares")]
        media: bool,
        /// List their reshares
        #[arg(long)]
        reshares: bool,
        /// Show the page after this post or reshare
        #[arg(long)]
        before: Option<ObjectId>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Change your profile. Anything not given stays as it is. Followers see
    /// the change at their next sync. Your picture is as public as your name
    EditProfile {
        #[arg(long)]
        name: Option<String>,
        /// A few words about you; an empty one removes it
        #[arg(long)]
        summary: Option<String>,
        /// A link to show, as `Name=https://...` (repeatable, at most 4).
        /// Giving any replaces them all
        #[arg(long = "link")]
        links: Vec<String>,
        /// Remove every link
        #[arg(long, conflicts_with = "links")]
        no_links: bool,
        /// A photo for your profile picture, cut to a square
        #[arg(long, conflicts_with = "no_picture")]
        picture: Option<PathBuf>,
        /// Remove your profile picture
        #[arg(long)]
        no_picture: bool,
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
    /// Reply to someone's post. Your node keeps trying their node for 30 days
    /// until it answers; `osp sent-replies` shows where each reply stands
    Reply {
        post: ObjectId,
        /// What you say, at most 4,000 bytes
        text: String,
    },
    /// Like someone's post. Your node keeps trying their node for 7 days until
    /// it answers; only they see who likes it
    Like { post: ObjectId },
    /// Take back your like of a post
    Unlike { post: ObjectId },
    /// List who likes one of your posts, newest first
    Likes { post: ObjectId },
    /// Show your Activity, the latest first: replies, likes, reshares, new
    /// followers, people who say they know you, and your replies published.
    /// Unread entries are marked with a dot
    Activity {
        /// Read on past the entry with this number
        #[arg(long)]
        before: Option<u64>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Mark Activity entries read, on your node, so your devices agree
    ActivityRead {
        /// Every entry up to this number (as `osp activity` shows it)
        #[arg(long)]
        through: Option<u64>,
        /// Entries by their key (as `osp activity --json` shows it)
        keys: Vec<String>,
    },
    /// Show the replies to a post, and who may reply to it. Others' posts are
    /// read without showing who you are, a page at a time
    Replies {
        post: ObjectId,
        /// Show the page after this place in the thread
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
    /// List your replies to others' posts, newest first, and where each stands
    SentReplies,
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
        /// Make a code for an app store's reviewer instead: it lasts days and
        /// pairs a few devices, which `--end-review` unpairs. Use it only on
        /// a demo node, never your own
        #[arg(long, conflicts_with = "end_review")]
        for_review: bool,
        /// How many days the review code lasts
        #[arg(long, default_value_t = 30, requires = "for_review")]
        days: u32,
        /// How many devices the review code pairs
        #[arg(long, default_value_t = 3, requires = "for_review")]
        devices: u32,
        /// End the review: the review code stops working and the devices it
        /// paired are unpaired
        #[arg(long)]
        end_review: bool,
    },
    /// List paired phones
    Devices,
    /// Show what this node measured about the nodes it deals with
    Peers,
    /// Who vouched for you (reshared your posts, or credited you with finding
    /// theirs early), and your side as last gathered (ADR 0029)
    Vouches {
        /// Gather your side now, in the background, rather than at its daily time
        #[arg(long)]
        gather: bool,
    },
    /// How are we connected? The paths your node knows from you to someone
    /// (ADR 0037). If they haven't replied to you, their node is asked once
    /// who vouched for them, from a throwaway key.
    Connected {
        /// Their name or the start of their identity
        who: String,
    },
    /// The card "Share my Universe" would share (ADR 0039): rounded totals
    /// and a drawing that names only the people given here.
    ShareCard {
        /// Someone to name on it, by name or the start of their identity
        #[arg(long = "name")]
        names: Vec<String>,
    },
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
enum BlockCommand {
    /// A person, by name or the start of their identity
    Account { who: String },
    /// A post, by its ID or an OSP code for it
    Post { post: String },
    /// A file, by its hash
    File { hash: BlobHash },
}

#[derive(Subcommand)]
enum CarrierCommand {
    /// Stop carrying, for troubleshooting: what it carries ends, and nothing
    /// new is taken. Your node still gets every post and video
    Pause,
    /// Carry again
    Resume,
    /// Say whether the connection is metered: carrying stops while it is
    Metered { answer: YesNo },
    /// Circuits carried at once [default: 2]
    Jobs { jobs: usize },
    /// The share of your upload carrying may use, in percent [default: 50]
    Share { percent: u8 },
    /// Your connection's upload speed in Mbit/s, which the share is of
    Upload { mbps: u64 },
    /// Your internet plan's monthly data cap in GB, 0 for unlimited:
    /// carrying keeps to half of it
    Cap { gb: u64 },
    /// What carrying may use in a month, in GB, both ways counted, in place
    /// of half your plan's cap. 0 for no limit
    Monthly { gb: u64 },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum YesNo {
    Yes,
    No,
}

#[derive(Subcommand)]
enum HideListCommand {
    /// Apply the list again; it's fetched within a minute or so
    On,
    /// Stop applying it: everything it hid comes back
    Off,
    /// Apply a signed list from a file, as if fetched. It's checked against
    /// the same key
    Apply { file: PathBuf },
    /// Don't hide this person's posts here. Their listed files may be
    /// fetched for you, but your node never passes them on
    Except { actor: PublicKey },
    /// Hide this person's listed posts again
    Unexcept { actor: PublicKey },
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
    /// On a directory: stop listing someone now, such as a node that's gone,
    /// rather than after 30 days without an answer. Readers forget them at
    /// their next read. A node that's still there is listed again when it asks
    Drop {
        /// Their identity, in full
        identity: PublicKey,
    },
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
            relay_token,
            relay_fallback,
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
            hide_list,
            no_hide_list,
            directory,
            carrier,
            upload_mbps,
            data_cap_gb,
            hops,
            no_hops,
        } => {
            let lookup = !no_address_lookup;
            let ask_name = name.is_none();
            let mut config = init_config(name, &relay, bind, lookup, backup.is_some(), !no_tor);
            set_relay_token(&mut config, relay_token, &relay_fallback)?;
            config.directory.known = known_directories(&directory);
            config.carrier.known = carrier
                .into_iter()
                .map(|machine| osp_node::SeedCarrier { machine })
                .collect();
            let answers = Answers {
                share: answered(share, no_share),
                list_me: answered(list_me, no_list_me),
                hide_list: answered(hide_list, no_hide_list),
                upload_mbps,
                data_cap_gb,
                hops: answered(hops, no_hops),
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
        Command::Pair {
            for_review: true,
            no_qr,
            days,
            devices,
            ..
        } => return pair_for_review(&home, no_qr, days, devices, cli.json).await,
        Command::Pair {
            end_review: true, ..
        } => ClientRequest::EndReview,
        Command::Pair { no_qr, .. } => return pair(&home, no_qr, cli.json).await,
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
        Command::Directory(DirectoryCommand::Drop { identity }) => {
            ClientRequest::DropListed { actor: identity }
        }
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
        Command::Block(what) => ClientRequest::Block {
            target: block_target(&home, what).await?,
        },
        Command::Unblock(what) => ClientRequest::Unblock {
            target: block_target(&home, what).await?,
        },
        Command::Blocks => ClientRequest::Blocks,
        Command::HideList { change } => match change {
            None => ClientRequest::HideList,
            Some(HideListCommand::On) => ClientRequest::SetHideList { on: true },
            Some(HideListCommand::Off) => ClientRequest::SetHideList { on: false },
            Some(HideListCommand::Apply { file }) => ClientRequest::ApplyHideList {
                list: std::fs::read_to_string(&file)
                    .with_context(|| format!("reading {}", file.display()))?,
            },
            Some(HideListCommand::Except { actor }) => ClientRequest::HideException {
                actor,
                except: true,
            },
            Some(HideListCommand::Unexcept { actor }) => ClientRequest::HideException {
                actor,
                except: false,
            },
        },
        Command::Carrier { change } => return carrier(&home, change, cli.json).await,
        Command::Hops { answer } => {
            let yes = matches!(answer, YesNo::Yes);
            done(send(&home, ClientRequest::SetHops { yes }).await?)?;
            if !cli.json {
                println!(
                    "{}",
                    match yes {
                        true =>
                            "Your node's video goes through the hops it picks; they see its address.",
                        false => "Your node's video goes through the public fallback relays.",
                    }
                );
            }
            return Ok(());
        }
        Command::Following => ClientRequest::Following,
        Command::Devices => ClientRequest::Devices,
        Command::Peers => ClientRequest::Peers,
        Command::Vouches { gather } => ClientRequest::Vouches { gather },
        Command::Connected { who } => ClientRequest::HowConnected {
            actor: find_person(&home, Some(&who)).await?,
        },
        Command::ShareCard { names } => {
            let mut named = Vec::new();
            for who in &names {
                named.push(find_person(&home, Some(who)).await?);
            }
            ClientRequest::ShareCard { named }
        }
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
        Command::People { limit } => ClientRequest::People { limit },
        Command::Universe => ClientRequest::Universe {
            level: Default::default(),
            region: None,
            layers: vec![],
            since: None,
        },
        Command::Events { since } => ClientRequest::UniverseEvents { since },
        Command::Profile {
            who,
            posts,
            media,
            reshares,
            before,
            limit,
        } => {
            let actor = find_person(&home, who.as_deref()).await?;
            let kind = match (posts, media, reshares) {
                (true, _, _) => Some(ProfileItemKind::Posts),
                (_, true, _) => Some(ProfileItemKind::Media),
                (_, _, true) => Some(ProfileItemKind::Reshares),
                _ => None,
            };
            match kind {
                Some(kind) => ClientRequest::ProfileItems {
                    actor,
                    kind,
                    before,
                    limit,
                },
                None => ClientRequest::Profile { actor },
            }
        }
        Command::EditProfile {
            name,
            summary,
            links,
            no_links,
            picture,
            no_picture,
        } => {
            let me = find_person(&home, None).await?;
            let current = match send(&home, ClientRequest::Profile { actor: me }).await? {
                ClientResponse::Profile { profile } => profile.person,
                ClientResponse::Error { message } => bail!(message),
                other => bail!("unexpected reply {other:?}"),
            };
            let links = match (no_links, links.is_empty()) {
                (true, _) => Vec::new(),
                (false, true) => current.attachment,
                (false, false) => links
                    .iter()
                    .map(|l| {
                        let (name, value) = l
                            .split_once('=')
                            .with_context(|| format!("{l:?}: give a link as Name=https://..."))?;
                        Ok(osp_proto::ProfileLink {
                            name: name.into(),
                            value: value.into(),
                        })
                    })
                    .collect::<Result<_>>()?,
            };
            let icon = match (picture, no_picture) {
                (Some(path), _) => IconChange::File {
                    // The daemon has its own working directory.
                    path: path
                        .canonicalize()
                        .with_context(|| format!("{}", path.display()))?,
                },
                (None, true) => IconChange::Remove,
                (None, false) => IconChange::Keep,
            };
            ClientRequest::SetProfile {
                name: name.unwrap_or(current.name),
                summary: summary.or(current.summary),
                links,
                icon,
            }
        }
        Command::Reshare { id, host } => ClientRequest::Reshare { id, host },
        Command::Fetch { id, out } => ClientRequest::Fetch {
            id,
            out_dir: out.map(|p| std::path::absolute(&p)).transpose()?,
        },
        Command::Sync => ClientRequest::Sync,
        Command::Reply { post, text } => ClientRequest::Reply {
            post,
            content: text,
        },
        Command::Like { post } => ClientRequest::Like { post, liked: true },
        Command::Unlike { post } => ClientRequest::Like { post, liked: false },
        Command::Likes { post } => ClientRequest::Likes { post },
        Command::Activity { before, limit } => ClientRequest::Activity { before, limit },
        Command::ActivityRead { through, keys } => ClientRequest::ActivityRead { keys, through },
        Command::Replies { post, after } => ClientRequest::Thread { post, after },
        Command::SentReplies => ClientRequest::SentReplies,
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
    relay: &[String],
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
    config.network.relay = relay_choice(relay);
    config.network.bind = bind;
    config.network.address_lookup = address_lookup;
    // On for new nodes only: a config without them keeps them off, as a node
    // on Tor reaches only nodes on Tor (ADR 0014).
    config.tor.enabled = tor;
    config.endpoints.enabled = tor;
    config
}

/// `--relay`'s values: "default" or "disabled" alone, otherwise relay URLs.
/// A word mixed in with URLs is taken as a URL, and the node refuses it at start.
fn relay_choice(relay: &[String]) -> RelayChoice {
    match relay {
        [only] if only == "default" => RelayChoice::Default,
        [only] if only == "disabled" => RelayChoice::Disabled,
        urls => RelayChoice::Custom(urls.to_vec()),
    }
}

/// `--relay-token` and `--relay-fallback` (ADR 0044). Both only make sense with
/// your own relays: the token must never go to n0's, and a fallback for n0's
/// relays is n0's relays.
fn set_relay_token(config: &mut Config, token: Option<String>, fallback: &str) -> Result<()> {
    let fallback = relay_choice(&[fallback.to_string()]);
    let token = token.filter(|t| !t.is_empty());
    let own = matches!(config.network.relay, RelayChoice::Custom(_));
    if !own && (token.is_some() || fallback != RelayChoice::Disabled) {
        bail!("--relay-token and --relay-fallback need --relay with your own relay URLs");
    }
    config.network.relay_token = token;
    config.network.relay_fallback = fallback;
    Ok(())
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
    hide_list: Option<bool>,
    upload_mbps: Option<u64>,
    data_cap_gb: Option<u64>,
    hops: Option<bool>,
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
    config.lists.hide = tell_hide_list(answers.hide_list);
    ask_carrying(&mut config, &answers)?;
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
    print_start_line(home);
    Ok(())
}

/// Whether the node will carry other nodes' video: it needs a machine name,
/// so Tor, and a relay.
fn will_carry(config: &Config) -> bool {
    config.carrier.enabled
        && config.tor.enabled
        && config.machine.enabled
        && config.network.relay != RelayChoice::Disabled
}

/// Tells the owner the node carries (ADR 0017, "Told at setup"), and asks
/// what it may use: the upload speed, and whether the hops it picks may see
/// its address (ADR 0043 §8). Carrying itself isn't asked.
fn ask_carrying(config: &mut Config, answers: &Answers) -> Result<()> {
    if !will_carry(config) {
        return Ok(());
    }
    println!();
    for line in CARRYING_TOLD {
        println!("{line}");
    }
    config.serving.upload_mbps = match answers.upload_mbps {
        Some(mbps) => mbps,
        None => ask_upload_mbps()?,
    };
    config.serving.data_cap_gb = match answers.data_cap_gb {
        Some(gb) => gb,
        None => ask_data_cap_gb()?,
    };
    if !config.carrier.known.is_empty() {
        config.carrier.hops = ask_hops(answers.hops)?;
    }
    Ok(())
}

/// ADR 0017's words for setup, as the install guide has them.
const CARRYING_TOLD: &[&str] = &[
    "Your node carries encrypted video between other nodes, under its machine name,",
    "within the limits you set. This uses some of your upload, and builds your node's",
    "standing with the nodes it helps, which can get it direct links and quicker",
    "service. You can't see what's inside. The nodes next to yours see your internet",
    "address sending it, never who you are, and you answer for what your connection",
    "sends, as with anything else you do online. You can pause it (`osp carrier pause`).",
];

/// The connection's upload speed, for carrying's share of it. 0, not known,
/// if the owner skips it: carrying then starts small.
fn ask_upload_mbps() -> Result<u64> {
    println!();
    println!("Carrying uses half your upload at most, always behind your own traffic. A speed");
    println!("test shows your upload; a guess is fine, and you can change it later");
    println!("(`osp carrier upload <Mbit/s>`).");
    loop {
        print!("Your upload speed in Mbit/s (Enter to skip): ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            println!();
            return Ok(0);
        }
        let line = line.trim().trim_end_matches("Mbit/s").trim();
        if line.is_empty() {
            println!("Skipped: carrying takes a slow home's 10 Mbit/s until you set it.");
            return Ok(0);
        }
        match line.parse::<f64>() {
            Ok(mbps) if (1.0..=100_000.0).contains(&mbps) => return Ok(mbps.round() as u64),
            _ => println!("Please give a number of Mbit/s, such as 20, or press Enter to skip."),
        }
    }
}

/// The internet plan's monthly data cap, 0 for unlimited: carrying keeps to
/// half of it in a month, both ways counted.
fn ask_data_cap_gb() -> Result<u64> {
    println!();
    println!("If your internet plan has a monthly data cap, carrying keeps to half of it,");
    println!("counting what comes in and goes out, and waits for the next month once that's");
    println!("used. Your plan says its cap; many are unlimited. You can change it later");
    println!("(`osp carrier cap <GB>`, or `osp carrier monthly <GB>` for carrying's own).");
    loop {
        print!("Your plan's monthly data cap in GB (Enter for unlimited): ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            println!();
            return Ok(0);
        }
        let line = line.trim().to_lowercase();
        let line = line.trim_end_matches("gb").trim();
        match line {
            "" | "unlimited" | "none" | "0" => return Ok(0),
            _ => match line.parse::<u64>() {
                Ok(gb) if gb > 0 => return Ok(gb),
                _ => println!(
                    "Please give a number of GB, such as 400, or press Enter for unlimited."
                ),
            },
        }
    }
}

/// Asks the question on the node's own address (ADR 0043 §8), with yes
/// recommended. With no answer it stays unasked, and the node's own video
/// stays on the fallback until the owner chooses.
fn ask_hops(choice: Option<bool>) -> Result<Option<bool>> {
    if choice.is_some() {
        return Ok(choice);
    }
    println!();
    for line in HOPS_QUESTION {
        println!("{line}");
    }
    println!();
    println!("Recommended: yes. With no, your node's own video goes through the public");
    println!("fallback relays, as today. You can change it later (`osp hops yes|no`).");
    loop {
        print!("Let the nodes yours picks see its address? [Yes] / no: ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            println!();
            println!("No answer: your node's video stays on the fallback until you choose.");
            return Ok(None);
        }
        match line.trim().to_lowercase().as_str() {
            "" | "yes" | "y" => return Ok(Some(true)),
            "no" | "n" => return Ok(Some(false)),
            _ => println!("Please press Enter for yes, or answer \"no\"."),
        }
    }
}

/// ADR 0043 §8's words for the question.
const HOPS_QUESTION: &[&str] = &[
    "Your node can fetch and send video through other nodes it picks, which is much",
    "faster than the public fallback. The node next to yours sees your internet address",
    "and which node you're fetching from, as the fallback does today, but never what's",
    "inside.",
];

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
    config.lists.hide = tell_hide_list(answers.hide_list);
    ask_carrying(&mut config, &answers)?;
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
        if info.vouches > 0 {
            println!("{}", vouch_line(info));
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
    print_start_line(home);
    Ok(())
}

/// How to start the new node. The installer (`scripts/install.sh`) sets
/// `OSP_INSTALLING` and starts it as a service right after, so someone
/// following the usual line there would start a second copy.
fn print_start_line(home: &Path) {
    if std::env::var_os("OSP_INSTALLING").is_some() {
        println!("The installer starts it next, as a service that starts at boot.");
    } else {
        println!("Start it with: osp --home {} daemon", home.display());
    }
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

/// Who vouched for the owner, and their side (ADR 0029).
fn print_vouches(
    vouchers: &[osp_proto::rpc::VoucherInfo],
    side: &[osp_proto::rpc::SideInfo],
    gathered_secs_ago: Option<u64>,
    gathering: bool,
) {
    if vouchers.is_empty() {
        println!(
            "Nobody has vouched for you yet: nobody's reshare of your posts, and no credit for finding a post early, has reached this node."
        );
    } else {
        println!("Who vouched for you, most recently heard of first:");
    }
    for v in vouchers {
        let name = v.name.as_deref().unwrap_or("(no profile kept)");
        let mut what = Vec::new();
        if v.reshares > 0 {
            what.push(format!("reshared {} of your posts", v.reshares));
        }
        if v.credits > 0 {
            what.push(format!("credited you {} times", v.credits));
        }
        println!("  {name}  {}  {}", v.actor, what.join(", "));
    }
    println!();
    match gathered_secs_ago {
        Some(secs) => {
            let steps = |n: u8| side.iter().filter(|s| s.step == n).count();
            println!(
                "Your side, gathered {} ago: {} people ({} 1 step back, {} 2, {} 3).",
                uptime(secs),
                side.len(),
                steps(1),
                steps(2),
                steps(3)
            );
        }
        None => println!("Your side hasn't been gathered yet."),
    }
    if gathering {
        println!(
            "Gathering it now; over Tor that can take a while. Run `osp vouches` again later."
        );
    } else {
        println!("It's gathered once a day; `osp vouches --gather` gathers it now.");
    }
}

/// How many vouches for its owner a backup holds (ADR 0029).
fn vouch_line(info: &osp_proto::rpc::BackupInfo) -> String {
    format!(
        "Vouches for you (reshares of your posts, credits for finding posts early): {}.",
        info.vouches
    )
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

/// Tells the owner about the project's hide list (ADR 0042 §6), which is on
/// unless `choice` (a flag) turned it off. Nothing is asked: unset, it's on.
fn tell_hide_list(choice: Option<bool>) -> Option<bool> {
    if choice == Some(false) {
        return choice;
    }
    println!();
    println!("Your node uses the project's hide list: a short list of posts and files with");
    println!("the worst material, such as sexual material involving children, or intimate");
    println!("images shared without consent. Your node doesn't show, fetch or pass on what's");
    println!("on it.");
    println!("- Your node fetches it once a day through Tor, so the project doesn't learn who");
    println!("  fetched it. The list is signed, so nobody else can change it.");
    println!("- Your own posts are never hidden. You can show a person's posts anyway");
    println!("  (`osp hide-list except`), and turn the list off at any time");
    println!("  (`osp hide-list off`).");
    choice
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

/// What `osp block` or `osp unblock` names.
async fn block_target(home: &Path, what: BlockCommand) -> Result<BlockTarget> {
    Ok(match what {
        BlockCommand::Account { who } => BlockTarget::Account {
            actor: find_person(home, Some(&who)).await?,
        },
        BlockCommand::Post { post } => match post.parse::<ObjectId>() {
            Ok(id) => BlockTarget::Post { id },
            Err(_) => match post.parse::<OspCode>() {
                Ok(OspCode::Post { id, .. }) => BlockTarget::Post { id },
                _ => bail!("give a post's ID, or an OSP code for a post"),
            },
        },
        BlockCommand::File { hash } => BlockTarget::File { hash },
    })
}

/// Shows a pairing QR code, then waits until a device pairs: a new one, or
/// one paired before pairing again, which the node lists at the new time.
async fn pair(home: &Path, no_qr: bool, json: bool) -> Result<()> {
    let before: Vec<(EndpointId, String)> = devices(home)
        .await?
        .into_iter()
        .map(|d| (d.id, d.paired))
        .collect();
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
            .find(|d| !before.iter().any(|(id, at)| *id == d.id && *at == d.paired))
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

/// Shows the review code, which the reviewer uses later, so no waiting.
async fn pair_for_review(
    home: &Path,
    no_qr: bool,
    days: u32,
    devices: u32,
    json: bool,
) -> Result<()> {
    let resp = send(home, ClientRequest::NewReviewPairing { days, devices }).await?;
    let ClientResponse::Pairing { ticket, .. } = &resp else {
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
        "This review code pairs up to {devices} device{} within {days} day{}, \
         and replaces any review code before it.",
        plural(devices as usize),
        plural(days as usize),
    );
    println!("Whoever has it can read and post as this node: give it only to the reviewer.");
    println!("When the review is over, `osp pair --end-review` unpairs their devices.");
    Ok(())
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
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

/// The paths of "How are we connected?", each in words, nearest first.
fn print_connection(c: &Connection) {
    let name = |who: &PublicKey| {
        c.people
            .iter()
            .find(|p| p.actor == *who)
            .and_then(|p| p.name.clone())
            .unwrap_or_else(|| format!("someone ({})", who.fmt_short()))
    };
    let from = match c.side {
        SideFrom::Replied => "from when they replied to you".to_string(),
        SideFrom::Asked { vouches: 1 } => "their node named 1 person who vouched for them".into(),
        SideFrom::Asked { vouches } => {
            format!("their node named {vouches} people who vouched for them")
        }
        SideFrom::NoAnswer => "their node didn't answer".into(),
        SideFrom::NotAsked => "their node wasn't asked".into(),
    };
    println!("How you're connected to {} ({from}):", name(&c.actor));
    if c.paths.is_empty() {
        println!("Your node hasn't found a connection.");
    }
    for (i, path) in c.paths.iter().enumerate() {
        println!("{}. {}", i + 1, path_words(path, &name));
    }
}

/// One path as sentences: "You follow Alice. Alice reshared Carol."
fn path_words(path: &connected::Path, name: &dyn Fn(&PublicKey) -> String) -> String {
    let mut words = Vec::new();
    let mut at: Option<PublicKey> = None;
    for hop in &path.hops {
        let to = name(&hop.to);
        let by = at.map(|a| name(&a)).unwrap_or_else(|| "You".into());
        words.push(match hop.how {
            How::Follows => format!("You follow {to}."),
            How::Knows => format!("You know {to}."),
            How::Reshared => format!("{by} reshared {to}."),
            How::CreditedEarly => format!("{by} credited {to} with finding their post early."),
            How::Gap { steps: 1 } => format!("{by}'s node says {to} is 1 step from {by}."),
            How::Gap { steps } => format!("{by}'s node says {to} is within {steps} steps of {by}."),
        });
        at = Some(hop.to);
    }
    words.join(" ")
}

/// Someone this node holds a profile of, by name or the start of their
/// identity, or with no one named, the owner.
async fn find_person(home: &Path, who: Option<&str>) -> Result<PublicKey> {
    let Some(who) = who else {
        return match send(home, ClientRequest::Status).await? {
            ClientResponse::Status(status) => Ok(status.identity),
            ClientResponse::Error { message } => bail!(message),
            other => bail!("unexpected reply {other:?}"),
        };
    };
    if let Ok(actor) = who.trim().parse::<PublicKey>() {
        return Ok(actor);
    }
    let people = match send(home, ClientRequest::People { limit: 500 }).await? {
        ClientResponse::People { people } => people,
        ClientResponse::Error { message } => bail!(message),
        other => bail!("unexpected reply {other:?}"),
    };
    let actors = people
        .iter()
        .filter(|p| matches_person(who, &p.name, &p.actor, None))
        .map(|p| p.actor)
        .collect();
    only_one(who, actors, "person your node found", "osp people")
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

fn print_events(events: &[osp_proto::universe::Event]) {
    use osp_proto::universe::EventWhat;
    if events.is_empty() {
        println!("Nothing in the last day.");
        return;
    }
    let whose = |author: &Option<osp_proto::PublicKey>| match author {
        Some(a) => format!("{}'s media", a.fmt_short()),
        None => "media".to_string(),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for e in events {
        let at = match now.saturating_sub(e.last) / 60 {
            0 => "just now".to_string(),
            m if m < 60 => format!("{m} min ago"),
            m => format!("{} h ago", m / 60),
        };
        let times = match e.count {
            1 => String::new(),
            n => format!(" ({n} times)"),
        };
        let what = match &e.what {
            EventWhat::Found { directory, people } => {
                format!("directory {directory} brought {people} new people")
            }
            EventWhat::Reshared { from, author, .. } => {
                format!("{} reshared {}", from.fmt_short(), author.fmt_short())
            }
            EventWhat::Fetched {
                author,
                bytes,
                host,
                kept_by_copy,
                from_author,
                ..
            } => {
                let from = match (host, from_author) {
                    (_, true) => "from the author's node".to_string(),
                    (Some(h), false) => format!("from {}, who hosts it", h.fmt_short()),
                    (None, false) => "from another node".to_string(),
                };
                let kept = match kept_by_copy {
                    true => ", kept available by a copy",
                    false => "",
                };
                format!("fetched {} of {} {from}{kept}", size(*bytes), whose(author))
            }
            EventWhat::Served { author, bytes, .. } => {
                format!("served {} of {}", size(*bytes), whose(author))
            }
            EventWhat::Machine { words, met: true } => format!("machine {words} answered"),
            EventWhat::Machine { words, met: false } => format!("machine {words} faded"),
        };
        println!("{:>5}  {at:>10}  {what}{times}", e.seq);
    }
}

fn print_universe(snapshot: &osp_proto::universe::Snapshot) {
    use osp_proto::universe::{ActivityBand, LineKind, MachineState};
    let name = |place: u32| {
        snapshot
            .people
            .get(place as usize)
            .map_or("?".into(), |p| plain(&p.name))
    };
    println!("People ({}):", snapshot.people.len());
    for p in &snapshot.people {
        let step = match p.step {
            Some(0) => "you".to_string(),
            Some(n) => format!("step {n}"),
            None => "unplaced".to_string(),
        };
        let mut notes = vec![step];
        if p.followed {
            notes.push("following".into());
        }
        if p.known {
            notes.push("known".into());
        }
        if let Some(d) = &p.found_through {
            notes.push(format!("found through {d}"));
        }
        println!("  {} ({})", plain(&p.name), notes.join(", "));
        let last = p.last_post.as_deref().unwrap_or("never");
        println!(
            "    {} posts, {} with media, {} reshares; last {last}",
            p.posts, p.media, p.reshares
        );
    }
    if snapshot.more_people > 0 {
        println!("  and {} more who didn't fit", snapshot.more_people);
    }
    println!("Lines ({}):", snapshot.lines.len());
    for l in &snapshot.lines {
        let what = match l.kind {
            LineKind::Reshared => "reshared",
            LineKind::CreditedEarly => "credited early",
        };
        println!(
            "  {} {what} {} ({}x, last {})",
            name(l.from),
            name(l.to),
            l.count,
            l.last
        );
    }
    println!("Machines ({}):", snapshot.machines.len());
    for m in &snapshot.machines {
        let state = match m.state {
            MachineState::HeardOf => "heard of",
            MachineState::Met => "met",
            MachineState::AnsweredLately => "answered lately",
            MachineState::Faded => "faded",
        };
        let activity = match m.activity {
            ActivityBand::None => "no activity yet",
            ActivityBand::Some => "some activity",
            ActivityBand::ALot => "a lot of activity",
        };
        let directory = if m.directory { ", a directory" } else { "" };
        println!("  {} ({state}{directory}, {activity})", m.words);
    }
    if let Some(n) = snapshot.network {
        println!(
            "Network: hosting {} posts, served {}, found {} people through directories",
            n.posts_hosted,
            size(n.bytes_served),
            n.people_found
        );
    }
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
        // Asked by phones only. Never the token.
        ClientResponse::PhoneRelays { relays } => match relays {
            Some(r) => println!("Phones use the relays {}.", r.urls.join(", ")),
            None => println!("Phones use n0's relays."),
        },
        ClientResponse::Thread {
            policy,
            replies,
            more,
            likes,
            liked,
        } => {
            let you = if liked { ", you among them" } else { "" };
            match likes {
                0 if liked => println!("You like it; its creator's node hasn't counted it yet."),
                0 => {}
                1 => println!("1 person likes it{you}."),
                n => println!("{n} people like it{you}."),
            }
            let who = match policy {
                ReplyPolicy::Off => "nobody",
                ReplyPolicy::Circle => "the creator's circle",
                ReplyPolicy::Network => "the creator's circle, and anyone else the creator lets in",
            };
            println!("Who may reply: {who} ({}).", policy.as_str());
            if replies.is_empty() {
                println!("No replies yet.");
            }
            replies.iter().for_each(print_reply);
            if more && let Some(last) = replies.last() {
                println!();
                println!("More follow: add `--after {}`.", last.seq);
            }
        }
        ClientResponse::Likes { likes } => {
            if likes.is_empty() {
                println!("Nobody has liked it yet.");
            }
            for like in &likes {
                let name = if like.name.is_empty() {
                    "Someone"
                } else {
                    &like.name
                };
                println!("{name}  {}  {}", like.actor.fmt_short(), like.published);
            }
        }
        ClientResponse::Activity {
            entries,
            unread,
            more,
        } => {
            if entries.is_empty() {
                println!("Nothing yet.");
            }
            for entry in &entries {
                print_activity(entry);
            }
            match unread {
                0 => {}
                1 => println!("1 unread."),
                n => println!("{n} unread."),
            }
            if let Some(first) = entries.first().filter(|_| unread > 0) {
                println!(
                    "Mark these read: `osp activity-read --through {}`.",
                    first.seq
                );
            }
            if let Some(last) = entries.last().filter(|_| more) {
                println!("More follow: add `--before {}`.", last.seq);
            }
        }
        ClientResponse::ReplySent { reply } => print_sent_reply(&reply),
        ClientResponse::SentReplies { replies } => {
            if replies.is_empty() {
                println!("You haven't replied to anyone's posts.");
            }
            replies.iter().for_each(print_sent_reply);
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
        ClientResponse::Blocked { posts, files } => {
            println!(
                "Blocked. Your node dropped {posts} post(s) and {files} file(s) it held, and told nobody."
            );
        }
        ClientResponse::Blocks { blocks } => {
            if blocks.is_empty() {
                println!("You haven't blocked anything.");
            }
            for b in blocks {
                let what = match b.target {
                    BlockTarget::Account { actor } => format!(
                        "person {}  {actor}",
                        plain(b.name.as_deref().unwrap_or("(name unknown)"))
                    ),
                    BlockTarget::Post { id } => format!("post {id}"),
                    BlockTarget::File { hash } => format!("file {hash}"),
                };
                println!("{what}\n    since {}", b.since);
            }
        }
        ClientResponse::HideList { info } => {
            match (info.on, info.key) {
                (_, None) => println!(
                    "The project's hide list isn't applied: this build has no key to check it by."
                ),
                (false, Some(_)) => println!(
                    "The project's hide list is off: nothing it holds is hidden here. `osp hide-list on` applies it again."
                ),
                (true, Some(key)) => {
                    println!("The project's hide list is on (key {}).", key.fmt_short());
                    match (info.seq, info.entries, info.published, info.fetched) {
                        (Some(seq), Some(entries), Some(published), Some(fetched)) => println!(
                            "List number {seq}, signed {published}: {entries} entries in force. Last fetched {fetched}."
                        ),
                        _ => println!("No list fetched yet: it comes through Tor, once a day."),
                    }
                }
            }
            if !info.exceptions.is_empty() {
                println!("Not hidden for you, as you chose (`osp hide-list unexcept` to undo):");
                for e in &info.exceptions {
                    println!(
                        "  {}  {}",
                        plain(e.name.as_deref().unwrap_or("(name unknown)")),
                        e.actor
                    );
                }
            }
        }
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
        ClientResponse::Profile { profile } => print_profile(&profile),
        ClientResponse::ProfileItems { items, next } => {
            if items.is_empty() {
                println!("Your node holds none.");
            }
            for item in &items {
                print_item(item);
                println!();
            }
            if let Some(next) = next {
                println!("More follow: add `--before {next}`.");
            }
        }
        ClientResponse::People { people } => {
            if people.is_empty() {
                println!("Your node has found nobody yet.");
            }
            for person in people {
                let followed = if person.followed { " (following)" } else { "" };
                println!("{}{followed}", plain(&person.name));
                if let Some(summary) = person.summary {
                    println!("  {}", plain(&summary));
                }
                println!("  identity: {}", person.actor);
                if let Some(newest) = person.newest {
                    println!("  newest post: {newest}");
                }
                if let Some(directory) = person.found_through {
                    println!("  found through directory {directory}");
                }
                println!();
            }
        }
        ClientResponse::Universe { snapshot } => print_universe(&snapshot),
        ClientResponse::UniverseUnchanged => println!("Unchanged."),
        ClientResponse::UniverseEvents { events } => print_events(&events),
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
        ClientResponse::ReviewEnded { code, unpaired } => {
            let code = if code {
                "The review code no longer works"
            } else {
                "No review code was left"
            };
            println!(
                "{code}; unpaired {unpaired} reviewer device{}.",
                plural(unpaired)
            );
        }
        ClientResponse::Devices { devices } => {
            if devices.is_empty() {
                println!("No paired devices. Pair your phone with `osp pair`.");
            }
            for d in devices {
                let connected = if d.connected { ", connected now" } else { "" };
                let review = if d.review {
                    ", with the review code"
                } else {
                    ""
                };
                println!("{}  {}", d.name.as_deref().unwrap_or("(no name)"), d.id);
                println!("    paired {}{review}{connected}", d.paired);
            }
        }
        ClientResponse::Peers { peers } => print_peers(&peers),
        ClientResponse::Vouches {
            vouchers,
            side,
            gathered_secs_ago,
            gathering,
        } => print_vouches(&vouchers, &side, gathered_secs_ago, gathering),
        ClientResponse::Connected { connection } => print_connection(&connection),
        ClientResponse::ShareCard { card } => {
            println!(
                "{} people discovered, {} direct connections",
                card.people, card.direct
            );
            println!("{} separate paths into the wider network", card.paths);
            println!("{} served peer to peer", size(card.bytes_served));
            let named: Vec<&str> = card
                .points
                .iter()
                .filter_map(|p| p.name.as_deref())
                .collect();
            println!(
                "Draws {} points and {} lines; names {}.",
                card.points.len(),
                card.lines.len(),
                match named.is_empty() {
                    true => "nobody".to_string(),
                    false => named.join(", "),
                }
            );
        }
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
            if info.vouches > 0 {
                println!("{}", vouch_line(&info));
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

/// One line on how carrying goes.
fn carrying_line(c: &osp_proto::rpc::CarrierStatus) -> String {
    let upload = format!(
        "{} at once, {}% of {} Mbit/s{}",
        c.jobs,
        c.upload_share,
        c.upload_mbps,
        match c.upload_from.as_str() {
            "set" => "",
            "cap" => " (your cap on serving)",
            "measured" => " (measured)",
            _ => " (assumed: `osp carrier upload <Mbit/s>`)",
        }
    );
    let upload = match (c.seed, c.bytes_per_sec) {
        (true, None) => format!(
            "{} at once, as a seed carrier, with its whole upload",
            c.jobs
        ),
        _ => upload,
    };
    if let Some(off) = &c.off {
        format!("off: {off}")
    } else if c.paused {
        "paused (`osp carrier resume`)".to_string()
    } else if c.metered {
        "stopped: the connection is metered (`osp carrier metered no`)".to_string()
    } else if c.over_budget {
        format!(
            "waiting for next month: this month's {} is used (`osp carrier monthly <GB>`)",
            size(c.month_budget.unwrap_or(0))
        )
    } else {
        format!(
            "on, {} circuit(s) now, {} since the node started; {upload}",
            c.circuits,
            size(c.bytes)
        )
    }
}

fn hops_line(hops: Option<bool>) -> &'static str {
    match hops {
        Some(true) => {
            "yes: your node's video goes through the hops it picks, which see its address"
        }
        Some(false) => "no: your node's video goes through the public fallback relays",
        None => "not asked yet: your node's video goes through relays (`osp hops yes|no`)",
    }
}

/// `osp carrier`: makes the change, if any, then says how carrying goes.
async fn carrier(home: &Path, change: Option<CarrierCommand>, json: bool) -> Result<()> {
    let request = change.map(|change| {
        let (mut paused, mut metered, mut jobs, mut upload_share, mut upload_mbps) =
            (None, None, None, None, None);
        let (mut monthly_gb, mut data_cap_gb) = (None, None);
        match change {
            CarrierCommand::Pause => paused = Some(true),
            CarrierCommand::Resume => paused = Some(false),
            CarrierCommand::Metered { answer } => metered = Some(matches!(answer, YesNo::Yes)),
            CarrierCommand::Jobs { jobs: n } => jobs = Some(n),
            CarrierCommand::Share { percent } => upload_share = Some(percent),
            CarrierCommand::Upload { mbps } => upload_mbps = Some(mbps),
            CarrierCommand::Cap { gb } => data_cap_gb = Some(gb),
            CarrierCommand::Monthly { gb } => monthly_gb = Some(gb),
        }
        ClientRequest::SetCarrying {
            paused,
            metered,
            jobs,
            upload_share,
            upload_mbps,
            monthly_gb,
            data_cap_gb,
        }
    });
    if let Some(request) = request {
        done(send(home, request).await?)?;
    }
    let ClientResponse::Status(status) = send(home, ClientRequest::Status).await? else {
        bail!("the node didn't answer with its status");
    };
    let Some(c) = status.carrier else {
        bail!("this node's version says nothing about carrying");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&c)?);
        return Ok(());
    }
    println!("Carrying:  {}", carrying_line(&c));
    if c.off.is_none() {
        println!(
            "Month:     {} carried, both ways{}",
            size(c.month_bytes),
            match c.month_budget {
                Some(budget) => format!(", of {}", size(budget)),
                None => ", no limit (`osp carrier cap <GB>` if your plan has one)".to_string(),
            }
        );
        println!(
            "Busy:      {} circuit(s) turned away since the node started",
            c.busy
        );
        println!(
            "Packets:   {} waited for the share ({:.1} s in all), {} dropped with the share's queue full, {} sent to a backed-up link, {} dropped in all",
            c.held,
            c.held_ms as f64 / 1000.0,
            c.share_full,
            c.link_full,
            c.dropped
        );
        println!(
            "Queues:    {} sends found a link's send queue full; at this node's circuit ends, {} packets dropped with the viewer's inbox full, {} with the host's, and {} and {} sends found the queue full",
            c.send_queue_full,
            c.viewer_inbox_full,
            c.host_inbox_full,
            c.viewer_send_queue_full,
            c.host_send_queue_full
        );
        println!(
            "Standing:  {} machine(s) earned it with your node, {} on probation",
            c.standing, c.probation
        );
    }
    match c.seeds {
        0 => println!("Seeds:     none known, so your node's own video goes through relays"),
        n => {
            println!("Seeds:     {n} seed carrier(s) known");
            println!("Hops:      {}", hops_line(c.hops));
        }
    }
    Ok(())
}

/// Ok for `Ok`, the node's message for an error.
fn done(response: ClientResponse) -> Result<()> {
    match response {
        ClientResponse::Ok => Ok(()),
        ClientResponse::Error { message } => bail!(message),
        other => bail!("unexpected answer: {other:?}"),
    }
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
        let dialled = match (tor.ready, tor.dialled) {
            (true, false) => ", reached no node yet",
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
        println!(
            "Tor:       {}{published}{dialled}, {streams}{restarts}",
            tor.state
        );
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
    match (s.found_people, s.directories_answered_secs_ago) {
        (0, None) => {}
        (0, Some(ago)) => println!(
            "Found:     nobody yet; a directory answered {} ago",
            uptime(ago)
        ),
        (n, _) => println!("Found:     {n} people through directories"),
    }
    if s.shared.posts > 0 {
        println!(
            "Shared:    {} post(s) with hosting, {} served to others (counted here only)",
            s.shared.posts,
            size(s.shared.served_bytes)
        );
    }
    println!(
        "Served:    {} to other nodes in the last 30 days; {} used in all",
        size(s.served_lately_bytes),
        size(s.space_bytes)
    );
    if let Some(c) = &s.carrier {
        println!("Carrying:  {}", carrying_line(c));
        if c.seeds > 0 {
            println!("Hops:      {}", hops_line(c.hops));
        }
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
    match s.reply_requests {
        0 => {}
        1 => println!("Replies:   a reply waiting for you (`osp reply-requests`)"),
        n => println!("Replies:   {n} replies waiting for you (`osp reply-requests`)"),
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

/// One of your replies, and where it stands.
/// "Jimmy", "Jimmy and Alice", "Jimmy, Alice and 3 others".
fn names(people: &[ActivityPerson], count: u64) -> String {
    let name = |p: &ActivityPerson| {
        if p.name.is_empty() {
            "Someone".to_string()
        } else {
            p.name.clone()
        }
    };
    let named: Vec<String> = people.iter().map(name).collect();
    let others = count.saturating_sub(named.len() as u64);
    match (named.as_slice(), others) {
        ([], n) => format!("{n} people"),
        ([one], 0) => one.clone(),
        ([a, b], 0) => format!("{a} and {b}"),
        (some, 1) => format!("{} and 1 other", some.join(", ")),
        (some, n) => format!("{} and {n} others", some.join(", ")),
    }
}

fn print_activity(entry: &ActivityEntry) {
    let dot = if entry.read { " " } else { "•" };
    let post = |text: &str| {
        let line = plain(text.lines().next().unwrap_or(""));
        if line.is_empty() {
            "your post".to_string()
        } else {
            format!("your post “{line}”")
        }
    };
    let said = |text: &str| plain(text.lines().next().unwrap_or(""));
    let who = |name: &str| {
        if name.is_empty() {
            "Someone".to_string()
        } else {
            name.to_string()
        }
    };
    let line = match &entry.what {
        ActivityWhat::Reply {
            post_text,
            name,
            content,
            waiting: false,
            ..
        } => format!(
            "{} replied to {}: {}",
            who(name),
            post(post_text),
            said(content)
        ),
        ActivityWhat::Reply {
            post_text,
            name,
            content,
            waiting: true,
            ..
        } => format!(
            "{}'s reply to {} is waiting for you: {}",
            who(name),
            post(post_text),
            said(content)
        ),
        ActivityWhat::Liked {
            post_text,
            people,
            count,
            ..
        } => format!("{} liked {}", names(people, *count), post(post_text)),
        ActivityWhat::Reshared {
            post_text,
            people,
            count,
            ..
        } => format!("{} reshared {}", names(people, *count), post(post_text)),
        ActivityWhat::Followed { person: None, .. } => "Someone new follows you".to_string(),
        ActivityWhat::Followed {
            person: Some(p), ..
        } => format!("{} follows you", who(&p.name)),
        ActivityWhat::KnowsYou { person, note, .. } if note.is_empty() => {
            format!("{} says they know you", who(&person.name))
        }
        ActivityWhat::KnowsYou { person, note, .. } => {
            format!("{} says they know you: {}", who(&person.name), said(note))
        }
        ActivityWhat::ReplyPublished {
            creator_name,
            content,
            ..
        } => {
            let whose = match creator_name.as_deref() {
                Some(n) if !n.is_empty() => format!("{n}'s"),
                _ => "their".to_string(),
            };
            format!("Your reply is in {whose} thread: {}", said(content))
        }
    };
    println!("{dot} {:>5}  {line}", entry.seq);
}

fn print_sent_reply(r: &SentReplyInfo) {
    let status = match r.status {
        SentStatus::Sending => "not answered yet; your node tries again",
        SentStatus::Published => "published",
        SentStatus::AwaitingApproval => "waiting for the creator to look",
        SentStatus::NotPublished => "not published",
        SentStatus::Closed => "the post takes no replies",
        SentStatus::Deleted => "the post was deleted",
        SentStatus::Refused => "refused",
        SentStatus::GaveUp => "their node didn't answer in 30 days",
        SentStatus::Deleting => "deleted; telling their node",
    };
    println!();
    println!(
        "To {}'s post {} · {}",
        r.creator.fmt_short(),
        r.post.fmt_short(),
        plain(&r.published)
    );
    for line in r.content.lines() {
        println!("  {}", plain(line));
    }
    match (&r.error, r.next_try_secs) {
        (Some(error), _) if r.status == SentStatus::Refused => {
            println!("  {status}: {}", plain(error))
        }
        (_, Some(secs)) => println!("  {status}, in {}", uptime(secs)),
        _ => println!("  {status}"),
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

fn print_profile(p: &ProfileInfo) {
    let person = &p.person;
    let mut marks = Vec::new();
    if p.followed {
        marks.push("following");
    }
    if p.known {
        marks.push("you know them");
    }
    match marks.is_empty() {
        true => println!("{}", plain(&person.name)),
        false => println!("{} ({})", plain(&person.name), marks.join(", ")),
    }
    if let Some(summary) = &person.summary {
        println!("  {}", plain(summary));
    }
    for link in &person.attachment {
        println!("  {}: {}", plain(&link.name), plain(&link.value));
    }
    println!("  identity: {}", person.actor);
    if let Some(icon) = &person.icon {
        let held = if p.icon_local {
            ""
        } else {
            ", not fetched yet"
        };
        println!("  picture: {} ({} bytes{held})", icon.blob, icon.size);
    }
    if let (Some(followers), Some(following)) = (p.followers, p.following) {
        println!("  {followers} following you, you follow {following}");
    }
    println!(
        "  your node holds {} posts ({} with media) and {} reshares of theirs",
        p.posts, p.media, p.reshares
    );
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
    // Unless this is that reshare, said just above.
    if let Some(mine) = item.reshared
        && item
            .reshared_by
            .as_ref()
            .is_none_or(|by| by.announce != mine)
    {
        let _ = writeln!(out, "  you reshared it ({})", mine.fmt_short());
    }
    match item.resharers {
        Some(1) => out.push_str("  reshared by 1 person\n"),
        Some(n) if n > 1 => {
            let _ = writeln!(out, "  reshared by {n} people");
        }
        _ => {}
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
    fn a_path_reads_as_sentences() {
        let keys: Vec<PublicKey> = (0..4)
            .map(|_| osp_proto::SecretKey::generate().public())
            .collect();
        let names = ["Alice", "Carol", "Dana", "Jimmy"];
        let name = |who: &PublicKey| {
            let i = keys.iter().position(|k| k == who).unwrap();
            names[i].to_string()
        };
        let hop = |i: usize, how| connected::Hop { to: keys[i], how };
        let path = connected::Path {
            hops: vec![
                hop(0, How::Follows),
                hop(1, How::Reshared),
                hop(3, How::CreditedEarly),
            ],
        };
        assert_eq!(
            path_words(&path, &name),
            "You follow Alice. Alice reshared Carol. \
             Carol credited Jimmy with finding their post early."
        );
        let gap = connected::Path {
            hops: vec![
                hop(2, How::Knows),
                hop(1, How::Gap { steps: 2 }),
                hop(3, How::Reshared),
            ],
        };
        assert_eq!(
            path_words(&gap, &name),
            "You know Dana. Dana's node says Carol is within 2 steps of Dana. Carol reshared Jimmy."
        );
    }

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
    fn init_takes_seed_carriers_by_machine_name() {
        let id = || osp_proto::SecretKey::generate().public().to_string();
        let (m1, m2) = (id(), id());
        let cli = Cli::try_parse_from(["osp", "init", "--carrier", &m1, "--carrier", &m2]).unwrap();
        let Command::Init { carrier, .. } = cli.command else {
            panic!("not init");
        };
        let shown: Vec<String> = carrier.iter().map(|c| c.to_string()).collect();
        assert_eq!(shown, [m1.clone(), m2]);
        // With Tor off there's no machine name to carry under.
        assert!(Cli::try_parse_from(["osp", "init", "--no-tor", "--carrier", &m1]).is_err());
    }

    #[test]
    fn init_takes_the_upload_and_the_hops_answer() {
        let cli = Cli::try_parse_from(["osp", "init", "--upload-mbps", "20", "--no-hops"]).unwrap();
        let Command::Init {
            upload_mbps,
            hops,
            no_hops,
            ..
        } = cli.command
        else {
            panic!("not init");
        };
        assert_eq!(upload_mbps, Some(20));
        assert_eq!(answered(hops, no_hops), Some(false));
        assert!(Cli::try_parse_from(["osp", "init", "--hops", "--no-hops"]).is_err());
        // Answered by flags, nothing is asked.
        let mut config = init_config(None, &["default".into()], None, true, false, true);
        config.carrier.known = vec![osp_node::SeedCarrier {
            machine: osp_proto::SecretKey::generate().public(),
        }];
        let answers = Answers {
            share: Some(true),
            list_me: Some(false),
            hide_list: Some(true),
            upload_mbps: Some(20),
            data_cap_gb: Some(400),
            hops: Some(true),
            ask_name: false,
        };
        ask_carrying(&mut config, &answers).unwrap();
        assert_eq!(config.serving.upload_mbps, 20);
        assert_eq!(config.serving.data_cap_gb, 400);
        assert_eq!(config.carrier.hops, Some(true));
    }

    #[test]
    fn carrier_and_hops_commands_parse() {
        for args in [
            &["osp", "carrier"][..],
            &["osp", "carrier", "pause"],
            &["osp", "carrier", "resume"],
            &["osp", "carrier", "metered", "yes"],
            &["osp", "carrier", "jobs", "3"],
            &["osp", "carrier", "share", "40"],
            &["osp", "carrier", "upload", "20"],
            &["osp", "carrier", "cap", "400"],
            &["osp", "carrier", "monthly", "50"],
            &["osp", "hops", "no"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok(), "{args:?}");
        }
        assert!(Cli::try_parse_from(["osp", "hops", "maybe"]).is_err());
    }

    /// What install.sh passes for the project's relays (ADR 0044 step 4).
    #[test]
    fn init_writes_own_relays_token_and_fallback() {
        let config = |args: &[&str]| -> Result<Config> {
            let cli = Cli::try_parse_from(["osp", "init"].iter().chain(args)).unwrap();
            let Command::Init {
                relay,
                relay_token,
                relay_fallback,
                ..
            } = cli.command
            else {
                panic!("not init");
            };
            let mut config = init_config(None, &relay, None, true, false, true);
            set_relay_token(&mut config, relay_token, &relay_fallback)?;
            Ok(config)
        };
        let (a, b) = ("https://a.example", "https://b.example");
        let ours = config(&[
            "--relay",
            a,
            "--relay",
            b,
            "--relay-token",
            "t0k",
            "--relay-fallback",
            "default",
        ])
        .unwrap();
        assert_eq!(
            ours.network.relay,
            RelayChoice::Custom(vec![a.into(), b.into()])
        );
        assert_eq!(ours.network.relay_token.as_deref(), Some("t0k"));
        assert_eq!(ours.network.relay_fallback, RelayChoice::Default);

        // Without flags: n0's relays, no token, no fallback.
        let plain = config(&[]).unwrap();
        assert_eq!(plain.network.relay, RelayChoice::Default);
        assert_eq!(plain.network.relay_token, None);
        assert_eq!(plain.network.relay_fallback, RelayChoice::Disabled);

        // The token never goes with n0's relays, and an empty one is none.
        assert!(config(&["--relay-token", "t0k"]).is_err());
        assert!(config(&["--relay-fallback", "default"]).is_err());
        assert_eq!(
            config(&["--relay", a, "--relay-token", ""])
                .unwrap()
                .network
                .relay_token,
            None
        );
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
            likes: None,
            liked: false,
            reshared: None,
            opened: false,
            actor_picture: None,
            found_reshares: vec![ResharedBy {
                actor,
                name: Some(format!("Jimmy{escape}")),
                announce: osp_proto::ObjectId::of(b"found reshare"),
                published: "2026-11-02".into(),
                hosting: true,
            }],
            resharers: None,
            regions: vec![],
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
