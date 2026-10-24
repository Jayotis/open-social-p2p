# Peer Reciprocity and Recognition

**Status: Proposal, second draft. Only phase 1, measuring, is built ([ADR 0011](decisions/0011-measuring-peers.md)).** The first draft ("Contribution accounting and network priority") was taken apart in a [critical review](reviews/2026-10-21-review-of-docs-12-and-13.md); this draft responds to it. The rest becomes a decision record once its blocking findings are resolved.

## Summary

The network depends on three kinds of contribution: **hosting** (giving upload bandwidth), **creating** (making posts) and **scouting** (finding good posts and resharing them). This draft keeps those three separate instead of turning them into one score:

| Contribution | What it earns |
| --- | --- |
| **Creating** | Attribution on every copy and reshare, and a **creator level** others can check |
| **Scouting** | "Found through…" recognition, and a **scout level** others can check |
| **Hosting** | Better treatment from the specific nodes you have helped (local reciprocity), plus a **self-reported host stat** that grants nothing |

There is no global credit, no public delivery receipt, and no central authority deciding anyone's standing. Each node learns about the peers it deals with, the way people learn whom they can rely on.

## Principles

1. **No global score.** Every node forms its own view of its peers from its own experience. Two nodes can rightly hold different views of the same peer.
2. **Measure, don't trust claims.** A node credits only what it observed itself, such as verified bytes received or requests answered.
3. **Generous, not punitive.** Strangers and newcomers always get a share of a busy node's capacity. Failures fade with time; nothing is punished permanently.
4. **Network trust is not content trust.** Preferring Bob's node for a download must never mean preferring Bob's posts in a feed. Peer assessment and feed ranking ([doc 08](08-open-feed-algorithm.md)) stay separate algorithms.
5. **Nothing here changes feed ranking,** including the levels.

## Part 1: local peer assessment

Every node keeps a private record about each peer node it has dealt with:

| Measure | What it captures |
| --- | --- |
| **Reliability** | How often the peer completes requests instead of timing out or failing |
| **Speed** | Throughput and time to first byte, as measured from this node's location |
| **Integrity** | Whether data from the peer passes hash verification. A single failure is serious: data is retried elsewhere and the peer marked down |
| **Availability** | How often the peer is reachable when tried |
| **Reciprocity** | Verified bytes the peer has sent this node, against bytes this node has sent it |
| **Recency** | All measures decay (half-life of 14 days as a starting point) so old behaviour fades |

The record is private. It is never published, never gossiped, and never presented as proof to anyone else.

Peers are grouped by identity, using the node list in each signed profile ([ADR 0004](decisions/0004-signed-objects-and-ids.md)), so a person with two nodes is treated as one peer.

### What the assessment is used for

1. **Choosing sources.** When downloading, prefer peers that are reliable, fast and honest from here, while keeping some requests for other peers so measurements stay fresh ([doc 03](03-peer-distribution-and-scoring.md)).
2. **Choosing whom to stay connected to.** Nodes keep connections open to peers that are reliable and reciprocate. Over time, well-behaved, well-connected nodes end up linked to each other. That is how "the best nodes stay in contact" without anyone appointing them.
3. **Deciding whom to serve first when busy** (generous tit-for-tat, below).

## Part 2: generous tit-for-tat when busy

- **When a node isn't busy,** everyone gets full speed. Reciprocity doesn't matter.
- **When it is busy** (its upload limit is reached, or more transfers want to run than it can serve fully):
  - A **stranger share** of its upload (30% as a starting point) is divided equally among requesters it has little or no history with, rotating so each gets a turn. This is BitTorrent's "optimistic unchoke".
  - The rest is divided among known peers according to reciprocity, on a logarithmic scale, so the thousandth gigabyte counts far less than the first.
  - The node owner can optionally give extra weight to people they follow.
