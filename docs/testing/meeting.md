# Machines meeting in the simulator

**Status:** ADR 0015's step 3 gate, run once on 2026-11-06: all three marks passed ([results](#results)). The run and its pass marks below were fixed before it ran.

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
