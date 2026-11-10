# ADR 0027: A signed list of directories, and a mirror of their listings

**Status:** Proposed (2026-11-09), approved in principle, revised after the maintainer's review (2026-11-09), for review again. The maintainer approves an offline-signed list of directories delivered by the installer, and directories' signed listings mirrored through Cloudflare. Adds to [ADR 0015](0015-nodes-meet-first.md)'s directories and [ADR 0014](0014-finding-people-and-private-messages.md)'s step 6.

Revised 2026-11-09, after the maintainer's review:

- **The same list again is fine.** Same version, same content: accepted, nothing changes. A lower version, or other content at a version already accepted, is refused.
- **A snapshot is checked against the list.** Its signature must verify against the directory identity that the trusted list names.
- **A first install** has no sequence numbers to compare. How old a snapshot it takes, and what it does about stale profiles and posts deleted since, are stated.
- **Limits, from measured sizes.** A person's listing is about 9 to 21 KB, not 1 KB. Download, parsing, signature and storage limits follow from that.
- **Restore, reinstall, rotation and a stolen key** are each stated, with what can't be recovered without testers acting by hand.

## Context

The alpha's installer should set a node's directories itself, so nobody picks a server (the maintainer, 2026-11-09). It also needs a fallback, and a test with the first directory off. Today directory IDs are set by hand, and the node itself has no built-in default, as the first directory is temporary ([ADR 0015](0015-nodes-meet-first.md)).

Two needs:

1. **Which directories to use,** changeable without a new release.
2. **Listings that stay readable when the directories are off.** The first and its fallback are both the maintainer's, and the fallback isn't always on.

The maintainer has a domain and a Cloudflare account. A directory itself can't run there: it reads and answers over Tor and checks copies over iroh's relays, and Cloudflare's Workers serve only HTTP. But it can serve signed files.

## Decision

### The signed list of directories

- **A small file** at a fixed path on the maintainer's domain. It names each directory by public ID and machine name, with its role (primary or fallback), and the path of its mirror. It has a version number, when it was issued, and when it expires (30 days after, a starting value). At most 16 directories, and at most 64 KiB.
- **Signed by a list key** held offline by the maintainer, not by the node or the directory. Its public half is in the installer script and written to the node's configuration, never built into the `osp` binary, as the binary keeps no default (ADR 0015).
- **The installer carries a list too,** the newest when it was made, so a first start doesn't depend on the domain.
- **The node fetches it over Tor,** as it fetches a domain's file for verification ([ADR 0014](0014-finding-people-and-private-messages.md)). The server sees a Tor exit, not the node. It's fetched once a day, and at install.
- **What it accepts:**

  | The list fetched | Is |
  | --- | --- |
  | Signature doesn't check against the trusted key | refused |
  | Version lower than the highest accepted | refused, so an old list can't be served again in place of a newer one |
  | Same version, byte for byte the same signed content | accepted, and nothing changes: a refresh. It only notes when the list was last confirmed |
  | Same version, other content | refused, and `osp status` warns. Two lists signed at one version means a mistake or a stolen key (below) |
  | Higher version, not expired, issued no more than an hour ahead of the node's clock | accepted |
  | Expired | never accepted as new |

- **Stale lists:** if no newer list can be fetched, the node keeps using the last list it accepted, past its expiry. `osp status` says the list is out of date, so a node never ends up with no directories because a server was down.
- **Adding to the node's own directories:** directories set by hand in `[[directory.known]]` stay, alongside the list. The owner can turn the list off.

### What the node keeps, and restoring

- **The trusted key** is in `config.toml`, as the installer wrote it. **The highest list version accepted,** and each directory's highest snapshot sequence number, are node state, in the database.
- **The backup** ([ADR 0009](0009-identity-backup-and-recovery.md)) includes the trusted key, the highest version and those sequence numbers. A restore takes them back, so restoring can't roll the node back to an older list.
- **A reinstall from a newer installer** brings its own key and list. The node takes the installer's key as trusted: running an installer is choosing whom to trust. If it's the same key as the backup's, the highest version is the higher of the two. If it's another key, the node starts from the installer's list, and keeps no versions from the old key.
- **A reinstall from an older installer** after a restore keeps the restored key and versions, if they're newer, and says so.

