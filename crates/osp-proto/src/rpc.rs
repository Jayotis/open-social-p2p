//! Messages between a home node and its own clients (ADR 0001): the local
//! command-line tool over a Unix socket, and paired phones over
//! `osp/client/0` (ADR 0008). Same framing as the social protocol.
//!
//! Each request gets one reply frame. Two requests also carry raw bytes on
//! the same stream, and are only accepted over `osp/client/0`: a `Post` with
//! `uploads` (the device sends the files after its request) and `ReadBlob`
//! (the node sends the bytes after its reply).

use std::path::PathBuf;

use iroh_base::{EndpointId, PublicKey};
use serde::{Deserialize, Serialize};

use crate::{
    object::ObjectId,
    types::{Attachment, BlobHash},
};

/// Largest request frame a node reads from a paired device, in bytes. The
/// biggest request is a `Post`, whose text is limited to 10,000 bytes.
pub const MAX_REQUEST_LEN: u32 = 256 * 1024;

/// Largest request frame a node reads from a device it doesn't know. Such a
/// device may only send `Pair`, which is tiny.
pub const MAX_PAIR_REQUEST_LEN: u32 = 4 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientRequest {
    Status,
    /// A follow ticket for this node's identity.
    Invite,
    Follow {
        ticket: String,
    },
    Unfollow {
        actor: PublicKey,
    },
    Following,
    /// Publish a post.
    ///
    /// `attachments` are files on the home node's machine, so they are
    /// accepted only from the local socket. `uploads` come from a device:
    /// their bytes follow this request on the same stream, back to back, in
    /// order. Either way the node transcodes them before publishing.
    Post {
        content: String,
        #[serde(default)]
        attachments: Vec<PathBuf>,
        #[serde(default)]
        uploads: Vec<Upload>,
    },
    Feed {
        #[serde(default = "default_feed_limit")]
        limit: u32,
    },
    Show {
        id: ObjectId,
    },
    /// Reshare a post; with `host`, keep a copy of its media and serve it.
    Reshare {
        id: ObjectId,
        #[serde(default)]
        host: bool,
    },
    /// Make sure a post's media is stored locally; optionally export it to a
    /// directory on the home node (local socket only).
    Fetch {
        id: ObjectId,
        #[serde(default)]
        out_dir: Option<PathBuf>,
    },
    /// Sync all follows now. From the local socket this also waits for
    /// prefetching to finish; a device gets its reply once outboxes are synced.
    Sync,
    /// Start pairing a new device (local socket only). The ticket works once,
    /// within a few minutes.
    NewPairing,
    /// List paired devices (local socket only).
    Devices,
    /// Forget a paired device and close its connections (local socket only).
    Unpair {
        device: EndpointId,
    },
    /// A new device's first request: the secret from a pairing ticket, and a
    /// name to show for the device (`osp/client/0` only).
    Pair {
        secret: String,
        name: String,
    },
    /// Read a blob the node has, fetching it first if it doesn't
    /// (`osp/client/0` only). The `Blob` reply is followed by the raw bytes
    /// from `offset`: `len` of them, or up to the end.
    ReadBlob {
        blob: BlobHash,
        #[serde(default)]
        offset: u64,
        #[serde(default)]
        len: Option<u64>,
    },
}

fn default_feed_limit() -> u32 {
    50
}

/// A file a device sends with a `Post`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Upload {
    /// The file's name on the device; only its extension is used.
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ClientResponse {
    Status(StatusInfo),
    Ticket {
        ticket: String,
    },
    Ok,
    Posted {
        id: ObjectId,
    },
    Following {
        follows: Vec<FollowInfo>,
    },
    Feed {
        items: Vec<FeedItem>,
    },
    Item {
        item: FeedItem,
    },
    Fetched {
        files: Vec<FetchedFile>,
    },
    Synced {
        new_objects: usize,
    },
    /// A pairing ticket for a new device.
    Pairing {
        ticket: String,
        expires_in_secs: u64,
    },
    Devices {
        devices: Vec<DeviceInfo>,
    },
    /// Pairing worked: whose node this is.
    Paired {
        identity: PublicKey,
        name: String,
    },
    /// The blob's total size; the requested bytes follow.
    Blob {
        size: u64,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusInfo {
    pub name: String,
    pub identity: PublicKey,
    pub node: EndpointId,
    pub relay: Option<String>,
    pub local_addrs: Vec<String>,
    pub follows: usize,
    pub followers: usize,
    pub storage: Vec<StorageInfo>,
    pub peers: Vec<PeerInfo>,
    /// `None` until the first network check finishes, or without relays.
    pub nat: Option<NatKind>,
    /// Version of the node software.
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub uptime_secs: u64,
    #[serde(default)]
    pub devices: usize,
    /// Free space on the disk that holds the node's home, if known.
    #[serde(default)]
    pub disk_free_bytes: Option<u64>,
    /// Things the owner should look at, in plain words. Empty when all is well.
    #[serde(default)]
    pub problems: Vec<String>,
}

/// How this node's network maps its outgoing UDP, which decides whether
/// other nodes can usually connect directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NatKind {
    /// The same public address towards every destination.
    Consistent,
    /// A different public address per destination, as behind many VPNs and
    /// carrier-grade NATs.
    VariesByDestination,
    /// No UDP gets out.
    NoUdp,
}

impl NatKind {
    pub fn describe(self) -> &'static str {
        match self {
            NatKind::Consistent => {
                "same public address for every peer; direct connections usually work"
            }
            NatKind::VariesByDestination => {
                "public address varies by destination (common behind VPNs); peers will mostly connect through the relay"
            }
            NatKind::NoUdp => "no UDP; every connection goes through the relay",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageInfo {
    /// `own`, `replica` or `cache` (ADR 0003).
    pub class: String,
    pub blobs: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub node: EndpointId,
    /// `direct`, `relay`, or `none` if the connection has no open path.
    pub path: String,
    pub rtt_ms: Option<u64>,
    pub direction: Direction,
    /// Whether the peer is one of this node's paired devices.
    #[serde(default)]
    pub device: bool,
}

/// Which side opened the live connections to a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Outgoing,
    Incoming,
    Both,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: EndpointId,
    pub name: Option<String>,
    /// When it was paired (RFC 3339).
    pub paired: String,
    /// Whether it has a live connection to the node right now.
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowInfo {
    pub actor: PublicKey,
    pub name: Option<String>,
    pub cursor: u64,
    pub last_sync: Option<String>,
    pub last_error: Option<String>,
}

/// A post as shown in a feed, with reshare context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    pub id: ObjectId,
    pub actor: PublicKey,
    pub actor_name: Option<String>,
    pub published: String,
    pub content: String,
    pub attachments: Vec<AttachmentInfo>,
    pub reshared_by: Option<ResharedBy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub attachment: Attachment,
    /// Whether this node already has the media.
    pub local: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResharedBy {
    pub actor: PublicKey,
    pub name: Option<String>,
    pub announce: ObjectId,
    pub published: String,
    pub hosting: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedFile {
    pub blob: BlobHash,
    pub media_type: String,
    pub size: u64,
    pub path: Option<PathBuf>,
}
