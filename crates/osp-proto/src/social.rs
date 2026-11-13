//! Messages of the node-to-node protocol `osp/social/2` (ADR 0003).
//!
//! One QUIC bidirectional stream per request: the requester sends one
//! [`SocialRequest`] frame, the responder answers with one [`SocialResponse`].

use iroh_base::{EndpointAddr, PublicKey};
use serde::{Deserialize, Serialize};

use crate::{
    binding::BoundEndpoint,
    object::{ObjectId, SignedObject},
    types::BlobHash,
};

/// Largest outbox page a node will return.
pub const MAX_OUTBOX_PAGE: u32 = 100;

/// Most objects one `GetObjects` may ask for. The largest object is about
/// 172 KiB once encoded, so this many always fit in one reply frame.
pub const MAX_GET_OBJECTS: usize = 50;

/// Largest request frame a node reads, in bytes. Every request is small;
/// replies (outbox pages) may use the full frame size.
pub const MAX_REQUEST_LEN: u32 = 64 * 1024;

/// Longest note sent with a request to be a trusted follower, in
/// characters (ADR 0013).
pub const MAX_TRUST_NOTE_CHARS: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SocialRequest {
    /// The signed profile of the identity this node speaks for.
    GetProfile,
    /// Outbox entries with a sequence number greater than `since`.
    GetOutbox { since: u64, limit: u32 },
    /// "I follow you; notify me of new posts." `addr` is how to reach the
    /// subscriber; it must belong to the connecting node. `shares_prefetched`
    /// says whether it serves what it fetches ahead of time to anyone, so
    /// may be named as a host of your media (the follower swarm, doc 13).
    /// `media` is its media endpoint (ADR 0014). `direct` is its direct
    /// endpoint, only if its rule allows anyone: unlike one offered with
    /// `Endpoints`, it may be named to other nodes.
    Subscribe {
        addr: EndpointAddr,
        #[serde(default)]
        shares_prefetched: bool,
        #[serde(default)]
        media: Option<Box<BoundEndpoint>>,
        #[serde(default)]
        direct: Option<Box<BoundEndpoint>>,
    },
    /// Push hint from a followed node: `actor` has published up to `seq`.
    Notify { actor: PublicKey, seq: u64 },
    /// Which nodes this node knows to be serving `blob`. To its followers,
    /// the node of the post's author, or of a resharer that hosts it, also
    /// names a few followers that hold a copy (doc 13).
    WhoHosts { blob: BlobHash },
    /// "I hold a verified copy of `blob`, media of one of your posts or of
    /// a post you reshared and host, and serve it to anyone." From a
    /// follower that subscribed with `shares_prefetched`, so your node can
    /// name it in `WhoHosts`, once it has fetched a piece of the blob from
    /// the follower to check. The reply waits for the check.
    ///
    /// Or from a resharer, follower or not (ADR 0025): `reshare` is its
    /// signed `Announce` with `hosting` of the post `blob` belongs to, and its
    /// owner's signed profile, which must list the sending node and gives its
    /// media endpoint. The author's node checks a piece from there, and names
    /// it as a host to anyone who asks if the profile says it may be listed.
    Hosting {
        blob: BlobHash,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reshare: Option<Box<HostingReshare>>,
    },
    /// "The identity you follow now lives on this node" (ADR 0009). Sent to
    /// followers when a node takes over an identity. `profile` is its signed
    /// profile listing the sending node; `addr` is how to reach that node and
    /// must belong to it.
    Moved {
        profile: SignedObject,
        addr: EndpointAddr,
    },
    /// Objects from this node's own outbox, by ID, at most
    /// [`MAX_GET_OBJECTS`]. A follower asks for them after the identity
    /// revoked a signing key, for copies signed by the new key in place of
    /// the ones it dropped (ADR 0010). IDs the node doesn't have are skipped.
    GetObjects { ids: Vec<ObjectId> },
    /// "My owner asks to be one of your trusted followers" (ADR 0013), from
    /// a follower. `profile` is its owner's signed profile, which must list
    /// the sending node; `note` says who they are, in at most
    /// [`MAX_TRUST_NOTE_CHARS`] characters. A valid request gets `Ok`
    /// whatever becomes of it, so the asking node never learns the answer.
    AskTrust { profile: SignedObject, note: String },
    /// "How can I reach your other endpoints?" (ADR 0014). `direct` is the
    /// sender's direct endpoint, if its rule allows a direct connection with
    /// this node; without it, this node's endpoint refuses the sender's.
    Endpoints {
        #[serde(default)]
        direct: Option<BoundEndpoint>,
    },
    /// "List me", to a directory's public endpoint and never from a machine name
    /// (ADR 0015). `profile` is the sender's owner's signed profile, which must
    /// list the sending node. If it says they may not be listed, this asks to
    /// be dropped.
    AskListing { profile: SignedObject },
    /// "Here's my owner's reply to one of your posts" (ADR 0028). `reply` is a
    /// signed `Note` whose `in_reply_to` names a post in this node's outbox;
    /// `profile` is its replier's signed profile, which must list the sending
    /// node. Answered with [`SocialResponse::Reply`]; sending the same reply
    /// again gives its current state.
    SubmitReply {
        reply: SignedObject,
        profile: SignedObject,
    },
    /// "My owner deleted their reply" (ADR 0028): their signed `Delete` of it,
    /// and their profile as with `SubmitReply`. The creator's node drops the
    /// reply and keeps the `Delete`, so an old copy can't come back.
    DeleteReply {
        delete: SignedObject,
        profile: SignedObject,
    },
    /// "My owner reshared one of your posts" (ADR 0029), from the resharer's
    /// node, for every reshare, hosting or not. `announce` is the signed
    /// `Announce`; `profile` is the resharer's signed profile, which must list
    /// the sending node. The author's node keeps it as a vouch for its owner.
    Reshared {
        announce: SignedObject,
        profile: SignedObject,
    },
    /// "My owner credits yours with finding one of their posts early" (ADR
    /// 0029), from the post's author's node. `credit` is the signed
    /// `FoundEarly`; `profile` is the author's signed profile, which must
    /// list the sending node. The finder's node keeps it as a vouch.
    FoundEarly {
        credit: SignedObject,
        profile: SignedObject,
    },
}

