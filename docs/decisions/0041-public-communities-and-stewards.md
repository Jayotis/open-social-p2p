# ADR 0041: Public communities governed by independent stewards

**Status:** Proposed (2026-11-16). Design direction approved by the maintainer; protocol, security and simulation gates remain open. **Not implemented.** Work is deferred until after the current Product Alpha gate in [ADR 0030](0030-product-alpha-before-testers.md). This record does not modify the current three-place shell in [ADR 0031](0031-app-shell-home-universe-you.md).

Builds on [ADR 0004](0004-signed-objects-and-ids.md) (signed content), [ADR 0010](0010-signing-keys-and-revocation.md) (keys), [ADR 0019](0019-taking-posts-and-media-down.md) (deletes and local blocking), [ADR 0028](0028-replies-let-in-by-the-creator.md) (reply admission), [ADR 0029](0029-vouchers-within-six-steps.md) (optional social evidence), and the [open-feed proposal](../08-open-feed-algorithm.md). A separate follow-on record should settle nested conversation wire details.

## Context

OSP needs a Front Page of public communities, with open reading and joining, community discussions, and locally chosen ranking. A community must survive its founder or an index node going offline. Moderators must be able to deal with spam promptly without becoming a network-wide authority or acquiring the power to alter or delete somebody else's signed post.

A community is **not** a cluster inferred by the Universe map and is **not** an OSP service account. It is a deliberately created, signed public governance space. OSP itself has no override key, reserved moderator identity, or global suspension mechanism.

### Responsibility split

This record follows [ADR 0001](0001-home-node-architecture.md) and [doc 15](../15-feature-responsibility-model.md):

| Layer | Community responsibility |
| --- | --- |
| **App** | Shows Front Page, communities, threads, reports, moderation queues and governance controls; captures the person's intent |
| **Client API** | Carries intent between the paired app and the owner's node, and returns durable state/progress |
| **Home node** | Stores subscriptions, fetches indexes, verifies governance, assembles threads/feeds, applies local policy, creates/signs the owner's permitted actions and keeps working while the phone is off |
| **OSP protocol** | Defines Community IDs, placements, governance/moderation records, nested replies and synchronization between independent nodes |
| **Other nodes/indexes** | Supply signed records and public content; independently verify them; optionally replicate within local limits |

The app does **not** become a community peer, verify governance itself or contact community indexes directly. A button such as Join, Moderate or Approve expresses intent to the owner's node. The node checks current state and authority, performs the cryptographic/network work, and returns the result.

## Decision

### 1. Identity and names

- A community has a stable **Community ID**, derived from its immutable signed founding record (domain-separated hash under a versioned community format). It remains constant across steward or node changes.
- The founding record states the founder's OSP identity, the initial steward set, the initial governance rules, and a unique random nonce. It is signed by the founder with their existing certified OSP signing key.
- Display names such as `c/prospecting`, icons, descriptions and topic labels are *mutable metadata*, not identifiers. Names may collide; every search result and link resolves to an ID and displays additional disambiguating identity information.
- A community never has a shared private signing key held by all stewards. Each steward signs independently using their OSP identity. Machines or community index holders gain no governance power by hosting records.
- Names and branding may change under governance; the permanent ID and founding record never change. Community codes include the ID, not just a name.
- All communities in this phase are **public**. No promise of hidden membership, confidential threads or secret steward votes.

### 2. Founding and steward progression

- One person can create and immediately operate a public community as its founding steward, without recruiting anybody first. The app clearly marks this as **single-steward governance**.
- A proposed steward must explicitly accept with a signed record. No one may appoint an identity without its consent.
- At two active stewards, both must approve governance changes; at three or more, a strict majority of the **entire authorized steward set**, not just online or voting stewards, must approve. For N active stewards, the normal governance quorum is `floor(N/2)+1`; at N=1, it is 1.
- Recommend five independent stewards for an established community. Cap the first protocol version at nine stewards, pending testing. Warn that several identities or keys controlled by one person undermine independence; cryptographic signatures alone cannot prove unique humans.
- The founder gets no extra authority after governance becomes shared. No unilateral founder takeover, dissolution, removal of other stewards or emergency override.
- A governance transition references the immediately prior accepted epoch and explicitly lists the entire next steward roster, thresholds and policies. A departing steward's resignation must be recorded; it does not silently weaken quorum.
- To prevent cycling through a weaker roster, **a community that has reached three stewards may not return to single-steward control through an ordinary governance change**. Reduction below three requires a separately defined and tested recovery rule, or the community remains governance-frozen. This restriction is part of the founding governance version.

