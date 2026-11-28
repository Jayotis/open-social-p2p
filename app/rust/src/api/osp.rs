//! Everything the Flutter app calls. Each function wraps `osp-client` and
//! turns its wire types into plain structs that flutter_rust_bridge can
//! hand to Dart. Byte counts are `i64` because Dart has no unsigned 64-bit
//! integers.

use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use flutter_rust_bridge::frb;
use osp_client::{
    device_key_from_hex,
    osp_proto::{
        connected,
        lists::{project_key, Listed, SignedList},
        rpc::{
            ActivityEntry, ActivityPerson, ActivityWhat, BlockTarget, ClientRequest,
            ClientResponse, FeedItem, LikeInfo, ProfileInfo, ProfileItemKind, Reached,
            ReplierChoice, ReplyAnswer, ReplyInfo, ReplyStatus, ResharedBy, SentReplyInfo,
            SentStatus, TrustInfo,
        },
        social::ReplyPolicy,
        universe, BlobHash, FollowTicket, MediaKind, ObjectId, OspCode, ProfileLink, PublicKey,
    },
    Client, ClientOptions, NoContact, Pairing, ProfilePicture,
};
use tokio::sync::oneshot;

use crate::{frb_generated::StreamSink, logging};

#[frb(init)]
pub fn init_app() {
    // Not flutter_rust_bridge's default setup: that sends every Rust log
    // line to logcat at trace level and sets RUST_BACKTRACE, so every error
    // would capture a backtrace. Warnings are enough for logcat; the app's
    // own log goes to a file (see `logging.rs`).
    flutter_rust_bridge::setup_log_to_console(log::LevelFilter::Warn);
}

/// The app's connection to its home node.
#[frb(opaque)]
pub struct OspApp {
    client: Arc<Client>,
    dir: PathBuf,
    /// Stops the post being sent, if any (`discard_post`).
    stop_post: Mutex<Option<oneshot::Sender<()>>>,
    /// The project's hide list as this app checked it, if the node applies
    /// one (ADR 0042 §6): set by `hide_list`.
    hide: Mutex<Option<Hider>>,
    /// When `status` last asked the node which relays its phones use.
    relays_checked: Mutex<Option<Instant>>,
}

/// How often `status` asks the node which relays and token its phones use
/// (ADR 0044), so a phone paired before they changed takes the new ones.
const RELAYS_CHECK_EVERY: Duration = Duration::from_secs(60 * 60);

/// What the app hides itself, besides what the node leaves out: posts,
/// files and replies on a list checked against the project's key, unless
/// by the owner or someone they excepted.
struct Hider {
    listed: Listed,
    spared: std::collections::HashSet<PublicKey>,
}

impl Hider {
    fn hides(&self, item: &FeedItem) -> bool {
        if self.spared.contains(&item.actor) {
            return false;
        }
        self.listed.post(&item.id)
            || item.attachments.iter().any(|a| {
                self.listed.file(&a.attachment.blob)
                    || a.attachment
                        .thumbnail
                        .as_ref()
                        .is_some_and(|t| self.listed.file(&t.blob))
            })
    }

    fn hides_reply(&self, reply: &ReplyInfo) -> bool {
        !self.spared.contains(&reply.actor) && self.listed.post(&reply.id)
    }
}

/// The project's hide list on the node (ADR 0042 §6).
pub struct HideListState {
    /// Whether it's on: unless the owner turned it off.
    pub in_use: bool,
    /// Whether the node has a key to check it by.
    pub has_key: bool,
    /// The list applied: its number, when the project signed it (RFC 3339),
    /// its entries in force, and when the node last fetched it.
    pub number: Option<u32>,
    pub signed: Option<String>,
    pub entries: Option<u32>,
    pub fetched: Option<String>,
    /// Whether this app checked the list against the project's key, and
    /// hides what it holds too.
    pub checked: bool,
    /// People whose posts it doesn't hide, first excepted first.
    pub exceptions: Vec<HideExceptionPerson>,
}

pub struct HideExceptionPerson {
    pub identity: String,
    /// Their name, or the short form of their identity.
    pub name: String,
}

/// The home node this phone is paired with.
pub struct NodeInfo {
    /// The node owner's display name.
    pub name: String,
    pub identity: String,
    /// When pairing happened (RFC 3339).
    pub paired: String,
}

/// The node's health, and how this phone reaches it.
pub struct NodeStatus {
    pub name: String,
    pub version: String,
    pub uptime_secs: i64,
    pub follows: u32,
    pub followers: u32,
    pub devices: u32,
    pub disk_free_bytes: Option<i64>,
    pub relay_connected: bool,
    /// Problems the node reports, in plain words. Empty when all is well.
    pub problems: Vec<String>,
    /// `direct`, `relay` or `none`.
    pub path: String,
    pub rtt_ms: Option<u32>,
    /// Requests to be a trusted follower waiting for an answer.
    pub trust_requests: u32,
    /// Replies to the owner's posts waiting for an answer (ADR 0028).
    pub reply_requests: u32,
    /// Activity entries the owner hasn't read (ADR 0038).
    pub activity_unread: u32,
    /// Messages between nodes go through Tor, so they take seconds, and a
    /// first contact up to a few minutes.
    pub over_tor: bool,
    /// Over Tor, it hasn't reached another node yet since Tor started: Tor
    /// may say it's ready a few minutes before that works.
    pub tor_starting: bool,
    /// Posts the node reshared with hosting, and keeps available.
    pub hosted_posts: u32,
    /// Media bytes it sent other nodes over the last 30 days (ADR 0036 §3).
    pub served_lately_bytes: i64,
    /// The space OSP uses on the node.
    pub space_bytes: i64,
    /// Carrying other nodes' encrypted video, and the question on the
    /// node's own address (ADR 0043 §8). None from a node before part 5.
    pub carrier: Option<CarrierState>,
    /// A directory has answered the node since it started, so if it found
    /// nobody, nobody is listed yet: it has looked, not only begun to.
    pub looked_for_people: bool,
}

/// Carrying for other nodes (ADR 0043 §8), as the owner set it and as it
/// goes.
pub struct CarrierState {
    /// Whether it carries now: not paused, not metered, within the month's
    /// budget, and running.
    pub carrying: bool,
    /// Why it doesn't run at all, in plain words, when it doesn't.
    pub off: Option<String>,
    pub paused: bool,
    pub metered: bool,
    /// A seed carrier, a server on the release's list.
    pub seed: bool,
    /// Circuits carrying at once.
    pub jobs: u32,
    /// The share of the upload carrying may use, in percent.
    pub upload_share: u32,
    /// The upload the share is of, in Mbit/s, and where that came from:
    /// `set` (the owner's), `cap`, `measured` or `assumed`.
    pub upload_mbps: f64,
    pub upload_from: String,
    /// Circuits open now; bytes carried since the node started.
    pub circuits: u32,
    pub bytes: i64,
    /// The owner's answer on the hops it picks seeing its address: None if
    /// not asked yet, when its own video goes through relays.
    pub hops: Option<bool>,
    /// Carried this UTC month, both ways, and what it may: None for no
    /// limit.
    pub month_bytes: i64,
    pub month_budget: Option<i64>,
    pub over_budget: bool,
    /// The plan's monthly data cap in GB, 0 for unlimited or not given.
    pub data_cap_gb: u32,
    /// The owner's own monthly budget in GB, if set instead of half the
    /// cap; 0 for no limit.
    pub monthly_gb: Option<u32>,
    /// Seed carriers the node knows: with none, its own video has no hop
    /// to go through, and the question isn't asked.
    pub seeds: u32,
    /// Machines at standing with this node, and on probation.
    pub standing: u32,
    pub probation: u32,
}

/// A request to be a trusted follower, or someone trusted (ADR 0013).
pub struct TrustEntry {
    /// The node asking, or trusted: what `answer_trust` takes.
    pub node: String,
    pub identity: String,
    /// What the person says: their name, and the note with their request
    /// (empty if trusted without one). Neither is checked: show them as
    /// their words, as plain text.
    pub name: String,
    pub note: String,
    /// Seconds since they asked, while waiting; since they were trusted.
    pub secs_ago: i64,
    /// Whether the owner follows this identity, which is the check that
    /// it's who they say: the owner added their follow code.
    pub followed: bool,
    /// Whether this person is trusted on another of their nodes.
    pub trusted_elsewhere: bool,
    /// How long the node has followed the owner's, in seconds; `None` if
    /// it no longer does.
    pub follower_secs: Option<i64>,
    /// `direct` or `relay` if the node reached the owner's in the last few
    /// minutes, `none` otherwise.
    pub reached: String,
    /// How many of the owner's files it holds a copy of.
    pub copies: u32,
}

/// Requests waiting, newest first, and the nodes trusted.
pub struct TrustList {
    pub requests: Vec<TrustEntry>,
    pub trusted: Vec<TrustEntry>,
}

/// Someone the node follows.
pub struct Followed {
    pub identity: String,
    pub name: String,
    /// The node hasn't reached theirs yet.
    pub connecting: bool,
}

/// What a search found on the node, the best match first in each list.
pub struct Found {
    pub people: Vec<FoundPerson>,
    pub posts: Vec<Post>,
}

pub struct FoundPerson {
    pub identity: String,
    pub name: String,
    pub summary: Option<String>,
    /// Whether the node follows them.
    pub followed: bool,
    /// The directory they came from, in words, if they're on the node only
    /// because of one.
    pub found_through: Option<String>,
    /// When their newest post the node holds was published (RFC 3339).
    pub newest: Option<String>,
    /// Search only (ADR 0035 §3): where they are on the map, as a
    /// snapshot names regions, the top (empty) first, then their groups,
    /// the widest first; or one `sky:32/x/y` in the public sky.
    pub regions: Vec<String>,
    /// Their profile picture, for `media_url`, only when the node already
    /// holds it: never fetched for a list.
    pub picture: Option<String>,
}

