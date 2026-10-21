# Peer Distribution and Adaptive Source Scoring

**Status:** Proposed protocol behavior; no implementation exists in this repository.

## Core concept

A creator can host original media from their own computer. When a second person reshares the associated public post, that person may **opt in** to keeping and serving a verified copy. As interest grows, additional peers can reduce load on the creator and improve availability.

A social repost and a content replica are separate events. The application must report accurately whether a device is actually hosting data.

## Peer classes

- **Origin:** Keeps the creator's chosen original(s).
- **Replica:** Voluntarily keeps allowed copies after resharing or explicit pinning.
- **Viewer:** Temporarily retrieves verified chunks; may optionally upload them during playback.
- **Relay:** Helps connectivity where direct connections fail; needs clear bandwidth accounting.
- **Optional hosted provider:** Supports people without reliable home equipment.

## Peer grading: measure rather than trust claims

Each downloading client maintains its own *local*, short-lived assessment of candidate sources. Proposed observations:

| Signal | Desired interpretation | Important caveat |
| --- | --- | --- |
| Measured throughput | Higher rate may help larger transfers | Depends on path, file size and contention |
| Time to first byte / RTT | Lower latency helps responsiveness | One ping alone is misleading |
| Completion ratio and timeouts | Reliable providers should be preferred | Network instability should decay out of the score |
| Current measured load | Spread requests across uncongested sources | Claimed load is advisory, not trusted |
| Chunk verification failures | Penalize incorrect data or abuse | Hash verification is mandatory regardless of ranking |
| Recent success on this network | Prefer recent local evidence | Avoid permanently blacklisting slow peers |

Weights, thresholds, and decay periods are **research questions**, not finalized constants.

## Proposed selection policy

1. Reject disallowed, unverified or unauthorized sources.
2. Honor user bandwidth caps and source constraints.
3. Start uncertain peers with limited trial requests to avoid a permanent winner-takes-all outcome.
4. Compare measured speed, latency and completion, then allocate more chunks to better-performing peers.
5. Retain some workload for other healthy peers to distribute demand and continue measuring.
6. Reduce requests when a peer is overloaded or failing; retry chunks elsewhere.
7. Refresh assessments continuously; do not publish globally authoritative speed grades.

Fast sources should generally carry more traffic *only when they have capacity and consent*. Slow connections still contribute when appropriate. Avoid concentrating all traffic on one very fast residential node.

## Integrity and trust

Content chunks require cryptographic verification against a signed manifest or other authenticated content reference. A fast node must not be trusted merely because it is fast.

Do not accept arbitrary third-party reputation reports as truth; that creates manipulation and Sybil-attack incentives. Distinguish performance (observable) from abuse/account trust (separate moderation decisions).

## Privacy requirements

Direct peer connections can disclose IP addresses and timing information. Allow privacy-preserving relays, with visible cost/performance tradeoffs. Do not put private-group media into public discovery or unrestricted chunk swarms.

For private content, authorization and encryption must be designed and independently reviewed before replication is permitted.

## Replication limits

Users choose how much disk space and upload bandwidth they offer. A reshare can *request* local replication only when the user has opted in; it must not force a download onto a mobile data plan or consume unlimited disk.

Use retention and eviction policy for other people's cached media, distinct from a creator's own originals. A 'share' must remain possible without hosting.

## Offline, deletion and moderation

Replicated public media may remain available when the origin goes offline. Removal requests should propagate through cooperating peers, but public copies outside project control may persist. Moderation, takedown review, abuse response, copyright reporting, privacy controls and transparency are required, not optional.

## Acceptance tests to define

- A publishes, B shares and voluntarily hosts, A disconnects, C can still retrieve verified content.
- One fast and two slow peers distribute work without exhausting the slow nodes.
- A dishonest source supplies a corrupt chunk; the client rejects it and retries.
- A peer disconnects mid-transfer; playback/retrieval recovers.
- A user disables hosting or hits their quota; new hosting stops and cleanup follows policy.
- Relayed connections work with difficult NAT configurations, while their costs are measured.
- Private content never enters the public provider index.