### 3. Signed governance and conflicting histories

- Every proposal has a unique ID, action, affected community, source governance epoch hash, proposed new state hash, creation/expiration bounds, and proposer signature.
- Each approval names the exact proposal ID and epoch. Only distinct currently authorized steward identities count. A key revoked for the relevant signing period and invalid/replayed/expired approvals do not count.
- A committed governance epoch contains the proposal, required individual signatures, prior epoch hash, full new state and version. Independent nodes verify the complete chain from the founding record before applying it.
- Approvals cannot be reused to authorize a modified proposal. A change of roster does not retroactively make earlier unauthorized signatures valid.
- **Concurrent competing changes are a safety-critical open implementation problem.** Quorum intersection alone does not resolve equivocation by compromised stewards. Index holders must retain evidence of conflicting valid branches. If a node observes incompatible governance commits for the same predecessor, it freezes *governance mutations* on that community pending a separately reviewed deterministic reconciliation protocol; it must not trust the first or lexicographically smallest branch by default. Existing readable public records and conservative moderation can remain accessible.
- The project must specify how clients detect withheld governance epochs, stale index responses, key compromise and revocation before shipping.

### 4. Public participation and content

- Anyone may read and discover public community threads without joining. Joining is a personal subscription to the community's public index and notification stream; it is not a permission grant or an endorsement.
- Anybody may propose a public post or reply, subject to community rules and bounded anti-spam controls. Joining alone does not guarantee admission. A community cannot force any independent node to index or serve content.
- The original author signs their Note under the existing OSP identity. A separate signed **community placement** refers to the immutable Note ID and Community ID; it does not copy or rewrite the Note. Multiple communities may reference the same Note and host distinct discussion contexts.
- A community discussion requires a signed thread root and nested replies. Every reply refers to its parent and root, and to the community context; this extends, but does not silently change, ADR 0028, which currently allows only root-level replies delivered to the creator. Replies must remain owned and deletable by their authors.
- Creator-controlled reply threads under ADR 0028 continue to work outside communities. Community thread admission is **community-scoped** and checked separately from an author's personal reply policy; the precise coexistence and propagation semantics are a follow-on protocol decision.
- Communities may version posting rules (format, topic, rate limits, links, media, new-participant review, etc.). Enforcement must use the rule version applicable to the action, with clear effective boundaries. Do not apply future rules retroactively without a separately recorded moderation action.
- Do not use raw media downloads, replica count, node performance or mandatory popularity scores as authority to post. ADR 0029 vouchers may inform optional anti-spam policy, but never grant stewardship automatically.

### 5. Moderation — immediate but reviewable

The community controls **inclusion in its own listings and discussions**, not authorship or global deletion. Signed moderation records reference the community ID, target object/identity, steward, reason code, public or restricted explanation, governing rules version, action identifier and any expiry. Reversals reference the prior action; no mutable silent edits.

| Action | Minimum authorization | Lifetime and effect |
| --- | --- | --- |
| Remove/hide a post or reply from the community | One steward | Immediate; remains until reversed; other stewards can review it |
| Reinstate community placement or reply | One steward for routine correction; quorum if reversing a quorum decision | Signed reversal, never erases the old audit record |
| Temporary posting restriction or suspension | One steward | Expires within **24 hours** unless renewed or replaced by an appropriately approved action |
| Permanent community participation ban | Normal governance quorum | Community-scoped only; must state reason and permit an appeal |
| Add/remove steward or change community-wide participation rules | Normal governance quorum | New signed governance epoch |
| Archive/close a community | Supermajority: `ceil(4N/5)` at N>=3, unanimous at N=2, sole steward at N=1 | Signed, independently verifiable; no deletion of underlying authored content |

