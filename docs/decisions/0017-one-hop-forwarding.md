# ADR 0017: Two hops for pairs without a direct link

**Status:** Proposed (2026-11-04). Not built. Stage A, measuring only, comes before the invited alpha; the rest is built only if stage A says it's worth it. Until then, [ADR 0014](0014-finding-people-and-private-messages.md)'s media rules stand: media between nodes without a direct connection goes through relays.

Revised 2026-11-07, still proposed. The first draft had one forwarder, between nodes that had each earned a direct link with it ([ADR 0018](0018-earned-direct-connections.md)). That left out the pairs that need it most: newcomers and casual viewers have no earned peers, and with media's default at `nobody`, that's nearly every pair. This draft uses two hops of volunteer nodes, each end picking the one next to it, and carrying now earns standing, credited by the node that received the bytes ([ADR 0016](0016-machine-acquaintances.md#the-local-record)). The first draft's one hop remains, as the case where both ends pick the same node.

## Context

OSP keeps a home address hidden from strangers. It doesn't promise anonymity, and public posts are public. People who want more can use an alias, a VPN, or a node away from home.

**Media's default stays `nobody`,** so media between nodes goes through relays: n0's public relays for now, which are meant for development and hobby use. [Measured](../testing/network-conditions.md#media-through-relays) with a relay at their limits, a 52 MB video to 300 followers, all behind the relay, put 19.3 GB through it, about one copy for each follower, and the swarm took over twice as long, missing two of B8's marks. Copies save the creator's upload ([B8](../testing/b8-creator-upload.md)), not the relay's.

[ADR 0018](0018-earned-direct-connections.md) moves bytes off relays between pairs that have earned a direct link, which takes a month or more of work. Until then, and for pairs that never earn one, somebody has to carry the bytes. Home nodes with upload to spare could, if each node's address is shown only to nodes it chose.

That costs upload. Two hops mean three home uploads for each byte: a 52 MB video to 300 viewers goes from 15.7 GB to about 47 GB. It's worth doing only if it frees enough relay traffic, and as pairs earn direct links, the hops carry less.

Questions to settle:

1. Is it worth building at all?
2. How many hops, and who picks them?
3. Who carries, and do they agree to it?
4. What does a hop see, and what does it get for carrying?

## Options considered

- **Video over Tor.** [ADR 0014](0014-finding-people-and-private-messages.md) measured 1 MB taking between 3.4 and 150 s. Rejected.
- **Relays only.** Simple, but one party pays for everything, and a relay sees both ends of everything it carries. Relays stay as the fallback, and for hole punches. Volunteer relays that anyone can run would make that view cheap to get. Community relays are a decision of their own.
- **One hop, between nodes that have each earned a link with it** (the first draft). The hop holds both ends' addresses with their consent, so a pair needs an earned peer in common. Newcomers and casual viewers have none. Kept as a case of the option chosen.
- **Every node carrying by default.** Rejected. A home address would be seen sending video its owner never chose. [ADR 0012](0012-sharing-prefetched-copies.md) asks at setup for less.
- **The hop fetching the file and serving it.** It could check the hash, but it would hold a file it never chose, which is what [the review](../reviews/2026-10-21-review-of-docs-12-and-13.md) dropped blind hosting over. Rejected.
- **Three hops, as Tor builds circuits,** with a middle hop that sees no address. Four uploads for each byte. On a public network where each end decides who sees its address, the middle hop protects nothing either end hadn't chosen to show. Rejected. If a goal arises that needs it, it gets its own record.
- **Two hops, each end picking the one next to it.** Chosen for testing.

## Decision

### What does what

| Function | Mechanism | What it gives |
| --- | --- | --- |
| Hosting | Whole verified copies, follower and resharer swarms | Sources; the creator is spared uploads |
| Downloading in parts | iroh-blobs ranges from up to 3 sources | Speed. Parts hide no address |
| Earned link | ADR 0018's direct connection | Cheap bytes between two nodes that see each other's address |
| Two hops | Volunteer nodes passing encrypted traffic, one picked by each end | Bytes off relays, each end's address seen only by the hop it picked |
| Relays | As today | Hole punches; the fallback |

A hop need not hold the video. A host need not carry.

### Two hops

A viewer V fetches from a host H, a follower or the creator, and the two have no direct link. V picks a hop next to it, P, and H has picked one next to it, Q. The bytes go H → Q → P → V.

| Who | Sees | Doesn't see |
| --- | --- | --- |
| V's hop, P | V's address; which host V is reaching; Q's address; the timing and the number of bytes | V's account, as V reaches P under a key used only for that; H's address; the contents |
| H's hop, Q | H's address and media endpoint, so whose node it is; P's address; the timing and the number of bytes | V's address or account; the contents |
| V and H | Each other's media endpoint | Each other's address |

- **The connection is encrypted from V's media endpoint to H's.** Neither hop can see or check what it carries.
- **H names its hop** when V asks for its endpoints over Tor, as it hands over its direct endpoint today. Its hop keeps a connection with it, as a node does with its relay.
- **No path is searched for,** and no lists are swapped, as the first draft's earned peers had to do: each end picks its own hop, and the host names its own. So there's no routing protocol to invent.
- **If V would pick Q itself,** the transfer takes that one hop.
- iroh's relays still coordinate hole punches between nodes and hops.

### Who picks

Each end picks the hop next to it, in this order:

1. **Its earned peers** ([ADR 0018](0018-earned-direct-connections.md)), which already see its address.
2. **Machines with standing in its own record** ([ADR 0016](0016-machine-acquaintances.md#the-local-record)): those that carried bytes it received and checked, or that showed a reference it counts.
3. **At the viewer's end only, 1 transfer in 10 goes to a machine with no standing yet,** so newcomers can start. A starting value.

The host's end never picks a machine without standing. Its hop learns whose node it is and where, the targeted flood in ADR 0018's table of harms, and that falls mostly on creators. A viewer's hop learns that an address is fetching from a host, with no account, which is less than a relay sees today.

### Credit

- **The node that received the bytes, and checked them against the hash, credits each hop they came through,** in its own record. It's first-hand, and nothing is shared.
- A hop can't make an honest node send traffic through it, as each end picks its own. Fakes crediting each other gain nothing with honest nodes.
- A transfer that fails credits nobody. A hop can't alter what it carries without breaking the encryption, so it can only drop or slow it, and one that does is picked less.
- What standing gets a machine is service, never access to content ([ADR 0016](0016-machine-acquaintances.md#service-not-content)).

This replaces the first draft's "it earns nothing", whose reason was that a forwarder had already earned its place, so nothing would tempt anyone to make fake traffic. Hops now include newcomers, who need a way to start, and the only traffic that earns anything is what honest nodes chose to send through them.

### Asked at setup

**Carrying is off without a yes,** as in ADR 0012, and offered with yes recommended. Setup says:

> Your node can carry encrypted video between other nodes, under its machine name. This uses some of your upload, and builds your node's standing with the nodes it helps, which can get it direct links and quicker service. You can't see what's inside. The nodes next to yours see your internet address sending it, and you answer for what your connection sends, as with anything else you do online.

- **Limits:** a share of `max_upload_mbps`, behind everything else the node does, and paused by the owner at any time or on a metered connection.
- **A refusal is normal.** A hop at its limit says busy, and V tries another hop or a relay.
- Limits are set from the simulator.
- **A node that doesn't carry** still gets every public post and video, through hops or relays.

### What it carries

- **OSP media between nodes only.** No general proxy, no internet exit, no address chosen by the asker, nothing on the local network.
- **A host's hop delivers only to the hosts that picked it,** and a viewer's hop sends only to a hop that a host named. Neither can see inside, so neither can tell an OSP video from other bytes. Stage B says how a hop checks the ends before it's built, so this doesn't become a general proxy.
- **It never fails open.** If anything is refused or fails, the transfer uses a relay or reports an error, and never dials a stranger directly.
- **After a [delete](0019-taking-posts-and-media-down.md),** hosts running the standard software stop serving, so a hop has nothing to carry. It can't refuse a file itself, as it can't see one.

### What it doesn't solve

- **Some pairs, and some hops, can't connect** because of their routers. Those stay on relays.
- **Viewers with earned peers** may often find a host among them, which needs no hop.
- **An operator holding a share p of the hops** picked at both ends sees both ends of about p² of transfers: 1% at 10%. Hosts pick only hops with standing, so it has to earn that first, by carrying real bytes. This can't be ruled out, only priced.

## Consequences

**Privacy.** Each end's address is seen by the hop it picked, as well as by its earned peers. A viewer's hop sees which host an address fetches from, as a relay does. A machine that carries shows its address under its machine name to the nodes next to it, so anyone who also sees its owner's node's address, such as an earned peer, can match the two. That changes [ADR 0015](0015-nodes-meet-first.md)'s machine name, which until now has shown no address. A node that doesn't carry is unchanged.

**Responsibility.** Anyone next to a hop sees its address sending, and it can't tell what it carries. Each owner answers for what their node sends, and no document may claim otherwise ([doc 13](../13-hosting-roles-and-follower-swarm.md)).

**Cost.** Each byte through two hops is uploaded from three homes: 15.7 GB becomes about 47 GB for a 52 MB video to 300 viewers. That holds for every transfer between a pair without a direct link, as the swarm spares the creator, not the hops. It shrinks only as pairs earn direct links, which stage A measures. Each hop adds a delay, and the slowest hop's spare upload limits the transfer.

**Operational.** One more protocol, likely on iroh's custom transports like [`osp-tor`](../../crates/osp-tor/), which sit outside iroh's semver guarantees.

**Changes to accepted records:**

- ADR 0014: media between nodes without a direct connection can go through hops before relays. ADR 0014 left this open, as relays and forwarding run by the community.
- ADR 0015: a machine that carries shows its address to the nodes next to it, and earns standing for what it carried ([ADR 0016](0016-machine-acquaintances.md#the-local-record)).

**Licensing.** No new dependency chosen.

## Building it

Each mark is set before its run.

| Stage | Work | Gate |
| --- | --- | --- |
| A | Measuring only, in `osp-sim`: B8 with several posts over a shortened month, run three ways: relays alone; ADR 0018's earned links and relays; earned links, two hops and relays. Some nodes say yes to carrying, each with an upload limit | **Build further only if the hops carry at least a quarter of what's still relayed with earned links alone** (a starting value), B8's marks hold, and the share going direct grows over the month |
| B | Picking hops, carrying with limits, credit, asking at setup | V and H fetch through two hops, neither learns the other's address, and V's hop learns no account, in a packet capture. Limits, pause and busy all work |
| C | Hostile tests: loops, floods, a hop that drops traffic, a hop naming a host that didn't pick it, a node asking for an address | No open proxy, no amplification, no stranger gets an address; failures fall back to relays |
| D | Full run: B8 with several posts, compared with relays alone and with ADR 0018 alone | Marks for startup, seeking, creator upload, relay bytes, each node's data use and each home's upload |

Stage A reports, for each way and each simulated week: the bytes going direct, through hops and through relays; the upload added to hops' homes; how long until followers hold the video; how many weeks a newcomer that carries takes to be picked at hosts' ends; and, with one operator holding 10% of the machines that carry, the share of transfers where it saw both ends. Only hops that said yes and have upload left under their limits count.

`osp-sim` has a relay since ADR 0014's "measuring media's default" ([results](../testing/network-conditions.md#media-through-relays)), but no routers yet, which need real networks as well. Stage A also needs ADR 0018's layers on a shortened clock, its step 2.

## Validation

A packet capture and tests that no public reply gives anyone an address; a hop at its limit doesn't harm its owner's connection; nothing reaches a loopback or local address; forged requests are refused. Every test counts each hop's bytes, and keeps creator upload apart from relay bytes. ADR 0018's price check includes an attacker carrying traffic: what share of viewers' and hosts' addresses it learns, and what it carried to get them.

**Revisit** if stage A misses its mark, if carrying proves harmful on ordinary home connections, if people turn it down at setup, or if an operator sees both ends of more transfers than its share of hops suggests.
