//! What `status` reports about the network: every live connection, whichever
//! side opened it and whatever protocol it carries, and how the local NAT
//! behaves.

use std::sync::{Arc, Mutex};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, Watcher,
    endpoint::{
        AfterHandshakeOutcome, BeforeConnectOutcome, Connection, EndpointHooks, Side, VarInt,
        WeakConnectionHandle,
    },
    unstable_net_report::NetReport,
};
use osp_proto::rpc::{Direction, NatKind, PeerInfo};
use tracing::{info, warn};

use crate::relays::RelayPolicy;

/// Remembers each connection once its handshake completes. Holds only weak
/// handles, so it never keeps a connection open. Also refuses to dial an
/// address the relay policy doesn't allow.
#[derive(Debug, Clone)]
pub(crate) struct ConnTracker {
    conns: Arc<Mutex<Vec<WeakConnectionHandle>>>,
    relays: RelayPolicy,
}

impl EndpointHooks for ConnTracker {
    /// Addresses are filtered before they get here (`RelayPolicy::filter`);
    /// this catches any that weren't.
    async fn before_connect(&self, remote: &EndpointAddr, _alpn: &[u8]) -> BeforeConnectOutcome {
        if remote.addrs.iter().all(|a| self.relays.permits(a)) {
            BeforeConnectOutcome::Accept
        } else {
            warn!(node = %remote.id.fmt_short(), "refused to dial an address the relay policy doesn't allow");
            BeforeConnectOutcome::Reject
        }
    }

    async fn after_handshake(&self, conn: &Connection) -> AfterHandshakeOutcome {
        let mut conns = self.lock();
        conns.retain(|weak| weak.upgrade().is_some_and(|c| c.close_reason().is_none()));
        conns.push(conn.weak_handle());
        AfterHandshakeOutcome::accept()
    }
}

impl ConnTracker {
    pub fn new(relays: RelayPolicy) -> Self {
        Self {
            conns: Arc::default(),
            relays,
        }
    }

    /// The same record of connections, for an endpoint with its own relays.
    pub fn with_relays(&self, relays: RelayPolicy) -> Self {
        Self {
            conns: self.conns.clone(),
            relays,
        }
    }

    /// One entry per remote node with a live connection.
    pub fn peers(&self) -> Vec<PeerInfo> {
        let live: Vec<Connection> = self
            .lock()
            .iter()
            .filter_map(|weak| weak.upgrade())
            .filter(|c| c.close_reason().is_none())
            .collect();
        let mut peers: Vec<PeerInfo> = Vec::new();
        for conn in live {
            let direction = match conn.side() {
                Side::Client => Direction::Outgoing,
                Side::Server => Direction::Incoming,
            };
            let (path, rtt_ms) = selected_path(&conn);
            match peers.iter_mut().find(|p| p.node == conn.remote_id()) {
                Some(peer) => {
                    if peer.direction != direction {
                        peer.direction = Direction::Both;
                    }
                    if peer.path == "none" {
                        (peer.path, peer.rtt_ms) = (path, rtt_ms);
                    }
                }
                None => peers.push(PeerInfo {
                    node: conn.remote_id(),
                    path,
                    rtt_ms,
                    direction,
                    device: false,
                }),
            }
        }
        peers
    }

    /// Closes every live connection with `remote`, on any protocol.
    pub fn close_all(&self, remote: EndpointId, reason: &[u8]) {
        for conn in self.lock().iter().filter_map(|weak| weak.upgrade()) {
            if conn.remote_id() == remote {
                conn.close(VarInt::from_u32(0), reason);
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<WeakConnectionHandle>> {
        self.conns.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Whether a connection's data goes straight to the other node now, not
/// through a relay. `None` while no path is selected: between one closing
/// and the next being chosen.
pub(crate) fn is_direct(conn: &Connection) -> Option<bool> {
    conn.paths()
        .iter()
        .find(|p| p.is_selected())
        .map(|p| !p.is_relay())
}

fn selected_path(conn: &Connection) -> (String, Option<u64>) {
    let paths = conn.paths();
    let selected = paths.iter().find(|p| p.is_selected());
    let kind = match &selected {
        Some(p) if p.is_relay() => "relay",
        Some(p) if p.is_ip() => "direct",
        _ => "none",
    };
    (
        kind.to_string(),
        selected.map(|p| p.rtt().as_millis() as u64),
    )
}

/// The NAT behaviour from iroh's latest network check, if it has finished
/// one. Only meaningful with relays: they are what iroh probes.
pub(crate) fn nat_kind(endpoint: &Endpoint) -> Option<NatKind> {
    endpoint.net_report().get().as_ref().and_then(classify)
}

fn classify(report: &NetReport) -> Option<NatKind> {
    if !report.has_udp() {
        return Some(NatKind::NoUdp);
    }
    report.mapping_varies_by_dest().map(|varies| {
        if varies {
            NatKind::VariesByDestination
        } else {
            NatKind::Consistent
        }
    })
}

/// Logs the NAT behaviour when it first becomes known and whenever it
/// changes. iroh itself warns on every network check (about every 25
/// seconds) while the address varies; that warning is filtered out by default.
pub(crate) async fn log_nat_changes(endpoint: Endpoint) {
    let mut reports = endpoint.net_report();
    let mut last = None;
    while let Ok(report) = reports.updated().await {
        let nat = report.as_ref().and_then(classify);
        if let Some(kind) = nat
            && nat != last
        {
            info!("NAT: {}", kind.describe());
            last = nat;
        }
    }
}
