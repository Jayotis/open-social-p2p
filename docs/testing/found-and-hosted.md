# Found, watched, shared and hosted, on real nodes

**Status:** [ADR 0025](../decisions/0025-shared-posts-are-found-and-hosted.md)'s step 3, run once on 2026-11-10, on `main` at bde562f with the delete fix below. Kim online and Kim offline met their marks. The deleted post met its marks for a node that opens it, and showed a directory gap, fixed since (below). The checks left for later all passed (end). Step 4, run once each way in `osp-sim` on 2026-11-11: B8 held with and without the place for a recently verified host, which didn't bring newcomers' first uploads sooner, so it is now off by default ([results](#step-4-results)).

## The run

Real nodes on three machines on a home network, all with messages over Tor, `[endpoints]` on and `direct = "nobody"`, so media goes only through n0's relays:

- **The directory,** the first one ([ADR 0015](../decisions/0015-nodes-meet-first.md)), which also follows Kim.
- **Kim,** listed, posts a 20-second 720p video (4.0 MB once transcoded).
- **Jimmy, Alice, Alice 2 and Alice 3,** new nodes that follow nobody. Jimmy asks to be listed; the Alices don't.

## Results

**Jimmy finds, watches and shares.**

- He found Kim's video through the directory 21 minutes after she posted it: the directory reads those it lists once an hour.
- Watching it took 2.6 s, through a viewing endpoint ([ADR 0024](../decisions/0024-watching-found-posts-without-your-identity.md)).
- He shared it with hosting. Kim's node was told, checked a piece of his copy, and noted him as a listed hosting resharer, about 15 s later.
- The directory checked his copy and listed his reshare as hosting at its next read of him, 9 minutes later.

**Kim online.** Alice found the post through the directory and opened it. All 4.0 MB came from Jimmy, at 2.0 MB/s, first byte after 555 ms, 2.8 s in all. The file matched Jimmy's byte for byte. Alice follows nobody.

**Kim offline** (her node stopped before Alice 2 started). Alice 2 got all 4.0 MB from Jimmy, at 2.2 MB/s, but in 11.8 s in all. About 10 s of that was waiting for Kim's media endpoint to answer who hosts it, before downloading from the sources it already had. Fixed since: a node now downloads from the hosts it was given while it asks the author's media endpoint, and waits for her at most 3 s more for a `Delete`.

**What Kim's and Jimmy's nodes kept.** Neither node's database or log holds Alice's public ID, identity or media endpoint. Jimmy's holds nothing of Alice 2's either.

**The post deleted.** Kim deleted it while Alice 3 was starting.

- Alice 3's first read of the directory still listed it.
- Opening it failed in 0.8 s, "its author deleted it": Kim's media endpoint answered with her `Delete`. Nothing was downloaded, and the node kept the tombstone.
- Search no longer showed it, and reading the directory again didn't bring it back.

**A gap, fixed.** The directory had heard of the `Delete` 17 s after Kim deleted, through its follow of Kim, and kept the tombstone. But it went on listing the post, and Jimmy's hosting reshare of it, until its next hourly read of Kim. Only the reading of a listed outbox dropped them. Now a directory drops a deleted post from its lists, and every listed reshare of it, as soon as it learns of the `Delete`, however it arrives. The test `a_directory_that_follows_the_author_drops_a_deleted_post_at_once` fails without the fix.

**Checked afterwards** (2026-11-10 and 11):

- **The directory's lists.** By the next morning the directory listed the post nowhere: neither under Kim nor as Jimmy's reshare.
- **The first Alice,** with the video already cached, had the post nowhere once she read the directory again: not in what she found, not in `show`, not in search. Her cached copy of the video (4.2 MB) stays on disk until the cache evicts it. It isn't shown or served.
- **Jimmy stopped serving.** His node's daily check with Kim's learned of the `Delete` about 16 hours after it, and he let go of his hosted copy: 2 files, 4.0 MB, down to none.

## Step 4: newcomers in `osp-sim`

The step's gate: B8 with a few hundred newcomers arriving over hours who find rather than follow, some sharing with hosting, run with and without the place for a recently verified host. Plan and marks fixed on 2026-11-11, before the runs.

**Before the runs, two fixes** (`c8860e4`):

- **Viewers get the place too.** Kim's media endpoint, which a newcomer's viewing endpoint asks who hosts a post, named resharers without the place: only `WhoHosts` gave it, and newcomers never send `WhoHosts`. Without this fix, the two runs couldn't differ for newcomers.
- **The place goes round.** With the 6 resharers named all taken, the place took the sixth of the day's draw, which still counted as named, so it could never win the place back and went unnamed all day. It now competes with the rest by how often each was named.