pub struct Post {
    pub id: String,
    pub author: String,
    /// The author's identity: the owner's own for their posts.
    pub author_identity: String,
    /// The author's profile picture, for `media_url`, only when the node
    /// already holds it: never fetched for a feed.
    pub author_picture: Option<String>,
    /// RFC 3339.
    pub published: String,
    pub text: String,
    pub media: Vec<Media>,
    /// Who reshared it, if this is a reshare.
    pub reshared_by: Option<String>,
    /// Whether the resharer keeps and serves a copy of its media.
    pub resharer_hosts: bool,
    /// The directory it came from, in words, if it's on the node only because
    /// of one.
    pub found_through: Option<String>,
    /// What the owner may delete here (ADR 0019): their reshare, if this is
    /// one, or else their own post. `None` for anyone else's.
    pub deletable: Option<String>,
    /// For one of the owner's own posts, how many people's nodes said they
    /// reshared it (ADR 0029). `None` for anyone else's.
    pub resharers: Option<u32>,
    /// For one of the owner's own posts, how many people like it (ADR
    /// 0038). `None` for anyone else's.
    pub likes: Option<u32>,
    /// Whether the owner likes it.
    pub liked: bool,
    /// The owner's own reshare of it, if they reshared it: what deleting
    /// takes the reshare back.
    pub reshared: Option<String>,
    /// Whether it's an opened post (ADR 0039 §4): opened from an OSP code,
    /// its author not followed, and kept only while the owner looks at it.
    pub opened: bool,
    /// Search only: where its author is on the map, as for a person.
    pub regions: Vec<String>,
}

/// Someone who likes one of the owner's posts (ADR 0038).
pub struct Liker {
    pub identity: String,
    /// From the profile that came with the like. Not checked.
    pub name: String,
    /// When they liked it (RFC 3339).
    pub published: String,
}

/// Someone named in an Activity entry.
pub struct ActivityName {
    pub identity: String,
    /// Their name as they gave it; may be empty.
    pub name: String,
}

/// One entry in the owner's Activity (ADR 0038 §3). The app says it in
/// words; each opens what it's about.
pub struct ActivityItem {
    /// For `activity_read`.
    pub key: String,
    /// Its place: higher for a later change.
    pub seq: i64,
    /// When it last changed, in Unix milliseconds.
    pub at_ms: i64,
    pub read: bool,
    /// `reply`, `liked`, `reshared`, `followed`, `knows_you` or
    /// `reply_published`.
    pub kind: String,
    /// The post it's about, for all but `followed` and `knows_you`.
    pub post: Option<String>,
    /// The start of the owner's post's text (`reply`, `liked`, `reshared`).
    pub post_text: String,
    /// The reply, for `reply` and `reply_published`.
    pub reply: Option<String>,
    /// Who: the replier, the latest likers or resharers, the follower if
    /// their node is known for sure (else none), the person who says they
    /// know the owner, or the creator whose thread has the owner's reply.
    pub people: Vec<ActivityName>,
    /// How many like or reshared it; 1 for the rest.
    pub count: u32,
    /// The reply's text, or the note sent with "says they know you".
    pub text: String,
    /// A reply waiting for the owner to let it in.
    pub waiting: bool,
    /// The node, for `followed` and `knows_you`.
    pub node: Option<String>,
}

/// A page of Activity, and how many entries are unread in all.
pub struct ActivityPage {
    pub entries: Vec<ActivityItem>,
    pub unread: u32,
    /// Another page follows: ask before the last entry's `seq`.
    pub more: bool,
}

/// A reply to a post (ADR 0028).
pub struct Reply {
    pub id: String,
    /// The post it answers.
    pub post: String,
    /// The replier's identity.
    pub identity: String,
    /// The name in the profile that came with it. Not checked.
    pub name: String,
    pub text: String,
    /// The date the replier gave it (RFC 3339).
    pub published: String,
    /// `published`, `hidden` or `request`.
    pub state: String,
    /// For a request, why it's there: "you don't follow them", say.
    pub why: Option<String>,
    /// Its place in the thread, to read on after it.
    pub seq: i64,
}

/// A post's replies, a page at a time.
pub struct Thread {
    /// `off`, `circle` or `network`. Only the owner's own posts say.
    pub policy: String,
    pub replies: Vec<Reply>,
    /// Another page follows: ask after the last reply's `seq`.
    pub more: bool,
    /// How many people like the post, as its creator's node says (ADR 0038).
    pub likes: u32,
    /// Whether the owner likes it, as their node has it.
    pub liked: bool,
}

/// One of the owner's replies to someone else's post, and where it stands.
pub struct SentReply {
    pub id: String,
    /// The post it answers.
    pub post: String,
    /// Whose post that is.
    pub creator: String,
    /// Empty once it's being deleted.
    pub text: String,
    pub published: String,
    /// `sending`, `published`, `awaiting_approval`, `not_published`,
    /// `closed`, `deleted`, `refused`, `gave_up` or `deleting`.
    pub status: String,
    /// Why the creator's node refused it, or why it's still being tried.
    pub error: Option<String>,
    /// While it's being sent: seconds until the next try.
    pub next_try_secs: Option<i64>,
}

pub struct Media {
    pub is_video: bool,
    pub blob: String,
    pub media_type: String,
    pub size: i64,
    pub width: u32,
    pub height: u32,
    pub duration_ms: Option<u32>,
    pub thumbnail_blob: Option<String>,
    /// Whether the home node already has it, so playback can start at once.
    pub on_node: bool,
}

/// A person's profile (ADR 0032), as the node holds it.
pub struct Profile {
    pub identity: String,
    pub name: String,
    pub summary: Option<String>,
    /// Links they give: shown, never checked.
    pub links: Vec<ProfileLinkEntry>,
    /// Their picture, a square WebP image, to show with `media_url`. A
    /// stranger's is fetched only then, without saying who looks.
    pub picture: Option<String>,
    /// Whether it's the owner's own profile.
    pub own: bool,
    pub followed: bool,
    /// Whether they're among the people the owner knows (ADR 0013).
    pub known: bool,
    /// How many of their posts, posts with photos or videos, and reshares
    /// the node holds.
    pub posts: i64,
    pub media: i64,
    pub reshares: i64,
    /// On the owner's own profile only: how many people follow them, and
    /// how many they follow. Nobody else's are known.
    pub followers: Option<i64>,
    pub following: Option<i64>,
}

pub struct ProfileLinkEntry {
    pub name: String,
    /// An `https` URL.
    pub url: String,
}

/// One page of a person's posts, media or reshares.
pub struct ProfilePage {
    pub posts: Vec<Post>,
    /// What to pass as `before` for the next page, if there may be one.
    pub next: Option<String>,
}

/// The feed as last fetched, for when the node can't be reached.
pub struct SavedFeed {
    pub posts: Vec<Post>,
    /// RFC 3339.
    pub saved: String,
}

/// What the Universe may show (ADR 0033): one snapshot from the node, field
/// for field as `osp_proto::universe::Snapshot`.
pub struct UniverseSnapshot {
    /// Ask `since` this; it changes whenever anything else does.
    pub version: String,
    pub level: UniverseLevel,
    /// What it opens, as `universe`'s `region` names it: empty for the top.
    /// A group that's gone is answered by the one most of its people went
    /// to, named here.
    pub region: String,
    /// The regions above it, the top (empty) first: zooming out asks for
    /// the last of them (ADR 0034 §6).
    pub path: Vec<String>,
    /// Groups shown closed, each one light.
    pub groups: Vec<UniverseGroup>,
    /// The owner first, then the one stood at, then the nearest and most
    /// active.
    pub people: Vec<UniversePerson>,
    /// Public acts between people, newest first.
    pub lines: Vec<UniverseLine>,
    /// People the node knows who didn't fit.
    pub more_people: i64,
    pub machines: Vec<UniverseMachine>,
    pub network: Option<NetworkTotals>,
    /// Cells of the public sky, each many people drawn as one light.
    pub sky: Vec<SkyCell>,
}

/// Something the node did or saw (ADR 0036 §1), as
/// `osp_proto::universe::Event`, its kind's fields flattened: those its
/// kind doesn't have are none.
pub struct UniverseEvent {
    /// Rises with each new event and each one summed into: ask `since` the
    /// highest held. One summed into comes again with the same `first`.
    pub seq: u64,
    /// When it first and last happened, in seconds since 1970.
    pub first: u64,
    pub last: u64,
    /// How many times it happened in that while.
    pub count: u32,
    pub kind: EventKind,
    /// `found`: the machine whose list brought them, in words. `machine`:
    /// the machine, in words.
    pub words: Option<String>,
    /// `found`: how many people it brought.
    pub people: u32,
    /// `reshared`: who reshared.
    pub from: Option<String>,
    /// `reshared`, `fetched`, `served`: whose post, when the node can tell.
    pub author: Option<String>,
    pub post: Option<String>,
    /// `fetched`, `served`.
    pub bytes: u64,
    /// `fetched`: the resharer who sent it, only when listed as hosting it.
    pub host: Option<String>,
    /// `fetched`: a copy sent it, as the author's node couldn't be reached.
    pub kept_by_copy: bool,
    /// `fetched`: the author's own node sent most of it.
    pub from_author: bool,
    /// `machine`: it answered (true) or faded.
    pub met: bool,
}

/// The card "Share my Universe" shares (ADR 0039), from the node's own
/// allowlist: rounded totals, and a drawing that names only the people the
/// owner ticked.
pub struct ShareCard {
    pub people: i64,
    pub direct: i64,
    pub paths: i64,
    pub bytes_served: i64,
    /// The owner first, at the centre.
    pub points: Vec<CardPoint>,
    pub lines: Vec<CardLine>,
}

/// One point of a card's drawing, in the Universe's units.
pub struct CardPoint {
    pub x: f32,
    pub y: f32,
    /// 1 for a person, more for a group.
    pub people: u32,
    pub you: bool,
    /// Only for someone the owner ticked.
    pub name: Option<String>,
}

/// A line between two points, by their place in `points`.
pub struct CardLine {
    pub from: u32,
    pub to: u32,
}

