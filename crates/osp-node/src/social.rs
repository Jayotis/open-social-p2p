//! The node-to-node protocol `osp/social/0` (ADR 0003): serving our profile
//! and outbox, and asking other nodes for theirs.

use std::{fmt, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use iroh::{
    EndpointAddr, EndpointId,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use osp_proto::{
    Payload, SOCIAL_ALPN, SignedObject,
    framing::{read_frame, read_frame_max, write_frame},
    social::{MAX_OUTBOX_PAGE, MAX_REQUEST_LEN, OutboxEntry, SocialRequest, SocialResponse},
};
use tokio::sync::Semaphore;
use tracing::debug;

use crate::{
    db::now_ms,
    node::{Core, check_addr, remember},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Most requests served at once, from all peers together. Anyone who knows
/// the node's ID can connect, so this keeps a flood from using up the
/// machine. Requests over a limit get a "busy" reply.
const MAX_REQUESTS: usize = 32;
/// Most requests served at once on one connection. Nodes send theirs one
/// at a time, so this is only reached by a peer misbehaving.
const MAX_REQUESTS_PER_CONN: usize = 4;
/// Longest one request may take, reading it and replying together, so a
/// peer that stops sending or stops reading can't keep its place.
const STREAM_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest we try to tell a peer we're busy.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Object bytes in one outbox page, at most. Encoded for sending, that is
/// still well under the 16 MiB frame limit.
const MAX_PAGE_BYTES: usize = 8 << 20;
/// Most followers a node keeps. Each one costs a notification per post.
const MAX_FOLLOWERS: usize = 10_000;
/// Most new followers accepted per hour. Anyone can subscribe, and new
/// identities are free, so this stops a flood of them. Someone refused
/// still sees new posts, by polling; they only miss the push notification
/// until they try again (nodes do once a day).
const MAX_NEW_FOLLOWERS_PER_HOUR: usize = 100;
/// Followers not heard from for this long are forgotten. A live follower's
/// node subscribes again every day, and answers notifications.
pub(crate) const FOLLOWER_SILENCE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Accepts incoming `osp/social/0` connections.
#[derive(Clone)]
pub(crate) struct SocialProtocol {
    core: Arc<Core>,
    /// Shared by every connection, for `MAX_REQUESTS`.
    requests: Arc<Semaphore>,
}

impl SocialProtocol {
    pub fn new(core: Arc<Core>) -> Self {
        Self {
            core,
            requests: Arc::new(Semaphore::new(MAX_REQUESTS)),
        }
    }
}

impl fmt::Debug for SocialProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SocialProtocol")
    }
}

impl ProtocolHandler for SocialProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let remote = conn.remote_id();
        let on_this_conn = Arc::new(Semaphore::new(MAX_REQUESTS_PER_CONN));
        // Each request arrives on its own stream; serve them concurrently,
        // within the limits.
        while let Ok((send, recv)) = conn.accept_bi().await {
            let (Ok(overall), Ok(here)) = (
                self.requests.clone().try_acquire_owned(),
                on_this_conn.clone().try_acquire_owned(),
            ) else {
                // Not even reading the request keeps being busy cheap.
                debug!(remote = %remote.fmt_short(), "too many social requests; replied busy");
                tell_busy(send).await;
                continue;
            };
            let core = self.core.clone();
            tokio::spawn(async move {
                let served =
                    tokio::time::timeout(STREAM_TIMEOUT, serve_stream(&core, remote, send, recv))
                        .await
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("took too long")));
                if let Err(e) = served {
                    debug!(remote = %remote.fmt_short(), "social request failed: {e:#}");
                }
                drop((overall, here));
            });
        }
        Ok(())
    }
}

async fn tell_busy(mut send: SendStream) {
    let resp = SocialResponse::Error {
        message: "this node is busy; try again later".to_string(),
    };
    let _ = tokio::time::timeout(BUSY_TIMEOUT, async {
        write_frame(&mut send, &resp).await?;
        send.finish()?;
        anyhow::Ok(())
    })
    .await;
}

