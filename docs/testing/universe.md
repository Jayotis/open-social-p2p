# Testing the Universe

What the Universe's gates ([ADR 0033](../decisions/0033-what-the-universe-may-show.md), [ADR 0034](../decisions/0034-a-universe-in-levels.md)) were checked with, and what came out.

## The tree on a million people

ADR 0034's gate asks for the time and memory the node takes to cluster a synthetic world of a million people, with the SBC estimate beside them.

**The world.** 1,000,000 people in friend groups of 20, the groups gathered into regions of 50. Each person has three lines inside their group, a one-in-three chance of a line elsewhere in their region, and a one-in-fifty chance of a line anywhere. About 4.3 million lines in all. It's made up for the test, as the gate says. No real node knows this many people.

**How to run it:**

```sh
cargo test -p osp-node --release --lib a_million -- --ignored --nocapture
```

**Result (2026-11-14, desktop, Ryzen 5 3600XT, one thread):**

| | |
| --- | --- |
| Grouping | 28 s |
| Peak memory, the world included | 551 MB |
| Groups | 49,915, every one within 100 children |
| Depth | 4 levels below the top |
| SBC estimate (5×, as ADR 0029's work) | about 2½ minutes, the same memory |

The node would run this in the background at low priority, answering from the last tree meanwhile (ADR 0034 §3). A real node holds at most about 5,000 people found through directories and the people near its owner, so its grouping takes a fraction of a second.

The ordinary test run checks the same rules on 20,000 people: every group within its limit, each person in one group, the same world giving the same tree, and more than 80% of each friend group sharing a bottom group.

## Stable places

ADR 0034 §4's stability gate: adding 1% new people moves no unrelated region's centre by more than a set distance, and keeps the IDs of nine groups in ten.

**The test** (`places::tests::a_few_new_people_move_no_unrelated_region`, in every run): 20,000 people as above, in regions of 10 groups. Then 200 newcomers join one region: 100 join its friend groups and 100 make five new groups there. Everyone has a random key and is numbered by key, as on the node, so the newcomers reorder everyone and the grouping runs in a different order. A region (a group just below the top) counts as unrelated when none of its people has a line to a newcomer. The set distance is 2 units (one person is about 1 unit across).

**Result (2026-11-14):** 1,101 groups, then 1,106. Every old ID kept, 5 new groups, 10 groups moved, all inside the region that changed. The 92 unrelated regions didn't move at all, because a group whose children didn't change isn't laid out again.

**A million people** (`a_million_people_are_placed`, run by hand as above): placing the tree from nothing takes 5.0 s. After 10,000 more people join (half into 50 friend groups, half into 250 new ones), placing again takes 2.2 s, and 46,935 of the 50,096 IDs are kept (94%). Peak memory for the whole test was 1.9 GB, but that includes two worlds, two graphs and two trees held at once.

## The public sky

ADR 0034's gate: in a world where a directory lists 5,000 people with no lines, none of them is in any cluster, and their field shows no structure. Each one joins a cluster once a public line places them. And every listed person can be reached by zooming into cells, with each snapshot within the limits.

**The tests** (`sky::tests`, in every run): 5,000 listed people with made-up keys and no lines. Each person's place is a hash of their key alone, so neither the directory nor anyone they know can move it.

**Result (2026-11-14):**

| Check | Result | An even spread |
| --- | --- | --- |
| People in any group | none; all 5,000 unplaced | none |
| χ² over a 16 × 16 grid | 231 | about 255; fails above 330 (1 in 1,000) |
| χ² over a 4 × 4 grid | 16.1 | about 15; fails above 37.7 |
| Nearest-neighbour ratio (Clark and Evans) | 0.998 | 1; clumps would show below 0.9 |
| Lights at the top | 298, within the 300 points | |
| Zooms from the top to the furthest person | 1 | |

One public line between two of them puts both in a group, and their places in the sky stay the same. People and machines get separate skies (ADR 0033 §4).

## Exploring the snapshot

ADR 0034's gate: a million people explored down the tree, with every snapshot within its limits, and dense and sparse regions opening to different depths. And moving around (§6) sends nothing new between nodes.

**How a region opens.** The top starts from the tree's roots, with Kim's own way down first. A group starts from its children. Then the node opens the largest group shown, again and again, while its children still fit in the 300 points. So a small group opens to its people while a large one beside it stays one light. Standing at someone opens their own group, adds up to 50 of the people they have lines with, and lists the groups above them as the way back up. At the top, the public sky gets 60 of the points while it has people.

**Where the sky sits.** The sky is a ring around the placed world, from 64 to 1,024 units from Kim (a person is about 1 unit across). A hashed place goes round the ring and outwards so that equal areas stay equal, which keeps an even spread even. The ring never moves as the placed world grows.

**The test** (`world::tests::a_million_people_are_explored`, run by hand):

```sh
cargo test -p osp-node --release --lib a_million_people_are_explored -- --ignored --nocapture
```

**Result (2026-11-14, desktop, one thread):** 1,000,000 people in 50,000 friend groups of 20, the groups joined in a ring. Every snapshot from the top down opened: 23,965 of them, at most 300 points each, and everyone was reached. Grouping and placing took 47 s, making the world's keys included. Opening a snapshot from the kept tree takes about a microsecond. The top showed groups 2, 3 and 4 levels down.

**Moving around** (`osp-sim`, `the_universe_holds_no_address_and_opening_it_sends_nothing`, in every run): Kim's node stands at each person it shows, opens each region on the way back up and each group shown, and opens four sky cells. It sends nothing new: no new connection, no request, no viewing endpoint.
