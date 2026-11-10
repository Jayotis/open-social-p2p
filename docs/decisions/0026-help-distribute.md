# ADR 0026: Help Distribute: hosting public media without a reshare

**Status:** Proposed (2026-11-09), approved in principle, revised after the maintainer's review (2026-11-09), for review again. Builds on [ADR 0024](0024-watching-found-posts-without-your-identity.md) and [ADR 0025](0025-shared-posts-are-found-and-hosted.md). Revisits part of [ADR 0016](0016-machine-acquaintances.md)'s rejection of machines serving media, without using the machine name ([ADR 0015](0015-nodes-meet-first.md)). Nothing here is built until the ADR 0024/0025 milestone, with its real-node tests, is done.

Revised 2026-11-09, after the maintainer's review:

- **Fetching uses a viewing endpoint.** The hosting key is used only to offer, be checked and serve. It isn't shown to sources while the copy is fetched.
- **One file, several commitments.** A file is stored once, held by each reason to keep it, and each endpoint serves only from its own list.
- **Bounds at the author's node** on offers waiting, checking work and records kept, not only on places in a reply.
- **No fixed cutoff while the author is offline,** for now. A distributor follows ADR 0025's rules for hosting and deletes. Long creator outages are tested before a cutoff is set.
- **Limits and disclosure kept,** and stated where they bind.
- **Losing, replacing or a thief copying the hosting key** each has a recovery, and old advertisements expire.

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
- **Used only to offer, be checked and serve.** It never fetches, and never asks anything of anyone but the authors it offers copies to.
- **Persistent until replaced.** One per node. Replacing it is below ("If the hosting key is lost, replaced or copied").

### Help Distribute

- **A deliberate act, on an eligible public post,** whether or not the owner watched it. "Help distribute" is a button of its own, never a side effect of watching, caching, following or sharing.
- **Eligible:** the post is public, not for trusted followers ([ADR 0013](0013-trusted-followers.md)); its author's newest profile is `discoverable`; the node holds no tombstone for it ([ADR 0019](0019-taking-posts-and-media-down.md)); and its media fits [ADR 0005](0005-media-profile-v1.md)'s profile.
- **Said at the moment of choosing,** before anything is fetched or offered: your public profile doesn't show this, and nothing you publish ties it to you; the author's node, and anyone who asks it, can see that an unnamed key of yours hosts this post, and which other posts that key hosts; the relay sees your node's IP address, and can link this key to your node's other keys that connect from it, and through those to your public ID; it isn't anonymous.
- **A complete, verified copy first, fetched without the hosting key.** The node fetches every file through a viewing endpoint, from public sources only, exactly as watching does ([ADR 0024](0024-watching-found-posts-without-your-identity.md)), and checks every chunk. A copy already on the node, from watching or sharing, is used as it is: its bytes were checked the same way. Sources see a viewing endpoint, which ends within the hour, never the hosting key. Nothing is offered until the copy is complete.
- **Offering.** Then, and only then, the hosting key offers the copy to the author's media endpoint, through the relay. The author's node checks a 1 KiB piece from the hosting key, as for a resharer ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)), and notes the key as a distributor of that post, within the bounds below.
  - **Who it names them to:** anyone who asks, over Tor or through its media endpoint. It names them by key alone, as nothing else about them is known.
  - **How many:** at most 2 of the 6 hosts it names in a reply (a starting value). The one place for a recently verified host (ADR 0025) can go to one of them, within those 2.
  - **Offered again daily,** by the hosting key, with nothing else in it. That keeps the author's record fresh, and lets old records expire (below).
- **Directories later.** Distributors aren't listed in directories in this record's first step. So while the author's node is off, newcomers find resharers through directories (ADR 0025), not distributors. Listing them waits for measuring.

### One file, several commitments

Today a node stores each file under one class (own > replica > prefetch > cache), and decides from that class whom it serves the file to. Help Distribute can't fit that: a file Jimmy watched, then shared, then helped distribute is held for three reasons, and dropping one mustn't drop the others or change who else is served.

- **A file is stored once,** with a list of holds: each reason to keep it. One hold per commitment: own post; a hosting reshare, by its `Announce`; a prefetch for a followed author; a distribution commitment, by post; the device cache.
- **Bytes go only when the last hold goes.** Withdrawing from distributing a post drops that hold only. A file still held as a replica or in the cache stays, and nothing else about it changes.
- **Quotas count holds, not bytes on disk.** Each quota counts every file held under it, so each commitment's limit holds by itself, and withdrawing one never pushes another over its quota. Disk use is the files held, once each.
- **Each endpoint serves from its own list:**

  | Endpoint | Serves a file only while it holds |
  | --- | --- |
  | Media endpoint, or the public ID on a node without others | own post, hosting reshare, or prefetch with ADR 0012's yes |
  | Hosting key | a distribution commitment for that file |
  | Direct endpoint, to the node's devices | anything held, the cache included |

  The hosting key's list is explicit: the files of the posts it was offered for, nothing else. The media endpoint never serves a file held only for distribution, which would tie it to the public ID.
