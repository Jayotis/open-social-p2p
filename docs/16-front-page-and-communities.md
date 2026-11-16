# Front Page and Public Communities

**Status:** Post-Product-Alpha product and implementation plan. Governance direction is proposed in [ADR 0041](decisions/0041-public-communities-and-stewards.md). This document does not make ADR 0041 accepted and does not change the current Home · Universe · You Product Alpha.

Front Page is OSP's public discussion experience: people discover communities, subscribe to them, start discussions and participate in nested conversations. Communities are public in the first version.

The implementation follows [doc 15](15-feature-responsibility-model.md): the app presents and captures intent; the home node maintains the person's community state and works in the background; independent nodes exchange signed community records.

## Product shape

The future Front Page offers at least:

- **Joined:** discussions from communities the person follows;
- **New:** chronological eligible community posts;
- **Hot:** locally ranked recent conversations gaining attention;
- **Top:** locally ranked posts over a selected period;
- **Discover:** public communities and discussions outside the person's subscriptions.

No view may imply that one node has a complete global count or ranking when it does not. [Doc 08](08-open-feed-algorithm.md) governs transparent ranking.

A community page shows:

- name, icon, description and public rules;
- whether the owner has joined;
- public discussions and nested replies;
- stewards and governance transparency;
- moderation state where it affects what the viewer sees;
- freshness/coverage when the local node does not have a complete current view.

## App work

The Flutter app owns the interaction:

- add Front Page to the future navigation;
- browse Joined/New/Hot/Top/Discover;
- open a community and its rules;
- join/leave;
- start a discussion;
- read and write nested replies;
- vote/react when that protocol is decided;
- report content;
- show community removal vs the owner's own local filter clearly;
- create a community;
- invite/accept stewardship;
- review moderation queues and appeals for stewards;
- approve or reject governance proposals;
- show conflicts or stale state in plain language.

Normal screens should not expose hashes, keys, endpoints, Tor or quorum arithmetic. Advanced may show the verifiable details.

The app never contacts community index nodes directly. It requests community state from its own home node.

## Client API work

Exact names are provisional. The client API should express user intent and return durable state, for example:

- `frontPage(mode, cursor)`
- `community(id)`
- `joinCommunity(id)` / `leaveCommunity(id)`
- `communityThread(root, cursor)`
- `createCommunity(draft)`
- `placePost(community, postDraft)`
- `replyInCommunity(parent, draft)`
- `reportCommunityContent(target, reason)`
- `moderationQueue(community)`
- `moderateCommunity(action)`
- `communityProposals(community)`
- `approveCommunityProposal(proposal)`

Long operations return a durable pending state. Reopening the app must not duplicate a post, moderation action or approval.

The API returns enough provenance for the app to explain "removed by community", "hidden by you", "governance conflict" and "community data may be stale" without making Flutter verify protocol records itself.

## Home-node work

The node owns the persistent community experience:

- community subscriptions;
- verified Community IDs and current governance epochs;
- rules and metadata;
- discovery and synchronization of independent community indexes;
- local community post/reply indexes;
- nested-thread assembly;
- moderation state and reversals;
- outgoing community actions and retry;
- local Front Page candidate generation and ranking;
- personal filtering before presentation;
- bounded cache/storage and community-history retention;
- background refresh while the app is closed;
- optional assisted moderation under [doc 19](19-node-assisted-moderation.md).

When the owner is a steward, the node checks current authority before signing an action. The app asking for an action is not enough to make it valid.

A community index that is missing records cannot cause the node to present "nothing exists" as a verified fact. Coverage/freshness are part of local state.

## Protocol work

[ADR 0041](decisions/0041-public-communities-and-stewards.md) proposes the governance foundation. Follow-on protocol work must settle:

- Community ID and immutable founding record;
- mutable community metadata/rules;
- steward acceptance, proposals, approvals and governance epochs;
- community placement of an existing signed Note;
- moderation actions and reversals;
- permanent community bans;
- authenticated community index snapshots and catch-up;
- nested replies with parent, root and community context;
- author deletes interacting with community indexes;
- fork/conflict detection and recovery;
- rate/size/replay limits.

The author's original Note stays an ordinary OSP signed object. A community references it instead of rewriting ownership.

The precise relationship between creator-controlled ADR 0028 threads and community-controlled nested discussion needs its own protocol decision before implementation.

## Ranking and voting

Front Page ranking runs on the owner's node.

**New** is chronological after eligibility and filtering.

**Hot** and **Top** may use signed public interactions and distributed evidence, but must:

- disclose scope/coverage;
- resist repeated or Sybil activity as far as tested;
- keep network performance/hosting capacity separate from social popularity;
- apply moderation and personal blocks before ranking;
- provide a "Why am I seeing this?" explanation;
- allow a chronological view.

A vote/reaction protocol is not invented in this document. Existing Likes may be reused or extended only after deciding what a community score means under incomplete distributed observations.

## Moderation

ADR 0041's rule is immediate, reviewable moderation:

- one steward may hide/remove from the community immediately;
- a one-steward temporary suspension expires within 24 hours unless appropriately renewed;
- permanent bans require the community's normal governance quorum;
- community moderation does not sign an author's Delete;
- appeals and reversals remain visible in the signed history.

The home node may help stewards with local rules/scripts/AI, but automation is not a steward and cannot count toward quorum. See [doc 19](19-node-assisted-moderation.md).

## Privacy

Public communities are not private groups.

Reading should avoid exposing a stable public identity merely because someone opened a thread, following ADR 0024/0028's viewing principles where practical. Posting, replying, steward actions and other signed public participation identify the signer.

Private reports and appeal evidence must not be copied into public community indexes merely because the moderation outcome is public.

Joined-community subscriptions are local state unless a later protocol explicitly makes them public.

## Build order

### F1 — Community identity and local subscriptions

- create/read a public community;
- join/leave locally;
- signed community metadata and rules;
- one-steward creation;
- basic independent index discovery;
- community page in the app.

### F2 — Discussions

- place existing posts into communities;
- community feed;
- nested replies;
- author delete handling;
- bounded pagination/catch-up;
- real phone-to-node journeys.

### F3 — Steward governance and moderation

- multi-steward acceptance and epochs;
- immediate moderation and review;
- permanent-ban quorum;
- appeals;
- governance-conflict state;
- steward app tools.

### F4 — Front Page ranking and discovery

- Joined/New/Hot/Top/Discover;
- transparent local ranking;
- community search;
- missing/stale coverage explanations;
- abuse and Sybil simulations.

### F5 — Recovery and wider release

- precommitted recovery policy;
- adversarial fork/revocation/recovery tests;
- independent index failure tests;
- moderation-flood tests;
- usability testing with people who did not build OSP.

## Gate

Front Page is ready for public testing only when:

- a person can join, read, post and reply from the phone while the node does the network work;
- public discussions remain readable from verified replicas when their creator or a steward is offline;
- one steward can act on spam promptly without gaining permanent unilateral ban power;
- a malicious/stale index cannot silently replace governance;
- personal filtering does not become a public moderation action;
- the app explains incomplete/conflicting state instead of fabricating certainty;
- resource limits hold under floods;
- the same signed post may appear in multiple communities without changing authorship.
