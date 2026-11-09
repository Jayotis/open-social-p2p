# Hops and earned links over three months, in a model

**Status:** Run on 2026-11-09, all marks judged ([results](#results)): mark 0 met, marks 5 and 9 met, mark 3 met in Z under ADR 0023's rules; marks 1, 2, 4, 6, 7 and 8 missed. The maintainer accepted the results and the plan's outcome the same day: nothing from ADR 0017 is built under these rules, and the alpha goes ahead on relays. A second run with lists asked only with a reason missed mark 9 too ([below](#lists-asked-only-with-a-reason)). The plan below was fixed on 2026-11-08 before any code or run, and no pass mark has changed since. It covers [ADR 0017](../decisions/0017-one-hop-forwarding.md)'s stage A, with [ADR 0018](../decisions/0018-earned-direct-connections.md)'s layers on a shortened clock (its step 2's question). [ADR 0023](../decisions/0023-followers-keep-working-together.md)'s partners run beside the rules as accepted, and [ADR 0022](../decisions/0022-lists-and-introductions-are-earned.md)'s earned lists get a first read. Changing a pass mark later needs a dated note here saying what changed and why.

Stage A asks two things. Would two hops of volunteer nodes take enough media off relays to be worth building? And do pairs of nodes deal with each other often enough to earn direct links? None of this is built, so the run measures the rules, not the code.

## Why a model

- **Real nodes run only what's built.** Hops, earned links, partners and earned lists aren't, and stage A decides whether to build them.
- **A month of real posts takes too long.** One 52 MB post to 300 followers takes 10 to 35 minutes on the [simulated network](network-conditions.md#the-simulated-network), and about 16 GB of disk. 45 posts in each run below would take days per run.
- **So `osp-month`, in `osp-sim`, runs no nodes.** It follows each transfer by the rules: who fetches from whom, the path each transfer takes, and what each node writes in its own record, as ADRs 0011, 0016 to 0018, 0022 and 0023 say. Three months take minutes.
- **What it can't show:** QUIC, real timings, or the node's code. So the model is first checked against runs measured on real nodes (mark 0). If the hops pass, one real run checks B8's marks with the paths the model gave ([below](#the-check-on-real-nodes)).

**Three months, not one.** An earned link needs 30 days since first contact (ADR 0018's layer 4), so a month from zero standing would show nothing direct. 13 weeks is the three months of ADR 0018's price check.

## Who's in it

**From zero standing (Z),** with nobody approving anyone:

- Kim, the creator, at 20 Mbit/s up and 100 down, as in B8.
- 300 followers from day 0, at 5, 10 and 20 Mbit/s up in turn, and 50, 100 and 300 down.
- 50 newcomers following Kim, arriving evenly over days 35 to 49, each told only of a directory, at the same speeds.
- 50 more newcomers following Kim, arriving evenly over days 57 to 63, after the directory has gone off. Each is told of one established machine picked at random, as if a friend had passed on its record. Starting from a machine someone gives you isn't built ([meeting](meeting.md#newcomers)), so this stands in for it.
- One operator's 30 nodes, following Kim ([below](#the-operator)).

A newcomer needs no directory for its media: following Kim gets it her posts, the followers her node recommends, and partners. The directory, or a machine given to it, only starts its machine, which [ADR 0015](../decisions/0015-nodes-meet-first.md) keeps apart from whom it follows. So the two groups of newcomers differ in how their machines start, not in how their media does.

**From 10 approved by hand (S):**

- Kim, as above.
- 10 followers approved by hand to one another from day 0.
- 100 followers arriving evenly over days 0 to 28.
- The 10 leave on day 45, halfway through the run. An earned link takes 30 days, so if they left sooner the run would show only that nothing had been earned yet.
- No operator, so this start is measured on its own.

**For everyone:**

- Kim posts a 52 MB video every 2 days, at a random hour: 45 posts.
- Every follower shares what it prefetches ([ADR 0012](../decisions/0012-sharing-prefetched-copies.md)).
- 3 in 4 honest nodes say yes to carrying, and 3 in 4 to earned links, each picked separately (starting values; both questions are offered with yes recommended). A node carries within half its upload, behind everything else it does.
- One relay, at n0's measured limits: 8 Mbit/s for each endpoint after a 2.7 MB burst ([media through relays](network-conditions.md#media-through-relays)). Its total isn't limited.
- Nodes stay on, except the 10 who leave.

## Three ways, two sets of rules

| Way | Media between two nodes goes |
| --- | --- |
| 1. Relays alone | through the relay, as today (`direct = "nobody"`) |
| 2. Earned links | directly between two nodes that each earned it ([ADR 0018](../decisions/0018-earned-direct-connections.md)); otherwise through the relay |
| 3. Earned links and hops | as in 2; otherwise through the hops ([ADR 0017](../decisions/0017-one-hop-forwarding.md)) where an end has one, any leg without one through the relay |

Each way runs twice. Under **the rules as accepted**, Jimmy fetches from the followers Kim's node names at random, and a middle machine is picked at random. Under **ADR 0023's rules**, he fetches first from his partners, and middle turns go back to machines that did one. That's 12 runs in all, each with the same posts, speeds and seed.

## What the model follows

**Each post.** Each follower waits a random moment in the 10 minutes after the post, drawn afresh for each post, so which of two partners holds a post first changes from post to post. It then fetches from up to 3 sources at once, and from Kim last, as the node does. Under the accepted rules, the sources are the 6 followers Kim's node names at random among those holding a copy. Under ADR 0023's, they're its partners first. Each node serves one transfer for each 1.5 Mbit/s of its upload, and refuses more as busy, as the node does ([places to suit the upload](network-conditions.md#places-to-suit-the-upload)). A transfer gets the smallest share along its path: of the sender's upload, of each hop's carrying limit, of the relay's limit for each endpoint, and of the receiver's download. The model steps a second at a time while a post spreads.

**Each path,** in way 3, as ADR 0017's "Who picks" sets it, for Jimmy fetching from Alice:

- Jimmy's hop: one of his direct peers, or a machine that finished jobs for him on 3 different days, with room under its limit, if he said yes to earned links.
- Alice's hop: one of her direct peers with room, if she said yes.
- On 1 in 10 transfers that use a hop, a machine in the middle that has no standing with Jimmy, from his table. It's picked at random under the accepted rules, and by ADR 0023's rule otherwise.
- A leg with no hop goes through the relay. A transfer takes its bytes off the relay only if none of its legs goes through it.

**Each record,** kept by each node alone, and fading by half every 14 days. A node credits only what it received and checked itself, so when Jimmy serves Alice, the work goes in Alice's record of Jimmy, and when she serves him, in his record of her. Nothing passes between the two records.

- Work, for ADR 0018's layers: downloads completed from a node; Kim's spot checks of her followers' copies; and, once a node has shown its machine at layer 2, the jobs that machine carried. Each counts only the bytes the node checked.
- Kim's spot checks: 100 a day (a starting value), each fetching one 1 KiB piece picked at random, as today's check of a follower's copy does, and crediting that 1 KiB and no more. So her checks put followers on her record and count days toward layers 1 to 3. But even all 100 a day on one follower for 91 days would come to 8.9 MiB, short of layer 4's 100 MB, and Kim never downloads from her followers. By these rules she earns no direct link with any of them, though [ADR 0018](../decisions/0018-earned-direct-connections.md#layers) expects her to. The model reports that as it finds it, and credits no bytes that nobody checked.
- Layers 1 to 4 as in ADR 0018's table, reached on actual days of work in the last 30 and kept while faded standing holds ([as clarified](../decisions/0018-earned-direct-connections.md#reaching-a-layer-and-keeping-it) on 2026-11-09). Layer 4 needs 100 MB of checked bytes received in the last 30 days (a starting value), and keeps it while faded checked bytes stay at 50 MB. ADR 0018's caps apply: 32 earned peers at once, and 2 new a week.
- Carrying, for ADR 0016's record: jobs finished as a hop, on different days.
- ADR 0023's partners: up to 4 for Kim's posts.

**Meeting, with lists earned** ([ADR 0022](../decisions/0022-lists-and-introductions-are-earned.md)):

- At the start, a machine knows only a directory's map and the machines that greet it. The map holds every machine that greeted the directory. Each machine greets those on the map, within ADR 0015's 6 dials an hour.
- Tables keep ADR 0015's and 0016's limits: 256 places, the 40 places of those who greet, and a quarter for one source. Tor's timing isn't modelled.
- Alice gives her list to a machine that did work for her on 3 different days, once a day, and an empty list to anyone else. She introduces a machine that did that work to machines that accept her introductions, as ADR 0022 sets. A node that counts an introduction halves layer 4's days and bytes for the machine introduced, as ADR 0018 does for a reference. Kim's node recommending sources isn't an introduction, and counts for nothing here ([ADR 0023](../decisions/0023-followers-keep-working-together.md)).
- The directory goes off on day 56, for the last five weeks.

## The operator

In Z, one operator runs 30 nodes following Kim, about a tenth of the machines that carry, each at 100 Mbit/s up and 1,000 down. They fetch each post the moment it's out, with no random wait, by the same rules as any follower: nobody else holds it yet, so their first copies come from Kim, and each copy counts in their download and in its sender's upload. Then they host it and serve it fast, and say yes to carrying and to earned links. They carry and finish every job they get, greet every machine on the map, and ask for every list they can. That's the expensive path ADR 0022 sets as the price: everything it gets, it gets by real work. It never drops a job, as dropping only loses standing; ADR 0017's stage C tests that.

The model reports what the operator got and what it paid:

- honest nodes' addresses, through direct links and as the hop next to an end;
- lists and introductions;
- the transfers where it held the hops on both sides, and so saw both ends;
- the terabytes it downloaded getting the posts, and uploaded serving and carrying them.

A fast server that serves well is a benefit, so nothing marks the operator down for that. Mark 9 only checks that it sees both ends no more often than its share of the hops suggests.

## Pass marks

Fixed on 2026-11-08, before the model is built.

**0. The model matches the measured runs.** This is checked first, and nothing else is judged until it holds. With one post, 300 followers, 10 latecomers and the relay, as in [media through relays](network-conditions.md#media-through-relays), the model runs with no followers, half of them, and all of them behind the relay. Its time for 95% of followers must be within a fifth of the 704, 655 and 1,535 s measured, the relay's copies within a tenth of the 0, 167 and 370 measured, and Kim's copies between 15 and 30.

*Note, 2026-11-09:* the all-relay run's 1,535 s and 370 copies came from a node that fetched some bytes twice (a waiting prefetch lost what had arrived; [the note on the relay runs](network-conditions.md#media-through-relays)). Measured again with the fix: 1,376 s and 325 copies, which replace them here. The model was built to the node's code, which never had that bug, so it's checked against the corrected run. The 50% run's 3% excess stays inside this mark's tenth, so its figures stand.

*Note, 2026-11-09:* the mark didn't say how many times the model runs. It runs each case with 5 seeds, and each figure is judged by its median over the 5, as a single real run can itself start slow or fast (the run with none behind the relay started slow).

**ADR 0017's gate,** for each start and each set of rules:

1. **Hops take at least a quarter of what way 2 still relays off the relay,** in weeks 10 to 13: way 3's relayed bytes are at most three quarters of way 2's.
2. **B8's marks hold** for the posts of weeks 10 to 13 in way 3: 95% of the followers present hold each post within 15 minutes and all within 30, and Kim sends copies to at most a tenth of them. Every follower starts behind the relay, where B8's marks were missed ([media through relays](network-conditions.md#media-through-relays)), so the early weeks can't be expected to meet them. Over all 45 posts, way 3 isn't slower than way 1: its time for 95% of followers is no later than way 1's for at least 9 posts in 10, and no more than a fifth later for any. A path through hops can be held up for a while, as any can, so this allows a few slow posts but not slower ones in general.
3. **The share going direct grows,** in ways 2 and 3: it's higher in weeks 10 to 13 than in weeks 5 to 8.

**ADR 0023,** judged on its rules against the accepted ones, in way 3 of Z:

4. **Followers earn links with each other:** by day 60, at least half the followers there from day 0 that said yes to earned links have an earned direct peer other than Kim.
5. **Partners aren't overloaded:** busy refusals for each post are at most one and a half times those under the accepted rules, week by week.
6. **The swarm doesn't split:** on day 91, the largest group of followers linked by partners holds at least 95% of the followers present. And newcomers serve their first copy as soon after arriving as under the accepted rules, going by the median days.

**ADR 0022's first read,** judged on ADR 0023's rules, in way 3 of Z:

7. **Newcomers get going,** in each of Z's two groups, judged apart: 9 in 10 of those that said yes to carrying get a middle turn within 7 days of arriving, standing with one node (jobs for it on 3 different days) within 21, and that node's list and an introduction within 28. For the second group, this is joining with the directory off, starting from one machine.
8. **Established nodes keep meeting with the directory off:** after day 56, each established node that carries gets at least one list or introduction naming a machine that isn't in its table. That counts what it hears of, whether or not its table has room to keep it.

**The operator,** in way 3 of Z, under each set of rules:

9. **It sees both ends no more often than its share suggests:** the share of transfers through hops where it held the hops on both sides is at most twice p², where p is its share of the hops picked.

**The model's own checks,** not the design's. Any failure is a fault in the model, fixed before the results count:

- No machine learns an address the rules don't give it.
- No node credits more bytes than it checked, and each side's record of the other holds only the work it received.
- The operator holds no post before a transfer has brought it one.

## Recorded too

For each run and each week:

- the bytes going direct, through hops and through the relay;
- the upload added to homes for each byte delivered;
- how long until followers hold each post;
- Kim's copies;
- busy refusals.

Also recorded:

- how many weeks pass before hops carry anything, and in S, whether they still carry after the 10 leave;
- how often a follower's sources are ones it used before;
- days to each layer for pairs of followers, with and without an introduction, and the days of work each side's record holds of the other;
- how evenly partners swap: for each pair, the share of the posts both fetched that each side served first, by their upload speeds, and the pairs that stay short of layer 3 because the work goes mostly one way. A fast follower serving a slow one more often is how the swarm should work, so this is measured, not required;
- the bytes Kim checked from each follower, and the direct links she earned, if any;
- for each newcomer, in each group, the days to its first copy served, middle turn, standing, list and introduction, and to being picked next to a viewer;
- lists asked for and refused;
- the operator's figures, above.

## The check on real nodes

If marks 0 to 3 are met under one set of rules, one real run checks B8's marks with the paths the model gave in its last week. It's `osp-sim --swarm --net --relay` with 300 followers and one post. Pairs the model has direct may connect directly. Transfers through hops go through those hops' connections on the simulated network, each held to its carrying limit, and everything else goes through the relay. It passes with B8's three marks ([pass marks](network-conditions.md#pass-marks)), and the relay's bytes within a fifth of the model's for that post. It needs the simulated network to send a pair's packets through other nodes' connections, in sim builds only: no protocol, and no hops in the node.

## What it decides

- **Marks 0 to 3 and the real check met:** ADR 0017's stage B is built, with the set of rules that met them.
- **Missed:** nothing from ADR 0017 is built, ADR 0014's rules stand, and the invited alpha goes ahead on relays. The figures go to the maintainer for the rules to be rethought, and a new ADR's rules are measured the same way, once.
- **Marks 4 to 6** decide whether ADR 0023 is accepted. Its partners can be built before any hops, as on their own they move bytes onto earned links (way 2).
- **Marks 7 and 8** decide whether ADR 0022's starting values go into its build.
- **Kim's links:** if she earns none, as the arithmetic above says, whether creators need direct links with their followers, and which rule would change (the size or number of spot checks, or the bytes layer 4 asks of them), is the maintainer's decision, made from the figures.
- **If a mark is missed,** the cause is looked for in the rules and in the model before anything changes, and the model is run once more.

## Results

`osp-month` in `osp-sim`, 2026-11-09. Each mark is judged on seed 1; the brackets give how many of seeds 1 to 5 meet it, as context. Mark 0 is judged on medians over 5 seeds. A run takes about 5 s and repeats exactly for its seed.

### How the model reads the rules

Where the plan left a choice, the model took these readings:

- **Standing, for lists and introductions:** ADR 0022's work first. Alice holds Jimmy at standing when his machine finished jobs for her on 3 different days in the last 30, or when he served her downloads on 3 days and his node has shown her its machine (his node holds hers at layer 2).
- **Asking for lists,** in the first run: every machine asks each one in its table once a day, at a check-in, and the operator's do the same. Alice answers with up to 20 machines from her table, or an empty list. That run is kept as the aggressive baseline (`--lists every-day`); the [second run](#lists-asked-only-with-a-reason) asks only with a reason.
- **Introductions:** every machine with standing asks to be introduced; Alice picks whom to, among machines that accept hers. A machine Kim counts an introduction for goes first among her middle machines, and halves layer 4's 30 days and 100 MB for it. "Answered sooner when busy" isn't modelled.
- **Tables:** 256 places: 40 for those who greet (an introduction counts among them), turns when they're full, a greeting first, and 64 for one source, the directory's map included. A machine that did work for the node leaves the greeters' places. A middle machine is drawn from the viewer's own table.
- **Mark 7's "that node's list":** a list from any node that holds the newcomer at standing. **Mark 8's established nodes:** the honest followers there from day 0 that carry.
- **Fixed in the model before these results:** S's 10 were never made direct peers. They are now, as ADR 0018 says for contacts approved by hand, until they leave on day 45.

Not modelled: Tor's timing, so a dial is answered at once; ADR 0015's 2 dials an hour for sources heard from less; forgetting machines that stop answering, except those that left.

### Every mark

| Mark | Case | Result | Seed 1 |
| --- | --- | --- | --- |
| 0 | the model against measured runs | met | 95% at 609, 653 and 1,296 s; relay 0, 163 and 326 copies; Kim 18 to 25 copies |
| 1 | Z, accepted | missed [0/5] | way 3 relayed 100% of way 2's (needs at most 75%) |
| 1 | Z, partners | missed [0/5] | 90% |
| 1 | S, accepted | missed [0/5] | 100% |
| 1 | S, partners | missed [0/5] | 99% |
| 2 | Z, accepted | missed [0/5] | B8 met on 0 of 13 posts; no later than way 1 on 45 of 45 |
| 2 | Z, partners | missed [0/5] | 0 of 13; 43 of 45, worst 1.03x |
| 2 | S, accepted | missed [0/5] | 0 of 13; 23 of 45, worst 1.18x |
| 2 | S, partners | missed [0/5] | 0 of 13; 19 of 45, worst 1.34x |
| 3 | Z, accepted | missed [0/5] | direct share 0% in both periods |
| 3 | Z, partners | met [4/5] | way 2 4.5% → 5.1%, way 3 5.4% → 5.8% |
| 3 | S, accepted | missed [0/5] | way 2 0.8% → 0.0%, way 3 0.2% → 0.0% |
| 3 | S, partners | missed [1/5] | way 2 0.8% → 0.2%, way 3 1.0% → 0.8% |
| 4 | Z, partners | missed [0/5] | 44% of day-0 followers linked by day 60 (needs 50%) |
| 5 | Z, partners against accepted | met [5/5] | worst week 1.18x the busy refusals (needs at most 1.5x) |
| 6 | Z, partners against accepted | missed [0/5] | largest partner group 430 of 430, met; newcomers' first copy served after a median 15.3 days, against 3.2 |
| 7 | Z, directory newcomers | missed [0/5] | of 39 that carry: middle turn within 7 days 2, standing within 21 days 0, a list and an introduction within 28 days 1 (needs 35 each) |
| 7 | Z, given newcomers | missed [0/5] | of 36: 0, 0, 1 (needs 32 each) |
| 8 | Z, partners | missed [0/5] | 165 of 206 established nodes heard of a new machine after day 56 (needs all) |
| 9 | Z, accepted | not judged | no transfer went through hops: no direct links, so no hops |
| 9 | Z, partners | met [5/5] | the operator held 24.5% of end hops, and both ends of 10.7% of transfers with both (needs at most 12.0%) |

In S, both of the 10's links lift weeks 5 to 8 and then leave with them on day 45, so mark 3 compares a period with them against one without.

### Where it falls short

**Hops rarely have an end to start from.** In Z, under ADR 0023's rules, of 17,299 transfers in way 3 that weren't over a direct link (seeds 1 to 5 in brackets):

| Hops the ends could pick | Transfers | From Kim's node |
| --- | --- | --- |
| both the host's and the viewer's | 868 (821–1,261) | 0 |
| the host's only | 1,737 (1,737–2,173) | 0 |
| the viewer's only | 2,441 (2,286–2,616) | 208 |
| neither | 12,253 (11,260–12,253) | 630 |

A host's hop is one of its direct peers, so on 85% of these transfers the host had none to pick. Kim's node has none at all, as her spot checks can't reach layer 4. Under the accepted rules no follower pair earns a link in Z, so every transfer is "neither".

**Partners mostly work one way.** ADR 0023 makes a partner from a download in either direction, expecting the order to change from post to post. In Z, way 3, at the end:

| Partner pairs | Pairs | Never served back | Direct | Not direct, both said yes: too few days either way / one way only |
| --- | --- | --- | --- | --- |
| both honest | 858 | 552 (64%) | 46 | 301 / 114 |
| with an operator's node | 378 | 287 (76%) | 24 | 122 / 131 |

The operator's nodes fetch at once, so they nearly always hold a post first, and their pairs are mostly one way. Honest pairs are too: whoever is faster or wakes earlier keeps serving. Bytes or age is the first thing missing for 3 pairs at most, so the work going one way, and on too few days, is what keeps pairs apart.

**Newcomers meet, but rarely carry.** A middle turn comes on 1 transfer in 10 through hops, so 489 in the whole run, about 5 a day for some 400 machines that carry. Directory newcomers greeted their way into tables, but only 15 of the 39 that carry got a middle turn at any time, and of all 50, 9 got a list and 4 an introduction; of newcomers given one machine, none got a middle turn, 4 a list and 1 an introduction. With no standing, lists stay shut, as ADR 0022 intends: 99.6% of 4.6 million asks were refused. Under ADR 0023's rules newcomers also serve later (mark 6), as Kim's node names first the followers her checks passed on the most days, and they aren't anyone's partner yet.

**What the operator got,** under ADR 0023's rules: direct links with 22 honest nodes; 3,923 of the 19,922 lists given (20%, with 7% of the machines) and 173 of 643 introductions; for 71 GB received and 0.36 TB sent serving and carrying. Every list came from work the honest node checked: mostly downloads the operator's nodes served.

### Lists asked only with a reason

ADR 0022's once a day is a limit, not a schedule (the maintainer, 2026-11-09). So in the second run, the model's default, an honest machine asks for a list only while its table has room, and only machines its own record gives it a reason to think it has worked for: its node served theirs on 3 days in the last 30 and has shown them its machine, or it carried as the hop next to that viewer on 3 days. A middle machine sees neither end, so its middle turns give it no one to ask. The machine asked still decides from its own record, as before. The operator's machines still ask every machine in their table every day, as the plan has them ask for every list they can. The rule for what earns a list is unchanged: 3 days of checked carrying, or 3 days of checked downloads once an authenticated binding ties the machine to its node; the binding itself earns nothing.

Every pass mark is unchanged, as are the results of marks 0 to 7, except for small differences in the figures. Under ADR 0023's rules, seed 1:

| | Every day (first run) | With a reason |
| --- | --- | --- |
| Lists asked by honest machines | 4,130,621 | 10,798 |
| of those refused | 99.6% | 0.1% |
| Lists given, to honest machines / to the operator's | 15,999 / 3,923 | 10,783 / 4,236 |
| Mark 8: established nodes hearing of a new machine after day 56 | 165 of 206 | 142 of 206 |
| Mark 9: the operator's share of end hops; both ends (limit) | 24.5%; 10.7% (12.0%), met [5/5] | 27.8%; 15.7% (15.4%), missed [3/5] |

With honest machines asking far less, the operator's machines, which still ask everyone, hold more of what nodes know: 28% of the lists given, with 7% of the machines. They end up next to more ends as hops, and mark 9 is now missed on seed 1. Every list they got still came from work the honest node checked. Mark 8 drops as established nodes ask less.

The second run's way to get a list still needs work both ways when it's earned by downloads: Jimmy's node shows its machine to Alice only once it holds her at layer 2, so Alice must have served him too. A machine that only serves, the role the swarm most needs, gets a list only by carrying.

### What the plan says follows

- **Marks 1 to 3 missed** under both sets of rules: nothing from ADR 0017 is built, ADR 0014's rules stand, and the invited alpha goes ahead on relays. The figures go to the maintainer, for the rules to be rethought.
- **Marks 4 and 6 missed:** ADR 0023 isn't accepted as it stands.
- **Marks 7 and 8 missed:** ADR 0022's starting values don't go into its build as they stand. Its gate rests on carrying, which rests on hops.
- **Kim's links:** none, as the plan's arithmetic said (most checked from one follower 15 KiB in 30 days, against 100 MB). Whether creators need direct links with their followers is the maintainer's decision.

## Not modelled

QUIC, and timings shorter than a second; Tor's timing for messages; routers, so every pair can connect; nodes going off and coming back; other creators, scouts' reshares and strangers, so whether creators' and scouts' nodes become the network's meeting points isn't shown; bad data and failing disks; an operator dropping jobs; other users of the relay; and phones, which are windows onto their owner's node.