### Key rotation

- **Planned:** a list may name the next key. A node accepts a list signed by that next key once a list signed by its current key has named it, and then trusts only the new one. The old key's last list names the new key at least one expiry period (30 days) before it stops being used, so every node that fetches at least once a month follows it. A node off for longer gets the new key from a new installer.
- **New installers carry the new key** from the day it's named, and a list signed by it.

### A stolen list key

- **What it can do:** sign lists as the maintainer. It can name directories of the thief's own, which can omit people and go stale, but can't forge anyone's profile or posts, which their authors sign ([ADR 0015](0015-nodes-meet-first.md)). And nodes still read the directories set by hand.
- **What it can do that's hard to undo:** sign a list with a much higher version. Nodes accept it, and then refuse the maintainer's real lists, which are lower. The hour's limit on how far ahead a list may be issued doesn't limit its version.
- **What recovery takes, by hand:** no list can revoke the key, as whoever holds it can sign lists too. The maintainer makes a new key and a new installer, and tells testers directly, outside OSP, to run it. Until a tester does, their node goes on following the thief's lists. The node can't detect a theft by itself, except when it sees two lists at one version (above). This is stated in the alpha's setup notes.
- **The later fix:** two keys, both needed to sign a list, kept apart.

### A mirror of each directory's listing

- **A directory can publish its listing as a snapshot,** signed by the directory's own key, with a sequence number and a time. Each entry inside is still its author's signed object, as now.
- **Served from Cloudflare,** by the maintainer, at the path the list names for that directory.
- **A node reads a mirror only when it can't reach the directory itself,** over Tor.
- **Whose signature counts.** A snapshot is accepted only if its signature verifies against the directory identity that the node's accepted list names for that mirror path, or a directory set by hand. A snapshot signed by any other key, even another listed directory's, is refused. Then every entry's signatures are checked, as from the directory.
- **Sequence numbers:** once a node has read a snapshot or the directory itself, it refuses a snapshot with a lower sequence number. The same number with the same content is a refresh, as for the list; with other content, it's refused and `osp status` warns.
- **What the mirror can't do:** change or add an entry, as the authors' and the directory's signatures cover them; or serve an older snapshot than one the node has read.
- **What it can do:** withhold the file, or serve the newest it has, however old.
- **The map of machines isn't mirrored.** Meeting stays between machines.

### A first install, and old snapshots

A new node has no sequence numbers, so it can't tell an old snapshot from a new one by itself.

- **It takes the snapshot's own date:** one dated more than 30 days before the node's clock (a starting value) is refused at first install. One dated more than an hour ahead is refused at any time.
- **A snapshot older than 7 days** (a starting value) is used, but its people and posts are marked as from an old copy in search, until the node reads the directory itself or a newer snapshot.
- **Stale profiles:** a profile from a snapshot is kept only if it's newer than any the node holds, as for every profile ([ADR 0009](0009-identity-backup-and-recovery.md)). The node uses it to reach the person, and the person's node gives its newest when asked.
- **Posts deleted since:** a new node holds no tombstone for them, so a stale snapshot can show a post its author has deleted. Opening it goes through a viewing endpoint, which asks the author's media endpoint who hosts it ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)). If the post was deleted, that answers with the author's `Delete`. The node then drops it and keeps the tombstone, and no later snapshot brings it back. With the author's node off too, a resharer that hasn't heard may still serve it, as ADR 0025 says. Those are the post's bytes, as the author signed them.
- **Hosting marks:** a reshare marked hosting in an old snapshot may no longer be. The downloader finds out at its first try, as from any source, and skips it.

