# Distributed Trending Signals and Privacy

**Status: Research design — not a protocol implementation.**

Open Social P2P proposes to use the peer-to-peer network as one source of evidence for discovering emerging public posts and topics. **Do not confuse traffic statistics with meaningful engagement, and do not make individual reading behavior public.**

The goal is a distributed discovery system, not an omniscient worldwide popularity counter.

## Roles in the proposed system

- **Federated social servers** exchange public posts and public interactions compatible with ActivityPub. They see an incomplete subset of the wider network.
- **Desktop media nodes** serve user-authorized copies of public files and locally observe their own connection performance and traffic.
- **Interested participants/clients** discover trend candidates from social servers, selected topic peers or optional discovery services and evaluate them using a publicly specified algorithm.
- **Optional aggregators** collect authorized, bounded, privacy-preserving summary reports from opt-in nodes and share signed *summaries*, not raw browsing logs.
- **Flutter app** displays the user's selected feeds and optional recommendations; it must not require installation of a desktop node.

Independent actors can operate compatible discovery infrastructure. Avoid mandatory reliance on one operator's secret database or global API.

## The information model: three distinct layers

### A. Public social activity

Examples include public post creation, replies, likes and repost/Announce activities from ActivityPub, where available. Normalize duplicate events and check activity provenance to the extent possible.

These signals can indicate communication or public engagement; **they do not reveal how every person on every server reacted**.

### B. P2P delivery health and availability

Examples include candidate providers, validated replica availability, chunk completion, latency and observed throughput. This data helps decide **where to retrieve media** and whether capacity should be spread across peers.

**Do not directly convert transfer speed, amount uploaded, replica count or network uptime into a popularity score.** This is part of the separate peer-performance algorithm.

### C. Optional aggregated demand hints

Coarse, opt-in summaries can suggest that a public media object is being requested unusually often in a given time interval, helping discover candidates and plan replication.

Raw requests, bytes, replay counts, prefetches and retry attempts must not count as independent votes or exact unique viewers. Automatic playback must not silently create popularity endorsements.

## Proposed distributed discovery flow

1. A public post enters the social network through federation. It has a stable canonical post reference and optional media/content manifest.
2. Eligible public social activities contribute to time-windowed candidate discovery on participating servers or nodes.
3. Desktop nodes may, with separate consent, contribute **bounded aggregate demand observations** about eligible public media over coarse intervals.
4. A P2P gossip or subscription mechanism may distribute signed candidate summaries among interested discovery participants, avoiding raw per-view event broadcasts.
5. Recipients validate provenance and declared methodology, compare multiple independent observations where possible, deduplicate relayed summaries and mark uncertain results.
6. The openly specified trending algorithm estimates rising public attention, with safeguards for low-volume and manipulated data.
7. The user's app performs eligibility filtering and optional local personalization, displaying the source/scope of any trend description.

**Important:** A transport like Gossipsub helps propagate data and locally assess message senders. It does not make popularity claims honest, solve Sybil resistance or guarantee representative global coverage.

## Designing a shareable summary

If research supports privacy-safe reporting, a candidate summary might describe:

- A canonical public post or topic identifier.
- A coarse observation time window.
- The declared population/scope of contributors (such as participating servers or opt-in nodes).
- Type of aggregate signal and count *band or privacy-protected estimate*, not exact identifiable download logs.
- Version of the published aggregation method.
- A validation mechanism for the reporting party and a freshness/expiry period.

The exact schema is deliberately undecided. Signed summaries authenticate the **sender**, not the truth or uniqueness of claimed audiences. Different summaries may overlap; naive addition will double count. Counting unique people without tracking them is a difficult research problem, not an achieved property.

## Non-negotiable privacy boundaries

- Private, followers-only, group-limited or direct-message content must not enter a public trending feed or public demand-reporting system.
- Never gossip a list of who watched, downloaded or hosted an item, together with timestamps or IP addresses.
- Do not upload personal browsing histories or a user's followed-topic profile to other peers by default.
- Media hosting consent and contribution of demand metrics must be **separate opt-in choices**. Peer seeding must not automatically enable popularity reporting.
- Geographical trends require separate, informed consent and coarse regions; no precise location collection for default trending.
- For small samples, suppress reporting or use a rigorously evaluated privacy technique. A simple threshold is not, by itself, a guarantee of privacy. Differential privacy or secure aggregation are research options, not built-in guarantees.
- Provide retention limits, locally erasable observations, and documentation of which aggregate information cannot be recalled after distribution.
- A direct P2P connection may expose home IP addresses; privacy-conscious users need relay options independent of whether they contribute trend observations.

## Anti-manipulation research requirements

- Adversarial participants can fabricate downloads, relays, self-hosting copies, accounts and entire servers.
- Cryptographic signatures prove control of a key, not that the key belongs to one real person.
- Cross-server "uniqueness" and diversity need skepticism; malicious operators can spawn many apparent sources.
- Rate limits, per-source contribution caps, time decay, independent corroboration and anomaly detection are candidate mechanisms, not complete defenses.
- Public interaction counts should be deduplicated per applicable actor/object/window where possible; document where ActivityPub permits repeated Announce events.
- Popularity measurement and safety moderation must be separate. Users can block content and moderators may restrict distribution even where public activity is high.
- Each reported trend should have a notion of confidence/scope. Prefer labels such as "trending in participating communities" over unsupported "most popular globally".

## Availability and fairness

Nodes join and leave intermittently. Popularity reports should expire. Small, remote or bandwidth-poor communities must not be automatically invisible because fewer nodes report data. Candidate selection should reserve room for low-volume emerging content.

## Research decisions

1. Which parts of aggregation occur locally, on social servers and on independently hosted coordination nodes?
2. Which data can be derived from public ActivityPub alone, and which would require user opt-in?
3. Can public post activity be distributed as a compact, authenticated trend summary without creating an invasive tracking network?
4. What protections work against malicious servers and coordinated fake accounts?
5. How are reposts and media aliases deduplicated across servers?
6. Can private aggregate metrics be reported safely at the sizes of our expected early communities?
7. How is trend scope, recency, confidence and server coverage shown honestly to users?
8. What is the resource cost of gossiping candidate summaries, and how do we prevent it from becoming another central infrastructure bottleneck?

## Success criteria (to define and test)

A set of independent participants can discover and rank public trending candidates without a required centralized source of personal browsing records; users can opt out of both recommendation feeds and optional demand reporting; no private content appears; adversarial download spam alone cannot reliably force trending placement; and the UI reports incomplete coverage honestly.

These are **design goals**, not assertions that a working, private or attack-resistant system exists today.

## Related documents

- [Open Feed and Recommendation Algorithm](08-open-feed-algorithm.md)
- [Peer Distribution and Adaptive Source Scoring](03-peer-distribution-and-scoring.md)
- [Security, Privacy and Governance](05-security-privacy-and-governance.md)
- [W3C ActivityPub](https://www.w3.org/TR/activitypub/)
- [Gossipsub peer scoring and gossip](https://github.com/libp2p/specs/blob/master/pubsub/gossipsub/gossipsub-v1.1.md)
