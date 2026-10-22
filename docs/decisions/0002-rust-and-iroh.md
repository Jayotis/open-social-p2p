# ADR 0002: Rust core on iroh and iroh-blobs

**Status:** Accepted (2026-10-21)

## Context

The home node needs: connections between peers behind NAT without port forwarding, a relay fallback, transport encryption, and verified download of large media files from several sources. The same networking code must also run inside the Android app so the phone can reach its home node from anywhere (ADR 0001). The maintainer develops on Debian Linux and tests on Android.

## Options considered

| Option | Strengths | Weaknesses |
| --- | --- | --- |
| **Rust + iroh 1.x + iroh-blobs** | Connect by public key; hole punching with relay fallback; QUIC; BLAKE3 verified streaming with range requests; one codebase for Linux, Android and later Windows/macOS/iOS; Flutter integration via `flutter_rust_bridge` | iroh-blobs is still pre-1.0 (0.103 at the time of writing; its README recommends 0.35 for production). Relay-only (hide-IP) mode is documented as upcoming, not shipped. Default relays and address lookup are run by n0. |
| Rust + rust-libp2p | Mature DHT, gossipsub, relay v2, hole punching | No maintained Bitswap or equivalent verified-content transfer; more assembly required |
| Go + go-libp2p / kubo (IPFS) | Most mature IPFS stack | Go-to-Flutter on Android is awkward (gomobile and platform channels); heavy binaries; Bitswap is less suited to streaming media |
| Pure Dart | One language with the app | No mature NAT traversal or verified-streaming libraries; would mean writing networking from scratch |

## Decision

Rust for the core, built on **iroh 1.x** for connectivity and **iroh-blobs** for content transfer.

- Every node runs one iroh endpoint. Each protocol is a separate ALPN on it: `iroh-blobs` (content), `osp/social/0` (node to node) and `osp/client/0` (phone to its home node).
- iroh-blobs is used from **one module only** (`crates/osp-node/src/store.rs`). If its API changes before 1.0, or it has to be replaced, the change stays in that module.
- iroh-blobs' provider event hook is used to **refuse any request for content the node has not chosen to serve** (see the serving rules in ADR 0003).
- **Minimum version iroh-blobs 0.103.1.** Earlier versions did not reliably respect the per-request-type settings in `EventMask` (fixed in 0.103.1, 2026-10-06, "Respect the per-request-type modes in EventMask"). The serving rules, and keeping pushes into our store disabled, depend on those settings. The workspace pins this minimum, and CI runs a dependency vulnerability audit.
- The Flutter app reaches the same Rust code through `flutter_rust_bridge` (Stage 1, milestone M2). The phone's side is `crates/osp-client` ([ADR 0008](0008-device-pairing-and-client-protocol.md)).

## Consequences

- **Licensing:** iroh and iroh-blobs are MIT/Apache-2.0, compatible with this repository's AGPL-3.0.
- **Privacy:** during development nodes use n0's public relays and DNS address lookup. Relays see which nodes talk to each other and their IPs, but not content. Before any public release we need either a project-run relay or self-hosting instructions, plus a verified relay-only mode ([ADR 0007](0007-transport-privacy-defaults.md)).
- **Maintenance:** we follow iroh-blobs releases closely until it reaches 1.0.
- **Contributors:** Rust raises the bar slightly for new contributors. Code is kept plain (tokio, serde, anyhow, few generics) to compensate.

## Validation and revisit conditions

- The Stage 1 automated test runs three nodes and passes "origin offline, replica serves" with BLAKE3 verification.
- Manual tests across real home networks (including a VPN) record whether connections are direct or relayed.
- Revisit if iroh-blobs is abandoned or stalls before 1.0, if relay costs dominate, or if browser clients become a priority (iroh's browser support is limited).
