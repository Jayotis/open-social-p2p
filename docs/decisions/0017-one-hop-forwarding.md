# ADR 0017: Two hops for pairs without a direct link

**Status:** Accepted (2026-11-08), proposed 2026-11-04. Not built. Stage A, measuring only, comes before the invited alpha; the rest is built only if stage A says it's worth it. Until then, [ADR 0014](0014-finding-people-and-private-messages.md)'s media rules stand: media between nodes without a direct connection goes through relays.

Revised 2026-11-07. The first draft had one forwarder, between nodes that had each earned a direct link with it ([ADR 0018](0018-earned-direct-connections.md)). That left out the pairs that need it most: newcomers and casual viewers have no earned peers, and with media's default at `nobody`, that's nearly every pair. This draft uses two hops of volunteer nodes, each end picking the one next to it, and carrying now earns standing, credited by the node that received the bytes ([ADR 0016](0016-machine-acquaintances.md#the-local-record)). The first draft's one hop remains, as the case where both ends pick the same node. The machine next to an end sees its address, so a host's end picks only its direct peers, as its hop learns what a direct link would, and a viewer's end also machines that finished jobs for it, both only with the owner's yes to [ADR 0018's question](0018-earned-direct-connections.md#asked-at-setup); newcomers start in the middle, where they see neither end. Credit is for finished jobs, not bytes, and a layer of encryption for each hop proves which hops a transfer went through.

## Context

OSP keeps a home address hidden from strangers. It doesn't promise anonymity, and public posts are public. People who want more can use an alias, a VPN, or a node away from home.

**Media's default stays `nobody`,** so media between nodes goes through relays: n0's public relays for now, which are meant for development and hobby use. [Measured](../testing/network-conditions.md#media-through-relays) with a relay at their limits, a 52 MB video to 300 followers, all behind the relay, put 19.3 GB through it, about one copy for each follower, and the swarm took over twice as long, missing two of B8's marks. Copies save the creator's upload ([B8](../testing/b8-creator-upload.md)), not the relay's.

[ADR 0018](0018-earned-direct-connections.md) moves bytes off relays between pairs that have earned a direct link, which takes a month or more of work. Until then, and for pairs that never earn one, somebody has to carry the bytes. Home nodes with upload to spare could, if each node's address is shown only to nodes it chose.

That costs upload. Two hops mean three home uploads for each byte: a 52 MB video to 300 viewers goes from 15.7 GB to about 47 GB. A newcomer's turn in the middle adds a fourth. It's worth doing only if it frees enough relay traffic, and as pairs earn direct links, the hops carry less.

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
- **Three hops on every transfer, as Tor builds circuits,** with a middle hop that sees no address. Four uploads for each byte. On a public network where each end decides who sees its address, the middle hop protects nothing either end hadn't chosen to show. Rejected. A middle hop is used only on newcomers' turns, below, as a place where a machine that has done nothing yet can carry without seeing either end.
- **Newcomers next to the viewer,** a share of viewers' picks kept for machines with no standing, so they could start. That machine, which had done nothing yet, would see the viewer's address, and a home address isn't given for nothing. Rejected for the middle.
- **n0 setting up the connections while home nodes carry the bytes,** so the hop next to an end never sees its address. Whoever exchanges packets with a node directly sees its address; a relay hides it only while it carries the bytes itself. So hiding an end from its hop puts the video back on the relay. Rejected. n0 still coordinates hole punches.
- **Each node in a transfer signing a receipt for the others,** kept to show who did its part. Receipts kept to show others publish who watches whom, which [the review](../reviews/2026-10-21-review-of-docs-12-and-13.md) removed (A1), and twenty names on one machine can sign them for one another (A4). Rejected: each node keeps its own record of what it checked.
- **Two hops, each end picking the one next to it from machines it has worked with, and a newcomer in the middle on its turns.** Chosen for testing.

## Decision

### What does what

| Function | Mechanism | What it gives |
| --- | --- | --- |
| Hosting | Whole verified copies, follower and resharer swarms | Sources; the creator is spared uploads |
| Downloading in parts | iroh-blobs ranges from up to 3 sources | Speed. Parts hide no address |
| Earned link | ADR 0018's direct connection | Cheap bytes between two nodes that see each other's address |
| Two hops | Volunteer nodes passing encrypted traffic, one picked by each end, and a newcomer between them on its turns | Bytes off relays, each end's address seen only by the hop it picked, never by a newcomer |
| Relays | As today | Hole punches; the fallback |

A hop need not hold the video. A host need not carry.

### Two hops

A viewer V fetches from a host H, a follower or the creator, and the two have no direct link. V picks a hop next to it, P, and H has picked one next to it, Q. The bytes go H → Q → P → V. On a newcomer's turn ("Who picks"), V puts a machine M between the two: H → Q → M → P → V.

| Who | Sees | Doesn't see |
| --- | --- | --- |
| V's hop, P | V's address; which host V is reaching; the next hop's address; the timing and the number of bytes | V's account, as V reaches P under a key used only for that; H's address; the contents |
| H's hop, Q | H's address and media endpoint, so whose node it is; the next hop's address; the timing and the number of bytes | V's address or account; the contents |
| A newcomer in the middle, M | P's and Q's addresses and machine names; the timing and the number of bytes | Either end's address or account; which host or viewer; the contents |
| V and H | Each other's media endpoint | Each other's address |

- **The connection is encrypted from V's media endpoint to H's.** No hop can see or check what it carries.
- **Each hop also adds a layer of encryption that only it and V share,** as Tor's cells do. A hop that's skipped breaks the transfer, so a file that arrives intact proves it went through every hop V set up, including those V can't see. Stage B takes the construction from a reviewed design, not one of our own.
- **H names its hop** when V asks for its endpoints over Tor, as it hands over its direct endpoint today. Its hop keeps a connection with it, as a node does with its relay.
- **No path is searched for,** and no lists are swapped, as the first draft's earned peers had to do: each end picks its own hop, and the host names its own. The layers are the one piece of routing to build, over a fixed path of up to three hops.
- **If V would pick Q itself,** the transfer takes that one hop.
- **An end with no hop of its own.** A host whose rule lets anyone connect directly, as a creator who chooses to be public may, needs none: the next hop reaches it directly. Otherwise that end's leg goes through a relay, as today, and the other hops still carry and earn. If neither end has a hop, the transfer goes through relays alone.
- iroh's relays still coordinate hole punches between nodes and hops.

### Who picks

Each end picks the hop next to it only with its owner's yes to earned links ("Asked at setup"). That hop learns the end's address, so it's picked as carefully as a direct link:

- **At a host's end, only its direct peers,** earned ([ADR 0018](0018-earned-direct-connections.md)) or approved by hand, which see its address already. The hop learns whose node it is too, the targeted flood in ADR 0018's table of harms, so it costs what a direct link costs.
- **At a viewer's end, also machines that finished jobs for it** ([ADR 0016](0016-machine-acquaintances.md#the-local-record)): transfers they carried for it that completed, on at least 3 different days (a starting value). This hop learns an address that's fetching, not an account, so the bar is lower.
- **Never a machine with no standing.** A home address isn't given for nothing.

Nobody picks anything by hand. Approving a friend's node only adds one more peer.

**Newcomers carry in the middle.** On 1 transfer in 10 that goes through hops (a starting value), V puts a machine with no standing between them, one from its own table of machines ([ADR 0016](0016-machine-acquaintances.md#meeting-more-machines)). It sees only what's next to it, hops that said yes to being seen as carriers or a relay, and neither end. It does a real job, so V credits it, and once it has finished jobs for V on enough days, V may pick it next to itself. A reference V counts ([ADR 0016](0016-machine-acquaintances.md#references)) gets a machine one of these turns sooner, never a place next to an end. Those transfers take a fourth home upload, about 3.1 for each byte on average.

**Starting from nothing.** A new network, with nobody approving anything, has no hops, so transfers go through relays, as today. Earned links come from downloads nodes make anyway, and from creators' spot checks of the followers who host their posts (ADR 0018); each earned peer can be a hop; hops give newcomers middle turns; and middle turns build standing at viewers' ends. A small group whose owners approve one another by hand, as the first testers may, gives it a quicker start, but must never be needed: nobody has to know anyone to join. Whether it grows with nobody acting, how fast, and whether it carries on once the first nodes leave, is stage A's first question.

### Credit

What counts is whether a machine did what it was asked.

- **A hop is credited for jobs it finished:** transfers that completed and passed the hash check at the node that received them, each above a minimum size and counted once, on different days. Jobs it dropped count against it. Bytes are counted for upload limits, not for standing, so a big file earns no more than a small one above the minimum.
- **Each node credits only what it checked itself.** The node that received the file credits every hop, as the layers prove the path. The host sees its own transfer finish, so it notes how its hop did, which keeps good hops picked; that's no first credit, as its hop is a direct peer already. Watching earns nothing.
- **Nothing is shared,** and no receipt leaves a node.
- **A closed group convinces only itself.** Machines crediting one another, or signing references for one another, gain nothing with a node that hasn't worked with them. Their way in is a machine that node already relies on, and that way is capped ([ADR 0016](0016-machine-acquaintances.md#references)).
- A hop can't make an honest node send traffic through it, as each end picks its own.
- A transfer that fails credits nobody. A hop can't alter what it carries without breaking the encryption, so it can only drop or slow it, and one that does is picked less.
- What standing gets a machine is service, never access to content ([ADR 0016](0016-machine-acquaintances.md#service-not-content)).

This replaces the first draft's "it earns nothing", whose reason was that a forwarder had already earned its place, so nothing would tempt anyone to make fake traffic. Hops now include newcomers, who need a way to start, and the only traffic that earns anything is what honest nodes chose to send through them.

### Asked at setup

**Two questions, each off without a yes,** as in ADR 0012, and offered with yes recommended.

**Carrying.** Setup says:

> Your node can carry encrypted video between other nodes, under its machine name. This uses some of your upload, and builds your node's standing with the nodes it helps, which can get it direct links and quicker service. You can't see what's inside. The nodes next to yours see your internet address sending it, and you answer for what your connection sends, as with anything else you do online.

**Letting machines that worked for the node see its address,** as the hop next to it or as a direct link, is one consent, so it's one question: [ADR 0018's](0018-earned-direct-connections.md#asked-at-setup), which sets `direct = "earned"`. It's offered once ADR 0018's tests pass, so using hops waits for those too. `nobody` keeps its meaning, and a node that hasn't said yes keeps its own leg on relays, as today. Nothing else is asked: no peers, relays or hops are picked by hand.

For carrying:

- **Limits:** a share of `max_upload_mbps`, behind everything else the node does, and paused by the owner at any time or on a metered connection.
- **A refusal is normal.** A hop at its limit says busy, and V tries another hop or a relay.
- Limits are set from the simulator.
- **A node that doesn't carry** still gets every public post and video, through hops or relays.

### What it carries

- **OSP media between nodes only.** No general proxy, no internet exit, no address chosen by the asker, nothing on the local network.
- **A host's hop delivers only to the hosts that picked it,** and the other hops send only along the path the viewer set up. None can see inside, so neither can tell an OSP video from other bytes. Stage B says how a hop checks the ends before it's built, so this doesn't become a general proxy.
- **It never fails open.** If anything is refused or fails, the transfer uses a relay or reports an error, and never dials a stranger directly.
- **After a [delete](0019-taking-posts-and-media-down.md),** hosts running the standard software stop serving, so a hop has nothing to carry. It can't refuse a file itself, as it can't see one.

### What it doesn't solve

- **Some pairs, and some hops, can't connect** because of their routers. Those stay on relays.
- **Viewers with earned peers** may often find a host among them, which needs no hop.
- **An operator holding a share p of the hops** picked at both ends sees both ends of about p² of transfers, 1% at 10%, if picks were independent. They aren't quite: ends that know the same few machines pick from the same few. Ends pick only machines with standing, so it has to earn that first, by finishing real jobs. This can't be ruled out, only priced, and stage A measures it rather than assuming p².
- **A host with no direct peers,** such as a new creator, has no hop of its own until it earns links. Its leg stays on relays meanwhile, unless it chooses to be public.

## Consequences

**Privacy.** With its owner's yes to earned links, each end's address is seen by the hop it picked: at a host, a direct peer, which sees it anyway; at a viewer, also a machine that finished jobs for it. Never by a machine new to it. A viewer's hop sees which host an address fetches from, as a relay does. A machine that carries shows its address under its machine name to the machines next to it, newcomers in the middle included, so anyone who also sees its owner's node's address, such as an earned peer, can match the two. That changes [ADR 0015](0015-nodes-meet-first.md)'s machine name, which until now has shown no address. A node that neither carries nor uses hops is unchanged.

**Responsibility.** Anyone next to a hop sees its address sending, and it can't tell what it carries. Each owner answers for what their node sends, and no document may claim otherwise ([doc 13](../13-hosting-roles-and-follower-swarm.md)).

**Cost.** Each byte through two hops is uploaded from three homes, four on a newcomer's turn, about 3.1 on average: 15.7 GB becomes about 49 GB for a 52 MB video to 300 viewers. That holds for every transfer between a pair without a direct link, as the swarm spares the creator, not the hops. It shrinks only as pairs earn direct links, which stage A measures. Each hop adds a delay, and the slowest hop's spare upload limits the transfer.

**Operational.** One more protocol, with a layer of encryption for each hop, likely on iroh's custom transports like [`osp-tor`](../../crates/osp-tor/), which sit outside iroh's semver guarantees.

**Changes to accepted records:**

- ADR 0014: media between nodes without a direct connection can go through hops before relays. ADR 0014 left this open, as relays and forwarding run by the community. A node that allows nobody still shows its address to no node it fetches from; with its yes to earned links ([ADR 0018](0018-earned-direct-connections.md)), the hop next to it sees it.
- ADR 0015: a machine that carries shows its address to the nodes next to it, and earns standing for what it carried ([ADR 0016](0016-machine-acquaintances.md#the-local-record)).

**Licensing.** No new dependency chosen.

## Building it

Each mark is set before its run.

| Stage | Work | Gate |
| --- | --- | --- |
| A | Measuring only, in `osp-sim`: B8 with several posts over a shortened month, run three ways: relays alone; ADR 0018's earned links and relays; earned links, two hops and relays. Some nodes say yes to carrying, each with an upload limit, and some to earned links. One run starts from zero standing, with nobody approving anything; another from 10 nodes approved by hand to one another, with 100 strangers joining over the month, and the 10 leaving halfway | **Build further only if the hops carry at least a quarter of what's still relayed with earned links alone** (a starting value), B8's marks hold, and the share going direct grows over the month |
| B | Picking hops, newcomers' turns, the layers, carrying with limits, credit for finished jobs, asking at setup | V and H fetch through two hops, and three on a newcomer's turn; neither learns the other's address, V's hop learns no account, and a newcomer neither end's address, in a packet capture. A skipped hop breaks the transfer. Limits, pause and busy all work |
| C | Hostile tests: loops, floods, a hop that drops traffic, a hop skipping the next one, a hop naming a host that didn't pick it, a node asking for an address, a newcomer trying to get next to an end, names crediting one another | No open proxy, no amplification, no stranger gets an address; failures fall back to relays |
| D | Full run: B8 with several posts, compared with relays alone and with ADR 0018 alone | Marks for startup, seeking, creator upload, relay bytes, each node's data use and each home's upload |

Stage A reports, for each way and each simulated week: the bytes going direct, through hops and through relays; the upload added to hops' homes; how long until followers hold the video; from zero standing and from the 10, how many weeks until hops carry anything, and whether they still do once the 10 leave; how many weeks a newcomer takes from the middle to being picked next to a viewer; and, with one operator holding 10% of the machines that carry, the share of transfers where it saw both ends, and how many ends' addresses it saw for the bytes it carried. Only hops that said yes and have upload left under their limits count.

`osp-sim` has a relay since ADR 0014's "measuring media's default" ([results](../testing/network-conditions.md#media-through-relays)), but no routers yet, which need real networks as well. Stage A also needs ADR 0018's layers on a shortened clock, its step 2.

## Validation

A packet capture and tests that no public reply gives anyone an address; a hop at its limit doesn't harm its owner's connection; nothing reaches a loopback or local address; forged requests are refused. Every test counts each hop's bytes, and keeps creator upload apart from relay bytes. ADR 0018's price check includes an attacker carrying traffic: what share of viewers' and hosts' addresses it learns, and what it carried to get them.

**Revisit** if stage A misses its mark, if carrying proves harmful on ordinary home connections, if people turn it down at setup, or if an operator sees both ends of more transfers than its share of hops suggests.
