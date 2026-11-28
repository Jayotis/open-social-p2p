//! Open Social P2P wire formats.
//!
//! Everything two parties must agree on lives here: signed objects (ADR 0004),
//! ActivityStreams payloads (ADR 0003), the request/response messages of the
//! `osp/social/2` and `osp/client/0` protocols, the frame format they travel in,
//! and follow tickets. This crate does no networking of its own.

pub mod binding;
pub mod code;
pub mod connected;
pub mod framing;
pub mod lists;
pub mod machine;
pub mod object;
pub mod rpc;
pub mod share;
pub mod social;
pub mod ticket;
pub mod types;
pub mod universe;

use std::time::SystemTime;

pub use binding::{BoundEndpoint, EndpointBinding, EndpointKind};
pub use code::{CodeError, OspCode};
pub use iroh_base::{EndpointAddr, EndpointId, PublicKey, SecretKey};
pub use object::{CertifiedKey, ObjectId, SignedObject};
pub use ticket::{
    FollowTicket, PairingRelays, PairingTicket, is_pairing_ticket, pairing_pass_hash,
};
pub use types::{
    Announce, Attachment, BlobHash, BlobRef, Delete, EarlierKeys, ForwardDelete, Key, Like,
    MediaKind, Note, Payload, Person, ProfileLink, Undo, Vouch, VouchKind,
};

/// ALPN of the node-to-node social protocol. Version 1 added objects
/// signed by certified keys (ADR 0010), which version 0 can't check.
/// Version 2 added `Delete` and `ForwardDelete` (ADR 0019): a node on
/// version 1 can't read them, so it can't sync with one on version 2, and
/// can't go on serving a deleted post to it.
pub const SOCIAL_ALPN: &[u8] = b"osp/social/2";

/// ALPN a paired device uses to talk to its home node.
pub const CLIENT_ALPN: &[u8] = b"osp/client/0";

/// What a home node tells a device it doesn't know, and what a phone says
/// when its node turns it away: the app looks for these words to offer
/// pairing again.
pub const NOT_PAIRED: &str = "this device isn't paired with this node";

/// What pairing says when the node turns the code away, whether the node
/// answers or its direct endpoint refuses the connection. Which it was, used
/// or out of time, isn't told: a stranger holding the code would learn it too.
pub const CODE_TURNED_DOWN: &str = "your node turned this pairing code down: it was already used, \
     or it's more than 10 minutes old. Run `osp pair` on the node for a new one";

/// The code a node's direct endpoint closes a connection with when its key
/// isn't let in, once the handshake has shown whose it is.
pub const NOT_ALLOWED: u32 = 403;

/// ALPN of the protocol between machine names (ADR 0015). Nothing else is
/// served on a machine endpoint, and this is served on no other.
pub const MACHINE_ALPN: &[u8] = b"osp/machine/1";

/// The current time as an RFC 3339 timestamp, the format used for `published`.
pub fn now_rfc3339() -> String {
    humantime::format_rfc3339_millis(SystemTime::now()).to_string()
}

/// Unix milliseconds as an RFC 3339 timestamp, like `published`.
pub fn rfc3339_from_ms(ms: u64) -> String {
    let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms);
    humantime::format_rfc3339_millis(t).to_string()
}

/// Parses a `published` timestamp into Unix milliseconds.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let t = humantime::parse_rfc3339_weak(s).ok()?;
    let ms = t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis();
    i64::try_from(ms).ok()
}
