# ADR 0018: Earned direct connections

**Status:** Proposed (2026-11-04). Not built. Would change [ADR 0014](0014-finding-people-and-private-messages.md)'s "Who may connect directly". Until accepted and built, ADR 0014's rules stand, and the layers and values below are a first draft to think over.

Revised 2026-11-07, still proposed. Work now counts whether a node served the bytes itself or its machine carried them as a hop ([ADR 0017](0017-one-hop-forwarding.md)), once the node shows the granting peer which machine is its own; a reference ([ADR 0016](0016-machine-acquaintances.md#references)) lowers the marks but never replaces first-hand work; and a peer that offers a link meets "both ways". Its question at setup is also the yes for using hops ([ADR 0017](0017-one-hop-forwarding.md#who-picks)), as the hop next to a node sees its address as a direct link does, and a machine's carrying counts as jobs finished, not bytes. Direct links are meant to become the main path between nodes that work together, taking bytes off hops and relays. Each changed rule says what it replaced and why.

## Context

**OSP is for people to post and be found in public.** It isn't a private platform, and nothing here is anonymity. Someone who wants more can use an alias, a VPN, or a node away from home. The aim is a network ordinary people can use, and that takes tradeoffs: [the vision](../01-vision-and-principles.md) says to avoid exposing home addresses *unnecessarily*.

A node today chooses one rule for direct connections: anyone, people it follows, contacts approved by hand, or nobody. `anyone` shows its address to strangers. `nobody` sends media through relays, and [ADR 0014](0014-finding-people-and-private-messages.md) says that costs someone bandwidth. `follows` and `approved` need a person to act, and they don't reach the people who most need a link: followers of one creator, who hold the same files and have never heard of each other.

So a node should be able to earn a direct connection by doing useful work over time: it works for the information. Strangers don't get an address. Media's default stays `nobody`: pairs without a direct link use [ADR 0017](0017-one-hop-forwarding.md)'s two hops, or relays, and earned links are the way off both.

**What a revealed address can lead to** (general knowledge, not tested here):

| Harm | How likely | What it takes |
| --- | --- | --- |
| Someone floods the connection until the internet drops | The likeliest targeted harm; happens to streamers and gamers | The address, and a reason to go after that account. Knowing the account is enough; a real name isn't needed |
| Rough location: a city or an ISP, not a street address | Easy, but matters mostly to someone already after them | The address |
| Someone reports traffic from the address to its ISP | Depends on what the node sends | The address |
| Being hacked | Low, about the same as being online | The address, and a flaw in something the machine has open |

For most people this is low and can be recovered from: a new address from the ISP ends it, and a VPN covers the rest. Restarting the router often keeps the same address, so it isn't a way out on its own. The first two fall mostly on people who attract hostility.

Questions to settle:

1. What counts as work?
2. What are the steps from stranger to direct?
3. Who agrees, and how is the endpoint handed over?
4. How does someone get out?

## Options considered

- **Machines earning it, through machine names** (an earlier draft of this record). Most of the evidence would be answering greetings and passing on valid records, which a rented server can do all day for almost nothing, and [ADR 0013](0013-trusted-followers.md) rejected automatic trust for that reason. It also needs media served under machine names: a new proof, new endpoints, and a follower telling each creator its machine key, which undoes [ADR 0015](0015-nodes-meet-first.md)'s separation. Rejected. The revision counts something else from the machine layer: jobs a machine finished as a hop, which cost real upload and which the granting node checked itself. That needs no media served under machine names, and a node shows its machine only to a peer it asks for a link ("What counts as work").
- **Approving each peer by hand.** Exists as `approved`, and stays. Too slow to matter at scale.
- **`follows`.** Exists, and stays. A follow is one-way, and says nothing of work.
- **Removing public IDs from `WhoHosts` first,** naming followers by a bare media endpoint. It would improve on [ADR 0012](0012-sharing-prefetched-copies.md)'s disclosure, but this record needs those IDs: a peer's record, and the request for its endpoints, both go by public ID. Not now. It could come later, with the handover moved onto the media endpoint and an unbound direct endpoint of its own. An alias covers the name meanwhile.
- **Verified media exchange.** Chosen. It costs real upload for each peer, and it helps the network even if that peer later turns out badly. The pairs that earn it are the ones holding the same files, which would otherwise fill the relays. [ADR 0011](0011-measuring-peers.md) already counts most of it.

## Decision

### A fifth rule

`direct = "earned"` allows contacts approved by hand and nodes that have earned it. It also lets the node use hops ([ADR 0017](0017-one-hop-forwarding.md#who-picks)): as a host, only its direct peers; as a viewer, also machines that finished jobs for it. As in ADR 0014, a connection is direct only when **both** sides' rules allow it, and otherwise media goes through relays.

### What counts as work

**Work is something this node asked for, received and checked.**

- **It counts:** a download from that node that completed, whole or its part of one, each chunk checked against the hash.
- **It counts too:** transfers that came to this node through that node's machine as a hop ([ADR 0017](0017-one-hop-forwarding.md#credit)) and completed, each chunk checked, once that node has shown the machine is its own (below).
- **A peer can't push work at a node.** A node asks only for what it wants, doesn't ask for a file it holds, and picks its own hop.
- **Fakes crediting each other gets nowhere** ([the review's A4](../reviews/2026-10-21-review-of-docs-12-and-13.md)). Each node counts only what it received itself, and nothing is shared.
- **Size counts too.** Three tiny files, or one file asked for in many small pieces, isn't real work. So layer 4 also needs a minimum of checked bytes, set in the simulator, and a piece fetched twice counts once.
- **The other direction** is media this node served to that peer. It's needed for "both ways", as it shows the pair actually trade, but it isn't work on its own, as a peer can ask for anything.
- **It doesn't count:** `Hello`, machine records, a reference, anything a node says of itself, uptime, or bytes that failed the check.

**A machine's work counts once it's shown.** The first draft counted nothing from the machine layer, as using it would tie a machine name to a public ID. Carrying is real work, so the tie is now offered, to one peer at a time. A node that carries asks a peer for a link once it fetches regularly from that peer (layer 2), and shows it which machine is its own; the peer then adds what that machine carried for it. Only that peer learns it. If the link is granted, the peer sees the node's address anyway. If it isn't, the peer still knows, and setup says so. It's shown live, leaving the peer nothing it can show to others, and the way of proving it is chosen from reviewed designs when it's built, as ADR 0015 says of introductions.

**A reference lowers the marks, never replaces the work.** A reference this node counts ([ADR 0016](0016-machine-acquaintances.md#references)), for a machine that has been shown, halves layer 4's days and checked bytes. Layers 1 to 3 are always reached first-hand, with the node granting the link.

### Layers

Each layer is reached by working through the one before. Values are starting values for the simulator.

| Layer | How it's reached | What it gives |
| --- | --- | --- |
| 0. Stranger | First contact | Media through relays; the stranger share when busy ([doc 12](../12-peer-reciprocity-and-recognition.md)) |
| 1. Has worked for us | One completed download from it, or a transfer its machine carried for us, once shown | On record, nothing more |
| 2. Regular source | Work on 3 different days, no bad data | Tried sooner when fetching, or picked sooner as a hop |
| 3. Both ways | Layer 2, and this node has served it on 3 different days, or it has offered this node a link | Ahead of strangers when busy |
| 4. Earned | Layer 3 held, at least 30 days since first contact, enough checked bytes, and room under the caps. With a reference, 15 days and half the bytes | A direct connection, if both sides' rules allow it |

- **An offer meets "both ways".** In the first draft, a creator and its followers never got there, as a creator doesn't fetch from its followers, however much a follower's machine carried for it. A peer that offers a link has counted this node's work. That's no easier for an attacker than before, as a peer could always meet "both ways" by asking for anything ("The other direction", above).
- **Layers 2 and 3 change who gets served first,** so each needs its own B8 run, and layer 3 needs the review's evidence on newcomers (A3). They can start as labels with no effect.
- **Counting days** takes one new counter per peer, in each direction, counting each day once, across restarts too. ADR 0011's rule holds: faded totals, not which files or when.
- **It's per node,** as ADR 0011's rows are. Grouping a person's nodes can come later.

### Limits

- **At most 32 earned peers at once, and 2 new ones a week.** Fewer means less exposure; more means more bytes off relays. The simulator sets these.
- **Standing fades,** by half every 14 days as in ADR 0011. A peer below half of a layer's mark drops back, so keeping a link takes continued work. What it has already learned, it keeps.
- **Bad data sends a peer back to layer 0,** and it can't rise again for 30 days. It's no judgement of anyone: a failing disk looks the same.

### Both sides, and the handover

Most of this exists and is tested in ADR 0014's direct endpoint:

1. When a peer reaches layer 4, the node asks it for its endpoints over Tor, offering its own direct endpoint, as [`Endpoints`](../../crates/osp-node/src/endpoints.rs) does now. A node that carries also asks at layer 2, showing its machine, as above; the peer's direct endpoint, if it comes back, is its offer.
2. The other node's rule decides for itself. Its direct endpoint comes back only if both allow it.
3. The direct endpoint keeps refusing everyone else on the first packet, before any address is shown. One gap stays, as ADR 0014 found: someone who already knows one of the node's addresses and its port, and comes straight to it, may be told its other addresses before it's refused.
4. If either side stops allowing it, its direct endpoint refuses and the other falls back on the media endpoint. It never falls back on a direct connection with a stranger.

No new endpoint or protocol is needed. New: the rule, the check behind it, the request at layers 2 and 4, showing a machine, and using an earned peer's direct endpoint when another node names that peer as a host.

### Getting out

- `osp direct list` shows who has earned a link, and `osp peers` shows each peer's layer.
- `osp direct remove <node>` drops one, and a new `osp direct deny <node>` keeps it out.
- **`osp direct reset` drops every earned peer at once,** sets the rule to `nobody` and closes direct connections, so a peer approved by hand doesn't keep one. The owner's phones aren't affected. Turning `earned` on again is the owner's choice. With a new address from the ISP or a VPN, that's the way out for someone who has become a target.
- None of this makes an address secret again.

### Asked at setup

Offered once its tests pass, with yes recommended. It's one question for direct links and for using hops, as both show the node's address to machines that worked for it ([ADR 0017](0017-one-hop-forwarding.md#asked-at-setup)). Existing nodes keep their rule until the owner changes it. ADR 0014 left the default to measuring, and [the same measurement](#building-it) applies here.

> Nodes that have worked well with yours for a while can connect to it directly, or carry its video as the machine next to it, which makes video cheaper and faster for everyone. They'll see your internet address, and can keep it or pass it on. Turning this off later can't take it back. Nobody else gets it this way, and machines new to yours never do. If your node carries video for others, the nodes it asks for a link learn which machine name is yours. You answer for what your node shares. For more privacy, use an alias or a VPN, or run your node away from home.

## Consequences

**Privacy.**

- An earned peer learns the node's address and public ID, and which files it fetched from that node. An alias covers the name.
- A node that carries shows the peers it asks for a link which machine is its own, and they keep that. A machine that carries shows its address to the nodes next to it ([ADR 0017](0017-one-hop-forwarding.md)). A node that doesn't carry keeps ADR 0015's separation as it was: its machine name stays on Tor and shows no address.
- ADR 0011's record still never leaves the node. A peer learns only that it was allowed.

**Security.**

- **It doesn't stop a patient attacker.** It makes them pay. At a few hundred MB a month for each node, a rented server with a few terabytes could still reach thousands. Meanwhile it does real work. This is a price, not a wall.
- A rate cap doesn't protect one node: one earned peer is enough to get its address.
- **Aiming at one person is harder.** A peer earns a place only with a node that keeps fetching from it, and it can't make a node do that. It can try, by hosting what that person watches and serving it fast, so the price check tries that too. Carrying for that person counts as well, but at a host's end only an earned peer is picked, and at a viewer's end it takes jobs finished for that node on several days, starting in the middle, where it sees neither end. If it does get there, it knows the address and the public ID, and only an alias keeps the name from it.

**Responsibility.** A follower's node shares copies of what its owner follows, and an earned peer can see its address doing so. Each owner answers for what their node sends, and setup says so. They can drop a file with [ADR 0019](0019-taking-posts-and-media-down.md)'s `osp block`.

**Operational.** Fewer relay bytes between pairs that earn a link, if pairs meet often enough, which is untested. Newcomers and casual viewers stay on relays. Some pairs can't connect because of their routers, and stay on relays too.

**Changes to accepted records,** to be named when this is accepted:

- ADR 0014: "Following someone, or knowing them a long time, never grants it on its own." With the owner's yes, work over time now can. "An address once given can't be taken back" stays true.
- ADR 0014: its list of rules gains `earned`.
- ADR 0011: its record starts changing behaviour, as doc 12's later phases planned.
- ADR 0015: jobs a machine finished as a hop count, once its node shows the machine is its own, as [ADR 0016](0016-machine-acquaintances.md#the-local-record) says. Being known still grants nothing.
- ADR 0013 doesn't change: being trusted grants nothing on its own, and standing never opens private posts.

**Licensing.** No new dependency.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | A relay in `osp-sim`, and ADR 0014's "measuring media's default" | Relay bytes counted with 0, 20 and 50% of followers fetching through relays |
| 2 | Day counters and layers, shown in `osp peers`, changing nothing. A shortened clock in `osp-sim`, as this takes a month | A B8 run with several posts shows **how often pairs meet again**, and, with ADR 0017's stage A, how many weeks a node that carries takes to reach each layer, with and without a reference |
| 3 | The rule, the handover, showing a machine, `deny`, `reset`, asking at setup | **At least a quarter of relayed bytes move to direct links** (a starting value). Refused nodes arriving through a relay learn no address, in a packet capture |
| 4 | Effects of layers 2 and 3, one at a time | B8's marks, and the review's A3 |

Not before the invited alpha: a handful of nodes won't reach layer 4 and have too little to exchange.

## Validation

- **A price check.** An attacker holding 10 and 25% of nodes, serving real media and carrying traffic for three simulated months, its names signing references for one another: what share of honest addresses does it hold, and how many bytes did it serve or carry to get there? It also tries tiny files, split requests, and a fast host aimed at one node. The result is reported, and decides whether setup recommends yes.
- **Admission.** A stranger, or a node with only machine evidence such as greetings and records, only references, or self-reported bytes, never gets an address. A peer that sent bad data loses its place. After `reset`, nothing connects directly, including peers approved by hand.
- **Leak test,** as in ADR 0014 and ADR 0015. Nothing public gains an address, and nothing ties a machine name to a public ID.

**Revisit** if pairs rarely meet again, if the price check shows addresses going too cheaply, if newcomers end up with worse service than they have today, or if people ask for the separation that unbinding `WhoHosts` would give.
