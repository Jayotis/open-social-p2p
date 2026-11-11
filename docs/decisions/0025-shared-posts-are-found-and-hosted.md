# ADR 0025: Shared posts are found, and hosted for whoever finds them

**Status:** Accepted (2026-11-09), after the maintainer's review and the revisions below. Step 1 built (2026-11-09), the minimum first ("Building it"). A hosting reshare past a post's 16 is left out of the listing, and logged. Search shows a found reshare as its post, not yet with who reshared it. Step 2 built (2026-11-09): resharers tell the author's node, which checks a piece, names those who may be listed in `WhoHosts` and on its media endpoint (`osp/media-hosts/0`), and answers there for a deleted post with its `Delete`. As built: only an author's node with other endpoints names resharers, by their media endpoints; a resharer's latest passed check counts as its recent verification, and its last check passing as being heard from lately; the media endpoint's request has one limit for everyone at once, not yet per viewer across keys ([ADR 0024](0024-watching-found-posts-without-your-identity.md)'s step 3); the place for a recently verified host is `swarm.recent_host_place`, off by default since step 4. Step 3 run on real nodes (2026-11-10, [results](../testing/found-and-hosted.md)): Kim online and offline met their marks; the deleted post met them for a node that opens it, and a directory that heard of the `Delete` through its follow now drops it from its lists at once. Step 4 run in `osp-sim` (2026-11-11, [results](../testing/found-and-hosted.md#step-4-results)): B8 held with 200 newcomers, with and without the place; the place didn't bring sharers' first uploads sooner, so it is off by default. Newcomers got 98% of the video from earlier newcomers, almost all from the first sharer the directory listed, as a viewer still downloads from one source at a time. Builds on [ADR 0024](0024-watching-found-posts-without-your-identity.md). Adds to [ADR 0015](0015-nodes-meet-first.md)'s directories and [doc 13](../13-hosting-roles-and-follower-swarm.md)'s resharers' swarms. It supersedes [ADR 0020](0020-sharing-media-you-open.md), which was accepted but never built (below).

Revised 2026-11-09, after the maintainer's review:

- **The follower swarm stays.** ADR 0012's swarm is unchanged. Share is what makes a copy of a post found outside the follows public. The first draft said it was the only way to host another's post.
- **ADR 0020 is superseded.** Viewing never starts sharing.
- **A directory entry is enough to reach the resharer.** It carries what Alice needs to reach Jimmy's copy without asking Kim.
- **Deletes keep their force.** ADR 0019's deletes and tombstones hold against stale directories.
- **A passed piece check is evidence, not proof.** Real download outcomes and rechecks stop recommending a source that fails.
- **A host's place can't be bought with fresh names.** Rotation and a cap on new identities stop one operator taking all 16 places by presenting freshly checked identities.
- **The newcomer place gets tested.** The one place for a recently verified host is measured with and without before it's kept.

## Context

Jimmy joins with no follows, finds Kim's video, watches it ([ADR 0024](0024-watching-found-posts-without-your-identity.md)), and presses **Share**, choosing to host a copy. A week later Alice joins, finds it, and should be able to get it from Jimmy. Today she can't:

- **Directories list posts only.** Jimmy's reshare isn't in any listing, so Alice can't find it.
- **Kim's node names copies only to her followers.** Alice doesn't follow her, so asking Kim names only Kim.
- **Kim's node is told of copies only by followers that share their prefetches.** Jimmy doesn't follow Kim, so it never hears of his.

So every newcomer who finds Kim's video costs Kim's upload, and a newcomer who shared it serves nobody.

What already exists:

- A reshare is a signed `Announce` naming the post and its author. With `hosting`, the resharer promises to keep and serve a copy, a `replica`, served to anyone who asks.
- Followers who said yes to [ADR 0012](0012-sharing-prefetched-copies.md) share the copies they prefetch, and Kim's node names them to her other followers.
- What a node opens from outside its follows stays `cache`, served only to its own devices.

## Decision

### Who hosts what

| Act | What's served, and to whom | Who learns of it |
| --- | --- | --- |
| Following | nothing more | the author's node |
| Following, with ADR 0012's yes | prefetched copies, to the author's other followers, as now | the author's node and its followers |
| Watching | nothing ([ADR 0024](0024-watching-found-posts-without-your-identity.md)) | the relay only |
| Caching | the node's own devices | nobody |
| A plain reshare | nothing: it advertises no source, and is never named as a host | anyone: a signed, attributed `Announce` |
| A hosting reshare | the verified copy, to anyone | anyone: the `Announce` with `hosting`, and its source |

- **The follower swarm doesn't change.** Prefetched copies are still shared with the author's followers under ADR 0012's rules, and named only to them.
- **For a post found outside the follows,** deliberately pressing Share and choosing to host is what makes a node's copy publicly discoverable. Watching, caching and following never publish anything.
- **Sharing is under the person, not the machine.** A reshare is signed by Jimmy's identity and shows his profile, to anyone who sees it. The machine name ([ADR 0015](0015-nodes-meet-first.md)) hides none of it. What stays hidden is his IP address, as his media endpoint is relay-only, and what he watched without sharing.
- **Said at the moment of sharing.** When Jimmy chooses to host, the app says plainly, before it's sent: anyone can see that you shared this and that your node serves it; your address stays hidden; others keep what they've seen even if you delete it later.
- **Hosting without a name** is a separate proposal, ADR 0026, to come: a machine hosting a verified copy under its machine name, with no reshare. It isn't part of this record.

### ADR 0020 is superseded

ADR 0020 would have a node, after one general yes at setup, serve what its owner opened to peers at layer 2. Under this record, **viewing alone never starts sharing, even with a prior general yes:** a deliberate Share controls public hosting of discovered posts. So:

- `share_opened` and its setup question aren't built.
- Neither are ADR 0020's lists of opened posts.

ADR 0020 is superseded by this record. Its daily confirmation with the author is kept in spirit, as ADR 0019's daily check for reshares without a follow (below).

### Directories list reshares

- **Whose:** only people who agreed to be listed (`discoverable`). A reshare from anyone else isn't listed, wherever it's seen.
- **How many:** a listing holds the latest 5 posts and the latest 5 reshares (starting values).
- **A hosting reshare's entry has everything Alice needs, signed, to reach Jimmy's copy without asking Kim:**
  - Jimmy's signed profile, with his media endpoint's binding ([ADR 0024](0024-watching-found-posts-without-your-identity.md)), which says where his current relay-only media endpoint is.
  - His signed `Announce` with `hosting`.
  - Kim's signed post, with her key certificate ([ADR 0010](0010-signing-keys-and-revocation.md)) to check it.

  Alice's node checks each signature, and uses Jimmy's binding only from the newest profile it holds.
- **Search** shows a reshare as the post, with who reshared it. A post reshared many times is still one result.

### Checking a host, and stopping when it fails

- **A piece check is evidence of limited availability, not proof of a complete, reliable copy.** Before marking a reshare as hosting, the directory fetches a random 1 KiB piece of the post's media from the resharer's media endpoint, as an author's node checks a follower's copy today. A reshare whose check fails is listed as plain.
- **Rechecks:** the directory checks each listed hosting reshare's copy again once a day, at one of its hourly reads of the resharer. After 2 failed checks in a row, it lists it as plain until a check passes.
- **Real outcomes, at the downloader:** Alice's node keeps each source's download outcomes in its private record ([ADR 0011](0011-measuring-peers.md), [ADR 0024](0024-watching-found-posts-without-your-identity.md)). A source that failed 3 downloads in a row, refused them as not hosting, or sent bad data is skipped for 24 hours (starting values).
- **Nobody reports outcomes to anyone.** Directories and the author's node can't see others' downloads, as public receipts were removed by design. They stop recommending a source only on their own rechecks.

### Rotation, and no one dominating

A directory lists at most 16 hosting reshares of one post. Which 16:

- **Who qualifies:** a hosting reshare whose latest check passed, from a person listed for at least 7 days (a starting value). A name made today can't qualify today.
- **Rotation:** when more than 16 qualify, the directory picks the day's 16 by a draw from the post's ID and the date. The pick holds for the day and changes the next. How recently a host was checked doesn't move it up, so presenting freshly checked names gains nothing.
- **A cap on new names:** of the 16, at most 8 go to people listed for less than 30 days. An operator wanting every place must keep many listed identities for a month, each hosting a real copy that passes rechecks, within ADR 0015's 100 new listings an hour.
- **Readers merge,** so no single directory decides: Alice's node takes at most 16 hosting sources for a post from all the directories it reads, by the same kind of daily draw.

### The author's node names hosting resharers

- **Telling Kim.** After Jimmy's node has the whole copy, each chunk checked, it tells Kim's node over Tor, sending its signed `Announce`. The share is public, so this uses Jimmy's own identity. Kim's node accepts it from a resharer as it does from a follower now (`Hosting`), checks a piece, and notes Jimmy as a hosting resharer, apart from its followers' swarm. It checks each again once a day, and stops naming one after 2 failed checks in a row, or once its reshare is deleted.
- **Naming them.** Kim's node names hosting resharers to anyone who asks, by their media endpoints, picked by the same daily draw when there are more than it names. They told the world they host it, so naming them reveals nothing they hadn't published.
- **Only discoverable resharers are named to strangers.** `discoverable` governs both directory listing and being named by the author. A resharer whose newest profile says no hosts for its own followers only, through its resharers' swarm, as today.
- **Two ways to ask, one list.** Over Tor, in `WhoHosts`, as followers ask now. And on Kim's media endpoint, through the relay, with a small request of its own that answers only this list. That second way is how a viewing endpoint asks ([ADR 0024](0024-watching-found-posts-without-your-identity.md)).
- **Neither way names a follower's copy to a non-follower.** The private follower-swarm list stays only for her followers.
- **If the post is deleted,** that request answers with Kim's signed `Delete` instead, so a viewer learns it without showing who it is.

### Deletes and stale directories

[ADR 0019](0019-taking-posts-and-media-down.md)'s rules hold, and directory data can never bring back a post known to be deleted.

- **Jimmy's node** acts on Kim's `Delete` as any resharer does: from a forwarded `ForwardDelete`, or from its daily `GetObjects` check with Kim's node for reshares without a follow. It stops serving the copy at once.
- **A directory keeps a tombstone** for every deleted post it learns of, from the author's outbox or a resharer's, for good, as nodes do. It drops every reshare of that post, and refuses to list one again from anyone.
- **Alice's node** refuses any listing or source naming a post it holds a tombstone for, from any directory, however old the data. It drops a found post as soon as any directory, source or Kim's media endpoint shows its `Delete`.
- **A stale directory that hasn't heard yet** can still list a deleted post to a node that never saw the `Delete`. That node finds Jimmy no longer serving it, and Kim's media endpoint answering with the `Delete`, which it then keeps.
- **What stays open, as for every reshare today:** if Kim deletes while Jimmy's node can't reach hers and no `ForwardDelete` reaches him, Jimmy serves the copy until he hears. That's within a day once Kim's node is back.

### When Kim is offline

- Jimmy's copy stays findable and fetchable through the directories, which check Jimmy, not Kim.
- Alice's node uses the sources its listings name, and goes on without Kim's.
- Jimmy's `Hosting` message to Kim waits until her node is back.

### A place for recently verified hosts

- **At most 1 of 6:** of the 6 hosts one `WhoHosts` reply names, at most 1 goes to a recently verified, under-used host. That's one whose copy passed its check within the last 7 days, and that this node has named least often (starting values).
- **Only eligible, available hosts:** a follower that shares, named only to followers, or a hosting resharer. It must also have been heard from lately.
- **No empty place:** if none qualifies, no place is kept, and the 6 are named as today.
- **Unchanged:** trusted followers' places ([ADR 0013](0013-trusted-followers.md)), and the rule that non-followers never get the follower-swarm list.
- **A setting until it's proven.** It's on during the alpha, and doesn't become permanent until B8 and newcomer participation have been compared with and without it (below). The downloader still takes the first 3 at once, and a faster host takes over a slower one's part. That limits the harm of a slow new host; it doesn't remove it.

## Consequences

**Privacy.** A node that shares with hosting is publicly a host of that post, by its media endpoint. That's relay-only, so no IP address is shown. Its direct endpoint is named only as its own rule allows, as today. Nothing about what anyone watched is listed. Followers who share prefetches stay known only to the author's followers.

**For newcomers.** Jimmy serves Alice from the day he shares, with no standing first. What he delivers is measured at Alice's end, as any source's is. Standing and direct-IP permission stay separate: serving well gets a node chosen more, never an address. [ADR 0018](0018-earned-direct-connections.md)'s automatic earned links stay unbuilt, and `nobody` stays the default. [ADR 0017](0017-one-hop-forwarding.md)'s forwarding rules don't change.

**Cost.**

- **For Kim:** a 1 KiB check per hosting resharer a day, and fewer copies sent.
- **For a directory:** a 1 KiB check per hosting reshare when listed and once a day after, and up to about 2 KB per reshare listed (the post, the `Announce`, the key certificate).
- **For Jimmy:** the upload he offered by hosting, within his limits.

**Abuse.**

- **False hosting claims** are caught at a check, or at the downloader as not hosting, and stop being recommended.
- **Bad data** is caught chunk by chunk, as from any source.
- **A crowd of names** gets at most 16 places per post per directory, at most 8 of them for names under a month old, by a daily draw. In `WhoHosts` it gets one place in 6.

## Building it

**The minimum first.** Steps 1 to 3 build the smallest working version. The rotation, the 7 days' listing before qualifying, and the cap on new names wait, as rules already decided, until a directory first holds more than 16 qualifying hosting reshares of one post, which it logs. Until then every qualifying one is listed.

| Step | Work | Gate |
| --- | --- | --- |
| 1 | Directories list reshares, with the signed entry, checks and rechecks, tombstones (rotation and the cap on new names later, above) | Tests: a non-discoverable resharer isn't listed; a plain reshare is never marked hosting; a failed check lists it as plain, and 2 failed rechecks take the mark off; the entry alone lets a node check Jimmy's binding and Kim's post; a 17th hosting reshare isn't listed, and is logged; a tombstoned post's reshares are dropped and refused, from a stale directory too |
| 2 | `Hosting` from resharers; `WhoHosts` and the media endpoint's request name them; the one place for a recently verified host | Tests: a non-follower, over Tor or through the media endpoint, is named discoverable hosting resharers and never a follower's copy, nor a resharer that isn't discoverable; a deleted post's request answers with its `Delete`; the place is empty when no host qualifies; trusted places unchanged; a resharer failing 2 daily checks stops being named |
| 3 | Real nodes, the integration test. Kim publishes. Jimmy, with no follows, finds her video, watches it ([ADR 0024](0024-watching-found-posts-without-your-identity.md)), and shares it with hosting. Alice joins later, finds Jimmy's reshare in a directory, and downloads from Jimmy through the relay. Three versions: **Kim online**; **Kim offline** throughout Alice's part; **the post deleted** before Alice looks | Online and offline: Alice gets verified media, at least partly from Jimmy, offline wholly from him; neither Kim's node nor Jimmy's records Alice's public ID or media endpoint; Alice follows nobody. Deleted: Alice's node doesn't show it once it has seen the `Delete`, and no source serves it; a directory still listing it from before is refused once her node holds the tombstone |
| 4 | `osp-sim`: B8, with a few hundred newcomers arriving over hours who find rather than follow, some sharing with hosting, run with and without the recently verified place | B8's marks unchanged and met in both. Recorded: newcomers' first uploads, the share of later newcomers' bytes from earlier ones, Kim's copies. The place is kept as a policy only if B8 holds and newcomers' first uploads come sooner with it |
| 5 | Whether it's worth having: `osp-month`, several creators of uneven popularity and newcomers arriving over weeks with no follows, finding posts by popularity and sharing with hosting at 5, 10 and 20% (inputs, as real rates are unknown); marks fixed before it runs. And a scripted alpha exercise: testers find and share one video in a set order, hours apart, and report from a local count in `osp status` of bytes served from what they shared, which leaves the node only if they choose | Marks set before the run, for example: at 10% sharing, at least half of a post's viewers' bytes come from resharers once it has 50 viewers, and the creator's upload per viewer falls as viewers grow. Steps 4 and 5 show the value; step 3 shows only the mechanism |

**Revisit** if checks pass for many copies that then fail downloads, if one resharer's copies serve most of a post's strangers, or if the recently verified place slows B8.
