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

## On the phone

ADR 0034's gate: a world of 20 people and 5 machines opens at Local and looks right on the test phones, and pan and zoom stay smooth on the slowest one (Pixel 3 XL).

**The world.** `osp-world` starts 20 made-up people as real nodes on one computer: two groups of seven friends who follow and reshare each other, one reshare between the groups, a group of three, and three people the owner follows who have no lines. The owner, Rae, follows a few in each group. A phone pairs with Rae's node by the code it prints. The people's nodes talk only to each other, on loopback; Rae's has relays, as an ordinary node does, so the phone can reach it.

```sh
cargo run --release -p osp-sim --bin osp-world -- --dir <empty directory>
```

It has no directory and no machines: on real nodes both need machine names, and those need Tor. The machines were checked on a lab node instead, which knew 10.

**Smoothness.** The app draws with Flutter, so Android's own frame counts (`dumpsys gfxinfo`) see nothing. Frame times come from the compositor instead: `adb shell dumpsys SurfaceFlinger --latency '<the app's SurfaceView (BLAST) layer>'`, cleared with `--latency-clear` before the gesture.

**Result (2026-11-14, Pixel 3 XL, app build 9):** Rae's Universe opened at Local with 21 people and 14 lines, no groups, every name readable, the two friend groups apart and the reshare between them drawn. 14 lines, not 17: three reshares are between people Rae doesn't follow, so her node never gets them. A 1.5 s pan drew 88 frames, every one at the screen's 16.7 ms, none missed. Standing at someone framed them with their lines brighter, and stepping back went up one step with the map unmoved. On a lab node with 13 people and 10 machines, the machines' list read in plain words.

## A newcomer exploring

ADR 0035's gate: on a fresh node with no follows, against the lab's world, the maintainer reaches people and posts by browsing alone, without typing a search. Every person and post the node holds is reachable from the map or the field's list.

**The node.** A new node with Tor on, reading the lab's two directories, following no one, and a phone paired with it (Pixel 3 XL, app build 10, then 12).

**Result (2026-11-15):**

| | |
| --- | --- |
| First directory read | 7 min 44 s after start. Both directories' first tries timed out over Tor before one answered. The clean install of 2026-11-14 took 50 s. |
| People held | 5, all shown as stars in the public sky (no lines, so no groups), none left out |
| Posts held | 10: one person's own 5, and the 5 she reshared, by someone no directory lists |
| Reached by browsing | every person, from the sky or the list of who the node found; her 5 posts from her profile's Posts, the other 5 from its Reshares |

So the gate's reach is met: nothing the node holds needed a search. The 5 posts by an unlisted author are reachable only through the resharer's Reshares tab, as that author has no star of their own.

**What the first look showed, and what changed:**
- The stars were too small and unnamed for a handful of people. With 40 or fewer, they're now drawn larger and all named.
- "Found through" followed by a machine's name meant nothing to a newcomer. It now says "Asked to be found, listed by" that machine.
- Closing the standing card stepped back only a little, so only the one stood at stayed in view. It now frames the whole region above. Pinching up still leaves the map where it is (ADR 0034 §6), which made pinching out feel like it did nothing.
- On a phone on its side, the card covered the Layers button and the owner's star. It now keeps to the bottom left.

**Still open:**
- The first view frames the whole sky ring, so a few people sit close round the owner's star and their names don't fit until one zooms in. A newcomer might not know the map zooms at all.
- Nothing explains why the machines shown have no names, or how a node comes to know people.

## What the network does

ADR 0036's gate, on the phone: what the map draws comes only from the node's events, and says in plain words what the node did. (The summary's lines against the node's own counts are checked in `osp-sim`: `osp-sim/tests/summary.rs`.)

**The node.** Three lab nodes on P6 part 3 (migration 35), a phone paired with one of them (Pixel 3 XL, app build 15), the Universe on screen and recorded. Someone that node follows posted two photos.

**Result (2026-11-15):**

| | |
| --- | --- |
| Before anything happened | No events on any node, and the map still. |
| First photo | The node fetched it ahead of time: one event, 274 KB in two parts. The author's node had one: served 548 KB in four parts, to two nodes. The list on the phone said "Your node fetched 274 KB of Kim's post from another node", as `osp events` did. |
| Second photo | Drawn about 20 s after the node fetched it: a flow from the edge of the view into the owner's star, about 3 s, with its line under the map. Then the map was still again. |
| Not seen | Reshares, people found and machines met or faded: none happened in the run. They're covered by `osp-sim/tests/events.rs` and the app's tests. |

**What the run showed, and what changed:**
- The line under the map said "fetched 19 KB": the first part of the photo was its own event, and the rest was summed into it on the node after the phone had drawn it. An event drawn in the last two minutes that grows now has its line shown again with the total, without being drawn again.
- The photo came from its author's node, whose star was right beside the owner's, but the flow came from the edge as "another node": an event named a host only when it was a resharer listed as hosting the post. A fetch now says when the author's own node sent most of it (`from_author`, ADR 0033), and flows from their star: "Your node fetched 458 KB of Kim's post from Kim's node".

**Again with both fixes (app build 16), a third photo:** opening the app drew the earlier fetch from the last hour, with its total. The new one was drawn as a flow from Kim's star into the owner's, its line saying "fetched 36 KB of Kim's post from Kim's node", and about 20 s later the line came back with the total, 593 KB. `osp events` said "from the author's node".

**Still open:**
- Kim's star is close to the owner's, so the flow from it is short and easy to miss. The line under the map says it either way.

## How are we connected?

ADR 0037's gate, over real Tor: the owner asks how they're connected to someone. Their node answers from the paths it keeps, and asks the other person's node who vouched for them, under a throwaway key that nobody can link to the owner. (The paths, and the answer from the other node, are checked in `osp-sim/tests/universe.rs` and `osp-node/tests/vouches.rs`. Those tests don't use real Tor.)

**The node.** Three lab nodes on P7 (migration 36), all on Tor. The owner's node follows Kim. A phone is paired with the owner's node (Pixel 3 XL, app build 17). We asked with `osp connected Kim` and with the phone's button on Kim's profile.

**Result (2026-11-15):**

| | |
| --- | --- |
| First asks | Kim's node answered 2 of 6. The other 4 waited the full 45 s and said "their node didn't answer". Kim's node saw no stream at all from the asks that failed. Each of those came soon after the owner's node had started. |
| The phone's fallback | When Kim's node didn't answer, the phone still showed what the owner's node knew: the path to Kim lit on the map, and a card saying "Their node didn't answer; only what yours knows". |
| The throwaway | Kim's node saw only the throwaway keys, and kept no record of them. Its list of peers named only the owner's node, from its usual sync. |

**What the run showed, and what changed:**
- After Tor starts, the owner's node often couldn't reach Kim's onion service at all for about two minutes. A plain SOCKS connection there timed out after 120 s, with or without isolation. Once Tor had warmed up, the same connection took 1 to 10 s. That's how Tor behaves, not this code.
- One ask that worked connected in 59 ms: the throwaway's stream went over a circuit Tor already had open to Kim's onion service for the owner's own node. Kim's Tor sees which circuit each stream comes in on, so a modified node could link the throwaway key to the owner. Each key's dials now go under its own SOCKS user name, which Tor uses to keep them on separate circuits (`IsolateSOCKSAuth`). A throwaway therefore never shares a circuit, and neither do the node's other keys.

**Again with the fix:** asked from the owner's node, Kim's node answered in 8.7 s: 5.5 s to dial a new circuit, then the question and the answer. From the phone, in 33 s, 28 s of it the dial. The card said "Their node named nobody who vouched for them", with the path to Kim lit. Both stayed within the 45 s the node waits. The owner's node kept syncing with its peers over Tor with the change.

**Still open:**
- A new circuit each time makes an ask take 5 to 30 s. That's the price of not being linked to the owner.

**Since then (P7 part 5):**
- An ask no longer fails in the first minutes after Tor starts. Tor says it's ready before onion services can be reached, so the node waits for a better sign: a stream through its Tor has reached another node's onion service since Tor last started. Any of the node's keys count, and the node's own syncing makes such streams anyway. Until then the ask waits, up to 30 s, and then has 20 s or more to dial, all inside the phone's minute. The card says "Tor is still starting; your node asks once it reaches others…". An ask dialled before Tor has worked doesn't count against the 10 an hour. `osp status` adds "reached no node yet" to Tor's line until then. The wait is checked against a fake Tor that says it's ready but can't reach anyone (`osp-tor/tests/fake_tor.rs`).
- The card's title is the person's name; "How you're connected to …" cut the name off.