/// Who a post lets reply (ADR 0028).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyPolicy {
    /// Nobody.
    Off,
    /// Those the creator follows, their trusted followers, and those the
    /// creator allowed.
    Circle,
    /// The circle directly; anyone else goes to the creator's requests.
    #[default]
    Network,
}

impl ReplyPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            ReplyPolicy::Off => "off",
            ReplyPolicy::Circle => "circle",
            ReplyPolicy::Network => "network",
        }
    }
}

impl std::str::FromStr for ReplyPolicy {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "off" => ReplyPolicy::Off,
            "circle" => ReplyPolicy::Circle,
            "network" => ReplyPolicy::Network,
            _ => anyhow::bail!("a reply policy is off, circle or network, not {s:?}"),
        })
    }
}

/// What became of a reply on its creator's node (ADR 0028).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyState {
    /// In the post's thread, for anyone to read.
    Published,
    /// Waiting among the creator's requests.
    AwaitingApproval,
    /// Refused, declined, hidden or past a limit: one answer for all, so a
    /// replier can't tell they're blocked.
    NotPublished,
    /// The post takes no replies.
    Closed,
    /// The reply, or the post it answers, was deleted.
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum SocialResponse {
    Profile {
        profile: SignedObject,
    },
    Outbox {
        entries: Vec<OutboxEntry>,
        /// Objects referenced by entries in this page (for example the post an
        /// Announce reshares), when this node has them.
        referenced: Vec<SignedObject>,
        /// The newest sequence number in the outbox.
        latest: u64,
    },
    Ok,
    Hosts {
        nodes: Vec<EndpointAddr>,
    },
    /// The answer to `WhoHosts` from a node with other endpoints (ADR 0014):
    /// each host by the endpoint that serves its media, tied to its public ID
    /// by its binding.
    BoundHosts {
        hosts: Vec<BoundEndpoint>,
    },
    Objects {
        objects: Vec<SignedObject>,
    },
    /// The answer to `SubmitReply` and `DeleteReply` (ADR 0028).
    Reply {
        state: ReplyState,
    },
    /// This node's media endpoint, and its direct one if its rule allows.
    Endpoints {
        media: Option<Box<BoundEndpoint>>,
        direct: Option<Box<BoundEndpoint>>,
    },
    Error {
        message: String,
    },
}

/// A hosting reshare, as its resharer tells the post's author of it (ADR 0025).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostingReshare {
    pub announce: SignedObject,
    pub profile: SignedObject,
}

/// ALPN of the one request an author's media endpoint answers besides media
/// (ADR 0025): who hosts one of its posts' files. It's how a viewing endpoint
/// asks, through the relay, without showing who it is (ADR 0024).
pub const MEDIA_HOSTS_ALPN: &[u8] = b"osp/media-hosts/0";

/// "Who hosts `blob`, of your post `post`?", on an author's media endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaHostsRequest {
    pub post: ObjectId,
    pub blob: BlobHash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum MediaHostsResponse {
    /// The hosting resharers that may be named to anyone, by their media
    /// endpoints, and the author's own. Never a follower's copy.
    Hosts {
        hosts: Vec<BoundEndpoint>,
    },
    /// The post was deleted: the author's signed `Delete` (ADR 0019).
    Deleted {
        delete: SignedObject,
    },
    Error {
        message: String,
    },
}

/// ALPN on which a creator's media endpoint serves the published replies to
/// its posts (ADR 0028). Readers ask from a viewing endpoint, through the
/// relay, so the creator's node doesn't learn who reads (ADR 0024).
pub const REPLIES_ALPN: &[u8] = b"osp/replies/0";

/// Most replies in one page of a thread.
pub const MAX_THREAD_PAGE: usize = 50;

/// Most bytes of replies and profiles in one page of a thread, once encoded.
pub const MAX_THREAD_PAGE_BYTES: usize = 256 * 1024;

/// "The replies to your post `post` after place `after`", on a creator's
/// media endpoint. `after` is 0 for the start of the thread, or the `seq` of
/// the last reply read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepliesRequest {
    pub post: ObjectId,
    #[serde(default)]
    pub after: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum RepliesResponse {
    /// Published replies, oldest first, at most [`MAX_THREAD_PAGE`] and
    /// [`MAX_THREAD_PAGE_BYTES`]. `more` if the thread goes on past them.
    Replies {
        policy: ReplyPolicy,
        replies: Vec<ThreadReply>,
        more: bool,
    },
    /// The post was deleted: the creator's signed `Delete` (ADR 0019).
    Deleted {
        delete: SignedObject,
    },
    Error {
        message: String,
    },
}

/// One published reply in a thread, with the profile its replier sent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadReply {
    /// Its place in the thread: ask `after` it for the ones that follow.
    pub seq: u64,
    pub reply: SignedObject,
    pub profile: SignedObject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub seq: u64,
    pub object: SignedObject,
}
