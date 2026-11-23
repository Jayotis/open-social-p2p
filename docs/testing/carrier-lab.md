# Carrier circuits on the lab nodes (ADR 0043 stage B, P11 part 6)

**Status:** Functional pass, 2026-11-21, the maintainer's call. Main at d7550d9 (stage B part 5b) on every node. The newcomer's middle turn passed the same day ([below](#the-newcomers-middle-turn)). Speed isn't graded here: that's stage D, which first measures the relay-only way on these nodes.

[ADR 0043](../decisions/0043-the-media-plane-before-the-alpha.md)'s stage B gate asks that a video go H → Q → P → V on the lab nodes, that a packet capture shows neither end learning the other's address, and that a new node carries in the middle with no setup answer.

## Setup

| Role | Node | Where |
| --- | --- | --- |
| H, the host | Empty, holding P11 part 6's 108.5 MB video | the x86_64 test VM |
| Q, H's hop | the project's first relay machine's node, `seed = true`, the only seed carrier in H's config | a cloud server, also the project's relay |
| P, V's hop | home node C, the only seed carrier in V's config (a lab-only setting) | a home machine |
| M, a newcomer | Middle, a node made that day, with no answer to the hops question | another home machine |
| V, the viewer | Robin, paired to the Pixel 4 XL | the arm64 test VM (emulated) |

Every node used the project's relay first and n0's as fallback, as an installed node does. H and V answered yes to the hops question. Captures (headers only) ran on H, V and Q. Both VMs are behind NAT and the desktop's VPN.

## The two-hop run

Robin followed Empty and fetched the 108.5 MB video.

- **It went through carriers:** Robin logged `fetched through carriers hops=2`: H → Q → P → V.
- **H** sent nothing to P's address or to the home network. Its leg to Q went through the project's relay.
- **Q**'s carrier had one direct peer during the fetch: P.
- **V** talked to P, its own hop, and otherwise only to its transport relays and Tor. Its leg to P also went through the project's relay.
- **A new node carries:** Middle carried from the middle (`Carrying: on`) without anyone answering the setup question.
- **The app** (build 5b, Pixel 3 XL against node A): once A knew a seed carrier, the hops question opened by itself, and the answer reached the node (`hops = true`). Before that, with no seed carrier listed, it didn't ask, as built. The Carrying screen was clear (the maintainer's check).

**Observed, not graded:** 108.5 MB in 4 min 24 s, about 0.41 MB/s. The arm64 VM is emulated, the relay machine has 1 vCPU, both VMs are behind NAT and a VPN, and every leg went through that same machine. Stage D measures the relay-only way first, with these nodes and this video.

## Finding: one operator in two roles

The project's relay machine was also Q. As both ends' legs to their hops went through its relay, its operator saw both ends' addresses. ADR 0043's table of who sees what assumed carrier and relay observations aren't combined, so it's amended: seed carriers and project relays are separate operator roles. Separate machines with the same operator don't count. Different operators in each role is the target deployment. The bootstrap alpha may have one operator in both for a while, and the table says what that operator can then link up. The layout with separate operators is tested once an outside tester keeps a node running. Even with separate operators, a relay that both ends' legs use sees both ends, as n0's does today. Stronger privacy needs relay diversity or direct paths, a separate requirement that wasn't taken up in stage B.

## Stage D plan (fixed 2026-11-21, before the run)

The maintainer set the marks before any stage D fetch. Same nodes, roles and video as above. V fetches the 108.5 MB video three ways, three times each, in turn (carriers, relay only, n0 only), dropping its copy between fetches. Each figure is the median of its three:

- **Through carriers:** as set up above, with H and V's hops answered yes.
- **Relay only:** V and H answer no to the hops question, so both legs go through the project's relay.
- **n0 only:** as relay only, with V and H on n0's relays.

Measured, each from the fetch command: the time until the whole file is stored (throughput), and the time to the first byte, standing in for the start of playing. The first byte comes from V's log of the download that finished (`download attempt`): its start, which is its log time less its `took_ms`, plus its `first_byte_ms`. That includes asking for offers and building the circuit. Asking the host for circuit offers over Tor is timed on its own (`asked the hosts for circuit offers`, `took_ms`).

**Marks:**

1. Through carriers, the whole file arrives within a tenth of relay-only's time.
2. Through carriers, the first byte arrives no more than 2 s after relay-only's.
3. From asking the trackers until the circuit is usable takes under 5 s each time.

n0 is measured for comparison and isn't graded. Changing a mark needs a dated note here saying what changed and why.

*Mark 3 changed 2026-11-21, after the first run (the maintainer).* It was "asking for circuit offers takes under 5 s each time". On the path these fetches take, the tracker's answer carries the host's offer, so there's no separate ask to time, and the first run logged none. The mark keeps its purpose: the time from V asking the trackers (`asking the trackers who hosts it`) until the circuit can carry the download (`the circuit is ready`), both in V's log.

*Marks changed 2026-11-22, after the fifth run (the maintainer).* Runs 1–5 stay as judged against the marks above.

- **Mark 1 is retired for later runs.** It was "the whole file arrives within a tenth of relay-only's time".
- **In its place, watchability:** through carriers, the sustained download speed is at least twice the video's encoded playback bitrate. This asks whether carrier playback stays ahead of viewing, not whether a longer, more private path matches one relay's bulk transfer time. Stage D's video averages 1.53 Mbit/s (113.8 million bytes over 595 s), so the mark is about 3.1 Mbit/s (0.38 MB/s).
- **Startup (mark 2) stays an open requirement.** It isn't passed. In run 5 the connection opened ahead worked: once the carrier download started, its first byte took about 1.0 s, against 1.1–1.2 s for relay only. But the circuit still added about 4–5 s before that.
- **The circuit's availability and reliability (mark 3) stay open too.** In run 5, V couldn't reach its own hop for 11 minutes after a restart. That is a real failure, not an artefact of a speed mark.

Stage D runs again under these marks (the sixth run) after two fixes: a freshly restarted viewer reaching its own carrier quickly, and less of the circuit build paid while the viewer waits.

## Stage D results

**Failed marks 1 and 2, 2026-11-21.** Nine fetches, carriers first, then relay only, then n0 only. Every carrier fetch went over two hops (`fetched through carriers hops=2`). Each figure is the median of three. First byte is counted from the start of the fetch step, which includes a sync, so it's the same extra time for all three ways.

| Way | Whole file | First byte |
|---|---|---|
| Through carriers | 213.1 s | 11.6 s |
| Relay only | 63.5 s | 7.6 s |
| n0 only | 64.5 s | 8.9 s |

1. **Whole file: failed.** The carrier fetch took 3.4 times relay-only's time (about 0.53 MB/s against 1.8 MB/s), where the mark allows 1.1 times. The three carrier fetches were close together (210–216 s), so this is the path's speed and not a bad round.
2. **First byte: failed, by about 2 s.** It came 4.0 s after relay-only's. In each carrier fetch, V's direct try at H failed after about 0.8 s, then 3.8–4.6 s passed before the carrier download started. That gap covers asking for offers and building the circuit. Once started, the first byte took 1.9 s, against 1.1 s for relay only.
3. **Circuit offers: not measured.** V logged no `circuit offers` line during the run, so there was no time for the step by itself. The 3.8–4.6 s gap above, which includes it, is under 5 s.

Relay only and n0 only were within 2 % of each other. *Corrected 2026-11-22:* the "n0 only" step never reached n0. The script swapped a `relay = ` line the nodes' configs don't have (they name the project's relay under `[network.relay]`), so that step was the project's relay again with the fallback turned off. The comparison says nothing about the project's relay against n0. The cause found below stands: the extra time was in the carrier path.

**The cause: P's upload share.** P is a home node set up before `osp init` asked the upload speed, so it carried at half of an assumed 10 Mbit/s, 625 kB/s, about the 0.55 MB/s seen once the layers' bytes are taken off. Packets over that were dropped, so the ends' QUIC kept seeing losses. Three single fetches through carriers ruled the rest out:

| Fetch | V → P | P's upload figure | Download |
| --- | --- | --- | --- |
| As in the run | through the project's relay | assumed 10 Mbit/s | 202 s |
| P's firewall letting V's address in | direct, on the home network | assumed 10 Mbit/s | 200 s |
| P's figure set by hand | direct, on the home network | 100 Mbit/s | 44.9 s |

The path's length didn't matter: V's leg going direct halved the round trip and changed nothing. With P's figure raised, the carrier path beat relay only (about 58 s). V's processor wasn't the limit either, at most half used. Two changes followed, the maintainer's (the design note's part 5, amended): with no figure from the owner, carrying raises the upload figure while it carries at its whole share and no link backs up, starting from 10 Mbit/s; and a packet over the share waits its turn instead of being dropped. Stage D runs again on every node's own settings.

## Stage D results, second run

**Failed all three marks, 2026-11-22, but the carrier path is 1.6 times faster than in the first run.** Run on `7bb85dd` (the paced share and the upload learned from carrying), on every node's own settings, except that P was V's seed carrier. Same nine fetches in the same order. Every carrier fetch went over two hops. Each figure is the median of three, counted as in the first run.

| Way | Whole file | First byte |
|---|---|---|
| Through carriers | 132.2 s | 17.1 s |
| Relay only | 70.7 s | 9.3 s |
| Relay only, fallback off (meant as n0; see the correction above) | 63.8 s | 7.9 s |

1. **Whole file: failed.** The carrier fetch took 1.9 times relay-only's time (the downloads alone: 112.9 s against 64.2 s, about 1.0 MB/s against 1.8 MB/s), where the mark allows 1.1 times. That's down from 3.4 times in the first run.
2. **First byte: failed.** It came 7.8 s after relay-only's. Counting from the moment V's direct try at H failed, which is the same moment in every way, the first byte took 6.2 s through carriers when no middle was tried (the third fetch), against 1.2 s for relay only.
3. **Until the circuit is usable: failed in two of three.** V took the download from the sync of a followed account, so it never asked the trackers, and the mark's start line (`asking the trackers who hosts it`) wasn't logged. The nearest start is the carrier step's own (`through carriers next`). From there to `the circuit is ready` took 27.2 s, 9.5 s and 4.2 s.

**Where the first byte and mark 3 went: middles that were gone.** In the first two fetches V picked a machine for the middle turn (`turn=Probation`, then `turn=Fresh`), tried a three-hop circuit through it, and only then built the two-hop one. One middle didn't answer and the setup timed out after 22.8 s; the other answered `Gone` after 5.4 s. V still held both from earlier tests, and M had been taken down after the middle-turn test; which of the two was M wasn't checked. Without a middle, building the circuit took 4.0–4.3 s, under the 5 s mark. A gone middle is ordinary on a real network too, so a failed middle costs the whole setup timeout before the two-hop try starts.

**Where the whole file went: P's learned upload stopped rising.** P learned 24.4 Mbit/s during the first fetch, four steps up from the assumed 10, and stayed there through the other two. Half of that is 1.53 MB/s, but the downloads came at about 1.0 MB/s, two-thirds of the share, under the 80 % a spell needs before the figure goes up. P's log was at its normal level, so this is read from the figures and isn't confirmed. With P's figure set to 100 Mbit/s by hand in the first run, the same path took 44.9 s.

## Stage D results, third run

**Passed mark 3, failed marks 1 and 2, 2026-11-22.** Run on `cdbce16` (#276: the middle's circuit raced against the two-hop one, and the upload learned from packets waiting for the share), on every node's own settings, except that P was V's seed carrier. P's carrier log was at debug this time. Same fetches in the same order. Every carrier fetch went over two hops. Each figure is the median of three, counted as before.

| Way | Whole file | First byte |
|---|---|---|
| Through carriers | 104.5 s | 12.9 s |
| Relay only | 67.7 s | 7.3 s |
| n0 only | not run | not run |

1. **Whole file: failed.** The carrier fetch took 1.5 times relay-only's time (the downloads alone: 93.5 s against 61.2 s, about 1.2 MB/s against 1.9 MB/s), where the mark allows 1.1 times. It was 1.9 times in the second run and 3.4 in the first.
2. **First byte: failed.** It came 5.6 s after relay-only's. After V's direct try at H failed, which is the same moment in every way, building the circuit took about 4 s and the first byte 1.8–2.0 s more, against 1.2 s for relay only. The circuit is only started once the direct try has failed.
3. **Until the circuit is usable: passed.** As in the second run, V took the download from a sync and logged no `asking the trackers who hosts it`, so the start is the carrier step's own (`through carriers next`). From there to `the circuit is ready` took 4.3 s, 4.2 s and 3.8 s. In the first fetch V picked a middle (`turn=Fresh`), and the two-hop circuit, built at the same time, was used.

**n0 wasn't run.** The script's change to n0 left the `relay_token` line in V's config, and a node refuses a token when none of its own relays are listed, so V didn't start for that step. The script removes the line now. The configs were put back at the end.

**Where the whole file went: P's learned upload still stops at 24.4 Mbit/s.** P's debug log shows the figure going up four times during the first fetch (12.5, 15.6, 19.5, then 24.4 Mbit/s), each in a spell where packets waited for the share 2.6–4.5 s, and then no more. It logged no link backing up. So above about 20 Mbit/s, packets waited less than half of each spell, while the downloads came at 1.2 MB/s against a share of 1.53 MB/s. With the figure set by hand to 100 Mbit/s in the first run, the same path ran at about 2.5 MB/s, so the share is still what slows it, and waiting packets alone don't show that. Why the ends' traffic stays under the share isn't confirmed: `osp carrier` doesn't show P's dropped packets.

## The share probe, after the third run

One carrier fetch on P's own settings, with P's counters now in `osp carrier` (packets held and for how long, packets dropped with the share's queue full, packets sent to a backed-up link, all drops), read every 5 s. P had restarted, so its learned figure began at the assumed 10 Mbit/s. The whole file took 140.5 s.

- **At 10 to 19.5 Mbit/s:** the share's queue was full and dropped 1,201 packets (892 in the first spell at 10 Mbit/s), and no link backed up. Every drop in the run was one of these. The figure went up four times, but the first rise took about 30 s, and spells with drops didn't always count, as packets then waited less than half the spell.
- **At 24.4 Mbit/s, the last 45 s:** no drops, no packets held, no link backed up. What was carried came to about 1.39 MB/s, 91 % of the share.

So the share was too low and dropped packets while it climbed, and the ends' QUIC backed off from the losses. A spell with packets dropped from a full queue and no link backed up now raises the figure as well. At 24.4 Mbit/s the share wasn't what held the fetch back during those 45 s, and this change doesn't explain or lift that plateau.

## Stage D results, fourth run

**Passed mark 3, failed marks 1 and 2, 2026-11-22.** Run on `22b13ce` (#279: drops from the share's full queue raise the learned upload, and the circuit is built beside the direct try), on every node's own settings, except that P was V's seed carrier. P's and V's logs were at debug. Same fetches in the same order. Every carrier fetch went over two hops. Each figure is the median of three, counted as before.

| Way | Whole file | First byte |
|---|---|---|
| Through carriers | 104.4 s | 11.9 s |
| Relay only | 62.2 s | 7.3 s |
| n0 only | not run | not run |

1. **Whole file: failed.** The carrier fetch took 1.7 times relay-only's time (the downloads alone: 94.6 s against 57.3 s, about 1.2 MB/s against 2.0 MB/s), where the mark allows 1.1 times. The carrier downloads took as long as in the third run (93.5 s). Relay only was faster this time. The first fetch's whole time (549.5 s) includes about 7 minutes of V syncing until it saw the post, so the median uses the other two.
2. **First byte: failed.** It came 4.6 s after relay-only's, down from 5.6 s. The circuit now starts with the direct try and was ready 2.8–3.3 s after that try failed, against about 4 s before. The first byte then took 1.8–2.0 s through the circuit, against 1.1–1.2 s for relay only.
3. **Until the circuit is usable: passed.** The circuit was ready before the carrier step began (`the circuit is ready` logged up to 1 s before `through carriers next`), 3.6–4.1 s after the download started.

**P's learned upload passed 24.4 Mbit/s, and the fetch didn't get faster.** In the first fetch the figure went up five times in 40 s (12.5, 15.6, 19.5, 24.4, then 30.5 Mbit/s). The first three rises came with packets dropped from the share's full queue, and the last two with packets held. Then it stayed at 30.5 Mbit/s, a share of 1.9 MB/s. The second and third fetches started at 30.5 Mbit/s and still carried about 1.2 MB/s, 63 % of the share. So the share isn't what limits the carrier path now, as the probe suggested. The limit is somewhere else on the path.

**n0 wasn't run.** The script's check that both nodes had left the project's relay looked for `osp2p.com` anywhere in the configs, and the hide list's address (`hide_url`) has it too. V's status showed it on n0's relay, so the step would have been n0. The check looks for `relay1.osp2p.com` now. The configs were put back at the end.

## The relayed leg probe, after the fourth run

The first run's three fetches left one case out: V's leg to P through the project's relay with P's figure out of the way. Two carrier fetches on 2026-11-22, P's figure set by hand to 100 Mbit/s, two hops, otherwise as in stage D. The first went with V → P through the project's relay, the second with P's firewall letting V's address in. A capture on V counted its UDP packets to P's home address to confirm each path. With the first run's three, that fills the table:

| V → P | P's upload figure | Download | First byte after the circuit is ready |
| --- | --- | --- | --- |
| through the project's relay | assumed 10 Mbit/s | 202 s | |
| direct, on the home network | assumed 10 Mbit/s | 200 s | |
| direct, on the home network | 100 Mbit/s | 44.9 s | |
| through the project's relay | 100 Mbit/s | 90.9 s | 2.7 s |
| direct, on the home network (again) | 100 Mbit/s | 47.4 s | 1.9 s |

- **Relayed: about 1.25 MB/s,** the same as the fourth run's carrier fetches at P's learned 30.5 Mbit/s. V sent 47 packets to P's home address, only hole-punching tries.
- **Direct: about 2.4 MB/s,** close to the first run's 44.9 s. V sent 60,675 packets to P's home address.
- **P's counters:** no packets held, none dropped with the share's queue full, none sent to a backed-up link in either fetch. One packet was dropped in all, during the direct one.

So with the share out of the way, V's relayed leg to P is what holds the carrier path to about 1.2 MB/s. The share stopped being the limit once P's figure passed about 24 Mbit/s.

**The first byte is a different matter.** Counted from the download's start, it took 1.8 s relayed and 1.9 s direct, against 1.1–1.2 s for relay only, so the relayed leg doesn't explain mark 2. The direct leg did make the circuit faster to build: 2.6 s against 4.1 s. In the relayed fetch the circuit was ready about 0.9 s before V's direct try at H had failed, and the download waited for it.

Nothing on the transport changes from this probe.

## The receive window, before the fifth run

**About 1 MB in flight per round trip on every path, 2026-11-22.** The download debug lines (#282) gave each path's round trip and speed. On the carrier path and on relay only alike, speed × round trip came to about 1 MB, the size of QUIC's default receive window (1.25 MB). So V's window, not the relayed leg as such, set the pace.

**An 8 MiB window confirmed it, and broke the relayed path.** A lab build on V (not kept) raised the stream receive window to 8 MiB, with P's figure at 100 Mbit/s. The carrier fetch went at about 2.3 MB/s for its first 10 s, against 1.2 MB/s before. Then it failed after 28 MB (`no provider could send it`), with H logging `The relay is rate-limiting this endpoint` and V `no available capacity`. The project's relay allows a client 4 MB/s with an 8 MB burst, and 8 MiB in flight goes past the burst.

So #283 set 3 MiB a stream and 6 MiB a connection (`osp_carrier::media_windows`), on each node's own endpoint and on the ends of its circuits. It also opens the connection to the host over the circuit while the direct try runs, for mark 2.

## Stage D results, fifth run

**Failed all three marks, 2026-11-22. The window made relay only 1.7 times faster, and the carrier path didn't keep up.** Run on `edd5375` (#283), on every node's own settings, except that P was V's seed carrier. P's and V's logs were at debug. Same fetches in the same order. Each figure is the median of three, counted as before, against the original marks. The marks were changed after this run ([above](#stage-d-plan-fixed-2026-11-21-before-the-run)).

| Way | Whole file | First byte |
|---|---|---|
| Through carriers | 114.9 s | 61.5 s (13.4 s from the first carrier download) |
| Relay only | 38.7 s | 7.8 s |
| n0 only | failed twice | 0.2 s into the download |

1. **Whole file: failed.** The carrier fetch took 3.0 times relay-only's time, where the mark allows 1.1 times. Relay only got much faster: its downloads took 33.0–33.4 s (about 3.4 MB/s, against 2.0 MB/s in the fourth run). Only two of the carrier fetches went through carriers, and they didn't speed up as much:
   - **Fetch 1 never got a circuit.** V had been restarted for the deploy 2.5 min before. It couldn't reach P (`reaching machine …: timed out`) until 11 min after that restart, so this fetch fell back to the relays after a 60 s wait (`no hop of this node's own could be had`).
   - **Fetch 2** went over two hops: its download took 102.9 s, about 1.1 MB/s.
   - **Fetch 3** went over two hops in two downloads. The first carried 69.3 MB in 36.8 s (1.9 MB/s), and then its connection over the circuit closed (`connection lost: closed`), with nothing in P's or H's log at that moment. V built a new circuit in 4.0 s and the other 44.5 MB came in 26.4 s (1.7 MB/s).
2. **First byte: failed.** The table's carrier figure is counted as before, from the download that finished. That is fetch 3's second download, and the fallback in fetch 1. Counted from each fetch's first carrier download instead, when playing could start, the first byte came 12.6 s and 13.4 s after the fetch began, still 4.8–5.6 s after relay-only's. The connection opened ahead did its part (`connected to the host over the circuit, ahead of the fetch` in both): the carrier download had `connect_ms=0` and its first byte 1.0 s after starting, against 1.1–1.2 s for relay only. What was left is the circuit itself. It was ready 4.3–4.5 s after V began trying sources, and connecting over it took 0.8 s more, while relay only moved on about 1 s in.
3. **Until the circuit is usable: failed in one of three.** In fetches 2 and 3 the circuit was ready 4.3 s and 4.5 s after the download started, before the carrier step began. Fetch 3's rebuilt circuit took 4.0 s. Fetch 1 had no circuit at all. As in the fourth run, V took the download from a sync, so the trackers line wasn't logged.

**Data in flight per round trip** (speed × V's round trip):
- **Relay only:** about 2.1 MB (3.4 MB/s at 605–662 ms), past the old 1 MB.
- **Carrier fetch 2:** 0.9 MB (1.1 MB/s at 787 ms).
- **Carrier fetch 3:** 1.6 MB and 1.4 MB (at 807–827 ms).

The carrier path's speed isn't set by V's window any more. On P, the share's learned upload rose from 30.5 to 59.6 Mbit/s during fetch 3, each rise with packets dropped from the share's full queue. V's path counted 479–512 lost packets in each relay-only download and 37–124 through carriers.

**The 8 MiB failure didn't come back.** V logged no `no available capacity`. H logged `The relay is rate-limiting this endpoint` once, during fetch 1's relayed download, and that download still finished in 33.3 s. A capture on V counted 206 UDP packets to P's home address, all 35-byte hole-punching tries from the moment V reached P. So V's leg to P went through the project's relay.

**n0: two fetches failed the same way, and the third wasn't run.** Both nodes were on n0's relays this time (the check fixed after the fourth run). V took the download from the first source in its list, another node holding the video (`c45240d144`, not H). It connected in 73 ms and had the first byte after 0.2 s. Both times the transfer then stopped at 83.7 MB: the path went silent (round trip read 60 s, no losses), and the attempt ran to its 30 min limit. V didn't move on to H. The script then waited without a limit for the file. After the second identical failure it was stopped by hand, and the configs were put back by hand. n0 isn't graded.

## Stage D results, sixth run

**Watchability passed in one of three, 2026-11-23: the other two carrier downloads broke partway, and the fetch finished over the relay.** Run on `246f62d` (#286: a restarted node reaches its own hop quickly; #287: Tor outlives the node's connections; #288: the link to the own hop kept between circuits), on every node's own settings, except that P was V's seed carrier. P's and V's logs were at debug. Same fetches in the same order. Judged against the marks as changed after the fifth run. Each figure is the median of three. Times are counted from V's `downloading` line, the same moment in every way.

| Way | Whole file | First byte |
|---|---|---|
| Through carriers | 83.5 s | 6.8 s |
| Relay only | 39.3 s | 2.2 s |
| n0 only | failed twice | 0.2 s into the download |

1. **Watchability: passed in fetch 1 only.** Fetch 1's carrier download carried the whole file at 10.9 Mbit/s, 3.5 times the 3.1 Mbit/s mark. In fetches 2 and 3 the carrier download broke twice each, after 22.0 MB and 4.7 MB, then 5.6 MB and 8.9 MB, at 3.6–7.7 Mbit/s while it ran. Each time V built a new circuit, and after the second break it took the rest over the relay. The script's `watch:` lines for those two (23.6 and 27.8 Mbit/s) measured that relay download, not a carrier one: it took the first finished download after the carrier step began. It now checks that the download went through carriers.
2. **Startup: open, not met.** The carrier path's first byte came 4.1 s, 6.8 s and 16.0 s after the download started, against 2.0–2.2 s for relay only. Fetch 2 was 1.9 s behind relay only, within the old mark's 2 s.
3. **The circuit: built faster, but not always started at once.** On the kept link the build took 1.6–2.0 s (five builds), against 4.3–4.5 s in the fifth run. Fetch 1's circuit was built on a fresh link, raced against a three-hop one through a middle that answered `Gone`, and took 4.3 s. Counted from the download's start, the circuit was ready after 13.3 s, 2.0 s and 4.8 s. In fetches 1 and 3, 9.0 s and 2.8 s passed before the build began, with nothing in V's log. Before building, V finds its own hop's descriptor, and a failed carrier fetch makes it ask again next time. That fits the gaps (fetch 1 was the first since the restart, and fetch 3 followed fetch 2's failures), but it isn't confirmed, because the ask isn't logged when it succeeds. Nothing went the 11 minutes without a hop seen in the fifth run.

**Why the carrier downloads broke.** Fetch 2's two ended with `connection lost: closed`, as fetch 3 did in the fifth run. Fetch 3's two ended with V's own QUIC giving up: `too many gaps in stream buffer`. That is noq's limit of 1,024 separate pieces waiting in one stream's receive buffer, so many of the packets on their way to V never came. The loss counts in V's download lines don't show it, because they count what V sent. On P, `osp carrier` counted 3,650 packets dropped with the share's queue full, and 462 sent to a backed-up link, since its start before the run. During fetch 2, P lowered its learned upload from 47.7 to 38.1 Mbit/s after a link backed up. V logged 186 `no available capacity` drops from the project's relay at one moment in fetch 2, and none in fetch 3. Fetch 1 went on a fresh link, which then became the kept one, and all four that broke went on that kept link. With one success, that doesn't tell the kept link apart from P's lowered share, and the fifth run's break was on a fresh link. Where the packets were lost isn't confirmed.

Relay only carried the file in 33.4–34.3 s each time, as in the fifth run.

**n0: failed as in the fifth run, and the third fetch wasn't run.** Both fetches went to `c45240d144` and stopped at 83.7 MB until the 30 min limit. The run was stopped by hand before the third, since it wasn't graded and the next run puts debug logs on more of the hops. The configs were put back by hand.

## The counter probe, after the sixth run

**The carriers' counters don't account for the loss, 2026-11-23.** One carrier fetch, V on #291 (each hop's descriptor logs where it came from and how long it took), with every lab node's `osp carrier` counters read before and after. P and Q (relay1's node) were the hops, as in the sixth run, each carrying about 40 MB. The fetch went as fetches 2 and 3 did: the carrier download broke after 12.6 MB (`too many gaps in stream buffer`) and again after 21.5 MB (`connection lost: closed`), then the relay carried the rest. The whole file took 70.6 s.

| Hop | Held for the share | Dropped, share full | Sent to a backed-up link | Dropped in all |
|---|---|---|---|---|
| P | 3,028 (1.2 s) | 106 | 0 | 106 |
| Q | 0 | 0 | 0 | 0 |

No other node's counters moved. The first break needed more than 1,024 holes in V's stream within 12.6 MB, and the carriers dropped 106 packets over the whole fetch. So most of the loss is where no counter looks: noq's datagram queues, which drop silently (on the send side the oldest waiting datagrams go to make room, on the receive side what isn't read in time), V's own queue between reading the link and its circuit's QUIC (`INBOX`, 1,024 packets, also dropped silently), or the relay between them. V is the emulated arm64 VM, and in the sixth run it logged `no available capacity` from the relay, so V not keeping up is the first suspect. It isn't confirmed. The first circuit went on a fresh link and broke the same way, so the kept link isn't the cause. P's learned upload rose from 47.7 to 74.5 Mbit/s.

**Two more fetches, the same day.** Both broke twice in the same two ways before the relay finished them.

- **The first:** no carrier counted a drop or a backed-up link, on any node.
- **The second, with CPU read each second:** P counted 1,075 packets sent to a backed-up link, and no drops, and its learned upload went down from 74.5 to 59.6 Mbit/s. V's `osp` used up to 2.7 of the VM's 4 cores while the carrier downloads ran (8–15 Mbit/s), and 1–2 cores for the relay download (about 25 Mbit/s). The emulator's process on the desktop peaked at 2.8 cores. So the VM wasn't out of cores, but the carrier path costs V several times the processor per byte that the relay path does, and one thread at its limit wouldn't show in these totals. A link backed up at P without P dropping anything fits noq making room in a full send queue by dropping the oldest datagrams, which no counter sees.

**The descriptor:** at the start it came from the cache in 0 ms. After the first break, it was asked of P's carrier endpoint again, which took 2.5 s, about the 2.8 s gap before fetch 3's build in the sixth run. A carrier fetch that fails forgets the hop's descriptor, so the next build waits for it. The 9.0 s gap of the sixth run's first fetch didn't recur here.

## The queue probe, after the counter probe

**Nothing in the nodes' own queues dropped the packets, 2026-11-23.** #292 counts what could go missing unseen: a packet dropped at a full circuit-end inbox (`viewer_inbox_full`, `host_inbox_full`, exact), and a send that found less room in a link's datagram queue than its frame (`send_queue_full`, at a carrier and at each end). Then QUIC discards the oldest waiting and says nothing, so that counts the pressure, not how many went. V, P, Q (relay1's node) and H's machine ran it. One carrier fetch, P's upload set to 100 Mbit/s by hand so its share stayed out of the way (put back after), and V's busiest thread read each second.

- **The fetch broke twice again,** both times `too many gaps in stream buffer`, after 16.2 MB and 29.8 MB, and the relay carried the rest.
- **Every new counter stayed at 0** on every node: no inbox drops at V or H, and no send that found a queue full. P counted no drops of its own. Q counted 374 packets sent to a backed-up link, with no drops.
- **V logged 196 `Dropping received relay packet: no available capacity`** during the second break: iroh dropping packets from the relay before V's QUIC saw them. None during the first.
- **V's processor:** its busiest thread peaked at 85 % of a core, and none stayed at its limit.
- **relay1's server** has dropped 55 packets since it started, in all.

**The same loss in a test on one machine.** In the new test, the host sends 3,000 packets to a viewer whose end reads nothing until the last is sent. P carried all 3,000 on, and then the viewer's link connection took in only 1,900–2,000 of them in some runs. Each one it took in was read or counted at the inbox, exactly. So the loss is between P's QUIC and the viewer's, and in the test that's the socket (QUIC doesn't send a datagram again). In the lab the links go through the relay, and V's `no available capacity` is the same kind of loss: more arriving than its endpoint takes in at once.

So the packets go missing between one QUIC and the next, below anything a node counts, when a burst arrives faster than the receiving endpoint drains it. The ends' QUIC then sees more than 1,024 holes within its 3 MiB window and gives up. It isn't V's processor at its limit, and it happens on an ordinary machine too, so it isn't only the emulated VM.

## The newcomer's middle turn

**Passed, 2026-11-21.** `osp machine hello` doesn't add a machine to the table V picks middles from, so Middle arrived through ordinary meeting. Then Empty posted a fresh 20 s clip (4.0 MB) and Robin fetched it.

- **Middle had the middle:** Robin logged `a machine has the middle turn=Fresh` for Middle, then `a machine is taken on probation` and `fetched through carriers hops=3`: H → Q → M → P → V.
- **H** talked only to Q's relay (and Tor), as in the two-hop run. Nothing went to the home network.
- **Q**'s carrier had one direct peer during the fetch: M, which is the next hop. Q sees M's address, as a neighbour on the path does.
- **M → P** didn't pass through Q's machine. Both are on the home network.
- **V** talked to Q's relay, which carried its leg to P, and sent a few small hole-punching packets to P, its own hop. It sent none to M.
- **Work offers:** Robin logged that Middle's first job may be offered on. The lines for asking for and taking work offers (`asked a machine at standing for work offers`, `took a work offer`, `turn=Offered`) need three days of standing, so they aren't seen yet.

**Observed, not graded:** the download took 15.5 s, with its first byte after 2.0 s. The fetch command took 23.6 s, including the circuit offer and building the circuit.

A first try at this ran for 3.5 h without a middle, for two reasons that came from the test itself. It posted the same clip each round, so after round 1 Robin already held the blob. And the clip was under 1 MiB, and files that small skip carriers (`SMALL_BLOB`).
