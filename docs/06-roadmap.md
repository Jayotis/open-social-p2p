# Development Roadmap

**Status:** Stage 1 in progress. Milestones M0 and M1 are done, and M2 is under way; see below.

## Stage 0 — Requirements and research (current)

- [ ] Agree on the project's scope, contributors' expectations and non-goals.
- [x] Decide whether to build on an ActivityPub backend such as Mastodon or Friendica. Decided: a native protocol for Stage 1, with an ActivityPub bridge in Stage 2 ([ADR 0003](decisions/0003-native-social-protocol-v0.md)).
- [x] Review libp2p, IPFS, WebRTC and relay models. Decided: iroh and iroh-blobs ([ADR 0002](decisions/0002-rust-and-iroh.md)). PeerTube is still to evaluate for longer video.
- [ ] Produce threat model and privacy requirements.
- [ ] Choose identity, data ownership, audience and media-manifest models.
- [ ] Specify adaptive peer grading and bandwidth-fairness goals.
- [ ] Define open, independently implementable feed ranking, chronological mode and "Why am I seeing this?" explanations.
- [ ] Research distributed public trend signals, aggregation privacy, federation coverage and Sybil/spam resistance.
- [ ] Evaluate manipulation scenarios before using P2P demand as any recommendation input.
- [ ] Specify optional exploration modes for overlooked work and topic-based differing perspectives.
- [ ] Research verifiable discovery trails and anti-gaming measures for early curators ('scouts').
- [ ] Define voluntary offline referral links and private-by-default local discovery history.
- [ ] Assess bounded, short-lived P2P discovery snapshot proposals before any private-data aggregation.
- [ ] Describe licensing, documentation and governance policies.

**Gate:** Architecture decision records reviewed; basic publish/reshare/retrieve flows specified.

## Stage 1 — Isolated desktop/P2P proof of concept (in progress)

Stage 1 is split into milestones:

- **M0 — decisions and skeleton (done):** [decision records](decisions/) 0001–0007, Cargo workspace, CI.
- **M1 — node-to-node prototype (CLI) (done; tested on one machine and across machines, direct and relayed):** identities, signed posts, follow tickets, outbox sync, media transcoding, reshare with hosting, verified retrieval with the origin offline. See [Stage 1 manual testing](testing/stage1-manual.md).
- **M2 — Android client and always-on node (in progress):** pairing by QR code, feed, playback, posting through the home node, on Wi-Fi and mobile data; the node runs as a service that starts at boot and reports its health to the phone.
  - Done: device pairing and the client protocol ([ADR 0008](decisions/0008-device-pairing-and-client-protocol.md)), health reporting, `osp service install`, and `osp-client` (the phone's networking, tested against a real node). See [M2 testing](testing/m2-phone.md).
  - Built: the Flutter Android app ([`app/`](../app/)) on top of `osp-client`: pairing, feed, playback, posting, reshare, follow codes and node health.
  - Tested on a phone on home Wi-Fi, against a node running as a service: pairing, feed, playback with seeking, posting a photo, and the offline banner ([results](testing/m2-phone.md#results-to-record)).
  - Still to test: the phone away from home (mobile data or another network, so the path is relayed), and the node coming back after a reboot without anyone logging in.
- **M3 — limits, resilience and the follower swarm:** upload limits with "busy, try another host", local peer assessment (measured first; [doc 12](12-peer-reciprocity-and-recognition.md)), follower swarm basics and staggered prefetch ([doc 13](13-hosting-roles-and-follower-swarm.md)), quotas and eviction, resumed transfers, a simulated slow/flaky network harness, and the relay-only privacy mode ([ADR 0007](decisions/0007-transport-privacy-defaults.md)).
- **Before any public release:** identity backup and recovery ([ADR 0006](decisions/0006-desktop-first-always-on-node.md)).

Original Stage 1 checklist:

- [ ] Pair two or more trusted test devices. (Pairing works with the pretend phone; the Android app is next.)
- [x] Share and verify a piece of public test media using content hashes.
- [x] Let a resharer optionally retain and serve the media.
- [x] Turn off the origin and retrieve from an authorized replica.
- [ ] Measure transfer performance across fast, slow and unstable peers.
- [ ] Enforce storage and bandwidth limits.
- [ ] Test peer discovery and relay/NAT scenarios.

**Gate:** Reproducible test results, including failure cases and privacy observations.

## Stage 2 — Federation through an ActivityPub bridge (future)

Stage 1 uses a native protocol ([ADR 0003](decisions/0003-native-social-protocol-v0.md)) because ActivityPub delivers to each account's public HTTPS inbox, which a home node doesn't have. Stage 2 adds a bridge: a gateway with a public HTTPS address that translates between the fediverse and home nodes.

- [ ] Decision record for the bridge: who runs gateways and who pays, how a home identity maps to a fediverse account and its keys, and what a gateway can see. Or adopt an emerging standard if one suits home nodes better.
- [ ] Identity backup and recovery first ([ADR 0006](decisions/0006-desktop-first-always-on-node.md)): a fediverse account needs a stable identity.
- [ ] Replies, likes and boosts in the native protocol, so interactions cross the bridge both ways.
- [ ] Media for fediverse servers, which fetch over HTTPS: the gateway fetches from home nodes and replicas, with caching and cost limits.
- [ ] Moderation, blocking and reporting for content arriving from other servers.
- [ ] Show bridged followers and replies in the app.

**Gate:** A Mastodon account can follow a home node, see its posts and media, and reply, without its user installing anything.

## Stage 3 — Flutter and desktop early alpha (future)

- [ ] Implement onboarding, posts, timeline, profiles and reshares.
- [ ] Build guided companion setup and pairing.
- [ ] Add availability indicators and hosting preferences.
- [ ] Implement mobile-friendly playback and optional caching.
- [ ] Accessibility and security testing on supported platforms.

**Gate:** A nontechnical test user can complete core journeys.

## Stage 4 — Communities, video and broader federation (future)

- [ ] Define and implement communities/group permissions and events.
- [ ] Test dedicated video service and adaptive stream integration.
- [ ] Validate private-content architecture before enabling private P2P.
- [ ] Develop reporting, appeals and operator tools.
- [ ] Load test popular media distribution and estimate relay costs.

**Gate:** Documented service sustainability and governance assumptions.

## Stage 5 — Public release preparation (future)

- [ ] Independent security review and fixes.
- [ ] Clear privacy and content policies.
- [ ] Compatibility and migration documentation.
- [ ] Deployment, accessibility and disaster-recovery procedures.
- [ ] Reproducible packaging and release process.

## Success measures to agree on

User onboarding completion; original-node bandwidth saved; source distribution fairness; transfer success rate; median and tail media start latency; bytes carried by optional relays; privacy leaks discovered in testing; moderation response time; and fediverse users' ability to interact without installing anything.
