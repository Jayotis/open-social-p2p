# Proposed System Architecture

**Status:** Proposal; interfaces and technology selections have not been finalized.

## User-facing products

- **Mobile app:** Flutter application targeting iOS and Android. Familiar feed, profiles, connections, communities, video, notifications, messaging as designed, and account management.
- **Web client:** Comparable experience in a browser; technology undecided.
- **Desktop companion:** Optional Windows, macOS and Linux application acting as a personal storage and peer-to-peer delivery node. It is not required for a basic social account.

## Logical subsystems

### 1. Social layer

Manages identity, account onboarding, posts, follow relationships, group membership, feeds, interactions, moderation and federation.

**Candidates:** Mastodon or Friendica as a starting backend, with ActivityPub for federation. Neither has been selected. Evaluate compatibility, extension points, API completeness, upgrade cost, group models, moderation and licensing.

### 2. Media ownership layer

The desktop companion maintains a private local library and a separately approved set of publishable assets. It may generate thumbnails or video renditions, track content hashes, enforce quotas, and decide which assets may be shared.

A home node must never expose arbitrary filesystem paths or private material simply because it is installed.

### 3. Content distribution layer

A distinct peer-to-peer transport retrieves media from one or more authorized copies. It should support chunk verification, peer discovery, load-aware source selection, transport encryption, resumable transfers, NAT traversal and optional relays. libp2p/IPFS/Bitswap/WebRTC are **candidates to evaluate**, not committed dependencies.

Social interactions and P2P file delivery are different responsibilities. ActivityPub reposts do not automatically create replicated media copies.

### 4. Optional video layer

PeerTube may supply video channels, transcoding, playback and federation, but its ordinary hosting model is not equivalent to individual home-node storage. Evaluate integration cost rather than assuming it already supplies home hosting.

### 5. Necessary coordination and fallback services

Even with user-hosted media, the network needs some infrastructure: account services, federation, discovery of authorized media providers, moderation, push notifications, identity recovery and potentially relays or optional hosting. These services need privacy, operating cost and scaling plans.

### 6. Open feed algorithm and distributed trending

Feed ordering is a **separate algorithm from P2P peer-performance grading**. Candidate discovery may use public ActivityPub interactions and separately opted-in, privacy-preserving aggregate P2P demand observations. Raw bandwidth, replica counts and download requests are not endorsements.

Provide chronological feeds, optional recommended/trending views, local user controls, transparent explanations and independently implementable published ranking rules. See [Open Feed and Recommendation Algorithm](08-open-feed-algorithm.md) and [Distributed Trending Signals](09-distributed-trending-signals.md).

## Identity and onboarding

The user experiences one signup and one profile. Internally, different services may have distinct identities. Single sign-on does not itself create one portable federated identity.

Design a documented account-mapping model. Advanced federation settings can be discoverable without imposing them on new members.

## Key flows

**Publish public media:** User chooses a file -> desktop node validates and prepares it -> app creates a post with signed, verifiable media references -> authorized nodes advertise availability -> viewers retrieve and verify.

**Reshare public media:** User reshares the post socially -> if peer hosting is enabled, their companion downloads and validates eligible content -> it offers a replica subject to quotas and source policy. Likes alone must not implicitly replicate media.

**View media:** Client discovers candidate providers -> prefers healthy, available sources -> verifies chunks before presentation -> optionally helps distribute permitted public chunks according to device and privacy settings.

**Offline owner:** Other authorized replicas may continue supplying content. If none remain, media is unavailable until a node returns or an optional host/cache is used.

## Interfaces to define before implementation

- Content identifiers, hashes and author-signed manifests.
- Post-to-media references, updates and versioning.
- Discovery and peer authorization.
- Desktop pairing and session keys.
- Peer capabilities and load reporting.
- Moderation and removal signal handling.
- Media access controls and audience changes.
- Storage/relay selection and failure handling.
- Shared login and service identity mapping.

## Evaluation

Prototype the smallest complete scenario and test failure states before selecting a permanent stack: publication from one home, replication by a sharer, retrieval by a third device after the first home goes offline, and limits enforcement.
