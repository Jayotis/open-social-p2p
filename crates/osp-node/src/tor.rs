//! The node's side of Tor (ADR 0014): its own Tor, and an endpoint on it
//! with the node's key, which other nodes reach at the onion address the
//! node's ID gives. It's the node's public endpoint: all messages between
//! nodes go through it, and go nowhere else.

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{Endpoint, EndpointId, SecretKey, endpoint::Connection};
use osp_proto::{
    SOCIAL_ALPN,
    rpc::TorStatus,
    social::{SocialRequest, SocialResponse},
};
use osp_tor::{Phase, Source, Tor, TorTransport};
use tracing::info;

use crate::{
    config::TorConfig, home::Home, network::ConnTracker, node::MAX_STREAMS_PER_CONN, social,
};

/// Longest `osp tor ping` waits to connect: a first connection over Tor
/// takes up to a minute.
const PING_CONNECT_TIMEOUT: Duration = Duration::from_secs(90);

pub(crate) struct TorSide {
    tor: Tor,
    transport: TorTransport,
    pub endpoint: Endpoint,
    /// Connections `osp tor ping` opened, reused as the node reuses its own.
    pings: Mutex<HashMap<EndpointId, Connection>>,
}

impl TorSide {
    /// Starts the node's Tor, from `config.binary` in the home's `tor`
    /// folder, and an endpoint on it with `key`. Tor itself may take a while
    /// to connect, or not start at all: `status` says.
    pub async fn start(
        home: &Home,
        config: &TorConfig,
        key: SecretKey,
        tracker: &ConnTracker,
    ) -> Result<Self> {
        let tor = Tor::start(Source::Launch {
            binary: config.binary.clone().into(),
            dir: home.tor(),
        });
        let transport = TorTransport::new(&tor, key.clone())
            .await
            .context("starting the Tor transport")?;
        let quic = osp_tor::quic_config()
            .max_concurrent_bidi_streams(MAX_STREAMS_PER_CONN.into())
            .max_concurrent_uni_streams(0u32.into())
            .build();
        let endpoint = Endpoint::builder(transport.preset())
            .transport_config(quic)
            .hooks(tracker.clone())
            .secret_key(key)
            .bind()
            .await
            .context("starting the Tor endpoint")?;
        info!(
            onion = %osp_tor::onion::service_id(&endpoint.id()),
            "Tor is on"
        );
        Ok(Self {
            tor,
            transport,
            endpoint,
            pings: Mutex::default(),
        })
    }

    /// Fails, and says why, unless Tor is ready to carry messages.
    pub fn check_ready(&self) -> Result<()> {
        let phase = self.tor.state().borrow().phase.clone();
        if phase != Phase::Ready {
            bail!("Tor isn't ready: {phase}");
        }
        Ok(())
    }

    /// Waits until Tor is ready to carry messages.
    pub async fn ready(&self) {
        let mut state = self.tor.state();
        let _ = state.wait_for(|s| s.phase == Phase::Ready).await;
    }

    /// Whether a stream to `node` is open, whichever of us dialled it, so a
    /// new connection is a round trip away.
    pub fn has_stream(&self, node: &EndpointId) -> bool {
        self.transport.has_stream(node)
    }

    pub fn status(&self) -> TorStatus {
        let state = self.tor.state().borrow().clone();
        let streams = self.transport.streams();
        TorStatus {
            state: state.phase.to_string(),
            ready: state.phase == Phase::Ready,
            restarts: state.restarts,
            published: state.onion_services > 0 && state.published == state.onion_services,
            onion_error: state.onion_error,
            streams: streams.open,
            dialling: streams.dialling,
        }
    }

    /// Problems for `osp status`. Connecting to the Tor network after a
    /// start isn't one: it takes seconds.
    pub fn problems(&self) -> Vec<String> {
        let state = self.tor.state().borrow().clone();
        let mut problems = Vec::new();
        if let Phase::Down(why) = state.phase {
            problems.push(format!(
                "Tor isn't running ({why}), so other nodes can't reach this one through it; the node starts it again by itself"
            ));
        }
        if let Some(e) = state.onion_error {
            problems.push(format!("Tor refused this node's onion service: {e}"));
        }
        problems
    }

    /// Reaches `node` over Tor and asks for its profile. Returns how long
    /// connecting took, if this connected, and how long the request took.
    pub async fn ping(&self, node: EndpointId) -> Result<(Option<Duration>, Duration)> {
        self.check_ready()?;
        let open = self
            .lock_pings()
            .get(&node)
            .filter(|c| c.close_reason().is_none())
            .cloned();
        let (conn, connected) = match open {
            Some(conn) => (conn, None),
            None => {
                let started = Instant::now();
                let conn = tokio::time::timeout(
                    PING_CONNECT_TIMEOUT,
                    self.endpoint.connect(node, SOCIAL_ALPN),
                )
                .await
                .with_context(|| format!("reaching {} over Tor timed out", node.fmt_short()))?
                .with_context(|| format!("reaching {} over Tor", node.fmt_short()))?;
                self.lock_pings().insert(node, conn.clone());
                (conn, Some(started.elapsed()))
            }
        };
        let started = Instant::now();
        let resp = social::request_on(&conn, &SocialRequest::GetProfile).await;
        if resp.is_err() {
            self.lock_pings().remove(&node);
        }
        ensure!(
            matches!(resp?, SocialResponse::Profile { .. }),
            "{} didn't answer with its profile",
            node.fmt_short()
        );
        Ok((connected, started.elapsed()))
    }

    fn lock_pings(&self) -> MutexGuard<'_, HashMap<EndpointId, Connection>> {
        self.pings.lock().unwrap_or_else(|e| e.into_inner())
    }
}
