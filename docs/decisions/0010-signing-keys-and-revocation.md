# ADR 0010: Signing keys, and taking a stolen one back

**Status:** Accepted (2026-10-22). Not built yet.

## Context

Today the identity key lives on the home node and signs everything: posts, reshares and the profile ([ADR 0004](0004-signed-objects-and-ids.md)). Its 24 recovery words are the same key written another way ([ADR 0009](0009-identity-backup-and-recovery.md)). Whoever copies that key, by taking the computer, copying its disk or running malware on it, can sign as the person for good:

- they can publish posts that every reader accepts;
- they can send `Moved` and point followers at their own node. The owner can answer with a newer `Moved`, but the thief can answer that too, and nothing tells followers which of the two is the owner;
- the owner can't revoke the key. Their only way out is a new identity, announced to followers some other way.

ADR 0009 accepted this for now and asked for a decision on two levels of keys before any public release. Changing how objects are signed is cheapest now, while every node is a test node.

Questions to settle:

1. Which key signs objects, and how does a reader know it speaks for the identity?
2. How does the owner replace a stolen key, and what happens to what it signed?
3. Where do the recovery words fit, and what changes for backups?

## Options considered

**Keys.** (a) Keep one key; theft means a lost identity (ADR 0009 today). (b) Two levels: the identity key stays off the node and signs a certificate for a signing key that lives on the node; the newest certificate wins. (c) Pre-rotation, as in KERI: each key commits in advance to the next one, which is kept offline until it's needed. (d) Two levels as in (b), with the identity key kept on the paired phone instead of on paper.

(c) needs new words written down after every key change. (d) puts the master key on the device people lose, reset and replace most often; the phone's secure storage can even clear itself after some resets ([ADR 0008](0008-device-pairing-and-client-protocol.md)). It could be added later as a second place to keep the key, not the only one.

**What happens to objects signed by a stolen key.** (i) Readers keep everything they already have and refuse only what arrives after they learn of the change. But dates are whatever the signer writes, so a thief can backdate. (ii) Every object signed by the stolen key becomes invalid, and the new key signs the owner's real posts again. (iii) The owner signs a list of the object IDs to keep. A list of every post soon outgrows one object, and it would need its own format.

## Decision

Keys option (b). For objects signed by a stolen key, option (ii), but only when the owner says the key may have been copied. Otherwise the old key is just retired, and what it signed stays valid.

**Two keys.**

- **Identity key** (ed25519). Its public half is the identity's ID, as today: in `actor`, in follow codes, everywhere. Its secret half exists **only as the 24 recovery words**. The node uses it in memory to sign a certificate, then forgets it. It is never written to disk.
- **Signing key** (ed25519), in `signing.key` on the node, readable only by its owner. It signs posts, reshares and the profile.
- **Key certificate**: a new object type, `Key`, signed by the identity key itself:

```json
{
  "type": "Key",
  "actor": "<hex public key of the identity>",
  "key": "<hex public key of the signing key>",
  "published": "2026-10-23T09:00:00.000Z",
  "earlierKeys": "retired"
}
```

`earlierKeys` is `retired` or `revoked`. It says what happens to every key certified before this one (below).

**Signed objects** get one optional field:

```json
{
  "payload": "<base64url of the exact UTF-8 JSON bytes>",
  "signer":  "<hex public key of the identity>",
  "key":     { "payload": "…", "signer": "…", "sig": "…" },
  "sig":     "<base64url signature by the key the certificate names>"
}
```

- With `key`: it must be a valid `Key` object, signed by the identity itself (so it has no `key` of its own), and its `actor` must be `signer`. `sig` must then check against the signing key it names. A certificate can't certify another key that certifies a third, so there are no chains to follow.
- Without `key`: `sig` is by the identity key itself, as today. Nodes do this only for `Key` objects. Every object made before this change was signed this way, and stays valid.
- Everything else in ADR 0004 is unchanged: the same `osp-v0:` prefix, the limits, and the **object ID is still the hash of the payload bytes**. The signature isn't part of the ID, so an object signed again by a new key keeps its ID.
- A certificate follows the same date rule as profiles: dated at most 5 minutes ahead of the reader's clock ([ADR 0009](0009-identity-backup-and-recovery.md)).
- Each object grows by about 500 bytes. A later version could send each certificate once per outbox page instead.

**What a reader keeps.** For each identity it has seen: the newest certificate, and the date of the newest one that said `revoked`, if any. Every object that arrives, by any path (an outbox page, a profile, `Moved`, a post bundled with a reshare), is checked like this:

1. Its signatures, as above.
2. If its certificate is newer than the one the reader has, that certificate becomes the identity's current one. If it says `revoked`, every key certified before it is revoked from then on.
3. An object whose certificate is dated before the latest revocation is refused. Objects signed by retired keys are accepted, since they are the person's history. So are objects the identity key signed itself, since only the words can make those.

So news of a revocation travels with anything the new key signs. A thief holding the signing key but not the words can't make a certificate. Once a reader has seen a revoking certificate, it refuses everything the thief signs, including a `Moved` back.

