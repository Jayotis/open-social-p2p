//! The ends' connection over a circuit: an iroh custom transport whose only
//! path is the circuit.
//!
//! The viewer's endpoint for a circuit and the host's circuit endpoint have
//! no IP transports, no relay and no address lookup but this one, so iroh
//! can't find a quicker path by itself: it can't move Jimmy's connection to
//! a direct link and show him Alice's address, or send him the addresses
//! Alice's endpoint has. Neither endpoint has an address to send. That's the
//! failing open ADR 0017 rules out.
//!
//! Path MTU discovery is off, so the ends' QUIC keeps to 1200-byte packets
//! and always fits a link ([`LINK_DATAGRAM`](crate::LINK_DATAGRAM)).

use std::{
    fmt, io,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use anyhow::Result;
use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr,
    address_lookup::{AddressLookup, EndpointData, EndpointInfo, Error, Item},
    endpoint::{
        Builder, QuicTransportConfig, QuicTransportConfigBuilder,
        presets::{Minimal, Preset},
        transports::{CustomEndpoint, CustomSender, CustomTransport, RecvInfo, Transmit},
    },
};
use iroh_base::CustomAddr;
use n0_future::boxed::BoxStream;
use tokio::sync::mpsc;

use crate::{CircuitId, HostEvent, HostLink, HostSender, ViewerCircuit, ViewerSender};

/// Marks addresses on this transport among custom transports'.
pub const TRANSPORT_ID: u64 = 0x6f73_7063_6972; // "ospcir"

const HOST: u8 = 0;
const CIRCUIT: u8 = 1;
const HERE: u8 = 2;

/// At the viewer: the host at the far end of its circuit.
fn host_addr(host: EndpointId) -> CustomAddr {
    CustomAddr::from_parts(TRANSPORT_ID, &[&[HOST][..], host.as_bytes()].concat())
}

/// At the host: one viewer's circuit, by the ID its hop gave it.
fn circuit_addr(circuit: CircuitId) -> CustomAddr {
    CustomAddr::from_parts(
        TRANSPORT_ID,
        &[&[CIRCUIT][..], &circuit.to_be_bytes()].concat(),
    )
}

fn circuit_of(addr: &CustomAddr) -> Option<CircuitId> {
    let data = addr.data();
    if addr.id() != TRANSPORT_ID || data.len() != 5 || data[0] != CIRCUIT {
        return None;
    }
    Some(CircuitId::from_be_bytes(data[1..].try_into().ok()?))
}

/// This end's own address: it names nothing anyone could reach.
fn here() -> CustomAddr {
    CustomAddr::from_parts(TRANSPORT_ID, &[HERE])
}

/// How much one stream may have on its way to this end before it reads any:
/// QUIC's receive window per stream. A stream can't go faster than this per
/// round trip, so QUIC's default, 1.25 MB, held a carrier fetch at a 0.8 s
/// round trip to about 1.2 MB/s (`docs/testing/carrier-lab.md`). Too large
/// breaks relayed paths instead: with 8 MiB, a round trip's burst met the
/// project relay's per-client limit (4 MB/s, bursting to 8 MB) and the
/// viewer's relay queue, and the fetch failed. 3 MiB allows about 3.9 MB/s
/// at 0.8 s, and a burst well inside the relay's.
pub const STREAM_WINDOW: u32 = 3 << 20;
/// The same for a whole connection: two streams' worth, so two transfers
/// can each have their whole window, and memory per connection is bounded.
pub const CONNECTION_WINDOW: u32 = 2 * STREAM_WINDOW;

/// OSP's receive windows ([`STREAM_WINDOW`], [`CONNECTION_WINDOW`]), for a
/// node's own endpoints. The ends of its circuits have a smaller one
/// ([`CIRCUIT_STREAM_WINDOW`]).
pub fn media_windows(config: QuicTransportConfigBuilder) -> QuicTransportConfigBuilder {
    config
        .stream_receive_window(STREAM_WINDOW.into())
        .receive_window(CONNECTION_WINDOW.into())
}

/// A stream's receive window at the ends of a circuit, below
/// [`STREAM_WINDOW`]. A burst lost between two hops' QUIC isn't sent again
/// (datagrams), and each lost packet leaves a hole in the stream until the
/// ends' QUIC sends it again. noq gives up on a stream with more than 1,024
/// pieces waiting around holes (`too many gaps in stream buffer`), and with
/// 3 MiB in flight, about 2,700 packets of 1,200 bytes, stage D's sixth run
/// and the probes after it hit that. 1 MiB is under 1,000 packets, so a
/// stream can't get there, and allows about 1.3 MB/s at a 0.8 s round trip,
/// more than three times the stage D video's 0.38 MB/s watchability mark.
pub const CIRCUIT_STREAM_WINDOW: u32 = 1 << 20;

