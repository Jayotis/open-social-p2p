# Proposed System Architecture

**Status:** Proposal, partly superseded. The home node model, networking stack and object formats are now settled in the [decision records](decisions/); the sections below remain as background where they don't conflict.

## User-facing products

- **Home node:** the person's persistent agent on the network ([ADR 0001](decisions/0001-home-node-architecture.md)). It runs on their desktop or another always-on machine and does the durable work: identity/signing, publishing, following, synchronization, discovery, local indexing/ranking, moderation policy, media preparation, caching and optional replica hosting. A personal node is required to join and is expected to stay on ([ADR 0006](decisions/0006-desktop-first-always-on-node.md)).
- **Mobile app:** Flutter application targeting Android and iOS. It is the person's primary display/interaction/content-capture interface and a client of its own home node only. It pairs once and reaches that node over one encrypted connection at home or away.
- **Future clients:** desktop/web interfaces may expose comparable experiences, but they remain clients of the person's node unless a later ADR deliberately changes that boundary.

The development split is formalized in [Feature Responsibility Model](15-feature-responsibility-model.md): **App -> Client API -> Home node -> OSP protocol -> Tests**. Product experiences such as Home, Universe, Media and Front Page are views backed by the same node, not separate networks.

## Logical subsystems

### 1. Social layer

Manages identity, account onboarding, posts, follow relationships, group membership, feeds, interactions, moderation and federation.

**Stage 1** ([ADR 0003](decisions/0003-native-social-protocol-v0.md)): a native node-to-node protocol over iroh, with objects in ActivityStreams 2.0 vocabulary, instead of building on Mastodon or Friendica. ActivityPub delivers to each account's public HTTPS inbox, which a home node behind a router or VPN doesn't have.

**Federation (Stage 2):** an ActivityPub bridge, a gateway with a public HTTPS address that translates between the fediverse and home nodes, so people on Mastodon and other servers can follow and reply without installing anything. Open questions include who runs gateways, how a home identity maps to a fediverse account, how fediverse servers get media that lives on home nodes, and which interactions and moderation tools must exist first. See the [roadmap](06-roadmap.md#stage-2--federation-through-an-activitypub-bridge-future).

### 2. Media ownership layer

The home node maintains a private local library and a separately approved set of publishable assets. It may generate thumbnails or video renditions, track content hashes, enforce quotas, and decide which assets may be shared.

A home node must never expose arbitrary filesystem paths or private material simply because it is installed.

### 3. Content distribution layer

A distinct peer-to-peer transport retrieves media from one or more authorized copies. It should support chunk verification, peer discovery, load-aware source selection, transport encryption, resumable transfers, NAT traversal and optional relays. Stage 1 uses iroh and iroh-blobs ([ADR 0002](decisions/0002-rust-and-iroh.md)).

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

**Reshare public media:** User reshares the post socially in the app -> their home node performs the signed/network action -> if peer hosting is enabled, the node downloads and validates eligible content and offers a replica subject to quotas and source policy. Likes alone must not implicitly replicate media.

**View media:** The app asks its home node for content -> the node discovers/selects candidate providers, retrieves and verifies media, and streams/serves it to the app. The phone does not join the public swarm for ordinary viewing.

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

## Planning documents

Post-Product-Alpha product work is split by responsibility rather than implemented as monolithic screens:

- [doc 15](15-feature-responsibility-model.md): common app/client/node/protocol/test boundary;
- [doc 16](16-front-page-and-communities.md): Front Page/public community implementation plan;
- [doc 17](17-media-experience.md): continuous Media/short-form video plan;
- [doc 18](18-node-capability-profiles.md): Home/Hunter/Creator resource profiles;
- [doc 19](19-node-assisted-moderation.md): local scripts/AI and steward-assistance boundary.

## Evaluation

Prototype the smallest complete scenario and test failure states before selecting a permanent stack: publication from one home, replication by a sharer, retrieval by a third device after the first home goes offline, and limits enforcement.
