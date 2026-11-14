# ADR 0034: A Universe in levels

**Status:** Proposed (2026-11-14). Step P4 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). Builds on [ADR 0033](0033-what-the-universe-may-show.md)'s snapshot. Work for the node and the app only; the node-to-node protocol is unchanged.

## Context

The Universe has to work at two very different sizes:

- **Small, now.** A node keeps at most 64 MiB of what it found in directories (about 5,000 people), meets at most 256 machines and maps at most 10,000. The first testers will see tens of people.
- **Large, later.** If OSP grows, a node could know far more. A million points can't be drawn on a phone, and wouldn't mean anything if they were.

And the map has to be a place: the part of the world where Kim's friends are should stay where it was yesterday.

## Decision

### 1. What the map is made of

- **Points** are people. Kim's own star is at the centre.
- **Lines** are public acts between people ([ADR 0033](0033-what-the-universe-may-show.md)): reshares and found-early credits, each fading over 90 days as vouches do in [ADR 0029](0029-vouchers-within-six-steps.md), plus Kim's own follows and the people she knows.
- **People with no lines yet,** found only through a directory, sit together near that directory's region, apart from everyone else, until a reshare places them.

Follows between other people aren't public, so the map never shows them. Its clusters are groups that reshare each other, which is closer to "who pays attention to whom" than follows would be anyway.

### 2. Levels, made on the node

The node groups people into clusters, then the clusters into larger ones, with a multilevel community algorithm (Leiden or Louvain style; the choice is the implementer's, and can change). Five levels, from far to near:

| Level | One point is |
| --- | --- |
| Universe | a region: the largest groups the node knows of |
| Galaxy | a large group within a region |
| Cluster | a smaller group, or an active area |
| Local | one person, with their important lines |
| Close | one person, with all their lines, reshares, vouch paths and activity |

Machines are a separate overlay ([ADR 0036](0036-showing-what-the-network-does.md)), never mixed into these clusters ([ADR 0033](0033-what-the-universe-may-show.md)).

The phone asks for one level and one region at a time, and gets only what's in view. For example:

1. Kim opens the Universe: 120 regions.
2. She zooms into region 42: it's replaced by its 85 groups.
3. She zooms into one group: it's replaced by its 70 people.

**A small world starts close.** With fewer than about 150 people, the Universe opens at Local and the higher levels don't exist yet. Nothing pretends to be a galaxy.

### 3. Limits

These are starting values, to be measured in P11 on the test phones:

- a snapshot holds at most **300 points and 600 lines**, and is at most **128 KiB**;
- the phone draws on one canvas, not one widget per point;
- the phone keeps the last snapshots, so the Universe opens at once from the cache, then refines;
- the node reclusters in the background and answers from the last result meanwhile. It never makes the phone wait for the whole graph.

### 4. Stable places

- **Clusters keep their IDs.** After reclustering, a cluster keeps its ID while more than half its people are still in it. A cluster that splits keeps a link to its parent, and the phone animates the split. A merge works the same way in reverse.
- **Positions are kept on the node.** A new cluster starts where a hash of its ID puts it, then near its neighbours. Each relaxation moves anything at most a little, so a few new people don't move unrelated regions.
- **Kim is anchored at the centre,** and the largest regions keep their direction from her.

The algorithm can change during development. Stability can't: it's tested (below).

### 5. Brightness

A point's brightness mixes how many people it holds, its recent public posts, its reshares and, in the Network layer, the hosting it does. Each enters as a logarithm, with a cap, so one very large creator can't outshine every small group. Brightness never uses follower counts, which the node doesn't know ([ADR 0032](0032-profiles-people-want-to-follow.md)).

## Options considered

- **Draw everyone, and let the phone cope.** Works for 50 people, fails long before a million.
- **Cluster on the phone.** Phones differ widely, and every phone would repeat the work. The node is always on, and sized for background work.
- **A fixed layout, such as a circle per step from Kim.** Stable and simple, but says nothing about who belongs with whom. A useful fallback for the Local level if P4 runs long ([ADR 0030](0030-product-alpha-before-testers.md)).

## Consequences

- **CPU and memory on the node.** Clustering is the heaviest new job. The node runs it in the background, at low priority, sized for an SBC: the same 5× slowdown estimated for ADR 0029's work.
- **The made-up million.** No real node will know a million people soon. The million-entity test is for the design, so the levels and limits exist before they're needed.
- **Honesty at small sizes.** The first testers' Universe is sparse. P9 makes it look intentional (a few bright stars, with room around them), not empty. It is never filled with made-up people.

## Gate

- A synthetic world of at least **1,000,000 people** can be explored level by level on a desktop node, with every snapshot within the limits above. The time and memory the node takes to cluster it are recorded, with the SBC estimate beside them.
- A world of **20 people and 5 machines** opens at Local and looks right on the test phones.
- **Stability test:** adding 1% new people to a world moves no unrelated region's centre by more than a set distance, and keeps the IDs of nine clusters in ten.
- Pan and zoom stay smooth on the slowest test phone (Pixel 3 XL).
