//! Commands from the node's own clients: the local `osp` tool (Unix socket)
//! and paired phones (`osp/client/0`, see `client.rs`).

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use n0_future::{BufferedStreamExt, StreamExt, stream};
use osp_proto::{
    Announce, Attachment, BlobRef, FollowTicket, Note, ObjectId, Payload, SignedObject,
    framing::{read_frame, write_frame},
    rpc::{
        AttachmentInfo, ClientRequest, ClientResponse, FeedItem, FetchedFile, ResharedBy,
        StatusInfo,
    },
    social::SocialRequest,
    types::{MAX_ATTACHMENTS, MAX_CONTENT_BYTES},
};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::SemaphorePermit,
    task::JoinHandle,
};
use tracing::{debug, info, warn};

use crate::{
    config::RelayChoice,
    db::{BlobClass, now_ms},
    health, media, network,
    node::{Core, check_addr, remember},
    social::FOLLOWER_SILENCE,
};

/// Followers notified at once after a post.
const NOTIFY_AT_ONCE: usize = 16;
/// Give up on notifying one follower after this long.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(15);
/// How often followers who haven't heard that we moved are told again.
const TELL_MOVED_EVERY: Duration = Duration::from_secs(60 * 60);
/// Most posts with media in progress at once: arriving from a phone,
/// waiting for their turn at ffmpeg, or transcoding. More are refused
/// until one is done.
pub(crate) const MAX_MEDIA_POSTS: usize = 4;

/// Where a command came from. Commands that touch the home node's own
/// filesystem, or manage devices, are only allowed locally.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Origin {
    Local,
    /// A paired device over `osp/client/0`.
    Device,
}

impl Core {
    pub(crate) async fn handle_client(
        self: &Arc<Self>,
        req: ClientRequest,
        origin: Origin,
    ) -> ClientResponse {
        match self.client_request(req, origin).await {
            Ok(resp) => resp,
            Err(e) => ClientResponse::Error {
                message: format!("{e:#}"),
            },
        }
    }

    async fn client_request(
        self: &Arc<Self>,
        req: ClientRequest,
        origin: Origin,
    ) -> Result<ClientResponse> {
        let local = matches!(origin, Origin::Local);
        Ok(match req {
            ClientRequest::Status => ClientResponse::Status(self.status().await?),
            ClientRequest::Invite => ClientResponse::Ticket {
                ticket: FollowTicket {
                    actor: self.identity.public(),
                    node: self.shareable_addr().await,
                }
                .to_string(),
            },
            ClientRequest::Follow { ticket } => {
                self.follow(&ticket.parse()?).await?;
                ClientResponse::Ok
            }
            ClientRequest::Unfollow { actor } => {
                ensure!(self.db.remove_follow(&actor)?, "not following {actor}");
                ClientResponse::Ok
            }
            ClientRequest::Following => ClientResponse::Following {
                follows: self.db.follow_infos()?,
            },
            ClientRequest::Post {
                content,
                attachments,
                uploads,
            } => {
                ensure!(
                    local || attachments.is_empty(),
                    "file paths are only accepted from the local socket"
                );
                ensure!(
                    uploads.is_empty(),
                    "uploads are only accepted over osp/client/0, without file paths"
                );
                ClientResponse::Posted {
                    id: self.post(content, &attachments).await?,
                }
            }
            ClientRequest::Feed { limit } => ClientResponse::Feed {
                items: self.feed(limit.min(500)).await?,
            },
            ClientRequest::Show { id } => ClientResponse::Item {
                item: self.show(&id).await?,
            },
            ClientRequest::Reshare { id, host } => ClientResponse::Posted {
                id: self.reshare(&id, host).await?,
            },
            ClientRequest::Fetch { id, out_dir } => {
                ensure!(
                    local || out_dir.is_none(),
                    "exporting files is only allowed from the local socket"
                );
                ClientResponse::Fetched {
                    files: self.fetch(&id, out_dir.as_deref()).await?,
                }
            }
            // A phone doesn't wait for prefetching: it can read media as soon as it's needed.
            ClientRequest::Sync => ClientResponse::Synced {
                new_objects: self.sync_all(local).await?,
            },
            ClientRequest::NewPairing => {
                ensure!(local, "devices can only be paired from the node itself");
                self.new_pairing().await?
            }
            ClientRequest::Devices => {
                ensure!(local, "devices can only be listed from the node itself");
                ClientResponse::Devices {
                    devices: self.devices()?,
                }
            }
            ClientRequest::Unpair { device } => {
                ensure!(local, "devices can only be unpaired from the node itself");
                self.unpair(&device)?;
                ClientResponse::Ok
            }
            ClientRequest::ExportBackup { path, media } => {
                ensure!(local, "backups are only made from the node itself");
                ClientResponse::BackedUp {
                    info: self.export_backup(&path, media).await?,
                    path,
                }
            }
            ClientRequest::Pair { .. } | ClientRequest::ReadBlob { .. } => {
                bail!("this request is only accepted over osp/client/0")
            }
        })
    }