### Limits

**Measured sizes.** One person's entry in a directory's listing, with the current code: the profile, with its media endpoint's binding and key certificate, about 1.6 KB; a photo post, with its thumbnail and key certificate, about 1.4 KB; a hosting reshare, carrying the post and its key certificate, about 2.5 KB. A person with 5 photo posts and no reshares: about 8.6 KB. With 5 reshares as well: about 21 KB. A listing of one person is capped at 32 KiB. So 5,000 people are about 43 to 105 MB, and at most 160 MiB. Encoded as now, in JSON, and not compressed: compression is unmeasured.

**The node's own limit binds first.** A node keeps at most 5,000 people and 64 MiB from directories. At these sizes 64 MiB holds about 3,000 to 7,800 people, so it's the bytes, not the count, that fill first.

**So a snapshot comes in pages,** and a node reads only what it can keep:

- **The snapshot file** is a signed index: the directory's identity, the sequence number, the date, and each page's hash and size. At most 64 KiB, so at most about 1,000 pages.
- **Each page** at most 1 MiB and 100 entries, checked against its hash before it's parsed, and parsed one entry at a time. An entry over 32 KiB is skipped.
- **Downloads:** each file over Tor, with 120 s to arrive, read up to its limit and no further. One snapshot read fetches at most 64 MiB, and stops once the node's limits on people found are reached.
- **Signatures:** one for the index; about 20 per person (the profile, posts, reshares and their key certificates). 5,000 people are about 100,000 checks. At the tens of microseconds an Ed25519 check takes, that's seconds of one core: done in the background, a page at a time. An estimate, to measure in step 2.
- **Storage:** pages aren't kept. Entries go into what the node keeps of people found, within its limits, as from a directory. Only the index's sequence number and hash are kept, per directory.

## Consequences

**Privacy.** The domain's server and Cloudflare see Tor exits fetching public files, and when. They see no node's identity or address. The files hold only what directories already give anyone who asks.

**Independence.** This gives listings that stay readable when both directories are off. It doesn't give an independent operator: the list key, both directories and the mirror are all the maintainer's. A directory someone else runs is still the real test of ADR 0014's step 6.

**Cost.** Under 64 KiB a day per node for the list. A snapshot read is up to 64 MiB, and only when a directory can't be reached. For the maintainer, the mirror's storage and transfer: up to about 100 MB a snapshot per directory, and 64 MiB per node that reads one. What that costs on Cloudflare isn't checked yet; it's checked before step 2.

**What a stolen list key costs** is above: testers reinstalling by hand.

## Building it

The list and the installer (step 1) can be built once this design is accepted, apart from ADR 0026. Nothing is built before.

| Step | Work | Gate |
| --- | --- | --- |
| 1 | The list: format, signing tool, fetch over Tor, checks, stale handling, rotation; the installer writing the key and first list; the key, version and sequence numbers in the backup | Tests: a list with a bad signature, a lower version, other content at the same version, or past its expiry isn't accepted; the same list again is accepted and changes nothing; a node with only an expired list keeps using it and says so; a rotation in two steps is followed; the old key is refused after it; a restore keeps the highest version; a reinstall with another key starts from the installer's list |
| 2 | Snapshots in pages, and the mirror | Tests: a snapshot signed by a key other than the listed directory's is refused; a changed page or entry is refused; an older sequence number is refused; at first install, one dated over 30 days back is refused, and one over 7 days is marked old; a page or entry over its limit is skipped; a read stops at the node's own limits; a tombstoned post in a snapshot stays gone; a deleted post from a stale snapshot is dropped once the author's media endpoint answers with its `Delete`. Measured: index and page sizes, and the time to check a full snapshot |
| 3 | Real nodes, with the first directory and its fallback on two test machines | A fresh install, given only the installer, finds a stranger; with the first off, through the fallback; with both off, through the mirror; with the mirror withholding too, the node says why it found nothing |
