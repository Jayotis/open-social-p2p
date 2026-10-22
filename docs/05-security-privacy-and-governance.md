# Security, Privacy, Moderation and Governance

**Status:** Threat-model starting point. A security audit has not occurred.

## Privacy boundaries

### Public posts

Treat public replicated media as reproducible by anyone who obtains it. Content addressing and P2P distribution make guaranteed deletion unrealistic. Explain this at publication time.

### Private communications and groups

Do not assume ActivityPub private messages are end-to-end encrypted. Group membership, authorized recipients, encryption, key rotation, revocation and cross-server delivery need explicit protocols and independent review.

Default to **no public peer replication** of restricted-audience content until that work is complete.

### Home-network exposure

Direct P2P may expose IP addresses. Research relay-only mode and NAT traversal tradeoffs. Never require router port forwarding for ordinary users. Use authenticated encrypted transport, network isolation and least-privilege filesystem permissions.

## Threat-model topics

- Malicious peers: tampering, corrupt chunks, spoofed availability, malicious payloads and flooding.
- Malicious users: harassment, impersonation, account compromise, scraping, illegal content sharing and mass reporting abuse.
- Collusion/Sybil: fabricated peer ratings, false availability and resource exhaustion.
- Metadata leakage: IP addresses, timing, following relationships, audience membership and media access.
- Compromised desktop companion: path traversal, untrusted file parsing, update supply chain and unsafe permissions.
- Abuse of relays/optional hosting: bandwidth theft, denial of service and unlawful redistribution.
- Misconfigured privacy: content published to broader audiences than intended.

## Controls to investigate

- Authenticated content manifests and integrity checking on every downloaded chunk.
- Secure pairing, credential storage and device revocation.
- Per-account and per-IP abuse rate controls appropriate to privacy goals.
- Signed client releases and reproducible build goals.
- Encryption in transit, safe defaults, minimal telemetry and data-retention limits.
- User reporting, blocking, content labels, appeals, transparent moderation and abuse response.
- Clear consent for seeding others' media; ability to disable hosting.
- A documented process for security reports and coordinated fixes.

## Ideas to explore (not designed yet)

### Trusted friends as a safety network

Each node already notices some misbehaviour for itself: oversized or endless requests, floods of subscribers, outboxes that never move on, objects that fail their signature or limits. The idea is to let people share those observations with friends they choose, so the network polices itself without a central moderator.

- The owner marks some identities as **trusted friends**. Their node can ask a friend's node, "have you seen problems from this node or identity?", and answers the same question for its friends.
- A node uses the answers for its own choices only: warn the owner before following someone, give a peer lower limits, or refuse it. Nothing becomes a network-wide ban.

Questions to settle before building it:

- **Observations, not verdicts.** Share what was measured ("sent 500 malformed objects on this date") rather than labels ("bad actor"). Keep measured protocol misbehaviour, which is objective, apart from reports about content, which are judgments.
- **False reports and cliques.** One friend's report shouldn't settle anything. Weight answers by how much the owner trusts each friend, let old observations fade, and show the owner where a warning came from.
- **Privacy.** Answers go only to friends who ask, never to a public list. Observations cover how a peer behaved towards your node, never what you read or watched.
- **Relation to other docs:** local peer assessment in [doc 12](12-peer-reciprocity-and-recognition.md) ("measure, don't trust claims") would supply the observations, and the open question below about network-wide abuse reports is the problem this would answer.

### Content screening on the home node

The home node receives everything before the phone does, which makes it the place to screen content for its owner: run a local vision model (for example one served by Ollama) over images, thumbnails and sampled video frames as they are prefetched, and blur, label or hide what the owner chose not to see, such as gore or nudity. The same model could warn the owner before they publish something themselves.

- **Private by design:** the model runs on the owner's own hardware and nothing leaves the house, in line with on-device feed filtering in [doc 08](08-open-feed-algorithm.md). The model, its version and its thresholds should be published like the feed algorithm.
- **Mistakes happen both ways.** Label and blur with a "show anyway" choice rather than deleting silently, and let the owner tune or turn it off.
- **Hardware:** a vision model needs a desktop GPU or a strong CPU. A low-power node, such as a single-board computer, would need an accelerator, or would have to leave screening off.
- **Illegal content**, above all child sexual abuse material, carries legal duties. A node must never keep or forward a detected match, and how detection and reporting should work for private individuals needs legal advice before anything is built. Known-image hash lists are usually only available to organizations.

## Licensing and contribution governance

The repository includes an AGPL-3.0 license. Determine treatment of design documentation and third-party dependencies before accepting significant external contributions. Keep security decisions and protocol changes open to review, but avoid publishing exploitable operational secrets.

## Open governance questions

- Who can operate compatible servers and coordinate peer discovery?
- How do users choose moderation policies or move between providers?
- How will a network-wide abuse report be handled without centralizing every file?
- What does a valid takedown request mean for independent home hosts?
- What telemetry is strictly necessary to evaluate network health?
