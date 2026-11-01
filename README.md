# Open Social P2P

**Run your own social network at home, connect it to everyone else's, and use it from your phone anywhere.**

**Status: Stage 1 prototype in progress.** A Rust home node ([`crates/`](crates/)) and an Android app ([`app/`](app/)) work together on test devices; limits and resilience come next ([roadmap](docs/06-roadmap.md)). There is no production application yet.

## The idea

Open Social P2P lets you create your own social media platform in your home. It's yours: your feed, your content, your connections. You connect it to a decentralized network of other people's home platforms and reach it from your phone, from anywhere.

There is no corporate administration, no data center holding everyone's posts, no handing your data to someone else, and no asking a company to show you content. You get your friends' content directly from them.

How it works:

- Your **home node** runs on a desktop or another always-on computer. It holds your identity, posts, photos and videos, follows people, gathers their posts and does all the heavy networking. Installing it creates your identity locally; there is no central sign-up ([ADR 0006](docs/decisions/0006-desktop-first-always-on-node.md)).
- The **Flutter phone app** is the remote control for your node: read, watch and post from anywhere, without your phone carrying the storage, data use or network load ([ADR 0001](docs/decisions/0001-home-node-architecture.md)).
- **Popular posts spread through the people who already have them.** Followers' nodes and people who reshare can serve verified copies, so a popular post doesn't overload the creator's home connection ([proposal](docs/13-hosting-roles-and-follower-swarm.md)).

Nodes still need some public helpers to get through home routers: relay servers that pass along encrypted traffic without storing it. During development these are run by the iroh project; project and community relays are planned ([ADR 0002](docs/decisions/0002-rust-and-iroh.md), [ADR 0007](docs/decisions/0007-transport-privacy-defaults.md)).

**Your internet address.** By default your node connects directly to the nodes it exchanges posts and media with, as BitTorrent does, so they can see your home's internet address. In relay-only mode (`relay_only = true` in the node's `config.toml`) everything goes through a relay instead: other nodes, and your own phone, never learn your address, but the relay operator does, and video is slower. Neither mode makes you anonymous ([ADR 0007](docs/decisions/0007-transport-privacy-defaults.md), [testing](docs/testing/relay-only.md)).

Peers should choose sources according to **locally measured speed, latency, reliability and available capacity**, and respect bandwidth/storage limits. Security, IP-address privacy, audience controls, moderation and deletion limitations are essential design requirements.

Design decisions are recorded in [docs/decisions/](docs/decisions/). The first prototype covers node-to-node publishing, resharing with hosting and verified retrieval. The second adds the Android app, paired with its node by QR code, with the node running as an always-on service. The third, under way, limits what other nodes can make a node do, and adds storage quotas and relay-only mode.

## Project documentation

| Document | Contents |
| --- | --- |
| [Vision and principles](docs/01-vision-and-principles.md) | Goals, user experience, non-goals and guiding values |
| [System architecture](docs/02-system-architecture.md) | Social layer, Flutter, media nodes, P2P transport and integrations |
| [Peer distribution and scoring](docs/03-peer-distribution-and-scoring.md) | Replication on reshare, fairness, verification and source selection |
| [App and companion requirements](docs/04-app-and-companion-requirements.md) | Mobile and desktop features and user journeys |
| [Security, privacy and governance](docs/05-security-privacy-and-governance.md) | Threat model, consent, moderation and policy requirements |
| [Development roadmap](docs/06-roadmap.md) | Research, proof of concept, integration, alpha and release milestones |
| [Contributing and open decisions](docs/07-contributing-and-open-decisions.md) | How to help and architectural questions needing review |
| [Open feed and recommendation algorithm](docs/08-open-feed-algorithm.md) | Open-source feed modes, trending criteria, user controls and transparency |
| [Distributed trending signals](docs/09-distributed-trending-signals.md) | P2P-assisted public trend discovery, data minimization and abuse resistance |
| [Exploration, divergent perspectives and discovery scouts](docs/10-exploration-divergence-and-scouts.md) | Overlooked posts, cross-perspective discovery and recognition for early curators |
| [Collective discovery history and referrals](docs/11-collective-discovery-history-and-referrals.md) | Voluntary public recommendations, offline word-of-mouth credit, private local history and P2P discovery snapshots |
| [Peer reciprocity and recognition](docs/12-peer-reciprocity-and-recognition.md) | **Proposal (second draft):** nodes assess their own peers, generous tit-for-tat when busy, creator and scout levels, self-reported host stat |
| [Hosting roles and the follower swarm](docs/13-hosting-roles-and-follower-swarm.md) | **Proposal (second draft):** followers' nodes share prefetched copies, preservation nodes, demand-based expiry |
| [Critical review of docs 12 and 13](docs/reviews/2026-10-21-review-of-docs-12-and-13.md) | Findings against the first drafts, with a disposition for each |
| [Architecture decision records](docs/decisions/) | Accepted decisions: home node model, Rust and iroh, native social protocol, signed objects, media profile, desktop-first node, transport privacy defaults, device pairing and the client protocol |

