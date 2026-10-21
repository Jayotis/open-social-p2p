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

## Licensing and contribution governance

The repository includes an AGPL-3.0 license. Determine treatment of design documentation and third-party dependencies before accepting significant external contributions. Keep security decisions and protocol changes open to review, but avoid publishing exploitable operational secrets.

## Open governance questions

- Who can operate compatible servers and coordinate peer discovery?
- How do users choose moderation policies or move between providers?
- How will a network-wide abuse report be handled without centralizing every file?
- What does a valid takedown request mean for independent home hosts?
- What telemetry is strictly necessary to evaluate network health?
