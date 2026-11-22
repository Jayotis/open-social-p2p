//! Carrier circuits (ADR 0043 and its design note): how a viewer's media
//! reaches a host through nodes that carry it, with a layer of encryption for
//! each hop.
//!
//! ```text
//! V → P → Q → H        V → P → M → Q → H
//! ```
//!
//! Say Jimmy (V) watches Alice's (H) video. Jimmy's node picked P, Alice's
//! node picked Q, and on a newcomer's turn Kim's new node (M) sits between
//! them. Jimmy's node sets up keys with each hop by HPKE, from the circuit key
//! in that hop's signed [`Descriptor`]. Towards Alice, it wraps each packet
//! for Q first, then M, then P, and each hop takes its own layer off. Towards
//! Jimmy, each hop adds its layer and Jimmy's node takes them all off. A hop
//! left out, a byte changed or a packet repeated fails a check, and the packet
//! is dropped.
//!
//! The descriptors, the frames on a link between neighbours, the commands
//! only a circuit's last hop reads, and the layers have no networking. The
//! [`Carrier`] carries them on a node's carrier endpoint; [`HostLink`] and
//! [`ViewerCircuit`] are the two ends, carrying the ends' own packets. The
//! circuit transport puts iroh's QUIC on those.

mod descriptor;
mod ends;
mod frame;
mod layer;
mod service;
mod transport;

pub use descriptor::{CarrierInfo, Descriptor, MAX_DESCRIPTOR_BYTES};
pub use ends::{HostEvent, HostLink, HostSender, ViewerCircuit, ViewerSender};
pub use frame::{CircuitId, DestroyReason, Frame, Relay, SLOT_LEN, Slot};
pub use layer::{
    CarrierKeys, CircuitKey, Direction, Layer, MAX_PACKETS, Onion, SETUP_LEN, Setup, WINDOW,
};
pub use service::{
    ALPN, Carrier, Config, Dial, LINK_DATAGRAM, Limits, Others, SETUP_TIMEOUT, SLOT_LIFETIME,
    Stats, relay_dial, request_slot, wait_for_room,
};
pub use transport::{
    CONNECTION_WINDOW, STREAM_WINDOW, TRANSPORT_ID, host_endpoint, media_windows, viewer_endpoint,
};

/// The protocol's name, which binds every key and signature to it.
pub const PROTOCOL: &[u8] = b"osp/carrier/1";
/// Its version, on every frame. Version 1 has one suite and no negotiation.
pub const VERSION: u8 = 1;
/// Bytes a `DATA` frame adds before its payload: the version, its kind (which
/// says its direction), the circuit ID and the packet number.
pub const DATA_HEADER: usize = 1 + 1 + 4 + 8;
/// Bytes each hop's layer adds: ChaCha20-Poly1305's tag.
pub const TAG: usize = 16;
/// Bytes under the layers that say what a cell asks of the circuit's last
/// hop: one, for the [`Relay`] command.
pub const COMMAND: usize = 1;

/// How much larger than the ends' own packet a `DATA` frame is with `hops`
/// layers: 14 + 16 a hop + 1. With three hops, 63 bytes on a 1200-byte QUIC
/// packet.
pub const fn overhead(hops: usize) -> usize {
    DATA_HEADER + TAG * hops + COMMAND
}

/// Most hops in a path: the viewer's, a newcomer's and the host's.
pub const MAX_HOPS: usize = 3;
