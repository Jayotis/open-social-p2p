//! Hosting resharers named by the author's node (ADR 0025, step 2).
//!
//! Jimmy follows nobody, finds Kim's video, and shares it with hosting. His
//! node tells Kim's (`Hosting`, with his `Announce` and profile), which
//! checks a piece of his copy and notes him apart from her followers' swarm.
//! If his profile says he may be listed, Kim's node names him to anyone who
//! asks who hosts the video: over Tor in `WhoHosts`, and on her media
//! endpoint, through the relay, with a small request of its own
//! (`MEDIA_HOSTS_ALPN`), which is how Alice's viewing endpoint asks (ADR 0024)
//! without showing who she is. A deleted post answers that request with Kim's
//! `Delete`.
//!
//! Kim's node checks each again once a day, and stops naming one after 2
//! failed checks in a row, or once his reshare is deleted. Jimmy's node tells
//! Kim's again once a day, so a message missed while her node was off gets
//! there, and she has his newest endpoint and listing choice.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use osp_proto::{
    BlobHash, BoundEndpoint, EndpointKind, Note, ObjectId, Payload, SignedObject,
    framing::{read_frame, read_frame_max, write_frame},
    social::{
        HostingReshare, MAX_REQUEST_LEN, MEDIA_HOSTS_ALPN, MediaHostsRequest, MediaHostsResponse,
        SocialRequest, SocialResponse,
    },
};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::{
    db::now_ms,
    node::Core,
    store::Source,
    sync::{jitter, random_fraction},
};

/// Most media-hosts requests served at once, from everyone together.
/// Viewing endpoints are new keys, so a limit per key would hold nobody back.
const MAX_REQUESTS: usize = 16;
/// Longest one request may take, reading it and replying together.
const STREAM_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a viewer waits for the author's media endpoint to answer before
/// going on with the sources it has.
const ASK_AUTHOR_WAIT: Duration = Duration::from_secs(10);
/// A resharer whose copy failed this many checks in a row is forgotten.
const FORGET_AFTER_FAILS: u32 = 7;
const HOUR: Duration = Duration::from_secs(60 * 60);

/// Answers `MEDIA_HOSTS_ALPN` on the media endpoint.
#[derive(Clone)]
pub(crate) struct MediaHostsProtocol {
    core: Arc<Core>,
    requests: Arc<Semaphore>,
}

impl MediaHostsProtocol {
    pub fn new(core: Arc<Core>) -> Self {
        Self {
            core,
            requests: Arc::new(Semaphore::new(MAX_REQUESTS)),
        }
    }
}

impl std::fmt::Debug for MediaHostsProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MediaHostsProtocol")
    }
}

impl ProtocolHandler for MediaHostsProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        // One request per connection: a viewer asks once.
        let Ok((mut send, mut recv)) = conn.accept_bi().await else {
            return Ok(());
        };
        let resp = match self.requests.clone().try_acquire_owned() {
            Ok(_permit) => {
                let answer = async {
                    let req: MediaHostsRequest = read_frame_max(&mut recv, MAX_REQUEST_LEN).await?;
                    self.core.answer_media_hosts(&req)
                };
                match tokio::time::timeout(STREAM_TIMEOUT, answer).await {
                    Ok(Ok(resp)) => resp,
                    Ok(Err(e)) => MediaHostsResponse::Error {
                        message: format!("{e:#}"),
                    },
                    Err(_) => return Ok(()),
                }
            }
            Err(_) => MediaHostsResponse::Error {
                message: "this node is busy; try again later".to_string(),
            },
        };
        let _ = tokio::time::timeout(STREAM_TIMEOUT, async {
            write_frame(&mut send, &resp).await?;
            send.finish()?;
            // Wait for the viewer to read it and close.
            conn.closed().await;
            anyhow::Ok(())
        })
        .await;
        Ok(())
    }
}