**The scenario.** The home run of [media through relays](network-conditions.md#media-through-relays) with a fifth of the followers behind the relay, which met every B8 mark. Today's default, every node behind the relay, already misses the 95% mark without newcomers, so it can't show whether they change B8. Then, in the same run:

- **A directory** lists Kim. She asks to be listed once she has posted, so its first read of her, within a minute, lists the post.
- **Once the followers and latecomers are done, 200 newcomers arrive,** evenly over 3 hours, following nobody. Each reads the directory as it arrives and opens the video through a viewing endpoint ([ADR 0024](../decisions/0024-watching-found-posts-without-your-identity.md)). It tries the hosts the directory lists first, one at a time, then those Kim's media endpoint names, and Kim's own last.
- **Every fifth newcomer, the first included, shares the video with hosting,** 40 in all, and asks to be listed at once. A real node asks within the hour. The directory lists a new node at its next read, within a minute, and reads it hourly after that. Kim's node checks a sharer's copy within seconds and names it.
- **Every other timing is a real node's.** Uploads are 5, 10 and 20 Mbit/s in turn, as for followers.

```sh
osp-sim --dir /path/on/disk/run --swarm --net --latecomers 10 --tor-delay \
  --endpoints anyone --relay --relay-only 20 \
  --newcomers 200 --newcomers-over-mins 180 --newcomer-share-percent 20 \
  [--recent-host-place] --label "$(git rev-parse --short HEAD)" --out run.json
```

**Recorded:**

- **First uploads.** When each sharing newcomer first served someone: at least 1 MB to anyone but Kim's and the directory's nodes, whose checks take a small piece.
- **From earlier newcomers.** The share of what newcomers received that came from earlier newcomers, for all of them and for the later half.
- **Kim's copies.** What Kim sent the newcomers, in copies of the video.
- **Opening times,** and any newcomer that couldn't open the video.

**Pass marks.** One run with the place and one without, on the same code, one after the other, on the NVMe partition.

1. **B8 holds in both:** the creator sends at most 30 copies, 95% of the followers hold the video within 15 minutes, and all within 30.
2. **The place is kept as a policy only if newcomers' first uploads come sooner with it.** The sharers judged are those that shared at least 30 minutes before the last newcomer arrived, so each had the same time to be asked. "Sooner" means a lower median time from sharing to first serving, a sharer that never served counting as never. If neither run's median is reached, it means more judged sharers served within 30 minutes.

If B8 holds and the place doesn't make first uploads sooner, `recent_host_place` goes off by default and stays a setting. If B8 misses in either run, the results go to the maintainer before anything changes.

### Step 4 results

Run on 2026-11-11 at e652f02 (`main` 25bed68), with the place and then without.

**1. B8 held in both.**

| | With the place | Without |
|---|---|---|
| Kim's copies to followers (at most 30) | 26.2 | 23.2 |
| 95% of followers held the video (within 15 min) | 645 s | 636 s |
| All held it (within 30 min) | 869 s | 755 s |
| Latecomers, all held it | 69.7 s | 59.2 s |

**2. The place didn't bring first uploads sooner.** In both runs, 2 of the 33 judged sharers served someone within 30 minutes, and 2 of all 40 by the end. Neither run reached a median, and neither had more sharers serving, so the place goes off by default (`swarm.recent_host_place = false`) and stays a setting; `osp-sim --recent-host-place` turns it on.

**Newcomers otherwise did well, the same in both runs.**

- All 200 opened the video: median 167 s with the place, 163 s without; slowest 231 s and 233 s. None failed.
- 98% of what newcomers received came from earlier newcomers (100% for the later half). Kim sent them 3.8 copies in all.
- One sharer carried nearly all of it: the most one newcomer sent was 122.7 copies with the place, 121.7 without.

**Why the place made no difference.** A viewer downloads from one source at a time and tries the hosts the directory lists first, so the first listed sharer serves nearly every newcomer and the rest are rarely asked. The place only changes whom Kim's media endpoint names, which a viewer reaches only after the directory's hosts. Spreading newcomers across sharers needs a viewer to download from several sources at once, as ADR 0025 says (the first 3); that gap is open.

## Still open

- **A viewer downloads from one source at a time.** ADR 0025 says the first 3 at once. Until then, the first host a directory lists serves nearly every newcomer (step 4).
- **A found post's cached files** could be let go as soon as the post drops out, rather than at eviction.
