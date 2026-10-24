# Development Roadmap

**Status:** Stage 1 in progress. Milestones M0, M1 and M2 are done, and M3 is under way; see below.

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
- **M2 — Android client and always-on node (done):** pairing by QR code, feed, playback, posting through the home node, on Wi-Fi and mobile data; the node runs as a service that starts at boot and reports its health to the phone.
  - Done: device pairing and the client protocol ([ADR 0008](decisions/0008-device-pairing-and-client-protocol.md)), health reporting, `osp service install`, and `osp-client` (the phone's networking, tested against a real node). See [M2 testing](testing/m2-phone.md).
  - Built: the Flutter Android app ([`app/`](../app/)) on top of `osp-client`: pairing, feed, playback, posting, reshare, follow codes and node health.
  - Tested on two phones against a node running as a service: pairing by QR code and by pasting, feed, playback with seeking, posting a photo, the offline banner, unpairing a phone while it plays, a relayed path standing in for a phone away from home, and the node starting at boot so both phones reached it before anyone logged in ([results](testing/m2-phone.md#results-to-record)).
  - Not tested on mobile data itself: the test phones have no SIM, so the relayed path on home Wi-Fi stood in for it.
- **M3 — limits, resilience and the follower swarm (in progress):** upload limits with "busy, try another host", local peer assessment (measured first; [doc 12](12-peer-reciprocity-and-recognition.md)), follower swarm basics and staggered prefetch ([doc 13](13-hosting-roles-and-follower-swarm.md)), quotas and eviction, resumed transfers, a simulated slow/flaky network harness, and the relay-only privacy mode ([ADR 0007](decisions/0007-transport-privacy-defaults.md)).
  - Done: limits on what other nodes and devices can make a node do: requests, followers, the addresses it keeps, object sizes, sync, and the disk space their media takes, with quotas and eviction ([ADR 0003](decisions/0003-native-social-protocol-v0.md), [ADR 0004](decisions/0004-signed-objects-and-ids.md), [ADR 0008](decisions/0008-device-pairing-and-client-protocol.md)).
  - Done: relay-only mode, opt-in, which hides the node's address from other nodes and phones, and only safe relays taken from other people's addresses ([ADR 0007](decisions/0007-transport-privacy-defaults.md) phase 1, [testing](testing/relay-only.md)).
  - Done: limits on posting media: one transcode at a time with time limits, a cap on posts in progress, and disk space set aside for an upload before it is sent ([ADR 0005](decisions/0005-media-profile-v1.md), [ADR 0008](decisions/0008-device-pairing-and-client-protocol.md)).
  - Done: limits on serving media: transfers at once (fewer for strangers than for followers, fewer with a slow upload cap, with places of their own for small files such as thumbnails), "busy" when they're all taken so the requester tries another host, a prefetch refused as busy tried again later, and an optional upload speed cap ([ADR 0003](decisions/0003-native-social-protocol-v0.md)). With the follower swarm on, a busy requester is pointed at followers who have a copy (below).
  - Done: follower swarm basics ([doc 13](13-hosting-roles-and-follower-swarm.md) phase 1, accepted 2026-10-24), behind a switch that is off by default. Prefetched media is its own class, `prefetch`, served to anyone when the owner shares it (`[swarm] share_prefetched`). A node that shares spreads its prefetches out over up to 10 minutes, tells the author's node when it holds a copy, and fetches from the followers that node names before the author. The author's node names them only to its followers. In the [B8 test](testing/b8-creator-upload.md#results) with 300 followers, the creator sent 20 copies of a new video instead of 300, and every follower had it within 14 minutes. With 100 followers, or a fifth of them stopping partway, the creator stayed within its mark and followers still had the video within 13 minutes.
  - Done: resharers' swarms (doc 13 phase 2, first half). A resharer that hosts a post's media keeps track of which of its followers hold a copy, and names them to its other followers, so people who see a post through a reshare get it from each other. The creator's node is always tried last. In the repeated B8 strangers run, 200 nodes following only the resharer all had the video 12 minutes after the reshare, against 152 after an hour before, and the creator sent 32 copies in all instead of 160.
  - Done: a responsive feed: a new post whose video or large photo is still on its way to the node is held back until it's there (at most 20 minutes), then shown on top, so a video tapped in the feed is usually on the node already. One that isn't is fetched from followers with a copy before its author, as prefetches are ([ADR 0003](decisions/0003-native-social-protocol-v0.md)).
  - Done: the phone's device key kept in its secure storage, encrypted with a key held in the Android Keystore ([ADR 0008](decisions/0008-device-pairing-and-client-protocol.md)).
  - Done: local peer measurement, the first phase of [doc 12](12-peer-reciprocity-and-recognition.md): each node keeps a private, fading record of how the nodes it deals with behave (reachable, answering, downloads by outcome, bytes each way) and shows it with `osp peers`, without changing behaviour yet ([ADR 0011](decisions/0011-measuring-peers.md)).
  - Done: a simulated network harness, `osp-sim`, running a creator and hundreds of followers as real nodes in one process, and B8 measured with it ([plan and results](testing/b8-creator-upload.md)): in the baseline, before the swarm, a creator with 300 followers sent 8 copies of a new video, and the other 292 followers didn't get it ahead of time. The harness can also stop some followers partway through, and add strangers who see the post through a reshare. Still to come: delay, loss and slow peers in the harness.
  - Done: release builds of the app signed with a key kept off GitHub ([app README](../app/README.md#release-signing)), and a CI check that no secret or key file is ever committed.
- **Before any public release:** identity backup and recovery ([ADR 0006](decisions/0006-desktop-first-always-on-node.md)). Built ([ADR 0009](decisions/0009-identity-backup-and-recovery.md)): recovery words, an optional encrypted backup file, and a `Moved` message so followers find a replacement node. Built ([ADR 0010](decisions/0010-signing-keys-and-revocation.md)): the identity key kept only as the recovery words, certifying a signing key on the node, so a copied signing key can be replaced or revoked, and followers drop what it signed.

Original Stage 1 checklist:

- [x] Pair two or more trusted test devices. (Two Android phones paired with one node.)
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
- [ ] Identity backup and recovery first ([ADR 0006](decisions/0006-desktop-first-always-on-node.md), [ADR 0009](decisions/0009-identity-backup-and-recovery.md)): a fediverse account needs a stable identity.
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