- A single steward may act immediately; another steward can challenge or undo a routine removal. Repeated reversals should be escalated to a quorum review, not endlessly toggled by competing individual actions.
- **A steward cannot review their own appeal alone.** An appeal should be assigned to other available stewards where possible; if no independent reviewer exists, label that limitation rather than fake an independent review.
- Reports and appeal details may contain private or sensitive information: public audit records carry minimal reason codes and commitments/references rather than raw reports, accusations or identifying details. Restricted case material needs a separate access-controlled protocol design; do not place it in public community indexes.
- Permanent bans require a majority of the full authorized roster, including offline stewards in the denominator. A banned identity may still post elsewhere on OSP. Signed ban lists cannot remove a signed post from unrelated nodes.
- Identity creation is cheap. Membership and signature verification do not prove one-person-one-account. Rate limits, bounded queues, node-local blocklists and simulation against Sybil attacks are required.
- Any member may report content; reporting does not itself grant a publishing or deletion right. Make clear to readers when content is hidden by community policy vs their own node's block rules.
- Nodes may use rules, scripts or machine-learning systems to classify content, prioritize reports and recommend moderation actions under [doc 19](../19-node-assisted-moderation.md). Such automation is **not a steward**, never counts toward quorum and cannot independently authorize permanent bans, governance changes or closure.
- A community may later authorize bounded automatic execution of routine, reversible moderation actions. Automated actions must identify the steward/policy authority under which they were issued, be rate-limited, auditable and reversible. The node's policy/signing boundary remains in control; an assistant never receives unrestricted signing authority.

### 6. Availability and stewardship are independent

- Any willing node may replicate and serve *verified public* community metadata, immutable post references, governance epochs and moderation decisions within opt-in storage/bandwidth caps. **Indexing is not governing.**
- Multiple independent indexes should advertise a community ID, snapshot epoch/root, coverage and freshness. A reader validates signed governance and moderation history and must not mistake an incomplete index for global or exhaustive coverage.
- Community media remains in OSP's existing author/resharer/opt-in host distribution system. An index is not a media CDN and cannot claim ownership merely by caching.
- Design paged, authenticated catch-up; bounded history; offline synchronization; deletion tombstones; and tests for missing, conflicting or malicious indexes. A node may choose not to carry community records or media.
- Community reading should reuse privacy-preserving viewing principles from ADR 0024/0028: avoid revealing stable viewer social identities to index holders. Connection metadata and relay limitations remain; public membership and signed participation are not anonymous.

### 7. Succession, outages and recovery

- An offline steward stays on the roster. No heartbeat, timeout or machine unreachability reduces the approval threshold.
- With a five-steward roster, three remaining available stewards can approve replacements; two cannot rewrite the roster. Ordinary reading can continue while governance is frozen.
- Every community may *pre-authorize* a separate recovery council while healthy. The signed policy identifies independent recovery identities, what they may change (only restoring the authorized roster), an approval threshold, a public notice/challenge period, and safeguards against concurrent ordinary governance.
- **Suggested parameters for security evaluation, not yet accepted wire constants:** three recovery approvals and 14 days of publicly verifiable notice. Recovery must not authorize changing the Community ID, hiding the governance record or retroactively deleting messages.
- Recovery records are verified just like governance records and must be rejected when they violate the last valid precommitted policy. An attacker cannot create or change a recovery council after quorum is lost.
- If both normal quorum and valid precommitted recovery authority are unavailable, governance remains frozen. There is **no OSP master recovery key**. Members can create a successor community under a new ID and link to the frozen one, but cannot claim to have silently taken over its identity.
- Compromised keys and signed forks may make recovery unsafe; secure recovery under these conditions needs explicit tests and potentially an additional ADR before enabling it.

### 8. Front Page UI, search and feeds

- A future app shell may add **Front Page**, but the current Product Alpha keeps Home · Universe · You unchanged until its gate is met.
- Front Page defaults to joined communities and offers New, Hot, Top and Discover. New is chronological. Ranking, count coverage and uncertainty must follow [doc 08](../08-open-feed-algorithm.md), not an invented global vote count or hidden central algorithm.
- Community pages show name, permanent identity (in Advanced), public rules, stewards, moderation transparency, and whether records are up to date. An empty page or offline index must not appear as a verified empty community.
- Creating a community: name, description, initial rules, explicit creation of a single-steward public governance space; immediate posting. Then offer inviting additional stewards, each requiring signed acceptance.
- Governance proposals and routine moderation show human-readable actions and reasons, not keys or endpoint IDs. Steward tools show awaiting approvals, review, appeals, available peers and recovery readiness.
- The existing three-place shell, notifications and Compose flow remain operational throughout implementation.

