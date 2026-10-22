# ADR 0004: Signed objects and content identifiers

**Status:** Accepted (2026-10-21)

## Context

Posts, profiles and reshares are copied between nodes, cached and served by people other than their author. Any node must be able to check that an object really comes from its author and has not been altered, and media chunks must be verifiable against something the author signed ([doc 03](../03-peer-distribution-and-scoring.md)).

Signing JSON usually requires canonicalising it first (sorting keys, normalising numbers), and implementations often get that subtly wrong.

## Decision

**Keys**

- **Identity key** (ed25519): the author. Signs posts, the profile and, later, device grants. Lives on the home node.
- **Node key** (ed25519, iroh's endpoint key): one per machine; identifies the node on the network. The profile, signed by the identity key, lists the identity's current node IDs.
- **Device key**: each paired phone has its own key, which its home node records at pairing.

Identity and node keys are separate from the start so that a person can move their identity to another machine, or run two nodes, without changing who they are.

**Signed object**

```json
{
  "payload": "<base64url of the exact UTF-8 JSON bytes>",
  "signer":  "<hex ed25519 public key of the identity>",
  "sig":     "<base64url ed25519 signature>"
}
```

- `sig = ed25519_sign(identity_key, "osp-v0:" || payload_bytes)`. The prefix separates our signatures from any other use of the same key.
- **Object ID = hex BLAKE3 of `payload_bytes`.**
- The payload bytes are stored and transmitted exactly as signed and **never re-serialised**, so no JSON canonicalisation is needed. Readers parse the bytes; writers never rebuild them.
- The payload's `actor` field must equal `signer`; receivers reject objects where it doesn't.
- **Limits.** Signing and verifying both refuse objects outside these limits, so one author can't make readers store or fetch without bound:
  - the payload is at most 128 KiB;
  - a profile name is at most 200 bytes, its summary at most 2,000, and it lists at most 8 nodes;
  - post text is at most 10,000 bytes, with at most 4 attachments;
  - an attachment is `video/mp4` or `image/webp` (media profile v1, [ADR 0005](0005-media-profile-v1.md)) and declares at most 1 GiB; its alt text is at most 1,500 bytes; its thumbnail is `image/webp` and declares at most 2 MiB.

**Media**

Each attachment inside a signed `Note` carries the BLAKE3 hash of the blob (plus size, MIME type, dimensions, duration, thumbnail hash and alt text). iroh-blobs addresses and verifies content by the same BLAKE3 hash, so **every downloaded chunk is checked against a hash the author signed**. The Note itself is the signed media manifest; there is no separate manifest object.

## Consequences

- Any language can implement verification: base64url-decode, verify ed25519, BLAKE3 the bytes.
- Objects are immutable. Edits and deletions will be new objects (`Update` and `Delete` in AS2 terms) that reference the original ID; deletion can be requested but not guaranteed ([doc 05](../05-security-privacy-and-governance.md)).
- Losing the identity key means losing the identity. Backup and recovery (recovery words and a backup file) are decided in [ADR 0009](0009-identity-backup-and-recovery.md).

## Validation and revisit conditions

- Unit tests: sign/verify round trip; a single changed payload byte fails verification; the ID is stable across runs.
- Revisit if key rotation or multi-device signing (the phone signing posts itself) is needed. That would add identity-signed delegation certificates, not change this format.
