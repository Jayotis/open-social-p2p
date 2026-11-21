# ADR 0043 design note: carrier circuits

**Status:** Accepted (2026-11-20), the maintainer's decision. ADR 0043's design stage. It picks the layer construction, the wire format, and how a hop checks it is on an OSP path and not a general proxy. Every node carries from the middle by default ([ADR 0017](0017-one-hop-forwarding.md) as amended 2026-11-20). Who may sit next to an end, how standing is earned, how often newcomers get a middle turn, the limits' values and the fallback stay with [ADR 0017](0017-one-hop-forwarding.md) and [ADR 0043](0043-the-media-plane-before-the-alpha.md).

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

Every node carries, so every machine publishes, signed by its machine key:

```text
version · machine ID · carrier endpoint ID · relay URL of the carrier endpoint
circuit key · its validity · what it offers · signature
```

No person's identity, and no IP address. Descriptors come from the machine layer, or from `carriers.txt` for seed carriers (ADR 0043 §6).

*Settled when building (2026-11-20).* A machine's record carries its descriptor, renewed within a minute of a new circuit key or relay, and a signed `started_at`. `carriers.txt` names seed carriers by machine ID only, since a descriptor lasts a day, and a node gets their descriptors from their records. The carrier endpoint's key is kept across restarts and goes with the machine name, so a new name shares nothing with the old. A reader holds a machine's start as the earliest its records have said, and never later than when it first saw that machine. A later or future `started_at` can't make a machine new again. A start is forgotten a month after first seen, once that machine is out of the table: by then a forgotten machine is no newer than a fresh key.

A carrier's endpoint is an iroh endpoint with a relay, **published nowhere** (no DNS or pkarr record). A neighbour reaches it by its endpoint ID through the relay in its descriptor, and iroh's hole punch then shows each the other's address, which is what ADR 0017 says a carrier shows the machines next to it. So addresses are seen only by neighbours, never listed.

**A carrier's address is a machine's, not a person's.** Anyone can build a circuit through a machine it names and see that machine's address next to its machine ID. That's accepted ([ADR 0017](0017-one-hop-forwarding.md#context)): nothing in a descriptor, a frame or a carrier link ties a machine to a person, and the separation ADR 0015 tests stays the protection. No relay hides one carrier from another, and a newcomer in the middle shows its address to both neighbours. A carrier's limit on circuits from one neighbour and on new circuits a minute bounds how fast one machine can make others work for it.

## Building a circuit

**1. The host's side.** H keeps a connection with Q, as a node keeps one with its relay. Q gives that connection a **host slot**: a random, short-lived token with no address in it. When V asks over Tor, H hands over Q's descriptor, the slot and H's circuit endpoint ID. V never gets H's address.

*Settled when building (2026-11-20).* Until ADR 0018, Q is a seed carrier from `carriers.txt`, and H reads its descriptor from its machine record. H dials Q from an endpoint made for that link alone, never from its machine name or its public ID, so Q sees the link's address and nothing that names H's machine or owner. Its circuit endpoint keeps its key across restarts and answers what its media endpoint answers. H hands out the offer in its answer to `Endpoints` over Tor and to a viewing endpoint's media-hosts request, and builds the link again when it closes and once a day after Q's circuit key is replaced.

**2. The first hop.** V connects to P and sends `CREATE` with the HPKE setup for P.

*Settled when building (2026-11-20).* Until ADR 0017's standing is built, V's hop P is a seed carrier too, picked at random from those whose record can be read; when it is Q, the path is one hop. V asks for H's offer the way it already asks who hosts a file: over Tor, or from a viewing endpoint to H's media endpoint. A circuit that can't be built, or a fetch that fails over it, leaves the fetch to the relays as before. A carrier that restarts has a new circuit key, so a viewer forgets a hop's descriptor when a circuit through it fails, and a host reads its hop's afresh for each link.

*Settled when building part 4d (2026-11-20).* V's hop P is a machine with standing with V, or else a seed carrier, never the host's hop Q while another answers. Standing is jobs on 3 different days in the last 30, credited by V after iroh-blobs' hash check, once a day for each hop that isn't a seed carrier V knows. The middle follows the sizing doc's sixth run: one of V's 2 probationers first, once a day each; else, once a day, the earliest into V's table of the machines in their first 7 days by their signed start and new to the table in the last 7 that V never finished a job with; else, on 1 transfer in 20, a machine from V's table at random, one V never worked with first. Only a middle machine takes a probation place, as the host's hop is the host's pick. It leaves when it earns standing, after 30 days, or when a circuit through it fails, and the transfer is then tried once more without it before the relays. There's no middle on one hop, where it would sit next to an end.

