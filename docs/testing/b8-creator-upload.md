# B8: the creator's upload, with and without the follower swarm

**Status:** Plan. The pass marks below were fixed on 2026-10-24, before anything was measured. Changing one later needs a dated note here saying what changed and why.

The [critical review](../reviews/2026-10-21-review-of-docs-12-and-13.md#b8-the-first-wave-still-hits-the-creator-serious) found that when someone posts, every follower's node fetches the media at once, so a home connection is swamped by the first wave (finding B8). The follower swarm ([doc 13](../13-hosting-roles-and-follower-swarm.md)) is meant to fix this: fetches are spread out, and early fetchers serve later ones. Doc 13 can't be accepted until this test shows that it works. The review requires the numbers to be written down first, so that results can't be read to fit afterwards.

## What happens today

- The creator's node serves at most 8 transfers at once. A request that finds no free slot within 10 seconds is refused as busy.
- A follower refused as busy asks the nodes it follows who else hosts the file. Only the creator has it, so the follower gives up. The media is fetched later, from the creator, when someone opens the post.
- Followers' prefetched copies are private (`cache`), served only to their own phones.

So today a creator with 300 followers sends a few copies at once, and most followers don't get the media ahead of time. If every follower kept trying until it got the file, the creator would send all 300 copies: about 15.7 GB, which takes about **1.7 hours** at 20 Mbit/s.

## The harness

`osp-sim`, a new program in the workspace, used only for testing. It runs one creator and a few hundred followers as **real nodes in one process**: the same code as `osp daemon`, each with its own home directory, database and blob store, talking over loopback with relays off. A trial run on an ordinary desktop started 300 nodes in 38 seconds, using about 1 MB of memory each.

- **Upload speed.** Each node's upload is held to the speed of a home connection with the existing upload cap (`max_upload_mbps`). Downloads aren't limited, and loopback adds no delay or loss. Home connections are much faster down than up, so the upload is what runs out.
- **Followers already subscribed.** A node accepts at most 100 new followers an hour, a guard against floods. A creator with 300 followers gathered them over time, so the harness lifts that limit while it sets up. Settings like this one sit behind a build option (a Cargo feature) that only the harness turns on, so a real node can't use them.
- **What it measures**, all times counted from the moment of posting:
  - the bytes the creator sent, taken from its own peer record ([ADR 0011](../decisions/0011-measuring-peers.md)): "served to it", summed over all peers;
  - when each follower holds the whole verified file, found by checking each follower's feed once a second;
  - each follower's bytes sent, busy refusals and failed downloads, from the same records.
- **Output:** one JSON file per run, and a summary table to copy into [Results](#results) below.
- **In CI:** a small run (20 followers, 1 MB) as a test, so the harness keeps working. The full runs below are started by hand: they take up to an hour and about 16 GB of disk.

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

## Runs

| Run | Code | What it shows |
| --- | --- | --- |
| **Baseline** | today's `main` | where things start: how many followers get the file in the hour, and what the creator sends |
| **Swarm** | swarm basics, switched on | **the pass marks are judged on this run** |
| Fewer followers | swarm, 100 followers | whether the creator's upload grows with the number of followers |
| Followers dropping out | swarm, a fifth of the followers stop partway through | whether the spread recovers when sources disappear |
| Strangers after a reshare | swarm, plus 200 nodes that don't follow the creator fetching the post through a reshare | the cost of `WhoHosts` pointing strangers only at the creator and resharers (doc 13) |

Only the swarm run is judged. The others are compared with the same marks, and reported either way.

## Results

None yet.
