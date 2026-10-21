//! Commands from the node's own clients: the local `osp` tool (Unix socket)
//! and, from milestone M2, paired phones.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use iroh::EndpointId;
use osp_proto::{
    Announce, Attachment, BlobRef, FollowTicket, Note, ObjectId, Payload, SignedObject,
    framing::{read_frame, write_frame},
    rpc::{
        AttachmentInfo, ClientRequest, ClientResponse, FeedItem, FetchedFile, PeerInfo, ResharedBy,
        StatusInfo,
    },
    social::SocialRequest,
};
use tokio::{
    net::{UnixListener, UnixStream},
    task::JoinHandle,
};
use tracing::{debug, warn};

use crate::{db::BlobClass, media, node::Core, node::remember};

/// Longest post text accepted, in bytes. Keeps outbox pages well under the frame limit.
const MAX_CONTENT_BYTES: usize = 10_000;

/// Where a command came from. Some commands touch the home node's own
/// filesystem and are only allowed locally.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Origin {
    Local,
    #[allow(dead_code)] // used from milestone M2
    Device(EndpointId),
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
            } => {
                ensure!(
                    local || attachments.is_empty(),
                    "file paths are only accepted from the local socket"
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
            ClientRequest::Sync => ClientResponse::Synced {
                new_objects: self.sync_all(true).await?,
            },
        })
    }

    async fn status(&self) -> Result<StatusInfo> {
        let addr = self.endpoint.addr();
        let peers = self
            .live_conns()
            .iter()
            .map(|conn| {
                let paths = conn.paths();
                let selected = paths.iter().find(|p| p.is_selected());
                PeerInfo {
                    node: conn.remote_id(),
                    path: match &selected {
                        Some(p) if p.is_relay() => "relay",
                        Some(p) if p.is_ip() => "direct",
                        _ => "none",
                    }
                    .to_string(),
                    rtt_ms: selected.map(|p| p.rtt().as_millis() as u64),
                }
            })
            .collect();
        Ok(StatusInfo {
            name: self.config.name.clone(),
            identity: self.identity.public(),
            node: self.endpoint.id(),
            relay: addr.relay_urls().next().map(|u| u.to_string()),
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
        })
    }

    async fn follow(&self, ticket: &FollowTicket) -> Result<()> {
        ensure!(
            ticket.actor != self.identity.public(),
            "you can't follow yourself"
        );
        remember(&self.lookup, &ticket.node);
        self.db.upsert_follow(&ticket.actor, &ticket.node)?;
        // Ask to be told about new posts. Best effort: polling covers us otherwise.
        let me = self.shareable_addr().await;
        if let Err(e) = self
            .social_request(&ticket.node, &SocialRequest::Subscribe { addr: me })
            .await
        {
            warn!("subscribing to {} failed: {e:#}", ticket.actor.fmt_short());
        }
        self.sync_now.notify_one();
        Ok(())
    }

    async fn post(self: &Arc<Self>, content: String, paths: &[PathBuf]) -> Result<ObjectId> {
        ensure!(
            !content.trim().is_empty() || !paths.is_empty(),
            "a post needs text or an attachment"
        );
        ensure!(
            content.len() <= MAX_CONTENT_BYTES,
            "post text is longer than {MAX_CONTENT_BYTES} bytes"
        );
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

    /// Transcodes a file and stores the rendition and thumbnail as our own blobs.
    async fn prepare_media(&self, path: &Path) -> Result<Attachment> {
        let r = media::transcode(&self.config.media, path, &self.home.work()).await?;
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
            for blob in note.blobs() {
                self.ensure_blob(&blob, BlobClass::Replica).await?;
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

    async fn notify_followers(&self, seq: u64) {
        let req = SocialRequest::Notify {
            actor: self.identity.public(),
            seq,
        };
        for addr in self.db.followers().unwrap_or_default() {
            let sent =
                tokio::time::timeout(Duration::from_secs(15), self.social_request(&addr, &req))
                    .await;
            if !matches!(sent, Ok(Ok(_))) {
                debug!(node = %addr.id.fmt_short(), "follower not reachable");
            }
        }
    }

    async fn fetch(&self, id: &ObjectId, out_dir: Option<&Path>) -> Result<Vec<FetchedFile>> {
        let (_, note) = self.resolve_note(id)?;
        let mut files = Vec::new();
        for a in &note.attachment {
            let size = self.ensure_blob(&a.blob, BlobClass::Cache).await?;
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
        }
    }
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