*Settled when building part 4d-2 (2026-11-20, the maintainer).* Work offers are asked for, not sent: a carrier doesn't know whom it carried for, as each viewer dials its first hop from a key made for that circuit. Say Mo's machine finishes its first middle job for Jimmy's node. Jimmy's node remembers that work as it does any, for standing and to know Mo's machine isn't new to it, and apart from that may name Mo's machine to those who ask its carrier endpoint on `osp/work-offers/0`; anyone may ask. It names it to at most 2, ever: later work doesn't add handouts. It does so for 30 days from Mo's last finished job for it, so real work keeps the offer going, while Mo's machine is in its table with a carrier that checks out, whose descriptor it hands, and until a circuit with Mo's machine in the middle fails for it. Of the machines it may name, it names the one whose first job for it was latest, so a newer machine goes ahead of one that keeps working; a made-up name can't jump ahead, as a tip needs a finished job. Alice's node asks only when it has room for a newcomer (a probation place free, no fresh machine waiting, no offer held), after a fetch through hops and never in its way, from a key made for the ask. It asks one machine it holds at standing at a time, the one asked longest ago, each at most once a day, never a seed carrier. It takes an offer only of a machine it never worked with that isn't at standing or on probation, keeps at most 2 for 30 days, and asks the offered machine for its descriptor when the one offered has run out. An offered machine has the once-a-day turn when no machine is fresh, and the offer goes once that machine has had any middle turn. After its job it goes through probation like any other; an offer is never standing. This differs from the sizing doc's third run, where a viewer sent each offer to up to 2 viewers it had been the earned hop for: the simulator measured sending, so part 6's lab run is the first look at asking.

*Settled when building part 5 (2026-11-21).* The owner's controls apply at once, without a restart, and are kept in `config.toml`. Carrying's upload share (50%, the sizing stage's) is of the connection's upload speed, which `osp init` asks, the maintainer's decision, as the installer sets no cap and a node that rarely serves is never measured. A node set up before that goes by `max_upload_mbps`, else the fastest it was measured serving, else a slow home's 10 Mbit/s, and status says which. A seed carrier carries with its whole upload unless its owner sets a share. `osp init` also asks the plan's monthly data cap, the maintainer's decision, as a node that carries well earns standing and some owners will carry all they can: carrying keeps to half of it in a UTC month, counting each carried byte in and out, then waits for the next month. The owner can set carrying's own monthly budget instead. Paused, or on a connection its owner says is metered, a carrier ends the circuits it carries, answers new ones `BUSY` and gives out no host slots. ADR 0043 §8's question on the node's own address is asked at setup when the installer names seed carriers, with yes recommended. Until a yes, the node fetches through no hop and its host doesn't link to one, so its own leg stays on relays; a no ends the host's link at once. Nodes set up before part 5 haven't been asked, so they stay on relays until their owners answer (`osp hops yes|no`).

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
- **Added when building (2026-11-20).** Under the last hop's layer, one byte says what the cell asks of it (`EXTEND`, `CONNECT_HOST`, `DATA`, or from it `EXTENDED`, `CONNECTED`, `BUSY`), so a hop in the middle can't tell a packet from a setup. `CREATE`, `CREATED`, `DATA`, `BUSY` and `DESTROY` are frames on a link; the rest travel under the layers. `DATA`'s kind says its direction, which keeps the frame at 14 bytes.
- **Packet numbers towards the viewer** are given by the circuit's last hop. Each layer remembers the numbers it has used, sealing or opening, so no key seals twice with one nonce; a hop that extends the circuit tells the next where to start (`CREATE`'s backward start), past the numbers it still uses itself.

**Packet size.** QUIC needs packets of at least 1200 bytes, so the ends' packets can't shrink to make room. Three layers add 48 bytes of tags, the frame 14 and a command byte under the layers 1 (63 in all), so a link between carriers must carry about 1350-byte packets once QUIC's and the network's headers are added. Most home links do, and a link through a relay always does. Stage B measures it on the lab nodes first, and a carrier that can't carry that size between two neighbours isn't used between them. Nothing relies on IP fragmentation. *Found when building (2026-11-20):* a new QUIC link starts at about 1160 bytes of datagram, and path MTU discovery raised it to 1414 within 3 s on loopback. So the viewer, the host and a carrier dialling its next hop each wait for room for a 1263-byte frame before using a new link, and give it up after the setup timeout.

## Limits

Each carrier enforces its own: circuits at once, circuits from one neighbour, new circuits a minute, bytes a second, its share of the owner's upload, a circuit's life and bytes, the size of setup messages, hops in a path, and setups pending. The values come from the sizing stage. `BUSY` is a normal answer, and carrying never pushes aside its owner's own traffic.

## Failure

A carrier that goes, refuses, times out or fails a check ends the circuit with `DESTROY`. V builds another through other hops or falls back to the project's relays or n0 ([ADR 0044](0044-project-relays-for-the-alpha.md)), never directly to a host whose address it may not learn.

## Who sees what

| Machine | Sees | Doesn't see |
| --- | --- | --- |
| P | V's address, its next hop's machine ID and address, timing, bytes | V's account, H's address, the contents |
| M | its two neighbours' machine IDs and addresses, timing, bytes | either end's address or account, the contents |
| Q | H's address and slot, so whose node it is; its previous hop's machine ID and address; timing, bytes | V's address or account, the contents |
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
11. A carrier at its limit says `BUSY` and its owner's own traffic is unharmed; one machine opening circuits as fast as it can is held to the per-neighbour and per-minute limits.
12. Killing M, P or Q ends the circuit, with a fallback to relays and never a direct connection.

**Stage D:** the 108.5 MB P11 video through a circuit against the project relay and n0, on marks set before the run.
