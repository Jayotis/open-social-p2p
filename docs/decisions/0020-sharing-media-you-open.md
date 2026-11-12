# ADR 0020: Sharing media you open, with peers that worked for you

**Status:** Superseded by [ADR 0025](0025-shared-posts-are-found-and-hosted.md) (2026-11-09) before it was built: viewing alone never starts sharing, even with a general yes; a deliberate Share controls public hosting of discovered posts. Was accepted (2026-11-08), with the maintainer's review folded in below. Not built, and not before the invited alpha: it needs [ADR 0018](0018-earned-direct-connections.md)'s layers (its step 2). Until then, media a node opens from outside its follows stays in a private cache, as [ADR 0012](0012-sharing-prefetched-copies.md) says.

Revised 2026-11-08, after the maintainer's review, before it was accepted. Sharing what you open is now its own question at setup, off unless chosen, instead of a part of ADR 0012's. A copy is served only on its author's own word, which settles what happens while the author is offline. The 48 hours are when a copy may be served, not how long it's kept. Confirming that a post stands takes a fresh answer from its author's own node, while a signed `Delete` from anywhere stops serving at once. The author's node learns which nodes hold a copy when they ask, which setup says. And stage A measures a sudden spike, where viewers may have few peers at layer 2 to fetch from. Each changed rule says what it replaced and why.

## Context

A node fetches ahead of time the posts of the people its owner follows, and with its owner's yes shares those copies, so a popular post doesn't swamp its author's connection ([ADR 0012](0012-sharing-prefetched-copies.md), [doc 13](../13-hosting-roles-and-follower-swarm.md)). Media the owner opens from anyone else is kept as `cache` and served only to the owner's devices. So:

- **A viewer from outside the follows helps nobody.** When a stranger's video is passed around, every viewer who doesn't follow its author fetches it from the author, or from the hosts the author names, which are its followers' copies, named only to its followers.
- **Nodes have little to tell the peers they work with.** [ADR 0018](0018-earned-direct-connections.md) builds standing from finished jobs, but a node can only be asked for what someone already knows it has.

What the code does today:

| Copy | Class | Kept | Served to |
| --- | --- | --- | --- |
| A post of its own | `own` | Never evicted | Anyone |
| A reshare | `replica` | Evicted after `prefetch` | Anyone |
| From someone it follows | `prefetch` | Evicted after `cache` | Its devices; anyone if `share_prefetched`, and the author names it to its followers |
| Opened from anyone else | `cache` | Evicted first | Its devices |

A host is named by its media endpoint, which is bound to the node's public ID ([ADR 0014](0014-finding-people-and-private-messages.md), "How the endpoints are tied to the person"). So whoever is told that a node has a copy can check whose node it is: it learns a person, not a machine.

## Options considered

1. **Keep today's rule.** What a node opens is never shared. Nothing is shown, and viewers from outside the follows cost the author's upload.
2. **Name copies of what you open to anyone, as file-sharing swarms do.** It spreads furthest. Anyone could ask an author's node who has each of its videos, and get the people who opened it in the last two days, by public ID, at no cost, as often as they like.
3. **Serve them under the machine name,** so a peer learns a machine, not a person. [ADR 0016](0016-machine-acquaintances.md) rejected machines serving media: what a machine hosts says what its owner follows, and here what its owner watched; the machine name is on Tor, too slow for video; and the work would have to be shown as the node's own to count toward a direct link anyway (ADR 0018).
4. **Share them for a short time, and tell only peers that have worked for the node on several days.** Strangers learn nothing. A peer that has served the node on 3 different days learns what its owner opened lately, and can fetch it from there, which counts as work at that peer's end.

## Decision

Option 4.

