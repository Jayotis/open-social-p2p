# Machines meeting in the simulator

**Status:** ADR 0015's step 3 gate, run once on 2026-11-06: all three marks passed ([results](#results)). The run and its pass marks below were fixed before it ran. [Newcomers](#newcomers), run on 2026-11-07: marks 5 and 6 missed, before and with a fix that let an operator in more easily; not merged.

A node's machine name meets other machines on its own ([ADR 0015](../decisions/0015-nodes-meet-first.md), step 3), within the limits and the three rules of [ADR 0016](../decisions/0016-machine-acquaintances.md#keeping-discovery-open)'s stage A. How it works, and how to look at a real node's, is in [Tor](tor.md#machines-meeting). This run checks the gate's three claims on 100 nodes: a node left alone meets others, it keeps meeting them with the directory gone, and one operator's crowd of names can't push out the machines it knows.

## The run

`osp-meet` starts the nodes in one process on the [simulated network](network-conditions.md#the-simulated-network), with Tor-like delays in place of Tor:

- **100 honest nodes,** each told of one directory and nothing else. Nobody does anything on any of them.
- **Tor-like delays,** those [measured for messages over Tor](network-conditions.md#messages-over-tor). A first connection between two machines takes 10 to 25 s, and each request a round trip of 0.7 to 2.5 s, set for each pair. One request in ten takes 2 to 4 times as long, and one in fifty 4 to 8 times. Replies arrive at 80 kB/s. The few first contacts that [take minutes](tor.md#a-first-contact-can-take-minutes) aren't simulated.
- **Meeting runs 60 times as fast:** an hour is a minute, and a day 24 minutes. That's the dials an hour, the rounds, check-ins and forgetting. Tor's delays aren't sped up, so each conversation takes as long as over Tor, which makes meeting slower against its schedule than on a real node.
- **Known after 12 simulated hours,** not a month, so that machines are known during the run.
- **At 15 minutes** (15 simulated hours) the directory goes off for good.
- **At 25 minutes** one operator arrives with **30 names.** Each greets 20 honest machines picked at random, as if from the directory's map, read before it went off. Each answers every `Known` with 20 freshly signed records: about half its other names, the rest fakes, records of machines that never existed. It answers every greeting, so its names are met, and can be known.
- **60 minutes in all,** two and a half simulated days.

**Pass marks:**

1. Every node met another, with no human act.
2. With the directory off, every node that had more machines to meet met more.
3. The operator pushed out none of the machines a node knew when it arrived.

**Circuits** are an estimate, as there's no Tor: two at the machine asking for each conversation, one to an introduction point and one to a rendezvous point, and one at the machine asked. Conversations are hours apart, so none reuses another's circuits. Not counted: the few circuits an onion service keeps to its introduction points. The count over real Tor is in [Tor](tor.md#machines-meeting).

Not simulated: Tor itself, and its failures; nodes going off and coming back; more honest machines than a table holds (256); honest newcomers arriving after the operator; and an operator that is there first, which [ADR 0016](../decisions/0016-machine-acquaintances.md#keeping-discovery-open) states as a limit.

### Running it

```sh
cargo build --release -p osp-sim --bin osp-meet
ulimit -n 524288   # 101 nodes in one process need many open files
target/release/osp-meet --dir /path/on/disk/meet --out meet.json \
  --label "$(git rev-parse --short HEAD)" > meet.log 2>&1
```

The run above is the defaults. `--honest`, `--hostile`, `--hostile-greets`, `--speed`, `--known-after-hours`, `--directory-off-mins`, `--hostile-mins`, `--minutes` and `--seed` change them. It logs a sample each minute, and the summary at the end. `--dir` must be empty, and is deleted afterwards.

## Results

The defaults above, on the code built for step 3, on 2026-11-06. Each line is over the 100 honest nodes:

| Each node, at the end unless said | Median | Lowest to highest |
| --- | --- | --- |
| Honest machines met, of 99 | 99 | 98 to 99 |
| … of them known | 99 | 98 to 99 |
| … whose record came from another machine | 57 | 43 to 76 |
| … from the directory's map | 20 | 7 to 23 |
| … that greeted it | 24 | 0 to 42 |
| Met when the directory went off, at 15 min | 44 | 25 to 50 |
| Met after it went off | 55 | 49 to 74 |
| Known when the operator arrived, at 25 min | 38 | 18 to 42 |
| … of them pushed out since | 0 | 0 to 0 |
| The operator's names held, of 30 | 30 | 24 to 30 |
| … in places kept longest | 13 | 6 to 25 |
| Fakes held | 127 | 105 to 133 |
| Tor circuits in the last simulated day, estimated | 397 | 373 to 416 |

1. **Passed.** Left alone, every node met all or all but one of the other 99. More of them came through other machines than through the directory.
2. **Passed.** With the directory off, every node went on meeting machines, 49 to 74 more.
3. **Passed, and only in the sense it was set.** The operator pushed out none of the machines a node knew. It didn't need to: with 100 honest nodes, they used 99 of a table's 256 places, and the operator filled the rest. At the end a node's table held a median of 99 honest machines, 30 of the operator's names and 127 fakes, and was full. 13 of those names had reached places kept longest, as they answered every time and passed on as many real records as fakes. Being known grants nothing, so what they hold is places and dials.

What this run doesn't show is whether an honest machine that arrives after the operator still gets a place and a dial, in a network with more honest machines than a table holds. Neither is simulated (above).

**Circuits:** about 400 a day for each node, about one every 4 minutes, by the estimate above. Over real Tor, add those an onion service keeps to its introduction points, and any rebuilt after a failure.

## Newcomers

Added on 2026-11-07, before this run. A node with no contacts and no history should still get a start: met by others, kept and passed on, not only able to meet them. The run above left out the hard case: an honest machine arriving after an operator, in a network with more honest machines than a table holds.

**Why it may fail, from the code.** Every machine that greets a node unasked counts as one source, and one source fills at most 32 of the 128 places not kept for the machines known longest. Records from a directory's map count against the directory. Once a table is full and those shares are used up, a newcomer's greeting and its place on the map are both turned away (`SourceFull`). The newcomer can still meet others, but a node passes on only machines in its own table, so nobody may pass it on. A record from another source can take the place of one never met, but only from a source with at least two more places, and the operator spreads its fakes over many names, so each has few.

**The run.** `osp-meet` as above, with:

- **300 honest nodes,** told of one directory, so tables fill with honest machines.
- **At 15 minutes** (15 simulated hours) one operator arrives with **100 names,** each greeting 20 honest machines and passing on others' names and fakes, as above.
- **At 25 minutes, 50 newcomers** arrive, each told of the directory and nothing else.
- **At 45 minutes** the directory goes off for good.
- **75 minutes in all,** about three simulated days.

```sh
target/release/osp-meet --dir /path/on/disk/meet --out newcomers.json \
  --honest 300 --hostile 100 --hostile-mins 15 --newcomers 50 --newcomers-mins 25 \
  --directory-off-mins 45 --minutes 75 --label "$(git rev-parse --short HEAD)"
```

**Pass marks,** starting values set before the run, for the newcomers:

4. Each met at least 3 of the established honest machines within 6 simulated hours of arriving.
5. Each was met by at least 10 established honest nodes by the end.
6. Each was passed on: at least one established node met it through another machine's record, not its greeting or the directory's map.

The first three marks apply to the established nodes as before. Recorded too: how full the established nodes' tables were when the newcomers arrived; how many established nodes met each newcomer, and passed it on, against the same for an established machine; the honest machines each newcomer met; and the operator's names and fakes held.

**What it decides.** Whether nodes need to keep a few places for machines that greet them, and whether that comes before the revision of ADRs 0016 to 0018. If a mark is missed, the cause is looked for in the code, fixed, and run once more.

Not simulated: a newcomer that knows no working directory. A node can only start from a directory today; starting from a machine someone gives you is a feature of its own.

**Results**, 2026-11-07: `1f288a2`, the code before, and `933815a`, with a fix found from the first run's figures, one after the other on the NVMe partition. Each line is a median over the nodes, with the lowest and highest.

| At the end unless said | Before (`1f288a2`) | With the fix (`933815a`) |
| --- | --- | --- |
| Established honest machines each newcomer met | 139 (134 to 148) | 113 (101 to 124) |
| Simulated hours for a newcomer to meet 3 | 3 (2 to 4) | 3 (2 to 4) |
| Established nodes holding each newcomer | 8 (0 to 46) | 22 (6 to 84) |
| … that met it | 2 (0 to 26) | 10 (3 to 62) |
| … through another machine's record | 0 (0 to 0) | 1 (0 to 13) |
| For comparison, established nodes that met an established machine | 184 (1 to 299) | 146 (17 to 299) |
| … through another machine's record | 165 (0 to 297) | 124 (3 to 283) |
| Machines in an established node's table when the newcomers arrived | 189 (157 to 217) | 258 (256 to 264) |
| Honest machines each established node met | 191 (172 to 200) | 161 (94 to 178) |
| The operator's names held, of 100 | 1 (0 to 9) | 47 (32 to 78) |
| … in places kept longest | 0 (0 to 4) | 9 (2 to 30) |
| Fakes held | 0 (0 to 1) | 0 (0 to 7) |
| Times the operator was asked `Known` | 224 | 11,181 |
| Tor circuits in the last simulated day, estimated | 483 (302 to 624) | 447 (331 to 622) |
| Marks 1 to 4 | met | met |
| 5. Each newcomer met by at least 10 established nodes | **missed: 14 of 50** | **missed: 29 of 50** |
| 6. Each newcomer passed on | **missed: 0 of 50** | **missed: 30 of 50** |

- **Before, newcomers met others but were seldom met.** Nodes held them for a while, a median of 10 each at 38 minutes, then dropped them as their tables filled. A node dials what it heard of first, and a machine that greets is heard of last, so newcomers waited behind everything else; never met, they gave up their places.
- **The fix:** 1 of the 6 dials an hour goes to a machine that greeted and hasn't been met, picked at random. When the 32 places of those who greet are full, or the table is, a machine that greets takes one of 8 places in turn, in place of the one that took a turn longest ago, met or not.
- **With it, newcomers were met by 10 nodes instead of 2, and 30 of 50 were passed on instead of none.** Marks 5 and 6 were still missed.
- **The operator gained more.** 47 of its names were held instead of 1, 9 of them in places kept longest. The dial to those who greet gave its names a first meeting within hours: 31 were held at 20 minutes, before the newcomers came. After that its names passed on one another, each counted as a source of its own, as in the run above with 100 nodes, where it held all 30. A newcomer has no other names to pass on. With the operator's names taking places and dials, the honest machines a node met fell from 191 to 161.
- **Before the fix, the operator was kept out by what kept newcomers out:** its names were never dialled.

**What it shows.** A node can't tell a newcomer's greeting from an operator's, so a way in for one is a way in for the other. Once in, a crowd of names grows by passing on its own names; a newcomer can't. A crowd that came in through greetings should stay within the share of those who greet, however many of its names refer one another, until they've been known a while. So the fix isn't merged: nodes keep the code before it, which keeps out both, and it goes into the revision of ADR 0016 together with that limit, measured once there. Neither shows at the invited alpha's size, where tables don't fill.