    async fn status(&self) -> Result<StatusInfo> {
        let addr = self.endpoint.addr();
        let nat = match self.config.network.relay {
            RelayChoice::Disabled => None,
            _ => network::nat_kind(&self.endpoint),
        };
        let relay = addr.relay_urls().next().map(|u| u.to_string());
        let disk_free_bytes = health::disk_free(self.home.root());
        let mut peers = self.tracker.peers();
        for peer in &mut peers {
            peer.device = self.db.is_device(&peer.node)?;
        }
        Ok(StatusInfo {
            name: self.config.name.clone(),
            identity: self.identity.public(),
            node: self.endpoint.id(),
            problems: self.problems(relay.as_deref(), disk_free_bytes)?,
            relay,
            local_addrs: self
                .endpoint
                .bound_sockets()
                .iter()
                .map(|a| a.to_string())
                .collect(),
            follows: self.db.follows()?.len(),
            followers: self.db.follower_count()?,
            storage: self.db.storage_summary()?,
            peers,
            nat,
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: self.started.elapsed().as_secs(),
            devices: self.db.devices()?.len(),
            disk_free_bytes,
        })
    }

    async fn follow(&self, ticket: &FollowTicket) -> Result<()> {
        ensure!(
            ticket.actor != self.identity.public(),
            "you can't follow yourself"
        );
        check_addr(&ticket.node)?;
        remember(&self.lookup, &ticket.node);
        self.db.upsert_follow(&ticket.actor, &ticket.node)?;
        // The sync this wakes also subscribes, so we're told about new posts.
        self.lock_subscribed().remove(&ticket.actor);
        self.sync_now.notify_one();
        Ok(())
    }

    pub(crate) async fn post(
        self: &Arc<Self>,
        content: String,
        paths: &[PathBuf],
    ) -> Result<ObjectId> {
        check_post(&content, paths.len())?;
        let _slot = if paths.is_empty() {
            None
        } else {
            Some(self.media_post_slot()?)
        };
        self.publish_post(content, paths).await
    }

