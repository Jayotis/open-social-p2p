# Sizing carriers: limits and the newcomer rate, in a model

**Status:** Run five times on 2026-11-20 ([results](#results)): in no run does a setting meet every mark, so nothing is set, and the figures go to the maintainer. Plan fixed on 2026-11-20, before any code or run. [ADR 0043](../decisions/0043-the-media-plane-before-the-alpha.md)'s sizing stage: carrying is built in P11 whatever this shows, so the run doesn't decide whether to build. It sets the starting values: a node's upload share for carrying, its jobs at once, its limits on circuits from one neighbour and new circuits a minute, and how often a transfer gives a newcomer a middle turn. Changing a pass mark later needs a dated note here saying what changed and why.

**Amended 2026-11-20, after the first run, before the second (the maintainer):** the first run's results stand below as run. Three changes for the second run, with the same settings and seeds:

- **Probationary carriers.** Middle machines picked at random never earned standing. The bar stays as it is: jobs for one viewer on 3 different days in the last 30, checked by that viewer. A viewer now keeps a middle machine that finished a job for it, as one of up to 2 probationary carriers. On each day it fetches, it gives each one a middle job first, before the random draw, at most one a day as the bar counts days. A probationer leaves when it earns standing, when it fails, or 30 days after it was taken on. The newcomer rate now sets how often a viewer tries a machine it hasn't worked with.
- **Different seed carriers at the two ends.** A viewer never takes the host's seed carrier as its own hop when another hop has room. The project runs every seed carrier, so the figures count transfers where the project's seed carriers held both ends apart from those where one machine did. A machine of the same operator can share its logs: during bootstrap, the project sees both ends as the relay does, and that's a limit of the bootstrap, said as such.
- **Mark 1** now asks whether carriers are materially slower, not slower at all. An extra hop costs a little, and the first run missed the old mark on 2 to 7% differences while meeting B8 on every late post. The first run would have met the new mark (its worst post was 7% later). Stage D measures the time to build a circuit on real nodes.

Also recorded: the relay's bytes on way 4's transfers by why (an end said no to its own address being seen, or no hop had room); how often the viewer's hop was an ordinary node it earned; probationers taken on, and how many earned standing.

**Amended again 2026-11-20, after the second run, before the third (the maintainer):** both runs' results stand below as run. For the third run, with the same settings and seeds:

- **Mark 2 measures the carriers, not the privacy choice.** An end that says no to its own address being seen keeps its leg on the relay, as it asked: that's the network doing what the choice requires, not a carrier failing. So every mark is judged on runs where no end says no, where every transfer could use carriers, and the carriers carry the most. Nobody knows yet how many will say no, which the Product Alpha's testers will show, so the relay's bytes are reported for the setting chosen (or 50%, 2 jobs and 10% if none is) with 10%, 25% and 50% saying no, for the relay's capacity and cost. They pass or fail nothing.
- **Two probation places, each with its own job.** Continuity: a machine that has finished a job for the viewer, given a middle job each day it fetches until it earns standing, or 30 days pass. Exploration: a machine the viewer has never finished a job with, taken at the newcomer rate, given a middle job each day too. When the continuity place empties, the exploration machine moves into it. The bar for standing stays: jobs on 3 different days in 30, checked by the viewer.
- **Work offers.** When a machine finishes its first middle job for a viewer, that viewer offers it to up to 2 viewers it has been the earned hop for on its own record. A viewer takes an offer only from a machine it holds at standing, at most one a day from each, and keeps at most 2 for 30 days. An offered machine goes first when its exploration place fills. An offer gives a machine a chance to work, never standing: it isn't one of [ADR 0022](../decisions/0022-lists-and-introductions-are-earned.md)'s introductions, and nothing in ADR 0018's records counts it. Stage C tries flooding offers.
- **Also recorded:** the offers sent, taken and used, and the share of transfers through hops where the viewer had no earned hop to pick, which sets how often the project's seed carriers hold both ends until ADR 0018 gives hosts hops of their own.

**Amended a third time 2026-11-20, after the third run, before the fourth (the maintainer):** the earlier results stand as run. Everything else worked once a machine had its first job, so only a newcomer's first job changes:

- **A continuity place and a newcomer turn.** The continuity place is as before: one machine that finished a job for the viewer, given a middle job each day until it earns standing or 30 days pass. The newcomer turn, once a day, goes to a machine the viewer has never finished a job with, from a queue: the machines in its places for those who greet ([ADR 0015](../decisions/0015-nodes-meet-first.md)'s 40, with its limits on one source), and those offered to it, in the order they came. A later arrival never goes ahead of one waiting, so minting new names doesn't jump the queue: ranking by newest was considered and rejected for that reason. A machine the viewer can't use when its turn comes (busy or gone) goes to the back. After one finished job it leaves the queue, takes the continuity place if it's free, and is offered on as before.
- **Offers** join the queue, in the order they came, rather than going first.
- **The newcomer rate** now sets the other middle turns: drawn at random among machines the viewer never finished a job with.
- **The circuit limits** come from this run, as the queue may change how circuits bunch.

**Amended a fourth time 2026-11-20, after the fourth run, before the fifth (the maintainer):** the earlier results stand as run. The fourth run's queue held machines that weren't new, and its daily turn ran whether or not anyone new was waiting. The third run's rules come back unchanged (two probation places, continuity after a first finished job, work offers after it, the newcomer rate's draw, 50% upload share, 2 jobs, 2 seed carriers). Only how a machine the viewer never worked with gets its first chance changes:

- **Fresh machines.** A machine is fresh to a viewer if the viewer has never finished a job with it and it first entered the viewer's table within the last 7 days, mark 3's own window. A machine that was in the table before doesn't become fresh when the rule starts, and leaving and coming back doesn't make it fresh again: the time counts from when it first came in.
- **A fresh turn.** At most once a day, a viewer gives the middle of a transfer through hops to the oldest of its fresh machines, after its probationers. A later arrival never goes ahead of an older one still fresh, so the one closest to its 7 days goes first, and a name minted later waits behind a real newcomer. One that's busy, gone or already on the path is passed over for the next, and stays fresh only until its own 7 days run out. With no fresh machine, there's no turn, so a settled network adds no third hop.
- **After a first job,** a fresh machine leaves the fresh list and goes through the third run's probation and offers. It still needs jobs on 3 different days in 30 for standing.
- **Offers** are as in the third run: they follow a real first job and give a chance to work, never standing.
- **The circuit limits** come from this run.

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
| Ends saying no to their own address | none, for every mark (amended 2026-11-20); 10%, 25% and 50% at one setting, for the relay's demand |

That's 14 settings and the 2 to compare against, each with seeds 1 to 5. Each mark is judged on seed 1, as in the hops model, and the brackets give how many of the 5 seeds meet it.

## Pass marks

Fixed on 2026-11-20, before the model is built.

1. **Not materially slower than the project relay** (amended 2026-11-20). Over all 45 posts, way 4's time for 95% of followers is within a tenth of the project-relay way's for at least 9 posts in 10, and no more than a fifth later for any. B8's three marks are met on at least as many posts of weeks 10 to 13 as the project-relay way meets. *Until 2026-11-20 it asked for no later, not within a tenth, on 9 posts in 10.*
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

### First run

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

### Second run

With probationary carriers, different seed carriers at the two ends and the amended mark 1 (2026-11-20). The ways compared against are unchanged.

| Mark | Result | Seed 1 |
| --- | --- | --- |
| 1, speed | met at 50% with 2 jobs [5/5], and at 50%, 4 jobs and 5% [5/5]; missed elsewhere | at 50% and 2 jobs, every post within a tenth, worst 1.06 to 1.09x. At 25%, worst 1.17 to 2.13x, and with 4 jobs B8 is met on as few as 2 of 13 late posts |
| 2, off the relay | missed with 1 and 2 seed carriers [0/5]; 4: met [4/5] | 59 to 73% of the project relay's bytes with 2 seed carriers, 78% with 1, 46% with 4 |
| 3, newcomers | missed in every setting [0/5] | standing within 28 days: 8 to 19 of 50 directory newcomers, 0 or 1 of 50 given one machine |
| 4, the operator | met in every setting | it held no hop next to both ends of any transfer |

Again nothing is set by the plan's rule.

**Graduation works for nodes already there.** At 50%, 2 jobs and 10%, viewers took on 2,346 probationers and 2,151 of them earned standing. On 18,593 transfers the viewer's hop was an ordinary node it had earned, against none in the first run. One seed carrier next to both ends fell from 99% of transfers with a hop at both ends to 5%, and the project's seed carriers holding both ends fell to 29%. Homes carried a median 11 MB a day, at most 78.

**What stays on the relay is the question on a node's own address.** 67 to 82% of the relay's bytes on way 4 are an end's leg where that end said no; the rest is no hop with room. On seed 1 at 50%, 2 jobs and 10%, the ends saying no alone send about 47% of the project relay's bytes: mark 2's 50% can't be met with 1 in 4 saying no, whatever the carriers. With 4 seed carriers nearly all of it is ends saying no.

**A quarter of a home's upload is too little for an earned hop.** At 25%, a 5 Mbit/s home carries at 1.25 Mbit/s, and a viewer whose earned hop is such a home fetches slowly: up to twice the project relay's time. More jobs at once split that share further. At 50% with 2 jobs, mark 1 is met on every seed.

**Newcomers still start too slowly.** Probation places go mostly to machines that are in many tables already: the established ones. A directory newcomer took a median 21 to 31 days to standing, and newcomers given one machine almost never got a middle turn: they're in very few tables.

**Circuits,** in every setting: a home held at most 4 from one neighbour at once and opened at most 10 in a minute; a seed carrier at most 21 from one neighbour.

### Third run

With two probation places, work offers, and every mark judged with no end saying no (amended again 2026-11-20).

| Mark | Result | Seed 1 |
| --- | --- | --- |
| 1, speed | met at 50% with 2 jobs [5/5 at each rate]; missed elsewhere | at 50%, 2 jobs and 10%: 44 of 45 posts within a tenth, worst 1.11x, B8 met on all 13 late posts. At 25%, worst 1.26 to 2.31x, and B8 met on as few as none |
| 2, off the relay | met at 50% with 2 jobs [5/5], and with 4 seed carriers [5/5]; missed elsewhere | 43 to 48% of the project relay's bytes at 50% with 2 jobs; 20% with 4 seed carriers; 74% with 1 |
| 3, newcomers | missed in every setting [0/5] | at 50%, 2 jobs and 10%: a middle turn within 7 days for 7 of 50 directory newcomers and none given one machine; standing within 28 days for 17 and none |
| 4, the operator | met in every setting | no transfer with both ends' hops all its own |

Nothing is set: mark 3 is missed everywhere.

**What works.** At 50% and 2 jobs, carriers take enough off the relay and aren't materially slower. Viewers took on 2,422 probationers and 2,179 earned standing; 2,919 work offers were sent, 1,764 taken and 1,185 used. The viewer had no earned hop on 24% of transfers through hops; one seed carrier was next to both ends on 7% of transfers with a hop at both ends, and the project's seed carriers on 34%. Homes carried a median 16 MB a day, at most 62.

**What doesn't: a newcomer's first job.** Probation places and offers all start from a first finished middle job, and a newcomer rarely gets one. A viewer's exploration draw picks among every machine in its table it hasn't worked with, and a newcomer is one of many there. A newcomer given one machine is in almost no tables: none got a middle turn within 7 days in any setting or seed. Offers spread a newcomer only after that first job.

**The relay's demand with ends saying no,** at 50%, 2 jobs, 10% and 2 seed carriers, in weeks 10 to 13 against the project relay's bytes: 47 to 50% with 10% saying no, 55 to 58% with 25%, 73 to 76% with 50%. The relay's busiest second was 550 to 730 Mbit/s.

**Circuits,** at 50% and 2 jobs: a home held at most 2 from one neighbour at once and opened at most 7 in a minute, over every rate and seed; in any setting, at most 4 and 9.

### Fourth run

With the newcomer queue (amended a third time 2026-11-20). Every mark is missed in every setting [0/5], and marks 1 and 2, met in the third run, are missed again. At 50%, 2 jobs and 10%, seed 1, against the third run:

| | Third run | Fourth run |
| --- | --- | --- |
| Mark 1: posts within a tenth; worst | 44 of 45; 1.11x | 31 of 45; 1.24x |
| Mark 2: relayed against the project relay | 46% | 54% |
| Mark 3: directory newcomers with a middle turn within 7 days; standing within 28 | 7; 17 of 50 | 5; 20 of 50 |
| Newcomers given one machine: either | none | none |
| Pairs that earned standing | 2,179 | 3,895 |
| Transfers through three hops | 4,942 | 12,174 |
| Homes carried a day, median | 16 MB | 37 MB |

**The queue is full of machines that aren't new.** A viewer's places for those who greet fill from day 0 with established machines it hasn't worked with yet. A newcomer arriving on day 35 waits behind them, at one turn a day, and a viewer fetches about every second day. So the queue graduates many established pairs, and reaches newcomers weeks late.

**A turn every day costs the others.** It puts a third hop on many more transfers, through homes carrying within half their upload, so posts take longer, carriers have less room, and more goes through the relay.

### Fifth run

With fresh turns (amended a fourth time 2026-11-20). Machines that came into a table in the same hour are taken in a random order, not by their number in the model: by number, the newest machines, numbered last, always lost a tie, and on the first post every viewer reached for the same few machines.

Every setting misses [0/5]. Directory newcomers now meet mark 3 everywhere, at 50% and 2 jobs on all 5 seeds; newcomers given one machine get much closer; marks 1 and 2, met in the third run, are missed. At 50%, 2 jobs and 10%, seed 1:

| | Third run | Fifth run |
| --- | --- | --- |
| Mark 1: posts within a tenth; worst | 44 of 45; 1.11x | 20 of 45; 1.22x |
| Mark 2: relayed against the project relay | 46% | 55% |
| Mark 3: directory newcomers with a middle turn within 7 days; standing within 28 | 7; 17 of 50 | 49; 50 of 50 |
| Newcomers given one machine: either | none | 36; 26 of 50 (27 to 41; 21 to 31 over the seeds) |
| Probationers taken on, earned standing | 2,422, 2,179 | 5,877, 5,188 |
| Transfers through three hops | 4,942 | 11,461 |
| Homes carried a day, median | 16 MB | 41 MB |

**Fresh to a viewer isn't new to the network.** Of 8,141 fresh turns, 2,315 went to Z's newcomers and 5,826 to machines already there that had just come into one more viewer's table, mostly by greeting it: 200 to 700 a week, falling to about 200 by week 13. A viewer can't tell the two apart from its own table, as both come in the same way.

**That costs both sides.** The other machines' turns are the extra third hops that slow posts (mark 1) and fill carriers (mark 2). And they fill the fresh lists: a median of 10 machines were fresh to a viewer at once, so a newcomer given one machine, arriving on days 57 to 63, often waits past its 7 days behind machines that aren't new, at one turn a day from a viewer that fetches about every second day. It isn't a lack of reach: 49 of the 50 were fresh to some viewer, 458 pairs in all, about 9 viewers each.

**The relay's demand with ends saying no,** at 50%, 2 jobs, 10% and 2 seed carriers: 57 to 59% with 10% saying no, 62 to 64% with 25%, 74 to 76% with 50%; the busiest second 560 to 700 Mbit/s.

**Circuits,** at 50% and 2 jobs over every rate and seed: a home held at most 2 from one neighbour at once and opened at most 6 in a minute, so the plan's rule would give 4 and 12; a seed carrier at most 18 from one neighbour and 81 a minute. Nothing is set, as no setting meets every mark.

## Not modelled

What the hops model leaves out, and: metered connections and the pause (nobody pauses); the project relay going down and n0 taking over; Tor's time to arrange a circuit, and a circuit's build time (a path is ready when picked); circuits reused for the next fetch.