- **Served for 48 hours at most, and never kept for them.** A complete copy of a public post the owner opened, each chunk checked, may be served for 48 hours after it was fetched (a starting value, set in the simulator), and not after. It stays `cache`: evicted first, within `cache_quota_mb`, so it may be gone sooner, and followed copies stay longer. No disk is set aside to keep it for the 48 hours. A copy evicted before then is left off the next list, and a peer asking for it gets the same answer as for a missing file. A part fetched while streaming isn't shared. This replaces "is served for 48 hours", which a copy evicted first can't promise.
- **Only public posts.** Posts for trusted followers ([ADR 0013](0013-trusted-followers.md)) are never shared this way, whoever may see them.
- **Told only to peers at layer 2.** A node tells the peers at [layer 2](0018-earned-direct-connections.md#layers) or above in its own record (work on 3 different days, no bad data) which public posts it holds copies of: what it opened, if it shares that, and what it shares already. It's a short list, at most 20 posts, sent at most once a day, at a random time, so it doesn't show when each post was opened. Layer 2 is where a node that carries already shows a peer its machine (ADR 0018), and so who it is.
- **Served only to those it told.** Anyone else asking for that copy gets the same answer as for a file the node doesn't have. The author doesn't name these copies, and isn't told of them.
- **The list is a lead, nothing more.** The peer checks each post's signature with its author, as for any post, and decides itself whether to fetch it. Holding a copy isn't liking or recommending it.
- **Credit for passing it on.** A download from the copy counts as work at the peer's end, as any download does ([ADR 0018](0018-earned-direct-connections.md#what-counts-as-work)). As the node served that peer, the two move toward "both ways".
- **Only on its author's word.** A copy of a post by someone the node doesn't follow gets no `Delete` through its follows. So it may be served only once the author's node has confirmed the post stands. A confirmation is a reply to [`GetObjects`](0019-taking-posts-and-media-down.md) that this node sent itself, no earlier than when it began fetching the copy, to a node the author's current signed profile names, over a connection authenticated as that node, carrying the post rather than its `Delete`. A valid signature isn't one: it shows who wrote the post, not that it's still up, so a signed copy from a peer, or an old answer, confirms nothing, and nor does the author's node serving the file. While the copy may be served, the node asks again once a day, at a time of its own, as for a reshare without a follow.
- **A `Delete` stops it at once,** however it arrives: in that daily answer, forwarded through reshares, or from an outbox the node syncs. Unlike a confirmation, a `Delete` signed by the author is enough from anyone, as it can only take a post down. The copy is no longer listed or served, and a tombstone is kept.
- **While the author is offline,** its silence neither ends a copy's 48 hours nor extends them. A copy the author's node showed standing may be served until its 48 hours are up. One fetched from a peer while the author's node can't be reached isn't served at all until that node confirms it. So a stranger's video stays available to peers at layer 2 for up to two days after its author's node goes off, from copies made while it was on, and spreads no further until it's back. A post deleted just before its author's node went off can be served from those copies for the rest of their 48 hours, where the author's followers drop it within minutes, or at their next sync ([ADR 0019](0019-taking-posts-and-media-down.md)). This replaces "its author's node is asked before the copy is first served, and once a day after", which said nothing of a node that doesn't answer. Only the author's node can say a post stands, as `GetObjects` answers only for a node's own posts, so the choice is between waiting for it and trusting silence. This waits for a new copy, and lets one already shown standing run out its time. What keeps a post available for longer while its author is offline stays the followers' copies ([ADR 0012](0012-sharing-prefetched-copies.md)); this adds to them for viewers from outside the follows.

### Asked at setup

**Its own question, off unless chosen.** Setup asks it after [ADR 0012](0012-sharing-prefetched-copies.md)'s question about followed copies, which doesn't change. The two answers are independent settings under `[swarm]`, `share_prefetched` and a new `share_opened`. The new question says, in plain words, that nodes which have worked for yours on several days may learn which public posts you opened in the last two days, and fetch them from you: what you watched, where sharing followed copies shows whom you follow. It also says that the author of each post you share this way learns that your node holds a copy, as yours asks the author's node about it once a day for up to two days, even when you fetched the post from someone else. Its default answer is no, and setup recommends neither answer. A scripted setup turns it on only with `--share-opened`, and `--share` answers ADR 0012's question alone. A node never asked doesn't share what it opens, and upgrading never turns it on. Turning it off stops the lists and the serving at once; what a peer already learned stays with it.

This replaces growing ADR 0012's question to cover both, with yes recommended. One yes would then have shared what you watched along with whom you follow, and what you watched is the more private of the two, so it gets a yes of its own.

## Consequences

**Privacy.** Nothing changes for a node whose owner didn't turn `share_opened` on, which by default is every node. Where it's on, a peer at layer 2 learns which public posts the owner opened in the last two days, up to 20 at a time, and that it's their node, and each post's author learns that the node holds a copy. Other strangers learn nothing new. Three cases:

- **A peer aimed at one person** has to serve or carry something that person's node wanted, on 3 different days, as a peer can't push work at a node (ADR 0018). Days of real work, to learn a short list of public posts someone opened.
- **The author of a post** learns, from the confirmations, which nodes with `share_opened` on hold a copy of its post, by public ID, and when they ask, once a day for up to two days. A node that fetched the file from the author's node had shown it that much already. One that fetched it from a peer had shown the author nothing, and now shows it a viewer, which is why setup says so. A resharer without a follow shows the author the same today ([ADR 0019](0019-taking-posts-and-media-down.md)).
- **An operator collecting at scale** runs machines that work honestly for many nodes, gets to layer 2 with many and keeps their lists. Its work is real and helps the network meanwhile. The cap and the daily timing limit what it gets from each node, and nothing stops it entirely. It's worth doing only once the network is large.

**Changes to other records.** ADR 0012's question doesn't change, and its "not what you watched" holds unless the owner turns `share_opened` on. [Doc 10](../10-exploration-divergence-and-hunters.md)'s "never publicize who merely viewed a file" holds toward strangers, not toward peers at layer 2 or the post's author. [ADR 0015](0015-nodes-meet-first.md)'s machine layer is unchanged: the lists go between nodes' public sides, never from a machine name.

**Cost.** Upload to the peers that ask, within `max_upload_mbps`, and no extra disk: copies come from the cache a node keeps anyway, and nothing is kept longer for this. A request a day to the author's node of each post the node may serve this way, at most 20 posts.

**Not here.** Passing more machine records to machines known for longer is a question for ADR 0016's machine layer.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| A | In `osp-sim`, with ADR 0018's layers on a shortened clock, beside today's swarm, two cases. **Established:** viewers from outside the follows whose nodes have worked together for weeks. **A sudden spike:** a stranger's video passed around a network whose layers were built beforehand, opened mostly by nodes that don't follow its author, with the author's node off for part of the first day | Marks, set before the run: in both cases it cuts the author's upload to those viewers, and the follower swarm still meets [B8](../testing/b8-creator-upload.md)'s marks. Recorded for each hour of the spike's first 24: the viewers that had a peer at layer 2 holding a copy it could serve, the share of their bytes such copies served, and the author's upload against today's rules; and how many copies were evicted before their 48 hours. Sets the 48 hours and the list's size. If few viewers in a spike have a peer to fetch from, that's stated as a limit; layer 2 isn't lowered for it without a new decision, as a lower bar widens who learns what someone watched |
| B | `share_opened` and its own question at setup, the lists, serving only to peers told, the author's word and the daily check | Tests: a stranger, and a peer below layer 2, get the same answer as for a missing file; a trusted followers' post is never listed or served; with the author's node reachable, a deletion stops serving within 24 hours; with it unreachable, a confirmed copy can't be listed or served once 48 hours have passed since it was fetched; a valid `Delete` received any other way stops serving at once; a signed copy from a peer, an answer from a node the author's profile doesn't name, or one older than the fetch, confirms nothing; a copy fetched from a peer while the author's node can't be reached isn't served; an evicted copy is left off the next list; Enter at the question, and `--share`, leave it off; turning it off stops the lists; upgrading doesn't turn it on |

**Revisit** if stage A shows little saved upload, if in a spike's first day most viewers have no peer to fetch from, if most copies are evicted before their 48 hours, if peers at layer 2 are too easy to reach in the simulator, or if few people turn it on once it says what it shows.
