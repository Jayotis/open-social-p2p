# ADR 0019: Taking posts and media down

**Status:** Accepted (2026-11-06), with an undo before a delete is sent ("Details"). Step 1 built (2026-11-06): `Delete` and `ForwardDelete` on `osp/social/2`, tombstones, the daily check of reshares without a follow, `osp delete` and the app's Delete with a minute's undo, with tests of each rule. Its gate was met in `osp-sim` the same day: online nodes dropped a post within 30 s through two reshares, those offline within 105 s of coming back, and no stale copy brought it back ([results](../testing/deleting.md)). Carries out [ADR 0004](0004-signed-objects-and-ids.md)'s plan for deletions, and answers part of [doc 05](../05-security-privacy-and-governance.md)'s question: "What does a valid takedown request mean for independent home hosts?" [Doc 03](../03-peer-distribution-and-scoring.md) calls takedown review and copyright reporting "required, not optional".

Revised 2026-11-05: a [review](../reviews/2026-11-04-review-of-adrs-0016-to-0019.md) found that the first draft's way of passing a delete on through reshares would be refused by sync, and that tombstones which expire let deleted posts come back.

## Context

A person needs to be able to remove what they posted, and an owner needs to decide what their own node holds. Neither is possible today.

- **A person can't delete a post.** `Payload` has no `Delete`. The only thing that removes objects from other nodes is a revoked key ([ADR 0010](0010-signing-keys-and-revocation.md)).
- **An owner can't refuse a file.** A follower's node fetches and shares copies of what a followed person posts, as its owner agreed at setup ([ADR 0012](0012-sharing-prefetched-copies.md)), with no way to drop one and keep it dropped.
- **There's no way to reach the poster.** Someone with a concern about a post, including someone who says it's theirs, can see which account posted it, but a profile has no contact field.

Questions to settle:

1. How can anyone see who posted a file?
2. How does a person remove what they posted?
3. How far does that reach?
4. How does an owner decide what their own node holds?
5. What do directories and relays do?

## Options considered

