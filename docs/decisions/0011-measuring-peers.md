# ADR 0011: Measuring peers

**Status:** Accepted (2026-10-23). Built (2026-10-23): media downloads in our own loop, each attempt ending in one of the outcomes below; the `peers` table (database migration 6), filled in by requests, downloads and media served; `osp peers` and the `Peers` client request. Served media is counted from iroh-blobs' transfer events, which the serving rules now ask for.

## Context

[Doc 12](../12-peer-reciprocity-and-recognition.md) proposes that each node keep a private assessment of the peers it deals with, and use it later to choose download sources, to decide which connections to keep, and to decide whom to serve first when busy. Its first phase only measures: record the assessment and show it to the owner, with no change in behaviour. The [critical review](../reviews/2026-10-21-review-of-docs-12-and-13.md) blocks later phases until they have evidence (A3 for serving priority, B8 for the follower swarm), but nothing blocks measuring. Both of those measurements need it.

What a node does with other nodes today:

- **Social requests** ([ADR 0003](0003-native-social-protocol-v0.md)): it asks followed nodes for profiles, outbox pages and objects, subscribes, asks who hosts a blob, and tells followers about new posts and moves.
- **Media downloads**: it prefetches followed people's media, fetches media it hosts after a reshare or a restore, and fetches media a phone opens. These go through iroh-blobs' `Downloader`, given a list of nodes thought to have the blob.
- **Media served**: other nodes download its own media, and media it hosts, within the serving limits. A request over the limits is refused as busy.

The `Downloader` tries the listed nodes one by one, and reports which one it is trying and when one fails, but not why. A node that is busy, one that doesn't host the blob, one that can't be reached and one that sent bytes not matching the hash all look the same. Measuring needs those told apart. So does the follower swarm ([doc 13](../13-hosting-roles-and-follower-swarm.md)), where a busy refusal should send the requester to another host, and so does retrying a prefetch refused as busy.

Questions to settle:

1. What is measured, and from which interactions?
2. How is the record kept small, private, and fading over time?
3. How does the node learn why a download from one node failed?
4. How does the owner see it?

## Options considered

**Download outcomes.** (a) Keep the `Downloader` and read its events: the node and the bytes are known, the reason for a failure isn't. (b) Our own loop on iroh-blobs' public request API, which returns typed errors: "busy" and "not allowed" arrive as the provider's error codes, and data not matching the hash as a hash-mismatch error. (c) Change iroh-blobs itself, and carry a patch.

(b) is about as long as the `Downloader`'s own loop for one blob, and keeps what it does now: asking the next node only for the parts still missing, through pooled connections (iroh-blobs' connection pool is public). It stays inside `store.rs`, the one file that uses iroh-blobs ([ADR 0002](0002-rust-and-iroh.md)).

**Keeping the record.** (a) In memory only: lost on every restart, and home nodes restart often (updates, reboots). (b) A database table of counters that fade over time. (c) A log of every event: it grows without end, and keeps exactly when each node asked for something.

**Showing it.** (a) Inside `osp status`, which the phone also shows: the table can be long. (b) A separate `osp peers` command, and a matching client request, so the phone can show it later.

## Decision

Download outcomes (b), a table of fading counters (b), and `osp peers` (b).

**Per peer node, the node records:**

| Measure | Counted from |
| --- | --- |
| **Reachability** | Connection attempts this node makes to the peer, and how many fail or time out |
| **Answers** | Social requests sent to the peer, how many got a reply in time, and how long replies took. An error reply counts as an answer: the node is up and responding |
| **Downloads from it** | Attempts to download media from it, by outcome: completed, busy, doesn't host it, bad data, failed (anything else, such as a dropped connection or a timeout), and handed on (it hadn't sent anything when the rest of its part of a file went to a faster node, most often because it was busy and the request was waiting for a place; neither a failure nor a refusal). For completed and partly completed ones, the bytes received, the time taken, and the time to the first byte |
| **Media served to it** | Bytes sent to it, transfers, and requests from it refused as busy |
| **When** | First and last contact |

These are the measures of doc 12 part 1: availability is reachability, reliability is answers and download outcomes, speed is bytes over time and time to first byte, integrity is "bad data", and reciprocity is bytes received against bytes sent.

