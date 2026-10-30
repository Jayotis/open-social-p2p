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