/// "How are we connected?" (ADR 0037): the paths the node knows from its
/// owner to `identity`, nearest first. None means the node hasn't found a
/// connection, not that there is none.
pub struct Connection {
    pub identity: String,
    pub paths: Vec<ConnectionPath>,
    /// Everyone a path names, `identity` first.
    pub people: Vec<ConnectedPerson>,
    pub side: SideFrom,
    /// `asked`: how many people their node named who vouched for them.
    pub vouches: u32,
}

/// One path, nearest hop first: the first from the owner.
pub struct ConnectionPath {
    pub hops: Vec<PathHop>,
}

/// One hop, from whoever the hop before reached, to `identity`.
pub struct PathHop {
    pub identity: String,
    pub how: HopKind,
    /// `gap`: how many steps it covers; 1 for the rest.
    pub steps: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HopKind {
    /// Known: the owner follows them.
    Follows,
    /// Known: the owner knows them.
    Knows,
    /// Signed: the one before reshared them.
    Reshared,
    /// Signed: the one before credited them with finding a post early.
    CreditedEarly,
    /// A gap: the node of the one before placed them within `steps` of its
    /// owner. Nobody inside is named.
    Gap,
}

pub struct ConnectedPerson {
    pub identity: String,
    pub name: Option<String>,
    /// Where they are on the map, as search gives it; none if the node
    /// doesn't hold them.
    pub regions: Vec<String>,
}

/// Where the paths through someone's side came from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SideFrom {
    /// From when they replied to the owner.
    Replied,
    /// Their node was asked who vouched for them.
    Asked,
    /// Their node was asked and didn't answer.
    NoAnswer,
    /// Their node wasn't asked.
    NotAsked,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Found,
    Reshared,
    Fetched,
    Served,
    Machine,
}

/// What lights up on the map (ADR 0035 §1). People alone is none.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UniverseLayer {
    /// Where recent public posts are.
    Posts,
    /// Where recent public video is.
    Video,
    /// Where reshares are spreading.
    Shared,
}

/// How close the camera is (ADR 0034's zoom bands).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UniverseLevel {
    Universe,
    Galaxy,
    Cluster,
    Local,
    Close,
}

/// A group of the tree, drawn as one light; opened with `g:` and its ID.
pub struct UniverseGroup {
    pub id: u64,
    pub parent: Option<u64>,
    /// For a group new since the last grouping, the one it split from.
    pub from: Option<u64>,
    /// Everyone in it, at any depth.
    pub people: u32,
    /// 0 to 100.
    pub bright: u8,
    /// With a layer lit: its people's recent items of that kind.
    pub lit: u32,
    pub x: f32,
    pub y: f32,
}

/// Many people of the public sky as one light; opened with
/// `sky:level/x/y`. Not a group: its people aren't related.
pub struct SkyCell {
    pub level: u8,
    pub x: u32,
    pub y: u32,
    pub people: u32,
    /// Those of them with public posts lately.
    pub creators: u32,
    /// With a layer lit: its people's recent items of that kind.
    pub lit: u32,
    /// Its centre.
    pub at_x: f32,
    pub at_y: f32,
}

pub struct UniversePerson {
    pub identity: String,
    pub name: String,
    /// Their profile picture's blob, for `media_url`.
    pub picture: Option<String>,
    /// Steps from the owner: 0 the owner, 1 their circle, 2 whom their
    /// circle reshared, 3 their gathered side; none for anyone else.
    pub step: Option<u8>,
    pub followed: bool,
    /// Among the people the owner knows.
    pub known: bool,
    /// The directory they came from, in words, if only because of one.
    pub found_through: Option<String>,
    pub posts: i64,
    pub media: i64,
    pub reshares: i64,
    /// The day of their newest post or reshare (`YYYY-MM-DD`, UTC).
    pub last_post: Option<String>,
    /// Where they are: placed by their lines, or in the public sky. The
    /// owner is at the centre.
    pub x: f32,
    pub y: f32,
    /// The group they're directly in; none in the sky.
    pub group: Option<u64>,
    /// 0 to 100.
    pub bright: u8,
    /// With a layer lit: their own recent items of that kind.
    pub lit: u32,
}

/// A public act between two people, each by their place in `people`.
pub struct UniverseLine {
    pub kind: LineKind,
    pub from: u32,
    pub to: u32,
    /// How many within 90 days.
    pub count: u32,
    /// The newest's day (`YYYY-MM-DD`, UTC).
    pub last: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// `from` reshared one of `to`'s posts.
    Reshared,
    /// `from`, a post's author, credited `to` with finding it early.
    CreditedEarly,
}

/// A machine, by its name in words: never beside a person.
pub struct UniverseMachine {
    pub words: String,
    pub state: MachineState,
    pub directory: bool,
    pub activity: ActivityBand,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MachineState {
    HeardOf,
    Met,
    AnsweredLately,
    Faded,
}

/// Recent machine-protocol interaction with the node, in bands.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActivityBand {
    None,
    Some,
    ALot,
}

/// The owner's own totals.
pub struct NetworkTotals {
    pub posts_hosted: i64,
    pub bytes_served: i64,
    pub people_found: i64,
}

#[derive(Clone, Copy)]
pub enum PostStage {
    /// Sending files to the node.
    Sending,
    /// Lost contact with the node; trying again.
    Waiting,
    /// The node is transcoding and publishing.
    Processing,
    Done,
    Failed,
}

#[derive(Clone)]
pub struct PostProgress {
    pub stage: PostStage,
    pub sent: i64,
    pub total: i64,
    /// The new post's ID when done, or what went wrong.
    pub message: Option<String>,
    /// When failed: the post is kept on the phone, to try again
    /// (`continue_post`) or discard.
    pub kept: bool,
    /// When failed: it gave up after a long time without contact with the
    /// node.
    pub no_contact: bool,
}

/// A post that didn't finish sending, kept on the phone until the node
/// publishes it or the owner discards it.
pub struct UnsentPost {
    pub text: String,
    pub files: Vec<UnsentFile>,
    /// When it was started (RFC 3339).
    pub started: String,
}

pub struct UnsentFile {
    pub path: String,
    pub size: i64,
}

impl OspApp {
    /// Starts the client. `dir` is the app's private data directory, and
    /// `device_key` the key the app keeps in the phone's secure storage.
    pub async fn open(dir: String, device_key: String) -> Result<OspApp> {
        let dir = PathBuf::from(dir);
        logging::init(&dir);
        tracing::info!("app starting");
        let key = device_key_from_hex(&device_key)?;
        let client = Client::open_with_key(&dir, key, ClientOptions::default()).await?;
        Ok(OspApp {
            client,
            dir,
            stop_post: Mutex::new(None),
            hide: Mutex::new(None),
            relays_checked: Mutex::new(None),
        })
    }

    #[frb(sync)]
    pub fn device_id(&self) -> String {
        self.client.device_id().to_string()
    }

    #[frb(sync)]
    pub fn paired_node(&self) -> Option<NodeInfo> {
        self.client.pairing().map(node_info)
    }

    pub async fn pair(&self, ticket: String, device_name: String) -> Result<NodeInfo> {
        Ok(node_info(self.client.pair(&ticket, &device_name).await?))
    }

    /// Forgets the node on this phone. Unpair it on the node too
    /// (`osp unpair`) to revoke its access.
    pub async fn forget(&self) -> Result<()> {
        self.client.forget().await
    }

    pub async fn status(&self) -> Result<NodeStatus> {
        let ClientResponse::Status(s) = self.client.request(&ClientRequest::Status).await? else {
            bail!("unexpected reply to Status");
        };
        let relays_due = {
            let mut checked = self.relays_checked.lock().unwrap();
            let due = checked.is_none_or(|at| at.elapsed() >= RELAYS_CHECK_EVERY);
            if due {
                *checked = Some(Instant::now());
            }
            due
        };
        if relays_due {
            if let Err(e) = self.client.update_relays().await {
                tracing::warn!("couldn't ask the node which relays its phones use: {e:#}");
            }
        }
        let conn = self.client.connection_info().await;
        Ok(NodeStatus {
            name: s.name,
            version: s.version,
            uptime_secs: s.uptime_secs as i64,
            follows: s.follows as u32,
            followers: s.followers as u32,
            devices: s.devices as u32,
            disk_free_bytes: s.disk_free_bytes.map(|b| b as i64),
            relay_connected: s.relay.is_some(),
            problems: s.problems,
            path: conn
                .as_ref()
                .map(|c| c.path.clone())
                .unwrap_or_else(|| "none".to_string()),
            rtt_ms: conn.and_then(|c| c.rtt_ms).map(|r| r as u32),
            trust_requests: s.trust_requests as u32,
            reply_requests: s.reply_requests as u32,
            activity_unread: s.activity_unread.min(u32::MAX as u64) as u32,
            over_tor: s.tor.is_some(),
            tor_starting: s.tor.as_ref().is_some_and(|t| !t.dialled),
            hosted_posts: s.shared.posts as u32,
            served_lately_bytes: s.served_lately_bytes as i64,
            space_bytes: s.space_bytes as i64,
            carrier: s.carrier.map(|c| carrier_state(*c)),
            looked_for_people: s.directories_answered_secs_ago.is_some(),
        })
    }

    /// `items` as posts, less those the hide list holds.
    fn shown(&self, items: Vec<FeedItem>) -> Vec<Post> {
        let hide = self.hide.lock().unwrap();
        items
            .into_iter()
            .filter(|i| !hide.as_ref().is_some_and(|h| h.hides(i)))
            .map(post)
            .collect()
    }

