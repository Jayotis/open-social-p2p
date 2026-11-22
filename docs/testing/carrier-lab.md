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

Relay only and n0 only were within 2 % of each other, so the project's relay isn't the slow part. The extra time is in the carrier path itself.

**The cause: P's upload share.** P is a home node set up before `osp init` asked the upload speed, so it carried at half of an assumed 10 Mbit/s, 625 kB/s, about the 0.55 MB/s seen once the layers' bytes are taken off. Packets over that were dropped, so the ends' QUIC kept seeing losses. Three single fetches through carriers ruled the rest out:

| Fetch | V → P | P's upload figure | Download |
| --- | --- | --- | --- |
| As in the run | through the project's relay | assumed 10 Mbit/s | 202 s |
| P's firewall letting V's address in | direct, on the home network | assumed 10 Mbit/s | 200 s |
| P's figure set by hand | direct, on the home network | 100 Mbit/s | 44.9 s |

The path's length didn't matter: V's leg going direct halved the round trip and changed nothing. With P's figure raised, the carrier path beat relay only (about 58 s). V's processor wasn't the limit either, at most half used. Two changes followed, the maintainer's (the design note's part 5, amended): with no figure from the owner, carrying raises the upload figure while it carries at its whole share and no link backs up, starting from 10 Mbit/s; and a packet over the share waits its turn instead of being dropped. Stage D runs again on every node's own settings.

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
