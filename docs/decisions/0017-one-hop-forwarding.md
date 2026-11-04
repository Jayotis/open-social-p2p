# ADR 0017: One-hop forwarding over earned links

**Status:** Proposed (2026-11-04). Not built and not scheduled. Built only if stage A below says it's worth it. Builds on [ADR 0018](0018-earned-direct-connections.md). Until then, [ADR 0014](0014-finding-people-and-private-messages.md)'s media rules stand.

## Context

OSP keeps a home address hidden from strangers. It doesn't promise anonymity, and public posts are public. People who want more can use an alias, a VPN, or a node away from home.

**Copies save the creator's upload, not the relay's.** In the [B8 test](../testing/b8-creator-upload.md), a creator with 300 followers sent 20 copies instead of 300. But a follower hiding its address still sends through a relay, so somebody pays for each byte, and public relays limit rates. ADR 0014 left forwarding "once there are enough users to run a forwarding network of our own".

[ADR 0018](0018-earned-direct-connections.md) moves bytes off relays between pairs that have earned a direct link. What's left is pairs that haven't. Some of those will share a peer that has earned a link with both, and it could pass traffic between them, with neither seeing the other's address.

That costs upload. One forwarder means two home uploads for each byte: a 52 MB video to 300 viewers goes from 15.7 GB to 31.3 GB. It's worth doing only if it frees enough relay traffic.

Questions to settle:

1. Is it worth building at all?
2. How many hops?
3. Who forwards, and do they agree to it?
4. What does a forwarder see and carry?

## Options considered

- **Video over Tor.** [ADR 0014](0014-finding-people-and-private-messages.md) measured 1 MB taking between 3.4 and 150 s. Rejected.
- **Onion circuits of several hops.** Rejected for now, as the goal doesn't need them. Hiding an address from the other end takes one hop. More hops only stop a forwarder seeing both ends, which a public network doesn't need, and a video's size and timing largely give it away anyway. They'd also cost another upload per hop, need a way of finding paths over private links, and need a reviewed routing protocol. If that goal ever arises, it gets its own record.
- **Every node forwarding by default.** Rejected. A home address would be seen sending video its owner never chose. [ADR 0012](0012-sharing-prefetched-copies.md) asks at setup for less.
- **The forwarder fetching the file and serving it.** Simple, and it could check the hash. But the node would then hold a file it never chose, which is what [the review](../reviews/2026-10-21-review-of-docs-12-and-13.md) dropped blind hosting over. Rejected.
- **Relays only.** Simple, but one party pays for everything. Relays stay as the fallback, and the answer for strangers. Community relays are a decision of their own.
- **One hop, between nodes that have each earned a link with the forwarder.** Chosen for testing.

## Decision

### What does what

| Function | Mechanism | What it gives |
| --- | --- | --- |
| Hosting | Whole verified copies, follower and resharer swarms | Sources; the creator is spared uploads |
| Downloading in parts | iroh-blobs ranges from up to 3 sources | Speed. Parts hide no address |
| Earned link | ADR 0018's direct connection | Cheap bytes between two nodes that see each other's address |
| Forwarding | One earned peer passing encrypted traffic between two others | Bytes off relays, with neither end seeing the other's address |
| Relays | As today | Everyone else |

A forwarder need not hold the video. A host need not forward.

### One hop

A viewer V and a host H each hold an earned link with a forwarder F. V reaches H through F.

- **F sees** both nodes' endpoints and addresses, which it already knows, along with the timing and the number of bytes.
- **F doesn't see** the contents: the connection is encrypted from V's media endpoint to H's. It can't check what it carries.
- **V and H see** each other's media endpoint, never each other's address.
- F needs its own earned link with each. Permission doesn't pass along, and F never tells either end the other's address.

### Finding a forwarder

Earned peers swap lists of whom they've earned a link with, capped by ADR 0018. V compares H against those lists at home, so nothing about what V wants leaves the node until it asks F to carry something. A list is kept only for this and isn't passed on. So the earned graph is visible to immediate earned peers, a tradeoff taken for the sake of having a path at all.

### Asked at setup

**Forwarding is off without a yes,** as in ADR 0012. It's offered only to a node that said yes to `earned`, with yes recommended. Setup says:

> Your node can pass encrypted video between nodes it has a direct link with. This uses some of your upload. You can't see what's inside. Those nodes see your internet address sending it, and you answer for what your connection sends, as with anything else you do online.

- **Limits:** a share of `max_upload_mbps`, behind everything else the node does, and paused by the owner at any time or on a metered connection.
- **A refusal is normal.** A forwarder at its limit says busy, and V uses a relay.
- Limits are set from the simulator.

### What it carries

- **OSP media between earned peers only.** No general proxy, no internet exit, no address chosen by the asker, nothing on the local network.
- **It earns nothing.** Forwarded bytes don't count as [work](0018-earned-direct-connections.md#what-counts-as-work), and F has already earned its place. So there's no reason to make fake traffic.
- **It never fails open.** If anything is refused or fails, the transfer uses a relay or reports an error, and never dials a stranger directly.
- **After a [delete](0019-taking-posts-and-media-down.md),** hosts running the standard software stop serving, so a forwarder has nothing to carry. It can't refuse a file itself, as it can't see one.

### What it doesn't solve

- **Newcomers and casual viewers** have no earned peers, so they stay on relays.
- **Some pairs can't connect** directly even when both allow it, because of their routers. Those stay on relays.
- **Viewers with earned peers** may often find a host among them, which needs no forwarding. That is why stage A comes first.

## Consequences

**Privacy.** It hides each end's address from the other. F sees who talks to whom and how much, as a relay does, and the address of each.

**Responsibility.** Anyone at the far end of a forwarder's link sees its address sending, and it can't tell what it carries. Each owner answers for what their node sends, and no document may claim otherwise ([doc 13](../13-hosting-roles-and-follower-swarm.md)).

**Cost.** Each forwarded byte is uploaded twice. Relay bills shrink only if this carries a real share of traffic.

**Operational.** One more protocol, likely on iroh's custom transports like [`osp-tor`](../../crates/osp-tor/), which sit outside iroh's semver guarantees.

**Changes to accepted records.** None. ADR 0014 left this open.

**Licensing.** No new dependency chosen.

## Building it

Each mark is set before its run.

| Stage | Work | Gate |
| --- | --- | --- |
| A | Measuring only: in `osp-sim` with relays and ADR 0018, count relayed bytes between pairs sharing an earned peer | **Build further only if at least a quarter of relayed bytes qualify** (a starting value) |
| B | Lists of earned peers; forwarding with limits; asking at setup | V and H fetch through F, and neither learns the other's address, in a packet capture. Limits, pause and busy all work |
| C | Hostile tests: loops, floods, a forwarder that drops traffic, false lists, a node asking for an address it has no link with | No open proxy, no amplification, no stranger gets an address; failures fall back to relays |
| D | Full run: B8 with several posts, compared with relays alone and with ADR 0018 alone | Marks for startup, seeking, creator upload, relay bytes and each node's data use |

`osp-sim` has no relays and no routers yet: the first is ADR 0014's "measuring media's default", and the second needs real networks as well.

## Validation

A packet capture and tests that no public reply gives anyone an address; a forwarder at its limit doesn't harm its owner's connection; nothing reaches a loopback or local address; forged requests are refused. Every test counts each hop's bytes, and keeps creator upload apart from relay bytes.

**Revisit** if stage A misses its mark, if forwarding proves harmful on ordinary home connections, if people turn it down at setup, or if a goal arises that one hop can't meet.
