//! The node's media and direct endpoints (ADR 0014), and who may connect to
//! the direct one.
//!
//! iroh tells a node that connects our addresses as soon as the handshake
//! ends, before any hook or protocol handler runs, so refusing there is too
//! late (`docs/testing/direct-endpoint.md` has the test results). What can
//! refuse in time is a filter on the first packet, and a packet through a relay
//! names its sender, as the relay checked it. Only the relay's operator could
//! forge that, and it sees our address anyway when we connect to its relay.
//!
//! A packet that came straight to one of our IP addresses names nobody. Its
//! sender has that address already, so it goes on to the handshake, and is
//! refused once its key is known. By then it may have been told our other
//! addresses; iroh gives no way of preventing that yet.

use std::{
    fmt,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey,
    address_lookup::MemoryLookup,
    endpoint::{AfterHandshakeOutcome, Connection, EndpointHooks, Incoming, IncomingAddr, VarInt},
    protocol::IncomingFilterOutcome,
};
use osp_proto::{
    BoundEndpoint, CLIENT_ALPN, EndpointBinding, EndpointKind, pairing_pass_hash,
    rpc::EndpointsStatus,
    social::{SocialRequest, SocialResponse},
};
use tracing::{debug, info, warn};

use crate::{
    client::PendingPairing,
    config::{Config, DirectRule, RelayChoice},
    db::{Db, now_ms},
    home::Home,
    keys,
    network::ConnTracker,
    node::{Core, check_addr, endpoint_builder, remember},
    relays::{self, RelayPolicy},
};

/// How far ahead of our clock a binding may be dated. The later one holds, so
/// one dated years ahead could never be replaced.
const MAX_DATED_AHEAD: Duration = Duration::from_secs(5 * 60);

/// The code a node refused after the handshake is told.
const NOT_ALLOWED: VarInt = VarInt::from_u32(403);

/// Pairing secrets handed out and not used yet.
pub(crate) type Pairings = Arc<Mutex<Vec<PendingPairing>>>;

/// One of the node's endpoints other than the public one.
pub(crate) struct OwnEndpoint {
    pub endpoint: Endpoint,
    pub binding: EndpointBinding,
}

pub(crate) struct OwnEndpoints {
    /// None without relays, which are all it could use.
    pub media: Option<OwnEndpoint>,
    pub direct: OwnEndpoint,
    pub gate: DirectGate,
}

impl OwnEndpoints {
    pub async fn start(
        home: &Home,
        config: &Config,
        public: &SecretKey,
        lookup: &MemoryLookup,
        relays: &RelayPolicy,
        tracker: &ConnTracker,
        gate: DirectGate,
    ) -> Result<Self> {
        let net = &config.network;
        let media = match net.relay {
            RelayChoice::Disabled => None,
            _ => {
                let (key, binding) = own_key(public, &home.media_key(), EndpointKind::Media)?;
                // Its address gives away only which relay it uses, so looking
                // it up by ID is fine, and it can't go anywhere else.
                let relays = RelayPolicy::new(relays::own_relays(&net.relay)?, true);
                let endpoint = endpoint_builder(net, lookup, &relays)?
                    .hooks(tracker.with_relays(relays))
                    .clear_ip_transports()
                    .secret_key(key)
                    .bind()
                    .await
                    .context("starting the media endpoint")?;
                Some(OwnEndpoint { endpoint, binding })
            }
        };
        let (key, binding) = own_key(public, &home.direct_key(), EndpointKind::Direct)?;
        // The relay is published under its ID, as phones find the node again
        // that way when its relay changes. That gets a stranger nothing: the
        // gate refuses it. IP addresses aren't published.
        let mut builder = endpoint_builder(net, lookup, relays)?
            .hooks(gate.clone())
            .hooks(tracker.clone());
        if net.relay_only {
            builder = builder.clear_ip_transports();
        } else if let Some(addr) = net.bind {
            // The node's main endpoint has any port the config names, until
            // ADR 0014's step 3 takes it off IP.
            builder = builder
                .clear_ip_transports()
                .bind_addr(SocketAddr::new(addr.ip(), 0))?;
        }
        let endpoint = builder
            .secret_key(key)
            .bind()
            .await
            .context("starting the direct endpoint")?;
        info!(
            direct = %endpoint.id().fmt_short(),
            media = %media.as_ref().map_or("none".to_string(), |m| m.endpoint.id().fmt_short().to_string()),
            "endpoints started"
        );
        Ok(Self {
            media,
            direct: OwnEndpoint { endpoint, binding },
            gate,
        })
    }

