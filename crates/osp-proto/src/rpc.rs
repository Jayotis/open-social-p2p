//! Messages between a home node and its own clients (ADR 0001): the local
//! command-line tool over a Unix socket, and later paired phones over
//! `osp/client/0`. Same framing as the social protocol.

use std::path::PathBuf;

use iroh_base::{EndpointId, PublicKey};
use serde::{Deserialize, Serialize};

use crate::{
    object::ObjectId,
    types::{Attachment, BlobHash},
};

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
    /// Publish a post. Attachment paths are files on the home node's machine,
    /// so they are accepted only from the local socket, never from a device.
    Post {
        content: String,
        #[serde(default)]
        attachments: Vec<PathBuf>,
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
    /// Sync all follows now and wait for prefetching to finish.
    Sync,
}

fn default_feed_limit() -> u32 {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ClientResponse {
    Status(StatusInfo),
    Ticket { ticket: String },
    Ok,
    Posted { id: ObjectId },
    Following { follows: Vec<FollowInfo> },
    Feed { items: Vec<FeedItem> },
    Item { item: FeedItem },
    Fetched { files: Vec<FetchedFile> },
    Synced { new_objects: usize },
    Error { message: String },
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