- **Refusals look alike.** Asked for a file not on its list, an endpoint answers as for a file it doesn't have, so no endpoint can be used to learn what the cache holds.

### Bounds at the author's node

Distributor keys cost nothing to make, so the 2 places in 6 don't stop an operator flooding Kim's node with keys. These bounds do, whatever the number of keys (starting values):

- **Offers waiting:** at most 8 checked at once, and at most 32 waiting, for all distributors together. Past that, an offer is answered "busy", and the key tries again the next day.
- **Checking work:** at most 60 new distributors checked an hour, for all posts together, and at most 4 an hour for any one post. A key already noted isn't checked again on its daily offer; the daily recheck covers it.
- **Records kept:** at most 16 distributors per post, and 1,000 in all. A new one takes a place only when one is free, or in place of one whose last 2 checks failed. Distributors that pass their checks are never pushed out by new keys.
- **The same budget as resharers?** No: resharers are counted apart ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)), so a flood of distributor keys can't crowd out people who shared under their names.

### Limits the owner keeps

- **Storage:** distribution commitments have their own quota, `distribute_quota_mb`, set when the owner first uses it (default 0: nothing until chosen). A file held for distribution isn't evicted while it's offered. With the quota full, a new Help Distribute is refused, with a message saying so, and an older one can be withdrawn.
- **Upload:** within `max_upload_mbps`, as everything is: the hosting key's transfers count in the same total as the media endpoint's. They come from the places for strangers, so they never take followers' or devices' places.
- **Withdrawal:** from one post or all, at once. The hosting key stops serving those files immediately, before anything is sent: the author's node is told afterwards, and stops naming it. If it can't be told, its next check fails and it stops naming the key within 2 days. Others keep what they've already seen.

### Deletes, and the author offline

Help Distribute follows [ADR 0025](0025-shared-posts-are-found-and-hosted.md)'s rules for a hosting resharer.

- **Checked without the hosting key.** Once a day, at a time of its own, the node asks the author's media endpoint from a viewing endpoint whether the post stands. It's answered with the hosts, or the post's signed `Delete` (ADR 0025's `osp/media-hosts/0`).
- **A `Delete` stops it at once,** however it arrives: in that answer, forwarded, from an outbox the node syncs, or as a tombstone from a directory. The files' distribution holds go, and a tombstone is kept.
- **The author offline:** for now, as for a resharer, the copy is served until a `Delete` reaches the node. The post is shown in `osp status` as unconfirmed once it hasn't been confirmed for 7 days.
- **Why no cutoff yet.** A draft stopped serving after 7 days unconfirmed. That bounds how long a deleted post is served, but it drops useful copies at exactly the time they help most: Kim's node off for a week. ADR 0025's resharers have no cutoff. So a cutoff is decided only after testing long creator outages (step 3): how many viewers' bytes distributors and resharers serve while Kim is off for 1, 7 and 30 days, and how long a deleted post goes on being served from a copy that missed the `Delete`. If a cutoff is kept, it applies to resharers and distributors alike.

### Service record

