# The follower swarm on slow and flaky connections

**Status:** The home run met every mark on 2026-10-25. The lossy and flaky runs missed the followers' marks the same day: followers whose download took too long gave up and never tried again, as many did in the slow-hosts run. After a fix the same day (`9f72b4f`), every follower got the video in every run: the home and flaky runs met every mark, and the lossy run every mark but 95% of followers within 15 minutes, which took 18 ([results](#results)). Each run is being [repeated](#repeats) to show how much its figures vary. The runs and pass marks below were fixed on 2026-10-25, before anything was measured, and the runs after the fix used them unchanged. Changing one later needs a dated note here saying what changed and why.

The [B8 runs](b8-creator-upload.md) showed the follower swarm sparing a creator with 300 followers, but over loopback: no delay or packet loss, unlimited downloads, every node always reachable, and each node's upload held down by its own upload cap, a setting real nodes leave off. Stage 1 asks for transfers to be measured across fast, slow and unstable peers ([roadmap](../06-roadmap.md)), and [doc 12](../12-peer-reciprocity-and-recognition.md#validation) asks for slow hosts alongside a fast one to be simulated before peers are chosen by what's been measured about them. These runs repeat B8 on a simulated network of home connections.

## The simulated network

`osp-sim --net` runs the nodes over a simulated network in the same process, instead of loopback. Each node's connection to it has:

- an **upload speed and a download speed**;
- a **delay** each way between the node and the middle of the network. A packet from one node to another takes both nodes' delays, so their round trip is twice the sum;
- a **chance of losing each packet**, on the way out and the same on the way in.

A packet from A to B waits its turn in A's upload queue, travels A's delay and B's delay, then waits its turn in B's download queue. Each queue holds up to 100 ms of packets at its connection's speed; a packet that would wait longer is dropped, as a full home router drops it. So a busy connection first gets slower, then loses packets, and QUIC's congestion control backs off, as on a real line.

A connection can also be **cut off** and restored. While it's cut off, everything it sends and everything sent to it is lost. QUIC sends again once it's back, if it hasn't given up by then.

The nodes are unchanged, and their QUIC connections run over the simulated network through iroh's custom transports. Only builds with the `sim` feature, the harness's and the tests', have it. With `--net`, nodes set **no upload cap**, as a node doesn't unless its owner sets one, and their connections hold their upload to its speed instead; `--caps` sets the caps as on loopback.

Not simulated: delay that varies, and packets arriving out of order; losses in bursts; a bottleneck in the middle of the network that several nodes share; NAT, hole punching and relays; and phones. Losses follow a seed (`--seed`), but timings vary from run to run, so two runs with the same seed aren't identical.

Tests in `osp-node` check delays, speeds, losses and cut-offs on real QUIC connections over the simulated network, and two nodes following each other over it. CI also runs a small `osp-sim` run over it: 12 followers on slow and fast connections with delays and some loss, two of them cut off now and then, and a latecomer.

### Running it

```sh
cargo run --release -p osp-sim -- --dir /path/on/disk/run --swarm --net \
  --latecomers 10 --label "$(git rev-parse --short HEAD)" --out run.json
```

With `--swarm --net --latecomers 10` and no other options, that's the [scenario](#the-scenario) below. Upload speeds are `--creator-mbps` and `--follower-mbps`, as on loopback. With `--net`:

- `--creator-down-mbps`, `--creator-delay-ms`, `--creator-loss`: the creator's download speed in Mbit/s, delay in milliseconds each way, and the percentage of packets lost each way.
- `--down-mbps`, `--delay-ms`, `--loss`: the same for followers, given in turn, as their upload speeds are; strangers and latecomers get them too. Lists of different lengths mix: with three upload speeds and four delays, every upload speed comes with every delay.
- `--caps`: nodes cap their upload at their connection's speed.
- `--flaky 30`: 30 followers' connections are cut off for `--outage-secs` (30) in every `--outage-every-secs` (300) after posting, taking turns. They're the middle one of each group of `followers / flaky`. Outages end with the run, before any latecomers.
- `--seed`: the random losses follow from it.

The summary adds the network's settings, and what happened to the packets sent through it: the share lost, dropped by full queues, or lost while cut off.

## The scenario

B8's swarm run, with downloads in parts, on home connections:

- One creator and **300 followers**; the creator posts a **52 MB video**. Followers share what they prefetch and spread their fetches over up to 10 minutes, as a real node with `share_prefetched` on does. Each run ends when every follower holds the video, or an hour after posting.
- **Upload and download speeds:** the creator 20 Mbit/s up and 100 down. Followers in turn 5 up and 50 down, 10 and 100, 20 and 300.
- **Delays each way:** the creator 20 ms; followers 10, 20, 30 and 40 ms in turn, so every speed comes with every delay. Round trips between followers are 40 to 160 ms, and to the creator 60 to 120 ms.
- **No packet loss and no outages,** except in the runs that add them.
- No upload caps, except in the run that adds them.
- Then **10 latecomers** follow the creator one after another and fetch the video at once, as in B8's runs in parts.

## Runs

| Run | Changes to the scenario | What it shows |
| --- | --- | --- |
| **Home** | none | **judged**: whether the swarm's B8 marks hold on home connections, with nodes setting no upload cap |
| Home, upload caps | `--caps` | whether nodes capping their upload, which also gives a slow connection fewer transfers at once, changes the spread |
| **Lossy** | creator 0.5% loss; followers 0, 0, 0.5, 1 and 2% in turn (`--creator-loss 0.5 --loss 0,0,0.5,1,2`) | **judged**: whether losses, up to a poor Wi-Fi's, slow the spread past the marks |
| **Flaky** | 30 followers cut off for 30 s in every 5 minutes (`--flaky 30`) | **judged**: whether connections dropping out and coming back slow the spread past the marks, for the flaky followers too |
| Slow hosts, a fast one in ten | followers upload at 1 Mbit/s, one in ten at 100 (`--follower-mbps 1,1,1,1,1,1,1,1,1,100`) | measured, not judged: where things start before nodes choose hosts by what they've measured of them ([doc 12](../12-peer-reciprocity-and-recognition.md) phase 2). The latecomers' times are the figure to beat |

## Pass marks

Each judged run passes when **all** of B8's marks for the swarm hold:

1. **The creator's upload stays low:** it sends at most **30 copies** in total, a tenth of the 300 it would send alone.
2. **Followers get the media promptly:** 95% of followers hold the verified file within **15 minutes** of posting, and every follower within **30 minutes**. In the flaky run, that includes the flaky followers.
3. **The creator is spared by latecomers:** it sends them at most **0.1 copies** between them, room for their thumbnails only.

Recorded too, without a pass mark: the latecomers' times, against 14.1 s for half and 23.6 s for all on loopback; busy refusals and failed downloads; and what the network did with packets.

### Repeats