    pub fn status(&self) -> EndpointsStatus {
        EndpointsStatus {
            media: self.media.as_ref().map(|m| m.endpoint.id()),
            direct: self.direct.endpoint.id(),
            rule: self.gate.rule.to_string(),
        }
    }
}

/// The endpoint's key, made if it's new, and its binding, dated when the key
/// was made.
fn own_key(
    public: &SecretKey,
    path: &Path,
    kind: EndpointKind,
) -> Result<(SecretKey, EndpointBinding)> {
    let key = match path.exists() {
        true => keys::load(path)?,
        false => keys::create(path)?,
    };
    let since_ms = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .with_context(|| format!("reading {}", path.display()))?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let binding = EndpointBinding::new(public, &key, kind, since_ms);
    Ok((key, binding))
}

/// Decides who may connect to the direct endpoint: the node's own phones, and
/// peers its rule allows (ADR 0014). It holds no endpoint, as iroh's hooks
/// mustn't.
#[derive(Clone)]
pub(crate) struct DirectGate {
    db: Arc<Db>,
    rule: DirectRule,
    pairings: Pairings,
}

impl fmt::Debug for DirectGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DirectGate({})", self.rule)
    }
}

impl DirectGate {
    pub fn new(db: Arc<Db>, rule: DirectRule, pairings: Pairings) -> Self {
        Self { db, rule, pairings }
    }

    /// Decides on a connection from its first packet, before any handshake,
    /// so whoever it refuses learns no address.
    pub fn check_first_packet(&self, incoming: &Incoming) -> IncomingFilterOutcome {
        let IncomingAddr::Relay { endpoint_id, .. } = incoming.remote_addr() else {
            return IncomingFilterOutcome::Accept;
        };
        if self.allows_endpoint(&endpoint_id) || self.has_pairing_pass(incoming) {
            return IncomingFilterOutcome::Accept;
        }
        debug!(node = %endpoint_id.fmt_short(), "refused a connection to the direct endpoint");
        IncomingFilterOutcome::Reject
    }

    /// Whether a connecting node's endpoint belongs to someone allowed. Fails
    /// closed: an address given can't be taken back.
    pub fn allows_endpoint(&self, endpoint: &EndpointId) -> bool {
        let allowed = || -> Result<bool> {
            if self.rule == DirectRule::Anyone || self.db.is_device(endpoint)? {
                return Ok(true);
            }
            match self.db.direct_endpoint_owner(endpoint)? {
                Some(public) => self.allows_public(&public),
                None => Ok(false),
            }
        };
        allowed().unwrap_or_else(|e| {
            warn!("checking who may connect directly: {e:#}");
            false
        })
    }

    /// Whether it takes knowing whose endpoint a node's is. Under `anyone` it
    /// doesn't, and anyone can make new keys.
    pub fn needs_endpoints(&self) -> bool {
        self.rule != DirectRule::Anyone
    }

    /// Whether the rule allows a direct connection with the node `public`.
    pub fn allows_public(&self, public: &EndpointId) -> Result<bool> {
        Ok(match self.rule {
            DirectRule::Anyone => true,
            DirectRule::Follows => self.db.follows()?.iter().any(|f| f.addr.id == *public),
            DirectRule::Approved => self.db.is_direct_contact(public)?,
            DirectRule::Nobody => false,
        })
    }

    /// Whether the first packet holds the pass of a pairing ticket not used
    /// yet. A phone that is pairing isn't known to the node until it has.
    fn has_pairing_pass(&self, incoming: &Incoming) -> bool {
        // Decrypting the packet costs more than the checks before it.
        if !self.pairing_waiting() {
            return false;
        }
        let Some(alpns) = incoming.decrypt().and_then(|initial| initial.alpns()) else {
            return false;
        };
        let now = Instant::now();
        let pairings = self.lock_pairings();
        alpns
            .filter_map(Result::ok)
            .filter_map(|alpn| pairing_pass_hash(&alpn))
            .any(|hash| pairings.iter().any(|p| p.matches(&hash, now)))
    }

    fn pairing_waiting(&self) -> bool {
        let now = Instant::now();
        self.lock_pairings().iter().any(|p| p.is_live(now))
    }

