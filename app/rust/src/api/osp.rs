//! Everything the Flutter app calls. Each function wraps `osp-client` and
//! turns its wire types into plain structs that flutter_rust_bridge can
//! hand to Dart. Byte counts are `i64` because Dart has no unsigned 64-bit
//! integers.

use std::{path::PathBuf, sync::Arc};

use anyhow::{bail, Result};
use flutter_rust_bridge::frb;
use osp_client::{
    device_key_from_hex,
    osp_proto::{
        rpc::{ClientRequest, ClientResponse, FeedItem},
        BlobHash, MediaKind, ObjectId,
    },
    Client, ClientOptions, Pairing,
};

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
}

pub struct Post {
    pub id: String,
    pub author: String,
    /// RFC 3339.
    pub published: String,
    pub text: String,
    pub media: Vec<Media>,
    /// Who reshared it, if this is a reshare.
    pub reshared_by: Option<String>,
    /// Whether the resharer keeps and serves a copy of its media.
    pub resharer_hosts: bool,
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

/// The feed as last fetched, for when the node can't be reached.
pub struct SavedFeed {
    pub posts: Vec<Post>,
    /// RFC 3339.
    pub saved: String,
}

#[derive(Clone, Copy)]
pub enum PostStage {
    /// Sending files to the node.
    Sending,
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
        Ok(OspApp { client, dir })
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
        })
    }

    pub async fn feed(&self, limit: u32) -> Result<Vec<Post>> {
        Ok(self
            .client
            .feed(limit)
            .await?
            .into_iter()
            .map(post)
            .collect())
    }

    #[frb(sync)]
    pub fn saved_feed(&self) -> Option<SavedFeed> {
        self.client.cached_feed().map(|f| SavedFeed {
            posts: f.items.into_iter().map(post).collect(),
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
    pub async fn post(
        &self,
        text: String,
        files: Vec<String>,
        sink: StreamSink<PostProgress>,
    ) -> Result<()> {
        let files: Vec<PathBuf> = files.into_iter().map(PathBuf::from).collect();
        let progress = sink.clone();
        let result = self
            .client
            .post(text, &files, move |sent, total| {
                let stage = if sent < total {
                    PostStage::Sending
                } else {
                    PostStage::Processing
                };
                let _ = progress.add(PostProgress {
                    stage,
                    sent: sent as i64,
                    total: total as i64,
                    message: None,
                });
            })
            .await;
        let last = match result {
            Ok(id) => PostProgress {
                stage: PostStage::Done,
                sent: 0,
                total: 0,
                message: Some(id.to_string()),
            },
            Err(e) => {
                tracing::warn!("post failed: {e:#}");
                PostProgress {
                    stage: PostStage::Failed,
                    sent: 0,
                    total: 0,
                    message: Some(format!("{e:#}")),
                }
            }
        };
        let _ = sink.add(last);
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

    /// The node owner's follow ticket, to show as a QR code.
    pub async fn invite(&self) -> Result<String> {
        match self.client.request(&ClientRequest::Invite).await? {
            ClientResponse::Ticket { ticket } => Ok(ticket),
            _ => bail!("unexpected reply to Invite"),
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

    /// Closes the connection while the app is in the background.
    pub async fn disconnect(&self) {
        self.client.disconnect().await;
    }

    /// The end of the app's log, for the status screen.
    #[frb(sync)]
    pub fn recent_log(&self, lines: u32) -> String {
        logging::tail(&self.dir, lines as usize)
    }
}

fn node_info(p: Pairing) -> NodeInfo {
    NodeInfo {
        name: p.name,
        identity: p.identity.to_string(),
        paired: p.paired,
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
        None => (None, false),
    };
    Post {
        id: item.id.to_string(),
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
    }
}

/// A new device key, as 64 hex digits, for the app to keep in the phone's
/// secure storage.
#[frb(sync)]
pub fn new_device_key() -> String {
    osp_client::new_device_key()
}

/// Makes sure a ticket looks like a pairing ticket before trying to pair.
#[frb(sync)]
pub fn is_pairing_ticket(text: String) -> bool {
    text.trim().to_ascii_uppercase().starts_with("OSPPAIR1")
}

/// Whether text looks like a follow ticket.
#[frb(sync)]
pub fn is_follow_ticket(text: String) -> bool {
    text.trim().to_ascii_uppercase().starts_with("OSP1")
}
