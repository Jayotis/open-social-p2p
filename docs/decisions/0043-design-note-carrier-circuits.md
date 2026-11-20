# ADR 0043 design note: carrier circuits

**Status:** Proposed (2026-11-20), for the maintainer's review. ADR 0043's design stage: nothing of carrying is built before this note is accepted. It picks the layer construction, the wire format, and how a hop checks it is on an OSP path and not a general proxy. Who may carry, how standing is earned, how often newcomers get a middle turn, the limits' values and the fallback stay with [ADR 0017](0017-one-hop-forwarding.md) and [ADR 0043](0043-the-media-plane-before-the-alpha.md).

## The path

A viewer V fetches from a host H through V's hop P and H's hop Q, and on a newcomer's turn a machine M between them:

```text
V → P → Q → H        V → P → M → Q → H
```

Say Jimmy (V) watches Alice's (H) video. Jimmy's node picked P, Alice's node picked Q, and on one fetch in ten Kim's new node (M) sits between them. P sees Jimmy's address and Q's or M's, never Alice's. Q sees Alice's and its previous hop's, never Jimmy's. Kim's node sees only P and Q. The video stays encrypted between Jimmy's and Alice's media endpoints, and no hop can read or change it.

This takes the construction of Tor's circuits, not Tor's network and not Arti's code: one connection between neighbours, a circuit ID that means something only on that connection, a circuit built one hop at a time so no hop is handed the whole path, and a layer of encryption for each hop. Arti's `tor-proto` is tied to Tor's cells, link handshake and directory, and isn't meant for use outside Tor. Tor still carries the small requests that arrange the path, as today. Video never goes over Tor.

## The ends' connection runs only over the circuit

The connection between V's and H's media endpoints is ordinary iroh QUIC, with its own end-to-end encryption, and the circuit is its only way through. **V's endpoint and the endpoint H answers circuits on have no internet address and no relay:** iroh's relay is turned off, its IP transports are cleared, and the circuit is added as a custom transport, as [`osp-tor`](../../crates/osp-tor/) already plugs Tor into iroh and [`machine.rs`](../../crates/osp-node/src/machine.rs) builds machines' endpoints. Otherwise iroh would look for a quicker path by itself: it could move Jimmy's connection to a direct link and show him Alice's address, or send him the addresses Alice's endpoint has. That is the failing open ADR 0017 rules out, and this is what stops it.

- V uses a new key for each circuit, made for it and published nowhere, as [`viewing_endpoint`](../../crates/osp-node/src/viewing.rs) does today. P learns an address that's fetching, not an account.
- H answers circuits on an endpoint of its own, apart from any endpoint with internet addresses, and names it when V asks for its endpoints over Tor.
- iroh's custom transports send packets, not a byte stream, so the inner QUIC packets go through the circuit as packets. A lost one is resent by the inner QUIC, end to end; no carrier resends anything.

## Keys for each hop

