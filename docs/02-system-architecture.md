# Proposed System Architecture

**Status:** Proposal, partly superseded. The home node model, networking stack and object formats are now settled in the [decision records](decisions/); the sections below remain as background where they don't conflict.

## User-facing products

- **Home node:** the person's agent on the network ([ADR 0001](decisions/0001-home-node-architecture.md)). It runs on their desktop (Linux first, then Windows and macOS) or another always-on machine and does the heavy work: identity, publishing, following and syncing other people's posts, prefetching media, transcoding, and optional replica hosting. A personal node is required to join and is expected to stay on ([ADR 0006](decisions/0006-desktop-first-always-on-node.md)); the software stays able to run on a server so hosted nodes could be offered later.
- **Mobile app:** Flutter application targeting Android first, then iOS. It is a client of its own home node only: it pairs once by QR code and then reaches the node over one encrypted connection, at home or away. Feed, profiles, video, posting and account management.
- **Web client:** Comparable experience in a browser; technology undecided.

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

**Publish public media:** User picks a photo or video on the phone -> it is uploaded to their home node -> the node transcodes it ([ADR 0005](decisions/0005-media-profile-v1.md)), stores the rendition and publishes a signed post whose attachments carry the BLAKE3 hashes ([ADR 0004](decisions/0004-signed-objects-and-ids.md)) -> followers' nodes sync the post and fetch and verify the media.

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
