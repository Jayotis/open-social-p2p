# Collective Discovery History, Personal Referrals, and P2P Snapshots

**Status: Proposed architecture only. No reporting feature, protocol, anonymity mechanism or attribution implementation exists.**

## Core intention

The social network should recognize people who **watch, explore, find and recommend other people's content**, not only the creators who upload content or accounts with the most followers. Human recommendations happen on- and offline: a friend tells a co-worker about a video, a teammate shows a phone to another teammate, or a student recommends a post verbally. The product should respect that real-world social discovery without attempting to track every offline conversation.

The network may maintain an **open, collectively available picture of discovery**, using opt-in P2P participation. However, **this must never mean uploading every user's recent viewing or browsing history to a globally replicated file**. Even de-identified sequences of visits can identify individuals.

Research: Mozilla's [2020 study on re-identification of browsing histories](https://www.usenix.org/conference/soups2020/presentation/bird) found that almost all observed browsing profiles were unique. See also [2025 Scientific Reports research](https://www.nature.com/articles/s41598-025-19950-3). "No account name" is not the same as "anonymous."

## Three different record classes

| Information | Ownership / publication | Purpose |
| --- | --- | --- |
| **Private personal activity** | Local to the user, absent a separate explicit action | Continue watching, private saves, personal recommendations; not gossiped |
| **Public, chosen referrals** | Explicitly published signed social activity with creator attribution | Let others follow someone's curation or see that they introduced a work |
| **Privacy-preserving aggregate trends** | Independently hosted short-lived summary snapshots, when a defensible aggregation process exists | Discover growing public interest without exposing participant histories |

These records must **never be merged into a discoverable per-person viewing trail**. Hosting a replica is also not the same as endorsing it.

## The offline referral problem

A watcher/curator—not the author—can become known among classmates, coworkers, teammates or friends as a good source of things to read and watch. The network should allow this without requiring a global "first finder" claim.

### Possible voluntary attribution interactions

- **Recommend**: A person publishes a public curation post about someone else's content, with commentary/context and clear original creator credit. Other people may follow that curator.
- **Share in person**: App displays a short link or QR code for the public work, optionally pointing at the curator's public recommendation, rather than generating a secret identifier for the recipient.
- **Give credit**: A recipient can voluntarily say "Found this through [curator]" on a *public* recommendation or thank-you post. It must never automatically publish a relationship, browsing event or encounter.
- **Curator collection**: A person publicly collects interesting work into topic shelves/collections; creator attribution remains visible. Lists can be followed if the curator chooses.

A QR or link associated with a public recommendation can attribute a referral **without tracking each person who scans it**. We should not silently collect recipient identities, scan histories, unique referral tokens or precise offline locations.

Multiple people can recommend the same work. A trustworthy curator is not necessarily the earliest recorded account and should not need a global scoreboard.

## Optional P2P trend contribution — NOT raw history upload

A user could opt into contributing limited **statistical signals** about approved public content to a discovery process. That is separate from opting into media seeding and from publishing a recommendation.

Proposed privacy workflow (subject to actual cryptographic evaluation):

1. Personal activity stays on the device. The default is *no private activity reporting*.
2. The user separately opts into a clearly explained contribution type and scope.
3. The app selects a bounded contribution under a published privacy budget; ordinary watching does not create a public endorsement.
4. Investigate **secure aggregation and/or local differential privacy**, encrypted transport, hiding source addresses from public snapshot peers, and suppression of information with too few participants. Simply hashing a content ID or omitting usernames does not anonymize an event.
5. Only vetted aggregate, time-windowed estimates for eligible **public** content are distributed; do not disseminate raw device contributions. Report cohort/scope and uncertainty.
6. Publish only snapshots with a finite expiry, reducing exposure and preventing a permanent public record of moment-by-moment behavior.

Privacy techniques need independent review and practical sample-size thresholds. For a small early network, genuinely private aggregate discovery metrics may be too noisy to use. Use **public reshares and curator recommendations** instead until privacy is defensible.