- **Nobody is refused for low reciprocity.** When a node is over capacity it answers "busy" (iroh-blobs' `RateLimited`) and points to other hosts (`WhoHosts`), and the requester moves on.

**Why a fresh identity doesn't help:** a new identity has no history, so it lands in the stranger share, which is the lowest treatment any peer gets. Registering again never improves service. History can only help.

**Honest limits:**

- Priority under contention is zero-sum. When one peer gets more of a busy node's upload, others get less.
- Strategic clients have gamed tit-for-tat systems before; see the BitTyrant work cited in the review.
- Because the ledger is private and only affects the node that keeps it, gaming one node's ledger gains little beyond that node. Test this before relying on it.

## Part 3: recognition

### Creator level

Built from public signed reshares (`Announce` objects, [ADR 0003](decisions/0003-native-social-protocol-v0.md)): how many distinct identities reshared a person's posts over the last 90 days, counted 1.5× when the resharer also hosts.

### Scout level

Built from the same public data: how often a person's reshares were followed by other people's reshares of the same post within 7 days.

- It measures **hit rate** (the share of a person's reshares that later took off), not volume, so resharing everything doesn't pay.
- "Earlier" means **first seen by the viewer's node**, not the timestamp the resharer claims, so reshares can't be backdated. Early always means early as seen from here.

### Resisting fake accounts

Levels are computed by **the viewer's node**, counting only reshares by identities within two follow hops of the viewer. Personalised PageRank (trust spreading out from the viewer, as in SybilRank) is a later option. Reshare rings remain possible; the review accepts this as a risk because levels grant recognition only, never bandwidth or reach.

### Self-reported host stat

From its own ledger, a node can show "served 40 GB to 230 nodes in the last 30 days". It is clearly labelled **self-reported**. It can't be verified, and it buys nothing: no priority, no reach, no level. Faking it gains only bragging rights, so there is little reason to. Showing it is optional, because a large figure also announces "this node redistributes a lot of content" (review finding A9).

### The profile card

- Shows the creator level and scout level (0 to 5, last 90 days) and, optionally, the self-reported host stat.
- Levels are always computed by the **viewer's** node and labelled "as seen from your network". Nobody computes their own levels for others.
- **In person:** the card's QR code opens the profile; the scanning node computes the levels itself.
- There is no leaderboard.

## What was removed from the first draft, and why

| Removed | Reason (review finding) |
| --- | --- |
| Public delivery receipts | They reveal whose content each person consumes (A1) |
| Global host credit and "vouched" credit | Fake accounts and rented servers could manufacture or buy it (A4, A5) |
| Creator and scout standing buying network priority | Priority is zero-sum and would reward virality with bandwidth (A10, second review) |
| Newcomer grace period | Unnecessary once fresh identities get the stranger share, and it invited repeated re-registration (second review) |

## Data and protocol changes

- **Database:** a `peers` table with the measures above, per peer node, plus decay timestamps.
- **No new network messages** are needed for Parts 1 and 2. Part 3 uses reshares nodes already sync.
- **Client API:** a peer table in `osp status`; `Profile { actor }` returns the card fields as the viewer's node computes them.
- **Config:** `upload_limit_kbps`, `reciprocity.stranger_share`, `reciprocity.favour_follows`, `profile.show_host_stat`.

## Phases

1. **Measure:** record the peer assessment and show it in `osp status`. No behaviour changes yet.
2. **Use it for source selection** and connection retention (milestone M3).
3. **Generous tit-for-tat under contention,** once real measurements show busy nodes.
4. **Creator and scout levels** and the profile card.

## Relationship to earlier documents

- [Doc 11](11-collective-discovery-history-and-referrals.md) rules out "automatic credit for transfer bandwidth". Local reciprocity is an operational decision each node makes about its own upload, not credit others can see, and the host stat is self-reported and earns nothing. This draft is consistent with doc 11.
- [Doc 10](10-exploration-divergence-and-scouts.md) recommends against universal public scores. Levels are per viewer and there is no leaderboard, but levels are still scores. Review finding A8 records this as an accepted risk.
- [ADR 0006](decisions/0006-desktop-first-always-on-node.md) means every participant runs a node, so everyone can reciprocate.

## Validation

Before Phase 3, simulate:

- A realistic mix of users (most mostly watch).
- A fake-account farm.
- Slow-connection hosts alongside one very fast host.
- A viral spike bringing in many strangers at once.

Measure: strangers' time to first frame during the spike; how much a farm can shift a single honest node's treatment; and whether slow-connection participants still get acceptable service.