/// QUIC's settings for an end over a circuit: iroh's, with path MTU
/// discovery off, packets kept at QUIC's 1,200 bytes, and a stream window
/// that no lost burst can push past noq's limit ([`CIRCUIT_STREAM_WINDOW`]).
fn quic_config() -> QuicTransportConfig {
    QuicTransportConfig::builder()
        .stream_receive_window(CIRCUIT_STREAM_WINDOW.into())
        .receive_window((2 * CIRCUIT_STREAM_WINDOW).into())
        .mtu_discovery_config(None)
        .build()
}

/// Packets from the other end, waiting for iroh to read them.
enum Inbox {
    Viewer(mpsc::Receiver<Vec<u8>>),
    Host(mpsc::Receiver<HostEvent>),
}

#[derive(Clone)]
enum Out {
    Viewer(Arc<ViewerSender>, EndpointId),
    Host(HostSender),
}

/// The transport for one end: the viewer's over its circuit, or the host's
/// over its link to its hop.
#[derive(Clone)]
struct CircuitTransport {
    out: Out,
    inbox: Arc<Mutex<Option<Inbox>>>,
}

impl fmt::Debug for CircuitTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CircuitTransport")
    }
}

impl CustomTransport for CircuitTransport {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
        let inbox = self
            .inbox
            .lock()
            .expect("inbox lock")
            .take()
            .ok_or_else(|| io::Error::other("this transport already has an endpoint"))?;
        Ok(Box::new(End {
            out: self.out.clone(),
            inbox,
            addrs: n0_watcher::Watchable::new(vec![here()]),
        }))
    }
}

struct End {
    out: Out,
    inbox: Inbox,
    addrs: n0_watcher::Watchable<Vec<CustomAddr>>,
}

impl fmt::Debug for End {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CircuitEnd")
    }
}

impl CustomEndpoint for End {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
        self.addrs.watch()
    }

    fn create_sender(&self) -> Arc<dyn CustomSender> {
        Arc::new(Sender {
            out: self.out.clone(),
        })
    }

    fn max_transmit_segments(&self) -> NonZeroUsize {
        NonZeroUsize::new(32).expect("not zero")
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        metas: &mut [noq_udp::RecvMeta],
        recv_infos: &mut [RecvInfo],
    ) -> Poll<io::Result<usize>> {
        let slots = bufs.len().min(metas.len()).min(recv_infos.len());
        let mut filled = 0;
        while filled < slots {
            let next = match &mut self.inbox {
                Inbox::Viewer(rx) => match rx.poll_recv(cx) {
                    Poll::Ready(Some(packet)) => Some((
                        match &self.out {
                            Out::Viewer(_, host) => host_addr(*host),
                            Out::Host(_) => unreachable!("a viewer's inbox"),
                        },
                        packet,
                    )),
                    Poll::Ready(None) => None,
                    Poll::Pending => break,
                },
                Inbox::Host(rx) => match rx.poll_recv(cx) {
                    Poll::Ready(Some(HostEvent::Packet(circuit, packet))) => {
                        Some((circuit_addr(circuit), packet))
                    }
                    // Circuits opening and closing: the ends' QUIC finds out
                    // for itself.
                    Poll::Ready(Some(_)) => continue,
                    Poll::Ready(None) => None,
                    Poll::Pending => break,
                },
            };
            let Some((from, packet)) = next else {
                if filled == 0 {
                    return Poll::Ready(Err(io::Error::other("the circuit closed")));
                }
                break;
            };
            let len = packet.len();
            // Too big for the buffer: lost, as a datagram would be.
            if len > bufs[filled].len() {
                continue;
            }
            bufs[filled][..len].copy_from_slice(&packet);
            metas[filled].len = len;
            metas[filled].stride = len;
            recv_infos[filled] = RecvInfo::new(from, Some(here()));
            filled += 1;
        }
        if filled == 0 && slots > 0 {
            Poll::Pending
        } else {
            Poll::Ready(Ok(filled))
        }
    }
}

struct Sender {
    out: Out,
}

impl fmt::Debug for Sender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CircuitSender")
    }
}

