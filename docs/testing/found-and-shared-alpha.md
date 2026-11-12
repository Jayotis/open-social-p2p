# Found and shared, in the invited alpha

**Status:** Plan for [ADR 0025](../decisions/0025-shared-posts-are-found-and-hosted.md)'s step 5, its scripted alpha exercise. Drafted and accepted by the maintainer 2026-11-11, with check 4 allowing for hosting checks. It runs in the invited alpha ([roadmap](../06-roadmap.md)), once testers have the installer ([docs/install.md](../install.md), which sets their directories; [ADR 0027](../decisions/0027-directory-bootstrap-list-and-mirror.md)'s signed list comes after the alpha). A dry run on the maintainer's own machines comes first. It runs on the minimum as built: the directory lists up to 16 hosting reshares, with no rotation (ADR 0025, amended 2026-11-11). Changing the plan after it starts needs a dated note here saying what changed and why.

[The model](found-over-weeks.md) showed that sharing with hosting carries a video at 5 to 20% sharing, but those rates were guesses. This asks real people.

## What it's for

1. **Do people share with hosting** when nobody asks them to?
2. **Do those who share later serve anything,** once others have shared before them?
3. **What's the real sharing rate?**

And whether the mechanism behaves as built (below). If it does, ADR 0025's rotation is the next thing built.

## Who's in it

- **Kim,** the maintainer's listed node, posts one video of about a minute (720p, roughly 10 to 15 MB once transcoded). Nothing else about it is announced. It's an ordinary clip someone could plausibly share or ignore, with an ordinary caption: nothing calls it a test or asks for shares, as that would sway the very choice being observed. The search words are two or three distinctive ordinary words from the caption.
- **Testers,** the alpha's invited people, each on their own node, listed by the directories (a condition of taking part: check 2 needs every hosting sharer listed, and a sharer who isn't would fail it for a reason that has nothing to do with ADR 0025). Each gets a slot, at least 2 hours after the one before: a new sharer is listed 0 to 2 hours after sharing, so everyone who shared before a slot is listed by then. Slots are scheduled 2½ hours apart, for a tester starting a little late or a directory read landing near the boundary.

## What each tester does, in their slot

1. Searches for the video's words (`osp search`) and opens it (`osp fetch`). A search first reads the directories, unless the node read them in the last 10 minutes, so it sees sharers listed up to then.
2. Decides whether to reshare it, with hosting or without, or not at all. The script says what each choice means, in the CLI's own words, and doesn't recommend one. Being asked to share would make the rate meaningless.
3. Notes the time and their choice.

After the last slot plus a day, each tester sends the maintainer the `Shared:` line from `osp status`, their choice and their slot, **if they choose to**. The count stays on their node otherwise.

## The script testers get

Reviewed by the maintainer 2026-11-12. The maintainer sends each tester this, with their slot's time and the words to search for. It describes each choice in the CLI's and ADR 0025's own words, and recommends none.

> **Before your slot.** Install your node ([docs/install.md](../install.md)) at least an hour before, to give Tor and the directory setup time to settle.
>
> **For this exercise your node needs to be listed by the directories:** answer yes to "Ask directories to list you?" at install, or set `list_me = true` under `[directory]` in `~/.local/share/osp/config.toml`, then `systemctl --user restart osp`. Listing makes your profile and your latest public posts and shares visible to anyone reading those directories. If you don't want to be listed, please don't take part in this exercise.
>
> **In your slot:**
>
> 1. Search for the words you were sent: `osp search <words>`. If nothing shows, run `osp directory read`, and search again when it finishes. The video's line starts with its author's name, and ends with `id: …`.
> 2. Open it: `osp fetch <id> --out ~/Videos`, and watch as much or as little as you normally would. Note whether it opened.
> 3. Decide whether to share it. Any of these is fine; choose what you'd do if nobody were asking:
>    - **Don't share it.** Nothing to do.
>    - **Share it:** `osp reshare <id>`. Your reshare is signed by you and shows your profile to anyone who sees it. Your node doesn't keep a copy for others.
>    - **Share it and host it:** `osp reshare <id> --host`. This also keeps a copy of its media and serves it to others. Anyone can see that you shared this and that your node serves it. This public hosting copy is served from your relay-only media endpoint, so it doesn't reveal your home IP address. Others keep what they've seen even if you delete it later. It uses some of your upload, which `max_upload_mbps` in config.toml can cap.
>
>    To take a share back later: `osp delete <reshare-id>`. Use the new ID printed after "Published" when you shared, not the video's original ID. It can't be undone, and copies people saved stay.
> 4. Write down the time and what you chose.
>
> **A day after the last slot,** if you choose to, send the maintainer your slot, what you chose, and the `Shared:` line from `osp status`. If you'd rather not, nothing is sent: the count stays on your node.

## What's recorded

- **The sharing rate:** testers who reshared with hosting, out of those who opened the video. Also how many reshared without hosting.
- **For each sharer,** by slot: bytes served from what they shared, and how many testers opened the video after they were listed.
- **Kim's side:** what Kim's node served of the video, and each sharer's listing time from the directory.
- **Opens:** whether each succeeded, as the tester reports.

Small numbers: a handful of testers can't show a rate to a percent. The figures are recorded, not judged against a mark.

## Whether the mechanism behaves

Any of these failing goes to the maintainer before the rotation is built:

1. **Every open succeeds.**
2. **Every sharer with hosting is listed as hosting** within 2 hours of sharing.
3. **Sharers serve:** if anyone opened the video after the first sharer was listed, sharers served more than nothing in all.
4. **The count is believable:** no sharer counts more than the video's size times the testers who opened it after they shared, plus the bytes of the hosting checks of their copy. Each check reads one 1 KiB piece. Kim's node and each directory check when told, then once a day, so this allows 1 KiB × checkers × (days since sharing + 1). That's about 16 KiB for two checkers over a week.

## What stays private

Testers' names, nodes, choices and counts stay in the maintainer's private notes. Only totals and slot numbers go here.

## Dry run

Before the testers, the same script runs on the maintainer's three machines, one as Kim and two as testers, with one sharing with hosting. It checks the script's commands and that the `Shared:` count rises on the sharer. Results here, as totals.

**Run 2026-11-11 to 12:** Kim posted one 60 s, 11.6 MB video. Two testers took part, both in slots: the first shared with hosting and the second didn't share. All four checks passed:

1. Both opens succeeded, 11 s each, and the files matched.
2. The sharer was listed as hosting 7 minutes after sharing. Kim's node had noted them as a hosting resharer 33 s after sharing.
3. The sharer served the second tester the whole video.
4. The sharer's `Shared:` count was the video's size plus two 1 KiB hosting checks.

What changed in the script: the first tester found the post about 5 hours after it was posted, and the second tester's node hadn't read the directory again before its slot. A node reads its directories only every 6 hours or so, and `osp sync` doesn't start a read. `osp directory read`, run by hand on the second tester's node afterwards, took 5 s and picked up the hosting listing. So a search or opening the feed now reads the directories too ([ADR 0015](../decisions/0015-nodes-meet-first.md), amended 2026-11-12), and testers need no extra step. Also, `osp show` on the post didn't say who hosts it, so the maintainer checked listings on the directory's side. Since 2026-11-12, search and `osp show` list a found post's reshares that came with it, and which host a copy; the directory's side stays the record for check 2's listing time.
