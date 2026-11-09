# ADR 0025: Shared posts are found, and hosted for whoever finds them

**Status:** Proposed (2026-11-09), for review before any code. Builds on [ADR 0024](0024-watching-found-posts-without-your-identity.md). Adds to [ADR 0015](0015-nodes-meet-first.md)'s directories and [doc 13](../13-hosting-roles-and-follower-swarm.md)'s resharers' swarms.

## Context

Jimmy joins with no follows, finds Kim's video, watches it, and presses **Share**, choosing to host a copy. A week later Alice joins, finds the same video, and should be able to get it from Jimmy. Today she can't:

- **Directories list posts only.** Jimmy's reshare isn't in any listing, so Alice can't find it.
- **Kim's node names copies only to her followers.** Alice doesn't follow her, so asking Kim names only Kim.
- **Kim's node is told of copies only by followers that share their prefetches.** Jimmy doesn't follow Kim, so it never hears of his.

So every newcomer who finds Kim's video costs Kim's upload, and a newcomer who shared it serves nobody.

What already exists: a reshare is a signed `Announce` naming the post and its author, with `hosting` when the resharer keeps and serves a copy. A hosted copy is a `replica`, served to anyone who asks. Viewing is separate: what a node opens stays `cache`, served only to its own devices ([ADR 0012](0012-sharing-prefetched-copies.md)), and [ADR 0020](0020-sharing-media-you-open.md)'s sharing of opened media stays unbuilt.

## Decision

**Share is the only way a node becomes a public host of someone else's post,** and it's a person's act. Watching, caching and following publish nothing. The five stay apart:

| Act | Who learns of it |
| --- | --- |
| Following | the author's node; what's synced |
| Watching | nobody but the relay ([ADR 0024](0024-watching-found-posts-without-your-identity.md)) |
| Caching | nobody: served to the node's own devices |
| Resharing | anyone: a signed, attributed `Announce` |
| Hosting | anyone: the same `Announce` with `hosting`, and a media source others may use |

### Directories list reshares

- A listed person's latest reshares go in their listing beside their posts, with the post they reshare (signed by its author). A listing holds at most 5 posts and 5 reshares (starting values).
- **Only from people who agreed to be listed** (`discoverable`). A reshare from anyone else isn't listed, wherever it's seen.
- **Hosting reshares advertise a source.** Before marking one as hosting, the directory fetches a random 1 KiB piece of the post's media from the resharer's media endpoint, as an author's node checks a follower's copy today. One that doesn't send it is listed as a plain reshare, and checked again at the next read.
- **Deleting it takes it out.** The directory reads each listed node's outbox hourly, as now. A `Delete` of the reshare drops it, and a `Delete` of the post drops every reshare of it.
- **No one dominates.** A directory lists at most 16 hosting reshares of one post (a starting value), so a crowd of names can't fill a post's sources. When it has more, it lists those verified most recently. Reshares count toward ADR 0015's cap on listings per hour like anything new.
- Search shows a reshare as the post, with who reshared it. A post reshared many times is still one result.

### The author's node names hosting resharers

- After Jimmy's node has the whole copy, each chunk checked, it tells Kim's node over Tor, sending its signed `Announce`. Kim's node accepts that from a resharer as it does from a follower now (`Hosting`), checks a piece of the copy, and notes Jimmy as a hosting resharer, apart from its followers' swarm.
- **Kim's node names hosting resharers to anyone who asks,** by their media endpoints. They told the world they host it, so naming them reveals nothing they hadn't published. It still names its followers' copies only to its followers.
- **Two ways to ask, one list.** Over Tor, in `WhoHosts`, as followers ask now. And on Kim's media endpoint, through the relay, with a small request of its own that answers only this list. That's how a viewing endpoint asks ([ADR 0024](0024-watching-found-posts-without-your-identity.md)), so Alice learns Jimmy's copy without telling Kim who she is. The media endpoint's answer never includes a follower's copy, whoever asks.
- A resharer whose reshare is deleted, or that fails a check, is no longer named.

### A place for recently verified hosts

Of the 6 hosts one `WhoHosts` reply names, **at most 1 goes to a recently verified, under-used host**: one whose copy passed its check within the last 7 days and that this node has named least often (starting values). It must be eligible (a follower that shares, or a hosting resharer) and heard from lately. If none qualifies, no place is kept, and the 6 are named as today. Trusted followers' places ([ADR 0013](0013-trusted-followers.md)) don't change.

The downloader still takes the first 3 at once, and a faster host takes over a slower one's part. That limits the harm of a slow new host. It doesn't remove it, which is what measuring is for.

## Consequences

**Privacy.** A node that shares with hosting is publicly a host of that post, by its media endpoint. That's relay-only, so no IP address is shown. Its direct endpoint is named only as its own rule allows, as today. Nothing about what anyone watched is listed. Followers of Kim who share their prefetches stay known only to her followers.

**For newcomers:** Jimmy serves Alice from the day he shares, with no standing first. What he delivers is measured at Alice's end ([ADR 0011](0011-measuring-peers.md)), as any source's is. Standing and direct-IP permission stay separate: serving well gets a node chosen more, never an address ([ADR 0018](0018-earned-direct-connections.md)'s automatic earned links stay unbuilt).

**Cost.** For Kim, a 1 KiB check per hosting resharer, and fewer copies sent. For a directory, a 1 KiB check per hosting reshare it lists, and up to about 1 KB per reshare listed. For Jimmy, the upload he offered by hosting, within his limits.

**Abuse.** A node claiming to host what it doesn't is caught at the piece check and dropped. A crowd of resharers is held to 16 per post per directory, and to one place in 6 in `WhoHosts`. A resharer serving bad data is caught chunk by chunk, as any source is.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | Directories list reshares: limits, piece check, deletes | Tests: a non-discoverable resharer isn't listed; a hosting claim that fails the check is listed as plain; a `Delete` drops it; a 17th hosting reshare of one post isn't listed |
| 2 | `Hosting` from resharers; `WhoHosts` and the media endpoint's request name them to anyone; the one place for a recently verified host | Tests: a stranger, over Tor or through the media endpoint, is named hosting resharers but never a follower's copy; the place is empty when no host qualifies; trusted places unchanged |
| 3 | Real nodes: Jimmy finds Kim, shares with hosting; Alice joins later, finds it, gets it from Jimmy | Alice's bytes come at least partly from Jimmy, through his media endpoint; Kim's node never learns Alice's public ID ([ADR 0024](0024-watching-found-posts-without-your-identity.md)) |
| 4 | `osp-sim`: B8, with newcomers who find rather than follow, some sharing with hosting | B8's marks unchanged and met. Recorded: newcomers' first uploads, the share of later newcomers' bytes from earlier ones, Kim's copies, with and without the recently verified place |

**Revisit** if the piece checks miss many false claims, if one resharer's copies serve most of a post's strangers, or if the recently verified place slows B8.
