# Sizing carriers: limits and the newcomer rate, in a model

**Status:** Run on 2026-11-20 ([results](#results)): no setting meets every mark, so nothing is set, and the figures go to the maintainer. Plan fixed on 2026-11-20, before any code or run. [ADR 0043](../decisions/0043-the-media-plane-before-the-alpha.md)'s sizing stage: carrying is built in P11 whatever this shows, so the run doesn't decide whether to build. It sets the starting values: a node's upload share for carrying, its jobs at once, its limits on circuits from one neighbour and new circuits a minute, and how often a transfer gives a newcomer a middle turn. Changing a pass mark later needs a dated note here saying what changed and why.

## The model

`osp-month` in `osp-sim`, the [hops model](hops-model.md), with a fourth way. Everything not named here is as there: Z's population (Kim, 300 followers, two groups of 50 newcomers, the operator's 30 nodes), the 45 posts of 52 MB, the speeds, the waits, meeting, lists and introductions, and three months from zero standing. ADR 0023's partners aren't accepted, so followers fetch from those Kim's node names at random (the accepted rules).

**Way 4, carriers,** as ADR 0043 amended on 2026-11-20 and its [design note](../decisions/0043-design-note-carrier-circuits.md) say:

- **No earned direct links.** ADR 0018 isn't built, so no pair connects directly. Records are still kept, as they decide standing.
- **Every node carries,** Kim's and the operator's too, within its upload share and its jobs at once, behind its own serving.
- **Seed carriers** (`carriers.txt`): always-on servers, at 200 Mbit/s each way, carrying with their whole upload, 40 jobs at once. They fetch nothing and aren't followers.
- **The host's hop** is a seed carrier with room (ADR 0043 §7: a host's hop is a direct peer or a seed, and there are no direct peers).
- **The viewer's hop** is one of the machines that finished jobs for it on 3 different days in the last 30, with room; if it has none, a seed carrier with room. If the viewer would pick the host's hop itself, the transfer takes that one hop, as in the hops model.
- **The question on a node's own address:** 3 in 4 honest nodes say yes (yes is recommended), drawn as the hops model draws its yes. A node that says no keeps its own leg on the relay. The operator says yes.
- **Middle turns:** on a share of transfers through hops (the newcomer rate), a machine with no standing with the viewer, from its own table, goes in the middle. A seed carrier is never a middle machine.
- **A leg with no hop** goes through the project's relay: each endpoint may send 4 MB/s into it after an 8 MB burst ([ADR 0044](../decisions/0044-project-relays-for-the-alpha.md) §7), and its total isn't limited. The relay is always up here, so n0's relays are never used.
- **Two ways to compare against**, with the same population, seed carriers left idle: every transfer through the project's relay, and every transfer through n0's (8 Mbit/s for each endpoint after 2.7 MB, the hops model's way 1).

**Swept:**

| Value | Runs |
| --- | --- |
| A node's upload share for carrying | 25%, 50% |
| Its jobs at once | 2, 4 |
| The newcomer rate | 5%, 10%, 20% of transfers through hops |
| Seed carriers | 2 in all of the above; 1 and 4 at 50%, 4 jobs and 10% |

That's 14 settings and the 2 to compare against, each with seeds 1 to 5. Each mark is judged on seed 1, as in the hops model, and the brackets give how many of the 5 seeds meet it.

## Pass marks

Fixed on 2026-11-20, before the model is built.

1. **Not slower than the project relay.** Over all 45 posts, way 4's time for 95% of followers is no later than the project-relay way's for at least 9 posts in 10, and no more than a fifth later for any. B8's three marks are met on at least as many posts of weeks 10 to 13 as the project-relay way meets.
2. **Off the relay.** In weeks 10 to 13, way 4 sends at most half the bytes through the relay that the project-relay way sends.
3. **Newcomers get going.** In each of Z's newcomer groups, judged apart: 9 in 10 get a middle turn within 7 days of arriving, and standing with one node (jobs for it on 3 different days in the last 30, the bar for sitting next to that viewer) within 28.
4. **The operator sees both ends no more often than its share suggests:** of the transfers with a hop at both ends, the share where it held them all is at most 2p², where p is its share of the hops next to an end.

## What it sets

- **Upload share, jobs at once and the newcomer rate:** the setting with 2 seed carriers that meets marks 1 to 4 with the smallest upload share, then the fewest jobs at once, then the lowest newcomer rate. If none meets them all, nothing is set, and the figures go to the maintainer.
- **Circuits from one neighbour, and new circuits a minute:** twice the most any carrier saw from honest use at that setting over the 5 seeds, rounded up. The model's operator behaves as an honest follower here, so its machines count as honest use. Stage C tests the limits against a machine that opens circuits as fast as it can.
- **Seed carriers:** the fewest of 1, 2 and 4 that meet marks 1 and 2 at 50%, 4 jobs and 10%. That's a count for the maintainer to plan, not a value in the software.

## Recorded too

For each setting:

- the bytes delivered through carriers and through the relay, by week;
- the busiest second through the relay, in Mbit/s, as the project relay's server has one link;
- what carrying adds to homes: the median and largest bytes a day an honest node carried, and the share of transfers that took one hop, two, or three;
- each seed carrier's bytes, and the share of transfers where one seed carrier was next to both ends: that machine sees both addresses, as the relay does today;
- the most circuits any carrier held from one neighbour at once, and the most it took in one minute;
- for newcomers, the median days to a first middle turn and to standing;
- honest nodes whose address the operator's machines learned.

## Results

`osp-month sizing` in `osp-sim`, 2026-11-20. Each mark is judged on seed 1; the brackets give how many of seeds 1 to 5 meet it. The whole sweep takes about 90 s. Unchanged ways still give the hops model's published figures.

**The ways compared against.** Through the project's relay, B8's marks are met on all 13 posts of weeks 10 to 13, with 95% of followers holding a post in a median 590 s. Through n0's, on none, in a median 1,345 s. Both send about 5,880 copies' worth through the relay in those weeks.

| Mark | Result | Seed 1 |
| --- | --- | --- |
| 1, speed | missed in every setting [0/5] | B8 met on all 13 late posts, as through the project relay, and never more than 7% later on any post (needs at most a fifth). But later on more than 1 post in 10: no later on 2 to 22 of 45 |
| 2, off the relay | 2 seed carriers: missed [3/5]; 1: missed [0/5]; 4: met [5/5] | 54 to 56% of the project relay's bytes with 2 seed carriers, 74% with 1, 42% with 4 (needs at most 50%) |
| 3, newcomers | missed in every setting [0/5] | a middle turn within 7 days: 9 to 34 of 50 directory newcomers, 0 to 2 of 50 given one machine. Standing within 28 days: 0 everywhere |
| 4, the operator | met in every setting | it never sat next to an end, as no node earned a place there |

By the plan's rule, no upload share, jobs at once or newcomer rate is set, and no count of seed carriers meets marks 1 and 2 together.

### Where it falls short

**The seed carriers are the network.** In weeks 10 to 13, 74% of bytes went through carriers (94% with 4 seed carriers), and nearly all through a seed. A host's hop must be a seed, and a viewer with no hop of its own picks a seed too, so on 97 to 100% of transfers with a hop at both ends, one seed carrier was next to both. That machine sees both ends' addresses, as the relay does. An honest home carried a median 1 MB a day, at most 22. Each of 2 seed carriers carried about 365 GB over the three months, for one creator's 430 followers.

**Nobody earns a place next to an end.** A middle machine is drawn at random from the viewer's table, so jobs for one viewer on 3 different days within 30 don't happen: no node, newcomer or not, reached standing with any viewer in any run. The hops model found the same for newcomers (its mark 7). Raising the newcomer rate gives more middle turns, not standing.

**What stays on the relay** is mostly ends that say no to the question on their own address. With 1 in 4 saying no, about 4 transfers in 9 have an end whose leg goes through the relay. The rest is seed carriers at their 40 jobs at once: 1 seed carrier leaves 74% on the relay, 4 leave 42%.

**Mark 1 is missed by small margins.** Way 4 is within 2 to 7% of the project relay on every post, and later more often than not. An extra hop doesn't make a transfer faster when the relay isn't the limit. The project relay's limits (4 MB/s for each endpoint) don't hold back homes uploading at 5 to 20 Mbit/s.

**The project relay's server.** Its busiest second carried about 960 Mbit/s with everything through it, and 700 with 2 seed carriers. That's about a 1 Gbit/s server link, for one creator with 430 followers.

**Circuits, if limits were set from these figures:** a home held at most 4 circuits from one neighbour at once and opened at most 9 in a minute, in any setting and seed. A seed carrier held at most 5 from one neighbour, and took up to 99 a minute.

## Not modelled

What the hops model leaves out, and: metered connections and the pause (nobody pauses); the project relay going down and n0 taking over; Tor's time to arrange a circuit, and a circuit's build time (a path is ready when picked); circuits reused for the next fetch.
