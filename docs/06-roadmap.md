# Development Roadmap

**Status:** Planning only. No implementation milestones are claimed complete.

## Stage 0 — Requirements and research (current)

- [ ] Agree on the project's scope, contributors' expectations and non-goals.
- [ ] Compare Mastodon, Friendica and other ActivityPub backends against required social features.
- [ ] Review PeerTube, libp2p, IPFS, WebRTC and possible relay models without prematurely selecting a stack.
- [ ] Produce threat model and privacy requirements.
- [ ] Choose identity, data ownership, audience and media-manifest models.
- [ ] Specify adaptive peer grading and bandwidth-fairness goals.
- [ ] Define open, independently implementable feed ranking, chronological mode and "Why am I seeing this?" explanations.
- [ ] Research distributed public trend signals, aggregation privacy, federation coverage and Sybil/spam resistance.
- [ ] Evaluate manipulation scenarios before using P2P demand as any recommendation input.
- [ ] Specify optional exploration modes for overlooked work and topic-based differing perspectives.
- [ ] Research verifiable discovery trails and anti-gaming measures for early curators ('scouts').
- [ ] Describe licensing, documentation and governance policies.

**Gate:** Architecture decision records reviewed; basic publish/reshare/retrieve flows specified.

## Stage 1 — Isolated desktop/P2P proof of concept (future)

- [ ] Pair two or more trusted test devices.
- [ ] Share and verify a piece of public test media using content hashes.
- [ ] Let a resharer optionally retain and serve the media.
- [ ] Turn off the origin and retrieve from an authorized replica.
- [ ] Measure transfer performance across fast, slow and unstable peers.
- [ ] Enforce storage and bandwidth limits.
- [ ] Test peer discovery and relay/NAT scenarios.

**Gate:** Reproducible test results, including failure cases and privacy observations.

## Stage 2 — Social integration (future)

- [ ] Choose social backend.
- [ ] Implement simple registration, federation and app integration.
- [ ] Link social posts to independently hosted media.
- [ ] Test consent-based replication when a public post is reshared.
- [ ] Verify moderation, authorization and recovery flows.

**Gate:** Third-party federated account can interact without requiring its user to install the companion.

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

User onboarding completion; original-node bandwidth saved; source distribution fairness; transfer success rate; median and tail media start latency; bytes carried by optional relays; privacy leaks discovered in testing; moderation response time; and ability to use the product without a companion.
