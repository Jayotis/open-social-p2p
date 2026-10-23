//! Messages of the node-to-node protocol `osp/social/1` (ADR 0003).
//!
//! One QUIC bidirectional stream per request: the requester sends one
//! [`SocialRequest`] frame, the responder answers with one [`SocialResponse`].

use iroh_base::{EndpointAddr, PublicKey};
use serde::{Deserialize, Serialize};

use crate::{
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SocialRequest {
    /// The signed profile of the identity this node speaks for.
    GetProfile,
    /// Outbox entries with a sequence number greater than `since`.
    GetOutbox { since: u64, limit: u32 },
    /// "I follow you; notify me of new posts." `addr` is how to reach the
    /// subscriber; it must belong to the connecting node.
    Subscribe { addr: EndpointAddr },
    /// Push hint from a followed node: `actor` has published up to `seq`.
    Notify { actor: PublicKey, seq: u64 },
    /// Which nodes this node knows to be serving `blob`.
    WhoHosts { blob: BlobHash },
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
    Objects {
        objects: Vec<SignedObject>,
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