    /// One of the `MAX_MEDIA_POSTS` places for posts with media, kept until
    /// the post is published or fails.
    pub(crate) fn media_post_slot(&self) -> Result<SemaphorePermit<'_>> {
        self.media_posts.try_acquire().map_err(|_| {
            anyhow::anyhow!(
                "the home node is busy preparing other posts; try again in a few minutes"
            )
        })
    }

    /// Transcodes the files and publishes the post. The caller has checked
    /// the post with `check_post` and, if it has files, holds a media slot.
    pub(crate) async fn publish_post(
        self: &Arc<Self>,
        content: String,
        paths: &[PathBuf],
    ) -> Result<ObjectId> {
        let mut attachment = Vec::new();
        for path in paths {
            attachment.push(self.prepare_media(path).await?);
        }
        self.publish(Payload::Note(Note {
            actor: self.identity.public(),
            content,
            published: osp_proto::now_rfc3339(),
            attachment,
            in_reply_to: None,
        }))
        .await
    }

    /// Transcodes a file, once it's this post's turn, and stores the
    /// rendition and thumbnail as our own blobs.
    async fn prepare_media(&self, path: &Path) -> Result<Attachment> {
        let r = {
            let _turn = self.transcode_lock.lock().await;
            media::transcode(&self.config.media, path, &self.home.work()).await?
        };
        let (blob, size) = self.store.import(&r.file, BlobClass::Own).await?;
        let (thumb, thumb_size) = self.store.import(&r.thumbnail, BlobClass::Own).await?;
        if let Some(job_dir) = r.file.parent() {
            let _ = std::fs::remove_dir_all(job_dir);
        }
        Ok(Attachment {
            kind: r.kind,
            media_type: r.media_type.to_string(),
            blob,
            size,
            width: r.width,
            height: r.height,
            duration_ms: r.duration_ms,
            name: None,
            thumbnail: Some(BlobRef {
                blob: thumb,
                size: thumb_size,
                media_type: "image/webp".to_string(),
            }),
            profile: media::PROFILE.to_string(),
        })
    }

    async fn reshare(self: &Arc<Self>, id: &ObjectId, host: bool) -> Result<ObjectId> {
        let (note_id, note) = self.resolve_note(id)?;
        if host {
            // Check the whole post fits before fetching any of it.
            let mut needed = 0;
            for (blob, size) in note.sized_blobs() {
                if !self.db.blob_class(&blob)?.is_some_and(|c| c.is_public()) {
                    needed += size;
                }
            }
            self.store.check_replica_room(needed)?;
            for (blob, size) in note.sized_blobs() {
                self.ensure_blob(&blob, BlobClass::Replica, size).await?;
            }
        }
        self.publish(Payload::Announce(Announce {
            actor: self.identity.public(),
            object: note_id,
            object_actor: note.actor,
            published: osp_proto::now_rfc3339(),
            hosting: host,
        }))
        .await
    }

    /// Signs, stores and appends to our outbox, then tells followers.
    async fn publish(self: &Arc<Self>, payload: Payload) -> Result<ObjectId> {
        let obj = SignedObject::sign(&self.identity, &payload)?;
        self.db.insert_object(&obj, &payload)?;
        let seq = self.db.append_outbox(&obj.id())?;
        let core = self.clone();
        tokio::spawn(async move { core.notify_followers(seq).await });
        Ok(obj.id())
    }

    fn forget_silent_followers(&self) {
        let silent_since = now_ms() - FOLLOWER_SILENCE.as_millis() as i64;
        match self.db.drop_silent_followers(silent_since) {
            Ok(0) => {}
            Ok(n) => info!(
                followers = n,
                "forgot followers not heard from in a long time"
            ),
            Err(e) => warn!("dropping silent followers: {e:#}"),
        }
    }

    /// Tells followers who haven't heard yet that this identity moved to
    /// this node (ADR 0009). Returns how many are still to be told.
    async fn tell_moved(&self) -> usize {
        // The startup loop and a new post can both get here; one at a time,
        // so each follower is told once.
        let _guard = self.tell_moved_lock.lock().await;
        self.forget_silent_followers();
        let to_tell = self.db.followers_to_tell_moved().unwrap_or_default();
        if to_tell.is_empty() {
            return 0;
        }
        let req = &SocialRequest::Moved {
            profile: self.profile.clone(),
            addr: self.shareable_addr().await,
        };
        let count = to_tell.len();
        let told: Vec<bool> = stream::iter(to_tell)
            .map(|addr| async move {
                let sent =
                    tokio::time::timeout(NOTIFY_TIMEOUT, self.social_request(&addr, req)).await;
                match sent {
                    Ok(Ok(_)) => self.db.told_moved(&addr.id).is_ok(),
                    Ok(Err(e)) => {
                        debug!(node = %addr.id.fmt_short(), "follower refused the move: {e:#}");
                        false
                    }
                    Err(_) => false,
                }
            })
            .buffered_unordered(NOTIFY_AT_ONCE)
            .collect()
            .await;
        let left = told.iter().filter(|told| !**told).count();
        info!(
            told = count - left,
            left, "told followers this node took over the identity"
        );
        left
    }

    async fn notify_followers(&self, seq: u64) {
        // Followers who don't know we moved would refuse the notice.
        self.tell_moved().await;
        let req = &SocialRequest::Notify {
            actor: self.identity.public(),
            seq,
        };
        // Several at once, so followers that don't answer don't hold up the rest.
        stream::iter(self.db.followers().unwrap_or_default())
            .map(|addr| async move {
                let sent =
                    tokio::time::timeout(NOTIFY_TIMEOUT, self.social_request(&addr, req)).await;
                if matches!(sent, Ok(Ok(_))) {
                    self.db.follower_seen(&addr.id).ok();
                } else {
                    debug!(node = %addr.id.fmt_short(), "follower not reachable");
                }
            })
            .buffered_unordered(NOTIFY_AT_ONCE)
            .for_each(|_| {})
            .await;
    }

    async fn fetch(&self, id: &ObjectId, out_dir: Option<&Path>) -> Result<Vec<FetchedFile>> {
        let (_, note) = self.resolve_note(id)?;
        let mut files = Vec::new();
        for a in &note.attachment {
            let size = self.ensure_blob(&a.blob, BlobClass::Cache, a.size).await?;
            let path = match out_dir {
                Some(dir) => {
                    std::fs::create_dir_all(dir)
                        .with_context(|| format!("creating {}", dir.display()))?;
                    let path = dir.join(format!("{}.{}", a.blob, extension(&a.media_type)));
                    self.store.export(&a.blob, &path).await?;
                    Some(path)
                }
                None => None,
            };
            files.push(FetchedFile {
                blob: a.blob,
                media_type: a.media_type.clone(),
                size,
                path,
            });
        }
        Ok(files)
    }

    async fn feed(&self, limit: u32) -> Result<Vec<FeedItem>> {
        let mut actors = vec![self.identity.public()];
        actors.extend(self.db.follows()?.into_iter().map(|f| f.actor));
        let mut items = Vec::new();
        for obj in self.db.feed(&actors, limit)? {
            if let Some(item) = self.feed_item(&obj).await? {
                items.push(item);
            }
        }
        Ok(items)
    }

    async fn show(&self, id: &ObjectId) -> Result<FeedItem> {
        let obj = self
            .db
            .get_object(id)?
            .with_context(|| format!("unknown object {id}"))?;
        self.feed_item(&obj)
            .await?
            .context("object is not a post or its original isn't available")
    }

    /// Turns a stored Note or Announce into a feed item; None if it can't be shown.
    async fn feed_item(&self, obj: &SignedObject) -> Result<Option<FeedItem>> {
        let (id, note, reshared_by) = match obj.verify() {
            Ok(Payload::Note(note)) => (obj.id(), note, None),
            Ok(Payload::Announce(a)) => {
                let Some(note) = self.stored_note(&a.object)? else {
                    return Ok(None);
                };
                let by = ResharedBy {
                    actor: a.actor,
                    name: self.db.profile_name(&a.actor)?,
                    announce: obj.id(),
                    published: a.published,
                    hosting: a.hosting,
                };
                (a.object, note, Some(by))
            }
            _ => return Ok(None),
        };
        let mut attachments = Vec::new();
        for a in note.attachment {
            attachments.push(AttachmentInfo {
                local: self.store.has(&a.blob).await?,
                attachment: a,
            });
        }
        Ok(Some(FeedItem {
            id,
            actor: note.actor,
            actor_name: self.db.profile_name(&note.actor)?,
            published: note.published,
            content: note.content,
            attachments,
            reshared_by,
        }))
    }

    /// The post an ID refers to: the Note itself, or the Note an Announce reshares.
    fn resolve_note(&self, id: &ObjectId) -> Result<(ObjectId, Note)> {
        let obj = self
            .db
            .get_object(id)?
            .with_context(|| format!("unknown object {id}"))?;
        match obj.verify()? {
            Payload::Note(note) => Ok((*id, note)),
            Payload::Announce(a) => {
                let note = self
                    .stored_note(&a.object)?
                    .context("the reshared post isn't available on this node")?;
                Ok((a.object, note))
            }
            Payload::Person(_) => bail!("{id} is a profile, not a post"),
            Payload::Key(_) => bail!("{id} is a key certificate, not a post"),
        }
    }
}

