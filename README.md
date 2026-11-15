# Open Social P2P

**Website:** [osp2p.com](https://osp2p.com) · [Roadmap](docs/06-roadmap.md) · [Architecture decisions](docs/decisions/) · [Testing records](docs/testing/)

**A free, open-source, home-hosted social network being built as an alternative to Facebook, X, Reddit and YouTube.**

Your identity, posts and media live on a node you control, while your phone is the everyday interface. OSP is designed so no single company has to hold every account, own the entire distribution network or control one global engagement feed.

**Status: Product Alpha in development.** The P2P engineering foundation already publishes, discovers, follows, reshares, hosts and serves media, deletes, replies across Tor, and admits some cross-community replies through signed social evidence. The next release turns that machinery into the first OSP product someone who did not build it would want to install and keep running: **Home · Universe · You**, profiles, exploration, visible network activity, likes/activity, sharing and a polished first-run experience.

Per [ADR 0030](docs/decisions/0030-product-alpha-before-testers.md), the draft mechanism alpha stays unpublished and there is no tester release before the Product Alpha hardening gate.

## What OSP is trying to do

Most social networks put your account, content, social graph, search and delivery behind infrastructure run by one company. OSP moves most of that work back to the people using the network.

- Your **home node** owns your identity, posts, photos, videos, follows, local search index and network state.
- Your **phone app** talks to your own node, so the phone does not need to be the always-on server or carry the network's storage and bandwidth load.
- Public posts and profiles are **signed by their authors**. Media is addressed and verified by hash.
- Nodes exchange social/control traffic over **Tor onion services**, so a stranger discovering your public profile does not automatically learn your home IP address.
- Discovery material from directories is brought to the node and **searched locally**. The words you type into search are not sent to a central search service.
- Followers and people who deliberately reshare with hosting can serve verified copies of media, so a popular creator does not have to upload every copy themselves.

OSP is being built as an alternative to the social space people now split across **Facebook, X, Reddit and YouTube**: profiles and relationships, public conversation, discussion and discovery, photos and video. It is not a feature-for-feature clone of those services today.

The software is **free and open source under AGPL-3.0**. Running a home node still uses the hardware, storage, internet connection and electricity you choose to provide.

OSP is not trying to make every machine anonymous, eliminate every public helper on day one, or pretend the hard parts of abuse, moderation and large-scale transport are already solved. Those trade-offs are documented as architecture decisions rather than hidden behind the app.

## What works today

The project has moved well beyond the original source-only prototype:

- **Local identity and recovery:** an identity is created on the node, with recovery words and signing-key revocation/replacement.
- **Posting:** signed public text, photo and video posts, including transcoding and resumable phone uploads.
- **Following and resharing:** followers sync posts; resharers can deliberately keep and serve a verified public copy.
- **Deletion:** signed deletes and tombstones propagate so stale copies do not simply bring a deleted post back.
- **P2P media:** downloads can use several verified sources, resume, stream to the phone while still arriving, and continue from replicas when the creator is offline.
- **Discovery:** opt-in directories provide signed public profiles, posts and reshares; nodes pull that material and search it at home.
- **Private viewing of found public posts:** a person can open media they found without using their stable public identity for the viewing path. Busy and interrupted sources are tested with fallback to others.
- **Replies:** signed reply threads, retry while a creator is offline, creator reply policies, requests, allow/block/hide controls and deletion.
- **Cross-community reply admission:** a creator's node can automatically admit a stranger's reply when enough independent people near the creator's social world have vouched for that person. The current design was exercised with 300 nodes across 10 simulated communities before being enabled by default.
- **Android and iPhone clients:** pairing, feed, posting, playback, search, replies and node controls have been tested on real phones. The public alpha release is currently centred on the Linux node rather than app-store distribution.

See the [development roadmap](docs/06-roadmap.md), [architecture decisions](docs/decisions/) and [testing records](docs/testing/) for the implementation and the failures that changed the design.

## The next release: Product Alpha

The earlier plan was to hand a mechanism-focused alpha to a few testers and build the social product afterward. [ADR 0030](docs/decisions/0030-product-alpha-before-testers.md) reversed that order: the next public testing round should get something worth exploring, using and showing to someone else.

Product Alpha is built around three places:

- **Home** — the feed and everyday social experience.
- **Universe** — a zoomable view of people, public connections, content and the participating network; it is also the cold-start discovery experience.
- **You** — your profile and your own node, with protocol details moved under Advanced.

Posting and Activity stay reachable throughout the app. Profiles, Universe, visible network activity, connection paths, likes/activity and sharing are built before the public tester release.

The existing [installation notes](docs/install.md) and [releases](https://github.com/Jayotis/open-social-p2p/releases) remain useful for development and clean-install checks, but they are **not currently an invitation to join a public alpha**.

## How the network is split

| Part | What it does now |
| --- | --- |
| **Home node** | Rust service that stores identity/content, syncs, searches, transcodes and serves media |
| **Phone** | Flutter client paired to the owner's node; a window onto the node rather than a server |
| **Public/control endpoint** | Tor-only endpoint for node-to-node social messages |
| **Media endpoint** | Relay-only endpoint for public media when a direct connection is not allowed |
| **Direct endpoint** | Used by the owner's phones and by peers allowed under the node's direct-connection policy |
| **Directories** | Opt-in public discovery/bootstrap; nodes download signed listings and search locally |
| **Follower/resharer swarms** | People who already hold verified media can serve it to others |

The alpha still depends on public relay infrastructure for media paths that cannot or should not connect directly. Replacing more of that relay load with a sustainable OSP-operated or peer-carried transport is active design work; the first two-hop/earned-link model did **not** meet its simulation gates and was deliberately not shipped.

## Privacy and limitations

- **OSP is not an anonymity network.** Tor is used for the control plane, but public posts are public and network participants can still learn information appropriate to the path they use.
- Found public media uses temporary viewing endpoints so the creator does not receive the viewer's stable social identity just because they opened a post.
- Media relays can see network metadata for traffic they carry. Current development uses iroh/n0 relay infrastructure.
- The alpha directories are currently maintained for bootstrapping the test network. A signed, replaceable directory-list design is planned before anything broader.
- Only **public content** is in scope for this alpha. Private groups/content require a separate validated design.
- **ActivityPub is planned, not implemented.** OSP currently uses its own native protocol. A bridge to Mastodon and the wider fediverse is a later stage.

## Evidence, not just plans

Design work in this repository is normally followed by a measurable gate. Examples include:

- real-node publish/reshare/retrieve tests with the original creator offline;
- real-node found-and-hosted tests where a stranger fetched a video from a resharer's copy;
- deletion/tombstone propagation tests;
- Android and iPhone tests against always-on nodes;
- reply tests across three real nodes over Tor and two phones;
- 300-follower media simulations under slow/flaky conditions;
- a 300-node, 10-community simulation for cross-community voucher admission, including a 200-throwaway-identity attacker.

The results, including failed gates and changes made because of them, are kept under [docs/testing/](docs/testing/).

## Technology

- **Rust** for the home node and protocol implementation.
- **iroh / iroh-blobs** for QUIC connectivity, relay fallback and BLAKE3-verified content transfer.
- **Tor** for the public control plane between nodes.
- **SQLite FTS5** for local full-text search.
- **ffmpeg** and **libheif** for the media pipeline.
- **Flutter** plus `flutter_rust_bridge` for Android and iPhone clients.

Stage 1 uses a native OSP protocol. An **ActivityPub bridge** is planned for a later stage so fediverse users can interact without running an OSP node.

## Repository map

| Document | Contents |
| --- | --- |
| [Vision and principles](docs/01-vision-and-principles.md) | What OSP is trying to be, and its non-goals |
| [System architecture](docs/02-system-architecture.md) | Components, data flow and protocol boundaries |
| [Security, privacy and governance](docs/05-security-privacy-and-governance.md) | Threats, privacy goals and open governance questions |
| [Development roadmap](docs/06-roadmap.md) | Completed milestones, alpha work and later stages |
| [Contributing and open decisions](docs/07-contributing-and-open-decisions.md) | How to help and what still needs decisions |
| [Architecture decision records](docs/decisions/) | The decisions that define the current system |
| [Testing records](docs/testing/) | Real-node and simulated experiments, including failures |

Code lives primarily in:

| Path | What it is |
| --- | --- |
| [`crates/osp-proto`](crates/osp-proto/) | Signed objects, wire formats, requests and tickets |
| [`crates/osp-node`](crates/osp-node/) | Home-node storage, networking, media, discovery and social behaviour |
| [`crates/osp-cli`](crates/osp-cli/) | `osp` command, daemon/service, setup and administration |
| [`crates/osp-client`](crates/osp-client/) | Client protocol used by paired phones |
| [`crates/osp-tor`](crates/osp-tor/) | OSP's Tor transport/control-plane integration |
| [`crates/osp-sim`](crates/osp-sim/) | Repeatable multi-node simulation harnesses |
| [`app/`](app/) | Flutter mobile application |

## Building from source

The Product Alpha is not yet released to testers. For development:

```sh
cargo build
cargo test
cargo run --bin osp -- --help
```

See [Stage 1 manual testing](docs/testing/stage1-manual.md) for running multiple development nodes.

## Contributing

The current priority is Product Alpha and the hardening required before outside recruitment. Code review, focused bug reports, privacy/security findings and work that supports the accepted Product Alpha ADRs are especially useful.

Architectural changes are recorded as ADRs so decisions can be reviewed and later superseded without rewriting project history. See [Contributing and open decisions](docs/07-contributing-and-open-decisions.md).

## License

The project is licensed under [AGPL-3.0](LICENSE). [`crates/osp-tor`](crates/osp-tor/) retains MIT or Apache-2.0 licensing for the code it originated from.
