# B8: the creator's upload, with and without the follower swarm

**Status:** The swarm run passed both marks on 2026-10-24, and the other three runs were measured the same day ([results](#results)). The pass marks below were fixed that day, before anything was measured. Changing one later needs a dated note here saying what changed and why. Marks for repeating the strangers run once resharers keep track of copies were added on 2026-10-24, also before that run ([below](#strangers-once-resharers-keep-track-of-copies)), and the repeated run met all of them the same day.

The [critical review](../reviews/2026-10-21-review-of-docs-12-and-13.md#b8-the-first-wave-still-hits-the-creator-serious) found that when someone posts, every follower's node fetches the media at once, so a home connection is swamped by the first wave (finding B8). The follower swarm ([doc 13](../13-hosting-roles-and-follower-swarm.md)) is meant to fix this: fetches are spread out, and early fetchers serve later ones. Doc 13 can't be accepted until this test shows that it works. The review requires the numbers to be written down first, so that results can't be read to fit afterwards.

## What happens today

- The creator's node serves at most 8 transfers at once. A request that finds no free slot within 10 seconds is refused as busy.
- A follower refused as busy asks the nodes it follows who else hosts the file. Only the creator has it, so the follower gives up. The media is fetched later, from the creator, when someone opens the post.
- Followers' prefetched copies are private, served only to their own phones.

Since the baseline was measured, a prefetch refused as busy is tried again later, waiting longer each time, and small files such as thumbnails have serving places of their own ([ADR 0003](../decisions/0003-native-social-protocol-v0.md)). Both apply whether or not the swarm is switched on.

So today a creator with 300 followers sends a few copies at once, and most followers don't get the media ahead of time. If every follower kept trying until it got the file, the creator would send all 300 copies: about 15.7 GB, which takes about **1.7 hours** at 20 Mbit/s.

## The harness

`osp-sim`, a new program in the workspace, used only for testing. It runs one creator and a few hundred followers as **real nodes in one process**: the same code as `osp daemon`, each with its own home directory, database and blob store, talking over loopback with relays off. A trial run on an ordinary desktop started 300 nodes in 38 seconds, using about 1 MB of memory each.

- **Upload speed.** Each node's upload is held to the speed of a home connection with the existing upload cap (`max_upload_mbps`). Downloads aren't limited, and loopback adds no delay or loss. Home connections are much faster down than up, so the upload is what runs out.
- **Followers already subscribed.** A node accepts at most 100 new followers an hour, a guard against floods. A creator with 300 followers gathered them over time, so the harness lifts that limit while it sets up. Settings like this one sit behind a build option (a Cargo feature) that only the harness turns on, so a real node can't use them.
- **What it measures**, all times counted from the moment of posting:
  - the bytes the creator sent, taken from its own peer record ([ADR 0011](../decisions/0011-measuring-peers.md)): "served to it", summed over all peers;
  - when each follower holds the whole verified file, found by checking each follower's feed once a second;
  - each follower's bytes sent, busy refusals and failed downloads, from the same records.
- **The video** is random noise, made with ffmpeg and posted through the creator's node like any other. Noise can't be compressed, so the node's encoder runs at its 1.5 Mbit/s ceiling and the size comes out as planned.
- **Output:** a summary for people to read, and optionally the full report as JSON. Copy the summary into [Results](#results) below.
- **In CI:** a small run (12 followers, 1 MB) as a test, so the harness keeps working. That's few enough that the creator's transfer limits never turn a follower away, which keeps the test steady. A second small run (12 followers, 2 MB, fetches spread over 10 seconds) has the swarm on, and two more add to it a quarter of the followers stopping, and 12 strangers after a reshare (with fetches spread over 30 seconds, so the first strangers hold a copy before the last ask who has one). The full runs below are started by hand: they take up to an hour and about 16 GB of disk, 26 GB with the strangers.

### Running it

Use a directory on a real disk: on many systems `/tmp` is held in memory. The directory must be empty or not exist yet, and is deleted afterwards (`--keep` keeps it).

```sh
cargo run --release -p osp-sim -- --dir /path/on/disk/b8-run \
  --label "$(git rev-parse --short HEAD)" --out b8-run.json
```

The defaults are the scenario below; `osp-sim --help` lists the options (followers, video size, upload speeds, time limit). Progress is logged every minute. Add `--swarm` for the swarm runs: followers then share what they prefetch, and spread their fetches of the video over up to 10 minutes, as a real node with `share_prefetched` on does (`--spread-secs` changes that). For the other runs below:

- **Fewer followers:** `--followers 100`.
- **Followers dropping out:** `--drop-out 60` stops 60 followers, the last of each group of five, so every upload speed loses some. By default they stop 8 minutes after posting (`--drop-out-secs`), when about 40% of followers held the video in the swarm run and the creator had sent all it would. The times count only the followers that stayed.
- **Strangers after a reshare:** `--strangers 200` adds 200 nodes that follow only the first follower. It reshares the post, hosting its media, as soon as it holds the video. The summary gives the strangers' times, from posting and from the reshare, what the resharer sent, how many copies the creator sent strangers, and the strangers' [marks](#strangers-once-resharers-keep-track-of-copies).

What loopback can't show: real delay and loss, NAT and relays. Those belong to the slow and flaky network harness, a later M3 item.

## The scenario

- One creator, **300 followers**, all online and subscribed.
- The creator posts one **52 MB video**: the size of a 5½-minute clip at the v1-720p profile ([ADR 0005](../decisions/0005-media-profile-v1.md)).
- **Upload speeds:** the creator 20 Mbit/s. Followers, a third each at 5, 10 and 20 Mbit/s.
- Each run ends when every follower has the file, or one hour after posting.

## Pass marks

The follower swarm passes B8 when, in the scenario above, **both** hold:

1. **The creator's upload falls substantially:** it sends at most **30 copies** (1.57 GB) in total, a tenth of the 300 it would send alone. At 20 Mbit/s it could send about 43 copies in the first 15 minutes, so most of the copies have to come from the swarm.
2. **Followers still get the media promptly:** 95% of followers hold the verified file within **15 minutes** of posting, and every follower within **30 minutes**. Doc 13 suggests spreading fetches over 0 to 10 minutes; that's a starting point and can change, but these two times can't.

Recorded too, without a pass mark:

- the time until 50%, 95% and 100% of followers hold the file;
- the creator's upload over time, minute by minute;
- what each follower sent: most, median, and how many sent nothing;
- busy refusals and failed downloads.

### Strangers, once resharers keep track of copies

Fixed on 2026-10-24, before the run was repeated. The first strangers run (below) was only compared with the marks. Once a resharer that hosts keeps track of which of its followers hold a copy ([doc 13](../13-hosting-roles-and-follower-swarm.md#strangers-arriving-from-outside) phase 2), the strangers run is repeated and judged. It passes when **all** of these hold:

1. **The creator's upload stays low:** it sends at most **50 copies** in total. That is the same rule as above, a tenth of the nodes that get the video, now 300 followers and 200 strangers. The first strangers run sent 160.
2. **Strangers get the media promptly:** 95% of strangers hold the verified file within **15 minutes of the reshare**, and every stranger within **30 minutes of it**. Strangers can't start before the reshare exists, so their clock starts then.
3. **Followers still meet their marks:** 95% within 15 minutes of posting and all within 30.

## Runs

| Run | Code | What it shows |
| --- | --- | --- |
| **Baseline** | today's `main` | where things start: how many followers get the file in the hour, and what the creator sends |
| **Swarm** | swarm basics, switched on | **the pass marks are judged on this run** |
| Fewer followers | swarm, 100 followers | whether the creator's upload grows with the number of followers |
| Followers dropping out | swarm, a fifth of the followers stop partway through | whether the spread recovers when sources disappear |
| Strangers after a reshare | swarm, plus 200 nodes that don't follow the creator fetching the post through a reshare | the cost of `WhoHosts` pointing strangers only at the creator and resharers (doc 13) |
| Strangers, resharers' swarms | the same, with doc 13 phase 2 | whether strangers that follow a resharer can get the video from each other; judged on the [marks above](#strangers-once-resharers-keep-track-of-copies) |

Only the swarm run and the second strangers run are judged. The others are compared with the same marks, and reported either way.

## Results

| Date | Run | Code | Followers holding the video | 50% / 95% / all | Creator sent | Pass marks |
| --- | --- | --- | --- | --- | --- | --- |
| 2026-10-24 | Baseline | `e61641f` | 8 of 300 after an hour | not reached | 8 copies (418 MB), all in the first 3 minutes; 1,130 requests refused as busy | creator met; 95% and all missed |
| 2026-10-24 | **Swarm** | `9d9c982` | 300 of 300 after 14 minutes | 509 s / 671 s / 843 s | 20.2 copies (1,055 MB), all in the first 8 minutes; 117 requests refused as busy | **all met** |
| 2026-10-24 | Fewer followers | `d1c4662` | 100 of 100 after 12 minutes | 417 s / 668 s / 708 s | 9.1 copies (473 MB), all in the first 4 minutes; 8 requests refused as busy | all met (the creator's mark is a tenth of the followers: 10 copies) |
| 2026-10-24 | Followers dropping out | `d1c4662` | 240 of the 240 that stayed, after 13 minutes | 462 s / 642 s / 761 s | 17.2 copies (897 MB), all in the first 7 minutes; 118 requests refused as busy | all met |
| 2026-10-24 | Strangers after a reshare | `d1c4662` | followers: 300 of 300 after 15 minutes; strangers: 152 of 200 after an hour | followers: 562 s / 742 s / 900 s; strangers: 2,624 s / not reached / not reached | 160.4 copies (8,367 MB): 24.2 to followers, 136.2 to strangers, still sending when the hour ended; 1,101 requests refused as busy | followers' times met; creator missed (what it sent followers alone would meet it); strangers missed both times |
| 2026-10-24 | **Strangers, resharers' swarms** | `2fd6be4` | followers: 300 of 300 after 13 minutes; strangers: 200 of 200, 12 minutes after the reshare | followers: 470 s / 692 s / 755 s; strangers, from the reshare: 395 s / 646 s / 734 s | 32.2 copies (1,681 MB): 20.2 to followers, 12.0 to strangers, none after 16 minutes; 196 requests refused as busy | **all met** |

**Baseline.** 300 followers subscribed in 5 seconds; the video came out at 52.2 MB.

- The 8 followers that got one of the creator's 8 transfer places shared its 20 Mbit/s and had the video at 171 s.
- Every other follower was refused as busy, asked who else hosts the video, was refused again, and gave up. Nothing tried again during the hour, so the creator's upload sat idle for the remaining 57 minutes while 292 followers didn't have the video.
- 274 followers didn't get the thumbnail either: all 8 places were taken by videos, so the thumbnails, 37 KB each, were refused too.
- Followers sent nothing, since their prefetched copies are private.
- The creator's pass mark is met only because almost nothing was delivered. That's why the swarm run has to meet both marks.

**Swarm.** The same scenario with `--swarm`: followers share what they prefetch, and each waited a random 0 to 10 minutes before fetching the video.

- The creator sent 20 copies of the video, and the thumbnail to every follower: 20.2 copies of the media, two thirds of what the mark allows. The followers sent the other 280 copies between them.
- The first followers held the video at 161 s. The creator's copies rose from 8 at 3 minutes to 20 at 8 minutes, then stayed at 20 while the rest of the followers fetched from each other.
- Half the followers held it at 8½ minutes, 95% at 11 minutes, and all 300 at 14 minutes. The spread of up to 10 minutes accounts for most of that.
- 119 followers sent something; the busiest sent 11 copies, and 181 sent nothing. Followers at 5, 10 and 20 Mbit/s sent 99, 76 and 105 copies: slow followers served about as much as fast ones, each copy just taking longer.
- Followers' downloads: 600 completed, 515 refused as busy, none failed or sent bad data. Each busy refusal cost the requester up to 10 seconds of waiting before it moved on to the next host. That cost was paid without missing a mark here, but it is the first thing to look at if a later run is slow.
- Loopback adds no delay or loss, every follower stayed online, and none were strangers. The runs below test followers stopping and strangers; delay and loss wait for the slow and flaky network harness.

**Fewer followers.** The swarm run with 100 followers. Run with `main` at `d1c4662`, whose node code is the same as `9d9c982`'s: only the harness changed.

- The creator sent 9.1 copies, all in the first 4 minutes: 8 to the followers that took its 8 transfer places first, then one more. Followers sent the other 91 between them; the busiest sent 6, and 53 sent nothing.
- With a third of the followers, the creator sent about half as many copies: 9 here, 20 for 300 followers. Its first copies go to whoever fetches first, before any follower holds one, so its share of the copies falls as the audience grows: 9% here, 7% for 300.
- 95% of followers held the video at 11 minutes and all at 12. Only 19 downloads were refused as busy, against 515 for 300 followers.

**Followers dropping out.** The swarm run with 60 followers, the last of each group of five, stopping 8 minutes after posting. By then 163 followers held the video, 32 of them among those stopping. The times count the 240 that stayed.

- The creator had sent its 17.2 copies by 7 minutes, before anyone stopped, and sent nothing after. The 240 that stayed got the rest from each other, and as promptly as in the swarm run: 95% at 642 s and all at 761 s.
- Losing sources cost little. 9 downloads failed when their source stopped partway, and 7 times a follower couldn't connect to a follower that had stopped but was still named as holding a copy, costing a 10-second connect timeout each time. The two slowest followers finished about 70 seconds after the rest.
- The log has one error per stopped follower, from the blob store's clean-up: it runs every 10 minutes and outlives a node that stops, so it fails once and ends. A real node's process ends when it stops, so this shows only when nodes stop inside a process that keeps running, as here.

**Strangers after a reshare.** The swarm run plus 200 nodes that follow only the first follower, which uploads at 5 Mbit/s and reshared the post with hosting at 574 s, once it held the video.

- Followers were hardly affected: all 300 held the video at 15 minutes, against 14 without strangers, and the creator sent them 24.2 copies.
- Strangers were slow: half held the video after 44 minutes, and 152 of 200 after an hour. They had two sources. The resharer sent 31 copies, as many as its upload allowed. The creator sent 136, and once the followers were done its 4 places for strangers took its whole upload, about 3 copies a minute until the hour ended.
- Strangers sent each other nothing, though they share what they prefetch. Nobody names a stranger with a copy to another stranger: the creator's node names to strangers only hosts that are public by choice, and the resharer's node doesn't keep track of which of its followers hold a reshared post's media. Strangers also fetch a reshared post's media at once instead of spreading their fetches out, so all 200 asked together: 2,925 of their requests were refused as busy.
- 13 strangers gave up prefetching the video: 5 after being refused as busy too many times, and 8 after a download took longer than the 10 minutes a download may take. The resharer's 5 Mbit/s shared by 8 transfers gives each about 0.6 Mbit/s, at which a 52 MB copy takes about 11 minutes. That is also below the video's 1.5 Mbit/s, so a node with a slow upload should perhaps offer fewer places at once.
- This is the cost [doc 13](../13-hosting-roles-and-follower-swarm.md#strangers-arriving-from-outside) expected of naming followers only to followers, now measured. Its proposed next step is that a resharer that hosts keeps track of its own followers' copies, as the creator's node does; this run would then be repeated with pass marks fixed first.

**Strangers, resharers' swarms.** The strangers run again with doc 13 phase 2, judged on the [marks fixed for it](#strangers-once-resharers-keep-track-of-copies). The resharer's node keeps track of which of its followers hold a copy and names a few to its other followers. Strangers spread their fetches out, ask it who has a copy, try the followers it names first and the creator's node last, and tell it when they hold a copy. The same code gives a node with an upload cap one transfer place per 1.5 Mbit/s, between 2 and 8: in the first strangers run, the resharer's 5 Mbit/s shared by 8 transfers was too slow to finish a copy within the 10-minute download limit. This run measures the two changes together. The resharer, uploading at 5 Mbit/s, reshared at 671 s, once it held the video.

- Every stranger held the video 12 minutes after the reshare, and 95% at 11 minutes, against 152 of 200 after an hour in the first strangers run. As for followers, the spread of up to 10 minutes accounts for most of that.
- The creator sent 32.2 copies in all, against 160 before: 20.2 to followers and 12 to strangers. The 12 went out between 2 and 5 minutes after the reshare, before strangers held copies of their own, and the creator sent nothing after that.
- Strangers sent each other 185 copies: 98 of them sent something, the busiest 9. The resharer sent 3.1 copies, against 31 before. With its followers naming each other, it's mostly a source of names rather than of copies.
- Busy refusals fell: 214 for strangers' downloads, against 2,925, and 196 at the creator, against 1,101. No download failed and no stranger gave up, against 35 failed downloads and 13 strangers giving up before.
- Followers met their marks as in the swarm run (95% at 692 s, all at 755 s), and the creator sent them 20.2 copies, the same as then. Their downloads were refused as busy 687 times, against 515 in the swarm run: followers with a slow upload now offer fewer places at once.
- The swarm run itself hasn't been repeated with the fewer places; this run's followers are the nearest measure.