    /// One post, unless the hide list holds it.
    fn one(&self, item: FeedItem) -> Result<Post> {
        if self
            .hide
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|h| h.hides(&item))
        {
            bail!("the project's hide list holds this post");
        }
        Ok(post(item))
    }

    pub async fn feed(&self, limit: u32) -> Result<Vec<Post>> {
        Ok(self.shown(self.client.feed(limit).await?))
    }

    /// Looks through the posts and profiles the node has received. Nothing
    /// leaves the node.
    pub async fn search(&self, words: String, limit: u32) -> Result<Found> {
        let (people, posts) = self.client.search(&words, limit).await?;
        Ok(Found {
            people: people.into_iter().map(found_person).collect(),
            posts: self.shown(posts),
        })
    }

    /// What a tap on a group or a sky cell opens (ADR 0035 §2): its people,
    /// newest post first, and their posts, newest first. `region` is `g:`
    /// or `sky:`. Nothing leaves the node.
    pub async fn region_items(&self, region: String, limit: u32) -> Result<Found> {
        let (people, posts) = self.client.region_items(&region, limit).await?;
        Ok(Found {
            people: people.into_iter().map(found_person).collect(),
            posts: self.shown(posts),
        })
    }

    /// Everyone the node holds a profile of: those it follows and those
    /// directories brought, newest post first. Nothing leaves the node.
    pub async fn people(&self, limit: u32) -> Result<Vec<FoundPerson>> {
        let people = self.client.people(limit).await?;
        Ok(people.into_iter().map(found_person).collect())
    }

    /// What the Universe may show (ADR 0033), built on the node from what
    /// it already knows: asking sends nothing to any other node. `region`
    /// is what it opens (ADR 0034 §6): none or empty for the top, `g:<id>`
    /// a group, `p:<identity>` standing at someone, `sky:<l>/<x>/<y>` a
    /// cell of the sky. `lit` lights up where that kind of item is (ADR
    /// 0035 §1); none shows people alone. With `since` the version the app
    /// holds, none means it's unchanged.
    pub async fn universe(
        &self,
        region: Option<String>,
        lit: Option<UniverseLayer>,
        since: Option<String>,
    ) -> Result<Option<UniverseSnapshot>> {
        let lit = lit.map(|l| match l {
            UniverseLayer::Posts => universe::Layer::Posts,
            UniverseLayer::Video => universe::Layer::Video,
            UniverseLayer::Shared => universe::Layer::Shared,
        });
        Ok(self
            .client
            .universe(region, lit, since)
            .await?
            .map(universe))
    }

    /// What the node did or saw lately (ADR 0036 §1), oldest first: those
    /// after `since`, the highest `seq` held, or the last day's with none.
    /// Answered by the node alone.
    pub async fn universe_events(&self, since: Option<u64>) -> Result<Vec<UniverseEvent>> {
        let events = self.client.universe_events(since).await?;
        Ok(events.into_iter().map(universe_event).collect())
    }

    /// The card "Share my Universe" shares (ADR 0039), naming only the
    /// people in `named`, whom the owner ticked, and only if it draws them.
    pub async fn share_card(&self, named: Vec<String>) -> Result<ShareCard> {
        let named = named
            .iter()
            .map(|n| n.parse())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let c = self.client.share_card(named).await?;
        let count = |n: u64| n.min(i64::MAX as u64) as i64;
        Ok(ShareCard {
            people: count(c.people),
            direct: count(c.direct),
            paths: count(c.paths),
            bytes_served: count(c.bytes_served),
            points: c
                .points
                .into_iter()
                .map(|p| CardPoint {
                    x: p.at[0],
                    y: p.at[1],
                    people: p.people,
                    you: p.you,
                    name: p.name,
                })
                .collect(),
            lines: c
                .lines
                .into_iter()
                .map(|[from, to]| CardLine { from, to })
                .collect(),
        })
    }

    /// "How are we connected?" (ADR 0037), for the owner's own tap only:
    /// the node may ask `identity`'s node once who vouched for them, from a
    /// throwaway key, and can take most of a minute.
    pub async fn how_connected(&self, identity: String) -> Result<Connection> {
        let c = self.client.how_connected(identity.parse()?).await?;
        let (side, vouches) = match c.side {
            connected::SideFrom::Replied => (SideFrom::Replied, 0),
            connected::SideFrom::Asked { vouches } => (SideFrom::Asked, vouches),
            connected::SideFrom::NoAnswer => (SideFrom::NoAnswer, 0),
            connected::SideFrom::NotAsked => (SideFrom::NotAsked, 0),
        };
        Ok(Connection {
            identity: c.actor.to_string(),
            paths: c
                .paths
                .into_iter()
                .map(|p| ConnectionPath {
                    hops: p.hops.into_iter().map(path_hop).collect(),
                })
                .collect(),
            people: c
                .people
                .into_iter()
                .map(|p| ConnectedPerson {
                    identity: p.actor.to_string(),
                    name: p.name,
                    regions: p.regions,
                })
                .collect(),
            side,
            vouches,
        })
    }

    /// The last top snapshot fetched, to show before the node answers.
    #[frb(sync)]
    pub fn saved_universe(&self) -> Option<UniverseSnapshot> {
        self.client.cached_universe().map(universe)
    }

    /// One person's profile, the owner's own too, and what the node holds
    /// of them. Nothing leaves the node.
    pub async fn profile(&self, identity: String) -> Result<Profile> {
        let info = self.client.profile(identity.parse()?).await?;
        Ok(self.profile_of(info))
    }

    /// One page of someone's `posts`, `media` or `reshares` that the node
    /// holds, newest first: those before `before`, or the newest with none.
    pub async fn profile_items(
        &self,
        identity: String,
        kind: String,
        before: Option<String>,
        limit: u32,
    ) -> Result<ProfilePage> {
        let kind = match kind.as_str() {
            "posts" => ProfileItemKind::Posts,
            "media" => ProfileItemKind::Media,
            "reshares" => ProfileItemKind::Reshares,
            _ => bail!("no such list: {kind:?}"),
        };
        let before = before.map(|b| b.parse()).transpose()?;
        let (items, next) = self
            .client
            .profile_items(identity.parse()?, kind, before, limit)
            .await?;
        Ok(ProfilePage {
            posts: self.shown(items),
            next: next.map(|n| n.to_string()),
        })
    }

    /// Publishes the owner's profile. `photo`, a file on this phone shrunk
    /// to about 1,024 px, becomes the new picture; without one,
    /// `remove_picture` takes the picture off, or else it stays. The picture
    /// is public, as the name is.
    pub async fn set_profile(
        &self,
        name: String,
        summary: Option<String>,
        links: Vec<ProfileLinkEntry>,
        photo: Option<String>,
        remove_picture: bool,
    ) -> Result<Profile> {
        let picture = match (photo, remove_picture) {
            (Some(path), _) => ProfilePicture::Photo(PathBuf::from(path)),
            (None, true) => ProfilePicture::Remove,
            (None, false) => ProfilePicture::Keep,
        };
        let links = links
            .into_iter()
            .map(|l| ProfileLink {
                name: l.name,
                value: l.url,
            })
            .collect();
        let summary = summary.filter(|s| !s.trim().is_empty());
        let info = self
            .client
            .set_profile(name.trim(), summary.as_deref(), links, picture)
            .await?;
        Ok(self.profile_of(info))
    }

    #[frb(sync)]
    pub fn saved_feed(&self) -> Option<SavedFeed> {
        self.client.cached_feed().map(|f| SavedFeed {
            posts: self.shown(f.items),
            saved: f.saved,
        })
    }

    /// A URL on this phone that serves a blob from the home node.
    pub async fn media_url(&self, blob: String, media_type: String) -> Result<String> {
        let blob: BlobHash = blob.parse()?;
        self.client.media_url(&blob, &media_type).await
    }

    /// Waits until the node has the start of a blob, asking it to fetch
    /// the blob if it hasn't, so a player can start on it. The rest
    /// reaches the player as it arrives.
    pub async fn wait_for_media(&self, blob: String) -> Result<()> {
        let blob: BlobHash = blob.parse()?;
        self.client.wait_for_media(&blob).await
    }

    /// Posts text with photos or videos from this phone, reporting progress.
    /// It carries on after short breaks in the connection; if it fails, the
    /// post is usually kept (`pending_post`) to try again.
    pub async fn post(
        &self,
        text: String,
        files: Vec<String>,
        sink: StreamSink<PostProgress>,
    ) -> Result<()> {
        let files: Vec<PathBuf> = files.into_iter().map(PathBuf::from).collect();
        let sending = self.client.post_with(text, &files, report_to(sink.clone()));
        self.run_post(sending, sink).await;
        Ok(())
    }

    /// The post that didn't finish sending, if any: one that failed, or was
    /// stopped when the app was.
    #[frb(sync)]
    pub fn pending_post(&self) -> Option<UnsentPost> {
        self.client.pending_post().map(|p| UnsentPost {
            text: p.content,
            files: p
                .files
                .into_iter()
                .map(|f| UnsentFile {
                    path: f.path.to_string_lossy().into_owned(),
                    size: f.size as i64,
                })
                .collect(),
            started: p.started,
        })
    }

    /// Carries on sending the pending post where it stopped.
    pub async fn continue_post(&self, sink: StreamSink<PostProgress>) -> Result<()> {
        let sending = self.client.continue_post(report_to(sink.clone()));
        self.run_post(sending, sink).await;
        Ok(())
    }

    /// Stops the post being sent, if any, and discards the pending post on
    /// the phone and the node. Fails with the node's reason if it's too
    /// late: the node is preparing the post, or published it.
    pub async fn discard_post(&self) -> Result<()> {
        let stop = self
            .stop_post
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(stop) = stop {
            let _ = stop.send(());
        }
        self.client.discard_post().await
    }

    /// Deletes one of the owner's posts or reshares, by `Post::deletable`
    /// (ADR 0019). It can't be undone: the app waits before calling this.
    pub async fn delete_post(&self, id: String) -> Result<()> {
        let id: ObjectId = id.parse()?;
        self.client.request(&ClientRequest::Delete { id }).await?;
        Ok(())
    }

    /// Reshares a post; with `host`, the node also keeps and serves its media.
    pub async fn reshare(&self, post_id: String, host: bool) -> Result<()> {
        let id: ObjectId = post_id.parse()?;
        self.client
            .request(&ClientRequest::Reshare { id, host })
            .await?;
        Ok(())
    }

    /// Follows someone by their follow ticket (`OSP1...`).
    pub async fn follow(&self, ticket: String) -> Result<()> {
        self.client
            .request(&ClientRequest::Follow { ticket })
            .await?;
        Ok(())
    }

    /// Follows someone a search found, by their identity. The node asks their
    /// own node before anything of theirs reaches the feed.
    pub async fn follow_person(&self, identity: String) -> Result<()> {
        let actor = identity.parse()?;
        self.client
            .request(&ClientRequest::FollowPerson { actor })
            .await?;
        Ok(())
    }

    /// Stops following someone, whether or not the node has reached theirs.
    pub async fn unfollow(&self, identity: String) -> Result<()> {
        let actor = identity.parse()?;
        self.client
            .request(&ClientRequest::Unfollow { actor })
            .await?;
        Ok(())
    }

    /// Blocks a person (`account`, by identity), a post (`post`, by ID) or
    /// a file (`file`, by hash) on the node (ADR 0042 §5): it drops what it
    /// holds of it and won't fetch, serve or show it. Nobody is told.
    pub async fn block(&self, kind: String, id: String) -> Result<()> {
        let target = block_target(&kind, &id)?;
        self.client
            .request(&ClientRequest::Block { target })
            .await?;
        Ok(())
    }

    /// Undoes a block.
    pub async fn unblock(&self, kind: String, id: String) -> Result<()> {
        let target = block_target(&kind, &id)?;
        self.client
            .request(&ClientRequest::Unblock { target })
            .await?;
        Ok(())
    }

    /// What the owner blocked, newest first.
    pub async fn blocks(&self) -> Result<Vec<BlockedThing>> {
        let ClientResponse::Blocks { blocks } = self.client.request(&ClientRequest::Blocks).await?
        else {
            bail!("unexpected reply to Blocks");
        };
        Ok(blocks
            .into_iter()
            .map(|b| {
                let (kind, id) = match b.target {
                    BlockTarget::Account { actor } => ("account", actor.to_string()),
                    BlockTarget::Post { id } => ("post", id.to_string()),
                    BlockTarget::File { hash } => ("file", hash.to_string()),
                };
                BlockedThing {
                    kind: kind.into(),
                    id,
                    name: b.name,
                    since: b.since,
                }
            })
            .collect())
    }

    /// The project's hide list on the node (ADR 0042 §6). Checks the list
    /// the node applies against the project's key, built into the app, and
    /// from then on hides what it holds too.
    pub async fn hide_list(&self) -> Result<HideListState> {
        let ClientResponse::HideList { info } =
            self.client.request(&ClientRequest::HideList).await?
        else {
            bail!("unexpected reply to HideList");
        };
        let listed = match (&info.signed, info.on) {
            (Some(signed), true) => match SignedList::from_json(signed.as_bytes())
                .and_then(|s| s.verify(&project_key()))
            {
                Ok(list) => Some(Listed::new(&list)),
                Err(e) => {
                    tracing::warn!("the node's hide list didn't check out here: {e:#}");
                    None
                }
            },
            _ => None,
        };
        let checked = listed.is_some();
        let mut spared: std::collections::HashSet<PublicKey> =
            info.exceptions.iter().map(|e| e.actor).collect();
        if let Some(pairing) = self.client.pairing() {
            spared.insert(pairing.identity);
        }
        *self.hide.lock().unwrap() = listed.map(|listed| Hider { listed, spared });
        Ok(HideListState {
            in_use: info.on,
            has_key: info.key.is_some(),
            number: info.seq.map(|n| n.min(u32::MAX as u64) as u32),
            signed: info.published,
            entries: info.entries.map(|n| n.min(u32::MAX as u64) as u32),
            fetched: info.fetched,
            checked,
            exceptions: info
                .exceptions
                .into_iter()
                .map(|e| HideExceptionPerson {
                    identity: e.actor.to_string(),
                    name: e.name.unwrap_or_else(|| e.actor.fmt_short().to_string()),
                })
                .collect(),
        })
    }

    /// Answers whether to use the list, or changes the answer. Off, all it
    /// hid comes back; on, the node fetches it soon.
    pub async fn set_hide_list(&self, in_use: bool) -> Result<HideListState> {
        self.client
            .request(&ClientRequest::SetHideList { on: in_use })
            .await?;
        self.hide_list().await
    }

    /// Stops the list hiding `identity`'s posts, or with `except` false
    /// takes that back. Their listed files may then be fetched for the
    /// owner, but the node never passes them on.
    pub async fn hide_exception(&self, identity: String, except: bool) -> Result<HideListState> {
        let actor = identity.parse()?;
        self.client
            .request(&ClientRequest::HideException { actor, except })
            .await?;
        self.hide_list().await
    }

    /// Changes how the node carries for others (ADR 0043 §8). Each one
    /// left out stays as it is. `monthly_gb` and `data_cap_gb` take 0 for
    /// no limit.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_carrying(
        &self,
        paused: Option<bool>,
        metered: Option<bool>,
        jobs: Option<u32>,
        upload_share: Option<u32>,
        upload_mbps: Option<u32>,
        monthly_gb: Option<u32>,
        data_cap_gb: Option<u32>,
    ) -> Result<()> {
        let upload_share = upload_share
            .map(|p| {
                u8::try_from(p).map_err(|_| anyhow::anyhow!("the share goes up to 100 percent"))
            })
            .transpose()?;
        self.client
            .request(&ClientRequest::SetCarrying {
                paused,
                metered,
                jobs: jobs.map(|j| j as usize),
                upload_share,
                upload_mbps: upload_mbps.map(u64::from),
                monthly_gb: monthly_gb.map(u64::from),
                data_cap_gb: data_cap_gb.map(u64::from),
            })
            .await?;
        Ok(())
    }

    /// The owner's answer on whether the hops the node picks may see its
    /// internet address: yes, its own video goes through them; no, through
    /// relays.
    pub async fn set_hops(&self, yes: bool) -> Result<()> {
        self.client.request(&ClientRequest::SetHops { yes }).await?;
        Ok(())
    }

    /// Who the node follows, by name.
    pub async fn following(&self) -> Result<Vec<Followed>> {
        let ClientResponse::Following { follows } =
            self.client.request(&ClientRequest::Following).await?
        else {
            bail!("unexpected reply to Following");
        };
        let mut followed: Vec<Followed> = follows
            .into_iter()
            .map(|f| Followed {
                identity: f.actor.to_string(),
                name: f.name.unwrap_or_else(|| f.actor.fmt_short().to_string()),
                connecting: f.last_sync.is_none(),
            })
            .collect();
        followed.sort_by_key(|f| f.name.to_lowercase());
        Ok(followed)
    }

    /// Replies to `post_id` as the owner (ADR 0028). The node keeps it and
    /// sends it to the post's creator, trying again for 30 days. A reshare's
    /// ID answers the post it shares.
    pub async fn reply(&self, post_id: String, text: String) -> Result<SentReply> {
        let post: ObjectId = post_id.parse()?;
        let ClientResponse::ReplySent { reply } = self
            .client
            .request(&ClientRequest::Reply {
                post,
                content: text,
            })
            .await?
        else {
            bail!("unexpected reply to Reply");
        };
        Ok(sent_reply(reply))
    }

    /// One page of a post's replies, after `after` (0 for the first). For the
    /// owner's own posts, all of them, hidden ones too, and the policy.
    /// Anyone else's are read from their node without saying who reads.
    pub async fn thread(&self, post_id: String, after: i64) -> Result<Thread> {
        let post: ObjectId = post_id.parse()?;
        let ClientResponse::Thread {
            policy,
            replies,
            more,
            likes,
            liked,
        } = self
            .client
            .request(&ClientRequest::Thread {
                post,
                after: after.max(0) as u64,
            })
            .await?
        else {
            bail!("unexpected reply to Thread");
        };
        Ok(Thread {
            policy: policy.as_str().to_string(),
            replies: {
                let hide = self.hide.lock().unwrap();
                replies
                    .into_iter()
                    .filter(|r| !hide.as_ref().is_some_and(|h| h.hides_reply(r)))
                    .map(reply_info)
                    .collect()
            },
            more,
            likes: likes.min(u32::MAX as u64) as u32,
            liked,
        })
    }

    /// One post, or the post a reshare shares, as the node has it.
    pub async fn show_post(&self, id: String) -> Result<Post> {
        let id = id.parse()?;
        let ClientResponse::Item { item } =
            self.client.request(&ClientRequest::Show { id }).await?
        else {
            bail!("unexpected reply to Show");
        };
        self.one(item)
    }

    /// Opens a post from an OSP code (ADR 0039 §4): as the node holds it,
    /// or fetched from its author's node and kept as an opened post.
    /// `code` is the whole `osp:post/...` code.
    pub async fn open_post(&self, code: String) -> Result<Post> {
        let OspCode::Post { id, by } = code.trim().parse()? else {
            bail!("that's not a code for a post");
        };
        let req = ClientRequest::OpenPost {
            id,
            ticket: by.to_string(),
        };
        let ClientResponse::Item { item } = self.client.request(&req).await? else {
            bail!("unexpected reply to OpenPost");
        };
        self.one(item)
    }

    /// Drops an opened post and its media now: call when the owner leaves it.
    pub async fn forget_opened(&self, post_id: String) -> Result<()> {
        let id = post_id.parse()?;
        self.client
            .request(&ClientRequest::ForgetOpened { id })
            .await?;
        Ok(())
    }

    /// Likes someone's post, or with `liked` false takes the like back
    /// (ADR 0038). The node keeps trying their node for 7 days; this returns
    /// once it's kept. A reshare's ID likes the post it shares.
    pub async fn like(&self, post_id: String, liked: bool) -> Result<()> {
        let post = post_id.parse()?;
        self.client
            .request(&ClientRequest::Like { post, liked })
            .await?;
        Ok(())
    }

    /// Who likes one of the owner's posts, newest first.
    pub async fn likes(&self, post_id: String) -> Result<Vec<Liker>> {
        let post = post_id.parse()?;
        let ClientResponse::Likes { likes } =
            self.client.request(&ClientRequest::Likes { post }).await?
        else {
            bail!("unexpected reply to Likes");
        };
        Ok(likes.into_iter().map(liker).collect())
    }

    /// A page of the owner's Activity, the latest change first: before the
    /// entry `before` (its `seq`), or from the top.
    pub async fn activity(&self, before: Option<i64>, limit: u32) -> Result<ActivityPage> {
        let ClientResponse::Activity {
            entries,
            unread,
            more,
        } = self
            .client
            .request(&ClientRequest::Activity {
                before: before.map(|b| b.max(0) as u64),
                limit: Some(limit),
            })
            .await?
        else {
            bail!("unexpected reply to Activity");
        };
        Ok(ActivityPage {
            entries: entries.into_iter().map(activity_item).collect(),
            unread: unread.min(u32::MAX as u64) as u32,
            more,
        })
    }

    /// Marks Activity entries read on the node, so every device agrees: those
    /// named, and every one up to `through` (a `seq`).
    pub async fn activity_read(&self, keys: Vec<String>, through: Option<i64>) -> Result<()> {
        self.client
            .request(&ClientRequest::ActivityRead {
                keys,
                through: through.map(|t| t.max(0) as u64),
            })
            .await?;
        Ok(())
    }

    /// The owner's replies to others' posts, newest first.
    pub async fn sent_replies(&self) -> Result<Vec<SentReply>> {
        let ClientResponse::SentReplies { replies } =
            self.client.request(&ClientRequest::SentReplies).await?
        else {
            bail!("unexpected reply to SentReplies");
        };
        Ok(replies.into_iter().map(sent_reply).collect())
    }

    /// Replies to the owner's posts waiting for an answer, newest first.
    pub async fn reply_requests(&self) -> Result<Vec<Reply>> {
        let ClientResponse::ReplyRequests { requests } =
            self.client.request(&ClientRequest::ReplyRequests).await?
        else {
            bail!("unexpected reply to ReplyRequests");
        };
        Ok(requests.into_iter().map(reply_info).collect())
    }

    /// Answers a waiting reply: `publish`, `publish_and_allow`, `decline` or
    /// `block`. The replier isn't told which.
    pub async fn answer_reply(&self, reply: String, answer: String) -> Result<()> {
        let reply: ObjectId = reply.parse()?;
        let answer = match answer.as_str() {
            "publish" => ReplyAnswer::Publish,
            "publish_and_allow" => ReplyAnswer::PublishAndAllow,
            "decline" => ReplyAnswer::Decline,
            "block" => ReplyAnswer::Block,
            _ => bail!("no such answer: {answer:?}"),
        };
        self.client
            .request(&ClientRequest::AnswerReply { reply, answer })
            .await?;
        Ok(())
    }

    /// Hides a published reply to one of the owner's posts, or shows it again.
    /// Nothing is deleted.
    pub async fn hide_reply(&self, reply: String, hidden: bool) -> Result<()> {
        let reply: ObjectId = reply.parse()?;
        self.client
            .request(&ClientRequest::HideReply { reply, hidden })
            .await?;
        Ok(())
    }

    /// Sets who may reply to one of the owner's posts: `off`, `circle` or
    /// `network`.
    pub async fn set_reply_policy(&self, post_id: String, policy: String) -> Result<()> {
        let post: ObjectId = post_id.parse()?;
        let policy: ReplyPolicy = policy.parse()?;
        self.client
            .request(&ClientRequest::SetReplyPolicy { post, policy })
            .await?;
        Ok(())
    }

    /// Lets someone reply to the owner's posts from now on (`allow`), blocks
    /// them (`block`), or with `None` forgets either.
    pub async fn set_replier(&self, identity: String, choice: Option<String>) -> Result<()> {
        let actor = identity.parse()?;
        let choice = match choice.as_deref() {
            None => None,
            Some("allow") => Some(ReplierChoice::Allow),
            Some("block") => Some(ReplierChoice::Block),
            Some(other) => bail!("no such choice: {other:?}"),
        };
        self.client
            .request(&ClientRequest::SetReplier { actor, choice })
            .await?;
        Ok(())
    }

    /// Requests to be a trusted follower waiting, newest first, and who is
    /// trusted.
    pub async fn trust_list(&self) -> Result<TrustList> {
        let ClientResponse::Trust { requests, trusted } =
            self.client.request(&ClientRequest::TrustRequests).await?
        else {
            bail!("unexpected reply to TrustRequests");
        };
        Ok(TrustList {
            requests: requests.into_iter().map(trust_entry).collect(),
            trusted: trusted.into_iter().map(trust_entry).collect(),
        })
    }

    /// Allows or declines the request from `node`. The node isn't told.
    pub async fn answer_trust(&self, node: String, allow: bool) -> Result<()> {
        let node = node.parse()?;
        self.client
            .request(&ClientRequest::AnswerTrust { node, allow })
            .await?;
        Ok(())
    }

    /// Trusts someone the node follows, without a request.
    pub async fn trust(&self, identity: String) -> Result<()> {
        let actor = identity.parse()?;
        self.client
            .request(&ClientRequest::TrustFollowed { actor })
            .await?;
        Ok(())
    }

    /// Stops trusting someone, on every node they were trusted on.
    pub async fn untrust(&self, identity: String) -> Result<()> {
        let actor = identity.parse()?;
        self.client
            .request(&ClientRequest::Untrust { actor })
            .await?;
        Ok(())
    }

    /// Asks someone the node follows to trust the owner, with a note saying
    /// who they are. Their node never says whether it did.
    pub async fn ask_trust(&self, identity: String, note: String) -> Result<()> {
        let actor = identity.parse()?;
        self.client
            .request(&ClientRequest::AskTrust { actor, note })
            .await?;
        Ok(())
    }

    /// The node owner's follow ticket, to show as a QR code.
    pub async fn invite(&self) -> Result<String> {
        match self.client.request(&ClientRequest::Invite).await? {
            ClientResponse::Ticket { ticket } => Ok(ticket),
            _ => bail!("unexpected reply to Invite"),
        }
    }

    /// An OSP code for someone (ADR 0039 §4): the owner, or anyone whose
    /// profile the node holds.
    pub async fn code_for_person(&self, identity: String) -> Result<String> {
        Ok(OspCode::Person(self.invite_for(&identity).await?).to_string())
    }

    /// An OSP code for a post, with its author's invite code in it.
    pub async fn code_for_post(&self, post_id: String, author: String) -> Result<String> {
        let id: ObjectId = post_id.parse()?;
        let by = self.invite_for(&author).await?;
        Ok(OspCode::Post { id, by }.to_string())
    }

    async fn invite_for(&self, identity: &str) -> Result<FollowTicket> {
        let actor = identity.parse()?;
        match self
            .client
            .request(&ClientRequest::InviteFor { actor })
            .await?
        {
            ClientResponse::Ticket { ticket } => ticket.parse(),
            _ => bail!("unexpected reply to InviteFor"),
        }
    }

    /// Asks the node to sync everyone it follows now. Returns how many new
    /// objects arrived.
    pub async fn sync(&self) -> Result<u32> {
        match self.client.request(&ClientRequest::Sync).await? {
            ClientResponse::Synced { new_objects } => Ok(new_objects as u32),
            _ => bail!("unexpected reply to Sync"),
        }
    }

    /// Call when the phone's network changes (Wi-Fi to mobile data, say).
    pub async fn network_changed(&self) {
        self.client.network_change().await;
    }

    /// Call when the app comes back to the foreground. The network may have
    /// changed meanwhile, and iOS may have taken back the app's sockets.
    pub async fn resumed(&self) {
        self.client.resumed();
        self.client.network_change().await;
    }

    /// Closes the connection while the app is in the background.
    pub async fn disconnect(&self) {
        self.client.disconnect().await;
    }

    /// The end of the app's log, for the status screen.
    #[frb(sync)]
    pub fn recent_log(&self, lines: u32) -> String {
        logging::tail(&self.dir, lines as usize)
    }

    /// The app's log file, to share.
    #[frb(sync)]
    pub fn log_file(&self) -> String {
        logging::path(&self.dir).to_string_lossy().into_owned()
    }

    fn profile_of(&self, p: ProfileInfo) -> Profile {
        let own = self
            .client
            .pairing()
            .is_some_and(|paired| paired.identity == p.person.actor);
        Profile {
            identity: p.person.actor.to_string(),
            name: p.person.name,
            summary: p.person.summary,
            links: p
                .person
                .attachment
                .into_iter()
                .map(|l| ProfileLinkEntry {
                    name: l.name,
                    url: l.value,
                })
                .collect(),
            picture: p.person.icon.map(|i| i.blob.to_string()),
            own,
            followed: p.followed,
            known: p.known,
            posts: p.posts as i64,
            media: p.media as i64,
            reshares: p.reshares as i64,
            followers: p.followers.map(|n| n as i64),
            following: p.following.map(|n| n as i64),
        }
    }

    /// Runs a post until it ends, and reports how, unless `discard_post`
    /// stops it first.
    async fn run_post(
        &self,
        sending: impl Future<Output = Result<ObjectId>>,
        sink: StreamSink<PostProgress>,
    ) {
        let (stop, stopped) = oneshot::channel();
        *self.stop_post.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop);
        let result = tokio::select! {
            result = sending => result,
            // Discarded; `discard_post` says how that went.
            Ok(()) = stopped => return,
        };
        let last = match result {
            Ok(id) => PostProgress {
                message: Some(id.to_string()),
                ..progress(PostStage::Done, 0, 0)
            },
            Err(e) => {
                tracing::warn!("post failed: {e:#}");
                PostProgress {
                    message: Some(format!("{e:#}")),
                    kept: self.client.pending_post().is_some(),
                    no_contact: e.downcast_ref::<NoContact>().is_some(),
                    ..progress(PostStage::Failed, 0, 0)
                }
            }
        };
        let _ = sink.add(last);
    }
}