Research example: [Apple's 2025 discussion of differential privacy for aggregate trends](https://machinelearning.apple.com/research/differential-privacy-aggregate-trends). That example is not an off-the-shelf design or proof that our network is anonymous.

## A replicated, sharded discovery snapshot (alternative to one giant shared file)

Do not publish a permanent raw-event ledger or one monolithic immutable file replicated to every user.

Conceptually, the network could create multiple **content-addressed, versioned discovery snapshots**, grouped by:

- Topic/community (without leaking private memberships).
- Broad observation window (for example, an interval appropriate to the signal).
- Public visibility and eligible region/language scope only when privacy can be maintained.
- Algorithm/snapshot format version.

A snapshot might have a manifest with: declared source scope; public candidate post/topic identifiers; coarse, privacy-protected aggregate trend bands; creation and expiration metadata; and a signature by its publisher. If it incorporates earlier snapshots, define deduplication and prevent double counting. A signature verifies who published a summary, **not** that the reported values are truthful.

P2P nodes **may voluntarily mirror selected snapshots** and offer them to others; not every desktop has to store every shard. Clients can subscribe to topics and retrieve only needed summaries. Retention/expiry and a successor snapshot mechanism limit storage and help correct mistakes.

**A global immutable replicated activity ledger is explicitly out of scope.** It would amplify privacy, takedown, storage and correction problems.

## How a collective discovery view might appear

- **Under the Radar**: public work with low observed exposure; not necessarily new.
- **Rising**: public work with recent attention beyond its previous baseline, with known coverage limits.
- **Shared by Scouts**: recommendations by public curators the user chooses to follow.
- **Discovered in Another Community**: public works introduced across participating communities, without exposing who privately saw them.
- **Offline Recommendations**: creator-linked public recommendations shared by QR or in conversation, with no mandatory tracking.

Labels like "most popular worldwide" or "nobody has seen this" would be misleading given incomplete federation and opt-in participation.

## Separate privacy and public attribution

A person's **public curation activity is naturally attributable when they choose to publish it**. That can help them build a reputation as the person who finds interesting things.

A person's **private browsing/watching record is not a public social graph**. It should not be published even in pseudonymous form. This is a core product boundary, not a cosmetic settings choice.

### Negative requirements

- No automatic anonymous-looking download log gossiped to peers.
- No shared per-device history file, hashed URL timeline, reusable activity pseudonym or unique scan tracking by default.
- No private-group/direct-message media identifiers in shared discovery snapshots.
- No automatic credit for autoplay, transfer bandwidth, number of replicas, or viewing alone.
- No public actor/recipient chain from an offline conversation without the people's explicit choices.
- No claims of anonymity from simply removing account names or signing summaries.

## Risks and research questions

1. Can a distributed secure-aggregation protocol work without a single trusted operator, and which parties see metadata during collection?
2. What sample sizes and privacy budgets are needed before any report about a low-interest work is safe to publish?
3. Can public engagement summaries be built from existing ActivityPub data without any private activity reporting?
4. How can collectors avoid Sybil farms, fake accounts, fabricated trend snapshots and double counting across federation?
5. How are snapshots versioned, invalidated, corrected and expired across unreliable home nodes?
6. Can a curator be recognized for introducing an overlooked post **without** tracking a recipient or falsely claiming causal influence?
7. How should offline word-of-mouth referrals be acknowledged without creating a physical-world surveillance system?
8. What is the optional bandwidth/storage cost for redistributing signed snapshot shards?

## Development gate

First demonstrate discovery using **explicit public recommendations only**. Do not turn on private-history-based statistics until an independently reviewed privacy design, manipulation tests, clear user consent and workable opt-out have been specified.

Related design documents: [Open Feed](08-open-feed-algorithm.md), [Distributed Trending](09-distributed-trending-signals.md), [Exploration and Scouts](10-exploration-divergence-and-scouts.md), and [Security and Privacy](05-security-privacy-and-governance.md).
