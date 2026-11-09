# Experiments A to C on the hops model

**Status:** Plan, fixed on 2026-11-09 before any code or run. The maintainer approved the three experiments after [the hops model](hops-model.md#results) missed most of its marks. Changing anything below later needs a dated note here saying what changed and why.

The hops model showed three things in the way of the design. Hosts rarely had a hop to pick, because a host's hop must be its direct peer. Direct links need work in both directions, but partners mostly work one way: a fast follower serving slower ones is the swarm doing its job, not failing. And newcomers had about 5 middle turns a day across the whole network to earn their first standing.

Each experiment changes one of those, as a candidate revision of [ADR 0018](../decisions/0018-earned-direct-connections.md), [ADR 0017](../decisions/0017-one-hop-forwarding.md) or [ADR 0023](../decisions/0023-followers-keep-working-together.md). None is built in the node, and meeting a mark here approves nothing: it gives the figures for a new ADR, which is then measured the same way.

## What stays the same

Everything in the [hops model's plan](hops-model.md) not named below: the two starts, the three ways, both sets of rules, the posts, the speeds, the operator, meeting with lists asked only with a reason ([second run](hops-model.md#lists-asked-only-with-a-reason)), and **every pass mark, 0 to 9, unchanged**. The second run is the baseline each experiment is compared with.

## A. One-way service toward a direct link

Today a direct link needs each side to hold the other at layer 4, and layer 3 needs work both ways. Under A, contribution and connection are counted apart:

- **Contribution, in Alice's record of Jimmy:** checked downloads Jimmy served her. Layers 1 and 2 as now. Alice's side allows a link when Jimmy served her on 3 different days in each of 2 of the last 4 weeks, with 100 MB checked in the last 30 days, and 30 days since first contact. Kept, as now, on faded standing at half the marks.
- **Client relationship, in Jimmy's record of Alice:** requests from Alice that Jimmy authenticated, on 3 different days in the last 30, and 30 days since first contact. Jimmy's side allows a link on that. Requests earn Alice no contribution standing: they count toward no layer, no list, no introduction and no hop pick.
- **A link opens** only when both sides allow it, each said yes to earned links, and each has room under ADR 0018's caps (32 at once, 2 new a week). Work both ways still counts, so a pair that swaps as now also qualifies.
- **The machine-to-node binding:** a node can prove which machine is its own with a signed binding, given to a node it served. The binding earns nothing. It replaces "once the node has shown its machine at layer 2" everywhere: a machine's checked downloads count toward lists, introductions and the viewer's hop picks from the first job, not only after the other side also served it.

## B. Work for newcomers, through the relay

A newcomer can't earn standing without jobs, and today jobs come only as middle turns on paths through hops. Under B, a viewer gives some of its relayed transfers to a machine with no standing:

- **An eligible transfer** is one that would go through the relay with no hop at either end.
- **On a share of them, fixed in advance at 5%, 10% or 20%** (three configurations), the viewer sends it through a machine from its own table that has no standing with it, says yes to carrying and has room. Both legs go through the relay, so that machine learns no end's address, and the relay's own copy of the path stays the fallback: if the machine stops or falls behind, the transfer goes on through the relay alone.
- **Each machine takes at most 2 such jobs a day** (a starting value) and turns away more.
- **Credit as now:** the viewer checks each job and credits it in its own record. Standing is jobs for the same viewer on 3 different days in the last 30.

The cost is real: each such transfer goes through the relay twice and through the newcomer's upload once. B is measured for what it costs, not only what it earns.

## C. Kim supplies the first copies

Under C, Kim's node gives each new post first to up to 4 followers:

- **Who:** followers that said yes to sharing what they hold and to earned links, picked by Kim's node as those whose copies passed her spot checks on the most days, ties at random. All followers in the model share what they hold ([ADR 0012](../decisions/0012-sharing-prefetched-copies.md)), so the yes to earned links decides.
- **What:** they're told of the post first and fetch it from Kim at once, within her upload limits (her usual number of places), and check it as any download. That's the copy each would have fetched anyway, so no transfer is added. Everyone else fetches as now.
- **Links:** none granted. Kim and her picks earn links under A's rule, or under today's, as any pair would.

## Runs

| Configuration | A | B | C |
| --- | --- | --- | --- |
| Baseline (the second run) | | | |
| A | yes | | |
| B5, B10, B20 | | 5%, 10%, 20% | |
| C | | | yes |
| ABC5, ABC10, ABC20 | yes | 5%, 10%, 20% | yes |

Each configuration runs both starts, the three ways and both sets of rules, seeds 1 to 5. Each runs once: no setting changes after its results.

## How it's judged

- **Each mark as in the plan:** judged on seed 1, with every seed's value shown, from 1 to 5, and their median, not only how many seeds meet it. For mark 9, the honest-node figure and its limit for every seed.
- **A mark met in a configuration** is reported as met there. An experiment is worth taking into a new ADR if it meets marks the baseline missed without missing any the baseline met, on seed 1 and on the median of seeds 1 to 5. Marks met on some seeds and missed on others are reported as such, with the spread.

## Recorded too

For each configuration and seed:

- **Resource costs:** bytes through the relay; home upload for each byte delivered; under B, the extra relay bytes and newcomers' upload for B's jobs; Kim's upload.
- **Fairness of distribution:** among honest followers, the share of bytes served by the 10% that served most, and the Gini of bytes served.
- **Newcomer opportunities:** B's jobs offered and taken each day; newcomers with at least one job; days from arriving to jobs on 3 days for the same viewer; jobs turned away by the 2-a-day limit, and newcomers that hit it on 3 days or more, to show whether it limits progress.
- **Delivery:** the time for 50% and 95% of followers to hold each post, against the baseline.
- **Addresses exposed:** honest nodes whose address any other machine learned, through a direct link or as the hop next to it, and honest nodes whose address the operator's machines learned.
- **Concentration of knowledge:** the operator's share of lists given, of introductions, of end hops, of the entries in honest machines' tables, and, under C, of Kim's picks.
- **A's links:** links opened on one-way service, and those that needed work both ways.

## Not modelled

As in the hops model's plan. Also: how a relay or a newcomer falling behind affects one transfer in detail (B's fallback is modelled as the transfer going on through the relay); bad bindings or forged requests, as signatures aren't modelled.