impl Core {
    /// Who hosts `req.blob`, of this node's post `req.post`, for anyone: this
    /// node's media endpoint and the hosting resharers it names, never a
    /// follower's copy. A deleted post is answered with its `Delete`.
    fn answer_media_hosts(&self, req: &MediaHostsRequest) -> Result<MediaHostsResponse> {
        let me = self.signer.identity();
        if let Some(delete) = self.db.delete_of(&req.post, &me)? {
            return Ok(MediaHostsResponse::Deleted { delete });
        }
        let has_blob = match self.db.outbox_object(&req.post)?.map(|o| o.verify()) {
            Some(Ok(Payload::Note(note))) => note.blobs().any(|b| b == req.blob),
            _ => false,
        };
        if !has_blob {
            bail!("that isn't media of one of this node's posts");
        }
        let mut hosts = Vec::new();
        if let Some(media) = self.endpoints.as_ref().and_then(|e| e.media.as_ref()) {
            hosts.push(BoundEndpoint {
                binding: media.binding.clone(),
                addr: EndpointAddr::new(media.endpoint.id()),
            });
        }
        hosts.extend(self.resharers_for_viewers(&req.blob)?);
        Ok(MediaHostsResponse::Hosts { hosts })
    }

    /// Asks the author's media endpoint, from a viewing endpoint, who hosts
    /// `blob` of `post` (ADR 0025). The hosts it names, their bindings
    /// checked; an error if the post was deleted, once its `Delete` is kept
    /// here. Nothing if it can't be reached in time.
    pub(crate) async fn ask_author_for_hosts(
        &self,
        viewing: &Endpoint,
        author_media: EndpointAddr,
        (post, note): (&ObjectId, &Note),
        blob: &BlobHash,
    ) -> Result<Vec<Source>> {
        let ask = async {
            let conn = viewing.connect(author_media, MEDIA_HOSTS_ALPN).await?;
            let (mut send, mut recv) = conn.open_bi().await?;
            let req = MediaHostsRequest {
                post: *post,
                blob: *blob,
            };
            write_frame(&mut send, &req).await?;
            send.finish()?;
            let resp: MediaHostsResponse = read_frame(&mut recv).await?;
            conn.close(0u32.into(), b"done");
            anyhow::Ok(resp)
        };
        let resp = match tokio::time::timeout(ASK_AUTHOR_WAIT, ask).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                debug!("couldn't ask the author's media endpoint who hosts it: {e:#}");
                return Ok(Vec::new());
            }
            Err(_) => {
                debug!("the author's media endpoint didn't say who hosts it in time");
                return Ok(Vec::new());
            }
        };
        match resp {
            MediaHostsResponse::Hosts { hosts } => Ok(hosts
                .iter()
                .filter(|h| h.check(&h.binding.public, EndpointKind::Media).is_ok())
                .map(|h| Source {
                    node: h.binding.public,
                    endpoint: h.binding.endpoint,
                    from_direct: false,
                })
                .collect()),
            MediaHostsResponse::Deleted { delete } => {
                self.found_deleted(&delete, post, note).await?;
                bail!("its author deleted it")
            }
            MediaHostsResponse::Error { message } => {
                debug!("the author's media endpoint replied: {message}");
                Ok(Vec::new())
            }
        }
    }

    /// Acts on a `Delete` of a found post that its author's media endpoint
    /// sent, once it checks out, so the post goes and its tombstone stays.
    async fn found_deleted(
        &self,
        delete: &SignedObject,
        post: &ObjectId,
        note: &Note,
    ) -> Result<()> {
        let Ok(Payload::Delete(d)) = delete.verify() else {
            bail!("the author's media endpoint sent a delete that doesn't check out");
        };
        if delete.signer() != note.actor || d.object != *post {
            bail!("the author's media endpoint sent a delete of something else");
        }
        let mut forwards = Vec::new();
        self.act_on_delete(delete, &d, true, &mut forwards).await
    }

    /// Tells the author's node of each of this node's hosting reshares of
    /// others' posts (ADR 0025), with its `Announce` and the owner's profile.
    /// Only with a media endpoint, which is how it'd be named. Returns how
    /// many were told.
    pub(crate) async fn tell_authors_hosting(&self) -> Result<usize> {
        if self
            .endpoints
            .as_ref()
            .and_then(|e| e.media.as_ref())
            .is_none()
        {
            return Ok(0);
        }
        let me = self.signer.identity();
        let mut told = 0;
        for (_, obj) in self.db.outbox_since(0, u32::MAX)? {
            let Ok(Payload::Announce(a)) = obj.verify() else {
                continue;
            };
            if !a.hosting || a.object_actor == me {
                continue;
            }
            match self.tell_author_hosting(&obj).await {
                Ok(()) => told += 1,
                Err(e) => {
                    debug!(post = %a.object.fmt_short(), "couldn't tell its author's node we host it: {e:#}")
                }
            }
        }
        Ok(told)
    }

    /// Tells the node of the author of the post `announce` reshares that
    /// this node hosts a copy.
    pub(crate) async fn tell_author_hosting(&self, announce: &SignedObject) -> Result<()> {
        let Payload::Announce(a) = announce.verify()? else {
            bail!("not a reshare");
        };
        let note = self
            .stored_note(&a.object)?
            .context("the reshared post isn't here")?;
        let blob = note.blobs().next().context("the post has no media")?;
        let node = *self
            .nodes_of(&a.object_actor)
            .first()
            .context("no node known for the post's author")?;
        let req = SocialRequest::Hosting {
            blob,
            reshare: Some(Box::new(HostingReshare {
                announce: announce.clone(),
                profile: self.profile.clone(),
            })),
        };
        match self.social_request(&EndpointAddr::new(node), &req).await? {
            SocialResponse::Ok => Ok(()),
            other => bail!("unexpected reply to Hosting: {other:?}"),
        }
    }

    /// Checks again each hosting resharer last checked over `every` ago: asks
    /// its node whether its reshare was deleted, then fetches a piece of its
    /// copy. Returns how many were checked.
    pub(crate) async fn recheck_resharers(&self, every: Duration) -> Result<usize> {
        let now = now_ms();
        let due = self
            .db
            .resharer_rechecks_due(now - every.as_millis() as i64)?;
        let mut checked = 0;
        for (host, blob) in due {
            let who = host.actor.fmt_short();
            if self
                .reshare_deleted(&host.node, &host.announce, &host.actor)
                .await
            {
                info!(resharer = %who, "a hosting resharer deleted its reshare; no longer named");
                self.db
                    .forget_resharer_hosts(&host.post, Some(&host.node))?;
                continue;
            }
            let size = self
                .stored_note(&host.post)?
                .and_then(|n| n.sized_blobs().find_map(|(b, s)| (b == blob).then_some(s)));
            let Some(size) = size else {
                self.db.forget_resharer_hosts(&host.post, None)?;
                continue;
            };
            let source = Source {
                node: host.node,
                endpoint: host.media.endpoint,
                from_direct: false,
            };
            let passed = match self.store.check_piece(&blob, size, source).await {
                Ok(()) => true,
                Err(e) => {
                    debug!(resharer = %who, "a hosting resharer's copy failed its recheck: {e:#}");
                    false
                }
            };
            self.db
                .note_resharer_check(&host.post, &host.node, passed, now_ms())?;
            if !passed && host.fails + 1 >= FORGET_AFTER_FAILS {
                self.db
                    .forget_resharer_hosts(&host.post, Some(&host.node))?;
            } else if !passed && host.fails + 1 == crate::social::RESHARER_FAILS_UNNAMED {
                info!(resharer = %who, "a hosting resharer's copy failed 2 checks in a row; no longer named");
            }
            checked += 1;
        }
        Ok(checked)
    }

    /// Whether `node` answers that `announce`, by `actor`, was deleted.
    /// Can't ask, or no answer: not deleted, as far as this node knows.
    async fn reshare_deleted(
        &self,
        node: &iroh::EndpointId,
        announce: &ObjectId,
        actor: &iroh::PublicKey,
    ) -> bool {
        let req = SocialRequest::GetObjects {
            ids: vec![*announce],
        };
        let Ok(SocialResponse::Objects { objects }) =
            self.social_request(&EndpointAddr::new(*node), &req).await
        else {
            return false;
        };
        objects.iter().any(|obj| {
            obj.signer() == *actor
                && matches!(obj.verify(), Ok(Payload::Delete(d)) if d.object == *announce)
        })
    }
}

/// Checks hosting resharers again, about once a day (ADR 0025).
pub(crate) async fn resharers_loop(core: Arc<Core>) {
    let every = core.sim.reshare_recheck_every;
    let tick = every.min(HOUR);
    tokio::time::sleep(tick.mul_f64(random_fraction())).await;
    loop {
        if let Some(tor) = &core.tor {
            tor.ready().await;
        }
        if let Err(e) = core.recheck_resharers(every).await {
            warn!("checking hosting resharers again failed: {e:#}");
        }
        tokio::time::sleep(jitter(tick)).await;
    }
}
