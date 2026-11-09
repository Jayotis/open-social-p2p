# ADR 0018: Earned direct connections

**Status:** Accepted (2026-11-08), proposed 2026-11-04. Not built. Changes [ADR 0014](0014-finding-people-and-private-messages.md)'s "Who may connect directly" once built; until then ADR 0014's rules stand. The layers and values below are starting values, which the stages' runs set. Clarified 2026-11-09: reaching a layer and keeping it are counted apart ([below](#reaching-a-layer-and-keeping-it)).

Revised 2026-11-07. Work now counts whether a node served the bytes itself or its machine carried them as a hop ([ADR 0017](0017-one-hop-forwarding.md)), once the node shows the granting peer which machine is its own; a reference ([ADR 0016](0016-machine-acquaintances.md#references)) lowers the marks but never replaces first-hand work; and a peer that offers a link meets "both ways". A creator's spot checks of its followers' copies count too, so a creator and the followers who host its posts can earn links, though the creator never fetches from them to watch anything. Its question at setup is also the yes for using hops ([ADR 0017](0017-one-hop-forwarding.md#who-picks)), as the hop next to a node sees its address as a direct link does, and a machine's carrying counts as jobs finished, not bytes. Direct links are meant to become the main path between nodes that work together, taking bytes off hops and relays. Each changed rule says what it replaced and why.

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
- **Downloaders' receipts telling a creator which follower served them.** They'd tell the creator who fetched what from whom, and fakes can sign them for one another ([the review](../reviews/2026-10-21-review-of-docs-12-and-13.md)'s A1 and A4). Rejected: the creator checks for itself, with spot checks ("What counts as work").

## Decision

### A fifth rule

`direct = "earned"` allows contacts approved by hand and nodes that have earned it. It also lets the node use hops ([ADR 0017](0017-one-hop-forwarding.md#who-picks)): as a host, only its direct peers; as a viewer, also machines that finished jobs for it. As in ADR 0014, a connection is direct only when **both** sides' rules allow it, and otherwise media goes through relays.

### What counts as work

**Work is something this node asked for, received and checked.**

- **It counts:** a download from that node that completed, whole or its part of one, each chunk checked against the hash.
- **It counts too:** transfers that came to this node through that node's machine as a hop ([ADR 0017](0017-one-hop-forwarding.md#credit)) and completed, each chunk checked, once that node has shown the machine is its own (below).
- **It counts too, for the node whose posts they are: spot checks of a follower's copies.** A creator never fetches from its followers to watch anything, yet a follower that keeps its posts and serves them does it real work: each copy it serves is one the creator didn't upload. Today the creator checks one 1 KiB chunk when a follower says it holds a copy, and a fake can pass that while stalling everyone else. So the creator's node also fetches a piece of its own posts from the followers it names in `WhoHosts`, at random times, under a key used only for that, which a follower can't tell from any downloader's. Each piece received and checked is work it asked for, and a busy refusal counts neither way. A bounded number a day, set in the simulator. A follower that fails them stops being named. Passing shows that the follower served those pieces, not that it serves everyone else, so the simulator measures how well passing predicts whole downloads under load.
- **A peer can't push work at a node.** A node asks only for what it wants, doesn't ask for a file it holds, except in its own spot checks, which it times and bounds itself, and picks its own hop.
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
| 1. Has worked for us | One completed download from it, a transfer its machine carried for us, once shown, or a spot check of its copy of our posts that passed | On record, nothing more |
| 2. Regular source | Work on 3 different days in the last 30, no bad data | Tried sooner when fetching, or picked sooner as a hop |
| 3. Both ways | Layer 2, and this node has served it on 3 different days in the last 30, or it has offered this node a link | Ahead of strangers when busy |
| 4. Earned | Layer 3 held, at least 30 days since first contact, 100 MB of checked bytes received in the last 30 days, and room under the caps. With a reference, 15 days and half the bytes | A direct connection, if both sides' rules allow it |

- **A creator and its followers can get there.** A creator never fetches from its followers to watch anything, so in the first draft neither could reach layer 3 with the other. Now the creator's spot checks put a follower that hosts its posts on its record. On the follower's side, which can't tell the checks from strangers, the creator's offer of a link meets "both ways": a peer that offers has counted this node's work. That's no easier for an attacker than before, as a peer could always meet "both ways" by asking for anything ("The other direction", above).
- **Layers 2 and 3 change who gets served first,** so each needs its own B8 run, and layer 3 needs the review's evidence on newcomers (A3). They can start as labels with no effect.
- **Counting days** keeps, for each peer and each direction, the days in the last 30 on which there was work, each day once, and the checked bytes received on each of those days as one total, across restarts too, beside ADR 0011's faded totals ([below](#reaching-a-layer-and-keeping-it)). That's which days and how much, never which files, and nothing older than 30 days.
- **It's per node,** as ADR 0011's rows are. Grouping a person's nodes can come later.

### Reaching a layer and keeping it

Clarified by the maintainer on 2026-11-09. A first reading counted the days toward a layer as a faded total too, so "3 days" meant 3 faded points, which takes work about every 8 days to reach. A layer means three actual days of verified work.

- **Reaching a layer counts actual days.** A day counts when the peer did work this node asked for, received and checked ("What counts as work", above) on it. Each day counts once for each peer and direction, and only days in the last 30 count. Three such days reach layer 2. Layer 3 still needs its own both-ways part: this node served the peer on 3 different days in the last 30, counted the same way, or the peer offered a link.
- **Keeping a layer uses the faded standing.** Once reached, a layer is kept while the faded totals, halving every 14 days as in ADR 0011, stay at half the layer's mark or more: 1.5 faded days for layer 2 and for layer 3's served days, and half the checked bytes for layer 4. Below that, the peer loses the layer. A peer that qualifies for a layer is never dropped from it in the same step.
- **What doesn't count is unchanged:** failed transfers, `Hello`, uptime, and anything unchecked or claimed. Bad data still sends a peer back to layer 0 for 30 days ("Limits", below).
- **Layer 4's bytes count the same way** (the maintainer's decision, 2026-11-09): 100 MB of bytes received from the peer and checked against their hashes in the last 30 days reach it, from daily totals, with no file named. Once reached, it's kept while the faded checked bytes stay at 50 MB or more, alongside the other layers' rules for keeping. Bytes this node didn't receive and check itself never count.
- **The rest of layer 4 is unchanged:** layer 3, 30 days since first contact, the caps, and both sides allowing it. A reference halves its 30 days and its 100 MB, and never replaces first-hand work.

### Limits

- **At most 32 earned peers at once, and 2 new ones a week.** Fewer means less exposure; more means more bytes off relays. The simulator sets these.
- **Standing fades,** by half every 14 days as in ADR 0011. A peer whose faded standing is below half of a layer's mark drops back ([above](#reaching-a-layer-and-keeping-it)), so keeping a link takes continued work. What it has already learned, it keeps.
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
- **Following a creator and hosting its posts for a month can get the creator's address,** if the creator said yes. A targeted flood is the likeliest harm, and it falls mostly on creators. It isn't cheap: the checks look like any downloader, so the follower has to keep the posts and serve the creator's other followers well for 30 days, real work for the creator's audience. A creator who'd rather not can say no at setup, or use a VPN. The price check tries it.

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
| 3 | The rule, the handover, showing a machine, spot checks of followers' copies, `deny`, `reset`, asking at setup | **At least a quarter of relayed bytes move to direct links** (a starting value). Refused nodes arriving through a relay learn no address, in a packet capture |
| 4 | Effects of layers 2 and 3, one at a time | B8's marks, and the review's A3 |

Not before the invited alpha: a handful of nodes won't reach layer 4 and have too little to exchange.

## Validation

- **A price check.** An attacker holding 10 and 25% of nodes, serving real media and carrying traffic for three simulated months, its names signing references for one another: what share of honest addresses does it hold, and how many bytes did it serve or carry to get there? It also tries tiny files, split requests, a fast host aimed at one node, and a follower hosting one creator's posts to reach it. The result is reported, and decides whether setup recommends yes.
- **Admission.** A stranger, or a node with only machine evidence such as greetings and records, only references, or self-reported bytes, never gets an address. A peer that sent bad data loses its place. After `reset`, nothing connects directly, including peers approved by hand.
- **Leak test,** as in ADR 0014 and ADR 0015. Nothing public gains an address, and nothing ties a machine name to a public ID.

**Revisit** if pairs rarely meet again, if the price check shows addresses going too cheaply, if newcomers end up with worse service than they have today, or if people ask for the separation that unbinding `WhoHosts` would give.
