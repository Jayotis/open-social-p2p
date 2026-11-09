# Experiments A to C on the hops model

**Status:** Run 2026-11-09 ([results](#results)). Plan fixed on 2026-11-09 before any code or run. The maintainer approved the three experiments after [the hops model](hops-model.md#results) missed most of its marks. Changing anything below later needs a dated note here saying what changed and why.

**Clarified 2026-11-09, still before any code (the maintainer):** A's weeks are four consecutive blocks of 7 days that don't overlap, so one day can't count toward two weeks, and the run reports how often active servers miss that bar. B's baseline keeps every machine online and counts the relay's standby capacity apart from the bytes it carries, and two more configurations have 1% and 5% of B's jobs fail. B's figures also follow each newcomer's work with each viewer, as that's what earns standing. No pass mark changed.

The hops model showed three things in the way of the design. Hosts rarely had a hop to pick, because a host's hop must be its direct peer. Direct links need work in both directions, but partners mostly work one way: a fast follower serving slower ones is the swarm doing its job, not failing. And newcomers had about 5 middle turns a day across the whole network to earn their first standing.

Each experiment changes one of those, as a candidate revision of [ADR 0018](../decisions/0018-earned-direct-connections.md), [ADR 0017](../decisions/0017-one-hop-forwarding.md) or [ADR 0023](../decisions/0023-followers-keep-working-together.md). None is built in the node, and meeting a mark here approves nothing: it gives the figures for a new ADR, which is then measured the same way.

## What stays the same

Everything in the [hops model's plan](hops-model.md) not named below: the two starts, the three ways, both sets of rules, the posts, the speeds, the operator, meeting with lists asked only with a reason ([second run](hops-model.md#lists-asked-only-with-a-reason)), and **every pass mark, 0 to 9, unchanged**. The second run is the baseline each experiment is compared with.

## A. One-way service toward a direct link

Today a direct link needs each side to hold the other at layer 4, and layer 3 needs work both ways. Under A, contribution and connection are counted apart:

- **Contribution, in Alice's record of Jimmy:** checked downloads Jimmy served her. Layers 1 and 2 as now. Alice's side allows a link when Jimmy served her on 3 different days in each of at least 2 of the last 4 weeks, with 100 MB checked in the last 30 days, and 30 days since first contact. The 4 weeks are consecutive blocks of 7 days counted back from now, which don't overlap, so a day counts toward one week only. Kept, as now, on faded standing at half the marks. Kim posts every 2 days, so 3 days of service in one week may be hard to reach: the run reports how often a server that served the same node on 3 days or more in the last 30 misses this bar.
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

**Failures.** In B5, B10 and B20 every machine stays online, so the fallback is never used. The relay's standby capacity for it, what a transfer could fall back on, is counted apart from the bytes it actually carries, and never as media delivered. Two more configurations, on B10, make a fixed share of B's jobs fail, picked by the seed so a run repeats exactly: **B10-f1**, 1% of them, and **B10-f5**, 5%. A failed job stops part way through, and the rest of the transfer goes on through the relay alone. They report whether every failed transfer was completed through the relay, the bytes that cost beyond a job that didn't fail, and that the machine got no credit for a failed job. They add to the acceptance runs, and change none of their marks.

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
| B10-f1, B10-f5 | | 10%, with 1% or 5% of its jobs failing | |
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
- **Newcomer opportunities:** B's jobs offered and taken each day; newcomers with at least one job; days from arriving to jobs on 3 days for the same viewer; jobs turned away by the 2-a-day limit, and newcomers that hit it on 3 days or more, to show whether it limits progress. **Work with the same viewer:** for each newcomer, the most days of jobs it did for any one viewer in the last 30, and how its jobs spread over viewers (jobs, viewers, and pairs with 1, 2, and 3 or more days), as many jobs spread over many viewers can leave nearly every newcomer short of standing.
- **Delivery:** the time for 50% and 95% of followers to hold each post, against the baseline.
- **Addresses exposed:** honest nodes whose address any other machine learned, through a direct link or as the hop next to it, and honest nodes whose address the operator's machines learned.
- **Concentration of knowledge:** the operator's share of lists given, of introductions, of end hops, of the entries in honest machines' tables, and, under C, of Kim's picks.
- **A's links:** links opened on one-way service, and those that needed work both ways; servers active with a node (3 days or more in 30) that miss the weekly bar.
- **The relay:** bytes it carried, and, apart, its standby capacity for B's fallback.

## Results

`osp-month run --experiment <configuration>` in `osp-sim`, 2026-11-09: 660 runs, each configuration once, as planned. A run repeats exactly for its seed. The baseline is the second run, unchanged: with the experiment set to `baseline` the model gives the same figures as before.

### Every mark

Each cell is seed 1 / the median of seeds 1 to 5, then how many of the 5 seeds meet the mark. **Bold** where it's met on seed 1 or on the median. Every seed's value is in [the figures for each seed](#the-figures-for-each-seed) below.

| Mark | Case | Baseline | A | B5 | B10 | B20 | B10-f1 | B10-f5 | C | ABC5 | ABC10 | ABC20 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | Z accepted rules | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 |
| 2 | Z accepted rules | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 |
| 3 | Z accepted rules | no / no 0 | **no / met** 3 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | **no / met** 3 | **no / met** 3 | **no / met** 2 |
| 1 | Z ADR 0023 | no / no 0 | **met / met** 5 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | **no / met** 3 | **no / met** 3 | **no / met** 3 |
| 2 | Z ADR 0023 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 |
| 3 | Z ADR 0023 | **met / met** 4 | **met / no** 3 | **met / met** 3 | **met / met** 4 | **met / met** 3 | **met / met** 4 | **met / met** 2 | **met / no** 3 | **met / met** 3 | **met / met** 2 | **met / met** 3 |
| 1 | S accepted rules | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 |
| 2 | S accepted rules | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 |
| 3 | S accepted rules | no / no 0 | no / no 1 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 |
| 1 | S ADR 0023 | no / no 0 | no / no 1 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 1 | no / no 0 |
| 2 | S ADR 0023 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 |
| 3 | S ADR 0023 | no / no 1 | **met / met** 5 | no / no 1 | no / no 1 | no / no 0 | no / no 1 | no / no 1 | no / no 0 | **met / met** 3 | **met / met** 4 | **met / met** 3 |
| 4 | Z | no / no 0 | **met / met** 5 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | **met / met** 5 | **met / met** 5 | **met / met** 5 |
| 5 | Z | **met / met** 5 | **met / met** 5 | **met / met** 5 | **met / met** 5 | **met / met** 5 | **met / met** 5 | **met / met** 5 | **met / met** 5 | **met / met** 5 | **met / met** 5 | **met / met** 5 |
| 6 | Z | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 |
| 7 | Z directory newcomers | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 |
| 7 | Z given newcomers | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 |
| 8 | Z | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 | no / no 0 |
| 9 | Z accepted rules | – | **met / met** 5 | – | – | – | – | – | – | **met / met** 5 | **met / met** 5 | **met / met** 5 |
| 9 | Z ADR 0023 | no / no 3 | **met / met** 5 | **met / met** 5 | **met / met** 5 | **met / met** 5 | **no / met** 3 | **met / met** 4 | **met / met** 5 | **met / met** 5 | **met / met** 5 | **met / met** 5 |

**Worth taking into a new ADR, by the plan's rule** (meets marks the baseline missed, without missing any it met, on seed 1 and on the median):

| Configuration | Met where the baseline missed | Missed where the baseline met | By the rule |
| --- | --- | --- | --- |
| A | seed 1: 1 (Z, ADR 0023), 3 (S, ADR 0023), 4, 9 (both rule sets); median: also 3 (Z, accepted rules) | median: 3 (Z, ADR 0023) | no |
| B5, B10, B20, B10-f5 | 9 (Z, ADR 0023) | none | yes, on mark 9 alone |
| B10-f1 | median: 9 (Z, ADR 0023) | none | no: seed 1 gains nothing |
| C | 9 (Z, ADR 0023) | median: 3 (Z, ADR 0023) | no |
| ABC5, ABC10, ABC20 | seed 1: 3 (S, ADR 0023), 4, 9 (both rule sets); median: also 1 (Z, ADR 0023) and 3 (Z, accepted rules) | none | yes |

The baseline missed mark 9 narrowly (15.7% against a limit of 15.4% on seed 1), so B's and C's gain on it is small.

**No configuration meets ADR 0017's gate.** Mark 2 is missed in every case: B8's marks hold on none of the 13 posts of weeks 10 to 13, in any configuration or seed. Mark 1 is met only for Z under ADR 0023's rules, by A (5 of 5 seeds) and the ABC runs (3 of 5). Marks 6, 7 and 8 are missed everywhere.

### What each experiment did

Z, ADR 0023's rules, way 3. Seed 1, with the median of seeds 1 to 5 in brackets:

| | Baseline | A | B10 | B10-f5 | C | ABC10 |
| --- | --- | --- | --- | --- | --- | --- |
| Bytes through the relay, GB | 835 [835] | 704 [667] | 914 [878] | 909 [873] | 846 [819] | 773 [714] |
| Home upload for each byte delivered | 0.95 [0.95] | 1.23 [1.22] | 1.00 [1.04] | 0.99 [1.06] | 0.98 [1.06] | 1.26 [1.35] |
| Kim's upload, GB | 45 [46] | 41 [55] | 49 [46] | 47 [45] | 50 [49] | 48 [61] |
| Median post: 50% / 95% of followers hold it, s | 974 / 1,313 [974 / 1,313] | 875 / 1,240 [642 / 845] | 985 / 1,327 [966 / 1,310] | 993 / 1,330 [956 / 1,311] | 989 / 1,322 [964 / 1,302] | 944 / 1,292 [701 / 891] |
| Earned links opened: on one-way service / with work both ways | 0 / 90 | 220 / 62 | 0 / 89 | 0 / 85 | 0 / 88 | 202 / 59 |
| Earned links at the end: one-way / both ways | 0 / 70 | 76 / 107 | 0 / 66 | 0 / 70 | 0 / 69 | 62 / 112 |
| Servers active with a node at the end, and those missing A's weekly bar | 556, 463 | 1,036, 878 | 539, 462 | 544, 468 | 534, 459 | 972, 806 |
| Newcomers (of 100) with any job | 24 [15] | 42 [42] | 25 [25] | 30 [24] | 11 [18] | 40 [40] |
| Newcomer–viewer pairs with jobs on 1 / 2 / 3 or more days | 34 / 4 / 5 | 90 / 20 / 7 | 38 / 6 / 3 | 75 / 9 / 12 | 19 / 4 / 1 | 117 / 16 / 10 |
| Honest nodes (of 401) whose address another machine learned / the operator's | 265 / 152 | 342 / 287 | 269 / 153 | 278 / 166 | 253 / 152 | 351 / 277 |
| The operator's share of lists given / of end hops | 28% / 28% | 44% / 32% | 25% / 23% | 28% / 30% | 29% / 33% | 36% / 34% |

**A** moves bytes onto links. 220 links opened on one-way service, and the binding, which counts a machine's downloads from its first job, let more pairs reach layer 4 both ways too (107 both-way links at the end, against 70). Relayed bytes fell by a sixth on seed 1. The time for 95% of followers to hold the median post fell 6% on seed 1, and a third on the median of the seeds. Mark 4 rose from 43% to 72%. The price: the operator's machines learned 287 honest nodes' addresses, against 152, by serving them, and gave 44% of the lists, against 28%, with 7% of the machines. Under the accepted rules the operator held 63% of end hops (Z, seed 1), as its fast servers earn links first. Mark 9 still holds there, as transfers with hops at both ends are few (130).

A's weekly bar is seldom met. Of the servers active with a node at the end (3 days or more in 30), 85% miss it, as Kim posts every 2 days.

**B** gave few jobs to newcomers. B10 offered 1,286 jobs on seed 1 and 1,248 were taken, but newcomers carried 1.8 GB of their 63 GB. A viewer picks from its own table, and newcomers are a small part of any table, so 97% of the bytes went to established machines. The jobs also spread over many viewers. Of B10's newcomer–viewer pairs on seed 1, 3 had jobs on 3 days or more, so B earned almost no newcomer standing, and mark 7 stayed missed. The daily limit of 2 turned away 13 jobs on seed 1. No newcomer hit it on 3 days or more, in any configuration or seed, so the limit isn't what holds newcomers back. The cost: B10 added 63 GB through the relay on seed 1 (relayed bytes up 9%), and the relay held the same 63 GB as standby for the fallback, counted apart.

**B's failures:** B10-f1 picked 11 jobs to fail on seed 1 and 10 did (one finished first). All 10 transfers completed through the relay alone, at 13 MB beyond jobs that didn't fail. B10-f5: 64 of 65, all completed, 84 MB. A failed job earned its machine nothing (by the model's rules and its test).

**C** changed little alone. Kim's upload rose by about a tenth, the operator's nodes were 3% of her picks on seed 1 (7% over the seeds), and delivery times moved by under 1%.

**ABC** keeps A's gains on marks 4 and 9 and adds mark 1 for Z under ADR 0023's rules on the median, without losing mark 3. Its relayed bytes sit between A's and the baseline's, as B's jobs add relay bytes that A's links save.

### How the model reads the plan

Where the plan left a choice:

- **B runs in way 3 only.** Machines carry only there, so in ways 1 and 2 the B configurations are the baseline.
- **B's machine** is picked as a middle machine is, by each rule set's rule: one introduced to the viewer first, and under ADR 0023's rules half go back to machines that did one. A machine at its daily limit turns the job away, and the transfer goes through the relay alone, with no second try.
- **A machine doing B's jobs knows its viewer's machine,** not its address, as the job comes from the viewer. Jobs for one viewer on 3 days give it a reason to ask that viewer for a list.
- **Mark 7's middle turn** is read as the newcomer's first job: a middle turn, or one of B's jobs. In the baseline the two are the same. The figures for each seed also give middle turns alone.
- **A's requests** are transfers the host served, on different days. Requests refused as busy aren't counted. A side that allows a link keeps it, as now, at half the marks, faded: 3 days and 50 MB for contribution, 1.5 days for a client. An introduction halves the days since first contact and the bytes, as for layer 4.
- **A's binding** counts downloads everywhere work counted only once shown: for lists, introductions and a viewer's hop picks, and so also as standing, which keeps a machine out of the middle.
- **B's failures** stop a job at a point drawn between 10% and 90% of its part. What the source had sent to the stopped machine and that didn't arrive, up to QUIC's stream window, crosses the relay for nothing. A job picked to fail can finish first.
- **Medians** are taken figure by figure, as mark 0's note sets. For mark 3, where a mark compares two figures, the median of each seed's change would read differently. It would make A and C meet mark 3 for Z under ADR 0023's rules on the median, as on seed 1, and A would then pass the plan's rule.
- **Links** are counted by whether both sides held each other at layer 4, by today's rules, when the link was first seen.
- **Addresses** are learned on each leg not through the relay, by the machines at its two ends.
- **Kim's picks** go to the followers with the most faded days of passed spot checks, as ADR 0023's naming orders them.

**Fixed in the model before these results:** the end-of-run report judged records not brought up to date after the last introductions. The model's own check stopped one of A's runs (S, ADR 0023's rules, way 3, seed 5). Records are now updated before the report. The baseline's figures are unchanged, and every configuration was run on the fixed model.

### What follows, by the plan

- **ABC5, ABC10 and ABC20 meet the plan's rule,** as do B5, B10, B20 and B10-f5, on mark 9 alone. None meets ADR 0017's gate (marks 1 to 3 in every case), so nothing from ADR 0017 is built on these figures.
- **A is where the gains come from:** marks 1 (Z, ADR 0023's rules), 3 (S), 4 and 9. It misses the plan's rule only on mark 3's median for Z under ADR 0023's rules, which depends on how that median is read (above). Whether that reading stands is the maintainer's decision.
- **B doesn't get newcomers going.** Too few jobs reach newcomers, and those that do are spread over too many viewers to make standing. Mark 7 is missed in every configuration.
- **What A costs** is mostly addresses and lists going to whoever serves most: here, the operator. That's the price [ADR 0022](../decisions/0022-lists-and-introductions-are-earned.md) accepts for real work, but it's larger under A.

### The figures for each seed

<details>
<summary>Every mark, each configuration: seeds 1 to 5 and the median</summary>

**Mark 1, Z accepted**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | 100% | 100% | 100% | 100% | 100% | 100% | missed / missed / 0 of 5 |
| A | 98% | 97% | 96% | 100% | 93% | 97% | missed / missed / 0 of 5 |
| B5 | 105% | 105% | 105% | 104% | 105% | 105% | missed / missed / 0 of 5 |
| B10 | 109% | 109% | 109% | 109% | 109% | 109% | missed / missed / 0 of 5 |
| B20 | 117% | 119% | 117% | 117% | 118% | 117% | missed / missed / 0 of 5 |
| B10-f1 | 109% | 110% | 109% | 109% | 109% | 109% | missed / missed / 0 of 5 |
| B10-f5 | 109% | 110% | 109% | 109% | 109% | 109% | missed / missed / 0 of 5 |
| C | 100% | 100% | 100% | 100% | 100% | 100% | missed / missed / 0 of 5 |
| ABC5 | 105% | 101% | 103% | 104% | 102% | 103% | missed / missed / 0 of 5 |
| ABC10 | 109% | 106% | 104% | 108% | 106% | 106% | missed / missed / 0 of 5 |
| ABC20 | 117% | 113% | 111% | 117% | 113% | 113% | missed / missed / 0 of 5 |

**Mark 2, Z accepted**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 45/45, worst 1.00x | missed / missed / 0 of 5 |
| A | B8 0/13, no later 39/45, worst 1.06x | B8 0/13, no later 42/45, worst 1.03x | B8 0/13, no later 43/45, worst 1.02x | B8 0/13, no later 27/45, worst 1.08x | B8 3/13, no later 42/45, worst 1.03x | B8 0/13, no later 42/45, worst 1.03x | missed / missed / 0 of 5 |
| B5 | B8 0/13, no later 21/45, worst 1.08x | B8 0/13, no later 20/45, worst 1.07x | B8 0/13, no later 30/45, worst 1.09x | B8 0/13, no later 20/45, worst 1.08x | B8 0/13, no later 19/45, worst 1.06x | B8 0/13, no later 20/45, worst 1.08x | missed / missed / 0 of 5 |
| B10 | B8 0/13, no later 24/45, worst 1.07x | B8 0/13, no later 25/45, worst 1.09x | B8 0/13, no later 20/45, worst 1.05x | B8 0/13, no later 20/45, worst 1.07x | B8 0/13, no later 22/45, worst 1.05x | B8 0/13, no later 22/45, worst 1.07x | missed / missed / 0 of 5 |
| B20 | B8 0/13, no later 15/45, worst 1.11x | B8 0/13, no later 14/45, worst 1.11x | B8 0/13, no later 13/45, worst 1.09x | B8 0/13, no later 13/45, worst 1.09x | B8 0/13, no later 15/45, worst 1.06x | B8 0/13, no later 14/45, worst 1.09x | missed / missed / 0 of 5 |
| B10-f1 | B8 0/13, no later 21/45, worst 1.06x | B8 0/13, no later 19/45, worst 1.06x | B8 0/13, no later 22/45, worst 1.09x | B8 0/13, no later 20/45, worst 1.07x | B8 0/13, no later 17/45, worst 1.07x | B8 0/13, no later 20/45, worst 1.07x | missed / missed / 0 of 5 |
| B10-f5 | B8 0/13, no later 23/45, worst 1.06x | B8 0/13, no later 22/45, worst 1.07x | B8 0/13, no later 23/45, worst 1.09x | B8 0/13, no later 18/45, worst 1.11x | B8 0/13, no later 20/45, worst 1.05x | B8 0/13, no later 22/45, worst 1.07x | missed / missed / 0 of 5 |
| C | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 45/45, worst 1.00x | missed / missed / 0 of 5 |
| ABC5 | B8 0/13, no later 20/45, worst 1.09x | B8 0/13, no later 37/45, worst 1.08x | B8 0/13, no later 30/45, worst 1.10x | B8 0/13, no later 22/45, worst 1.08x | B8 1/13, no later 39/45, worst 1.07x | B8 0/13, no later 30/45, worst 1.08x | missed / missed / 0 of 5 |
| ABC10 | B8 0/13, no later 22/45, worst 1.08x | B8 0/13, no later 40/45, worst 1.07x | B8 2/13, no later 40/45, worst 1.03x | B8 0/13, no later 20/45, worst 1.05x | B8 0/13, no later 35/45, worst 1.07x | B8 0/13, no later 35/45, worst 1.07x | missed / missed / 0 of 5 |
| ABC20 | B8 0/13, no later 18/45, worst 1.07x | B8 0/13, no later 34/45, worst 1.14x | B8 0/13, no later 35/45, worst 1.05x | B8 0/13, no later 17/45, worst 1.10x | B8 1/13, no later 37/45, worst 1.08x | B8 0/13, no later 34/45, worst 1.08x | missed / missed / 0 of 5 |

**Mark 3, Z accepted**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | missed / missed / 0 of 5 |
| A | way 2 0.0->0.0%, way 3 0.0->0.1% | way 2 0.6->1.1%, way 3 0.8->1.0% | way 2 0.7->0.9%, way 3 0.5->0.7% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.5->0.8%, way 3 0.7->1.2% | way 2 0.5->0.8%, way 3 0.5->0.7% | missed / met / 3 of 5 |
| B5 | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | missed / missed / 0 of 5 |
| B10 | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | missed / missed / 0 of 5 |
| B20 | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | missed / missed / 0 of 5 |
| B10-f1 | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | missed / missed / 0 of 5 |
| B10-f5 | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | missed / missed / 0 of 5 |
| C | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.0->0.0%, way 3 0.0->0.0% | missed / missed / 0 of 5 |
| ABC5 | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.1->0.5%, way 3 0.3->0.7% | way 2 0.2->0.4%, way 3 0.0->0.2% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.2->0.5%, way 3 0.2->0.4% | way 2 0.1->0.4%, way 3 0.0->0.2% | missed / met / 3 of 5 |
| ABC10 | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.1->0.5%, way 3 0.2->0.6% | way 2 0.2->0.4%, way 3 0.3->0.4% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.2->0.5%, way 3 0.1->0.3% | way 2 0.1->0.4%, way 3 0.1->0.3% | missed / met / 3 of 5 |
| ABC20 | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.1->0.5%, way 3 0.1->0.4% | way 2 0.2->0.4%, way 3 0.1->0.1% | way 2 0.0->0.0%, way 3 0.0->0.0% | way 2 0.2->0.5%, way 3 0.1->0.1% | way 2 0.1->0.4%, way 3 0.1->0.1% | missed / met / 2 of 5 |

**Mark 1, Z partners**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | 89% | 88% | 89% | 90% | 87% | 89% | missed / missed / 0 of 5 |
| A | 73% | 60% | 59% | 74% | 54% | 60% | met / met / 5 of 5 |
| B5 | 92% | 92% | 87% | 92% | 92% | 92% | missed / missed / 0 of 5 |
| B10 | 96% | 90% | 89% | 91% | 91% | 91% | missed / missed / 0 of 5 |
| B20 | 96% | 91% | 85% | 98% | 89% | 91% | missed / missed / 0 of 5 |
| B10-f1 | 96% | 90% | 92% | 94% | 90% | 92% | missed / missed / 0 of 5 |
| B10-f5 | 95% | 89% | 86% | 94% | 90% | 90% | missed / missed / 0 of 5 |
| C | 92% | 86% | 84% | 86% | 86% | 86% | missed / missed / 0 of 5 |
| ABC5 | 77% | 68% | 62% | 77% | 72% | 72% | missed / met / 3 of 5 |
| ABC10 | 80% | 73% | 67% | 82% | 65% | 73% | missed / met / 3 of 5 |
| ABC20 | 82% | 72% | 69% | 80% | 68% | 72% | missed / met / 3 of 5 |

**Mark 2, Z partners**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 43/45, worst 1.02x | B8 0/13, no later 43/45, worst 1.01x | B8 0/13, no later 43/45, worst 1.01x | B8 0/13, no later 42/45, worst 1.01x | B8 0/13, no later 43/45, worst 1.01x | missed / missed / 0 of 5 |
| A | B8 0/13, no later 38/45, worst 1.05x | B8 13/13, no later 37/45, worst 1.08x | B8 13/13, no later 34/45, worst 1.06x | B8 0/13, no later 36/45, worst 1.07x | B8 13/13, no later 37/45, worst 1.05x | B8 13/13, no later 37/45, worst 1.06x | missed / missed / 0 of 5 |
| B5 | B8 0/13, no later 34/45, worst 1.05x | B8 0/13, no later 32/45, worst 1.04x | B8 0/13, no later 37/45, worst 1.06x | B8 0/13, no later 31/45, worst 1.03x | B8 0/13, no later 35/45, worst 1.06x | B8 0/13, no later 34/45, worst 1.05x | missed / missed / 0 of 5 |
| B10 | B8 0/13, no later 30/45, worst 1.06x | B8 0/13, no later 32/45, worst 1.07x | B8 0/13, no later 40/45, worst 1.05x | B8 0/13, no later 31/45, worst 1.07x | B8 0/13, no later 35/45, worst 1.08x | B8 0/13, no later 32/45, worst 1.07x | missed / missed / 0 of 5 |
| B20 | B8 0/13, no later 30/45, worst 1.07x | B8 0/13, no later 31/45, worst 1.08x | B8 0/13, no later 34/45, worst 1.08x | B8 0/13, no later 28/45, worst 1.05x | B8 0/13, no later 34/45, worst 1.05x | B8 0/13, no later 31/45, worst 1.07x | missed / missed / 0 of 5 |
| B10-f1 | B8 0/13, no later 29/45, worst 1.09x | B8 0/13, no later 39/45, worst 1.06x | B8 0/13, no later 33/45, worst 1.05x | B8 0/13, no later 36/45, worst 1.07x | B8 0/13, no later 34/45, worst 1.08x | B8 0/13, no later 34/45, worst 1.07x | missed / missed / 0 of 5 |
| B10-f5 | B8 0/13, no later 30/45, worst 1.08x | B8 0/13, no later 33/45, worst 1.07x | B8 0/13, no later 35/45, worst 1.08x | B8 0/13, no later 36/45, worst 1.06x | B8 0/13, no later 36/45, worst 1.08x | B8 0/13, no later 35/45, worst 1.08x | missed / missed / 0 of 5 |
| C | B8 0/13, no later 42/45, worst 1.02x | B8 0/13, no later 44/45, worst 1.04x | B8 0/13, no later 45/45, worst 1.00x | B8 0/13, no later 43/45, worst 1.03x | B8 0/13, no later 41/45, worst 1.04x | B8 0/13, no later 43/45, worst 1.03x | missed / missed / 0 of 5 |
| ABC5 | B8 0/13, no later 33/45, worst 1.07x | B8 11/13, no later 40/45, worst 1.07x | B8 13/13, no later 38/45, worst 1.11x | B8 0/13, no later 31/45, worst 1.08x | B8 12/13, no later 37/45, worst 1.05x | B8 11/13, no later 37/45, worst 1.07x | missed / missed / 0 of 5 |
| ABC10 | B8 0/13, no later 31/45, worst 1.06x | B8 12/13, no later 35/45, worst 1.07x | B8 11/13, no later 36/45, worst 1.07x | B8 0/13, no later 30/45, worst 1.06x | B8 13/13, no later 39/45, worst 1.09x | B8 11/13, no later 35/45, worst 1.07x | missed / missed / 0 of 5 |
| ABC20 | B8 0/13, no later 31/45, worst 1.07x | B8 13/13, no later 35/45, worst 1.10x | B8 12/13, no later 34/45, worst 1.09x | B8 0/13, no later 33/45, worst 1.08x | B8 13/13, no later 34/45, worst 1.05x | B8 12/13, no later 34/45, worst 1.08x | missed / missed / 0 of 5 |

**Mark 3, Z partners**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | way 2 4.5->5.1%, way 3 5.3->6.3% | way 2 5.4->6.3%, way 3 6.3->6.4% | way 2 5.7->5.8%, way 3 5.7->6.7% | way 2 5.0->5.2%, way 3 4.9->6.2% | way 2 6.2->5.7%, way 3 5.8->5.8% | way 2 5.4->5.7%, way 3 5.7->6.3% | met / met / 4 of 5 |
| A | way 2 12.3->14.5%, way 3 8.3->9.6% | way 2 8.8->7.5%, way 3 6.2->7.5% | way 2 8.1->8.7%, way 3 7.0->8.2% | way 2 9.6->12.7%, way 3 8.6->9.7% | way 2 9.9->8.8%, way 3 6.2->9.8% | way 2 9.6->8.8%, way 3 7.0->9.6% | met / missed / 3 of 5 |
| B5 | way 2 4.5->5.1%, way 3 4.8->5.8% | way 2 5.4->6.3%, way 3 6.1->7.2% | way 2 5.7->5.8%, way 3 6.0->5.0% | way 2 5.0->5.2%, way 3 4.3->4.9% | way 2 6.2->5.7%, way 3 5.7->5.7% | way 2 5.4->5.7%, way 3 5.7->5.7% | met / met / 3 of 5 |
| B10 | way 2 4.5->5.1%, way 3 4.3->5.7% | way 2 5.4->6.3%, way 3 6.5->7.0% | way 2 5.7->5.8%, way 3 5.4->6.6% | way 2 5.0->5.2%, way 3 4.3->5.4% | way 2 6.2->5.7%, way 3 5.5->6.1% | way 2 5.4->5.7%, way 3 5.4->6.1% | met / met / 4 of 5 |
| B20 | way 2 4.5->5.1%, way 3 4.7->5.0% | way 2 5.4->6.3%, way 3 5.5->6.0% | way 2 5.7->5.8%, way 3 5.9->5.4% | way 2 5.0->5.2%, way 3 3.7->5.0% | way 2 6.2->5.7%, way 3 4.7->5.2% | way 2 5.4->5.7%, way 3 4.7->5.2% | met / met / 3 of 5 |
| B10-f1 | way 2 4.5->5.1%, way 3 4.0->5.0% | way 2 5.4->6.3%, way 3 6.2->6.8% | way 2 5.7->5.8%, way 3 4.7->6.1% | way 2 5.0->5.2%, way 3 4.0->4.5% | way 2 6.2->5.7%, way 3 5.2->5.5% | way 2 5.4->5.7%, way 3 4.7->5.5% | met / met / 4 of 5 |
| B10-f5 | way 2 4.5->5.1%, way 3 4.3->5.0% | way 2 5.4->6.3%, way 3 6.5->6.9% | way 2 5.7->5.8%, way 3 6.3->5.8% | way 2 5.0->5.2%, way 3 4.5->4.0% | way 2 6.2->5.7%, way 3 5.0->5.6% | way 2 5.4->5.7%, way 3 5.0->5.6% | met / met / 2 of 5 |
| C | way 2 4.7->5.7%, way 3 5.7->6.2% | way 2 6.0->6.2%, way 3 6.9->7.3% | way 2 6.7->6.5%, way 3 6.6->7.4% | way 2 4.3->4.6%, way 3 5.3->6.4% | way 2 6.7->5.6%, way 3 6.1->6.7% | way 2 6.0->5.7%, way 3 6.1->6.7% | met / missed / 3 of 5 |
| ABC5 | way 2 9.5->12.6%, way 3 8.6->9.9% | way 2 12.4->12.7%, way 3 5.2->5.6% | way 2 9.5->9.3%, way 3 7.6->8.2% | way 2 10.0->10.8%, way 3 8.0->9.8% | way 2 11.0->10.2%, way 3 4.9->5.0% | way 2 10.0->10.8%, way 3 7.6->8.2% | met / met / 3 of 5 |
| ABC10 | way 2 9.5->12.6%, way 3 8.6->10.1% | way 2 12.4->12.7%, way 3 6.1->5.2% | way 2 9.5->9.3%, way 3 6.2->6.3% | way 2 10.0->10.8%, way 3 8.5->10.4% | way 2 11.0->10.2%, way 3 4.8->6.8% | way 2 10.0->10.8%, way 3 6.2->6.8% | met / met / 2 of 5 |
| ABC20 | way 2 9.5->12.6%, way 3 8.7->11.5% | way 2 12.4->12.7%, way 3 5.2->6.3% | way 2 9.5->9.3%, way 3 5.3->6.2% | way 2 10.0->10.8%, way 3 7.2->8.0% | way 2 11.0->10.2%, way 3 4.9->6.9% | way 2 10.0->10.8%, way 3 5.3->6.9% | met / met / 3 of 5 |

**Mark 1, S accepted**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | 100% | 100% | 100% | 100% | 100% | 100% | missed / missed / 0 of 5 |
| A | 100% | 93% | 91% | 100% | 96% | 96% | missed / missed / 0 of 5 |
| B5 | 104% | 104% | 105% | 106% | 105% | 105% | missed / missed / 0 of 5 |
| B10 | 109% | 109% | 110% | 110% | 110% | 110% | missed / missed / 0 of 5 |
| B20 | 117% | 117% | 120% | 118% | 118% | 118% | missed / missed / 0 of 5 |
| B10-f1 | 109% | 109% | 111% | 110% | 110% | 110% | missed / missed / 0 of 5 |
| B10-f5 | 108% | 109% | 110% | 110% | 110% | 110% | missed / missed / 0 of 5 |
| C | 100% | 100% | 100% | 100% | 100% | 100% | missed / missed / 0 of 5 |
| ABC5 | 103% | 100% | 91% | 105% | 98% | 100% | missed / missed / 0 of 5 |
| ABC10 | 103% | 98% | 94% | 106% | 103% | 103% | missed / missed / 0 of 5 |
| ABC20 | 113% | 104% | 104% | 117% | 105% | 105% | missed / missed / 0 of 5 |

**Mark 2, S accepted**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | B8 0/13, no later 23/45, worst 1.25x | B8 0/13, no later 23/45, worst 1.35x | B8 0/13, no later 18/45, worst 1.23x | B8 0/13, no later 21/45, worst 1.30x | B8 0/13, no later 24/45, worst 1.34x | B8 0/13, no later 23/45, worst 1.30x | missed / missed / 0 of 5 |
| A | B8 0/13, no later 21/45, worst 1.22x | B8 0/13, no later 40/45, worst 1.08x | B8 0/13, no later 39/45, worst 1.15x | B8 0/13, no later 21/45, worst 1.29x | B8 0/13, no later 41/45, worst 1.08x | B8 0/13, no later 39/45, worst 1.15x | missed / missed / 0 of 5 |
| B5 | B8 0/13, no later 20/45, worst 1.25x | B8 0/13, no later 23/45, worst 1.32x | B8 0/13, no later 22/45, worst 1.23x | B8 0/13, no later 16/45, worst 1.31x | B8 0/13, no later 22/45, worst 1.32x | B8 0/13, no later 22/45, worst 1.31x | missed / missed / 0 of 5 |
| B10 | B8 0/13, no later 15/45, worst 1.40x | B8 0/13, no later 18/45, worst 1.35x | B8 0/13, no later 17/45, worst 1.27x | B8 0/13, no later 15/45, worst 1.31x | B8 0/13, no later 17/45, worst 1.21x | B8 0/13, no later 17/45, worst 1.31x | missed / missed / 0 of 5 |
| B20 | B8 0/13, no later 18/45, worst 1.25x | B8 0/13, no later 14/45, worst 1.24x | B8 0/13, no later 19/45, worst 1.30x | B8 0/13, no later 18/45, worst 1.23x | B8 0/13, no later 13/45, worst 1.28x | B8 0/13, no later 18/45, worst 1.25x | missed / missed / 0 of 5 |
| B10-f1 | B8 0/13, no later 15/45, worst 1.40x | B8 0/13, no later 17/45, worst 1.42x | B8 0/13, no later 18/45, worst 1.16x | B8 0/13, no later 15/45, worst 1.31x | B8 0/13, no later 20/45, worst 1.22x | B8 0/13, no later 17/45, worst 1.31x | missed / missed / 0 of 5 |
| B10-f5 | B8 0/13, no later 24/45, worst 1.25x | B8 0/13, no later 16/45, worst 1.31x | B8 0/13, no later 20/45, worst 1.23x | B8 0/13, no later 20/45, worst 1.35x | B8 0/13, no later 18/45, worst 1.35x | B8 0/13, no later 20/45, worst 1.31x | missed / missed / 0 of 5 |
| C | B8 0/13, no later 33/45, worst 1.35x | B8 0/13, no later 19/45, worst 1.40x | B8 0/13, no later 25/45, worst 1.27x | B8 0/13, no later 24/45, worst 1.12x | B8 0/13, no later 25/45, worst 1.27x | B8 0/13, no later 25/45, worst 1.27x | missed / missed / 0 of 5 |
| ABC5 | B8 0/13, no later 28/45, worst 1.30x | B8 0/13, no later 36/45, worst 1.40x | B8 0/13, no later 37/45, worst 1.19x | B8 0/13, no later 25/45, worst 1.19x | B8 0/13, no later 39/45, worst 1.15x | B8 0/13, no later 36/45, worst 1.19x | missed / missed / 0 of 5 |
| ABC10 | B8 0/13, no later 26/45, worst 1.26x | B8 0/13, no later 36/45, worst 1.24x | B8 0/13, no later 39/45, worst 1.11x | B8 0/13, no later 19/45, worst 1.23x | B8 0/13, no later 37/45, worst 1.17x | B8 0/13, no later 36/45, worst 1.23x | missed / missed / 0 of 5 |
| ABC20 | B8 0/13, no later 17/45, worst 1.36x | B8 0/13, no later 37/45, worst 1.22x | B8 0/13, no later 36/45, worst 1.10x | B8 0/13, no later 20/45, worst 1.26x | B8 0/13, no later 40/45, worst 1.17x | B8 0/13, no later 36/45, worst 1.22x | missed / missed / 0 of 5 |

**Mark 3, S accepted**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | way 2 0.8->0.0%, way 3 0.4->0.0% | way 2 1.3->0.0%, way 3 0.7->0.0% | way 2 0.4->0.0%, way 3 0.4->0.0% | way 2 0.3->0.0%, way 3 0.5->0.0% | way 2 0.7->0.0%, way 3 0.6->0.0% | way 2 0.7->0.0%, way 3 0.5->0.0% | missed / missed / 0 of 5 |
| A | way 2 0.8->0.0%, way 3 0.7->0.0% | way 2 2.3->1.1%, way 3 1.5->1.8% | way 2 1.0->1.2%, way 3 1.0->1.4% | way 2 0.3->0.0%, way 3 0.4->0.0% | way 2 1.5->1.1%, way 3 1.6->0.7% | way 2 1.0->1.1%, way 3 1.0->0.7% | missed / missed / 1 of 5 |
| B5 | way 2 0.8->0.0%, way 3 0.5->0.0% | way 2 1.3->0.0%, way 3 0.2->0.0% | way 2 0.4->0.0%, way 3 0.3->0.0% | way 2 0.3->0.0%, way 3 0.2->0.0% | way 2 0.7->0.0%, way 3 0.7->0.0% | way 2 0.7->0.0%, way 3 0.3->0.0% | missed / missed / 0 of 5 |
| B10 | way 2 0.8->0.0%, way 3 0.5->0.0% | way 2 1.3->0.0%, way 3 0.7->0.0% | way 2 0.4->0.0%, way 3 0.3->0.0% | way 2 0.3->0.0%, way 3 0.7->0.0% | way 2 0.7->0.0%, way 3 0.5->0.0% | way 2 0.7->0.0%, way 3 0.5->0.0% | missed / missed / 0 of 5 |
| B20 | way 2 0.8->0.0%, way 3 0.6->0.0% | way 2 1.3->0.0%, way 3 0.9->0.0% | way 2 0.4->0.0%, way 3 0.4->0.0% | way 2 0.3->0.0%, way 3 0.4->0.0% | way 2 0.7->0.0%, way 3 0.7->0.0% | way 2 0.7->0.0%, way 3 0.6->0.0% | missed / missed / 0 of 5 |
| B10-f1 | way 2 0.8->0.0%, way 3 0.5->0.0% | way 2 1.3->0.0%, way 3 0.7->0.0% | way 2 0.4->0.0%, way 3 0.3->0.0% | way 2 0.3->0.0%, way 3 0.7->0.0% | way 2 0.7->0.0%, way 3 0.6->0.0% | way 2 0.7->0.0%, way 3 0.6->0.0% | missed / missed / 0 of 5 |
| B10-f5 | way 2 0.8->0.0%, way 3 0.7->0.0% | way 2 1.3->0.0%, way 3 0.7->0.0% | way 2 0.4->0.0%, way 3 0.5->0.0% | way 2 0.3->0.0%, way 3 0.7->0.0% | way 2 0.7->0.0%, way 3 0.5->0.0% | way 2 0.7->0.0%, way 3 0.7->0.0% | missed / missed / 0 of 5 |
| C | way 2 1.1->0.0%, way 3 1.0->0.0% | way 2 1.0->0.0%, way 3 1.0->0.0% | way 2 1.5->0.0%, way 3 0.9->0.0% | way 2 1.5->0.0%, way 3 1.1->0.0% | way 2 0.9->0.0%, way 3 0.9->0.0% | way 2 1.1->0.0%, way 3 1.0->0.0% | missed / missed / 0 of 5 |
| ABC5 | way 2 1.1->0.3%, way 3 0.9->0.2% | way 2 1.8->1.5%, way 3 2.8->0.3% | way 2 4.1->1.0%, way 3 3.2->1.4% | way 2 1.5->0.2%, way 3 1.2->0.0% | way 2 2.4->1.5%, way 3 1.8->1.2% | way 2 1.8->1.0%, way 3 1.8->0.3% | missed / missed / 0 of 5 |
| ABC10 | way 2 1.1->0.3%, way 3 1.2->0.4% | way 2 1.8->1.5%, way 3 2.9->1.8% | way 2 4.1->1.0%, way 3 2.6->2.2% | way 2 1.5->0.2%, way 3 1.0->0.2% | way 2 2.4->1.5%, way 3 2.7->1.1% | way 2 1.8->1.0%, way 3 2.6->1.1% | missed / missed / 0 of 5 |
| ABC20 | way 2 1.1->0.3%, way 3 0.5->0.2% | way 2 1.8->1.5%, way 3 2.7->2.2% | way 2 4.1->1.0%, way 3 3.0->0.5% | way 2 1.5->0.2%, way 3 0.8->0.0% | way 2 2.4->1.5%, way 3 2.4->1.6% | way 2 1.8->1.0%, way 3 2.4->0.5% | missed / missed / 0 of 5 |

**Mark 1, S partners**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | 100% | 99% | 99% | 100% | 98% | 99% | missed / missed / 0 of 5 |
| A | 93% | 77% | 71% | 91% | 82% | 82% | missed / missed / 1 of 5 |
| B5 | 103% | 101% | 105% | 106% | 103% | 103% | missed / missed / 0 of 5 |
| B10 | 105% | 104% | 104% | 107% | 106% | 105% | missed / missed / 0 of 5 |
| B20 | 111% | 109% | 112% | 112% | 106% | 111% | missed / missed / 0 of 5 |
| B10-f1 | 105% | 105% | 107% | 107% | 106% | 106% | missed / missed / 0 of 5 |
| B10-f5 | 106% | 106% | 107% | 107% | 107% | 107% | missed / missed / 0 of 5 |
| C | 100% | 97% | 100% | 100% | 100% | 100% | missed / missed / 0 of 5 |
| ABC5 | 93% | 81% | 82% | 93% | 88% | 88% | missed / missed / 0 of 5 |
| ABC10 | 90% | 87% | 71% | 98% | 81% | 87% | missed / missed / 1 of 5 |
| ABC20 | 96% | 90% | 78% | 105% | 80% | 90% | missed / missed / 0 of 5 |

**Mark 2, S partners**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | B8 0/13, no later 22/45, worst 1.34x | B8 0/13, no later 22/45, worst 1.37x | B8 0/13, no later 22/45, worst 1.22x | B8 0/13, no later 23/45, worst 1.21x | B8 0/13, no later 27/45, worst 1.27x | B8 0/13, no later 22/45, worst 1.27x | missed / missed / 0 of 5 |
| A | B8 0/13, no later 24/45, worst 1.22x | B8 0/13, no later 38/45, worst 1.25x | B8 1/13, no later 38/45, worst 1.13x | B8 0/13, no later 29/45, worst 1.23x | B8 0/13, no later 40/45, worst 1.14x | B8 0/13, no later 38/45, worst 1.22x | missed / missed / 0 of 5 |
| B5 | B8 0/13, no later 20/45, worst 1.29x | B8 0/13, no later 20/45, worst 1.26x | B8 0/13, no later 22/45, worst 1.25x | B8 0/13, no later 23/45, worst 1.14x | B8 0/13, no later 23/45, worst 1.27x | B8 0/13, no later 22/45, worst 1.26x | missed / missed / 0 of 5 |
| B10 | B8 0/13, no later 23/45, worst 1.31x | B8 0/13, no later 17/45, worst 1.26x | B8 0/13, no later 19/45, worst 1.22x | B8 0/13, no later 19/45, worst 1.20x | B8 0/13, no later 20/45, worst 1.35x | B8 0/13, no later 19/45, worst 1.26x | missed / missed / 0 of 5 |
| B20 | B8 0/13, no later 23/45, worst 1.23x | B8 0/13, no later 21/45, worst 1.22x | B8 0/13, no later 20/45, worst 1.36x | B8 0/13, no later 16/45, worst 1.25x | B8 0/13, no later 20/45, worst 1.31x | B8 0/13, no later 20/45, worst 1.25x | missed / missed / 0 of 5 |
| B10-f1 | B8 0/13, no later 23/45, worst 1.31x | B8 0/13, no later 21/45, worst 1.29x | B8 0/13, no later 21/45, worst 1.23x | B8 0/13, no later 20/45, worst 1.20x | B8 0/13, no later 21/45, worst 1.35x | B8 0/13, no later 21/45, worst 1.29x | missed / missed / 0 of 5 |
| B10-f5 | B8 0/13, no later 18/45, worst 1.31x | B8 0/13, no later 21/45, worst 1.33x | B8 0/13, no later 18/45, worst 1.23x | B8 0/13, no later 23/45, worst 1.26x | B8 0/13, no later 24/45, worst 1.35x | B8 0/13, no later 21/45, worst 1.31x | missed / missed / 0 of 5 |
| C | B8 0/13, no later 23/45, worst 1.21x | B8 0/13, no later 20/45, worst 1.19x | B8 0/13, no later 21/45, worst 1.39x | B8 0/13, no later 23/45, worst 1.22x | B8 0/13, no later 29/45, worst 1.36x | B8 0/13, no later 23/45, worst 1.22x | missed / missed / 0 of 5 |
| ABC5 | B8 0/13, no later 27/45, worst 1.15x | B8 0/13, no later 39/45, worst 1.21x | B8 0/13, no later 36/45, worst 1.23x | B8 0/13, no later 17/45, worst 1.24x | B8 0/13, no later 35/45, worst 1.08x | B8 0/13, no later 35/45, worst 1.21x | missed / missed / 0 of 5 |
| ABC10 | B8 0/13, no later 23/45, worst 1.23x | B8 0/13, no later 40/45, worst 1.14x | B8 1/13, no later 38/45, worst 1.21x | B8 0/13, no later 20/45, worst 1.38x | B8 0/13, no later 35/45, worst 1.22x | B8 0/13, no later 35/45, worst 1.22x | missed / missed / 0 of 5 |
| ABC20 | B8 0/13, no later 23/45, worst 1.23x | B8 0/13, no later 40/45, worst 1.37x | B8 0/13, no later 36/45, worst 1.27x | B8 0/13, no later 23/45, worst 1.26x | B8 0/13, no later 39/45, worst 1.16x | B8 0/13, no later 36/45, worst 1.26x | missed / missed / 0 of 5 |

**Mark 3, S partners**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | way 2 0.8->0.2%, way 3 0.6->0.1% | way 2 1.3->0.6%, way 3 1.0->1.1% | way 2 0.7->1.1%, way 3 0.8->1.4% | way 2 1.2->0.3%, way 3 1.4->0.4% | way 2 1.2->0.2%, way 3 1.1->1.6% | way 2 1.2->0.3%, way 3 1.0->1.1% | missed / missed / 1 of 5 |
| A | way 2 1.0->2.8%, way 3 0.9->3.0% | way 2 2.1->2.4%, way 3 1.6->3.7% | way 2 1.0->4.5%, way 3 1.9->6.2% | way 2 1.6->3.0%, way 3 1.2->3.6% | way 2 2.3->2.5%, way 3 2.4->3.1% | way 2 1.6->2.8%, way 3 1.6->3.6% | met / met / 5 of 5 |
| B5 | way 2 0.8->0.2%, way 3 1.3->0.7% | way 2 1.3->0.6%, way 3 1.9->1.3% | way 2 0.7->1.1%, way 3 0.4->0.8% | way 2 1.2->0.3%, way 3 0.7->0.3% | way 2 1.2->0.2%, way 3 1.2->1.4% | way 2 1.2->0.3%, way 3 1.2->0.8% | missed / missed / 1 of 5 |
| B10 | way 2 0.8->0.2%, way 3 0.6->1.0% | way 2 1.3->0.6%, way 3 1.1->1.5% | way 2 0.7->1.1%, way 3 0.7->2.0% | way 2 1.2->0.3%, way 3 0.9->0.5% | way 2 1.2->0.2%, way 3 0.6->1.2% | way 2 1.2->0.3%, way 3 0.7->1.2% | missed / missed / 1 of 5 |
| B20 | way 2 0.8->0.2%, way 3 0.5->1.2% | way 2 1.3->0.6%, way 3 1.2->1.3% | way 2 0.7->1.1%, way 3 0.8->0.5% | way 2 1.2->0.3%, way 3 0.6->0.2% | way 2 1.2->0.2%, way 3 0.6->1.3% | way 2 1.2->0.3%, way 3 0.6->1.2% | missed / missed / 0 of 5 |
| B10-f1 | way 2 0.8->0.2%, way 3 0.6->1.0% | way 2 1.3->0.6%, way 3 1.1->1.0% | way 2 0.7->1.1%, way 3 0.8->1.5% | way 2 1.2->0.3%, way 3 0.9->0.8% | way 2 1.2->0.2%, way 3 0.6->1.2% | way 2 1.2->0.3%, way 3 0.8->1.0% | missed / missed / 1 of 5 |
| B10-f5 | way 2 0.8->0.2%, way 3 0.5->0.8% | way 2 1.3->0.6%, way 3 1.1->0.9% | way 2 0.7->1.1%, way 3 0.7->1.1% | way 2 1.2->0.3%, way 3 0.8->1.0% | way 2 1.2->0.2%, way 3 0.5->1.0% | way 2 1.2->0.3%, way 3 0.7->1.0% | missed / missed / 1 of 5 |
| C | way 2 1.7->0.2%, way 3 1.7->0.6% | way 2 1.5->0.5%, way 3 1.4->2.7% | way 2 2.3->0.8%, way 3 1.7->0.6% | way 2 1.6->0.3%, way 3 2.1->0.5% | way 2 1.1->0.7%, way 3 1.6->0.9% | way 2 1.6->0.5%, way 3 1.7->0.6% | missed / missed / 0 of 5 |
| ABC5 | way 2 2.0->2.8%, way 3 1.6->3.9% | way 2 3.2->4.7%, way 3 2.4->3.8% | way 2 4.2->2.3%, way 3 2.5->4.1% | way 2 1.8->2.9%, way 3 2.8->2.1% | way 2 1.9->3.7%, way 3 2.0->2.8% | way 2 2.0->2.9%, way 3 2.4->3.8% | met / met / 3 of 5 |
| ABC10 | way 2 2.0->2.8%, way 3 2.4->4.2% | way 2 3.2->4.7%, way 3 3.1->3.6% | way 2 4.2->2.3%, way 3 3.9->4.6% | way 2 1.8->2.9%, way 3 1.5->2.7% | way 2 1.9->3.7%, way 3 2.0->3.2% | way 2 2.0->2.9%, way 3 2.4->3.6% | met / met / 4 of 5 |
| ABC20 | way 2 2.0->2.8%, way 3 2.1->2.3% | way 2 3.2->4.7%, way 3 3.9->3.0% | way 2 4.2->2.3%, way 3 2.6->4.5% | way 2 1.8->2.9%, way 3 1.6->1.9% | way 2 1.9->3.7%, way 3 2.8->4.3% | way 2 2.0->2.9%, way 3 2.6->3.0% | met / met / 3 of 5 |

**Mark 4, Z**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | 43% | 43% | 43% | 39% | 39% | 43% | missed / missed / 0 of 5 |
| A | 72% | 72% | 71% | 72% | 72% | 72% | met / met / 5 of 5 |
| B5 | 40% | 41% | 39% | 39% | 40% | 40% | missed / missed / 0 of 5 |
| B10 | 38% | 46% | 40% | 43% | 40% | 40% | missed / missed / 0 of 5 |
| B20 | 36% | 39% | 44% | 40% | 34% | 39% | missed / missed / 0 of 5 |
| B10-f1 | 35% | 43% | 37% | 35% | 38% | 37% | missed / missed / 0 of 5 |
| B10-f5 | 37% | 43% | 48% | 37% | 37% | 37% | missed / missed / 0 of 5 |
| C | 39% | 50% | 47% | 41% | 44% | 44% | missed / missed / 0 of 5 |
| ABC5 | 75% | 64% | 77% | 75% | 64% | 75% | met / met / 5 of 5 |
| ABC10 | 74% | 77% | 72% | 74% | 63% | 74% | met / met / 5 of 5 |
| ABC20 | 77% | 66% | 69% | 73% | 60% | 69% | met / met / 5 of 5 |

**Mark 5, Z**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | worst week 1.18x | worst week 1.23x | worst week 1.19x | worst week 1.22x | worst week 1.18x | worst week 1.19x | met / met / 5 of 5 |
| A | worst week 1.19x | worst week 1.22x | worst week 1.21x | worst week 1.19x | worst week 1.19x | worst week 1.19x | met / met / 5 of 5 |
| B5 | worst week 1.19x | worst week 1.16x | worst week 1.16x | worst week 1.18x | worst week 1.17x | worst week 1.17x | met / met / 5 of 5 |
| B10 | worst week 1.15x | worst week 1.19x | worst week 1.21x | worst week 1.18x | worst week 1.17x | worst week 1.18x | met / met / 5 of 5 |
| B20 | worst week 1.17x | worst week 1.21x | worst week 1.17x | worst week 1.16x | worst week 1.18x | worst week 1.17x | met / met / 5 of 5 |
| B10-f1 | worst week 1.15x | worst week 1.21x | worst week 1.18x | worst week 1.18x | worst week 1.19x | worst week 1.18x | met / met / 5 of 5 |
| B10-f5 | worst week 1.18x | worst week 1.21x | worst week 1.21x | worst week 1.19x | worst week 1.18x | worst week 1.19x | met / met / 5 of 5 |
| C | worst week 1.18x | worst week 1.16x | worst week 1.19x | worst week 1.19x | worst week 1.18x | worst week 1.18x | met / met / 5 of 5 |
| ABC5 | worst week 1.22x | worst week 1.17x | worst week 1.18x | worst week 1.19x | worst week 1.17x | worst week 1.18x | met / met / 5 of 5 |
| ABC10 | worst week 1.19x | worst week 1.17x | worst week 1.21x | worst week 1.22x | worst week 1.20x | worst week 1.20x | met / met / 5 of 5 |
| ABC20 | worst week 1.18x | worst week 1.19x | worst week 1.18x | worst week 1.19x | worst week 1.20x | worst week 1.19x | met / met / 5 of 5 |

**Mark 6, Z**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | group 100%, first copy served 13.7 d vs 3.2 d | group 100%, first copy served 18.2 d vs 3.3 d | group 100%, first copy served 13.0 d vs 3.0 d | group 100%, first copy served 12.9 d vs 2.4 d | group 100%, first copy served 17.3 d vs 2.7 d | group 100%, first copy served 13.7 d vs 3.0 d | missed / missed / 0 of 5 |
| A | group 100%, first copy served 13.7 d vs 3.7 d | group 100%, first copy served 16.0 d vs 6.4 d | group 100%, first copy served 15.7 d vs 7.0 d | group 100%, first copy served 15.3 d vs 3.2 d | group 100%, first copy served 12.1 d vs 6.0 d | group 100%, first copy served 15.3 d vs 6.0 d | missed / missed / 0 of 5 |
| B5 | group 100%, first copy served 12.7 d vs 4.8 d | group 100%, first copy served 12.6 d vs 3.5 d | group 100%, first copy served 14.0 d vs 3.4 d | group 100%, first copy served 11.1 d vs 3.6 d | group 100%, first copy served 12.7 d vs 3.0 d | group 100%, first copy served 12.7 d vs 3.5 d | missed / missed / 0 of 5 |
| B10 | group 100%, first copy served 12.4 d vs 3.8 d | group 100%, first copy served 15.2 d vs 3.5 d | group 100%, first copy served 14.7 d vs 3.3 d | group 100%, first copy served 16.3 d vs 2.3 d | group 100%, first copy served 14.8 d vs 2.9 d | group 100%, first copy served 14.8 d vs 3.3 d | missed / missed / 0 of 5 |
| B20 | group 100%, first copy served 12.3 d vs 3.4 d | group 100%, first copy served 16.2 d vs 2.9 d | group 100%, first copy served 13.1 d vs 2.7 d | group 100%, first copy served 11.4 d vs 2.6 d | group 100%, first copy served 15.8 d vs 3.2 d | group 100%, first copy served 13.1 d vs 2.9 d | missed / missed / 0 of 5 |
| B10-f1 | group 100%, first copy served 13.4 d vs 3.2 d | group 100%, first copy served 13.2 d vs 3.2 d | group 100%, first copy served 14.2 d vs 3.5 d | group 100%, first copy served 14.7 d vs 2.3 d | group 100%, first copy served 14.7 d vs 2.4 d | group 100%, first copy served 14.2 d vs 3.2 d | missed / missed / 0 of 5 |
| B10-f5 | group 100%, first copy served 14.3 d vs 2.9 d | group 100%, first copy served 17.9 d vs 3.2 d | group 100%, first copy served 12.4 d vs 2.8 d | group 100%, first copy served 13.2 d vs 3.1 d | group 100%, first copy served 13.1 d vs 2.8 d | group 100%, first copy served 13.2 d vs 2.9 d | missed / missed / 0 of 5 |
| C | group 100%, first copy served 14.1 d vs 3.1 d | group 100%, first copy served 12.6 d vs 4.0 d | group 100%, first copy served 14.6 d vs 3.4 d | group 100%, first copy served 14.9 d vs 2.9 d | group 100%, first copy served 15.1 d vs 2.8 d | group 100%, first copy served 14.6 d vs 3.1 d | missed / missed / 0 of 5 |
| ABC5 | group 100%, first copy served 12.8 d vs 3.3 d | group 100%, first copy served 14.3 d vs 5.4 d | group 100%, first copy served 11.0 d vs 4.0 d | group 100%, first copy served 14.5 d vs 2.4 d | group 100%, first copy served 14.8 d vs 7.2 d | group 100%, first copy served 14.3 d vs 4.0 d | missed / missed / 0 of 5 |
| ABC10 | group 100%, first copy served 12.0 d vs 2.6 d | group 100%, first copy served 11.3 d vs 6.4 d | group 100%, first copy served 13.5 d vs 6.0 d | group 100%, first copy served 12.5 d vs 3.0 d | group 100%, first copy served 11.5 d vs 6.7 d | group 100%, first copy served 12.0 d vs 6.0 d | missed / missed / 0 of 5 |
| ABC20 | group 100%, first copy served 11.1 d vs 3.9 d | group 100%, first copy served 16.1 d vs 6.6 d | group 100%, first copy served 14.1 d vs 6.4 d | group 100%, first copy served 13.2 d vs 3.3 d | group 100%, first copy served 10.4 d vs 5.6 d | group 100%, first copy served 13.2 d vs 5.6 d | missed / missed / 0 of 5 |

**Mark 7, Z directory**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | of 39: job<=7d 1 (middle 1), standing<=21d 0, list+intro<=28d 1 | of 41: job<=7d 3 (middle 3), standing<=21d 0, list+intro<=28d 0 | of 40: job<=7d 2 (middle 2), standing<=21d 0, list+intro<=28d 0 | of 32: job<=7d 1 (middle 1), standing<=21d 0, list+intro<=28d 1 | of 39: job<=7d 2 (middle 2), standing<=21d 1, list+intro<=28d 2 | of 39: job<=7d 2 (middle 2), standing<=21d 0, list+intro<=28d 1 | missed / missed / 0 of 5 |
| A | of 39: job<=7d 3 (middle 3), standing<=21d 0, list+intro<=28d 0 | of 41: job<=7d 1 (middle 1), standing<=21d 0, list+intro<=28d 5 | of 40: job<=7d 5 (middle 5), standing<=21d 2, list+intro<=28d 4 | of 32: job<=7d 2 (middle 2), standing<=21d 0, list+intro<=28d 3 | of 39: job<=7d 4 (middle 4), standing<=21d 1, list+intro<=28d 5 | of 39: job<=7d 3 (middle 3), standing<=21d 0, list+intro<=28d 4 | missed / missed / 0 of 5 |
| B5 | of 39: job<=7d 5 (middle 3), standing<=21d 0, list+intro<=28d 1 | of 41: job<=7d 5 (middle 2), standing<=21d 0, list+intro<=28d 0 | of 40: job<=7d 3 (middle 1), standing<=21d 0, list+intro<=28d 0 | of 32: job<=7d 4 (middle 1), standing<=21d 0, list+intro<=28d 0 | of 39: job<=7d 2 (middle 2), standing<=21d 0, list+intro<=28d 0 | of 39: job<=7d 4 (middle 2), standing<=21d 0, list+intro<=28d 0 | missed / missed / 0 of 5 |
| B10 | of 39: job<=7d 6 (middle 1), standing<=21d 0, list+intro<=28d 1 | of 41: job<=7d 8 (middle 5), standing<=21d 0, list+intro<=28d 0 | of 40: job<=7d 4 (middle 3), standing<=21d 0, list+intro<=28d 1 | of 32: job<=7d 6 (middle 1), standing<=21d 0, list+intro<=28d 0 | of 39: job<=7d 3 (middle 1), standing<=21d 0, list+intro<=28d 0 | of 39: job<=7d 6 (middle 1), standing<=21d 0, list+intro<=28d 0 | missed / missed / 0 of 5 |
| B20 | of 39: job<=7d 9 (middle 1), standing<=21d 0, list+intro<=28d 0 | of 41: job<=7d 8 (middle 2), standing<=21d 0, list+intro<=28d 2 | of 40: job<=7d 7 (middle 3), standing<=21d 0, list+intro<=28d 1 | of 32: job<=7d 4 (middle 1), standing<=21d 0, list+intro<=28d 1 | of 39: job<=7d 5 (middle 2), standing<=21d 1, list+intro<=28d 1 | of 39: job<=7d 7 (middle 2), standing<=21d 0, list+intro<=28d 1 | missed / missed / 0 of 5 |
| B10-f1 | of 39: job<=7d 8 (middle 1), standing<=21d 0, list+intro<=28d 1 | of 41: job<=7d 11 (middle 3), standing<=21d 1, list+intro<=28d 1 | of 40: job<=7d 7 (middle 3), standing<=21d 0, list+intro<=28d 0 | of 32: job<=7d 6 (middle 1), standing<=21d 0, list+intro<=28d 1 | of 39: job<=7d 4 (middle 1), standing<=21d 0, list+intro<=28d 0 | of 39: job<=7d 7 (middle 1), standing<=21d 0, list+intro<=28d 1 | missed / missed / 0 of 5 |
| B10-f5 | of 39: job<=7d 6 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 41: job<=7d 9 (middle 3), standing<=21d 0, list+intro<=28d 1 | of 40: job<=7d 6 (middle 1), standing<=21d 0, list+intro<=28d 0 | of 32: job<=7d 5 (middle 1), standing<=21d 0, list+intro<=28d 1 | of 39: job<=7d 2 (middle 0), standing<=21d 0, list+intro<=28d 2 | of 39: job<=7d 6 (middle 1), standing<=21d 0, list+intro<=28d 1 | missed / missed / 0 of 5 |
| C | of 39: job<=7d 2 (middle 2), standing<=21d 0, list+intro<=28d 0 | of 41: job<=7d 3 (middle 3), standing<=21d 0, list+intro<=28d 0 | of 40: job<=7d 6 (middle 6), standing<=21d 0, list+intro<=28d 1 | of 32: job<=7d 2 (middle 2), standing<=21d 0, list+intro<=28d 0 | of 39: job<=7d 1 (middle 1), standing<=21d 0, list+intro<=28d 2 | of 39: job<=7d 2 (middle 2), standing<=21d 0, list+intro<=28d 0 | missed / missed / 0 of 5 |
| ABC5 | of 39: job<=7d 3 (middle 3), standing<=21d 1, list+intro<=28d 10 | of 41: job<=7d 1 (middle 1), standing<=21d 1, list+intro<=28d 6 | of 40: job<=7d 6 (middle 5), standing<=21d 2, list+intro<=28d 7 | of 32: job<=7d 6 (middle 5), standing<=21d 0, list+intro<=28d 4 | of 39: job<=7d 1 (middle 1), standing<=21d 0, list+intro<=28d 1 | of 39: job<=7d 3 (middle 3), standing<=21d 1, list+intro<=28d 6 | missed / missed / 0 of 5 |
| ABC10 | of 39: job<=7d 3 (middle 3), standing<=21d 1, list+intro<=28d 4 | of 41: job<=7d 3 (middle 3), standing<=21d 0, list+intro<=28d 1 | of 40: job<=7d 3 (middle 3), standing<=21d 0, list+intro<=28d 4 | of 32: job<=7d 5 (middle 4), standing<=21d 0, list+intro<=28d 3 | of 39: job<=7d 4 (middle 3), standing<=21d 1, list+intro<=28d 4 | of 39: job<=7d 3 (middle 3), standing<=21d 0, list+intro<=28d 4 | missed / missed / 0 of 5 |
| ABC20 | of 39: job<=7d 2 (middle 2), standing<=21d 0, list+intro<=28d 5 | of 41: job<=7d 1 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 40: job<=7d 4 (middle 3), standing<=21d 1, list+intro<=28d 4 | of 32: job<=7d 6 (middle 5), standing<=21d 1, list+intro<=28d 2 | of 39: job<=7d 5 (middle 3), standing<=21d 1, list+intro<=28d 7 | of 39: job<=7d 4 (middle 3), standing<=21d 1, list+intro<=28d 4 | missed / missed / 0 of 5 |

**Mark 7, Z given**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 43: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 38: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 32: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | missed / missed / 0 of 5 |
| A | of 36: job<=7d 0 (middle 0), standing<=21d 1, list+intro<=28d 8 | of 43: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 8 | of 38: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 4 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 5 | of 32: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 3 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 5 | missed / missed / 0 of 5 |
| B5 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 43: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 38: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 32: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | missed / missed / 0 of 5 |
| B10 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 43: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 38: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 32: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | missed / missed / 0 of 5 |
| B20 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 43: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 38: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 3 | of 32: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | missed / missed / 0 of 5 |
| B10-f1 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 43: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 38: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 2 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 32: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | missed / missed / 0 of 5 |
| B10-f5 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 43: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 38: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 32: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | missed / missed / 0 of 5 |
| C | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 43: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 38: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 32: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 0 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | missed / missed / 0 of 5 |
| ABC5 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 6 | of 43: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 7 | of 38: job<=7d 1 (middle 1), standing<=21d 1, list+intro<=28d 4 | of 36: job<=7d 0 (middle 0), standing<=21d 1, list+intro<=28d 7 | of 32: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 1 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 6 | missed / missed / 0 of 5 |
| ABC10 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 2 | of 43: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 5 | of 38: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 3 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 4 | of 32: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 2 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 3 | missed / missed / 0 of 5 |
| ABC20 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 4 | of 43: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 4 | of 38: job<=7d 0 (middle 0), standing<=21d 1, list+intro<=28d 4 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 4 | of 32: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 5 | of 36: job<=7d 0 (middle 0), standing<=21d 0, list+intro<=28d 4 | missed / missed / 0 of 5 |

**Mark 8, Z**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | 142 of 206 | 173 of 230 | 166 of 220 | 175 of 235 | 148 of 222 | 166 of 222 | missed / missed / 0 of 5 |
| A | 182 of 206 | 211 of 230 | 205 of 220 | 211 of 235 | 206 of 222 | 206 of 222 | missed / missed / 0 of 5 |
| B5 | 147 of 206 | 179 of 230 | 160 of 220 | 173 of 235 | 156 of 222 | 160 of 222 | missed / missed / 0 of 5 |
| B10 | 167 of 206 | 167 of 230 | 162 of 220 | 172 of 235 | 164 of 222 | 167 of 222 | missed / missed / 0 of 5 |
| B20 | 172 of 206 | 187 of 230 | 171 of 220 | 198 of 235 | 176 of 222 | 176 of 222 | missed / missed / 0 of 5 |
| B10-f1 | 159 of 206 | 173 of 230 | 169 of 220 | 177 of 235 | 163 of 222 | 169 of 222 | missed / missed / 0 of 5 |
| B10-f5 | 147 of 206 | 176 of 230 | 159 of 220 | 163 of 235 | 167 of 222 | 163 of 222 | missed / missed / 0 of 5 |
| C | 162 of 206 | 186 of 230 | 178 of 220 | 174 of 235 | 181 of 222 | 178 of 222 | missed / missed / 0 of 5 |
| ABC5 | 177 of 206 | 212 of 230 | 200 of 220 | 209 of 235 | 186 of 222 | 200 of 222 | missed / missed / 0 of 5 |
| ABC10 | 183 of 206 | 218 of 230 | 200 of 220 | 208 of 235 | 199 of 222 | 200 of 222 | missed / missed / 0 of 5 |
| ABC20 | 176 of 206 | 206 of 230 | 196 of 220 | 211 of 235 | 205 of 222 | 205 of 222 | missed / missed / 0 of 5 |

**Mark 9, Z accepted**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | no hops at both ends | - | - | - | - | - | not judged |
| A | 28.5% (limit 79.6%) | 51.4% (limit 105.8%) | 64.7% (limit 124.6%) | 20.0% (limit 121.1%) | 45.3% (limit 97.5%) | 45.3% (limit 105.8%) | met / met / 5 of 5 |
| B5 | no hops at both ends | - | - | - | - | - | not judged |
| B10 | no hops at both ends | - | - | - | - | - | not judged |
| B20 | no hops at both ends | - | - | - | - | - | not judged |
| B10-f1 | no hops at both ends | - | - | - | - | - | not judged |
| B10-f5 | no hops at both ends | - | - | - | - | - | not judged |
| C | no hops at both ends | - | - | - | - | - | not judged |
| ABC5 | 66.7% (limit 123.3%) | 61.6% (limit 121.0%) | 83.3% (limit 155.4%) | 0.0% (limit 76.7%) | 47.6% (limit 90.7%) | 61.6% (limit 121.0%) | met / met / 5 of 5 |
| ABC10 | 0.0% (limit 34.3%) | 46.2% (limit 90.3%) | 61.5% (limit 94.8%) | 0.0% (limit 37.5%) | 61.6% (limit 120.4%) | 46.2% (limit 90.3%) | met / met / 5 of 5 |
| ABC20 | 0.0% (limit 16.7%) | 46.0% (limit 65.9%) | 53.4% (limit 89.3%) | 0.0% (limit 22.0%) | 24.3% (limit 58.6%) | 24.3% (limit 58.6%) | met / met / 5 of 5 |

**Mark 9, Z partners**

| Configuration | Seed 1 | 2 | 3 | 4 | 5 | Median | Seed 1 / median / seeds met |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Baseline | 15.7% (limit 15.4%) | 10.9% (limit 11.7%) | 16.7% (limit 19.4%) | 11.7% (limit 15.6%) | 25.3% (limit 20.5%) | 15.7% (limit 15.6%) | missed / missed / 3 of 5 |
| A | 11.0% (limit 21.0%) | 12.7% (limit 25.2%) | 13.5% (limit 26.5%) | 13.9% (limit 23.1%) | 15.4% (limit 24.9%) | 13.5% (limit 24.9%) | met / met / 5 of 5 |
| B5 | 16.1% (limit 21.8%) | 15.6% (limit 17.5%) | 18.1% (limit 23.0%) | 10.7% (limit 13.2%) | 9.5% (limit 14.5%) | 15.6% (limit 17.5%) | met / met / 5 of 5 |
| B10 | 6.9% (limit 10.9%) | 16.3% (limit 17.8%) | 10.5% (limit 13.9%) | 13.8% (limit 18.0%) | 9.8% (limit 12.2%) | 10.5% (limit 13.9%) | met / met / 5 of 5 |
| B20 | 15.2% (limit 20.5%) | 8.1% (limit 13.1%) | 11.1% (limit 16.8%) | 6.9% (limit 8.6%) | 17.9% (limit 19.9%) | 11.1% (limit 16.8%) | met / met / 5 of 5 |
| B10-f1 | 10.5% (limit 10.5%) | 15.2% (limit 19.9%) | 23.7% (limit 23.4%) | 8.4% (limit 16.6%) | 10.5% (limit 13.3%) | 10.5% (limit 16.6%) | missed / met / 3 of 5 |
| B10-f5 | 14.0% (limit 18.3%) | 13.1% (limit 17.6%) | 20.3% (limit 18.3%) | 8.3% (limit 13.3%) | 11.1% (limit 13.0%) | 13.1% (limit 17.6%) | met / met / 4 of 5 |
| C | 18.0% (limit 22.0%) | 13.9% (limit 15.9%) | 13.9% (limit 14.0%) | 17.7% (limit 23.4%) | 13.8% (limit 20.6%) | 13.9% (limit 20.6%) | met / met / 5 of 5 |
| ABC5 | 13.3% (limit 22.5%) | 14.0% (limit 20.9%) | 13.3% (limit 22.1%) | 11.4% (limit 18.2%) | 7.8% (limit 13.8%) | 13.3% (limit 20.9%) | met / met / 5 of 5 |
| ABC10 | 12.7% (limit 23.6%) | 12.5% (limit 21.2%) | 14.3% (limit 24.6%) | 7.9% (limit 12.1%) | 14.9% (limit 24.8%) | 12.7% (limit 23.6%) | met / met / 5 of 5 |
| ABC20 | 11.4% (limit 17.9%) | 12.8% (limit 20.0%) | 12.5% (limit 21.1%) | 10.5% (limit 14.7%) | 13.9% (limit 22.9%) | 12.5% (limit 20.0%) | met / met / 5 of 5 |

</details>

## Not modelled

As in the hops model's plan. Also: failures other than B10-f1's and B10-f5's, and how a newcomer falling behind affects one transfer in detail (a failed job's transfer goes on through the relay); bad bindings or forged requests, as signatures aren't modelled.