**When a reader learns of a revocation**, it deletes the objects it holds that the revoked keys signed. If it follows the identity, it first asks the identity's node for copies signed by the new key: a new request in `osp/social`, `GetObjects { ids }`, at most 100 IDs at a time, answered from the node's own posts and reshares. A copy that comes back replaces the old one under the same ID. What doesn't come back, such as posts the thief made, is gone.

**Commands.**

- `osp init` makes the identity key, shows its 24 words **once**, and asks for three of them back to check they were written down. It then makes the signing key and its certificate, and forgets the identity key. `osp backup words` goes away, since the node no longer has the key. In its place, `osp keys check` asks for the words and says whether they belong to this node's identity.
- `osp keys replace` (someone may have copied this node's key, but the owner still has the node): asks for the words, makes a new signing key with a certificate that revokes the earlier ones, and signs the node's whole outbox again. The outbox is complete, so none of the owner's own posts are lost.
- `osp init --restore [--backup <file>]` (the computer is gone): as in ADR 0009, plus a new signing key. It asks whether anyone else could have a copy of the old node's key:
  - **No** (for example, the disk died): the old key is retired. Followers keep every post it signed.
  - **Yes or not sure** (stolen, malware, a disk sold or lost): the old key is revoked. The posts in the backup file are signed again with the new key. Posts made after the backup, and any the thief made, are gone. With the words alone, every earlier post is gone.

  The flags `--retired` and `--revoked` answer the question without asking.
- After any key change, the node marks every follower it knows as still to be told and sends them `Moved`, as after a move (ADR 0009), even when the node itself hasn't changed.
- A new certificate is dated later than the newest one the node knows of: its own, or the one in the backup file. With the words alone the node knows none, so its clock must be right. Followers refuse an older certificate, and say why.

**Backups.** The backup file can no longer be encrypted with a key derived from the identity key, since the node doesn't have it. When the node signs a certificate, it derives a **backup key** from the identity key and the new signing key's public half, and keeps that in `backup.key`. Files are encrypted with that key, as in ADR 0009. The file's header names the signing key, so a restore can derive the same key from the words. A key change gives a new backup key, so a thief who copied the old one can't open later files. Files made before this change still open with the words.

**Nodes made before this change** still hold `identity.key`. They keep working: objects signed by the identity key itself are valid. `osp status` reports it as a problem until the owner runs `osp keys protect`. That shows the words, asks for three back, makes the signing key, certificate and backup key, and deletes `identity.key`. Nothing needs signing again.

**The protocol becomes `osp/social/1`.** Nodes too old to know `Key` objects would refuse every new object as wrongly signed. Changing the protocol name makes a mismatched node fail with a clear error instead. The client protocol to the phone is unchanged; the phone doesn't check signatures (ADR 0008).

## Consequences

- **A copied signing key can be taken back**, by anyone with the words. Readers who have seen the new certificate refuse the old key.
- **Readers the owner can't reach keep trusting the thief** until something signed with the new key reaches them. That includes followers who joined after the backup file was made, and people who see the person's posts only through reshares.
- **The words are shown once, and the node can't show them again.** Losing them means the identity can't be restored, and a copied key can't be revoked. `osp init` checks they were written down; `osp keys check` checks the paper copy later. The words belong on paper, not on the computer they protect.
- **The words are still everything.** Whoever has them can certify their own key and revoke the owner's. This is the same as ADR 0009, and the answer is still a new identity.
- **Revoking costs history**: posts not in the backup file are lost along with the thief's. Retiring keeps every post, including any a thief made with the old key. The owner chooses, and the restore says so in plain words.
- **Followers fetch the person's posts again after a revocation**, up to 100 per request. An identity with a long history means a few seconds of extra syncing.
- **Objects are about 500 bytes bigger.**
- **Every test node is updated at once.** Nodes on `osp/social/0` and `osp/social/1` can't talk. Stage 1 nodes are all test nodes ([ADR 0006](0006-desktop-first-always-on-node.md)), so they are simply updated.
- **Readers store a little more**: one row per identity they've seen, for its key state (a database migration).
- **A stolen node still holds the phone pairings.** Phones keep reaching the stolen node until they pair with the new one, which replaces it in the app.

## Validation and revisit conditions

- Unit tests: an object signed by a certified key verifies; refused when the certificate is for another identity, has a `key` of its own, has been tampered with, or is dated too far ahead; an object signed by the identity key itself still verifies; an object signed again by a new key keeps its ID.
- Integration tests: a thief holding the signing key posts and moves Bob's follow to their node. The owner restores with the words, a backup file and "revoked". Bob moves back, the thief's posts are gone, the owner's backed-up posts are there with the same IDs, and the thief's next post and `Moved` are refused. A restore with the words alone and "retired" keeps every earlier post. `osp keys replace` keeps every post on the node, and followers take the new certificate. A backup made after a key change opens with the words. An old home with `identity.key` keeps working, and `osp keys protect` converts it without changing any post's ID.
- On test nodes: run `osp keys protect` on each; `osp keys replace` on one, with its followers taking the new certificate.
- Revisit if people often lose their words (keep the identity key on the paired phone as well, option d); if a person needs several nodes or a phone that signs posts itself (several current keys at once); or if followers often miss a revocation (spread certificates further, for example through the nodes that host a person's media).