**HPKE** ([RFC 9180](https://www.rfc-editor.org/rfc/rfc9180)), Base mode, one suite, no negotiation in version 1:

```text
DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305
```

- **Each carrier has a circuit key,** an X25519 key used for nothing else, signed by its machine key and published in its descriptor (below). It's **replaced once a day** (a starting value), and the old one is kept 10 minutes for circuits being built.
- V sets up a context with each hop, bound to `"osp/carrier/1"`, the protocol version, the carrier's machine ID and a fresh circuit nonce. HPKE's exporter gives one key for each direction. No key or nonce is used on two circuits.
- Base mode is deliberate: V checks the carrier through its signed descriptor, and the carrier learns nothing about who V is.
- **A carrier remembers every setup it has seen** (HPKE's encapsulated key) until the circuit key it used expires, and refuses a repeat.

**What this gives up against Tor.** Tor's handshake adds a fresh key from the relay for each circuit. HPKE's Base mode doesn't, so someone who records a carrier's traffic and later steals its circuit key, before it is replaced, can read that day's layer for that carrier: which hop came next and, at Q, the host slot. Not the video, which iroh's encryption still covers. The harm is a path for a day's circuits through one machine, the effort is breaking into that machine, and a daily key bounds it. If that proves too much, `CREATED` can carry a fresh key from the carrier in a later version.

**Library:** the [`hpke`](https://crates.io/crates/hpke) crate (MIT or Apache-2.0), which implements RFC 9180 with this suite and checks itself against its test vectors. The version is pinned and reviewed when building starts.

## A carrier's descriptor

A machine that carries publishes, signed by its machine key:

```text
version · machine ID · carrier endpoint ID · relay URL of the carrier endpoint
circuit key · its validity · what it offers · signature
```

No person's identity, and no IP address. Descriptors come from the machine layer, or from `carriers.txt` for seed carriers (ADR 0043 §6).

A carrier's endpoint is an iroh endpoint with a relay, **published nowhere** (no DNS or pkarr record). A neighbour reaches it by its endpoint ID through the relay in its descriptor, and iroh's hole punch then shows each the other's address, which is what ADR 0017 says a carrier shows the machines next to it. So addresses are seen only by neighbours, never listed.

## Building a circuit

**1. The host's side.** H keeps a connection with Q, as a node keeps one with its relay. Q gives that connection a **host slot**: a random, short-lived token with no address in it. When V asks over Tor, H hands over Q's descriptor, the slot and H's circuit endpoint ID. V never gets H's address.

**2. The first hop.** V connects to P and sends `CREATE` with the HPKE setup for P.

**3. One hop at a time.** V sends `EXTEND` through the circuit so far, naming the next carrier by machine ID with its descriptor. Only the last hop can read it, checks the descriptor's signature, dials that carrier's endpoint, and passes on the next hop's HPKE setup. P learns its next neighbour, never what follows.

**4. Reaching the host.** With Q last, V sends `CONNECT_HOST` with the slot, which only Q can read. Q joins the circuit to the host connection holding that slot. A slot that doesn't exist, has expired or belongs to another connection is refused.

**Reuse.** V may keep a circuit for further fetches from the same host for up to 10 minutes (a starting value), as each hop added costs a round trip to build. Stage D measures the time to start playing with and without reuse.

## Not a proxy

Nothing in the protocol names a place to connect to except an OSP machine ID with a valid descriptor, at `EXTEND`, and a host slot Q made itself, at `CONNECT_HOST`. There's no general `CONNECT`: no IP address, DNS name, port, loopback or local address, or anything outside OSP can be asked for. A malformed or unknown request is refused. A host's hop therefore delivers only to hosts that picked it, as ADR 0017 requires.

## Frames

```text
CREATE · CREATED · EXTEND · EXTENDED · CONNECT_HOST · CONNECTED · DATA · BUSY · DESTROY
```

Each neighbour connection gives a circuit its own random circuit ID; no ID goes along the whole path. A `DATA` frame is:

```text
version · circuit ID · direction · packet number · payload
```

- **Towards the host,** V wraps the packet for Q first, then M, then P. Each hop removes its layer and passes the rest on. Q hands H a packet still under the ends' own encryption.
- **Towards the viewer,** each hop adds its layer, and V removes them all.
- Each layer is ChaCha20-Poly1305 with its nonce taken from the packet number, and authenticates the version, direction, packet number and circuit context with the payload. A hop skipped, repeated or reordered, or a byte changed, fails the check and the packet is dropped.
- Packet numbers only go up. A repeat, or one outside a window, is dropped. A circuit near its packet or byte limit is replaced, never wraps.

**Packet size.** QUIC needs packets of at least 1200 bytes, so the ends' packets can't shrink to make room. Three layers add 48 bytes of tags and the frame 14, so a link between carriers must carry about 1350-byte packets once QUIC's and the network's headers are added. Most home links do, and a link through a relay always does. Stage B measures it on the lab nodes first, and a carrier that can't carry that size between two neighbours isn't used between them. Nothing relies on IP fragmentation.

## Limits

Each carrier enforces its own: circuits at once, circuits from one neighbour, new circuits a minute, bytes a second, its share of the owner's upload, a circuit's life and bytes, the size of setup messages, hops in a path, and setups pending. The values come from the sizing stage. `BUSY` is a normal answer, and carrying never pushes aside its owner's own traffic.

## Failure

A carrier that goes, refuses, times out or fails a check ends the circuit with `DESTROY`. V builds another through other hops or falls back to the project's relays or n0 ([ADR 0044](0044-project-relays-for-the-alpha.md)), never directly to a host whose address it may not learn.

## Who sees what

| Machine | Sees | Doesn't see |
| --- | --- | --- |
| P | V's address, its next hop, timing, bytes | V's account, H's address, the contents |
| M | its two neighbours, timing, bytes | either end's address or account, the contents |
| Q | H's address and slot, its previous hop, timing, bytes | V's address or account, the contents |
| V | the hops it picked, Q's descriptor | H's address |
| H | Q, its own media endpoint | V's address |

Carriers working together learn more, and someone watching the traffic at several places can match timing and volume. If several carrier links go through the same relay, its operator can match them the same way. This doesn't claim Tor's anonymity, as ADR 0017 says.

## Tests

**Stage B** (with ADR 0043's gate):

1. A test video goes H → Q → P → V, and H → Q → M → P → V on a newcomer's turn.
2. In a packet capture, V never exchanges a packet with H, nor H with V.
3. M gets neither end's address, and P no account of V's.
4. A connection over a circuit never gets a direct address or a relay path from iroh.
5. Leaving out any hop's layer, or changing a byte in one, makes the transfer fail.
6. A repeated `DATA` frame, and a repeated setup, are refused.
7. A finished transfer still passes iroh-blobs' hash check, and only then credits the hops.
8. The largest packet that fits through three layers is measured on the lab links.

**Stage C** (with ADR 0017's hostile tests):

9. No request makes a carrier dial an IP address, DNS name, local or loopback address, or port.
10. A made-up or expired host slot, or another host's, is refused.
11. A carrier at its limit says `BUSY` and its owner's own traffic is unharmed.
12. Killing M, P or Q ends the circuit, with a fallback to relays and never a direct connection.

**Stage D:** the 108.5 MB P11 video through a circuit against the project relay and n0, on marks set before the run.
