# ADR 0018: Earned direct connections

**Status:** Proposed (2026-11-04). Not built. Would change [ADR 0014](0014-finding-people-and-private-messages.md)'s "Who may connect directly". Until accepted and built, ADR 0014's rules stand, and the layers and values below are a first draft to think over.

## Context

**OSP is for people to post and be found in public.** It isn't a private platform, and nothing here is anonymity. Someone who wants more can use an alias, a VPN, or a node away from home. The aim is a network ordinary people can use, and that takes tradeoffs: [the vision](../01-vision-and-principles.md) says to avoid exposing home addresses *unnecessarily*.

A node today chooses one rule for direct connections: anyone, people it follows, contacts approved by hand, or nobody. `anyone` shows its address to strangers. `nobody` sends every byte through relays, and [ADR 0014](0014-finding-people-and-private-messages.md) says that costs someone bandwidth. `follows` and `approved` need a person to act, and they don't reach the people who most need a link: followers of one creator, who hold the same files and have never heard of each other.

So a node should be able to earn a direct connection by doing useful work over time: it works for the information. Strangers don't get an address.

**What a revealed address can lead to** (general knowledge, not tested here):

| Harm | How likely | Needs the person's name? |
| --- | --- | --- |
| Someone floods the connection until the internet drops | The likeliest targeted harm; happens to streamers and gamers | Yes |
| Rough location: a city or an ISP, not a street address | Easy, but matters mostly to someone already after them | Yes |
| Someone reports traffic from the address to its ISP | Depends on what the node sends | No |
| Being hacked | Low, about the same as being online | No |

For most people this is low and can be recovered from: many home addresses change when the router restarts, and a VPN covers the rest. The first two fall mostly on people who attract hostility.

Questions to settle:

1. What counts as work?
2. What are the steps from stranger to direct?
3. Who agrees, and how is the endpoint handed over?
4. How does someone get out?

## Options considered

- **Machines earning it, through machine names** (an earlier draft of this record). Most of the evidence would be answering greetings and passing on valid records, which a rented server can do all day for almost nothing, and [ADR 0013](0013-trusted-followers.md) rejected automatic trust for that reason. It also needs media served under machine names: a new proof, new endpoints, and a follower telling each creator its machine key, which undoes [ADR 0015](0015-nodes-meet-first.md)'s separation. Rejected.
- **Approving each peer by hand.** Exists as `approved`, and stays. Too slow to matter at scale.
- **`follows`.** Exists, and stays. A follow is one-way, and says nothing of work.
- **Removing public IDs from `WhoHosts` first,** naming followers by a bare media endpoint. It would improve on [ADR 0012](0012-sharing-prefetched-copies.md)'s disclosure, but this record needs those IDs: a peer's record, and the request for its endpoints, both go by public ID. Not now. It could come later, with the handover moved onto the media endpoint and an unbound direct endpoint of its own. An alias covers the name meanwhile.
- **Verified media exchange.** Chosen. It costs real upload for each peer, and it helps the network even if that peer later turns out badly. The pairs that earn it are the ones holding the same files, which would otherwise fill the relays. [ADR 0011](0011-measuring-peers.md) already counts most of it.

## Decision

### A fifth rule

`direct = "earned"` allows contacts approved by hand and nodes that have earned it. As in ADR 0014, a connection is direct only when **both** sides' rules allow it, and otherwise media goes through relays.

### What counts as work

**Work is something this node asked for, received and checked.**

