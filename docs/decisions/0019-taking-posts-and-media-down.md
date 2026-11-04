# ADR 0019: Taking posts and media down

**Status:** Proposed (2026-11-04). Not built. Carries out [ADR 0004](0004-signed-objects-and-ids.md)'s plan for deletions, and answers part of [doc 05](../05-security-privacy-and-governance.md)'s question: "What does a valid takedown request mean for independent home hosts?" [Doc 03](../03-peer-distribution-and-scoring.md) calls takedown review and copyright reporting "required, not optional".

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

A new signed object, `Delete { actor, object, published }`, as ADR 0004 planned. Only the identity that signed a post or reshare can delete it.

**How it travels:**

- In the author's outbox, with followers told at once as for a new post. A follower who missed it gets it at its next sync.
- A directory drops the post when it next reads that person, about hourly.
- **A node that reshared the post puts the author's signed `Delete` in its own outbox, unchanged,** so it reaches those who got the post through a reshare.

**What a node does on receiving it:**

- Removes the post from its objects, its index and its found posts.
- **Stops serving the media at once,** and drops it from every class, including the private cache and the author's own copy, unless another post it keeps names the same file.
- Stops naming hosts for it in `WhoHosts`.
- **Keeps a small tombstone** (the object ID and media hashes, for a year, oldest going first past a cap), so a stale reshare can't bring it back.

**Details:**

- A `Delete` signed by a key later revoked goes with everything else that key signed, and [`GetObjects`](0010-signing-keys-and-revocation.md) brings the post back. So a thief's deletions can be undone, where copies remain.
- `Delete` doesn't count against the daily limit on new objects, so someone can remove their history.
- Older nodes can't read a new object type, so this means a new protocol name, as ADR 0010 did.
- The app says plainly what it does, as doc 05 asks: copies on nodes running the standard software go as they hear of it, and copies people saved don't.

### How far it reaches

Expected, until step 1 measures it:

| Copy | What happens |
| --- | --- |
| A node running the standard software, online | Gone within minutes; through a reshare or a directory, up to about an hour |
| Such a node, offline | Gone at its next sync |
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

**Privacy.** A `Delete` shows that a post existed, to nodes that never held it. A list of files shows what its publisher blocks, which a hash of each hash limits. `contact` is optional and public.

**Security.** A forged `Delete` is refused, as any bad signature is. Replaying one does nothing more. A thief with a signing key could delete posts: see above.

**Operational.** Tombstones take a little space on each node. Hosting by follows now also means dropping files when told.

**Changes to accepted records.** None. This carries out ADR 0004 and doc 13, and adds a new object type.

**Licensing.** No new dependency.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | `Delete`, tombstones, passing it on through reshares, the app and CLI | In `osp-sim`: 99% of online nodes stop serving within 15 minutes and all after their next sync (starting marks); a stale reshare doesn't bring it back; a forged one is refused; a revocation restores what a thief deleted |
| 2 | `osp block` | A blocked file isn't fetched or served, and no message leaves the node |
| 3 | `osp who-posted`, `contact`, a directory dropping one post, its published policy | A file found in a listing leads to its post and contact |
| 4 | Lists, once someone will run one | Subscribing and retracting work; a list shows no plain hash |

**Step 1 belongs before the invited alpha:** people need to be able to delete a post. The rest comes before a public release.

## Validation

Step 1's run measures how far a delete reaches, over reshares, with nodes offline. A test checks a blocked file stays blocked after a restart and after a restore. Setup and publishing wording is checked against what actually happens.

**Revisit** if deletes don't reach nodes as expected, if owners want more say over what their node holds, or if people use `Delete` to rewrite what they said in ways that harm others.
