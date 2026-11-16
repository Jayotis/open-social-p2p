# Node Capability Profiles

**Status:** Later planning. The names below describe resource presets for the same OSP node software. They do not grant social authority, ranking priority or governance power.

A person can start on an existing computer, move to a small always-on box or upgrade later without changing identity.

The three useful working names are **Home**, **Library** and **Studio**. "Hunter" is not used here because OSP already uses Hunter for a person who finds good content early ([doc 10](10-exploration-divergence-and-hunters.md)).

## Home

The normal profile.

It should handle:

- identity and everyday social activity;
- Home/Universe and later Front Page/Media;
- ordinary posting and transcoding;
- modest search/discovery;
- bounded optional media hosting;
- simple local moderation helpers.

The defaults should work on modest storage and home upload.

## Library

A node that volunteers more storage and background discovery/distribution work.

Possible larger budgets:

- public discovery/index cache;
- found public metadata;
- replica storage;
- background search/index maintenance;
- public content distribution.

Calling a machine a Library node does not make its owner a Hunter, give it standing, or make its results more trusted.

## Studio

A node tuned for heavier content creation.

Possible larger budgets:

- parallel/faster transcodes;
- originals and media storage;
- upload serving;
- prepared thumbnails/renditions;
- local creator tools;
- stronger local media or AI processing where hardware permits.

A Studio node still publishes the same OSP posts and follows the same rules as Home.

## Same software, different limits

These should be presets over ordinary settings, not three forks of OSP.

| Resource | Home | Library | Studio |
| --- | --- | --- | --- |
| Discovery/index budget | Normal | Higher | Normal/higher |
| Public replica storage | Normal | Higher | Higher |
| Media transcodes | Low/normal | Low/normal | Higher |
| Upload serving | Normal | Higher | Higher |
| Local moderation/AI | Hardware-dependent | Hardware-dependent | Hardware-dependent |
| Social/governance authority | Same | Same | Same |

The actual numbers come from measurements later.

## Resource controls

Whichever preset is used, the owner controls:

- disk quotas;
- upload cap;
- concurrent transfers;
- transcode/CPU budget;
- public indexing/replication;
- cache eviction;
- optional moderation-helper resources.

The node should report when it cannot do something within those limits rather than silently consuming more.

## Testing

When these presets become real, check at least:

- the minimum Home hardware under low disk/upload;
- moving one identity between profiles;
- Studio media load without starving social/control work;
- Library indexing under hostile public input;
- changing profile never changes social rank or governance authority;
- moderation/AI work cannot exhaust the node.

## Later hardware products

Prebuilt OSP Home/Library/Studio boxes are a possible future product, not part of this plan. Vendor choices, SBC models and product SKUs are parked in [notes/future-ideas.md](../notes/future-ideas.md) until real workloads have been measured.
