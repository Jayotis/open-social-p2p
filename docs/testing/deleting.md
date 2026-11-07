# Deleting a post in the simulator

**Status:** ADR 0019's step 1 gate, run once on 2026-11-06: its marks were met ([results](#results)). The marks are [ADR 0019](../decisions/0019-taking-posts-and-media-down.md#building-it)'s, set before the run was written.

A post's author signs a `Delete`; followers drop the post, its reshares and its files, and a node that reshared it passes the `Delete` on to its own followers ([how it works](stage1-manual.md#running-a-node-by-hand)). This run checks that it reaches every node that holds the post, through two levels of reshares, including nodes that were offline, and that no old copy brings it back.

## The run

`osp-delete` starts 109 nodes in one process, each on a fast link of the [simulated network](network-conditions.md#the-simulated-network) (100 Mbit/s each way, 10 ms), with the [Tor-like delays](network-conditions.md#messages-over-tor) on messages that `osp-meet` uses. Nodes poll the nodes they follow every minute, as real ones do, and are told at once of new posts.

- **A creator** posts a photo, and **40 followers** fetch it.
- **4 of them reshare it,** hosting it. Each has **8 followers of its own** who don't follow the creator. Of those, **2 reshare it again**, hosting it, each to **4 followers** who follow only them. So the post reaches some nodes through two reshares.
- **4 watchers** follow the creator and the first resharer.
- Once every node holds the photo, **the second resharer stops following the creator,** so it, and the 16 nodes below it, hear only when it asks about its reshares, as it does once a day.
- **11 nodes go offline**, cut off from the network: the first resharer, and one node in ten of the rest. Then the creator deletes the post.
- **At 5 minutes** the offline nodes come back. The first resharer's outbox still has its reshare until it hears, and the watchers, who have dropped the post, read that stale reshare.
- **At 8 minutes** the second resharer asks about its reshares: the daily check, brought forward.

**A node has dropped it** when it holds neither the post, in its feed or as a reshare, nor the photo. The harness looks every 5 s.

**Pass marks** (ADR 0019's, as starting values):

1. 99% of the online nodes that can hear at once drop it within 15 minutes.
2. Every other node drops it in the end: those offline once they're back, and those below the resharer without a follow once it asks.
3. No node that dropped it shows it again.

Not simulated: Tor itself, and the first contacts that [take minutes](tor.md#a-first-contact-can-take-minutes); a node offline for days; a resharer whose author's node is gone; nodes running other software.

### Running it

```sh
cargo build --release -p osp-sim --bin osp-delete
ulimit -n 524288   # many nodes in one process need many open files
target/release/osp-delete --dir /path/on/disk/delete --out delete.json \
  --label "$(git rev-parse --short HEAD)" > delete.log 2>&1
```

The run above is the defaults. `--followers`, `--resharers`, `--per-resharer`, `--second-resharers`, `--per-second`, `--watchers`, `--offline-percent`, `--back-after-mins`, `--daily-check-after-mins`, `--minutes`, `--no-tor-delay` and `--seed` change them. `--dir` must be empty, and is deleted afterwards. It exits with an error if a mark is missed.

## Results

All three marks were met. Seconds until a node dropped the post, counted from the delete, or from coming back for nodes that were offline:

| Nodes | Online | Median | Slowest | Offline, once back | Slowest |
| --- | ---: | ---: | ---: | ---: | ---: |
| Followers | 33 | 10 | 25 | 3 | 45 |
| Resharers | 2 | 15 | 15 | 1 | 45 |
| Resharers' followers | 11 | 20 | 25 | 1 | 10 |
| Second resharers | 4 | 20 | 20 | | |
| Their followers | 14 | 25 | 30 | 2 | 105 |
| Watchers of a stale reshare | 3 | 5 | 10 | 1 | 45 |
| Below the resharer without a follow | 15 | 492 | 512 | 2 | 192 |
| Below the offline resharer | 15 | 355 | 370 | 1 | 55 |

- **Nodes online heard within 30 s,** through two reshares: the mark is 15 minutes. Each was told at once and synced; a forward costs a resharer one more sync.
- **Nodes that were offline** dropped it within 105 s of coming back, at their next poll or the next poll of the resharer they follow.
- **Below the resharer without a follow,** nodes dropped it 12 to 32 s after it asked (at 480 s). On a real node that's up to a day after the delete.
- **Below the offline resharer,** nodes dropped it within 70 s of it coming back (at 300 s).
- **No node showed it again**, the watchers included, who read the offline resharer's stale reshare.

The log's only warnings were syncs failing to reach nodes while they were offline. Before the delete, every node took 549 s to hold the photo, as the harness set the nodes up one group after another.
