# Open Social P2P

**A home-hosted, peer-to-peer social network: your identity, posts and media live on your own node, while your phone is the everyday interface.**

> **Alpha testers wanted.** OSP has reached the point where the main unknown is no longer whether the pieces can work in the lab, but how they behave on independent home networks. If you run Linux on x86_64 or arm64 and are willing to try an early system, see [Installing a node](docs/install.md) and the [GitHub releases](https://github.com/Jayotis/open-social-p2p/releases).

**Status: early alpha.** Prebuilt Linux releases exist, the node has been exercised across real machines and networks, and Android and iPhone clients have been tested against home nodes. This is still experimental software, not a production social network. The current alpha is focused on public content and Linux home nodes.

## What OSP is trying to do

Most social networks put your account, content, social graph, search and delivery behind infrastructure run by one company. OSP moves most of that work back to the people using the network.

- Your **home node** owns your identity, posts, photos, videos, follows, local search index and network state.
- Your **phone app** talks to your own node, so the phone does not need to be the always-on server or carry the network's storage and bandwidth load.
- Public posts and profiles are **signed by their authors**. Media is addressed and verified by hash.
- Nodes exchange social/control traffic over **Tor onion services**, so a stranger discovering your public profile does not automatically learn your home IP address.
- Discovery material from directories is brought to the node and **searched locally**. The words you type into search are not sent to a central search service.
- Followers and people who deliberately reshare with hosting can serve verified copies of media, so a popular creator does not have to upload every copy themselves.

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

## We need independent alpha testers

The home lab can prove protocol behaviour, but it cannot tell us how OSP behaves across a handful of unrelated homes, ISPs, routers and usage habits.

The next alpha round is looking for a small number of people who can run a node and report what actually happens. Useful testing includes:

- installing from a clean Linux machine and leaving the node running;
- Tor's first-start and reconnect behaviour on a normal home connection;
- finding people and public posts without exchanging follow codes;
- opening a found video and seeing which creator or resharer actually serves it;
- following, posting, resharing and replying between independent nodes;
- seeing whether sharing-with-hosting is something people would choose without being prompted to do it;
- reporting confusing setup, resource use, failures and anything that only appears outside the maintainer's lab.

You do **not** need to build Rust from source to participate. The alpha installer uses release binaries.

## Install the alpha

Current node releases support **Linux x86_64 and arm64 with systemd**. Debian 12, Ubuntu 22.04 and Raspberry Pi OS 12 (64-bit), or newer versions, are the easiest path. A Raspberry Pi 4/5 or another always-on Linux machine is suitable, but a normal desktop is fine for testing.

Start with:

- [Installing a node](docs/install.md) — requirements, installer, updating and removal
- [Releases](https://github.com/Jayotis/open-social-p2p/releases) — prebuilt Linux packages, installer and checksums

The installer creates the node, configures Tor and the alpha directories, installs the user service and shows the node's recovery words. Run the installer as your normal user, not as root.

**Please treat the alpha as disposable experimental software.** Keep the recovery words if you care about the test identity, expect rough edges, and do not use it for private or sensitive material.

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

Alpha testers should normally use a release. For development:

```sh
cargo build
cargo test
cargo run --bin osp -- --help
```

See [Stage 1 manual testing](docs/testing/stage1-manual.md) for running multiple development nodes.

## Contributing

Right now, **independent testing is at least as valuable as more code**. Bug reports that include the distro, hardware, what you expected, what happened and relevant `osp status` output are especially useful.

Architectural changes are recorded as ADRs so decisions can be reviewed and later superseded without rewriting project history. See [Contributing and open decisions](docs/07-contributing-and-open-decisions.md).

## License

The project is licensed under [AGPL-3.0](LICENSE). [`crates/osp-tor`](crates/osp-tor/) retains MIT or Apache-2.0 licensing for the code it originated from.
