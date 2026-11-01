//! Messages of the node-to-node protocol `osp/social/1` (ADR 0003).
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
    Hosting { blob: BlobHash },
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
    /// This node's media endpoint, and its direct one if its rule allows.
    Endpoints {
        media: Option<Box<BoundEndpoint>>,
        direct: Option<Box<BoundEndpoint>>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub seq: u64,
    pub object: SignedObject,
}
