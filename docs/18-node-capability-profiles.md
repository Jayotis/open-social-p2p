# Node Capability Profiles

**Status:** Planning guide. This document describes capability profiles for the same OSP node software. It does not grant social authority, ranking priority or governance power.

OSP may eventually be distributed on different hardware products, but the network should not fragment into incompatible node types. A **Home**, **Hunter** or **Creator** node is the same OSP software with different resource budgets, enabled work and recommended hardware.

See [ADR 0001](decisions/0001-home-node-architecture.md) and [doc 15](15-feature-responsibility-model.md).

## Principle

> More capable hardware may do more work for its owner and volunteer more resources to the network. It does not get more votes, more organic reach or special trust merely because it is expensive or fast.

A person can begin on a desktop, move to an SBC, or upgrade storage/CPU later without changing identity.

## Common responsibilities

Every supported node profile must be able to:

- hold the owner's OSP signing state;
- pair with the owner's app;
- publish and sync social objects;
- keep Home/Universe/Front Page state as applicable;
- verify signatures and governance;
- store the owner's posts and settings;
- fetch/play media for the owner;
- enforce personal blocks/mutes/moderation;
- participate safely within explicit CPU/storage/bandwidth limits;
- update, back up and recover according to the same protocol rules.

Features must not require Creator-class hardware merely to participate socially.

## Home node

The normal profile for an ordinary person.

Primary goals:

- low power and low administration;
- reliable always-on identity/social state;
- everyday media consumption;
- modest local discovery/ranking;
- ordinary posting/transcoding;
- bounded optional replica hosting;
- personal moderation assistance where hardware permits.

Expected hardware can range from a person's existing desktop to a small always-on box.

The Home profile should prefer conservative defaults so OSP remains usable on modest storage and upload connections.

## Hunter node

A Hunter profile opts into doing more **discovery and public-network assistance**.

Potential additional work:

- larger public discovery cache/index;
- broader exploration of directories and public content;
- more storage for found public metadata;
- more background checks/referrals where protocol permits;
- greater voluntary public media distribution;
- more capable local search and curation tools.

Hunter status is a local role/configuration, not a signed badge that automatically raises someone's standing. If OSP later recognizes useful Hunter activity, that recognition must come from verifiable public actions and separate ADRs, not from a config flag or hardware SKU.

## Creator node

A Creator profile is optimized for people publishing substantial media.

Potential additional work:

- faster/multiple media transcodes;
- larger originals and post-media storage;
- higher outbound serving limits;
- more prepared renditions/thumbnails;
- larger local creator analytics derived from the owner's own node observations;
- stronger resilience when many followers request a new post;
- optional local AI/media tools if the hardware supports them.

A Creator node still publishes ordinary OSP signed posts and uses the same distribution protocol. Its extra resources improve availability and workflow, not organic ranking.

## Profiles are composable

Profiles should be implemented as named presets over explicit capabilities/limits rather than hard-coded product branches.

Example categories:

| Capability | Home | Hunter | Creator |
| --- | --- | --- | --- |
| Discovery/index budget | Normal | High | Normal/High |
| Public replica storage | Normal | High | High |
| Media transcode concurrency | Low/Normal | Low/Normal | High |
| Upload serving budget | Normal | High | High |
| Local moderation/AI budget | Hardware-dependent | Hardware-dependent | Hardware-dependent |
| Community indexing | Joined/needed | Broader opt-in | Joined/needed |
| Governance authority | Identity-based only | Identity-based only | Identity-based only |

Actual numbers must come from measurement, not this table.

A person may enable Hunter-like discovery on Creator hardware, or use a high-storage Home node without calling it a Hunter.

## Resource policy

Each capability has explicit owner controls and safe defaults:

- disk quota by class;
- upload cap;
- number of concurrent transfers;
- CPU/transcode concurrency;
- schedule/quiet hours where useful;
- whether public indexing/replication is enabled;
- cache eviction;
- moderation-assistant CPU/RAM limits.

The node reports when hardware or limits prevent work. It should degrade gracefully instead of pretending a feature is complete.

## Hardware products

If OSP offers prebuilt hardware later, product names may map to these presets:

- **OSP Home** — simple always-on personal node;
- **OSP Hunter** — more storage/network capacity for discovery and distribution;
- **OSP Creator** — stronger CPU/storage/network for heavy media publishing.

The software remains installable on user-owned hardware. Buying official hardware must not be required to access protocol features available within the user's resources.

## Testing

Every feature should identify the minimum supported node profile and be tested there.

Important tests:

- Home profile under low upload/storage;
- same identity moved between profiles;
- Creator load without starving ordinary social/control traffic;
- Hunter discovery/storage bounded under hostile public input;
- changing a profile never changes governance or ranking authority;
- optional AI/moderation workloads cannot exhaust the node's social/network duties.

## Revisit

Revisit this document when real hardware benchmarks exist for candidate SBCs and when the Product Alpha gives enough workload data to choose meaningful default budgets.
