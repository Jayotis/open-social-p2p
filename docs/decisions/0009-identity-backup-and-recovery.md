# ADR 0009: Identity backup and recovery

**Status:** Accepted (2026-10-22). Partly built: the recovery words, restoring onto a new node, follows that change node starting over, and `Moved`. Still to build, before any public release ([ADR 0006](0006-desktop-first-always-on-node.md)): the backup file.

## Context

A person's identity is the identity key on their home node. It signs every post and their profile, and other people know them by its public half ([ADR 0004](0004-signed-objects-and-ids.md)). Losing the key means losing the identity. ADR 0006 kept the identity key separate from the machine's node key so that it can be backed up and moved, and requires backup and recovery to be designed before any public release.

What a node holds today, in its home directory:

- `identity.key` (who the person is) and `node.key` (which machine this is on the network);
- the database: the person's own posts in the order they were published, their profile, who they follow, which nodes follow them, their paired phones, and other people's posts;
- the blob store: media of the person's own posts, replicas they host, and media cached for their phone.

How followers find a node today ([ADR 0003](0003-native-social-protocol-v0.md)): a follower's node keeps one address per identity it follows, taken from the follow code. It polls that node, takes its posts by their position in that node's outbox, and accepts "new posts" notices only from that node. The profile lists the identity's node IDs, but followers don't use that list to look for another node. An identity runs on one node at a time; when a node is replaced, the new one takes over rather than joining it.

Questions to settle:

1. What is backed up, and in what form?
2. How does a replacement node take over, and how do followers find it?
3. What happens when the key is lost or stolen?

## Options considered

**Backup.** (a) Recovery words for the identity key only. (b) Recovery words, plus an optional backup file of follows, followers and posts. (c) One file holding the key and the data, encrypted with a passphrase the person chooses. (d) Copies of the whole home directory.

Words on paper survive a dead disk and never change, but hold only 32 bytes. A file can hold everything but goes stale, and is lost with the disk it sits on unless it's kept elsewhere. With (c), forgetting the passphrase or losing the file loses everything. (d) carries the node key too, so two machines could end up running as the same node, and copies paired phones and other people's posts that the new node can fetch again.

**Theft.** (a) Accept that a stolen key means a lost identity. (b) Two levels of keys: a recovery key kept offline that can replace the everyday key on the node. Option (b) changes how objects are signed (ADR 0004: the signer would no longer be the identity itself but a key the identity certifies) and how every reader verifies them.

## Decision

Backup option (b), theft option (a) for now.

**Moving while the old computer still works.** Stop the node, copy its whole home directory to the new computer, and run `osp service install` there. The node key moves too, so nothing changes for followers or phones. The old copy must never run again. This needs no new code; it is the documented way to move.

**Recovery words.**

- The identity key's 32 bytes are written as **24 words** from the BIP39 English list. The words encode the key's bytes directly (BIP39's entropy encoding, not its seed derivation), so identities created before this change get words too. The last word carries a checksum, so a mistyped word is caught.
- `osp init` shows the words once and asks the owner to write them down. `osp backup words` shows them again.
- The words are shown **only in the node's own terminal**. No paired phone can ask for them, as with managing devices ([ADR 0008](0008-device-pairing-and-client-protocol.md)).
- There is no passphrase on top of the words: they are the key.

**Backup file** (optional): `osp backup export <file>`.

- It holds the profile, the person's own posts in outbox order (the signed objects byte for byte, so their IDs don't change), who they follow (identity and address), the nodes that follow them, and the media of their own posts. `--no-media` leaves the media out for a small file; restored posts then show their media only where someone who reshared them hosts a copy.
- It leaves out the node key, the paired phones (they belong to the old node; phones pair again), other people's posts and cached media (synced again from the people followed), and replicas (fetched again from other hosts after the restore, within the replica quota).
- It is encrypted and authenticated with a key derived from the identity key, in chunks, so a file of several gigabytes never needs to fit in memory. Whoever has the words can open it; without them it is unreadable, so it can be kept on a USB stick or in a cloud folder. The file starts with a format version and the identity's public key, so a restore can tell at once whether it matches the words.
- The file is a snapshot made by hand. Making it automatically, for example to a chosen folder or to the paired phone, is a later step.

