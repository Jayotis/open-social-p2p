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