/// Passes a post's progress on to Dart.
fn report_to(sink: StreamSink<PostProgress>) -> impl Fn(osp_client::PostProgress) {
    move |p| {
        let report = match p {
            osp_client::PostProgress::Sending { sent, total } if sent < total => {
                progress(PostStage::Sending, sent, total)
            }
            osp_client::PostProgress::Sending { sent, total } => {
                progress(PostStage::Processing, sent, total)
            }
            osp_client::PostProgress::Waiting => progress(PostStage::Waiting, 0, 0),
        };
        let _ = sink.add(report);
    }
}

fn progress(stage: PostStage, sent: u64, total: u64) -> PostProgress {
    PostProgress {
        stage,
        sent: sent as i64,
        total: total as i64,
        message: None,
        kept: false,
        no_contact: false,
    }
}

fn trust_entry(t: TrustInfo) -> TrustEntry {
    TrustEntry {
        node: t.node.to_string(),
        identity: t.actor.to_string(),
        name: t.name,
        note: t.note,
        secs_ago: t.secs_ago as i64,
        followed: t.followed,
        trusted_elsewhere: t.trusted_elsewhere,
        follower_secs: t.follower_secs.map(|s| s as i64),
        reached: match t.reached {
            Some(Reached::Direct) => "direct",
            Some(Reached::Relay) => "relay",
            Some(Reached::Tor) => "tor",
            None => "none",
        }
        .to_string(),
        copies: t.copies as u32,
    }
}