impl CustomSender for Sender {
    fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
        match &self.out {
            Out::Viewer(_, host) => *addr == host_addr(*host),
            Out::Host(_) => circuit_of(addr).is_some(),
        }
    }

    fn poll_send(
        &self,
        _cx: &mut Context,
        dst: &CustomAddr,
        _src: Option<&CustomAddr>,
        transmit: &Transmit<'_>,
    ) -> Poll<io::Result<()>> {
        let contents = transmit.contents;
        let size = transmit.segment_size.unwrap_or(contents.len()).max(1);
        match &self.out {
            Out::Viewer(sender, host) => {
                if *dst != host_addr(*host) {
                    return Poll::Ready(Err(io::Error::other("not this circuit's host")));
                }
                // A packet that can't be sent is lost; the ends' QUIC sends
                // it again.
                for packet in contents.chunks(size) {
                    let _ = sender.send(packet.to_vec());
                }
            }
            Out::Host(sender) => {
                let Some(circuit) = circuit_of(dst) else {
                    return Poll::Ready(Err(io::Error::other("not a circuit")));
                };
                for packet in contents.chunks(size) {
                    let _ = sender.send(circuit, packet.to_vec());
                }
            }
        }
        Poll::Ready(Ok(()))
    }
}

struct CircuitPreset {
    transport: CircuitTransport,
}

impl Preset for CircuitPreset {
    fn apply(self, builder: Builder) -> Builder {
        let lookup = match &self.transport.out {
            Out::Viewer(_, host) => Lookup(Some(*host)),
            Out::Host(_) => Lookup(None),
        };
        // `Minimal` sets the crypto provider iroh needs; the rest is ours.
        Minimal
            .apply(builder)
            .clear_ip_transports()
            .clear_relay_transports()
            .clear_address_lookup()
            .add_custom_transport(Arc::new(self.transport))
            .address_lookup(lookup)
            .transport_config(quic_config())
    }
}

/// Finds the host at the far end of the viewer's circuit, and nothing else.
#[derive(Debug, Clone)]
struct Lookup(Option<EndpointId>);

impl AddressLookup for Lookup {
    fn publish(&self, _data: &EndpointData) {}

    fn resolve(&self, node: EndpointId) -> Option<BoxStream<Result<Item, Error>>> {
        if self.0 != Some(node) {
            return None;
        }
        let found = EndpointAddr::from_parts(node, [TransportAddr::Custom(host_addr(node))]);
        let item = Item::new(EndpointInfo::from(found), "circuit", None);
        Some(Box::pin(n0_future::stream::once(Ok(item))))
    }
}

/// The viewer's endpoint for one circuit, under a new key made for it and
/// published nowhere. Its only peer is `host`, the host's circuit endpoint,
/// at the far end of `circuit`.
pub async fn viewer_endpoint(circuit: ViewerCircuit, host: EndpointId) -> Result<Endpoint> {
    let (sender, packets) = circuit.split();
    let transport = CircuitTransport {
        out: Out::Viewer(Arc::new(sender), host),
        inbox: Arc::new(Mutex::new(Some(Inbox::Viewer(packets)))),
    };
    Ok(Endpoint::builder(CircuitPreset { transport })
        .secret_key(SecretKey::generate())
        .bind()
        .await?)
}

/// The host's circuit endpoint, apart from any endpoint with internet
/// addresses: it answers the viewers' circuits that its hop joins to `link`,
/// under `key`, on `alpns`.
pub async fn host_endpoint(
    link: HostLink,
    key: SecretKey,
    alpns: Vec<Vec<u8>>,
) -> Result<Endpoint> {
    let (sender, events) = link.split();
    let transport = CircuitTransport {
        out: Out::Host(sender),
        inbox: Arc::new(Mutex::new(Some(Inbox::Host(events)))),
    };
    Ok(Endpoint::builder(CircuitPreset { transport })
        .secret_key(key)
        .alpns(alpns)
        .bind()
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(circuit_of(&circuit_addr(7)), Some(7));
        assert_eq!(circuit_of(&circuit_addr(u32::MAX)), Some(u32::MAX));
        let host = SecretKey::generate().public();
        assert_eq!(circuit_of(&host_addr(host)), None);
        assert_eq!(circuit_of(&here()), None);
        let other = CustomAddr::from_parts(TRANSPORT_ID + 1, &[CIRCUIT, 0, 0, 0, 7]);
        assert_eq!(circuit_of(&other), None);
    }
}
