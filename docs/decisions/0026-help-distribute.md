# ADR 0026: Help Distribute: hosting public media without a reshare

**Status:** Proposed (2026-11-09), for review. Builds on [ADR 0024](0024-watching-found-posts-without-your-identity.md) and [ADR 0025](0025-shared-posts-are-found-and-hosted.md). Revisits part of [ADR 0016](0016-machine-acquaintances.md)'s rejection of machines serving media, without using the machine name ([ADR 0015](0015-nodes-meet-first.md)).

## Context

[ADR 0025](0025-shared-posts-are-found-and-hosted.md) lets Jimmy share Kim's video and host it, under his profile: everyone sees that "Jimmy" reshared it. Some people would gladly lend bandwidth and disk to spread public media without saying anything under their name. The maintainer decided (2026-11-09) that this is a separate, deliberate act:

- **Share** publishes an attributed reshare under the person's profile, optionally hosting the media.
- **Help Distribute** volunteers bandwidth and storage for public media, and publishes no reshare.
- **Watching or caching alone never starts either.**

**Why not the machine name.** It's the obvious candidate, as it's tied to no person. But it's on Tor only, too slow for video. It's also known widely: greeted, put on maps, kept in tables and passed in lists ([ADR 0015](0015-nodes-meet-first.md), [ADR 0022](0022-lists-and-introductions-are-earned.md)). And ADR 0018 lets a node show a peer which machine is its own. Serving video under it would mean giving it a relay endpoint. It would also tie the machine everyone meets to the set of posts it hosts. Both change what the machine name was accepted to reveal.

## Options considered

1. **Serve under the machine name.** Rejected, above.
2. **A fresh key for every post.** Little to link, but no service record could build up, and each post needs its own checks.
3. **One hosting identity per node,** relay-only, bound to nothing, that the owner can replace. Its record builds up from service, and what it hosts can be linked to it, but not to the person by anything it publishes.

## Decision

Option 3.

### The hosting identity

- **A key of its own,** apart from the public, media, direct and machine endpoints, and from viewing endpoints ([ADR 0024](0024-watching-found-posts-without-your-identity.md)).
- **Relay-only,** with no IP sockets, so it has no address to give. Never on Tor.
- **Bound to nothing.** No binding ties it to the public ID or the machine name, and none of the node's other identities ever names it. It's never in a profile, a follow, a list, a greeting or a reshare.
- **Persistent until replaced.** One per node. The owner can replace it at any time, which drops what it hosts and its service record with others, and starts afresh.

### Help Distribute

- **A deliberate act, on an eligible public post,** whether or not the owner watched it. "Help distribute" is a button of its own, never a side effect of watching, caching, following or sharing.
- **Eligible:** the post is public, not for trusted followers ([ADR 0013](0013-trusted-followers.md)); its author's newest profile is `discoverable`; the node holds no tombstone for it ([ADR 0019](0019-taking-posts-and-media-down.md)); and its media fits [ADR 0005](0005-media-profile-v1.md)'s profile.
- **A complete, verified copy first.** The node fetches every file through the hosting identity, from public sources only, as a viewing endpoint does ([ADR 0024](0024-watching-found-posts-without-your-identity.md)), and checks every chunk. A copy already on the node from watching may be used, as its bytes are checked the same way. Nothing is advertised until the copy is complete.
- **Advertising.** The hosting identity offers the copy to the author's media endpoint, through the relay. The author's node checks a 1 KiB piece, as for a resharer ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)), and notes the key as a distributor of that post.
  - **Who it names them to:** anyone who asks, over Tor or through its media endpoint. It names them by key alone, as nothing else about them is known.
  - **How many:** at most 2 of the 6 hosts it names in a reply (a starting value). Distributor keys cost nothing to make, so they can't fill a reply. The one place for a recently verified host (ADR 0025) can go to one of them.
- **Directories later.** Distributors aren't listed in directories in this record's first step. So while the author's node is off, newcomers find resharers through directories (ADR 0025), not distributors. Listing them waits for measuring.

### Limits the owner keeps

- **Storage:** a class of its own, `distribute`, with its own quota, `distribute_quota_mb`, set when the owner first uses it (default 0: nothing until chosen). A copy isn't evicted while it's offered. With the quota full, a new Help Distribute is refused, with a message saying so, and an older one can be withdrawn.
- **Upload:** within `max_upload_mbps`, as everything is. Distributors' transfers come from the places for strangers, so they never take followers' or devices' places.
- **Withdrawal:** from one post or all, at once. The node stops serving, and tells the author's node that the copy is gone, which then stops naming it. Others keep what they've already seen.

