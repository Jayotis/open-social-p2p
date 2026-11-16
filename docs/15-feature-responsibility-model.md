# Feature Responsibility Model

**Status:** Planning guide. It does not change OSP's protocol or Product Alpha. It puts the existing architecture from [ADR 0001](decisions/0001-home-node-architecture.md), [ADR 0008](decisions/0008-device-pairing-and-client-protocol.md) and [ADR 0010](decisions/0010-signing-keys-and-revocation.md) into one rule for future work.

> **The app shows things and captures what the person wants. The home node does the lasting work.**

The phone talks only to its own node. A new screen does not turn the phone into another OSP peer.

## Where work belongs

| Part | What it does |
| --- | --- |
| **App** | Display, interaction, camera/media capture, editing, playback and accessibility |
| **Client API** | Carries the person's requests to their node and returns results/progress |
| **Home node** | Stores state, signs and verifies, discovers, indexes, ranks, prepares media, applies policy and keeps working while the phone is off |
| **OSP protocol** | Carries facts or actions that another independent node must be able to verify |
| **Tests** | Prove the whole journey works across the app, node and other nodes |

A feature may need little or no work in one of these parts. That is good. Do not add a network protocol just because a new screen needs data.

## App

The app should handle work that needs the person's attention or suits a phone:

- navigation and accessibility;
- camera and microphone capture;
- light editing and preview;
- playback and touch controls;
- composing posts, replies and reports;
- showing moderation choices and other decisions;
- asking for confirmation;
- enough local cache for a responsive/offline view and resumable uploads.

The app should not:

- crawl directories or community indexes itself;
- keep a second copy of the social database;
- verify remote governance or protocol signatures;
- hold the OSP identity signing key;
- contact arbitrary OSP peers for ordinary features;
- run a second recommendation system that disagrees with the node.

## Client API

The app should ask for the outcome the person wants, not build protocol objects itself.

Examples:

- follow this person;
- join this community;
- give me the next Media items;
- publish this draft;
- hide this item for me;
- approve this moderation action.

The node checks the current state, creates and signs anything needed, does the network work, and returns a result the app can show.

Long work should have durable progress. Restarting the phone must not accidentally create a second post, moderation action or other network action.

## Home node

The node is the person's always-on participant in OSP. Depending on the feature, it may:

- keep identity and signing state;
- fetch and verify profiles, posts and community records;
- maintain local search/indexes and subscriptions;
- build Home, Media or Front Page candidates;
- apply blocks, mutes and content preferences;
- prefetch thumbnails/media within limits;
- transcode uploads;
- cache and optionally serve public media;
- synchronize while the phone is off;
- retry outgoing work when another node returns;
- run optional moderation helpers under local limits;
- give the app plain-language status and put technical details under Advanced.

A local choice made by the node for its owner does not become a rule for everybody else.

## OSP protocol

Add protocol only when another independent node must verify or act on something.

Examples:

- a signed post or Delete;
- a public community's identity;
- placing a post in a community;
- a community moderation action;
- an index snapshot another node can verify.

Screen layout, personal mute words, local ranking weights and a local spam score normally stay off the wire.

## How to plan a feature

Do not require one parent issue plus five child issues. For a solo project, use this checklist in the issue or design note:

1. What problem does the person see?
2. What belongs in the app?
3. What keeps working on the node after the app closes?
4. Does another node need to verify anything new?
5. What app-to-node request expresses the person's intent?
6. What private information stays local?
7. What CPU, storage, bandwidth and abuse limits are needed?
8. What end-to-end test proves it works?

Split work into separate issues only when that makes parallel development or review easier.

## Testing

A feature is not finished just because the screen renders or a protocol object verifies.

Check, as needed:

- app interaction and offline/error states;
- app-to-node requests, retry and duplicate prevention;
- node persistence and restart behaviour;
- malformed or hostile remote input;
- a real phone -> real node -> other nodes journey;
- simulation for scale or abuse that is impractical in the lab.

Mock data is fine while building a screen, but it must be clearly test data. The feature gate uses real node responses.

## Moderation and plugins

Scripts, classifiers and AI helpers run **under the node's policy**, not above it. They may suggest actions without receiving the owner's signing keys. See [doc 19](19-node-assisted-moderation.md).

Likewise, more CPU, disk or bandwidth lets a node do more work; it does not grant more votes, organic reach or community authority.

## Product Alpha

[ADR 0030](decisions/0030-product-alpha-before-testers.md) remains the current release gate. This guide is for work that follows it and does not change P9-P12.
