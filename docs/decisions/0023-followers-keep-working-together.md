# ADR 0023: Followers keep working together

**Status:** Proposed (2026-11-08). Planning [ADR 0017](0017-one-hop-forwarding.md)'s stage A showed that, under the accepted rules, two followers of one creator almost never deal with each other on 3 different days. So they never earn a direct link, and hops can't start. This record adds a use to [ADR 0018](0018-earned-direct-connections.md)'s layer 1, and says how ADR 0017 picks a machine for the middle. Each change says what it replaces. Not built. It's measured first in ADR 0017's stage A model, beside the rules as accepted.

## Context

Kim posts a video, and her 300 followers fetch it ([doc 13](../13-hosting-roles-and-follower-swarm.md#how-a-post-spreads)):

1. Each follower's node waits a random moment in the next 10 minutes, then asks Kim's node who holds a copy (`WhoHosts`).
2. Kim's node names up to 6 followers that told it they hold one (`Hosting`) and sent it a piece it checked. It picks them at random, putting those it heard from in the last 5 minutes first. The first followers to ask find no copies and fetch from Kim.
3. Jimmy fetches from up to 3 of those named at once, and from Kim last. Then he tells Kim's node he holds a copy, and is named to others.
4. Jimmy uses the names for that download only. For Kim's next post he asks again, and gets 6 others at random.

Kim sends 20 to 25 copies, and her followers send the other 275 or so to one another ([B8](../testing/b8-creator-upload.md#results)). They work for each other all the time, but seldom for the same follower twice. Jimmy and Alice deal with each other on about 1 post in 100. With a post every 2 days, the chance that they do so on 3 different days in a month is about 1 in 2,400, and almost no pair does it both ways. Under ADR 0018:

- **Followers don't earn direct links with each other.** Kim can, through her spot checks, but only from day 30 and only 2 new a week.
- **A follower that hosts has no hop of its own,** as a host's hop must be one of its direct peers (ADR 0017). Its leg of every transfer goes through a relay.
- **A viewer's hop must have carried for it on 3 different days.** Its only way in is a middle turn, on 1 in 10 of the viewer's transfers through hops. Only Kim's transfers use hops, and the middle machine is drawn from the viewer's whole table of machines. So no machine reaches 3 days with one viewer.
- **Relay bytes fall only when both legs of a transfer have a hop,** which almost never happens. Stage A's gate can't be met, from zero standing or from the 10 nodes approved by hand.
- **[ADR 0022](0022-lists-and-introductions-are-earned.md)'s newcomer stalls for the same reason.** It earns a list and introductions by middle turns, and it seldom gets them from the same node on 3 days.

Doc 13 left this open: "The downloading node then picks sources using its peer assessment ([doc 12](../12-peer-reciprocity-and-recognition.md)); for now, in the order given." [ADR 0011](0011-measuring-peers.md)'s record counts each peer's work, but nothing uses it to choose.

## Options considered

- **Kim's node naming the same followers to Jimmy each time.** It gets pairs to meet again, but makes the creator's node the matchmaker for every pair of her followers, and Jimmy's own record plays no part. Rejected.
- **Jimmy telling Kim's node whom he'd like named.** Kim's node would learn which of her followers work with which: a map of who fetches from whom, kept by one node, which is what [the review](../reviews/2026-10-21-review-of-docs-12-and-13.md)'s A1 found receipts gave away. Rejected.
- **Asking every peer Jimmy has worked with, whatever creator they share.** A peer that doesn't follow Kim would learn that Jimmy wants her post. Rejected: partners are kept for each creator.
- **Jimmy trying first the followers he has worked with, and keeping a place for others.** First-hand, as every rule of ADRs 0016 to 0018 is. Chosen.

## Decision

### Followers fetch first from those they worked with

Once Jimmy's node has swapped a creator's media with one of her followers, in either direction, it keeps that follower as a partner for her posts, and tries it first next time.

1. Kim posts again. Jimmy's node waits its random moment, as now: that spreads the first fetches, so Kim sends 25 copies, not 300.
2. It asks its partners for Kim's posts whether they hold this one, and Kim's node who holds a copy, all at once and with a short timeout, so asking doesn't hold the download up.
3. It fetches from partners that hold it, in 2 of its 3 places at once. The third goes to a follower Kim's node named that Jimmy hasn't worked with, if there is one, so the swarm keeps mixing and new followers get picked.
4. If a partner is busy or doesn't hold the post yet, the followers Kim named take its place. If nobody holds it yet, Jimmy fetches from Kim within her upload limits, or tries again as copies spread, as now.

- **A partner** is a follower with a completed download, every chunk checked, from it or to it. When Jimmy serves Alice, Alice checks every chunk and credits Jimmy in her own record; when Alice serves Jimmy, Jimmy credits Alice in his. Neither tells the other, as no receipt leaves a node, and a direct link opens only when both records allow it (ADR 0018). Either direction makes a partner, so whoever has the post first serves the other. The order changes with each post, so the work goes both ways.
- **At most 4 partners for each creator** (a starting value), keeping those with the most work done. One that sends bad data stops being a partner, as in ADR 0018.
- **Partners fade** with ADR 0011's record, so one that stops serving drops out.
- **Kim's node recommends sources as it does now:** it names only followers whose copy it checked. Once it spot-checks followers' copies (ADR 0018), it names first those that passed on the most days, in half of the 6 places, and the rest at random, so new followers are still named. A recommendation gets Jimmy a follower to try, and changes no layer.
- **A recommendation isn't [ADR 0022](0022-lists-and-introductions-are-earned.md)'s introduction.** An introduction counts only from a machine that did work for the one receiving it on 3 different days. Alice gets most of Kim's videos from other followers, so she may never have checked 3 days of work from Kim, and wouldn't count Kim's introductions. Whether a creator's word should count for more is a decision of its own, not made here.

**What it gets Jimmy and Alice:** they swap Kim's posts again, on about 2 posts in 5 in each direction (an estimate, which the model tests). So each can reach ADR 0018's layer 2 and layer 3 with the other within about two weeks, and layer 4 once 30 days have passed, within its caps. It earns nothing on its own: layers still come only from checked work.

**What this replaces:**

- In ADR 0018's layers, layer 1 gave "On record, nothing more". It now also gives a place among those tried first for the same creator's posts. Layer 2's "Tried sooner when fetching" stands.
- In doc 13, "for now, in the order given".
- A node used the followers a creator's node named "for this download only, not remembered". Jimmy now keeps those he worked with, for his own use. He still never passes them on, as doc 13 requires.
- ADR 0011's record gains, for each peer, the followed node whose posts it was a partner for. It still holds no blobs and no times.

### Middle turns go back to machines that did one

ADR 0017 has a viewer put a machine with no standing in the middle of 1 in 10 of its transfers through hops, "one from its own table of machines". It doesn't say which.

- **Half of a node's middle turns go to machines that finished one for it before** and have no standing with it yet, those with the most finished jobs first. **The rest go to machines that haven't** (starting values).
- So a newcomer that finished one job for Alice gets more of her turns, and reaches 3 days with her, which ADR 0022's list and introduction need.

**What this adds:** the rule ADR 0017 left out. No rule is reversed.

### Why

A node should prefer machines that worked for it over strangers (the maintainer, 2026-11-08). The preference is first-hand: Jimmy tries Alice first because of downloads he checked, not because anyone said so. It turns the work followers already do for one another into the repeated work that ADR 0018's layers count.

## Consequences

- **More pairs earn direct links,** among the followers of one creator, which is where the swarm's bytes go. A follower with earned links has hops of its own as a host and as a viewer, so ADR 0017's hops can start.
- **A fast host is picked once and kept,** which is what it's for: a fast server that keeps serving well is a benefit to Kim's followers, not a threat in itself, and nothing here holds it back. What it can gain is measured. Named at random as anyone is, it becomes a partner of each follower it served, or that served it, and after a month of serving each of them well it can earn their direct links, and with them their addresses if they said yes ([ADR 0018](0018-earned-direct-connections.md#consequences)). An operator would pay for that with real upload to Kim's whole audience for a month, and get addresses that may be a VPN's, good mostly for a flood, which a new address from the ISP ends. Each follower's caps (4 partners for a creator, 2 new earned peers a week, 32 at once) bound how many places one operator gets. The model reports what it got for its upload, as ADR 0018's price check asks.
- **Load.** Partners share the work they'd share anyway. A busy one refuses as now, and Kim's names fill in. The third place, for a follower not yet worked with, keeps newcomers getting picked, and keeps the swarm from splitting into groups. The model measures both.
- **Privacy.** Jimmy asking a partner whether it holds a post tells it he wants that post now, which it could guess, as a follower of the same creator. As partners are kept for each creator, he never asks a node that doesn't follow her. Jimmy's node holds which of Kim's followers he worked with, as faded totals. Someone who copies its disk learns that, and the list of whom he follows is there already.
- **A newcomer needs no directory for its media.** Following Kim gets it her posts, the followers her node recommends, partners and, in time, earned links. A directory only starts its machine, which ADR 0015 keeps apart from whom it follows.
- **Changes to accepted records:** ADR 0018's layer 1, ADR 0011's record and doc 13, as above. ADR 0017 gains the middle rule. ADR 0022 doesn't change: its newcomer reaches 3 days of work sooner, and a creator's recommendation isn't one of its introductions.

**Licensing.** No new dependency.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | In ADR 0017's stage A model, beside the rules as accepted | [The plan](../testing/hops-model.md)'s marks 4 to 6, set before it runs: follower pairs earning links, the load on partners, and whether the swarm splits into groups; and its marks for ADR 0017, under both sets of rules |
| 2 | Partners first, in the node: the record, asking partners, the places | B8's swarm run with several posts on the simulated network: its marks, and pairs swapping again about as often as the model said |
| 3 | Middle turns, with ADR 0017's stage B | ADR 0017's |

## Validation

The gates above, and ADR 0018's price check with an operator's fast hosts among Kim's followers.

**Revisit** if the swarm splits into groups that don't reach one another, if partners are overloaded, if newcomers are picked less than before, or if an operator holds more partner places than its share of the hosting suggests.