fn reply_info(r: ReplyInfo) -> Reply {
    Reply {
        id: r.id.to_string(),
        post: r.post.to_string(),
        identity: r.actor.to_string(),
        name: r.name,
        text: r.content,
        published: r.published,
        state: match r.state {
            ReplyStatus::Published => "published",
            ReplyStatus::Hidden => "hidden",
            ReplyStatus::Request => "request",
        }
        .to_string(),
        why: r.why,
        seq: r.seq as i64,
    }
}

fn sent_reply(r: SentReplyInfo) -> SentReply {
    SentReply {
        id: r.id.to_string(),
        post: r.post.to_string(),
        creator: r.creator.to_string(),
        text: r.content,
        published: r.published,
        status: match r.status {
            SentStatus::Sending => "sending",
            SentStatus::Published => "published",
            SentStatus::AwaitingApproval => "awaiting_approval",
            SentStatus::NotPublished => "not_published",
            SentStatus::Closed => "closed",
            SentStatus::Deleted => "deleted",
            SentStatus::Refused => "refused",
            SentStatus::GaveUp => "gave_up",
            SentStatus::Deleting => "deleting",
        }
        .to_string(),
        error: r.error,
        next_try_secs: r.next_try_secs.map(|s| s as i64),
    }
}

fn liker(l: LikeInfo) -> Liker {
    Liker {
        identity: l.actor.to_string(),
        name: l.name,
        published: l.published,
    }
}

