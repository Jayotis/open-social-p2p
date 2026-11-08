# ADR 0020: Sharing media you open, with peers that worked for you

**Status:** Proposed (2026-11-08). Not built, and not before the invited alpha: it needs [ADR 0018](0018-earned-direct-connections.md)'s layers (its step 2). Until then, media a node opens from outside its follows stays in a private cache, as [ADR 0012](0012-sharing-prefetched-copies.md) says.

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

- **Kept briefly, followed copies longer.** A complete copy of a public post the owner opened, each chunk checked, is served for 48 hours after it was fetched (a starting value, set in the simulator), then kept as private cache until evicted. It stays `cache`: evicted first, within `cache_quota_mb`, so followed copies stay longer. A part fetched while streaming isn't shared.
- **Only public posts.** Posts for trusted followers ([ADR 0013](0013-trusted-followers.md)) are never shared this way, whoever may see them.
- **Told only to peers at layer 2.** A node tells the peers at [layer 2](0018-earned-direct-connections.md#layers) or above in its own record (work on 3 different days, no bad data) which public posts it holds copies of: what it opened, if it shares that, and what it shares already. It's a short list, at most 20 posts, sent at most once a day, at a random time, so it doesn't show when each post was opened. Layer 2 is where a node that carries already shows a peer its machine (ADR 0018), and so who it is.
- **Served only to those it told.** Anyone else asking for that copy gets the same answer as for a file the node doesn't have. The author doesn't name these copies, and isn't told of them.
- **The list is a lead, nothing more.** The peer checks each post's signature with its author, as for any post, and decides itself whether to fetch it. Holding a copy isn't liking or recommending it.
- **Credit for passing it on.** A download from the copy counts as work at the peer's end, as any download does ([ADR 0018](0018-earned-direct-connections.md#what-counts-as-work)). As the node served that peer, the two move toward "both ways".
- **Deleted posts.** A copy of a post by someone the node doesn't follow gets no `Delete` through its follows, so it's checked as a reshare without a follow is ([ADR 0019](0019-taking-posts-and-media-down.md)): its author's node is asked before the copy is first served, and once a day after. A tombstone drops it.

### Asked at setup

[ADR 0012](0012-sharing-prefetched-copies.md)'s question grows to cover both, so it stays one question, with yes recommended. Each answer is written as its own setting, `share_prefetched` and a new `share_opened` under `[swarm]`, so either can be turned off later. Setup no longer says sharing never shows what you watched. It says, in plain words, that nodes which have worked for yours on several days may learn which public posts you opened in the last two days, and fetch them from you. A node never asked doesn't share what it opens, and upgrading never turns it on. Turning it off stops the lists and the serving at once; what a peer already learned stays with it.

## Consequences

**Privacy.** Strangers learn nothing new. A peer at layer 2 learns which public posts the owner opened in the last two days, up to 20 at a time, and that it's their node. Two cases:

- **A peer aimed at one person** has to serve or carry something that person's node wanted, on 3 different days, as a peer can't push work at a node (ADR 0018). Days of real work, to learn a short list of public posts someone opened.
- **An operator collecting at scale** runs machines that work honestly for many nodes, gets to layer 2 with many and keeps their lists. Its work is real and helps the network meanwhile. The cap and the daily timing limit what it gets from each node, and nothing stops it entirely. It's worth doing only once the network is large.

**Changes to other records.** ADR 0012's "not what you watched" no longer holds for nodes that say yes. [Doc 10](../10-exploration-divergence-and-scouts.md)'s "never publicize who merely viewed a file" holds toward strangers, not toward peers at layer 2. [ADR 0015](0015-nodes-meet-first.md)'s machine layer is unchanged: the lists go between nodes' public sides, never from a machine name.

**Cost.** Upload to the peers that ask, within `max_upload_mbps`, and no extra disk: copies come from the cache a node keeps anyway.

**Not here.** Passing more machine records to machines known for longer is a question for ADR 0016's machine layer.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| A | In `osp-sim`, with ADR 0018's layers on a shortened clock: viewers from outside the follows sharing for 48 hours, beside today's swarm | It cuts the author's upload to those viewers, and the follower swarm still meets [B8](../testing/b8-creator-upload.md)'s marks. Sets the 48 hours and the list's size |
| B | `share_opened`, the lists, serving only to peers told, the delete check, the question at setup | Tests: a stranger, and a peer below layer 2, get the same answer as for a missing file; a trusted followers' post is never listed or served; a deleted post's copy is dropped within a day; turning it off stops the lists; upgrading doesn't turn it on |

**Revisit** if stage A shows little saved upload, if peers at layer 2 are too easy to reach in the simulator, or if people turn the question down once it says what it shows.
