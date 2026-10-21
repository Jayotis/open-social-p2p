//! Open Social P2P wire formats.
//!
//! Everything two parties must agree on lives here: signed objects (ADR 0004),
//! ActivityStreams payloads (ADR 0003), the request/response messages of the
//! `osp/social/0` and `osp/client/0` protocols, the frame format they travel in,
//! and follow tickets. This crate does no networking of its own.

pub mod framing;
pub mod object;
pub mod rpc;
pub mod social;
pub mod ticket;
pub mod types;

use std::time::SystemTime;

pub use iroh_base::{EndpointAddr, EndpointId, PublicKey, SecretKey};
pub use object::{ObjectId, SignedObject};
pub use ticket::FollowTicket;
pub use types::{Announce, Attachment, BlobHash, BlobRef, MediaKind, Note, Payload, Person};

/// ALPN of the node-to-node social protocol.
pub const SOCIAL_ALPN: &[u8] = b"osp/social/0";

/// ALPN a paired device uses to talk to its home node.
pub const CLIENT_ALPN: &[u8] = b"osp/client/0";

/// The current time as an RFC 3339 timestamp, the format used for `published`.
pub fn now_rfc3339() -> String {
    humantime::format_rfc3339_millis(SystemTime::now()).to_string()
}

/// Parses a `published` timestamp into Unix milliseconds.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let t = humantime::parse_rfc3339_weak(s).ok()?;
    let ms = t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis();
    i64::try_from(ms).ok()
}
