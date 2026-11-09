# Hops and earned links over three months, in a model

**Status:** Plan, fixed on 2026-11-08 before any code or run. It covers [ADR 0017](../decisions/0017-one-hop-forwarding.md)'s stage A, with [ADR 0018](../decisions/0018-earned-direct-connections.md)'s layers on a shortened clock (its step 2's question). [ADR 0023](../decisions/0023-followers-keep-working-together.md)'s partners run beside the rules as accepted, and [ADR 0022](../decisions/0022-lists-and-introductions-are-earned.md)'s earned lists get a first read. Changing a pass mark later needs a dated note here saying what changed and why.

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
- Layers 1 to 4 as in ADR 0018's table. Layer 4 needs 100 MB of checked bytes (a starting value). ADR 0018's caps apply: 32 earned peers at once, and 2 new a week.
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

## Not modelled

QUIC, and timings shorter than a second; Tor's timing for messages; routers, so every pair can connect; nodes going off and coming back; other creators, scouts' reshares and strangers, so whether creators' and scouts' nodes become the network's meeting points isn't shown; bad data and failing disks; an operator dropping jobs; other users of the relay; and phones, which are windows onto their owner's node.