## Technologies

Selected for the Stage 1 prototype (see the [decision records](docs/decisions/)):

- **Rust** home node built on **iroh** (peer connections by public key, hole punching, relay fallback) and **iroh-blobs** (BLAKE3-verified content transfer).
- **ffmpeg** for transcoding to a low-bitrate 720p profile, and **libheif** for reading iPhone photos.
- **Flutter** for the phone app, on Android and iPhone, calling the same Rust code through `flutter_rust_bridge`.

Still to evaluate:

- **ActivityPub** bridge for federation with Mastodon and others (Stage 2).
- **PeerTube** or adaptive streaming for longer video.

## Building the prototype

Requirements: Rust (stable), ffmpeg, and libheif-examples for iPhone photos (HEIC).

```sh
cargo build
cargo test            # three-node publish / reshare / origin-offline, and phone pairing tests
cargo run --bin osp -- --help
```

See [docs/testing/stage1-manual.md](docs/testing/stage1-manual.md) for running several nodes and testing across machines, [docs/testing/m2-phone.md](docs/testing/m2-phone.md) for running a node as a service and pairing a phone, [docs/testing/relay-only.md](docs/testing/relay-only.md) for relay-only mode, [docs/testing/tor.md](docs/testing/tor.md) for messages over Tor, being built, and [docs/testing/direct-endpoint.md](docs/testing/direct-endpoint.md) for the media and direct endpoints, being built.

| Crate | What it is |
| --- | --- |
| [`osp-proto`](crates/osp-proto/) | Wire formats: signed objects, payloads, requests, tickets |
| [`osp-node`](crates/osp-node/) | The home node: storage, media pipeline, sync, serving phones |
| [`osp-cli`](crates/osp-cli/) | The `osp` command: daemon, service, pairing and everything else |
| [`osp-client`](crates/osp-client/) | What a paired phone runs; used by the Android app |
| [`osp-tor`](crates/osp-tor/) | Messages between nodes over Tor, and the node's own Tor (MIT or Apache-2.0, as the code it started from) |

## Key design questions

Can resharers voluntarily improve availability by seeding verified copies? How can local observations assign work fairly across faster and slower peers? How can we protect private groups, avoid revealing home network addresses, keep always-on home nodes simple to run, and manage abuse in a replicated network?

See the [open decisions](docs/07-contributing-and-open-decisions.md) to participate in those discussions.

## Separate search project

A personal search engine with a home-hosted crawler is a **separate idea and not part of Open Social P2P**.

## License

See [LICENSE](LICENSE) for the repository's current AGPL-3.0 license. [`crates/osp-tor`](crates/osp-tor/) is under MIT or Apache-2.0 instead, as the code it started from. Documentation licensing and third-party compatibility will be reviewed as the project develops.