**Restoring after the old computer is lost.**

- `osp init --restore` on the new computer asks for the words, and optionally a backup file. It makes a new node key and refuses to write over an existing home.
- The node signs a new profile that lists only itself, dated now.
- With a backup file, it imports the posts in their original order, the follows (each one starts like a new follow, with the latest 100 entries), the followers and the media. Then it tells each follower it has moved (below).
- With the words alone, the identity is back, but followers only find the new node when the owner gives them a new follow code (`osp invite`). Their copies of old posts stay valid, since the identity hasn't changed. The new node starts with no posts.

**Telling followers: `Moved`.** A new request in `osp/social/0`: `Moved { profile, addr }`, a signed `Person` and the sending node's address, which must be its own. A node accepts it only if:

- it follows that identity;
- the profile is valid under the object rules (ADR 0004) and its `nodes` list includes the sending node (the connection proves the sender holds that node's key);
- the profile is newer (`published` later) than the one it already has for that identity, so an old profile can't be replayed to point it back.

It then points the follow at the sending node's address, stores the new profile and subscribes to the new node.

A node knows it has taken an identity over when the profile it has stored lists a different node. It signs a new profile, dated later than the old one even if its clock is behind, and marks every follower as still to be told. It sends them `Moved` when it starts, then every hour, and before notifying them of each new post, until each has answered or subscribed to it. Followers not heard from for 30 days are forgotten as usual ([ADR 0003](0003-native-social-protocol-v0.md)).

**A follow that changes node starts over.** The new node numbers its outbox from 1, so a follow's position in the old node's outbox means nothing there. When a follow's node changes, whether through `Moved` or by following again with a code for a different node, the follow restarts like a new one: the latest 100 entries, skipping objects it already has (object IDs are content hashes, so duplicates are recognised).

**Lost or stolen.**

- **Words lost and no working node:** the identity is gone. The person starts a new identity, and followers follow the new one. Copies of the old posts stay signed by the old identity.
- **Key or words stolen:** whoever has them can sign as the person, including a `Moved` that points followers at their own node. The owner can answer with a newer `Moved`, but can't lock the thief out. The response is a new identity, announced to followers by other means. Until then, the key file stays readable only by its owner, the words are shown only in the node's terminal, and the backup file is encrypted.
- **Before any public release**, decide whether to add two levels of keys (theft option b). Changing how objects are signed is cheapest while nobody outside the project depends on the format.

## Consequences

- **Writing the words down is a one-time task.** They never change, even when the node is replaced.
- **The words are the identity.** Anyone who sees them can become the person. Setup must say so plainly.
- **A backup file goes stale.** Posts, follows and followers added after it was made are missing after a restore. Followers who joined later aren't told about the move; they find the person again through a new follow code.
- **Media makes the backup file large**, up to the size of everything the person has posted.
- **The backup file shows who the person follows and who follows them** to anyone with the words. Without the words, it shows only its size and the identity's public key.
- **Follow codes made before a restore point at the old node** and stop working. The owner makes new ones.
- **Phones pair again** with the new node.
- **Nodes too old to know `Moved`** can't act on it; their owners have to follow again. Stage 1 nodes are all test nodes, so they are simply updated.
- **No defence against theft yet.** A stolen key is a lost identity until the two-level decision is made.

## Validation and revisit conditions

- Tests: words to key and back; a wrong or missing word is refused; an export and import keep posts in order with the same IDs, with and without media; a file made for another identity is refused; a follower moves to the new node on `Moved` and restarts the follow, and refuses a `Moved` with an older profile, a profile that doesn't list the sender, or one for an identity it doesn't follow; following again with a code for a different node restarts the follow.
- On test nodes: restore one node's identity onto a fresh home with a new node key, check its followers move to it and receive its new posts, and pair a phone with it.
- Revisit before any public release (two levels of keys); if people often restore from stale files or lose them (make backups automatic); if followers who joined after a backup are often left behind (let a node ask the nodes it follows where an identity moved).
