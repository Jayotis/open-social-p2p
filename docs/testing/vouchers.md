# Replies let in by who vouched, in the simulator

**Status:** [ADR 0029](../decisions/0029-vouchers-within-six-steps.md)'s step 4, run on 2026-11-13 ([results](#results)): the marks were met after two changes the first rounds led to, and `vouchers` is now on by default. The scenario and pass marks are the ADR's ("Checking it"), set before the run was written; the mark for work was raised from 50 to 150 ms after the first round.

Kim's node lets a stranger's reply in when enough people in her circle's world vouched for the stranger, and the farther out, the more of them it needs. This run checks that, among a few hundred people, popular creators and hunters from other communities get in from 4 to 6 steps away, and an operator with throwaway identities doesn't. It also checks what a meeting costs.

## The run

`osp-vouch` starts 300 nodes in one process, each on a fast link of the [simulated network](network-conditions.md#the-simulated-network), with the [Tor-like delays](network-conditions.md#messages-over-tor) on messages that `osp-meet` and `osp-delete` use. Every node is a real node; only the operator's is made by hand.

**The world.** 10 communities of 30 people sit in a ring. Everyone follows 8 people in their own community and 1 in a neighbouring one. A first try with that one follow going to any community made the world too small: every popular person was 2 or 3 steps from every creator, so the ring is what lets communities far apart be far apart.

- In each community, **one creator** has `vouchers` on. Their replies are the ones watched.
- **2 popular creators** in each community (20 in all) are followed by at least 18 people in their own community and 6 in each of the 2 neighbouring ones, and everyone who follows them reshares their post.
- **1 hunter** in each community (10) is followed the same way, and also reshares popular creators' posts in its own and nearby communities before anyone else. Each post with 10 resharers credits its first 3 finders.
- Everyone else reshares the post of each ordinary person they follow 3 times in 10.

By this plan, each of the 30 popular people is vouched for by at least 31 people across 3 communities. **Each popular person replies to a creator in another community that the plan puts 4 to 6 steps away** (19 at 4, 9 at 5, 2 at 6). **50 ordinary people** (5 in each community) reply to a creator in another community picked at random.

**The operator** holds 200 throwaway identities. Each vouches for the operator, and for the next throwaway. **One careless person**, an ordinary member whom the first creator's node places 3 steps away through her circle, reshared the first throwaway, which puts the operator 5 steps from that creator. The operator replies to every creator and, asked, shows the fullest side it can: the 200 vouches for itself, the careless reshare, and whoever vouched for the careless person, which that person's node tells anyone who asks, topped up to 256 with the throwaways' vouches for each other.

**In order:** everyone follows and syncs; hunters reshare first; everyone else reshares; authors' nodes hear of every reshare, and hunters' nodes get their credits; everyone syncs again, so followers' nodes hold the reshares; each replier's node gathers its side, as it does once a day; each replier follows the creator they answer, to read the post, and replies, four at a time.

A node tells an author's node of a reshare at once and, if that fails, on its next pass, which comes a day later. One reshare in about 200 failed at once in a trial run, so while the harness waits it has every node make that pass each minute.

**Measured:** from each creator's node, every meeting: the side's size, the circle's nodes asked and those that answered, its own work (checking the side and meeting it, not the wait for the circle), and what it found. From each replier's node, whether the reply was published, and how many nodes gathering its side asked.

**Pass marks** (ADR 0029's):

1. No throwaway reply is published.
2. At least 80% of the popular strangers' replies are published.
3. A meeting asks at most 16 questions, and takes at most 150 ms of work on an SBC-sized node (50 ms before the first round).
4. Gathering one's side costs under 100 requests a day.

**An SBC-sized node** is taken as 5 times slower than the desktop the run is on (a Ryzen 5 3600XT): roughly a Raspberry Pi 4's core against it. An estimate; no SBC was at hand. The desktop runs 300 nodes at once, so its own times are, if anything, high.

Not simulated: Tor itself; reshares older than a day; nodes offline; an operator with more than one careless reshare, or with real people's reshares bought.

### Running it

```sh
cargo build --release -p osp-sim --bin osp-vouch
ulimit -n 524288   # many nodes in one process need many open files
target/release/osp-vouch --dir /path/on/disk/vouch --out vouch.json \
  --label "$(git rev-parse --short HEAD)" > vouch.log 2>&1
```

The run above is the defaults. `--plan` prints the plan and starts no nodes. `--communities`, `--members`, `--popular-creators`, `--hunters`, `--ordinary-repliers`, `--follows-in`, `--follows-out`, `--popular-own`, `--popular-out`, `--popular-communities`, `--ordinary-reshare`, `--throwaways`, `--no-tor-delay` and `--seed` change it. The summary also shows what other tables would have let in, from each meeting's separate points at every distance. `--dir` must be empty, and is deleted afterwards. It exits with an error if a mark is missed.

## Results

Each round took about 50 minutes: 40 to set up (most of it gathering sides, each replier's node asking up to 80 nodes in turn, 10 to 25 s for each first contact), and 5 for the replies.

### First round: the table as accepted

The marks for throwaways, popular strangers, questions and gathering were met: no throwaway reply was published, all 30 popular strangers' were, a meeting asked at most 11 of the circle's nodes, and gathering a side asked at most 80. **The mark for work was missed:** at most 22 ms on the desktop (median 17), about 111 ms on an SBC-sized node.

Nearly all of that is checking the side's signatures, about 54 µs each, so 14 ms for a full side of 256; the checks for revoked keys take 1 ms, and the meeting itself under 1 ms. A side can't be checked in 50 ms on an SBC one signature at a time. A creator's node checks each waiting request's side once and keeps at most 100 waiting, so a flood costs an SBC about 11 s. **The maintainer raised the mark to 150 ms.**

The round also showed a gap in how meeting points were counted. The operator's side holds whoever vouched for the careless person, 3 steps back, and each of them that the creator's world placed counted as a meeting point of its own: the operator met two creators' worlds at 5 points. Short of the 10 needed then, but a careless person better vouched-for near the creator would have let it in. **The maintainer decided that meeting points count only as far as their chains to the replier are separate,** sharing nobody but the replier.

### Second round: separate chains

With the same world, every throwaway reply met at most 1 point. But **14 of 30 popular strangers got in:** far away, their meeting points reach them through the few who vouched for them from near the creator, and they met the creator's world at 4 to 8 separate points, against 5 at 4 steps and 10 at 5 or 6.

A third round recorded each meeting's separate points at every distance, to see what other tables would let in:

| Points needed at 2–3 / 4 / 5–6 steps | Popular, of 30 | Ordinary, of 50 | Throwaway, of 10 |
| --- | ---: | ---: | ---: |
| 3 / 5 / 10 (as accepted) | 14 | 1 | 0 |
| 3 / 5 / 8 | 18 | 1 | 0 |
| 3 / 4 / 8 | 22 | 2 | 0 |
| 3 / 4 / 6 | 27 | 2 | 0 |
| 3 / 4 / 5 | 28 | 2 | 0 |
| 3 / 3 / 4 | 29 | 6 | 0 |
| 2 / 3 / 4 | 29 | 6 | 0 |

**The maintainer chose 3 / 4 / 6,** the strictest that meets the mark. Each separate chain needs another real person's vouch, so a point counts for more than a different person did.

### Confirming round: a world the table wasn't picked from

The same scenario with another seed (`--seed 2`), whose careless person has 4 vouchers near the first creator against 2 before. **Every mark was met.**

| | Replies | Published | Fewest steps, median | Separate points, median |
| --- | ---: | ---: | ---: | ---: |
| Popular strangers | 30 | 28 | 3 | 9 |
| Ordinary strangers | 50 | 3 | 4 | 2 |
| The operator | 10 | 0 | 6 | 1 |

- **Throwaways:** each reply met the creator's world at 1 point at most, through up to 11 people in the circle; at 6 of the 10 creators it met it at all.
- **Popular strangers:** 28 of 30 published. The two that waited met at 4 steps with 5 and 4 points; they waited for want of points nearer than their farthest.
- **Ordinary strangers:** 3 of 50 published, one of them without a meeting: the creator follows its replier, from the neighbouring community, so it was published as from the creator's circle.
- **Questions:** at most 13 of the circle's nodes a meeting.
- **Work:** at most 24 ms on the desktop (median 17), about 120 ms on an SBC-sized node.
- **Gathering a side:** at most 80 nodes asked, the node's own limit; the median side held 114 people.

### Found on the way

- A node that can't tell an author's node of a reshare at once, as when that node is busy, tells it on its next pass, a day later. About 1 reshare in 200 waited so in a trial run. By design; the harness brings the pass forward.
- With follows outside one's community going to any community, every popular person was 2 or 3 steps from every creator, so the run puts communities in a ring.

### On real nodes

Checked on three nodes over Tor, 2026-11-13. A reply from someone near the creator's world was published. A careless person's reply waited among the requests. With the creator's node offline, then one of its circle's nodes silent, the replies were still placed. The circle node that did answer still counted, and the replier's node sent again about 10 minutes after the creator's node was back. A replier far away wasn't tried on real nodes: the rounds above cover it.
