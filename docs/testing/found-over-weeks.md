# Found posts over weeks, in a model

**Status:** Plan for [ADR 0025](../decisions/0025-shared-posts-are-found-and-hosted.md)'s step 5, fixed 2026-11-11 before any code or run; the maintainer accepted the pass marks the same day. Run the same day ([results](#results)): all three marks met at 10%, on every seed. But at every rate the 3 resharers listed first served 89 to 100% of a video, and the rest almost nothing. Each viewer now takes the hosts in its own order, which brought that to 22 to 89%; its own mark missed by a point on one seed, and the change was kept ([below](#each-viewer-in-its-own-order)). Changing one later needs a dated note here saying what changed and why.

Step 5 asks whether sharing with hosting is worth having: when strangers find a creator's video over weeks and some share it, do the sharers take the load off the creator? [Step 4](found-and-hosted.md#step-4-newcomers-in-osp-sim) ran real code for 200 newcomers over 3 hours. Weeks of thousands of viewers is too long for that, so this runs in `osp-month`'s model ([hops model](hops-model.md#why-a-model)), which follows each transfer by the rules and is checked against real runs (its mark 0).

## Who's in it

- **10 creators,** each with one 52 MB video posted on day 0, at 20 Mbit/s up and 100 down. They stay on.
- **1,000 newcomers,** arriving evenly over 28 days, following nobody, at 5, 10 and 20 Mbit/s up in turn and 50, 100 and 300 down. Each opens 3 of the 10 videos, picked by popularity: the creator ranked *k* is picked in proportion to 1/*k*. So the most popular video gets roughly 6 times the viewers of the least, and every video gets well over 50.
- **Sharing with hosting:** each time a newcomer opens a video, it shares it with hosting with chance *p*: 5%, 10% and 20%, one run each. Real rates are unknown, so these are inputs.
- **Not in it:** creators' followers. Their copies are never named to strangers (ADR 0025), so they don't change what strangers get, only the creator's total upload.

## What the model follows

As built today, for Alice opening Kim's video:

1. **The directory's hosts.** Alice reads the directory, which lists up to 16 hosting resharers of the post (as built, a 17th is left out). It takes the first 3 at once, then the others one at a time ([#130](found-and-hosted.md#step-4-results)).
2. **Then the hosts Kim names,** asked at the same time: Kim's media endpoint names up to 6 hosting resharers, by a daily draw.
3. **Kim's own node last.**

- **A new sharer is listed 0 to 2 hours after sharing:** it asks within the hour, and the directory checks its copy at its next hourly read. Kim's node names it seconds after it tells her.
- **Every transfer goes through the relay**, as viewing endpoints and media endpoints are relay-only by default: 8 Mbit/s for each endpoint after a 2.7 MB burst.
- **Places and busy:** each node serves one transfer for each 1.5 Mbit/s of upload and refuses more as busy, as in the hops model.
- **A viewer that doesn't share** keeps its copy in its cache but never serves it.

## What's recorded

For each video:

- **Bytes from resharers:** the share of what its viewers received that came from resharers, for its first 50 viewers and for the viewers after them.
- **The creator's copies per viewer,** for the first 50 viewers and for the viewers after them.
- **One resharer's share:** the most any one resharer served of the post's bytes. ADR 0025 says to revisit the design if one resharer serves most of a post's strangers.
- **Opening times,** median and slowest, and any viewer that couldn't open it.

## Pass marks

Judged in the 10% run, on seed 1; seeds 1 to 5 give how many meet each mark, as context. The 5% and 20% runs are recorded, not judged.

1. **Resharers carry the load:** for every video, at least half of the bytes received by viewers after its 50th come from resharers.
2. **The creator's cost per viewer falls:** for every video, the creator's copies per viewer after its 50th viewer are at most half those for its first 50.
3. **Nobody can't watch:** every open succeeds.

If all three are met, the model's half of step 5 passes and the scripted alpha exercise follows. If any is missed, the results go to the maintainer before anything changes. If one resharer serves more than half of a post's bytes, that's reported as ADR 0025's revisit condition, whatever the marks say.

## Results

`osp-month found` in `osp-sim`, 2026-11-11, at 2e4bac0: one round, all three rates, seeds 1 to 5, 14 s in all. A run repeats exactly for its seed. The top 3's share was added afterwards as a recorded figure (e06f413); the rerun gave the same figures otherwise.

**The marks, at 10%, seed 1: all met, and on all 5 seeds.**

| Video | Viewers | Sharers | From resharers, after the 50th | Creator's copies per viewer, first 50 / after | Creator's copies | Top resharer / top 3 |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 759 | 99 | 100% | 0.04 / 0 | 2 | 34% / 100% |
| 2 | 508 | 48 | 100% | 0.02 / 0 | 1 | 35% / 100% |
| 3 | 402 | 36 | 100% | 0.20 / 0 | 10 | 47% / 98% |
| 4 | 277 | 33 | 100% | 0.20 / 0 | 10 | 45% / 96% |
| 5 | 254 | 23 | 100% | 0.04 / 0 | 2 | 37% / 99% |
| 6 | 185 | 16 | 100% | 0.34 / 0 | 17 | 40% / 91% |
| 7 | 185 | 11 | 100% | 0.08 / 0 | 4 | 48% / 98% |
| 8 | 154 | 10 | 100% | 0.08 / 0 | 4 | 45% / 97% |
| 9 | 146 | 18 | 100% | 0.02 / 0 | 1 | 39% / 99% |
| 10 | 130 | 23 | 100% | 0.02 / 0 | 1 | 43% / 99% |

Every open succeeded: median 19 to 28 s from asking for sources, slowest 91 s. That leaves out reading the directory over Tor, so it isn't comparable with step 4's opening times.

**At 5% and 20%** (recorded): the marks' figures were met on every seed there too. At 5%, on seed 1, the creator sent at most 53 copies of one video (video 8, whose first 50 viewers found no sharer listed yet), and on 3 of 5 seeds one resharer served over half of some video's bytes, ADR 0025's revisit condition, where a video had only 3 sharers.

**The 3 listed first carry nearly everything.** At every rate, the 3 resharers that served most served 89 to 100% of a video (66% for one video at 5%). On video 1 at 10%, 99 newcomers shared it and 3 of them sent its 759 copies, about 13 GB each over the 28 days. The other 96 sent close to nothing. That's because every viewer reads the same list and takes its first 3 at once; with viewers arriving one at a time, those 3 always have a free place. So sharing takes the load off creators, as the marks say, but it doesn't spread it: a few early sharers carry a popular video, and the rest share for nothing. ADR 0025's daily rotation, built only once a directory holds more than 16, would move it to another 3 each day.

**What the model leaves out:**

- **Each video runs on its own.** A newcomer that hosts two videos serves each with its whole upload. At 10%, about 3% of newcomers host more than one.
- **Viewers arrive evenly.** A video found by many at once would queue for places, which [step 4](found-and-hosted.md#step-4-results) and B8 cover.
- **Nodes stay on.** A host that goes off is replaced by the next listed, which the model doesn't test.

## Each viewer in its own order

Added 2026-11-11, after the results above, and fixed before the change was built. The maintainer chose to have each viewer shuffle the hosts it's given before taking 3 ([ADR 0025](../decisions/0025-shared-posts-are-found-and-hosted.md#rotation-and-no-one-dominating), amended). The model gets the same change, and runs once more, as above.

**Pass mark,** at 10%, seed 1: marks 1 to 3 are still met, and on every video with at least 16 sharers, a directory's full list, the 3 resharers that serve most serve at most half of its bytes.

**Results,** at 9be41e7, one round as above. `--same-order` repeats the first run exactly.

- **Marks 1 to 3: met** at 10%, on all 5 seeds, with the same figures for them as before to the copy. Opening times are within a few seconds of the first run's.
- **The top 3's mark: missed** on seed 1, met on 3 of 5 seeds. Video 9, with 18 sharers, had its top 3 serve 51%.
- **The load spread out.** The top 3's share of each video:

| Sharing | Every viewer in the order given | Each in its own order |
| --- | --- | --- |
| 5% | 66 to 100% | 30 to 89% |
| 10% | 91 to 100% | 23 to 67% |
| 20% | 96 to 100% | 22 to 38% |

On video 1 at 10%, the top resharer's share fell from 34% to 8%, and the top 3's from 100% to 23%. What's left comes from timing: a sharer listed early is a choice for every later viewer, one listed late for a few, so on a smaller video the first sharers still serve more.

**Decision** (the maintainer, 2026-11-11): the change stays, with the miss recorded. Whether to give hosts listed later a better chance is a design question for later.