- **The downloader keeps the record.** Each downloader keeps its own record of a distributor key's outcomes ([ADR 0011](0011-measuring-peers.md)), as for any source, and skips one that keeps failing ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)). The author's node rechecks each distributor's copy once a day, and stops naming one after 2 failed checks in a row.
- **Nothing is reported to anyone.** There are no receipts.
- **Service only.** Standing gets a distributor chosen more, and nothing else. Never a direct connection or an IP address ([ADR 0018](0018-earned-direct-connections.md), the maintainer's decision of 2026-11-09). Its record is the hosting key's, and doesn't pass to the node's public ID, machine name or a replacing key.

### If the hosting key is lost, replaced or copied

- **Old advertisements expire by themselves.** An author's node forgets a distributor record that hasn't been offered again for 3 days, or whose last 2 checks failed. So a key nobody runs any more stops being named within 2 days and is forgotten within 3, wherever it was offered.
- **Replaced by the owner.** The node signs a retirement with the old key, for every author it offered to, and sends it before it deletes the old key. An author's node that gets one drops that key's records at once, and refuses offers from it for 30 days. Then the new key offers the commitments again, spread over a day, so that authors can't tell old and new apart by timing alone. That doesn't stop an author who hosted many of the same posts from guessing the link: the set of posts is the same.
- **Lost** (disk gone, or a restore). The key isn't in the backup ([ADR 0009](0009-identity-backup-and-recovery.md)), like the machine key. A restored node makes a new one, and its commitments start again from Help Distribute. The old key's records expire as above, as it no longer answers checks.
- **Copied by someone else.** A copy lets them serve as the key, and so be named, but only while they hold a real copy: every chunk is checked by the downloader against the author's hashes, so they can't serve anything else. What they gain is seeing which viewing endpoints download from them, which ADR 0024 makes worthless for knowing who. The owner retires the key as above, which a thief can't undo; offers from it are then refused for 30 days, and its records are gone.

## Consequences

**Help Distribute isn't anonymous**, and nothing may describe it so. What each party can observe:

| Who | Can see |
| --- | --- |
| **The author's node** | That a key it doesn't know holds a verified copy of its post, when it offered it, its relay, and how its checks went. If the owner watched before helping, the timing may hint that the viewer and the distributor were one node, though the viewer was a viewing endpoint. |
| **Anyone who asks** | The distributor keys named for a post. Asking many authors, they can gather which public posts one key hosts: a profile of one unnamed node's tastes, for as long as the key lasts. |
| **Sources while fetching** | A viewing endpoint, as for anyone watching. Never the hosting key. |
| **Downloaders** | The key and its relay, and how well it served them. |
| **Relay operators** (n0's, for now) | The key's connections and the node's IP address. They can link it with the node's other keys that connect from the same address: its media endpoint, its viewing endpoints. Through those, they can link it to the public ID, as they already see which public node a media endpoint serves. |
| **Directories** | Nothing, until distributors are listed. |
| **The node's own followers and peers** | Nothing: none of its identities names the hosting key. |

**What stops the link to the person:** nothing the node publishes ties the key to the public ID, so other nodes and viewers can't make the link from what they're given. What can: the relay's view of IP addresses, timing, a relay URL few others use, and anything the owner says. Replacing the key cuts the set it hosts from the next one, except for the guess above.

**Cost:** the upload and disk the owner gives, within the limits; a daily check per post from a viewing endpoint, and a daily offer per post from the hosting key; a 1 KiB check a day by each author.

**Abuse:**

- **False claims** are caught at a check, or at a download, and stop being named.
- **A crowd of keys** gets at most 2 of 6 places in a reply, 4 new checks an hour per post, 16 records per post and 1,000 in all at the author's node, without pushing out distributors that pass their checks. It's also held by the limits on strangers' requests at the author's media endpoint ([ADR 0024](0024-watching-found-posts-without-your-identity.md)).
- **Serving bad data** is caught chunk by chunk.

**Changes to other records:**

- **ADR 0016's reasons for keeping media off machines:** speed (a relay endpoint solves it), what hosting shows about its owner (here, about an unnamed key, as the table says), and counting work toward links (standing and addresses are now separate).
- **ADR 0012's swarm** doesn't change.
- **ADR 0025's Share** doesn't change. A node may both share and help distribute the same post; they're separate commitments on one stored copy.
- **The node's storage** moves from one class per file to holds (above). That changes how every file is kept and served, so it's built and tested on its own first (step 1), before anything of Help Distribute uses it.

## Building it

After the ADR 0024/0025 milestone, with its real-node tests, so the basic journey works first.

| Step | Work | Gate |
| --- | --- | --- |
| 1 | Holds in place of one class per file; each endpoint's serving list | Tests: a file held for two reasons stays when one goes; withdrawing one hold never makes another over quota; the media endpoint never serves a file held only for distribution, nor the hosting key one held only in the cache; a refused file looks like a missing one; every existing test of storage and serving passes unchanged |
| 2 | The hosting key, the distribution quota, Help Distribute and withdrawal, the offer, bounds and checks at the author, the daily offer and check, retirement and expiry | Tests: watching, caching, following and sharing never start it; nothing is offered before the copy is complete and checked; no request from the hosting key while fetching, and none from the public, media, direct or machine endpoint during fetching, offering or checking; a non-discoverable author's post or a trusted followers' post can't be chosen; the quota refuses; withdrawal stops serving at once, before the author is told; a `Delete` stops it; at most 2 of 6 names go to distributors; a flood of keys is held to the bounds and pushes out no distributor that passes its checks; a record not offered for 3 days is forgotten; a retired key's records go at once, and its offers are refused |
| 3 | Long creator outages, in `osp-month`: Kim off for 1, 7 and 30 days, with resharers and distributors; and real nodes: Jimmy helps distribute Kim's video without watching it; Alice, a stranger, gets part of it from his hosting key | Recorded: viewers' bytes served while Kim is off, and how long a deleted post goes on being served from a copy that missed the `Delete`, with no cutoff and with 7 and 30 days. The maintainer then decides on a cutoff, for distributors and resharers alike. Real nodes: Kim's and Alice's nodes record no public ID, media endpoint or machine name of Jimmy's, and no source sees his hosting key while he fetches |
| 4 | Measured in `osp-sim` beside ADR 0025's runs | B8's marks met; recorded: distributors' share of strangers' bytes, and Kim's copies |

**Revisit** if distributor keys come to dominate who's named, if few people use it, or if a relay of our own changes what relays see.