- **Author details or a watermark inside the file.** [ADR 0005](0005-media-profile-v1.md) strips metadata. A mark would give each poster's copy a different hash, and adds nothing to the signed post. Rejected.
- **A lookup from a file's hash to the posts naming it.** Cheap, and it uses what's already signed. Chosen.
- **Only the poster can act.** Simple, but an owner would have no say over what their own node holds. Not enough on its own.
- **A resharer putting the author's `Delete` in its own outbox, unchanged.** Sync skips any outbox entry not signed by the account being followed, and that check protects every outbox. Rejected.
- **A forward signed by the resharer, carrying the author's `Delete`.** The resharer's signature shows where it came from, and the author's gives the right to delete. Chosen.
- **A separate feed of deletes.** It would get round the daily limit on new objects, but adds a request and a position to keep for every follow. Not needed while the outbox works.
- **A network-wide authority.** There's nobody to run it without becoming the central service this project avoids, and [ADR 0015](0015-nodes-meet-first.md) rules out network-wide bans. Rejected.
- **Each node choosing what it carries, with optional shared lists.** Chosen, with the lists left open below. Some IPFS gateways filter by [shared lists of hashes](https://specs.ipfs.tech/compact-denylist-format/), which hold a hash of each hash, so a list doesn't double as a catalogue.
- **Matching video by fingerprint.** Needs a reference database, and false matches hurt creators. Not now.

## Decision

### Finding who posted

- **Every post is signed and names its media's hash,** so a post leads to an identity and its profile.
- **A new `osp who-posted <hash>`** looks in the node's own posts and in the directory listings it has copied. Listings hold the latest 5 posts of people who opted in, so it won't find everything.
- **A profile gets an optional `contact` field,** so people can reach the poster.

**What this shows:**

- **An account.** Its name is whatever its owner chose.
- **Who posted it.** A signature says nothing about who owns what was posted, and two accounts can post one file. Dates are whatever the signer wrote.
- **That exact file.** Another encoding has another hash.

### Removing a post

A new signed object, `Delete { actor, object, published }`, as ADR 0004 planned. It names one post or reshare by its ID, not a file by its hash. Only the identity that signed that post or reshare can delete it.

- Deleting a post removes it, and every reshare of it, on each node that hears.
- Deleting a reshare removes only that reshare, and the resharer's promise to host.
- Neither touches another account's post naming the same file.

**How it travels:**

- **In the author's outbox,** with followers told at once as for a new post. A follower who missed it gets it at its next sync, which reads on from where it stopped, however many posts came after.
- **Through reshares, as a forward.** A node that reshared the post signs a `ForwardDelete { actor, delete, published }` in its own outbox, carrying the author's `Delete` unchanged, as a reshare carries its post. That's one carried object per entry, within today's limit of two. A node that receives it:
  1. checks the forward like any entry in that outbox: signed by the resharer it follows;
  2. checks the `Delete` inside on its own: signed by the post's author, with a key that hasn't been revoked. A resharer's signature never gives the right to delete;
  3. removes the post only if it holds it, and the post's signer is the `Delete`'s;
  4. forwards it once if it reshared the post too, and never forwards a forward, so loops end.
- **A node that reshared a post whose author it doesn't follow** asks the author's node, named in the profile its reshare carries, about it once a day with [`GetObjects`](0010-signing-keys-and-revocation.md), up to 50 at a time, which now answers a deleted post's ID with its `Delete`. That covers a resharer who unfollowed the author, and one restored from a backup, which keeps what its reshares point at ([ADR 0009](0009-identity-backup-and-recovery.md)).
- **A directory** drops the post when it next reads that person, about hourly. A reader replaces a person's found posts with each new listing of them, so the post leaves its search when it next reads the directory.

**What a node does on receiving it:**

- Removes the post, and reshares of it, from its objects, feed, search and found posts.
- **Stops serving the media at once,** cancels any fetch of it, and drops it and its thumbnail from every class, including the private cache and the author's own copy, unless another post it keeps names the same file. A reshare of the deleted post doesn't count as another post.
- Stops naming hosts for it in `WhoHosts`, unless another post still names the file.
- **Keeps a tombstone for good:** the post's ID, its author and the signed `Delete`. Signed posts don't expire, so a tombstone that did would let an old copy come back. A node keeps one for every `Delete` in an outbox it syncs, even for a post it never had, so a stale copy arriving later is refused. From a forward, it keeps one only for a post it held.

**Details:**

- **A thief's deletes.** A `Delete` signed by a key later revoked goes with everything else that key signed, tombstone included. A follower then adds the post it named to what it asks the author's node for again, as ADR 0010 does for revoked posts, and gets it back if the owner's node still has it. Elsewhere it stays gone, as deleting removed it and its media. When the owner replaces a key, the node signs its outbox again as it does now, its own `Delete`s included. A thief's deletes never reach that outbox unless the thief had the node itself, and then the owner restores from a backup, as ADR 0010 says.
- **`Delete` doesn't count against the daily limit** of 500 new objects from one account, so someone can remove their history. Sync still stops at that limit, so behind a flood of posts, a `Delete` waits for the next day.
- **The author's own backup.** Restoring one made before a delete brings the post back on that node, until it's deleted again. Nodes holding the tombstone keep refusing it.
- **What it can't reach.** A node that missed a delete can still take an old copy from somewhere that missed it too, such as a node running changed software. A signature shows who wrote a post, not that it's still up.
- **A new protocol name,** `osp/social/2`, as ADR 0010 did. Older nodes can't read the new objects, so a node on `osp/social/1` can't sync with one on `/2`, and can't quietly keep serving a deleted post to it; it has to update. The app's delete button needs the updated node too.
- The app says plainly what it does, as doc 05 asks: copies on nodes running the standard software go as they hear of it, and copies people saved don't.
- **An undo first.** Once sent, a `Delete` can't be taken back, and tombstones refuse the post for good. So the app waits about a minute before sending it, with an Undo button (decided 2026-11-06). Leaving the app sends it at once. The phone keeps it until the node has it, so one the phone stops on its way goes when the app is back, for up to a day.

### How far it reaches

Expected, until step 1 measures it:

| Copy | What happens |
| --- | --- |
| A node running the standard software, online | Gone within minutes; through a reshare or a directory, up to about an hour |
| Such a node, offline | Gone at its next sync |
| A node that reshared it without following its author | Gone within a day |
| The author's node, restored from a backup made before the delete | Back on that node until deleted again; others keep refusing it |
| A node on the older protocol | Can't sync with updated nodes until it updates |
| A node running changed software | Up to its owner |
| A file someone saved | Stays |
| The same file in another account's post | Stays, until that post is deleted or a node blocks the file |

### What each owner decides

1. **Each owner decides what their node carries.** A new `osp block <post, file or account>` drops it and refuses to fetch or serve it. It sends no message, and can be undone.
2. **Anyone can publish a list of files,** and a node can subscribe to any it chooses. A list is signed by its publisher, each entry has a date and can be retracted, and it uses a hash of each hash. A node acting on a list only stops fetching and serving; the post stays, with its media unavailable. Nothing forces this.
3. **Who publishes the first list, and whether setup recommends one, is open.**

### Directories and relays

These are optional roles anyone can take on. Whoever runs one sets its policy and publishes a contact.

- **A directory** holds the profiles and latest posts of people who asked to be listed, and no media. It can drop a listing or one post.
- **A relay** carries encrypted bytes and can't see files. It can refuse an endpoint.

The first directory, run by the maintainer so early testers can find each other, is a temporary stand-in ([ADR 0014](0014-finding-people-and-private-messages.md)). It's set by hand in `config.toml` and isn't built into the software ([ADR 0015](0015-nodes-meet-first.md)).

### The project's position

**OSP is open-source peer-to-peer software for people to communicate.** It's a personal social platform, under nobody's control but each person who runs a node on their own equipment and internet connection.

- **Each person decides** what they post and what their node holds and sends, and answers for it.
- **Post only what you have the right to post.**
- **There's no company, no account with the project and no central service.** The software reports nothing to the project.
- **The maintainer can't change or remove anything on anyone else's node.** The tools for that belong to the people concerned: the poster deletes, and each owner blocks.

This goes in the README and at setup. Nothing in these documents is legal advice, and none may claim legal protection for anyone ([doc 13](../13-hosting-roles-and-follower-swarm.md)).

## Consequences

**People.** Only the poster can delete a post across the network. Each owner decides for their own node.

**Privacy.** A `Delete` shows that a post existed, to nodes that never held it. A list of files shows what its publisher blocks. A hash of each hash keeps it from being a catalogue, but anyone holding a file can still check whether it's on the list. `contact` is optional and public.

**Security.** A forged `Delete` is refused, as any bad signature is. Replaying one does nothing more. A resharer can't delete someone else's post: a forward carries the author's signature or does nothing. A thief with a signing key could delete posts: see above.

**Operational.** Tombstones are small, and kept for good, one for each deleted post a node heard of. A reshare without a follow costs one request a day to its author's node. Hosting by follows now also means dropping files when told.

**Changes to accepted records.** None. This carries out ADR 0004 and doc 13. It adds two object types under a new protocol name, and `GetObjects` answers a deleted post with its `Delete`, both in `osp/social` (ADR 0010).

**Licensing.** No new dependency.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | `Delete`, `ForwardDelete`, tombstones, the daily check for reshares without a follow, the app and CLI | In `osp-sim`: 99% of online nodes stop serving within 15 minutes and all after their next sync (starting marks), through two reshares; a resharer without a follow within a day; a stale reshare doesn't bring it back; a forged one, or one by someone other than the author, is refused; a revocation brings a thief-deleted post back to followers where the owner's node still has it |
| 2 | `osp block` | A blocked file isn't fetched or served, and no message leaves the node |
| 3 | `osp who-posted`, `contact`, a directory dropping one post, its published policy | A file found in a listing leads to its post and contact |
| 4 | Lists, once someone will run one | Subscribing and retracting work; a list shows no plain hash |

**Step 1 belongs before the invited alpha:** people need to be able to delete a post. The rest comes before a public release.

## Validation

Step 1's run measures how far a delete reaches, over reshares, with nodes offline. Tests cover a reshare loop, a forward for a post the node doesn't hold, the same file in two accounts' posts, a fetch under way when the delete arrives, and a restart. A test checks a blocked file stays blocked after a restart and after a restore. Setup and publishing wording is checked against what actually happens.

**Revisit** if deletes don't reach nodes as expected, if owners want more say over what their node holds, or if people use `Delete` to rewrite what they said in ways that harm others.