- **Peer nodes, not identities.** Rows are per node ID. `osp peers` shows the name of the identity a node belongs to when a profile this node keeps lists it (people it follows, and authors of posts it keeps), and otherwise what the node is to this one: a follower, a host of some media, or neither. Grouping a person's nodes into one peer waits for phase 2, when the record is first used.
- **Paired phones aren't peers.** They're the owner's own devices, and are left out.
- **Counts fade by half every 14 days** (doc 12's starting point). Each counter is stored with the time it was last updated, and faded whenever it is read or added to. Averages, such as reply time, are a faded sum divided by a faded count, so old behaviour fades out of them too.
- **Bad data is serious.** A download that fails with a hash mismatch is recorded, logged as a warning naming the node, and continues from the next node. Nothing else changes in this phase.

**Bounded.** At most 1,000 rows for nodes this node contacted itself, and 1,000 for nodes that only contacted it, such as strangers downloading media. When one group is full, its row with the oldest last contact goes. Anyone can make new node IDs for free, so keeping the groups apart means a crowd of strangers can't push out the record of the nodes this node follows and hosts with. Rows with no contact for 90 days are deleted.

**Private.** The record never leaves the node: no network message carries it, it isn't in the backup file, and it is shown only to the owner, through `osp peers` and paired phones. It holds faded totals per node, not which blobs a node asked for or when.

**Showing it.** `osp peers` lists the peers, most recent contact first. For example:

```
B (all-in-one)  ec66132b14  you follow them
  last contact 2 min ago, first 3 days ago
  reached 100% of 12 tries; answered 100% of 31 requests, in 45 ms on average
  downloads from it: 4 completed, 0 busy, 0 bad data, 0 failed; 52.3 MB at 9.8 MB/s, first byte after 120 ms
  served to it: 3 transfers, 52.3 MB; 0 refused as busy
```

The numbers are the faded counts, rounded; the output says so in a line at the top.

**The download loop.** `store.rs` replaces the `Downloader` with its own loop for one blob. It keeps every rule downloads follow now: chunks verified against the hash before they are stored, a stop once more than the post's declared size arrives, room reserved within the quotas first, and 10 minutes at most for the whole download (since 2026-10-25, 30 seconds at most without bytes from a node, and 30 minutes for the whole download: [ADR 0003](0003-native-social-protocol-v0.md)). Each node is asked only for the parts still missing. The order nodes are tried in doesn't change: the most recently seen hosts first, then any found by asking who hosts it. Each attempt ends in one of the outcomes above, which the loop records.

No protocol change. One database migration adds the table.

## Consequences

- **The measurements doc 12 and doc 13 depend on exist**, and the owner can see how each peer behaves. On a creator's node, "served to it" summed over all peers is the creator's total upload, which is what the follower swarm's B8 test measures.
- **A node knows when it was refused as busy**, so later changes can try another host or try again later.
- **More code is ours**: the download loop for one blob, which iroh-blobs used to provide. It still uses only iroh-blobs' public API, and stays in `store.rs`, where an iroh-blobs update has to be checked anyway.
- **Someone who copies the node's disk learns which nodes downloaded media from it recently**, as faded totals, not what they downloaded. The database already lists followers and hosts.
- **A small write per request or transfer**: one database update each. Incoming requests are already limited (at most 32 social requests and 8 transfers at once), and a node sends few of its own, so this stays small.

## Validation and revisit conditions

- Unit tests: counters fade by half in 14 days; averages fade with them; a full group drops its oldest row, and only from that group; rows with no contact for 90 days go; paired phones aren't recorded.
- Integration tests, with real nodes: a completed download records the bytes and the time against the node that sent them; a node refusing as busy is recorded as busy, and the download continues from the next node; a node that doesn't host the blob is recorded as such; an unreachable node counts as a failed connection attempt; media served to a peer is recorded on the serving side; `osp peers` shows them.
- A node that sends bytes not matching the hash is recorded as bad data, if a test can stage one. iroh-blobs' own provider only sends verified data, so this needs a fake provider in the test.
- On test nodes: `osp peers` on A, B and C after a few days shows sensible numbers for each other.
- Revisit if the writes show up as load on a busy node (then keep counters in memory and save them every minute), if phase 2 needs a measure not listed here, or when grouping by identity is needed.
