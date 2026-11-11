# Found posts over weeks, in a model

**Status:** Plan for [ADR 0025](../decisions/0025-shared-posts-are-found-and-hosted.md)'s step 5, fixed 2026-11-11 before any code or run; the maintainer accepted the pass marks the same day. Not yet run. Changing one later needs a dated note here saying what changed and why.

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
