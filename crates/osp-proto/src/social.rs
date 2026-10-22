//! Messages of the node-to-node protocol `osp/social/0` (ADR 0003).
//!
//! One QUIC bidirectional stream per request: the requester sends one
//! [`SocialRequest`] frame, the responder answers with one [`SocialResponse`].

use iroh_base::{EndpointAddr, PublicKey};
use serde::{Deserialize, Serialize};

use crate::{object::SignedObject, types::BlobHash};

/// Largest outbox page a node will return.
pub const MAX_OUTBOX_PAGE: u32 = 100;

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
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub seq: u64,
    pub object: SignedObject,
}
