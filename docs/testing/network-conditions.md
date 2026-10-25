# The follower swarm on slow and flaky connections

**Status:** The home run met every mark on 2026-10-25. The lossy and flaky runs missed the followers' marks the same day: followers whose download took too long gave up and never tried again, as many did in the slow-hosts run ([results](#results)). The runs and pass marks below were fixed on 2026-10-25, before anything was measured. Changing one later needs a dated note here saying what changed and why.

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

## Results

| Date | Run | Code | Followers holding the video | 50% / 95% / all | Creator sent | Pass marks |
| --- | --- | --- | --- | --- | --- | --- |
| 2026-10-25 | **Home** | `4a6ad0e` | 300 of 300 after 15 minutes; latecomers: half within 17.1 s, all within 26.6 s | 587 s / 775 s / 893 s | 25.0 copies (1,306 MB), all in the first 12 minutes; 247 requests refused as busy; latecomers 0.01 | **all met** |
| 2026-10-25 | Home, upload caps | `4a6ad0e` | 300 of 300 after 14 minutes; latecomers: half within 17.2 s, all within 25.0 s | 553 s / 727 s / 802 s | 19.0 copies (990 MB), all in the first 10 minutes; 190 requests refused as busy; latecomers 0.01 | all met (not judged) |
| 2026-10-25 | **Lossy** | `4a6ad0e` | 283 of 300 after an hour, the last of them at 21 minutes; latecomers: half within 115.4 s, all within 252.7 s | 679 s / not reached / not reached | 18.4 copies (961 MB), all in the first 15 minutes; 235 requests refused as busy; latecomers 0.01 | creator met; **95% and all missed** |
| 2026-10-25 | **Flaky** | `4a6ad0e` | 260 of 300 after an hour, the last of them at 21 minutes; latecomers: half within 15.8 s, all within 17.9 s | 900 s / not reached / not reached | 23.5 copies (1,228 MB), all in the first 17 minutes; 294 requests refused as busy; latecomers 0.01 | creator met; **95% and all missed** |
| 2026-10-25 | Slow hosts, a fast one in ten | `4a6ad0e` | 175 of 300 after an hour, the last of them at 20 minutes; latecomers: half within 145.3 s, all within 150.6 s | 984 s / not reached / not reached | 17.2 copies (897 MB), all in the first 10 minutes; 133 requests refused as busy; latecomers 0.01 | not judged; the followers' marks would be missed |

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
- Each of the 17 that didn't get it gave up prefetching it once, between 13 and 19 minutes after posting. Their download wasn't finished 10 minutes after it started, the most a whole download may take. A prefetch refused as busy is tried again later, but one that fails for any other reason isn't: the media is fetched when someone opens the post.
- 16 of the 17 have the poorest connections, 2% loss: 16 of the 60 followers at 2%. The other has 0.5%. QUIC takes a lost packet as a sign of a busy line and slows down, so at 2% loss a transfer runs far below the line's speed.
- Loss also made followers poor hosts. The 120 followers with no loss sent 255 copies between them; the 60 at each of 0.5%, 1% and 2% sent 20, 6.7 and 5.6.
- The creator, losing 0.5% itself, sent 18.4 copies, fewer than in the home run. Followers' downloads failed 32 times, against none in the home run.
- Latecomers took far longer, half within 115 s and all within 253 s: four of the ten have 1% or 2% loss too. The creator still sent them only their thumbnails.
- Simulator's own trouble: 25 minutes after posting, a build on the same machine filled its disk, and one sync failed with "database or disk is full". By then every give-up had happened and the last follower to get the video had it, so this didn't change the result.

**Flaky.** 30 followers cut off for 30 seconds in every 5 minutes, taking turns.

- 260 of 300 followers held the video after an hour, the last of them at 21 minutes. The marks for 95% and for every follower were missed.
- The 40 that didn't get it gave up their prefetch, as in the lossy run: 39 because their download wasn't finished after 10 minutes, between 14½ and 21 minutes after posting, and one at 2½ minutes because no node it asked could send it.
- Only 4 of the 40 are flaky themselves; 26 of the 30 flaky followers held the video. The other 36 have steady connections. The logs don't say which host each was waiting on; most likely their download stalled on a host that was cut off, waiting for it instead of moving on to another, until its time ran out. The fix will have to check that.
- Followers' downloads failed 140 times, and 30 times a follower couldn't connect to another, against none and 9 in the home run.
- Full router queues dropped 7.8% of packets, against 2.9% in the home run, and 0.1% were lost while cut off. Why the queues dropped so many more isn't known yet.
- Latecomers, who came after the outages had ended, were as quick as on loopback.

**Slow hosts, a fast one in ten.** Followers uploading at 1 Mbit/s, one in ten at 100. Measured, not judged.

- 175 of 300 followers held the video after an hour, the last of them at 20 minutes. The other 125 each gave up their prefetch, between 13 and 20 minutes after posting, because their download wasn't finished 10 minutes after it started: the same as in the lossy and flaky runs, on a larger scale.
- A copy of the video takes 7 minutes from a host at 1 Mbit/s with nothing else to send, and much longer when it's sending several. Even 15 of the 30 fast followers gave up: a download that drew slow hosts stayed with them.
- The 270 followers at 1 Mbit/s sent 130 copies between them, and the 30 at 100 Mbit/s sent 104.
- Latecomers took 144 to 151 s each, about what the video takes from three hosts at 1 Mbit/s at once. That is the figure for choosing hosts by what's been measured of them ([doc 12](../12-peer-reciprocity-and-recognition.md) phase 2) to beat, once followers stop giving up.
- The creator sent 17.2 copies, all in the first 10 minutes.

**What it shows.** On home connections, lossy, dropping out or slow, the swarm kept the creator's upload well within its mark. But on loopback every download finished in time, which hid this: a follower whose download isn't finished 10 minutes after it starts gives up, and nothing tries again until someone opens the post. Next, in its own change: a download's time limit should be for making no progress, not for the whole download, and a prefetch that fails should be tried again later, as one refused as busy is. Then the lossy and flaky runs are repeated against the same marks, and the slow-hosts run measured again.