/// Refuses a post that is empty, too long or has too many attachments,
/// before any of its files are received or transcoded.
pub(crate) fn check_post(content: &str, attachments: usize) -> Result<()> {
    ensure!(
        !content.trim().is_empty() || attachments > 0,
        "a post needs text or an attachment"
    );
    ensure!(
        content.len() <= MAX_CONTENT_BYTES,
        "post text is longer than {MAX_CONTENT_BYTES} bytes"
    );
    ensure!(
        attachments <= MAX_ATTACHMENTS,
        "a post can have at most {MAX_ATTACHMENTS} attachments"
    );
    Ok(())
}

fn extension(media_type: &str) -> &str {
    match media_type {
        "video/mp4" => "mp4",
        "image/webp" => "webp",
        _ => "bin",
    }
}

/// Serves the local command-line tool. The socket lives in the node's home
/// directory (mode 0700) and is itself readable only by the owner.
/// After this node takes an identity over, tells its followers at startup
/// and then every hour until each has heard, or been forgotten as silent.
/// New posts tell them too (`notify_followers`).
pub(crate) async fn tell_moved_loop(core: Arc<Core>) {
    while core.tell_moved().await > 0 {
        tokio::time::sleep(TELL_MOVED_EVERY).await;
    }
}

pub(crate) async fn serve_unix(core: Arc<Core>, path: PathBuf) -> Result<JoinHandle<()>> {
    if path.exists() {
        if UnixStream::connect(&path).await.is_ok() {
            bail!("another node is already running here ({})", path.display());
        }
        std::fs::remove_file(&path)?;
    }
    let listener =
        UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(tokio::spawn(async move {
        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    warn!("local socket: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            };
            let core = core.clone();
            tokio::spawn(async move {
                let (mut reader, mut writer) = stream.into_split();
                let Ok(req) = read_frame::<_, ClientRequest>(&mut reader).await else {
                    return;
                };
                let resp = core.handle_client(req, Origin::Local).await;
                let _ = write_frame(&mut writer, &resp).await;
            });
        }
    }))
}