fn activity_item(e: ActivityEntry) -> ActivityItem {
    let named = |p: ActivityPerson| ActivityName {
        identity: p.actor.to_string(),
        name: p.name,
    };
    let count = |n: u64| n.min(u32::MAX as u64) as u32;
    let mut item = ActivityItem {
        key: e.key,
        seq: e.seq.min(i64::MAX as u64) as i64,
        at_ms: e.at_ms,
        read: e.read,
        kind: String::new(),
        post: None,
        post_text: String::new(),
        reply: None,
        people: Vec::new(),
        count: 1,
        text: String::new(),
        waiting: false,
        node: None,
    };
    match e.what {
        ActivityWhat::Reply {
            post,
            post_text,
            reply,
            actor,
            name,
            content,
            waiting,
        } => {
            item.kind = "reply".into();
            item.post = Some(post.to_string());
            item.post_text = post_text;
            item.reply = Some(reply.to_string());
            item.people = vec![named(ActivityPerson { actor, name })];
            item.text = content;
            item.waiting = waiting;
        }
        ActivityWhat::Liked {
            post,
            post_text,
            people,
            count: n,
        } => {
            item.kind = "liked".into();
            item.post = Some(post.to_string());
            item.post_text = post_text;
            item.people = people.into_iter().map(named).collect();
            item.count = count(n);
        }
        ActivityWhat::Reshared {
            post,
            post_text,
            people,
            count: n,
        } => {
            item.kind = "reshared".into();
            item.post = Some(post.to_string());
            item.post_text = post_text;
            item.people = people.into_iter().map(named).collect();
            item.count = count(n);
        }
        ActivityWhat::Followed { node, person } => {
            item.kind = "followed".into();
            item.node = Some(node.to_string());
            item.people = person.into_iter().map(named).collect();
        }
        ActivityWhat::KnowsYou { node, person, note } => {
            item.kind = "knows_you".into();
            item.node = Some(node.to_string());
            item.people = vec![named(person)];
            item.text = note;
        }
        ActivityWhat::ReplyPublished {
            post,
            reply,
            creator,
            creator_name,
            content,
        } => {
            item.kind = "reply_published".into();
            item.post = Some(post.to_string());
            item.reply = Some(reply.to_string());
            item.people = vec![ActivityName {
                identity: creator.to_string(),
                name: creator_name.unwrap_or_default(),
            }];
            item.text = content;
        }
    }
    item
}

fn node_info(p: Pairing) -> NodeInfo {
    NodeInfo {
        name: p.name,
        identity: p.identity.to_string(),
        paired: p.paired,
    }
}

fn carrier_state(c: osp_client::osp_proto::rpc::CarrierStatus) -> CarrierState {
    let n = |v: usize| v.min(u32::MAX as usize) as u32;
    let gb = |v: u64| v.min(u32::MAX as u64) as u32;
    let b = |v: u64| v.min(i64::MAX as u64) as i64;
    CarrierState {
        carrying: c.carrying,
        off: c.off,
        paused: c.paused,
        metered: c.metered,
        seed: c.seed,
        jobs: n(c.jobs),
        upload_share: c.upload_share.into(),
        upload_mbps: c.upload_mbps,
        upload_from: c.upload_from,
        circuits: n(c.circuits),
        bytes: b(c.bytes),
        hops: c.hops,
        month_bytes: b(c.month_bytes),
        month_budget: c.month_budget.map(b),
        over_budget: c.over_budget,
        data_cap_gb: gb(c.data_cap_gb),
        monthly_gb: c.monthly_gb.map(gb),
        seeds: n(c.seeds),
        standing: n(c.standing),
        probation: n(c.probation),
    }
}

fn post(item: FeedItem) -> Post {
    let author = item
        .actor_name
        .unwrap_or_else(|| item.actor.fmt_short().to_string());
    let (reshared_by, resharer_hosts) = match item.reshared_by {
        Some(by) => (
            Some(by.name.unwrap_or_else(|| by.actor.fmt_short().to_string())),
            by.hosting,
        ),
        None => found_resharers(&item.found_reshares),
    };
    Post {
        id: item.id.to_string(),
        author_identity: item.actor.to_string(),
        author_picture: item.actor_picture.map(|blob| blob.to_string()),
        author,
        published: item.published,
        text: item.content,
        media: item
            .attachments
            .into_iter()
            .map(|a| Media {
                is_video: a.attachment.kind == MediaKind::Video,
                blob: a.attachment.blob.to_string(),
                media_type: a.attachment.media_type,
                size: a.attachment.size as i64,
                width: a.attachment.width,
                height: a.attachment.height,
                duration_ms: a.attachment.duration_ms.map(|d| d as u32),
                thumbnail_blob: a.attachment.thumbnail.map(|t| t.blob.to_string()),
                on_node: a.local,
            })
            .collect(),
        reshared_by,
        resharer_hosts,
        found_through: item.found_through,
        deletable: item.deletable.map(|id| id.to_string()),
        resharers: item.resharers,
        likes: item.likes.map(|n| n.min(u32::MAX as u64) as u32),
        liked: item.liked,
        reshared: item.reshared.map(|id| id.to_string()),
        opened: item.opened,
        regions: item.regions,
    }
}

/// Who reshared a post found through a directory (ADR 0025), for the card's
/// "Reshared by" line: the newest one with a name, and how many others, so
/// a newcomer sees who passed it on. A post nobody reshared gets nothing.
fn found_resharers(found: &[ResharedBy]) -> (Option<String>, bool) {
    if found.is_empty() {
        return (None, false);
    }
    let others = found.len() - 1;
    // "who hosts a copy" reads right only for one.
    let hosts = others == 0 && found[0].hosting;
    let text = match found.iter().find_map(|r| r.name.as_deref()) {
        Some(name) if others == 0 => name.to_string(),
        Some(name) if others == 1 => format!("{name} and 1 other"),
        Some(name) => format!("{name} and {others} others"),
        None if others == 0 => "1 person".to_string(),
        None => format!("{} people", found.len()),
    };
    (Some(text), hosts)
}

/// A new device key, as 64 hex digits, for the app to keep in the phone's
/// secure storage.
#[frb(sync)]
pub fn new_device_key() -> String {
    osp_client::new_device_key()
}

/// Makes sure a ticket looks like a pairing ticket before trying to pair:
/// `OSPPAIR1`, or `OSPPAIR2` from a node with its own relays (ADR 0044).
#[frb(sync)]
pub fn is_pairing_ticket(text: String) -> bool {
    osp_client::osp_proto::is_pairing_ticket(&text)
}

/// Whether text looks like a follow ticket.
#[frb(sync)]
pub fn is_follow_ticket(text: String) -> bool {
    text.trim().to_ascii_uppercase().starts_with("OSP1")
}

/// What an OSP code shares (ADR 0039 §4): a person, or a post and its
/// author.
pub struct SharedCode {
    /// The code, written out in full.
    pub code: String,
    /// The invite code inside it: the person's, or the post author's.
    pub invite: String,
    /// The person, or the post's author.
    pub actor: String,
    /// The post's ID, for a post code.
    pub post: Option<String>,
}

/// Reads an OSP code, such as one pasted where an invite code goes. A
/// malformed one fails with a message to show as it is.
#[frb(sync)]
pub fn read_osp_code(text: String) -> Result<SharedCode> {
    Ok(shared_code(text.parse::<OspCode>()?))
}

/// The first OSP code in `text`, as on the clipboard, if any.
#[frb(sync)]
pub fn find_osp_code(text: String) -> Option<SharedCode> {
    OspCode::find_in(&text).map(shared_code)
}

