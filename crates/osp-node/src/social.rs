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
    Payload, SOCIAL_ALPN,
    framing::{read_frame, write_frame},
    social::{MAX_OUTBOX_PAGE, OutboxEntry, SocialRequest, SocialResponse},
};
use tracing::debug;

use crate::node::{Core, remember};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Accepts incoming `osp/social/0` connections.
#[derive(Clone)]
pub(crate) struct SocialProtocol {
    core: Arc<Core>,
}

impl SocialProtocol {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
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
        // Each request arrives on its own stream; serve them concurrently.
        while let Ok((send, recv)) = conn.accept_bi().await {
            let core = self.core.clone();
            tokio::spawn(async move {
                if let Err(e) = serve_stream(&core, remote, send, recv).await {
                    debug!(remote = %remote.fmt_short(), "social request failed: {e:#}");
                }
            });
        }
        Ok(())
    }
}

async fn serve_stream(
    core: &Core,
    remote: EndpointId,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<()> {
    let req: SocialRequest = tokio::time::timeout(REQUEST_TIMEOUT, read_frame(&mut recv))
        .await
        .context("request timed out")??;
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
                let rows = self.db.outbox_since(since, limit.min(MAX_OUTBOX_PAGE))?;
                // Include what each reshare points at, and its author's
                // profile, so a reader can show it even if the author is offline.
                let mut referenced = Vec::new();
                for (_, obj) in &rows {
                    if let Ok(Payload::Announce(a)) = obj.verify() {
                        if let Some(target) = self.db.get_object(&a.object)? {
                            referenced.push(target);
                        }
                        if let Some(profile) = self.db.profile(&a.object_actor)?
                            && !referenced.contains(&profile)
                        {
                            referenced.push(profile);
                        }
                    }
                }
                let entries = rows
                    .into_iter()
                    .map(|(seq, object)| OutboxEntry { seq, object })
                    .collect();
                SocialResponse::Outbox {
                    entries,
                    referenced,
                    latest: self.db.outbox_latest()?,
                }
            }
            SocialRequest::Subscribe { addr } => {
                ensure!(addr.id == remote, "can only subscribe on your own behalf");
                self.db.add_follower(&addr)?;
                remember(&self.lookup, &addr);
                SocialResponse::Ok
            }
            SocialRequest::Notify { actor, seq } => {
                let from_followed = self
                    .db
                    .follows()?
                    .iter()
                    .any(|f| f.actor == actor && f.addr.id == remote);
                if from_followed {
                    debug!(%seq, actor = %actor.fmt_short(), "notified of new posts");
                    self.sync_now.notify_one();
                }
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

    /// Open connections, for `status`.
    pub(crate) fn live_conns(&self) -> Vec<Connection> {
        self.lock_conns()
            .values()
            .filter(|c| c.close_reason().is_none())
            .cloned()
            .collect()
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
