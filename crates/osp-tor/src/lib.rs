//! Messages between nodes over Tor (ADR 0014): an iroh transport whose
//! packets travel on streams between onion services, so neither node
//! learns the other's address; and the node's own Tor, which it starts,
//! watches, and starts again when it stops.
//!
//! The transport started from n0's `iroh-tor-transport` 0.1.0 (MIT or
//! Apache-2.0), with these changes:
//!
//! - one stream carries a pair of nodes' packets both ways, in order, and
//!   the node that dialled proves its key on it first ([`transport`]'s
//!   wire format, which isn't upstream's);
//! - the onion service is set up again whenever Tor restarts;
//! - a control-port client of its own instead of `torut`;
//! - limits on streams and dials, and pauses between failed dials.
//!
//! Tor hides addresses; it doesn't make anyone anonymous. Someone watching
//! both ends' traffic can still match it up.
//!
//! [`transport`]: TorTransport

mod control;
pub mod onion;
mod socks;
mod tor;
mod transport;

pub use tor::{Phase, Source, Tor, TorState};
pub use transport::{
    Dial, MAX_IDLE, ONION_PORT, Streams, TRANSPORT_ID, TorTransport, addr, quic_config,
};
