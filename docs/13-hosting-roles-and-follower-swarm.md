# Hosting Roles, the Follower Swarm and Expiry

**Status: Phase 1 accepted (2026-10-24); phase 2, resharers' swarms and downloads in parts, built and measured (2026-10-24); phases 3 and 4 are proposals.** Phase 1 is built behind a switch that `osp init` asks about, and passed the [B8](testing/b8-creator-upload.md#results) swarm run on 2026-10-24: with 300 followers, the creator sent 20 copies of a new video instead of 300, and every follower had it within 14 minutes. With 100 followers, or with a fifth of them stopping partway, the marks held too. Strangers arriving through a reshare were the weak spot: the creator sent 200 of them 136 copies in an hour. Once a resharer that hosts kept track of its followers' copies, the same 200 strangers all had the video 12 minutes after the reshare, and the creator sent them 12 copies ([below](#strangers-arriving-from-outside)). With `share_prefetched` under `[swarm]` on, a node shares its `prefetch` copies, spreads its prefetches out, tells the author's node (or a hosting resharer's) when it holds a copy, and fetches from the followers that node names first, and from the author's node last. That node names them only to its followers, whether or not they share; any follower refused as busy is sent to them too. A file of 2 MiB or more comes in parts from up to three of the followers named at once ([below](#how-a-post-spreads)): in B8, a download from the swarm was about three times as fast, and the spread was the fastest yet, with the creator sending 18 copies. Not built yet: a freshness time per host in `WhoHosts`, and choosing sources by peer assessment. The first draft ("Segment hosting, random assignment and content expiry") was taken apart in a [critical review](reviews/2026-10-21-review-of-docs-12-and-13.md); this draft responds to it. Phase 1's gates are met: B8 passed, and a test checks that followers with a copy are named only to followers (`WhoHosts` [reveals the follower list](#whohosts-reveals-the-follower-list)). Sharing is now asked at setup, recommending yes, and stays off without a yes ([ADR 0012](decisions/0012-sharing-prefetched-copies.md)).

## Summary

Popular posts are spread by the people who already have them: **the followers' nodes**. Home nodes prefetch new posts from the people they follow ([ADR 0001](decisions/0001-home-node-architecture.md)), so shortly after a post appears, many followers' nodes hold a verified copy. In this design those copies are served to anyone who asks, and the creator's node points busy requesters at them. The creator uploads a few copies; the swarm handles the rest.

Copies other than the creator's expire when demand fades. People who want to keep content long-term can run their node as a **preservation node**. Blind random hosting, from the first draft, is dropped.

## Goals

1. Stop popular posts from overloading the creator's home connection.
2. Spread load using copies that already exist, rather than creating new ones for cover.
3. Keep content available while people want it, then free the space.
4. Keep the creator's copy on the creator's drive until the creator deletes it.
5. Never claim privacy or legal protection the design doesn't provide.

## The problem in numbers

A typical home upload of 10–20 Mbps can serve roughly **6 to 13** simultaneous streams at the v1-720p bitrate of 1.5 Mbps ([ADR 0005](decisions/0005-media-profile-v1.md)).

The current prototype makes this worse: when someone posts, **every follower's node prefetches the post at once**, before any person has watched it. A creator with 300 followers uploads 300 copies in the first minutes, all driven by machines. This is the first wave, and the follower swarm is aimed squarely at it.

## Hosting roles

| Role | What it holds | Served to | How long |
| --- | --- | --- | --- |
| **Creator's node** (`own`) | The creator's posts, whole files | Anyone | Until the creator deletes them or the disk fails |
| **Follower swarm** (`prefetch`, new) | Posts from people this node follows, and posts they reshared; whole files, fetched automatically | Anyone | Until demand fades, or under disk pressure |
| **Resharer host** (`replica`) | Posts the owner reshared with "host" | Anyone | Until demand fades, or under disk pressure |
| **Preservation node** (`preserve`, new) | Public posts the owner chose to keep long-term | Anyone | Until the owner removes them |
| **Private cache** (`cache`) | Things the owner opened on demand from outside their follows | The owner's own devices only | Under disk pressure |

This amends the serving rules in [ADR 0003](decisions/0003-native-social-protocol-v0.md), which currently keeps every prefetched copy private.

**The phone never hosts.**

## The follower swarm

### What it reveals

A prefetched copy is fetched **automatically because a node follows someone**, not because its owner watched anything. Serving it reveals that **the node follows that person**, or someone who reshared the post. It reveals nothing about viewing. Most social networks show follow relationships anyway.

Things a person opens on demand from outside their follows stay in the private cache and are never served to others.

Under [ADR 0007](decisions/0007-transport-privacy-defaults.md), media connections are direct by default, so peers in a swarm can see each other's IP addresses unless private media is switched on. Setup must say so.

### `WhoHosts` reveals the follower list

A `WhoHosts` reply that lists the followers holding a post, together with their addresses, tells whoever asked **who follows the creator**. They learn this without downloading anything. Peer discovery, address publication and transport privacy are one connected problem ([ADR 0007](decisions/0007-transport-privacy-defaults.md)).

Interim rules, to be tested:

- The creator's node lists swarm members only to **its own followers** (nodes that subscribed), and only a small random sample per request, rate-limited.
- Strangers are pointed only at hosts that are public by choice: the creator, resharers who host (their reshare is public anyway) and preservation nodes.
- In private mode, replies carry relay addresses only, never direct addresses.

The cost of these rules is that strangers arriving through a viral link put more load on the creator and resharers than on the swarm. That trade-off is part of the B8 measurement.

### Consent

During setup the node asks whether to share copies of posts from people the owner follows. It explains what this reveals, recommends yes, and can be turned off at any time. Sharing counts against the node's upload limit. Decided and built in [ADR 0012](decisions/0012-sharing-prefetched-copies.md): `osp init` asks and needs a yes or no; a node never asked, such as one set up before, doesn't share.

### How a post spreads

1. **Notify.** The creator publishes and notifies followers' nodes (already implemented).
2. **Stagger.** Each follower's node waits a random delay before fetching (0–10 minutes as a starting point, longer for creators with many followers), so fetches spread out.
3. **Ask who has it.** Before fetching, a node asks the creator's node `WhoHosts`. The creator's node lists the followers that hold the full file alongside itself, and the node tries them before the creator. Followers that share tell the creator's node when they hold a verified copy (`Hosting`): it can't see downloads from other followers. The node picks sources using its peer assessment ([doc 12](12-peer-reciprocity-and-recognition.md)); for now, at random.
4. **Download from several sources.** A file of 2 MiB or more is fetched in parts from up to three of the followers named as holding a copy, at the same time. Each is asked for one stretch of the file, so it holds one transfer place throughout, as for a whole file. One that finishes takes over the end of the part that will take longest at its node's rate so far, split by the two nodes' rates so they finish together: from a node sending at a hundredth of its rate it takes all but a hundredth of the rest, and from a node that has sent nothing for 2 seconds, all of it. It does so only if that ends the part at least 2 seconds sooner, and until rates are known it takes the back half of the part with the most to come. A host keeps a node's places for a moment after a part, for its next request for the same file, so taking over doesn't mean queuing again. A part a node couldn't send waits for a node that has sent its own. The nodes everyone asks, the trackers and the author's own nodes last, are asked one at a time for anything still missing, so parts never fall on the creator. Every piece is verified against the hash in the signed post. Media opened on demand comes the same way, from the hosts the nodes it follows name. Built and measured in phase 2 ([B8](testing/b8-creator-upload.md#downloads-in-parts)).
5. **Redirect when busy.** Once the creator's node hits its upload limit, it answers "busy" (`RateLimited`). The requester then asks `WhoHosts`, and moves to the followers who have the file. A refusal can't carry the list itself: it's only an error code.

Early fetchers become sources for later ones, so the creator's upload grows with a handful of copies rather than with follower count.

### Strangers arriving from outside

Someone who doesn't follow the creator, for example someone who follows a resharer, asks the creator's node or the resharer's node `WhoHosts`. Under the interim rules above, the creator's node names them only hosts that are public by choice, such as the resharer.

B8 measured what that costs ([strangers after a reshare](testing/b8-creator-upload.md#results)): 200 nodes that follow only a resharer uploading at 5 Mbit/s. Half of them held the video after 44 minutes, and the creator sent them 136 copies in the hour, against 24 for its 300 followers. They sent each other nothing: the resharer's node doesn't keep track of which of its followers hold a reshared post's media, and their nodes fetch a reshared post's media at once instead of spreading their fetches out.

**Resharers' swarms** (phase 2, built 2026-10-24): a resharer that hosts does for its own followers what the creator's node does. They spread their fetches, tell it when they hold a copy, and it names them to its other followers. The rule stays the same: a node is named only to nodes that follow the same node it does. A node fetching through a swarm tries the creator's own nodes, listed in the creator's profile, after every other source, so a resharer naming the creator as a host doesn't send strangers there first. Someone who arrives through a link without following anyone keeps a private copy, which is never served, so their fetches still fall on hosts that are public by choice.

The strangers run was repeated with [pass marks fixed first](testing/b8-creator-upload.md#strangers-once-resharers-keep-track-of-copies), and met them all. With the same 200 strangers, every one held the video 12 minutes after the reshare, against 152 of them after an hour before. The creator sent 32 copies in all, 12 of them to strangers, against 160. Strangers sent each other 185 copies, and the resharer, now mostly a source of names, sent 3. The run also included a second change: a node with an upload cap serves fewer transfers at once, one per 1.5 Mbit/s, so each one finishes in minutes.

## Preservation nodes

People like to keep things, and some will try to keep everything. A preservation node puts that urge to work:

- **Opt in,** with a disk budget.
- **Chooses what to keep:** creators, topics or communities. There is nothing random about it.
- **Keeps whole files,** and serves them to anyone.
- **Deletion:** cooperating preservation nodes honour authors' deletion requests and takedowns. But nobody can force a node to delete public content it already holds, which [doc 05](05-security-privacy-and-governance.md) already warns about. People must be told at publish time that public posts can be kept by others.

## Expiry

Each host counts requests per file. A `prefetch` or `replica` copy is dropped when **both** of these hold:

- it has had no requests for 14 days (starting point), with the exact day randomised per node so hosts don't all drop at once; **and**
- the node has recently confirmed (via `WhoHosts` or `Observe`) that at least 3 other copies exist.

Under disk pressure, evict in this order: `cache`, then the least-requested `prefetch` copies, then `replica`. `preserve` and `own` are never evicted automatically.

Author deletions propagate to followers' and resharers' nodes, which delete their copies.

## Segments: for speed, not for scattering

Files are still downloaded in byte ranges from several sources when that helps (large files, saturated sources). Hosting is always of **whole files**. With the same storage, whole-file copies keep a post far more available than segments scattered across different hosts (see the review, finding B2, and the table below), and they avoid every problem of hosting fragments nobody chose.

If each host is online with probability *u*, an 11-segment file is available with probability:

| Host online probability | Copies | Whole-file copies | Segments scattered across different hosts |
| --- | --- | --- | --- |
| 50% | 3 | 87.5% | 23% |
| 50% | 6 | 98.4% | 84% |
| 80% | 3 | 99.2% | 92% |

If the same hosts each hold every segment, availability equals the whole-file column: the placement matters, not the splitting.

## What was removed from the first draft, and why

| Removed | Reason (review finding) |
| --- | --- |
| Random "help wanted" assignment of segments | Segments can be viewed on their own; flooding and planting illegal content; leaks community membership; costs bandwidth for cover (B1, B4, B5, B9) |
| Claiming deniability for hosts | Downloads remain visible, and statistical identification has precedent (B3, B6) |
| Scattered segment hosting | Lowers availability for the same storage (B2) |
| Encrypting media with the key in the post | The key is public, so it isn't confidentiality; it only prevents casual inspection (second review) |
| Viewer seeding of watched items | Replaced by the follower swarm, which doesn't reveal viewing. It could return later as an opt-in with honest disclosure |

## Data and protocol changes

- **Blob classes:** add `prefetch` and `preserve`; prefetches move out of `cache`.
- **Creator's node, and a hosting resharer's:** records which followers say they hold a copy of each blob it hosts (`Hosting`), to answer `WhoHosts`, and whether each follower shares (`Subscribe`).
- **`WhoHosts` replies:** gain a freshness time per host.
- **Busy redirect:** a `RateLimited` refusal is followed by a `WhoHosts` lookup on the requester's side.
- **Downloads in parts:** no protocol change, since a blob request can already ask for byte ranges. A host keeps a node's transfer places for a moment after a part, for its next part of the same file ([ADR 0003](decisions/0003-native-social-protocol-v0.md)). Each node records who posted each blob, to leave the author's nodes out of the parts.
- **Sync:** random prefetch delay.
- **Config:** `swarm.share_prefetched`, `preserve.enabled`, `preserve.disk_mb`, `expiry.idle_days`, `expiry.min_copies`.

## Phases

1. **Swarm basics:** prefetched copies served publicly, staggered prefetch, `WhoHosts` listing followers, busy redirect, upload limit. Pairs naturally with milestone M3.
2. **Resharers' swarms and parallel range downloads:** a resharer that hosts keeps track of its own followers' copies ([strangers](#strangers-arriving-from-outside); built and measured 2026-10-24), and large files are fetched in byte ranges from several whole-file holders ([how a post spreads](#how-a-post-spreads), step 4; built and measured 2026-10-24).
3. **Demand-based expiry.**
4. **Preservation nodes.**

## Validation

On the test machines, then in simulation:

- One creator, a few hundred simulated followers: measure the creator's total upload with and without the swarm. The [plan and pass marks](testing/b8-creator-upload.md) were fixed before measuring. Measured 2026-10-24, together with the next two.
- A sudden stranger spike after a reshare. Measured 2026-10-24, then again with resharers' swarms.
- Followers going offline mid-spread.
- Expiry under shrinking demand: confirm copies don't all vanish at once.
