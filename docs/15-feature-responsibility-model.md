# Feature Responsibility Model

**Status:** Planning guide. This document does not change protocol or product behaviour by itself. It makes the architecture already accepted in [ADR 0001](decisions/0001-home-node-architecture.md), [ADR 0008](decisions/0008-device-pairing-and-client-protocol.md) and [ADR 0010](decisions/0010-signing-keys-and-revocation.md) explicit for future product work.

OSP has one persistent participant in the network for each person: **their home node**. The phone is the person's primary interface to that node. Product features such as Home, Universe, Media and Front Page are user experiences in the app backed by work performed on the node.

The rule for future development is:

> **The app captures what the person wants and presents results. The home node performs persistent social, storage, ranking, cryptographic and peer-to-peer work.**

The phone talks only to its own node. It does not become an independent OSP peer merely because a new screen needs network data.

## The five implementation layers

Every substantial product feature is planned under the same five headings.

| Layer | Responsibility |
| --- | --- |
| **App** | Display, interaction, camera/media capture, editing, playback, accessibility and clear user controls |
| **Client API** | The authenticated contract between the app and its paired node: requests, responses, progress and errors |
| **Home node** | Persistent state, signing, verification, discovery, ranking, indexing, storage, media preparation, policy and background work |
| **OSP protocol** | Signed objects and node-to-node requests required for independent nodes to interoperate |
| **Tests** | Unit, integration, phone, multi-node and simulation evidence that the complete journey works |

A feature may need little or no new work in one layer. That is a useful result. A new screen does not justify a new network protocol when the node can already provide the needed information.

## Product experiences are not network roles

The app may expose several product experiences:

- **Home:** people the owner follows and what they share.
- **Universe:** exploration of people, content and the network.
- **Media:** a continuous media viewer, including short-form video.
- **Front Page:** public communities and their discussions.
- **You:** the owner's identity, posts, relationships, node and settings.

These are views of the same identity and node. They do not create separate accounts, storage systems or peer-to-peer networks.

Similarly, Home, Hunter and Creator node profiles are capability profiles of the same node software, not different social classes. See [doc 18](18-node-capability-profiles.md).

## App responsibilities

The app should do what requires the user's attention or what the phone is especially good at:

- navigation, layout and accessibility;
- camera and microphone capture;
- lightweight editing and preview;
- playback and touch interactions;
- composing posts, replies and reports;
- presenting moderation decisions and governance proposals;
- asking for explicit choices and confirmation;
- retaining only the local state needed for a responsive client, offline display and resumable uploads.

The app should not independently:

- crawl directories or community indexes;
- maintain a second social database;
- decide whether a remote governance signature is valid;
- hold OSP identity signing authority;
- contact arbitrary OSP peers for ordinary product features;
- implement a second recommendation engine whose state disagrees with the node.

The app may cache display data, but the node remains authoritative for the owner's OSP state.

## Client API responsibilities

The client API is a deliberate boundary, not an implementation detail.

Requests should express **user intent**, for example:

- follow this person;
- join this community;
- give me the next Media candidates;
- post this draft;
- hide this item for me;
- approve this steward proposal.

The app should not have to assemble protocol records itself. The node validates the current state, creates and signs records where appropriate, performs network work, and returns a result suitable for the app.

For long work, requests expose progress or durable job state rather than requiring the phone to stay awake. A phone restart must not create a second post, vote, moderation action or governance approval.

Client API changes are versioned and tested independently of the Flutter widgets that use them.

## Home-node responsibilities

The node is the person's always-on agent. Depending on the feature it may:

- keep identity and signing state;
- fetch and verify profiles, posts, community records and moderation records;
- maintain local indexes and subscriptions;
- build Home, Media and Front Page candidate sets;
- apply the owner's blocks, mutes, content preferences and moderation policy;
- prefetch thumbnails or media within explicit resource limits;
- transcode media uploaded from the phone;
- cache and optionally serve verified public media;
- synchronize while the phone is off;
- queue outgoing actions until peers return;
- verify community steward authority and quorum;
- run optional moderation-assistance rules or models under bounded permissions;
- expose plain-language state to the app and detailed diagnostics under Advanced.

The node may make local choices for its owner, but those choices do not become network-wide authority unless the owner signs a protocol action that actually has such scope.

## OSP protocol responsibilities

A protocol addition is justified only when independent OSP nodes need a shared, verifiable fact or action.

Examples include:

- a signed community identity;
- a post placement in a community;
- a steward moderation record;
- a delete by the author;
- an index snapshot another node can verify.

Local ranking weights, screen layout, personal mute terms and a local AI spam score generally do **not** belong in the network protocol.

Protocol records have explicit versioning, size/rate bounds, replay rules, signature rules and compatibility tests before product code relies on them.

## Testing responsibilities

A feature is not complete because its screen renders or its protocol object verifies. The test plan follows the same layers.

1. **App tests:** interaction, accessibility, error states and cached/offline display.
2. **Client API tests:** request/response compatibility, retries and idempotence.
3. **Node tests:** persistence, policy, ranking, resource limits and restart behaviour.
4. **Protocol tests:** signatures, malformed peers, replay, conflicting state and version mismatch.
5. **Journey tests:** real phone -> real node -> independent nodes, including outages.
6. **Simulation:** scale, abuse and adversarial cases that are impractical on a few lab machines.

Mock app data is allowed for UI development, but is visibly test data and must not be confused with live discoveries. Before a feature gate passes, the same screen is exercised against real node responses.

## How work is divided in issues

A product feature should normally have one parent issue and separate implementation tasks where parallel work helps:

- **Product/App**
- **Client API**
- **Node**
- **Protocol**
- **Tests**

Each task states what it consumes and produces. A protocol or API task should land before downstream code depends on an unstable shape, unless both sides work against an explicitly versioned mock contract.

A feature issue should answer:

1. What problem does the person see?
2. Which work belongs on the app?
3. Which work continues on the node after the app closes?
4. Does another node need to verify anything new?
5. What client request expresses the person's intent?
6. What private data stays local?
7. What are the storage, CPU, bandwidth and abuse limits?
8. What proves the complete journey works?

## Security boundary

Plugins, scripts, recommendation code and moderation assistants run **inside the node's policy boundary**, not above it. A component may suggest an action without receiving unrestricted signing authority. See [doc 19](19-node-assisted-moderation.md).

The same principle applies to hardware: extra CPU, disk or bandwidth can let a node do more work, but does not grant additional social, ranking or governance authority.

## Relationship to the Product Alpha

[ADR 0030](decisions/0030-product-alpha-before-testers.md) remains the current release gate. This document does not interrupt its order.

After the Product Alpha, new experiences such as Front Page and Media should be planned with this responsibility model from their first issue, rather than being implemented as a screen first and separated later.