Added on 2026-10-25, before any repeat was run. One run of each leaves open how much a figure varies from run to run, so the runs are repeated: the home run five more times with the fix (`9f72b4f`, [below](#results)) and five times with `8533ea3`, the same code without it; the lossy and flaky runs five more times each, and the slow-hosts run twice more, all with the fix. They run one at a time on the same machine, in rounds of home without the fix, home with it, lossy and flaky, so that both versions of the code share whatever else the machine is doing.

Each repeat is judged on the same marks. A judged run counts as meeting a mark only when every one of its runs, the first included, meets it. The results give how many runs met each mark, and the median and range of each figure.

## Results

| Date | Run | Code | Followers holding the video | 50% / 95% / all | Creator sent | Pass marks |
| --- | --- | --- | --- | --- | --- | --- |
| 2026-10-25 | **Home** | `4a6ad0e` | 300 of 300 after 15 minutes; latecomers: half within 17.1 s, all within 26.6 s | 587 s / 775 s / 893 s | 25.0 copies (1,306 MB), all in the first 12 minutes; 247 requests refused as busy; latecomers 0.01 | **all met** |
| 2026-10-25 | Home, upload caps | `4a6ad0e` | 300 of 300 after 14 minutes; latecomers: half within 17.2 s, all within 25.0 s | 553 s / 727 s / 802 s | 19.0 copies (990 MB), all in the first 10 minutes; 190 requests refused as busy; latecomers 0.01 | all met (not judged) |
| 2026-10-25 | **Lossy** | `4a6ad0e` | 283 of 300 after an hour, the last of them at 21 minutes; latecomers: half within 115.4 s, all within 252.7 s | 679 s / not reached / not reached | 18.4 copies (961 MB), all in the first 15 minutes; 235 requests refused as busy; latecomers 0.01 | creator met; **95% and all missed** |
| 2026-10-25 | **Flaky** | `4a6ad0e` | 260 of 300 after an hour, the last of them at 21 minutes; latecomers: half within 15.8 s, all within 17.9 s | 900 s / not reached / not reached | 23.5 copies (1,228 MB), all in the first 17 minutes; 294 requests refused as busy; latecomers 0.01 | creator met; **95% and all missed** |
| 2026-10-25 | Slow hosts, a fast one in ten | `4a6ad0e` | 175 of 300 after an hour, the last of them at 20 minutes; latecomers: half within 145.3 s, all within 150.6 s | 984 s / not reached / not reached | 17.2 copies (897 MB), all in the first 10 minutes; 133 requests refused as busy; latecomers 0.01 | not judged; the followers' marks would be missed |
| 2026-10-25 | **Home** | `9f72b4f` | 300 of 300 after 14 minutes; latecomers: half within 16.0 s, all within 18.3 s | 633 s / 814 s / 863 s | 23.7 copies (1,236 MB), all in the first 13 minutes; 168 requests refused as busy; latecomers 0.01 | **all met** |
| 2026-10-25 | **Lossy** | `9f72b4f` | 300 of 300 after 22 minutes; latecomers: half within 154.6 s, all within 209.9 s | 700 s / 1,078 s / 1,330 s | 21.5 copies (1,124 MB), all in the first 17 minutes; 224 requests refused as busy; latecomers 0.01 | creator and all met; **95% missed** |
| 2026-10-25 | **Flaky** | `9f72b4f` | 300 of 300 after 16 minutes; latecomers: half within 17.2 s, all within 30.6 s | 622 s / 844 s / 950 s | 23.7 copies (1,238 MB), all in the first 14 minutes; 200 requests refused as busy; latecomers 0.01 | **all met** |
| 2026-10-25 | Slow hosts, a fast one in ten | `9f72b4f` | 300 of 300 after 36 minutes; latecomers: half within 144.2 s, all within 147.4 s | 1,159 s / 1,955 s / 2,133 s | 25.8 copies (1,349 MB), all in the first 12 minutes; 163 requests refused as busy; latecomers 0.01 | not judged; the followers' marks would be missed |

**Home.** The [scenario](#the-scenario) as it stands. The video came out at 52.2 MB.

- Every mark held, but more slowly than on loopback: 95% of followers held the video at 775 s and all at 893 s, against 610 s and 675 s in [B8's run in parts](b8-creator-upload.md#results) (`5890a80`).
- The creator sent 25.0 copies, against 18.2 on loopback, all in the first 12 minutes. The run with upload caps, below, shows why.
- Followers' downloads: 1,607 completed and 1,458 refused as busy, against 1,541 and 861 on loopback. None failed or sent bad data, and 9 times a follower couldn't connect to another.
- Full router queues dropped 2.9% of packets: busy connections slowing QUIC down, as on a real line.
- Latecomers: half held the video within 17.1 s and all within 26.6 s, against 14.1 s and 23.6 s on loopback.

**Home, upload caps.** The home run with every node capping its upload at its connection's speed, as on loopback.

- The spread was quicker and the creator sent fewer copies: 95% of followers held the video at 727 s and all at 802 s, against 775 s and 893 s with no caps, and the creator sent 19.0 copies, against 25.0, close to loopback's 18.2.
- A capped node gets one transfer place per 1.5 Mbit/s of its cap, between 2 and 8, so a follower uploading at 5 Mbit/s serves 3 transfers at once. With no cap it serves 8 at once, whatever its line, and each copy it sends takes longer. The creator's copies part ways from 6 minutes on, as followers start serving. The likely reason, not yet measured: with slow followers' copies finishing later, followers hold the video later, and more fetches reach the creator in the meantime.
- Busy refusals were 985, against 1,458 with no caps.
- Nodes set no cap unless their owner does, so the home run is the one that counts. Giving a node with no cap places to suit what its upload has been measured to do may close the gap.
- Full router queues dropped 4.8% of packets, against 2.9% with no caps.

**Lossy.** Packet loss from none to 2%, a poor Wi-Fi's.

- 283 of 300 followers held the video after an hour. The last of them had it at 21 minutes, and nothing changed in the 39 minutes after that. The marks for 95% and for every follower were missed.
- Each of the 17 that didn't get it gave up prefetching it once, between 13 and 19 minutes after posting. Their download wasn't finished 10 minutes after it started, the most a whole download could then take. A prefetch refused as busy was tried again later, but one that failed for any other reason wasn't: the media was fetched when someone opened the post.
- 16 of the 17 have the poorest connections, 2% loss: 16 of the 60 followers at 2%. The other has 0.5%. QUIC takes a lost packet as a sign of a busy line and slows down, so at 2% loss a transfer runs far below the line's speed.
- Loss also made followers poor hosts. The 120 followers with no loss sent 255 copies between them; the 60 at each of 0.5%, 1% and 2% sent 20, 6.7 and 5.6.
- The creator, losing 0.5% itself, sent 18.4 copies, fewer than in the home run. Followers' downloads failed 32 times, against none in the home run.
- Latecomers took far longer, half within 115 s and all within 253 s: four of the ten have 1% or 2% loss too. The creator still sent them only their thumbnails.
- Simulator's own trouble: 25 minutes after posting, a build on the same machine filled its disk, and one sync failed with "database or disk is full". By then every give-up had happened and the last follower to get the video had it, so this didn't change the result.

**Flaky.** 30 followers cut off for 30 seconds in every 5 minutes, taking turns.

- 260 of 300 followers held the video after an hour, the last of them at 21 minutes. The marks for 95% and for every follower were missed.
- The 40 that didn't get it gave up their prefetch, as in the lossy run: 39 because their download wasn't finished after 10 minutes, between 14½ and 21 minutes after posting, and one at 2½ minutes because no node it asked could send it.
- Only 4 of the 40 are flaky themselves; 26 of the 30 flaky followers held the video. The other 36 have steady connections. The logs don't say which host each was waiting on; most likely their download stalled on a host that was cut off, waiting for it instead of moving on to another, until its time ran out. The run after the fix, below, fits that.
- Followers' downloads failed 140 times, and 30 times a follower couldn't connect to another, against none and 9 in the home run.
- Full router queues dropped 7.8% of packets, against 2.9% in the home run, and 0.1% were lost while cut off. Why the queues dropped so many more isn't known yet.
- Latecomers, who came after the outages had ended, were as quick as on loopback.

**Slow hosts, a fast one in ten.** Followers uploading at 1 Mbit/s, one in ten at 100. Measured, not judged.

- 175 of 300 followers held the video after an hour, the last of them at 20 minutes. The other 125 each gave up their prefetch, between 13 and 20 minutes after posting, because their download wasn't finished 10 minutes after it started: the same as in the lossy and flaky runs, on a larger scale.
- A copy of the video takes 7 minutes from a host at 1 Mbit/s with nothing else to send, and much longer when it's sending several. Even 15 of the 30 fast followers gave up: a download that drew slow hosts stayed with them.
- The 270 followers at 1 Mbit/s sent 130 copies between them, and the 30 at 100 Mbit/s sent 104.
- Latecomers took 144 to 151 s each, about what the video takes from three hosts at 1 Mbit/s at once. That is the figure for choosing hosts by what's been measured of them ([doc 12](../12-peer-reciprocity-and-recognition.md) phase 2) to beat, once followers stop giving up.
- The creator sent 17.2 copies, all in the first 10 minutes.

**After the fix.** With `9f72b4f`, a download gives up on a node that has sent nothing for 30 seconds and asks the next; a whole download may take 30 minutes instead of 10; and a prefetch that no node could send just then is tried again later, as one refused as busy already was ([ADR 0003](../decisions/0003-native-social-protocol-v0.md)). The home, lossy, flaky and slow-hosts runs were repeated with it the same day, unchanged. The run with upload caps wasn't.

- No follower gave up its prefetch in any run, against 17, 40 and 125 in the lossy, flaky and slow-hosts runs before, and every follower got the video in every run.
- **Home:** every mark held again. 95% of followers held the video at 814 s and all at 863 s, against 775 s and 893 s, and the creator sent 23.7 copies, against 25.0. Followers' downloads failed 16 times, against none: most likely hosts given up on after sending nothing for 30 seconds, which count as failed. Full router queues dropped 5.6% of packets, against 2.9%. With one run of each, how much of these differences is the fix and how much varies from run to run isn't known. Latecomers: half within 16.0 s, all within 18.3 s.
- **Lossy:** every follower held the video by 1,330 s, within the 30-minute mark, against 283 after an hour. But 95% held it only at 1,078 s, 18 minutes, so that mark was missed: at 15 minutes, 247 of 300 had it. The followers at 2% loss came last: 25 of their 60 had it at 15 minutes, against 116 of the 120 with no loss, and they sent 0.5 copies between them. The creator sent 21.5 copies, against 18.4, now that every follower fetches the video. Latecomers: half within 154.6 s and all within 209.9 s, against 115.4 s and 252.7 s. Even those with no loss took 80 to 169 s, since their hosts have losses.
- **Flaky:** every mark held. 95% of followers held the video at 844 s and all at 950 s. All 30 flaky followers got it, the last at 842 s, against 26 before, and none of the steady followers gave up, against 36. That fits the first run's guess that they had stalled on a host that was cut off. Followers' downloads failed 47 times, against 140. Full router queues dropped 5.2% of packets, against 7.8% before and 5.6% in the home run after the fix, so that share varies more from run to run than the first runs suggested.
- **Slow hosts:** every follower held the video by 2,133 s, 36 minutes, against 175 after an hour; 95% at 1,955 s. Not judged; both followers' marks would be missed. The 30 fast followers were no quicker to get it: half of them had it at 1,334 s, against 1,158 s for the 270 slow ones, most likely because nine in ten of the hosts a download can draw are slow. The slow followers sent 198 copies between them and the fast ones 79, against 130 and 104. The creator sent 25.8 copies, against 17.2. Latecomers took 144 to 147 s each, as before: still the figure for choosing hosts by what's been measured of them to beat.

**Repeats.** As [planned](#repeats), on 2026-10-26: the home run five times with `8533ea3`, the same code without the fix, and with the fix (`9f72b4f`) the home, lossy and flaky runs five more times each and the slow-hosts run twice more, unchanged. The runs with the fix include the first of each, above.

| Run | Code | Runs | 50% / 95% / all, in seconds: median (range) | Creator sent, copies: median (range) | Runs meeting each mark: creator / 95% / all | Pass marks |
| --- | --- | --- | --- | --- | --- | --- |
| **Home** | `8533ea3`, without the fix | 5 | 615 (469–856) / reached in 3: 621–807 / reached in 2: 696 and 820 | 22.9 (21.5–25.8) | 5 / 3 / 2 | creator met; **95% and all missed** |
| **Home** | `9f72b4f` | 6 | 596 (478–674) / 777 (613–879) / 834 (747–988) | 24.4 (21.3–26.1) | 6 / 6 / 6 | **all met** |
| **Lossy** | `9f72b4f` | 6 | 689 (558–896) / 1,029 (805–1,218) / 1,274 (1,107–1,388) | 21.5 (17.4–22.0) | 6 / 1 / 6 | creator and all met; **95% missed** |
| **Flaky** | `9f72b4f` | 6 | 644 (534–692) / 845 (653–893) / 941 (788–1,169) | 25.2 (23.7–30.6) | 5 / 6 / 6 | 95% and all met; **creator missed** |
| Slow hosts, a fast one in ten | `9f72b4f` | 3 | 1,413 (1,159–1,438) / 2,123 (1,955–2,188) / 2,202 (2,133–2,275) | 35.5 (25.8–36.8) | 1 / 0 / 0 | not judged |

- **Without the fix,** followers gave up their prefetch in 3 of the 5 home runs: 30, 16 and 2 of them, each once, with "it timed out", as in the first lossy and flaky runs. They never got the video, so those runs went on for the full hour, ending with 270, 284 and 298 of 300. In the other two, no download took 10 minutes, and every mark was met. The first home run, which met every mark without the fix, was like those two: no follower gave up in it.
- **With the fix,** no follower gave up in any of the 21 runs, and every follower got the video in every one. The home run met every mark in all six.
- **Lossy:** every follower got the video in every run, the last at 1,388 s, 23 minutes. 95% held it within 15 minutes in only one run of six, at 805 s; in the others it took 919 to 1,218 s. At 15 minutes, 154 to 294 of 300 held it.
- **Flaky:** both followers' marks were met in every run. In one, the creator sent 30.6 copies, just over its 30; in the other five, 23.7 to 26.3. So by the rule set before the repeats, the creator's mark is missed.
- **Slow hosts:** every follower got the video in every run, the last within 2,133 to 2,275 s, 38 minutes at most. The creator sent 35.5 and 36.8 copies in the repeats, against 25.8 in the first run: not judged, but over the 30 a judged run is allowed. Why it sent more isn't known yet.
- **Latecomers:** the creator sent them 0.01 copies in every run, so that mark was met throughout. With the fix, on home connections, half held the video within 15.9 s and all within 22.7 s (medians), against 14.3 s and 24.7 s without it. With loss, 139.8 s and 250.1 s; with slow hosts, 144.2 s and 145.1 s, still the figure to beat.
- **The machine mattered.** Partway through the third round, full router queues began dropping far fewer packets, for both versions of the code alike: in the home runs, 4.1 to 7.2% of packets before and 1.1 to 1.9% after. The spread was quicker after: with the fix, 95% of followers held the video at 739 to 879 s in the home runs before, and at 613 and 672 s after. All three home runs without the fix that missed marks came before. The runs with the fix in the same rounds met every mark, so the rounds kept the comparison fair. Nothing in the runs changed. Most likely the machine was busier before: the simulator runs in real time, so when it's held up, packets wait longer in its queues and more of them overflow. That wasn't checked. It may also be why the first runs' dropped packets differed so much, from 2.9% to 7.8%. Figures from runs at different times aren't comparable, then; comparing two versions of the code needs them in the same rounds, as here.
- Followers' failed downloads varied widely from run to run: with the fix, 0 to 16 in the home runs, 0 to 46 with loss, and 21 to 92 in the flaky runs.

**What it shows.** On home connections, lossy or dropping out, the swarm kept the creator's upload within its mark, if only just in one flaky run (30.6 copies against 30); with nine hosts in ten slow, it went over, to 36.8. Loopback hid a flaw the first runs found: a follower whose download wasn't finished 10 minutes after it started gave up, and nothing tried again until someone opened the post. Without the fix, that left followers without the video in 3 of 5 home runs; with it, every follower got the video in all 21 runs, and the home run met every mark every time. What's left is speed, and the creator's upload when hosts are slow. At 2% loss a follower is slow to get the video and of little use as a host, so the lossy run misses the 15-minute mark in most runs; and with nine hosts in ten at 1 Mbit/s, the spread takes over half an hour. Choosing hosts by what's been measured of them ([doc 12](../12-peer-reciprocity-and-recognition.md) phase 2) is the next chance to improve both.

## Switching away from slow hosts

Added on 2026-10-30, before any of these runs, as the first part of [doc 12](../12-peer-reciprocity-and-recognition.md) phase 2. The slow-hosts run left latecomers at 144 to 147 s, about what the video takes from three hosts at 1 Mbit/s at once. A node that finished its part took over the back half of the part with the most bytes still to come, and never one with less than 2 MiB left, so a slow host could keep up to 2 MiB to send, about 17 s at 1 Mbit/s. Now (`576aaaf`) it takes over the end of the part that will take longest at its node's rate so far, split by the two nodes' rates so they finish together, and all of it from a node that has sent nothing for 2 s ([doc 13](../13-hosting-roles-and-follower-swarm.md)).

The harness now reports where each latecomer's bytes came from (the bytes, the rate they came at, the host's upload speed, and busy and failed tries), the followers' downloads added up by the sending nodes' speed, and how much of the machine's CPU the run and other programs took (`6291a72`).

**The runs.** The slow-hosts run as before (`--swarm --net --latecomers 10 --follower-mbps 1,1,1,1,1,1,1,1,1,100`), with `576aaaf` and with `6291a72`, the code before the change, in the same round.

**What counts as better.** No new pass marks: the B8 marks stay as they are. The change helps a latecomer only when one of the three hosts it draws is fast, so the figures to compare are those latecomers' times, against the same in the run without the change, and against the 17 s or so a slow host's last 2 MiB took. Nothing should get worse: the followers' times and the creator's copies should stay within the variation the repeats showed, and the followers' busy refusals shouldn't grow much, since every takeover is a new request to a host.

**Results**, 2026-10-30, one round on the same machine: the run without the change, the change, and the run without it again. During the first, other programs took up to 30% of the machine's CPUs while its followers fetched (its report says so); its latecomers came after that. In the other two, other programs took at most 1%.

| Code | Latecomers that drew a fast host | The rest | Followers: 50% / 95% / all | Creator's copies | Followers' requests refused as busy | Followers' failed downloads |
| --- | --- | --- | --- | --- | --- | --- |
| `6291a72`, before | 2: 23.1 s and 22.7 s | 8: 144.0 to 145.1 s | 1,223 / 2,010 / 2,162 s | 23.2 | 1,016 | 20 |
| `576aaaf`, the change | 2: **6.4 s and 8.5 s** | 8: 144.0 to 145.1 s | 1,120 / 1,936 / 2,064 s | 25.8 | 721 | 71 |
| `6291a72`, before, again | none | 10: 144.2 to 145.3 s | 1,235 / 1,963 / 2,200 s | 29.2 | 1,837 | 0 |

- **A fast host now does nearly all of it.** Before, a latecomer that drew one 100 Mbit/s host got 47 MB from it, taking over half a part seven times, and still waited on its two slow hosts for the last 2.6 to 2.7 MB each, about 22 s at 1 Mbit/s. With the change, the slow hosts sent 0.5 to 0.8 MB each and the fast one the other 51 MB, in three requests: 6.4 and 8.5 s instead of 23.
- **Most latecomers didn't draw one.** With one host in ten fast, three hosts drawn at random are all slow nearly three times in four: 8, 8 and 10 latecomers of 10 in these runs. They took 144 to 145 s whatever the code, so half the latecomers still took 144 s.
- **Nothing got worse for the followers.** Their times were within or better than the range of the earlier slow-hosts runs (1,159 to 1,438 s for half, 1,955 to 2,188 for 95%, 2,133 to 2,275 for all), the creator's 25.8 copies were within it (25.8 to 36.8), and busy refusals were fewer.
- **Followers' failed downloads rose to 71, all from 1 Mbit/s hosts.** Most likely these are hosts serving several others at once that hadn't sent a byte 2 seconds after being asked: the change hands such a part to another node and records the attempt as failed. The followers still got the video sooner. A host's record now counts a slow start as a failure, which matters once source choice uses the records.

**What it shows.** Switching by the rates removes the wait on slow hosts once a fast one is among the three, but most downloads never draw one. The rest is choosing which hosts to ask: the others named in the `WhoHosts` reply, of which a download uses three of up to six, and the record of how hosts did before.

**Handed on, not failed.** Added on 2026-10-30, before these runs. A host that is busy holds a new request for up to 10 s, waiting for a place, before it refuses (`BUSY_WAIT`), and a download hands a part on once its node has sent nothing for 2 s. So the 71 failed downloads were most likely requests waiting for a place, not failures. Such an attempt is now its own outcome, handed on, counted apart in the peer record (`ac4a2d9`). The slow-hosts run again with it; and once with the wait before handing a part on raised to 12 s, longer than a busy host takes to refuse, to see what waiting longer costs (a build for this run only, not committed). Expected with `ac4a2d9`: failed downloads near 0 and about as many handed on as there were failed before. With 12 s: refusals heard as busy again, and the followers' times, the latecomers with a fast host and the busy refusals compared with `ac4a2d9`.

**Results**, 2026-10-30, one round on the same machine; other programs took at most 6% of its CPUs.

| Wait before handing on | Latecomers that drew a fast host | The rest | Followers: 50% / 95% / all | Creator's copies | Followers' requests refused as busy | Followers' failed / handed on |
| --- | --- | --- | --- | --- | --- | --- |
| 2 s (`ac4a2d9`) | 2: 6.7 s and 10.4 s | 8: 144.0 to 145.3 s | 1,768 / 2,327 / 2,558 s | 48.6 | 3,515 | 6 / 46 |
| 12 s | 3: 5.0, 11.0 and 14.2 s | 7: 144.0 to 145.0 s | 1,523 / 2,107 / 2,232 s | 33.1 | 1,955 | 6 / 10 |

- **Handed on, as expected.** With `ac4a2d9`, 6 downloads failed and 46 were handed on, against 71 failed with the same download code before (`576aaaf`): slow starts are now told apart from failures.
- **12 s did better in this pair on every count.** Followers had the video sooner, the creator sent 15.5 fewer copies, fewer requests were refused as busy, the 100 Mbit/s nodes sent more (394 transfers and 3,382 MB, against 132 and 2,189 MB), and fewer parts were handed on. Latecomers with a fast host were as quick.
- **But the 2 s run is the odd one out.** `ac4a2d9` downloads exactly as `576aaaf` does (it changes only what is recorded), and `576aaaf`'s run did better than both: all followers at 2,064 s, 25.8 creator's copies, 721 refused as busy. In the 2 s run the creator's copies climbed from 35 to 48 between minutes 15 and 19, which nothing in the code explains. The runs vary more than the two waits differ, so this pair doesn't settle it.
- **Unchanged:** latecomers whose three hosts were all slow took 144 to 145 s either way.

**Decision**, 2026-10-30: the wait becomes 12 s (`SLOW_START`), without repeating the pair, to keep moving. A busy host's refusal is then heard as one, so the peer record counts busy hosts as busy and fewer requests are made, and this pair showed no cost. Revisit if real use shows downloads held up by nodes that send nothing; the constant's comment says so.

## Places to suit the upload

Added on 2026-10-30, before any of these runs. With no upload cap, a node served 8 transfers at once whatever its line, so a node uploading at 1 Mbit/s sent each of 8 copies at an eighth of that. The home run with upload caps, where each node had one place per 1.5 Mbit/s of its line, spread the video sooner (95% at 727 s against 775 s) and the creator sent fewer copies (19.0 against 25.0), but real nodes set no cap unless their owner does ([above](#results)). Now (`0f7a881`) a node with no cap starts with 2 places and measures what it sends: the most in any 5 seconds, leaving out each transfer's first 1.25 MB, which QUIC takes at once. It adds places by the cap's rule as faster speeds are measured, and never takes them away ([ADR 0003](../decisions/0003-native-social-protocol-v0.md)). The report now gives each node's places and the speed it measured when the run ended, by the followers' upload.

Before the runs, in short runs and tests: a node at 2 Mbit/s measured 1.9; one at 30 Mbit/s, sending 3 MB copies one after another, measured 17.7 and had 8 places, reading low since each copy's first 1.25 MB isn't counted. In a 30-follower trial of the slow-hosts run, the creator measured 19.9 Mbit/s and had 8 places, and every 1 Mbit/s follower that served measured 1.0 and kept 2. The 100 Mbit/s followers weren't measured with an earlier version that left out each transfer's first second, not its first 1.25 MB: their parts took about a second each.

**The runs.** The home run and the slow-hosts run, each with `0f7a881` and with `954e720`, the code before it, in the same round: home before, home with the change, slow hosts before, slow hosts with the change. Two rounds, since the runs vary from one to the next as much as the [last pair](#switching-away-from-slow-hosts) differed.

**What counts as better.** No new pass marks. The home run is judged on B8's marks, as before, and should meet them with the change too.

- **The places match the lines:** at the end, followers at 5, 10 and 20 Mbit/s have 3, 6 and 8 places, as a cap at their speed would give, those at 1 Mbit/s have 2, and the creator 8. A node that served little may not have been measured, and keeps 2.
- **Home:** the followers' times and the creator's copies move toward the run with caps, and are no worse than the run before the change in the same round, within the variation the [repeats](#repeats) showed.
- **Slow hosts:** the 1 Mbit/s hosts serve 2 at once instead of 8, so each copy they take on finishes sooner and the rest go to other hosts. The followers' times should be shorter than before the change; the creator's copies may rise, since more requests are refused as busy, and should stay within what earlier slow-hosts runs showed (23.2 to 48.6 copies on 2026-10-30). The latecomers' times are recorded.
- **Busy refusals** will rise in both: refusing early is the point. Not a failure in itself.

**Results**, 2026-10-30 and 31: two rounds on the same machine, each in the planned order. Round 2's slow-hosts pair was cut off partway and run again from the start some hours later, after the machine had restarted; its two runs still ran one after the other. Other programs took at most 9% of the machine's CPUs in any run.

| Run | Round | Code | Followers: 50% / 95% / all | Creator's copies | Followers' requests refused as busy | Latecomers: half / all |
| --- | --- | --- | --- | --- | --- | --- |
| Home | 1 | `954e720`, before | 492 / 632 / 777 s | 22.5 | 1,044 | 16.1 / 30.9 s |
| Home | 1 | `0f7a881`, the change | 534 / 689 / 832 s | 24.5 | 1,380 | 13.3 / 24.1 s |
| Home | 2 | `954e720`, before | 601 / 783 / 872 s | 24.5 | 1,611 | 15.7 / 30.2 s |
| Home | 2 | `0f7a881`, the change | 606 / 741 / 884 s | 24.6 | 1,519 | 13.1 / 17.3 s |
| Slow hosts | 1 | `954e720`, before | 2,055 / 2,634 / 3,292 s | 69.8 | 4,962 | 144.5 / 147.5 s |
| Slow hosts | 1 | `0f7a881`, the change | **607 / 1,031 / 1,385 s** | 33.3 | 1,451 | 144.6 / 146.0 s |
| Slow hosts | 2 | `954e720`, before | 979 / 1,775 / 2,085 s | 20.2 | 618 | 144.2 / 145.0 s |
| Slow hosts | 2 | `0f7a881`, the change | **656 / 1,090 / 1,364 s** | 34.5 | 2,442 | 144.3 / 146.8 s |

- **Slow hosts: every follower had the video within 23 minutes.** With the change, all held it at 1,385 and 1,364 s, against 3,292 and 2,085 s without, and sooner than in any slow-hosts run before (2,064 to 2,558 s on 2026-10-30). These are the first slow-hosts runs to meet B8's mark for every follower within 30 minutes; 95% within 15 minutes is still missed, at 17 and 18 minutes. The two runs with the change came out close to each other, the two without it far apart.
- **The slow hosts sent each copy faster, and the fast ones sent more.** A 1 Mbit/s host's transfers came at 0.89 and 0.86 Mbit/s each, against 0.29 and 0.33 when it served 8 at once. The 100 Mbit/s followers sent 7.1 and 7.3 GB, against 0.5 and 4.4 GB, and more of them sent anything: 17 and 19 of the 30, against 9 and 6. Full router queues dropped 0.7 and 0.9% of packets, against 2.9 and 2.4%, with fewer transfers at once on the slow lines.
- **The creator sent 33.3 and 34.5 copies,** against 69.8 and 20.2 without the change: within what earlier slow-hosts runs showed (23.2 to 48.6), as the plan allowed. The two runs without the change differ more than threefold, so this pair can't say whether the change costs the creator anything.
- **Busy refusals** fell in round 1 (1,451 against 4,962) and rose in round 2 (2,442 against 618): expected to rise, they did once.
- **Slow-hosts latecomers were unchanged:** 39 of the 40 drew three slow hosts and took 144 to 147.5 s. The other, before the change, drew a fast host and took 11.1 s.
- **Home: no worse, and no nearer the run with caps.** All four runs met every B8 mark. The change was 57 s slower to 95% in round 1 and 42 s quicker in round 2, and all four runs are within the earlier [repeats](#repeats)' range (95% at 613 to 879 s, creator 21.3 to 26.1 copies). The creator sent 24.5 and 24.6 copies, against 22.5 and 24.5, not toward the run with caps' 19.0. Latecomers were quicker with the change in both rounds, most likely by the hosts they drew: the slowest before the change, at 30 s, drew three 5 Mbit/s hosts, which none did with it, and no latecomer was refused as busy in any run.
- **Places mostly matched the lines.** Of the followers that were measured, 72 and 81% of those at 5 Mbit/s had 3 places, 68 and 74% of those at 10 had 6, and 89 and 95% of those at 20 had 8, as a cap at their speed gives. The rest measured lower, never having sent enough at once to fill their line, except one at 10 Mbit/s that read 10.5 and had 7. 31 to 39 of the 100 at each speed served too little to be measured and kept 2. In the slow-hosts runs every 1 Mbit/s follower kept 2 (measured at 0.7 to 1.1 Mbit/s), and every 100 Mbit/s one that was measured had 8.
- **The creator read 44 to 47 Mbit/s on its 20 Mbit/s line** in three of the four runs. After a download in parts falls short, a node asks one host at a time for what is left of the blob, the author's node last, and what is left can have gaps between its ranges; the meter counted the bytes in the gaps as sent. It now counts only the bytes asked for (`bd056b6`). The creator has 8 places from 12 Mbit/s up, so this didn't change its places in these runs; on a slow node it could have given more places than the line can serve. A test on the simulated network catches it: a 1 Mbit/s node asked for the start and end of a photo read 2.6 Mbit/s before the fix.

**What it shows.** Places sized to the measured upload change little on home connections, where every follower can serve a copy in minutes, but with nine hosts in ten at 1 Mbit/s every follower had the video within 23 minutes, against 35 and 55. The slow hosts take on only what they can send in good time, and the rest goes elsewhere. Left in the slow-hosts run: latecomers whose three hosts are all slow (144 s), the 95% mark, and the creator's copies, over B8's 30.

## Checking that a follower holds its copy

Added on 2026-10-31, before this run. A follower that shares tells the author's node when it holds a copy (`Hosting`), and the author's node named it to other followers without checking. Now (`3aa2591`) the author's node first asks the follower for one 1 KiB chunk of the file, picked at random, and checks it against the hash, and names the follower only if it arrives within 15 seconds. The follower serves a request that small from the places for small files, so it can answer while its other places are busy ([ADR 0003](../decisions/0003-native-social-protocol-v0.md)). A follower named once isn't checked again, and once 100 are named for a file, the ones that have followed longest stay ([doc 13](../13-hosting-roles-and-follower-swarm.md#how-a-post-spreads)).

**The runs.** The home run with `3aa2591`, and with `d76a178`, the code before it, in the same round. Both log each social request a node refuses as busy, nearly all of which go to the author's node, and the run with the change each check a follower fails (`osp_node::social=debug` added to `RUST_LOG`).

**What counts as better.** No new pass marks. On home connections every follower holds a real copy, so the change should cost nothing:

- **No follower fails the check.** A follower that fails holds a copy that no one is sent to.
- **B8's marks are met.** The followers' times, the creator's copies and the busy refusals are within the variation the [repeats](#repeats) showed, and no worse than the run before the change.
- **Social requests refused as busy** don't rise much: a follower's `Hosting` request now waits for the check, keeping one of the author's node's 32 places for social requests a little longer.

The checks themselves cost the author's node at most one 16 KiB block per follower, under 5 MB for 300, a tenth of one copy of the video.

**Results**, 2026-10-31, one round on the same machine; other programs took at most 2% of its CPUs.

| Code | Followers: 50% / 95% / all | Creator's copies | Followers' requests refused as busy | Followers that sent copies to others | Latecomers: half / all |
| --- | --- | --- | --- | --- | --- |
| `d76a178`, before | 524 / 666 / 755 s | 23.2 | 1,180 | 209 | 15.5 / 26.6 s |
| `3aa2591`, the check | 671 / 813 / 922 s | 26.2 | 2,214 | 135 | 15.9 / 30.9 s |

- **B8's marks were met in both,** and the check's times are within the [repeats](#repeats)' range (95% at 613 to 879 s). But it was 147 s slower to 95% than the run before it, its creator's 26.2 copies are just over the repeats' 26.1, and its 2,214 busy refusals are more than in any earlier home run (at most 2,076).
- **No social request was refused as busy** in either run: waiting for the check didn't fill the author's node's places.
- **Five followers failed the check,** so that mark was missed. The author's node couldn't connect to four of them within 10 s, and the fifth's piece took over 15 s. All five came 10 to 12 minutes after posting, while the creator was sending the most, three of them at the same moment. A follower told the author's node only once, so these five were never named.
- **Fewer followers did the hosting.** 135 followers sent copies to others, against 209 before and 155 to 205 in the four home runs of [the last section](#places-to-suit-the-upload); one sent 17.4 copies, more than any follower in an earlier home run (at most 15.9). Every latecomer's hosts were among the first 145 followers to follow, where before they came from all over. The likely reason is the new rule for a full list, not the check. The list kept the 100 most recently named, so a follower that had just got the video, with its places free, was named at once; now the 100 that have followed longest stay. The harness has every follower follow at the start, one after another, so once 100 are named, a follower is named only in place of one that followed after it, and the list settles on the first to follow.

**A second round, and a run with a larger list.** Added on 2026-10-31, during the second round's first run, before any of its results. One pair can differ by as much as runs vary ([repeats](#repeats)), so both runs are repeated in a second round, in the same order. Then the check once more with the list's limit raised from 100 to 300, in a build for that run only, so that with 300 followers the list never fills: that run shows what the check costs by itself.

- If the gap holds in the second round and the run with the larger list comes out close to the runs before the change, the rule for a full list is what slows the spread, and needs another look.
- If the run with the larger list is as slow as the check's, the check itself costs that time.

**Results**, 2026-10-31, in the planned order; other programs took at most 4% of the machine's CPUs. Round 1 is repeated from above.

| Round | Code | Followers: 50% / 95% / all | Creator's copies | Followers' requests refused as busy | Followers that sent copies to others | Latecomers: half / all | Checks failed |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `d76a178`, before | 524 / 666 / 755 s | 23.2 | 1,180 | 209 | 15.5 / 26.6 s | none made |
| 1 | `3aa2591`, the check | 671 / 813 / 922 s | 26.2 | 2,214 | 135 | 15.9 / 30.9 s | 5 |
| 2 | `d76a178`, before | 527 / 667 / 737 s | 22.9 | 1,213 | 195 | 16.0 / 30.5 s | none made |
| 2 | `3aa2591`, the check | 627 / 740 / 874 s | 26.0 | 2,189 | 130 | 13.7 / 26.4 s | 0 |
| — | `3aa2591`, the check, with a list of 300 | 526 / 622 / 1,020 s | 22.5 | 1,125 | 172 | 15.2 / 30.5 s | 0 |

- **The gap held.** With the check and a list of 100, 95% of the followers held the video 73 s later than in the run before it in round 2, and 147 s later in round 1; all of them, 137 and 167 s later. The creator sent 26.0 and 26.2 copies, against 22.9 and 23.2; busy refusals nearly doubled; and 130 and 135 followers sent copies to others, against 195 and 209. The two runs before the change came out close to each other, and so did the two with it.
- **With a list that never fills, the check cost nothing measurable.** Half the followers held the video at 526 s and 95% at 622 s, as soon as or sooner than in either run before the change; the creator sent 22.5 copies, and 1,125 requests were refused as busy. The rule for a full list slowed the spread, not the check.
- **One follower came last, alone,** at 1,020 s, four minutes after the one before it. It failed no check, and the logs, which leave out busy refusals and the tries after them, don't say why; most likely its download was refused as busy and tried again later. Every follower still had the video within 30 minutes.
- **No check failed in round 2** or in the run with a list of 300, so the five failures of round 1 didn't recur. No social request was refused as busy in any run.

**What it shows.** Checking a follower's copy before naming it costs nothing measurable on home connections. Keeping the followers that have followed longest once 100 are named does: those first named go on being named while followers that got the video later, with their places free, never are, so fewer followers do the hosting, and more requests are refused as busy.

**Told once more.** Added on 2026-10-31. A follower that couldn't tell the author's node, or a hosting resharer's, that it holds a copy, because the node refused, as when the check failed, or couldn't be reached, now tells it once more after 2½ to 7½ minutes (`525e6ea`). None of the runs above have it.

**Half and half.** Added on 2026-10-31, before this run. Once 100 are named for a file, half the places now go to the followers that have followed longest, as before, so a crowd of new followers still can't push them out, and half to those named last, so followers that got the video later, with their places free, are named too (`f2cfa3a`, which tells once more too). The home run with it, and with `d76a178`, the code before the check, in the same round.

- **Expected:** close to the runs before the check, as the run with a list of 300 was: the followers' times, the creator's copies, the busy refusals and the number of followers sending copies to others within what the runs before the check showed in rounds 1 and 2, and no worse than the run before it in this round.
- **Checks failed:** counted from the log, as before. A follower that fails one now tells the author's node once more and is checked again, so the log shows a follower failing both times twice, and one passing the second time once.

**Results**, 2026-10-31, one round on the same machine; other programs took at most 4% of its CPUs.

| Code | Followers: 50% / 95% / all | Creator's copies | Followers' requests refused as busy | Followers that sent copies to others | Latecomers: half / all | Checks failed |
| --- | --- | --- | --- | --- | --- | --- |
| `d76a178`, before the check | 587 / 699 / 823 s | 23.9 | 1,657 | 183 | 16.0 / 33.5 s | none made |
| `f2cfa3a`, half and half | 478 / 617 / 662 s | 20.7 | 826 | 209 | 14.3 / 23.0 s | 2 |

- **No worse than before the check, as expected, and in this pair better on every count.** 95% of the followers held the video 82 s sooner than in the run before it, and all of them 161 s sooner; the creator sent 20.7 copies, against 23.9; half as many requests were refused as busy; and 209 followers sent copies to others, against 183. Against all three runs before the check (rounds 1 to 3) too, it was the quickest to 95% and to all, the creator sent the fewest copies, and the fewest requests were refused as busy. One pair can't say how much of that is the change and how much the run; the run before it in this round was the slowest of the three.
- **Two checks failed,** for two followers, a second apart, 7 minutes after posting: each piece took over 15 s. Each was due to tell the author's node once more 589 to 890 s after posting. The nodes ran until 817 s, after the latecomers, and neither failed again; the log records only failures, so whether each was named in the end isn't known.
- No social request was refused as busy.

**What it shows.** With half the list kept for the followers that have followed longest and half for those named last, the swarm spread the video as fast as before the check, or faster, while a crowd of new followers still can't push out the half that came first. Checking a follower's copy costs nothing measurable. A check fails now and then, at the busiest time, when a follower's piece takes over 15 s: five in round 1, none in round 2, two here.

## Naming followers heard from lately first

Added on 2026-10-31, before these runs. The author's node named the followers with a copy of a file at random, from up to 100. A follower whose node had stopped stayed on that list and went on being named, and a downloader that asked it waited for a connection that never came, while the list's other followers were online. Now (`0edaca4`) the author's node names first the followers that sent it a request in the last 5 minutes (a node asks the nodes it follows for new posts every minute), those whose requests came straight from their node before those that came through a relay, and picks at random within each ([ADR 0003](../decisions/0003-native-social-protocol-v0.md)). It goes only by what it saw itself, never by what other nodes say about a follower. On the simulated network every connection is direct, so here only followers that stop make a difference.

The report now also lists the three followers last to hold the video, with what each received from each node and how its tries ended, so a straggler like the [one above](#checking-that-a-follower-holds-its-copy) can be explained, and how often each source couldn't be reached. Both builds have this (`6406df4`).

**The runs.** In this order, in one round, each with `6406df4` (the code before, with the new report) and then with `0edaca4`:

1. The home run.
2. **A third offline:** the home run, but once every follower holds the video, 100 of the 300 stop, at 900 s after posting (`--drop-out 100 --drop-out-secs 900`), and the latecomers follow from 1,260 s on (`--latecomers-after-secs 1260`), when the followers that stopped have been silent for 6 minutes. This is someone opening a post after many of the followers holding it have gone offline, as at night. B8's drop-out run stops a fifth of the followers at 480 s instead, while they're still fetching; there, the half of the list kept for the followers named last soon fills with followers still running, so a ranking would change little.

**What counts as better.** No new pass marks; B8's marks should be met in all four.

- **A third offline:** before the change, about a third of the followers named to a latecomer had stopped, so each latecomer could expect to ask one of them among the three it asks at once. With the change, none of the six named should have stopped: the latecomers' "couldn't connect" should fall from several to none, and their times should be no worse.
- **Home:** every follower is heard from every minute, directly, so the names are as random as before: the followers' times, the creator's copies and the busy refusals within the [repeats](#repeats)' variation, and no worse than the run before the change in this round.
- **The last followers to hold the video:** recorded. If they were refused as busy and tried again a minute or more later, that explains the straggler above.

**Results**, 2026-10-31, one round on the same machine; other programs took at most 5% of its CPUs. The 100 followers that stop are every third one, those with a 20 Mbit/s upload; in the runs with a third offline, the followers' figures count only the 200 that stayed.

| Run | Code | Followers: 50% / 95% / all | Creator's copies | Followers' requests refused as busy | Followers that sent copies to others | Latecomers: half / all | Latecomers' tries that couldn't connect | Checks failed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Home | `6406df4`, before | 694 / 875 / 1,232 s | 23.9 | 1,747 | 129 | 17.2 / 31.1 s | 0 | 4 |
| Home | `0edaca4`, heard from lately first | 771 / 946 / 1,158 s | 25.8 | 2,180 | 132 | 13.5 / 21.3 s | 0 | 9 |
| A third offline | `6406df4`, before | 739 / 917 / 1,030 s | 25.6 | 1,736 | 117 | 30.4 / 44.6 s | 8 | 4 |
| A third offline | `0edaca4`, heard from lately first | 705 / 896 / 1,157 s | 24.9 | 1,726 | 113 | 21.1 / 31.6 s | 0 | 8 |

- **With a third offline, the change did what it was for.** Before it, the latecomers' tries couldn't connect 8 times, each time to a follower that had stopped. With it, never: half the latecomers had the video within 21.1 s instead of 30.4, and all within 31.6 s instead of 44.6.
- **Followers still fetching when the third stopped** couldn't connect 36 times with the change, against 12 before. That's expected for a while: a follower that stopped at 900 s had been heard from in the last 5 minutes, so it went on being named first until 1,200 s. With the change, the last follower got the video at 1,157 s, against 1,030 s before, so more of the fetching fell in that time.
- **One mark was missed in each code.** With the change, 95% of the followers held the video at 946 s in the home run, 46 s past 15 minutes, against 875 s before it. Before the change, the run with a third offline was the one to miss, at 917 s. On the home run the change names the same followers as before: every follower asks the author's node for new posts every minute, directly, so all count as heard from lately, and the order among them stays random. All it adds there is a lookup in the author's node's database for each request.
- **The whole round was slow.** `6406df4` runs the same node code as the [half-and-half run](#checking-that-a-follower-holds-its-copy) (`f2cfa3a`): that run had 95% at 617 s, 826 refusals as busy and 209 followers sending copies to others, against 875 s, 1,747 and 129 here. All four runs had 113 to 132 followers sending copies, about as few as with the check and the old rule for a full list (130 and 135). That section allowed that one pair couldn't say how much was the change and how much the run. These runs suggest the half-and-half run was a fast one, and whether half and half undoes the full list's slowdown is open again.
- **Checks failed** 4 and 9 times in the home runs (the 9 were for 8 followers; one failed again when it told the author's node once more) and 4 and 8 with a third offline, all from 8 to 15 minutes after posting, while the creator was sending the most. That's more than in the earlier rounds (0 to 5). No social request was refused as busy.
- **The last followers to hold the video had waited on busy nodes, not on unreachable ones.** Of the twelve listed (three per run), ten were refused as busy 10 to 19 times, by 7 to 15 nodes each, the creator among them; none couldn't connect more than once. Four got the whole video from a single follower that was sending to others at the same time. When every node a follower asks is busy, it tries again after about a minute, then two, then four, each give or take half, so a few such rounds in a row cost several minutes. This is the likeliest explanation for the [straggler above](#checking-that-a-follower-holds-its-copy) too.

**A second round.** Added on 2026-10-31, after the first round's results and before any of the second's. The home run with `6406df4` and then with `0edaca4` once more, in the same order.

- **Expected:** the change makes no difference on the home run, so the two runs should differ no more than runs do, and both should meet B8's marks.
- **Half and half:** if `6406df4` again has about 130 followers sending copies to others and more than 1,500 requests refused as busy, the half-and-half run was the fast end of the variation, and the full list's slowdown needs another look, in a change of its own.

**Results of the second round**, 2026-10-31, on the same machine; other programs took at most 5% of its CPUs. The first round's home runs are repeated from above.

| Round | Code | Followers: 50% / 95% / all | Creator's copies | Followers' requests refused as busy | Followers that sent copies to others | Followers' downloads that failed | Followers' tries that couldn't connect | Latecomers: half / all |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `6406df4`, before | 694 / 875 / 1,232 s | 23.9 | 1,747 | 129 | 0 | 10 | 17.2 / 31.1 s |
| 1 | `0edaca4`, heard from lately first | 771 / 946 / 1,158 s | 25.8 | 2,180 | 132 | 32 | 11 | 13.5 / 21.3 s |
| 2 | `6406df4`, before | 675 / 833 / 925 s | 25.1 | 1,771 | 131 | 0 | 3 | 16.4 / 29.8 s |
| 2 | `0edaca4`, heard from lately first | 789 / 960 / 1,397 s | 24.5 | 1,926 | 128 | 57 | 38 | 15.7 / 23.9 s |

- **The change was slower on the home run again,** where it should make no difference: 95% of the followers held the video at 960 s against 833 s, missing the 15-minute mark again, and the last at 1,397 s.
- **Downloads failed with the change, and only with it.** 57 of the followers' downloads failed and 38 tries couldn't connect, against none and 3 before. In the 13 home runs since [switching away from slow hosts](#switching-away-from-slow-hosts), the code before the change never had more than 2 fail; the change had 32 and 57. A download fails when its host stops sending, and a try can't connect when the host's upload is so full that its replies are dropped: both happen to a host asked by more nodes than it can serve. The copies were sent by fewer followers too: the 60 that sent the most sent 87% and 89% of the followers' copies with the change, against 84% and 85% before, and 70% in the half-and-half run.
- **So the change seems to name some followers more often than others even here,** though every follower asks the author's node for new posts directly every minute. One way it could: a request counts as direct only if its connection has picked a direct path when the request arrives (`network::is_direct`), and a connection just opened may not have picked one yet. A follower whose last request came on such a connection would count as reached through a relay, on a network with no relay, and be named after those whose connection was already open. Not checked yet.
- **Half and half:** `6406df4` again had about 130 followers sending copies to others and more than 1,700 requests refused as busy, against 209 and 826 in the half-and-half run, with the same node code. By the end of this round the machine had moved 1.5 GB to swap and spent 76 minutes in all waiting on its disk since it started, a day earlier; the runs record only its CPUs. Whether the half-and-half run was a fast one, or these two rounds were slowed by the machine, isn't settled.

**A third round.** Added on 2026-10-31, after the second round's results and before any of the third's. The machine was restarted before it, and nothing else heavy runs during the round. The home run with `6406df4`, then with `24f649a`: `0edaca4` with a debug log of each sample it names, saying how many of the followers holding the file were heard from lately directly, through a relay, or not lately, and which were named. The round's progress file records, before and after each run, the machine's total time waiting on its disk and its swap in use.

- **Half and half:** if `6406df4` comes out close to the half-and-half run (95% near 620 s, about 200 followers sending copies to others, fewer than about 1,000 requests refused as busy), the first two rounds were slowed by the machine, and half and half stands. If it again has about 130 followers sending copies and more than 1,500 refusals as busy, the half-and-half run was a fast one, and the full list's slowdown needs another look, in a change of its own.
- **The ranking:** on the simulated network every follower should count as heard from directly, so the log should show none through a relay. If some are, requests that arrive before their connection has picked a path are counted wrongly, and that is fixed before this change is merged. If none are, and the change again has dozens of failed downloads where the code before it has none, the names in the log show which followers it names more often, and the cause is found before merging.

**Results of the third round**, 2026-10-31, on the restarted machine: it used no swap and waited on its disk for 282 s during the first run and 374 s during the second; other programs took at most 4% of its CPUs. The first two rounds' home runs are repeated from above.

| Round | Code | Followers: 50% / 95% / all | Creator's copies | Followers' requests refused as busy | Followers that sent copies to others | Followers' downloads that failed | Followers' tries that couldn't connect | Latecomers: half / all |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | `6406df4`, before | 694 / 875 / 1,232 s | 23.9 | 1,747 | 129 | 0 | 10 | 17.2 / 31.1 s |
| 1 | `0edaca4`, heard from lately first | 771 / 946 / 1,158 s | 25.8 | 2,180 | 132 | 32 | 11 | 13.5 / 21.3 s |
| 2 | `6406df4`, before | 675 / 833 / 925 s | 25.1 | 1,771 | 131 | 0 | 3 | 16.4 / 29.8 s |
| 2 | `0edaca4`, heard from lately first | 789 / 960 / 1,397 s | 24.5 | 1,926 | 128 | 57 | 38 | 15.7 / 23.9 s |
| 3 | `6406df4`, before | 615 / 809 / 852 s | 23.4 | 1,336 | 141 | 0 | 3 | 12.7 / 30.2 s |
| 3 | `24f649a`, heard from lately first, with the log | 760 / 939 / 1,110 s | 25.9 | 2,337 | 135 | 88 | 13 | 16.1 / 24.1 s |

- **The change was slower on the home run a third time,** and missed the 15-minute mark a third time, at 939 s against 809 s. 88 of its followers' downloads failed, against none before it. The 60 followers that sent the most sent 86% of the followers' copies with it, against 79% before.
- **It names the same followers as the code before it.** The log has 662 samples, 112 of them from before any follower held the video. No follower counted as not heard from lately, in any sample. One counted as reached through a relay in 13 samples, 13 of the 19,784 followers listed in all: requests that arrive while their connection has no path picked were counted wrongly, as the second round suspected, but too rarely to matter here. They now count the way the follower was last heard, or as direct for a new one (`ef23ad6`). So the order among the followers holding the video stayed random, as before the change, and the second round's explanation is ruled out. Ten followers were named 100 to 208 times each, about half of the 3,146 names; they were the first to hold the video, named in nearly every sample while few others held it, as the code before would name them too: it picked six at random from the same followers.
- **So the cause is something the change does besides ranking,** or the order of the runs: the change ran second in every round. Besides ranking, it reads the author's node's database on every social request, to check that the node asking is a follower, and it reads the whole list of followers holding a file for each sample instead of six. The simulator runs all the nodes in one program, on as many threads as the machine has CPUs, and a database read holds its thread while it waits for the node's database, running nothing else for any node meanwhile. If enough reads wait behind writes to the author's node's database at once, downloads stall without using much CPU.
- **Half and half:** on the restarted machine, `6406df4` was faster than in the first two rounds but still short of the half-and-half run: 95% at 809 s against 617 s, 141 followers sending copies to others against 209, and 1,336 requests refused as busy against 826. So the first two rounds were slowed by the machine only in part, and the half-and-half run looks like a fast one. Whether half and half undoes the full list's slowdown needs another look, in a change of its own.

**A fourth round.** Added on 2026-10-31, after the third round's results and before any of the fourth's. The home run, in this order: `24f649a`, `6406df4`, and then `e786108`. `e786108` is `24f649a` with the fix for paths not yet picked and without the database read on each request: it notes every node that sends a request, and ranks only followers, so a node that isn't one is never named. The fix changes how 13 of about 20,000 listings were counted, so it shouldn't change the result here. The progress file records the machine's disk wait and swap as in the third round.

- **If the order of the runs is the cause,** `24f649a`, run first, is the fastest of the three and has no failed downloads, and `6406df4` is slower than it.
- **If the database read is the cause,** `24f649a` again has dozens of failed downloads and is slower than `6406df4`, and `e786108` is close to `6406df4`, with at most a few failed. `e786108` is then the change to merge.
- **If both rankings are slower and have dozens of failed downloads,** the cause is something else in the change, found before merging.
