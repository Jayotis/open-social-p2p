# Found, watched, shared and hosted, on real nodes

**Status:** [ADR 0025](../decisions/0025-shared-posts-are-found-and-hosted.md)'s step 3, run once on 2026-11-10, on `main` at bde562f with the delete fix below. Kim online and Kim offline met their marks. The deleted post met its marks for a node that opens it, and showed a directory gap, fixed since (below). Two checks are still open (end).

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

**Kim offline** (her node stopped before Alice 2 started). Alice 2 got all 4.0 MB from Jimmy, at 2.2 MB/s, but in 11.8 s in all. About 10 s of that was waiting for Kim's media endpoint to answer who hosts it, before downloading from the sources it already had. Asking in parallel would save that wait (open below).

**What Kim's and Jimmy's nodes kept.** Neither node's database or log holds Alice's public ID, identity or media endpoint. Jimmy's holds nothing of Alice 2's either.

**The post deleted.** Kim deleted it while Alice 3 was starting.

- Alice 3's first read of the directory still listed it.
- Opening it failed in 0.8 s, "its author deleted it": Kim's media endpoint answered with her `Delete`. Nothing was downloaded, and the node kept the tombstone.
- Search no longer showed it, and reading the directory again didn't bring it back.

**A gap, fixed.** The directory had heard of the `Delete` 17 s after Kim deleted, through its follow of Kim, and kept the tombstone. But it went on listing the post, and Jimmy's hosting reshare of it, until its next hourly read of Kim. Only the reading of a listed outbox dropped them. Now a directory drops a deleted post from its lists, and every listed reshare of it, as soon as it learns of the `Delete`, however it arrives. The test `a_directory_that_follows_the_author_drops_a_deleted_post_at_once` fails without the fix.

## Still open

- **Jimmy goes on serving the deleted post** until his node's daily check with Kim's, as ADR 0025 says for a resharer without a follow. That check hasn't been seen to run on these nodes yet.
- **The first Alice, with the video already cached,** still shows the post: she hadn't asked anyone since the delete. Whether she drops it once the directory's list no longer has it is to be checked.
- **The 10 s wait** for an author's media endpoint that's off: ask it alongside the download, rather than first.
