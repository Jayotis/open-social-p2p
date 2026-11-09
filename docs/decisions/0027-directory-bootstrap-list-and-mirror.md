# ADR 0027: A signed list of directories, and a mirror of their listings

**Status:** Proposed (2026-11-09), for review. Adds to [ADR 0015](0015-nodes-meet-first.md)'s directories and [ADR 0014](0014-finding-people-and-private-messages.md)'s step 6.

## Context

The alpha's installer should set a node's directories itself, so nobody picks a server (the maintainer, 2026-11-09). It also needs a fallback, and a test with the first directory off. Today directory IDs are set by hand, and the node itself has no built-in default, as the first directory is temporary ([ADR 0015](0015-nodes-meet-first.md)).

Two needs:

1. **Which directories to use,** changeable without a new release.
2. **Listings that stay readable when the directories are off.** The first and its fallback are both the maintainer's, and the fallback isn't always on.

The maintainer has a domain and a Cloudflare account. A directory itself can't run there: it reads and answers over Tor and checks copies over iroh's relays, and Cloudflare's Workers serve only HTTP. But it can serve signed files.

## Decision

### The signed list of directories

- **A small file** at a fixed path on the maintainer's domain. It names each directory by public ID and machine name, with its role (primary or fallback), a version number, when it was issued, and when it expires (30 days after, a starting value).
- **Signed by a list key** held offline by the maintainer, not by the node or the directory. Its public half is in the installer script and written to the node's configuration, never built into the `osp` binary, as the binary keeps no default (ADR 0015).
- **The node fetches it over Tor,** as it fetches a domain's file for verification ([ADR 0014](0014-finding-people-and-private-messages.md)). The server sees a Tor exit, not the node. It's fetched once a day, and at install.
- **It accepts a list only if** the signature checks against a key it trusts, the version is higher than any it has accepted, and it hasn't expired. A lower version is refused, so an old list can't be served again in place of a newer one.
- **Stale lists:** if no newer list can be fetched, the node keeps using the last list it accepted, past its expiry. `osp status` says the list is out of date, so a node never ends up with no directories because a server was down. An expired list is never accepted as new.
- **Adding to the node's own directories:** directories set by hand in `[[directory.known]]` stay, alongside the list. The owner can turn the list off.

### Key rotation

- **Planned:** a list may name the next key. A node accepts a list signed by that next key once a list signed by its current key has named it, and then trusts only the new one. The old key's last list names the new key at least one expiry period before it stops being used.
- **Lost or stolen key:** no list can revoke it, as whoever holds it can sign lists too. Recovery is a new installer with a new key, and testers told directly. That's stated in setup's notes for the alpha. Two keys, both needed to sign, is the later fix.
- **What a stolen key can do:** name directories of the thief's own. Those can omit and go stale, but can't forge anyone's profile or posts, which their authors sign ([ADR 0015](0015-nodes-meet-first.md)). And nodes still read the directories set by hand.

### A mirror of each directory's listing

- **A directory can publish its listing as a file,** a snapshot of all its entries, signed by the directory's own key, with a sequence number and a time. Each entry inside is still its author's signed object, as now.
- **Served from Cloudflare,** by the maintainer, at a path of the domain the list names for that directory.
- **A node reads a mirror only when it can't reach the directory itself,** over Tor, and checks the snapshot's signature, then every entry's, as from the directory.
- **What the mirror can't do:** change or add an entry, as the authors' and the directory's signatures cover them; or serve an older snapshot than one the node has read, as the sequence number is checked.
- **What it can do:** withhold the file, or serve the newest it has, however old. A snapshot older than 7 days (a starting value) is used for search, marked as old in results, and never brings back a post or person the node holds a tombstone or withdrawal for ([ADR 0019](0019-taking-posts-and-media-down.md), [ADR 0025](0025-shared-posts-are-found-and-hosted.md)).
- **The map of machines isn't mirrored.** Meeting stays between machines.

## Consequences

**Privacy.** The domain's server and Cloudflare see Tor exits fetching public files, and when. They see no node's identity or address. The files hold only what directories already give anyone who asks.

**Independence.** This gives listings that stay readable when both directories are off. It doesn't give an independent operator: the list key, both directories and the mirror are all the maintainer's. A directory someone else runs is still the real test of ADR 0014's step 6.

**Cost.** A few kilobytes a day per node for the list. A snapshot is roughly 1 KB per listed person, so about 5 MB for a full directory of 5,000, fetched only when the directory can't be reached.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | The list: format, signing tool, fetch over Tor, checks, stale handling, rotation; the installer writing the key and first list | Tests: a list with a bad signature, a lower version or past its expiry isn't accepted; a node with only an expired list keeps using it and says so; a rotation in two steps is followed; the old key is refused after it |
| 2 | Snapshots and the mirror | Tests: a changed entry or snapshot is refused; an older sequence number is refused; a tombstoned post in a snapshot stays gone |
| 3 | Real nodes, with the first directory and its fallback on two test machines | A fresh install, given only the installer, finds a stranger; with the first off, through the fallback; with both off, through the mirror; with the mirror withholding too, the node says why it found nothing |