    fn lock_pairings(&self) -> MutexGuard<'_, Vec<PendingPairing>> {
        self.pairings.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl EndpointHooks for DirectGate {
    /// For connections that came straight to an IP address, whose key is
    /// known only now. A phone that is pairing at home may be one: it can
    /// only try to pair (`ClientProtocol`).
    async fn after_handshake(&self, conn: &Connection) -> AfterHandshakeOutcome {
        if conn.side().is_client()
            || self.allows_endpoint(&conn.remote_id())
            || (conn.alpn() == CLIENT_ALPN && self.pairing_waiting())
        {
            return AfterHandshakeOutcome::Accept;
        }
        debug!(node = %conn.remote_id().fmt_short(), "refused a connection to the direct endpoint");
        AfterHandshakeOutcome::Reject {
            error_code: NOT_ALLOWED,
            reason: b"not allowed".to_vec(),
        }
    }
}

impl Core {
    /// An endpoint of ours as we hand it to another node.
    async fn bound_endpoint(&self, own: &OwnEndpoint) -> BoundEndpoint {
        BoundEndpoint {
            binding: own.binding.clone(),
            addr: self.shareable_addr_of(&own.endpoint).await,
        }
    }

    /// Answers a node asking for our endpoints. Its direct endpoint is kept
    /// only if the gate will need it, so strangers can't fill the table.
    pub(crate) async fn answer_endpoints(
        &self,
        remote: EndpointId,
        offered: Option<BoundEndpoint>,
    ) -> Result<SocialResponse> {
        let own = self
            .endpoints
            .as_ref()
            .context("this node has no other endpoints yet")?;
        let allowed = own.gate.allows_public(&remote)?;
        if let Some(bound) = offered.filter(|_| allowed && own.gate.needs_endpoints()) {
            self.keep_endpoint(&remote, EndpointKind::Direct, &bound)?;
        }
        let media = match &own.media {
            Some(media) => Some(Box::new(self.bound_endpoint(media).await)),
            None => None,
        };
        let direct = match allowed {
            true => Some(Box::new(self.bound_endpoint(&own.direct).await)),
            false => None,
        };
        Ok(SocialResponse::Endpoints { media, direct })
    }

    /// Asks `node` for its endpoints, offering our direct one if our rule
    /// allows a direct connection with it. Checks what comes back. Its direct
    /// endpoint comes back only if both rules allow it.
    pub(crate) async fn ask_endpoints(&self, node: &EndpointAddr) -> Result<PeerEndpoints> {
        let own = self
            .endpoints
            .as_ref()
            .context("this node's other endpoints are off: set enabled = true under [endpoints]")?;
        let allowed = own.gate.allows_public(&node.id)?;
        let offer = match allowed {
            true => Some(self.bound_endpoint(&own.direct).await),
            false => None,
        };
        let req = SocialRequest::Endpoints { direct: offer };
        let SocialResponse::Endpoints { media, direct } = self.social_request(node, &req).await?
        else {
            bail!("unexpected reply to Endpoints");
        };
        if let Some(bound) = &media {
            self.keep_endpoint(&node.id, EndpointKind::Media, bound)?;
        }
        let direct = direct.filter(|_| allowed);
        if let Some(bound) = &direct {
            self.keep_endpoint(&node.id, EndpointKind::Direct, bound)?;
        }
        Ok(PeerEndpoints {
            media: media.map(|b| b.addr),
            direct: direct.map(|b| b.addr),
        })
    }

    /// Checks another node's endpoint and keeps it, and how to reach it.
    fn keep_endpoint(
        &self,
        public: &EndpointId,
        kind: EndpointKind,
        bound: &BoundEndpoint,
    ) -> Result<()> {
        bound.check(public, kind)?;
        check_addr(&bound.addr)?;
        let latest = now_ms().saturating_add(MAX_DATED_AHEAD.as_millis() as i64);
        ensure!(
            i64::try_from(bound.binding.since_ms).is_ok_and(|since| since <= latest),
            "this endpoint's binding is dated in the future"
        );
        if self.db.keep_endpoint(bound)? {
            remember(&self.lookup, &bound.addr);
        }
        Ok(())
    }
}

/// How to reach another node's endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerEndpoints {
    pub media: Option<EndpointAddr>,
    pub direct: Option<EndpointAddr>,
}
