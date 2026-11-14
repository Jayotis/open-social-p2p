# ADR 0034: A Universe in levels

**Status:** Proposed (2026-11-14). Step P4 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). Builds on [ADR 0033](0033-what-the-universe-may-show.md)'s snapshot. Work for the node and the app only; the node-to-node protocol is unchanged.

## Context

The Universe has to work at two very different sizes:

- **Small, now.** A node keeps at most 64 MiB of what it found in directories (about 5,000 people), meets at most 256 machines and maps at most 10,000. The first testers will see tens of people.
- **Large, later.** If OSP grows, a node could know far more. A million points can't be drawn on a phone, and wouldn't mean anything if they were.

And the map has to be a place: the part of the world where Kim's friends are should stay where it was yesterday.

## Decision

### 1. Only public evidence makes geography

- **Points** are people. Kim's own star is at the centre.
- **Lines** are public acts between people ([ADR 0033](0033-what-the-universe-may-show.md)): reshares and found-early credits, each fading over 90 days as vouches do in [ADR 0029](0029-vouchers-within-six-steps.md), plus Kim's own follows and the people she knows.
- **Nearness means lines.** Two people are drawn near each other only because of lines between them, or between people near them. Nothing else puts anyone near anyone.

Follows between other people aren't public, so the map never shows them. Its clusters are groups that reshare each other, which is closer to "who pays attention to whom" than follows would be anyway.

**People with no lines yet are unplaced.** Being listed by the same directory is infrastructure, not a relationship, so it never groups people. With one directory, that would make thousands of strangers into one false galaxy. Instead, each unplaced person is a faint, isolated star in a loose background field around the placed world. Its position comes from a hash of the person's key, so it is stable, and it is spread evenly, so no shape forms. A person leaves the field and joins the geography when the first public line places them. The phone draws the field as background texture: at far zoom levels, counted per area of sky, not drawn one by one.

**Grouping by topic, if ever, is shown as topic.** A later grouping by what people post about would be a separate layer, labelled as a topic and drawn in its own way. It would never be shown as a social connection, and never mixed into the social clusters.

Machines are a separate overlay ([ADR 0036](0036-showing-what-the-network-does.md)), never mixed into the people's clusters either ([ADR 0033](0033-what-the-universe-may-show.md)).

### 2. A recursive tree, with five names for zoom

**The tree.** The node groups placed people into clusters with a multilevel community algorithm (Leiden or Louvain style; the choice is the implementer's, and can change). It then groups clusters into larger ones, recursively, until the top has few enough to show at once. The tree's depth isn't fixed. A bright point may hold eight people, or eight clusters of clusters, or six more levels, depending on how much the node knows there. Each node of the tree has an ID, a parent, its children and a summary (how many people, brightness, position).

**The names.** Universe, Galaxy, Cluster, Local and Close are **zoom bands**: words for how close the camera is, used by the app and in these docs.

| Band | What's on screen |
| --- | --- |
| Universe | the top of the tree: the largest groups the node knows |
| Galaxy | large groups, wherever they are in the tree |
| Cluster | smaller groups and active areas |
| Local | individual people, with their important lines |
| Close | one person, with all their lines, reshares, vouch paths and activity |

The phone asks for a region and a zoom. The node walks down the tree from that region, opening the brightest and largest nodes first, until the visible budget (below) is used. So at one zoom, a dense area may show tree nodes three levels deeper than a sparse one beside it. For example:

1. Kim opens the Universe: 120 top-level groups.
2. She zooms into group 42: it opens into its 85 children, some of them people, some groups.
3. She zooms into one of those: it opens into 70 people, or into more groups if it is that large.

**A small world starts close.** If everyone fits in the budget, the Universe opens at Local, and the bands above it simply don't appear. Nothing pretends to be a galaxy.

### 3. Limits

These are starting values, to be measured in P11 on the test phones:

- a snapshot holds at most **300 points and 600 lines**, and is at most **128 KiB**, plus the background field as counts per area;
- the phone draws on one canvas, not one widget per point;
- the phone keeps the last snapshots, so the Universe opens at once from the cache, then refines;
- the node reclusters in the background and answers from the last tree meanwhile. It never makes the phone wait for the whole graph.

### 4. Stable places

- **Tree nodes keep their IDs.** After reclustering, a group keeps its ID while more than half its people are still in it. A group that splits keeps a link to its parent, and the phone animates the split. A merge works the same way in reverse. Inserting or removing a level in the tree keeps the IDs below it.
- **Positions are kept on the node.** A new group starts where a hash of its ID puts it, then near its neighbours. Each relaxation moves anything at most a little, so a few new people don't move unrelated regions.
- **Kim is anchored at the centre,** and the largest groups keep their direction from her.
- **The background field doesn't move.** A person in it stays at their hashed place until a line places them.

The algorithm can change during development. Stability can't: it's tested (below).

### 5. Brightness

A point's brightness mixes how many people it holds, its recent public posts, its reshares and, in the Network layer, the hosting it does. Each enters as a logarithm, with a cap, so one very large creator can't outshine every small group. Brightness never uses follower counts, which the node doesn't know ([ADR 0032](0032-profiles-people-want-to-follow.md)).

## Options considered

- **Draw everyone, and let the phone cope.** Works for 50 people, fails long before a million.
- **Cluster on the phone.** Phones differ widely, and every phone would repeat the work. The node is always on, and sized for background work.
- **Five fixed levels.** Simpler to build, but a fixed depth either crushes a large world into too few points or pads a small one with empty levels.
- **Group unplaced people by the directory that listed them.** Fills the map, but with a false social shape (above).
- **A fixed layout, such as a circle per step from Kim.** Stable and simple, but says nothing about who belongs with whom. A useful fallback for the Local band if P4 runs long ([ADR 0030](0030-product-alpha-before-testers.md)).

## Consequences

- **CPU and memory on the node.** Clustering is the heaviest new job. The node runs it in the background, at low priority, sized for an SBC: the same 5× slowdown estimated for ADR 0029's work.
- **The made-up million.** No real node will know a million people soon. The million-entity test is for the design, so the tree and the limits exist before they're needed.
- **Honesty at small sizes.** The first testers' Universe is mostly a background field with a few placed stars around Kim. P9 makes that look intentional, not empty. It is never filled with made-up people or made-up nearness.

## Gate

- A synthetic world of at least **1,000,000 people** can be explored down the tree on a desktop node, with every snapshot within the limits above, and with dense and sparse regions opening to different depths. The time and memory the node takes to cluster it are recorded, with the SBC estimate beside them.
- A world of **20 people and 5 machines** opens at Local and looks right on the test phones.
- **No false geography:** in a world where a directory lists 5,000 people with no lines, none of them is in any cluster, and their field shows no structure (a uniformity test on their positions). Each one joins a cluster once a public line places them.
- **Stability test:** adding 1% new people to a world moves no unrelated region's centre by more than a set distance, and keeps the IDs of nine groups in ten.
- Pan and zoom stay smooth on the slowest test phone (Pixel 3 XL).