### 9. Suggested protocol records (names provisional)

- `CommunityCreate`: founding record, nonce, founder identity, initial roster, policy.
- `CommunityMetadata`: versioned public name, icon, description, rules.
- `CommunityProposal`, `CommunityApproval`, `CommunityEpoch`: governance transitions and commitments.
- `CommunityStewardAcceptance`: explicit roster participation.
- `CommunityPlacement`: signed reference to an existing post in a community.
- `CommunityModeration`, `CommunityModerationReversal`, `CommunityAppealOutcome`: scoped actions and review.
- `CommunityRecovery`: precommitted recovery-only roster repair.
- `CommunityIndexSnapshot`: authenticated bounded index root, coverage and freshness; not an authority certificate.

All records must use OSP's exact signed-byte, certified-key and object-ID conventions, be domain-separated/versioned, and have explicit max sizes, rates and replay limits. Do **not** implement these names without first writing and reviewing the serialization, compatibility and transport changes.

### 10. Gates and tests before acceptance as implementation-ready

1. **Identity:** duplicate names resolve to distinct IDs; founder or steward machine replacement leaves ID unchanged; forged identity and altered payloads fail.
2. **Quorum:** one steward can remove spam; only quorum can issue permanent bans or change the roster; offline stewards are counted; replayed or stale approvals cannot pass.
3. **Forks:** delayed, conflicting, censored and equivocated governance epochs never silently grant control; readers visibly detect unresolved conflicts.
4. **Moderation:** action propagation, reversal, audit history, 24-hour temporary suspension expiry and appeals work across independent index holders; no moderator signs another author's Delete.
5. **Succession:** founder departure, two of five online, three of five online, total outage, key revocation, precommitted recovery, attempted forged recovery and no-recovery freeze.
6. **Content:** one signed post can appear in two communities with independent moderation and threaded discussion; author Delete propagates per ADR 0019; no community can rewrite it.
7. **Security:** simulate throwaway identities flooding reports, applications, comments and votes; cap memory, storage, CPU, index reads and Tor requests; moderation queues cannot be cheaply exhausted indefinitely.
8. **Availability:** independent indexes disagree, omit records or go offline; readers reject stale proofs where required and distinguish incomplete coverage; creators and stewards may be offline while cached public discussion remains readable.
9. **Privacy:** anonymous public reading does not reveal stable social IDs; public audit and governance records do not publish private reports, membership assumptions, endpoint IDs or home addresses.
10. **Client:** novice user can create, join, read and post without dealing with keys, quorum terminology or transport jargon; advanced controls show trustworthy diagnostic detail.

## Options rejected

- **Founder keeps a hidden superuser key:** defeats shared governance and succession.
- **Any single moderator can ban permanently:** too much unilateral control.
- **Require quorum for every spam removal:** leaves communities exposed during offline periods.
- **Automatically shrink quorum after downtime:** allows denial-of-service attacks to enable takeover.
- **An OSP-wide moderator or central naming authority:** incompatible with independent communities.
- **Index replicas vote on governance:** hosting capacity does not confer social authority.
- **Assume signed majority alone prevents forks:** unsafe under equivocation and delayed visibility.

## Consequences and unresolved engineering work

This design adds substantial protocol surface: group governance, verifiable indexes, nested threads, anti-abuse limits, appeals, recovery, revocation and fork detection. Existing OSP replies and directories are reusable foundations but not a finished community system. A detailed protocol ADR must settle signature encoding, governance conflict resolution and canonical catch-up before an implementation PR. No private communities or secret moderation inboxes are in scope.

## Build order

The product/app/node implementation plan is in [doc 16](../16-front-page-and-communities.md). The cross-feature responsibility model is [doc 15](../15-feature-responsibility-model.md).

1. After ADR 0030's Product Alpha: public community identity, single-steward creation and signed placement references.
2. Shared steward membership, signed governance epochs, quorum and routine moderation.
3. Community replies/indexing, signed decisions and appeals; independent mirrored indexes.
4. Recovery protocol and full adversarial tests. No wider public launch of shared stewardship before the fork/recovery gates pass.
