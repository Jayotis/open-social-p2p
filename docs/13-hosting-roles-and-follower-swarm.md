# Hosting Roles, the Follower Swarm and Expiry

**Status: Proposal, second draft. Nothing here is implemented.** The first draft ("Segment hosting, random assignment and content expiry") was taken apart in a [critical review](reviews/2026-10-21-review-of-docs-12-and-13.md); this draft responds to it. It becomes a decision record once its blocking findings are resolved.

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

During setup the node asks whether to share copies of posts from people the owner follows. It explains what this reveals, recommends yes, and can be turned off at any time. Sharing counts against the node's upload limit.

### How a post spreads

1. **Notify.** The creator publishes and notifies followers' nodes (already implemented).
2. **Stagger.** Each follower's node waits a random delay before fetching (0–10 minutes as a starting point, longer for creators with many followers), so fetches spread out.
3. **Ask who has it.** Before fetching, a node asks the creator's node `WhoHosts`. The creator's node already knows which followers have fetched the full file, and lists them alongside itself. The node picks sources using its peer assessment ([doc 12](12-peer-reciprocity-and-recognition.md)).
4. **Download from several sources.** A large file can be fetched in parallel byte ranges from several whole-file holders. Every piece is verified against the hash in the signed post.
5. **Redirect when busy.** Once the creator's node hits its upload limit, it answers "busy" (`RateLimited`) together with the list of followers who have the file, and the requester moves to them.

Early fetchers become sources for later ones, so the creator's upload grows with a handful of copies rather than with follower count.

### Strangers arriving from outside

Someone who doesn't follow the creator (for example arriving through a reshared link) asks the creator's node or the resharer's node `WhoHosts`, gets the same list of swarm members and resharer hosts, and downloads from them.

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
- **Creator's node:** records which nodes completed a download of each of its blobs, to answer `WhoHosts`.
- **`WhoHosts` replies:** gain a freshness time per host.
- **Busy redirect:** a `RateLimited` refusal is followed by a `WhoHosts` lookup on the requester's side.
- **Sync:** random prefetch delay.
- **Config:** `swarm.share_prefetched`, `preserve.enabled`, `preserve.disk_mb`, `expiry.idle_days`, `expiry.min_copies`.

## Phases

1. **Swarm basics:** prefetched copies served publicly, staggered prefetch, `WhoHosts` listing followers, busy redirect, upload limit. Pairs naturally with milestone M3.
2. **Parallel range downloads** from several whole-file holders.
3. **Demand-based expiry.**
4. **Preservation nodes.**

## Validation

On the test machines, then in simulation:

- One creator, a few hundred simulated followers: measure the creator's total upload with and without the swarm.
- A sudden stranger spike after a reshare.
- Followers going offline mid-spread.
- Expiry under shrinking demand: confirm copies don't all vanish at once.