- **It counts:** a download from that node that completed, whole or its part of one, each chunk checked against the hash.
- **A peer can't push work at a node.** A node asks only for what it wants, and doesn't ask for a file it holds.
- **Fakes crediting each other gets nowhere** ([the review's A4](../reviews/2026-10-21-review-of-docs-12-and-13.md)). Each node counts only what it received itself, and nothing is shared.
- **The other direction** is media this node served to that peer. It's needed for "both ways", as it shows the pair actually trade, but it isn't work on its own, as a peer can ask for anything.
- **It doesn't count:** `Hello`, machine records, a reference or vouch, anything a node says of itself, uptime, forwarded bytes, or bytes that failed the check.
- **Nothing from the machine layer counts.** Using it would tie a machine name to a public ID.

### Layers

Each layer is reached by working through the one before. Values are starting values for the simulator.

| Layer | How it's reached | What it gives |
| --- | --- | --- |
| 0. Stranger | First contact | Media through relays; the stranger share when busy ([doc 12](../12-peer-reciprocity-and-recognition.md)) |
| 1. Has served us | One completed download from it | On record, nothing more |
| 2. Regular source | Completed downloads on 3 different days, no bad data | Tried sooner when fetching |
| 3. Both ways | Layer 2, and this node has served it on 3 different days | Ahead of strangers when busy |
| 4. Earned | Layer 3 held, at least 30 days since first contact, and room under the caps | A direct connection, if both sides' rules allow it |

- **Layers 2 and 3 change who gets served first,** so each needs its own B8 run, and layer 3 needs the review's evidence on newcomers (A3). They can start as labels with no effect.
- **Counting days** takes one new counter per peer, in each direction. ADR 0011's rule holds: faded totals, not which files or when.
- **It's per node,** as ADR 0011's rows are. Grouping a person's nodes can come later.

### Limits

- **At most 32 earned peers at once, and 2 new ones a week.** Fewer means less exposure; more means more bytes off relays. The simulator sets these.
- **Standing fades,** by half every 14 days as in ADR 0011. A peer below half of a layer's mark drops back, so keeping a link takes continued work. What it has already learned, it keeps.
- **Bad data sends a peer back to layer 0,** and it can't rise again for 30 days. It's no judgement of anyone: a failing disk looks the same.

### Both sides, and the handover

Most of this exists and is tested in ADR 0014's direct endpoint:

1. When a peer reaches layer 4, the node asks it for its endpoints over Tor, offering its own direct endpoint, as [`Endpoints`](../../crates/osp-node/src/endpoints.rs) does now.
2. The other node's rule decides for itself. Its direct endpoint comes back only if both allow it.
3. The direct endpoint keeps refusing everyone else on the first packet, before any address is shown.
4. If either side stops allowing it, its direct endpoint refuses and the other falls back on the media endpoint. It never falls back on a direct connection with a stranger.

No new proof, endpoint or protocol is needed. New: the rule, the check behind it, the request at layer 4, and using an earned peer's direct endpoint when another node names that peer as a host.

### Getting out

- `osp direct list` shows who has earned a link, and `osp peers` shows each peer's layer.
- `osp direct remove <node>` drops one, and a new `osp direct deny <node>` keeps it out.
- **`osp direct reset` drops every earned peer at once** and goes back to relays. With a new address from the ISP or a VPN, that's the way out for someone who has become a target.
- None of this makes an address secret again.

### Asked at setup

Offered once its tests pass, with yes recommended. Existing nodes keep their rule until the owner changes it. ADR 0014 left the default to measuring, and [the same measurement](#building-it) applies here.

> Nodes that have worked well with yours for a while can connect to it directly, which makes video cheaper and faster for everyone. They'll see your internet address, and can keep it or pass it on. Turning this off later can't take it back. Nobody else gets it this way. You answer for what your node shares. For more privacy, use an alias or a VPN, or run your node away from home.

## Consequences

**Privacy.**

- An earned peer learns the node's address and public ID, and which files it fetched from that node. An alias covers the name.
- Machine names stay on Tor and show no address, so an address can't be matched to one. ADR 0015's separation is unchanged.
- ADR 0011's record still never leaves the node. A peer learns only that it was allowed.

**Security.**

- **It doesn't stop a patient attacker.** It makes them pay. At a few hundred MB a month for each node, a rented server with a few terabytes could still reach thousands. Meanwhile it does real work. This is a price, not a wall.
- A rate cap doesn't protect one node: one earned peer is enough to get its address.
- **Aiming at one person is harder.** A peer earns a place only with a node that keeps fetching from it, and it can't make a node do that. If it does get there, it knows the address and the public ID, and only an alias keeps the name from it.

**Responsibility.** A follower's node shares copies of what its owner follows, and an earned peer can see its address doing so. Each owner answers for what their node sends, and setup says so. They can drop a file with [ADR 0019](0019-taking-posts-and-media-down.md)'s `osp block`.

**Operational.** Fewer relay bytes between pairs that earn a link, if pairs meet often enough, which is untested. Newcomers and casual viewers stay on relays. Some pairs can't connect because of their routers, and stay on relays too.

**Changes to accepted records,** to be named when this is accepted:

- ADR 0014: "Following someone, or knowing them a long time, never grants it on its own." With the owner's yes, work over time now can. "An address once given can't be taken back" stays true.
- ADR 0014: its list of rules gains `earned`.
- ADR 0011: its record starts changing behaviour, as doc 12's later phases planned.
- ADR 0015 and ADR 0013 don't change: being known, or being trusted, still grants nothing.

**Licensing.** No new dependency.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | A relay in `osp-sim`, and ADR 0014's "measuring media's default" | Relay bytes counted with 0, 20 and 50% of followers fetching through relays |
| 2 | Day counters and layers, shown in `osp peers`, changing nothing. A shortened clock in `osp-sim`, as this takes a month | A B8 run with several posts shows **how often pairs meet again** |
| 3 | The rule, the handover, `deny`, `reset`, asking at setup | **At least a quarter of relayed bytes move to direct links** (a starting value). Refused nodes learn no address, in a packet capture |
| 4 | Effects of layers 2 and 3, one at a time | B8's marks, and the review's A3 |

Not before the invited alpha: a handful of nodes won't reach layer 4 and have too little to exchange.

## Validation

- **A price check.** An attacker holding 10 and 25% of nodes, serving real media for three simulated months: what share of honest addresses does it hold, and how many bytes did it serve to get there? The result is reported, and decides whether setup recommends yes.
- **Admission.** A stranger, or a node with only machine evidence or self-reported bytes, never gets an address. A peer that sent bad data loses its place. After `reset`, nothing connects directly.
- **Leak test,** as in ADR 0014 and ADR 0015. Nothing public gains an address, and nothing ties a machine name to a public ID.

**Revisit** if pairs rarely meet again, if the price check shows addresses going too cheaply, if newcomers end up with worse service than they have today, or if people ask for the separation that unbinding `WhoHosts` would give.