async fn serve_stream(
    core: &Core,
    remote: EndpointId,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<()> {
    let req: SocialRequest = read_frame_max(&mut recv, MAX_REQUEST_LEN).await?;
    let resp = match core.answer_social(remote, req).await {
        Ok(resp) => resp,
        Err(e) => SocialResponse::Error {
            message: format!("{e:#}"),
        },
    };
    write_frame(&mut send, &resp).await?;
    send.finish()?;
    Ok(())
}

impl Core {
    async fn answer_social(
        &self,
        remote: EndpointId,
        req: SocialRequest,
    ) -> Result<SocialResponse> {
        Ok(match req {
            SocialRequest::GetProfile => SocialResponse::Profile {
                profile: self.profile.clone(),
            },
            SocialRequest::GetOutbox { since, limit } => {
                let mut entries = Vec::new();
                let mut referenced: Vec<SignedObject> = Vec::new();
                let mut page_bytes = 0;
                for (seq, object) in self.db.outbox_since(since, limit.min(MAX_OUTBOX_PAGE))? {
                    // Include what each reshare points at, and its author's
                    // profile, so a reader can show it even if the author is offline.
                    let mut extra = Vec::new();
                    if let Ok(Payload::Announce(a)) = object.verify() {
                        if let Some(target) = self.db.get_object(&a.object)? {
                            extra.push(target);
                        }
                        if let Some(profile) = self.db.profile(&a.object_actor)?
                            && !referenced.contains(&profile)
                        {
                            extra.push(profile);
                        }
                    }
                    // Keep the page well inside the frame limit; the rest
                    // comes with the next page.
                    let bytes: usize = std::iter::once(&object)
                        .chain(&extra)
                        .map(|o| o.payload_bytes().len())
                        .sum();
                    if !entries.is_empty() && page_bytes + bytes > MAX_PAGE_BYTES {
                        break;
                    }
                    page_bytes += bytes;
                    entries.push(OutboxEntry { seq, object });
                    referenced.extend(extra);
                }
                SocialResponse::Outbox {
                    entries,
                    referenced,
                    latest: self.db.outbox_latest()?,
                }
            }
            SocialRequest::Subscribe { addr } => {
                ensure!(addr.id == remote, "can only subscribe on your own behalf");
                check_addr(&addr)?;
                self.accept_follower(&addr)?;
                remember(&self.lookup, &addr);
                SocialResponse::Ok
            }
            SocialRequest::Notify { actor, seq } => {
                let from_followed = self
                    .db
                    .follows()?
                    .iter()
                    .any(|f| f.actor == actor && f.addr.id == remote);
                // An error tells a node we stopped following that we're not
                // listening, so it stops counting us as a live follower.
                ensure!(from_followed, "not following {}", actor.fmt_short());
                debug!(%seq, actor = %actor.fmt_short(), "notified of new posts");
                self.sync_now.notify_one();
                SocialResponse::Ok
            }
            SocialRequest::WhoHosts { blob } => {
                let mut nodes = Vec::new();
                if self.db.blob_class(&blob)?.is_some_and(|c| c.is_public()) {
                    nodes.push(self.shareable_addr().await);
                }
                nodes.extend(
                    self.db
                        .hosts(&blob)?
                        .into_iter()
                        .filter(|a| a.id != remote && a.id != self.endpoint.id()),
                );
                SocialResponse::Hosts { nodes }
            }
        })
    }

    /// Records a subscriber. A node we already have just gets its address
    /// refreshed; a new one must fit within the limits on followers.
    fn accept_follower(&self, addr: &EndpointAddr) -> Result<()> {
        if !self.db.is_follower(&addr.id)? {
            let now = now_ms();
            self.db
                .drop_silent_followers(now - FOLLOWER_SILENCE.as_millis() as i64)?;
            ensure!(
                self.db.follower_count()? < MAX_FOLLOWERS,
                "this node has as many followers as it can notify"
            );
            ensure!(
                self.db.new_followers_since(now - 60 * 60 * 1000)? < MAX_NEW_FOLLOWERS_PER_HOUR,
                "this node is busy with new followers; try again later"
            );
        }
        self.db.add_follower(addr)
    }

    /// Sends one request to another node, reusing an open connection when we have one.
    pub(crate) async fn social_request(
        &self,
        addr: &EndpointAddr,
        req: &SocialRequest,
    ) -> Result<SocialResponse> {
        let conn = self.social_conn(addr).await?;
        let resp = match request_on(&conn, req).await {
            Ok(resp) => resp,
            Err(_) => {
                // The cached connection may have died quietly; try once on a fresh one.
                self.forget_conn(&addr.id);
                let conn = self.social_conn(addr).await?;
                request_on(&conn, req).await?
            }
        };
        if let SocialResponse::Error { message } = &resp {
            bail!("{} replied: {message}", addr.id.fmt_short());
        }
        Ok(resp)
    }

    async fn social_conn(&self, addr: &EndpointAddr) -> Result<Connection> {
        if let Some(conn) = self.cached_conn(&addr.id) {
            return Ok(conn);
        }
        remember(&self.lookup, addr);
        let conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(addr.clone(), SOCIAL_ALPN),
        )
        .await
        .with_context(|| format!("connecting to {} timed out", addr.id.fmt_short()))?
        .with_context(|| format!("connecting to {}", addr.id.fmt_short()))?;
        self.lock_conns().insert(addr.id, conn.clone());
        Ok(conn)
    }

    fn cached_conn(&self, id: &EndpointId) -> Option<Connection> {
        let mut conns = self.lock_conns();
        match conns.get(id) {
            Some(conn) if conn.close_reason().is_none() => Some(conn.clone()),
            Some(_) => {
                conns.remove(id);
                None
            }
            None => None,
        }
    }

    fn forget_conn(&self, id: &EndpointId) {
        self.lock_conns().remove(id);
    }

    fn lock_conns(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<EndpointId, Connection>> {
        self.conns.lock().unwrap_or_else(|e| e.into_inner())
    }
}

async fn request_on(conn: &Connection, req: &SocialRequest) -> Result<SocialResponse> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, req).await?;
    send.finish()?;
    tokio::time::timeout(REQUEST_TIMEOUT, read_frame(&mut recv))
        .await
        .context("no reply in time")?
}