### Deletes

- **Checked without a name.** Once a day, at a time of its own, the hosting identity asks the author's media endpoint whether the post stands. It's answered with the post, or its signed `Delete` ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)).
- **A `Delete` stops it at once,** however it arrives: in that answer, forwarded, or from an outbox the node syncs. The copy is dropped and a tombstone kept.
- **While the author is offline:** a copy whose post was last confirmed standing more than 7 days ago (a starting value) isn't served until it's confirmed again. That bounds how long a deleted post can be served from a distributor that missed the `Delete`.

### Service record

- **The downloader keeps the record.** Each downloader keeps its own record of a distributor key's outcomes ([ADR 0011](0011-measuring-peers.md)), as for any source, and skips one that keeps failing ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)). The author's node rechecks each distributor's copy once a day, and stops naming one after 2 failed checks in a row.
- **Nothing is reported to anyone.** There are no receipts.
- **Service only.** Standing gets a distributor chosen more, and nothing else. Never a direct connection or an address ([ADR 0018](0018-earned-direct-connections.md), the maintainer's decision of 2026-11-09).

## Consequences

**Help Distribute isn't anonymous**, and nothing may describe it so. What each party can observe:

| Who | Can see |
| --- | --- |
| **The author's node** | That a key it doesn't know holds a verified copy of its post, when it offered it, its relay, and how its checks went. If the owner watched before helping, the timing may hint that the viewer and the distributor were one node. |
| **Anyone who asks** | The distributor keys named for a post. Asking many authors, they can gather which public posts one key hosts: a profile of one unnamed node's tastes, for as long as the key lasts. |
| **Downloaders** | The key and its relay, and how well it served them. |
| **Relay operators** (n0's, for now) | The key's connections and the node's IP address. They can link it with the node's other keys that connect from the same address: its media endpoint, its viewing endpoints. Through those, they can link it to the public ID, as they already see which public node a media endpoint serves. |
| **Directories** | Nothing, until distributors are listed. |
| **The node's own followers and peers** | Nothing: none of its identities names the hosting key. |

**What stops the link to the person:** nothing the node publishes ties the key to the public ID, so other nodes and viewers can't make the link from what they're given. What can: the relay's view of IP addresses, timing, a relay URL few others use, and anything the owner says. Replacing the key cuts the set it hosts from the next one.

**Cost:** the upload and disk the owner gives, within the limits; a daily check per post with its author; a 1 KiB check a day by each author.

**Abuse:**

- **False claims** are caught at a check, or at a download, and stop being named.
- **A crowd of keys** gets at most 2 of 6 places, and is held by the limits on strangers' requests at the author's media endpoint ([ADR 0024](0024-watching-found-posts-without-your-identity.md)).
- **Serving bad data** is caught chunk by chunk.

**Changes to other records:**

- **ADR 0016's reasons for keeping media off machines:** speed (a relay endpoint solves it), what hosting shows about its owner (here, about an unnamed key, as the table says), and counting work toward links (standing and addresses are now separate).
- **ADR 0012's swarm** doesn't change.
- **ADR 0025's Share** doesn't change. A node may both share and help distribute the same post; they're separate decisions, served from one stored copy.

## Building it

After [ADR 0025](0025-shared-posts-are-found-and-hosted.md)'s step 3, so the basic journey works first.

| Step | Work | Gate |
| --- | --- | --- |
| 1 | The hosting identity, `distribute` and its quota, Help Distribute and withdrawal, the offer and checks at the author, the daily check, the 7 days | Tests: watching, caching, following and sharing never start it; nothing is offered before the copy is complete and checked; no request from the public, media, direct or machine endpoint during fetching, offering or checking; a non-discoverable author's post or a trusted followers' post can't be chosen; the quota refuses; withdrawal stops serving and naming at once; a `Delete` stops it; a copy unconfirmed for 7 days isn't served; at most 2 of 6 names go to distributors |
| 2 | Real nodes: Jimmy helps distribute Kim's video without watching it; Alice, a stranger, gets part of it from his hosting key | Kim's and Alice's nodes record no public ID, media endpoint or machine name of Jimmy's |
| 3 | Measured in `osp-sim` beside ADR 0025's runs | B8's marks met; recorded: distributors' share of strangers' bytes, and Kim's copies |

**Revisit** if distributor keys come to dominate who's named, if few people use it, or if a relay of our own changes what relays see.