fn shared_code(code: OspCode) -> SharedCode {
    let ticket = code.ticket();
    SharedCode {
        code: code.to_string(),
        invite: ticket.to_string(),
        actor: ticket.actor.to_string(),
        post: match &code {
            OspCode::Post { id, .. } => Some(id.to_string()),
            OspCode::Person(_) => None,
        },
    }
}

fn universe(s: universe::Snapshot) -> UniverseSnapshot {
    let count = |n: u64| n.min(i64::MAX as u64) as i64;
    UniverseSnapshot {
        version: s.version,
        level: match s.level {
            universe::Level::Universe => UniverseLevel::Universe,
            universe::Level::Galaxy => UniverseLevel::Galaxy,
            universe::Level::Cluster => UniverseLevel::Cluster,
            universe::Level::Local => UniverseLevel::Local,
            universe::Level::Close => UniverseLevel::Close,
        },
        region: s.region,
        path: s.path,
        groups: s
            .groups
            .into_iter()
            .map(|g| UniverseGroup {
                id: g.id,
                parent: g.parent,
                from: g.from,
                people: g.people,
                bright: g.bright,
                lit: g.lit,
                x: g.at[0],
                y: g.at[1],
            })
            .collect(),
        people: s
            .people
            .into_iter()
            .map(|p| UniversePerson {
                identity: p.actor.to_string(),
                name: p.name,
                picture: p.picture.map(|h| h.to_string()),
                step: p.step,
                followed: p.followed,
                known: p.known,
                found_through: p.found_through,
                posts: count(p.posts),
                media: count(p.media),
                reshares: count(p.reshares),
                last_post: p.last_post,
                x: p.at[0],
                y: p.at[1],
                group: p.group,
                bright: p.bright,
                lit: p.lit,
            })
            .collect(),
        lines: s
            .lines
            .into_iter()
            .map(|l| UniverseLine {
                kind: match l.kind {
                    universe::LineKind::Reshared => LineKind::Reshared,
                    universe::LineKind::CreditedEarly => LineKind::CreditedEarly,
                },
                from: l.from,
                to: l.to,
                count: l.count,
                last: l.last,
            })
            .collect(),
        more_people: count(s.more_people),
        machines: s
            .machines
            .into_iter()
            .map(|m| UniverseMachine {
                words: m.words,
                state: match m.state {
                    universe::MachineState::HeardOf => MachineState::HeardOf,
                    universe::MachineState::Met => MachineState::Met,
                    universe::MachineState::AnsweredLately => MachineState::AnsweredLately,
                    universe::MachineState::Faded => MachineState::Faded,
                },
                directory: m.directory,
                activity: match m.activity {
                    universe::ActivityBand::None => ActivityBand::None,
                    universe::ActivityBand::Some => ActivityBand::Some,
                    universe::ActivityBand::ALot => ActivityBand::ALot,
                },
            })
            .collect(),
        network: s.network.map(|n| NetworkTotals {
            posts_hosted: count(n.posts_hosted),
            bytes_served: count(n.bytes_served),
            people_found: count(n.people_found),
        }),
        sky: s
            .sky
            .into_iter()
            .map(|c| SkyCell {
                level: c.level,
                x: c.x,
                y: c.y,
                people: c.people,
                creators: c.creators,
                lit: c.lit,
                at_x: c.at[0],
                at_y: c.at[1],
            })
            .collect(),
    }
}

fn path_hop(h: connected::Hop) -> PathHop {
    use connected::How;
    let (how, steps) = match h.how {
        How::Follows => (HopKind::Follows, 1),
        How::Knows => (HopKind::Knows, 1),
        How::Reshared => (HopKind::Reshared, 1),
        How::CreditedEarly => (HopKind::CreditedEarly, 1),
        How::Gap { steps } => (HopKind::Gap, steps),
    };
    PathHop {
        identity: h.to.to_string(),
        how,
        steps,
    }
}

fn universe_event(e: universe::Event) -> UniverseEvent {
    use universe::EventWhat;
    let mut out = UniverseEvent {
        seq: e.seq,
        first: e.first,
        last: e.last,
        count: e.count,
        kind: EventKind::Found,
        words: None,
        people: 0,
        from: None,
        author: None,
        post: None,
        bytes: 0,
        host: None,
        kept_by_copy: false,
        from_author: false,
        met: false,
    };
    match e.what {
        EventWhat::Found { directory, people } => {
            out.words = Some(directory);
            out.people = people;
        }
        EventWhat::Reshared { from, author, post } => {
            out.kind = EventKind::Reshared;
            out.from = Some(from.to_string());
            out.author = Some(author.to_string());
            out.post = Some(post.to_string());
        }
        EventWhat::Fetched {
            author,
            post,
            bytes,
            host,
            kept_by_copy,
            from_author,
        } => {
            out.kind = EventKind::Fetched;
            out.from_author = from_author;
            out.author = author.map(|a| a.to_string());
            out.post = post.map(|p| p.to_string());
            out.bytes = bytes;
            out.host = host.map(|h| h.to_string());
            out.kept_by_copy = kept_by_copy;
        }
        EventWhat::Served {
            author,
            post,
            bytes,
        } => {
            out.kind = EventKind::Served;
            out.author = author.map(|a| a.to_string());
            out.post = post.map(|p| p.to_string());
            out.bytes = bytes;
        }
        EventWhat::Machine { words, met } => {
            out.kind = EventKind::Machine;
            out.words = Some(words);
            out.met = met;
        }
    }
    out
}

fn found_person(p: osp_client::osp_proto::rpc::PersonInfo) -> FoundPerson {
    FoundPerson {
        identity: p.actor.to_string(),
        name: p.name,
        summary: p.summary,
        followed: p.followed,
        found_through: p.found_through,
        newest: p.newest,
        regions: p.regions,
        picture: p.picture.map(|blob| blob.to_string()),
    }
}

/// Something the owner blocked: `kind` is `account`, `post` or `file`.
pub struct BlockedThing {
    pub kind: String,
    pub id: String,
    /// A person's name, if known when they were blocked.
    pub name: Option<String>,
    /// When, as RFC 3339.
    pub since: String,
}

fn block_target(kind: &str, id: &str) -> Result<BlockTarget> {
    Ok(match kind {
        "account" => BlockTarget::Account { actor: id.parse()? },
        "post" => BlockTarget::Post { id: id.parse()? },
        "file" => BlockTarget::File { hash: id.parse()? },
        other => bail!("can't block a {other}"),
    })
}

#[cfg(test)]
mod tests {
    use osp_client::SecretKey;

    use super::*;

    fn by(name: Option<&str>, hosting: bool) -> ResharedBy {
        let actor = SecretKey::from_bytes(&[name.map_or(0, |n| n.len() as u8); 32]).public();
        ResharedBy {
            actor,
            name: name.map(String::from),
            announce: ObjectId::of(b"reshare"),
            published: String::new(),
            hosting,
        }
    }

    #[test]
    fn a_found_post_says_who_reshared_it() {
        assert_eq!(found_resharers(&[]), (None, false));
        assert_eq!(
            found_resharers(&[by(Some("Jimmy"), true)]),
            (Some("Jimmy".into()), true)
        );
        // The newest with a name, and how many others; hosting only for one.
        let two = [by(None, true), by(Some("Jimmy"), false)];
        assert_eq!(
            found_resharers(&two),
            (Some("Jimmy and 1 other".into()), false)
        );
        let three = [by(None, false), by(None, false), by(None, true)];
        assert_eq!(found_resharers(&three), (Some("3 people".into()), false));
    }

    fn item(id: ObjectId, actor: PublicKey, photo: Option<BlobHash>) -> FeedItem {
        FeedItem {
            id,
            actor,
            actor_name: None,
            published: String::new(),
            content: String::new(),
            attachments: photo
                .into_iter()
                .map(|blob| osp_client::osp_proto::rpc::AttachmentInfo {
                    attachment: osp_client::osp_proto::Attachment {
                        kind: MediaKind::Image,
                        media_type: "image/jpeg".into(),
                        blob,
                        size: 1,
                        width: 1,
                        height: 1,
                        duration_ms: None,
                        name: None,
                        thumbnail: None,
                        profile: String::new(),
                    },
                    local: false,
                })
                .collect(),
            reshared_by: None,
            found_through: None,
            deletable: None,
            found_reshares: vec![],
            resharers: None,
            likes: None,
            liked: false,
            reshared: None,
            opened: false,
            regions: vec![],
            actor_picture: None,
        }
    }

    #[test]
    fn the_app_hides_listed_posts_files_and_replies_but_not_the_spared() {
        use osp_client::osp_proto::lists::{file_entry, post_entry, HideKind, HideList};

        let key = SecretKey::from_bytes(&[3; 32]);
        let (kim, me, excepted) = (
            SecretKey::from_bytes(&[4; 32]).public(),
            SecretKey::from_bytes(&[5; 32]).public(),
            SecretKey::from_bytes(&[6; 32]).public(),
        );
        let (words, photo) = (ObjectId::of(b"words"), BlobHash::from_bytes([9; 32]));
        let mut list = HideList::new(key.public());
        list.add(HideKind::Post, post_entry(&words), "2026-11-17T00:00:00Z");
        list.add(HideKind::File, file_entry(&photo), "2026-11-17T00:00:00Z");
        let hider = Hider {
            listed: Listed::new(&list),
            spared: [me, excepted].into_iter().collect(),
        };

        assert!(hider.hides(&item(words, kim, None)));
        assert!(hider.hides(&item(ObjectId::of(b"other"), kim, Some(photo))));
        assert!(!hider.hides(&item(ObjectId::of(b"other"), kim, None)));
        // The owner's own, and someone excepted, aren't hidden.
        assert!(!hider.hides(&item(words, me, Some(photo))));
        assert!(!hider.hides(&item(words, excepted, None)));

        let reply = |actor| ReplyInfo {
            id: words,
            post: ObjectId::of(b"post"),
            actor,
            name: String::new(),
            content: String::new(),
            published: String::new(),
            state: ReplyStatus::Published,
            why: None,
            seq: 1,
        };
        assert!(hider.hides_reply(&reply(kim)));
        assert!(!hider.hides_reply(&reply(excepted)));
    }
}
