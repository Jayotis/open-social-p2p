# The follower swarm on slow and flaky connections

**Status:** Plan. The runs and pass marks below were fixed on 2026-10-25, before anything was measured. Changing one later needs a dated note here saying what changed and why.

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

None yet.
