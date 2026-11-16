# Front Page and Public Communities

**Status:** Post-Product-Alpha product and implementation plan. Governance direction is proposed in [ADR 0041](decisions/0041-public-communities-and-stewards.md). This document does not make ADR 0041 accepted and does not change the current Home · Universe · You Product Alpha.

Front Page is OSP's public discussion experience: people discover communities, subscribe to them, start discussions and participate in nested conversations. Communities are public in the first version.

The implementation follows [doc 15](15-feature-responsibility-model.md): the app presents and captures intent; the home node maintains the person's community state and works in the background; independent nodes exchange signed community records.

**Navigation is not decided here.** [ADR 0031](decisions/0031-app-shell-home-universe-you.md) still defines Home · Universe · You. Before Front Page becomes another place, decide whether the shell changes or Front Page lives inside an existing place. That choice is parked in [notes/future-ideas.md](../notes/future-ideas.md).

## Product shape

The first Front Page prototype should be simple:

- **Joined:** discussions from communities the person follows;
- **New:** chronological eligible community posts.

**Hot, Top and Discover come later.** [ADR 0038 §2](decisions/0038-likes-and-activity.md#2-what-a-like-is-not) explicitly says Likes do not affect feed order, and changing that needs its own decision record. Front Page must not quietly turn Likes into votes or ranking input.

No view may imply that one node has a complete global count or ranking when it does not. [Doc 08](08-open-feed-algorithm.md) remains the proposal for transparent ranking once a later ADR allows ranking signals.

A community page shows:

- name, icon, description and public rules;
- whether the owner has joined;
- public discussions and nested replies;
- stewards and governance transparency;
- moderation state where it affects what the viewer sees;
- freshness/coverage when the local node does not have a complete current view.

## App work

The Flutter app owns the interaction:

- show the Front Page experience after its navigation placement is decided;
- browse Joined/New in the first prototype;
- open a community and its rules;
- join/leave;
- start a discussion;
- read and write nested replies;
- vote/react when that protocol is decided;
- report content;
- show community removal vs the owner's own local filter clearly;
- create a community;
- invite/accept co-moderators;
- review a simple moderation queue;
- show stale/incomplete community data in plain language.

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
- verified Community IDs and current owner/co-moderator list;
- rules and metadata;
- discovery and synchronization of independent community indexes;
- local community post/reply indexes;
- nested-thread assembly;
- moderation hides/restores and 24-hour mutes;
- outgoing community actions and retry;
- local Front Page candidate generation and ranking;
- personal filtering before presentation;
- bounded cache/storage and community-history retention;
- background refresh while the app is closed;
- optional assisted moderation under [doc 19](19-node-assisted-moderation.md).

When the owner is a steward, the node checks current authority before signing an action. The app asking for an action is not enough to make it valid.

A community index that is missing records cannot cause the node to present "nothing exists" as a verified fact. Coverage/freshness are part of local state.

## Protocol work

[ADR 0041](decisions/0041-public-communities-and-stewards.md) proposes the first community foundation. The first protocol work should settle only what F1-F2 need:

- Community ID and immutable founding record;
- mutable community metadata/rules;
- owner and accepted co-moderators;
- community placement of an existing signed Note;
- reversible community hides/restores and 24-hour mutes;
- basic community index/catch-up;
- nested replies with parent, root and community context;
- author deletes interacting with community indexes;
- rate/size/replay limits.

Shared stewardship, permanent-ban voting, appeals, succession/recovery and conflicting governance histories are later work, parked in [notes/future-ideas.md](../notes/future-ideas.md) until real communities show which problems actually need solving.

The author's original Note stays an ordinary OSP signed object. A community references it instead of rewriting ownership.

The precise relationship between creator-controlled ADR 0028 threads and community-controlled nested discussion needs its own protocol decision before implementation.

## Ranking and voting

The first prototype uses **New**, chronological after eligibility and filtering, and **Joined**.

Hot/Top/Discover are blocked on a later ADR because [ADR 0038 §2](decisions/0038-likes-and-activity.md#2-what-a-like-is-not) says Likes do not affect any feed order. That ADR must be deliberately changed or a different ranking/voting signal designed and tested first.

When ranking is added later, it still runs on the owner's node and must keep moderation, personal blocks and network performance separate from popularity.

## Moderation

ADR 0041's first prototype uses immediate, reversible moderation:

- the owner or an accepted co-moderator may hide/remove from the community immediately;
- they may restore it;
- they may mute someone from the community for at most 24 hours;
- there is no permanent community ban in F1-F2;
- community moderation never signs an author's Delete.

Permanent bans, appeals and shared governance wait until real use shows what is needed.

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

### F3 — Learn from real use

Do not prebuild the full governance system. After F1-F2 have real users, measure:

- how often one owner is not enough;
- how co-moderators disagree;
- whether 24-hour mutes are enough;
- what spam/abuse actually reaches communities;
- which index failures occur in practice.

Only then decide shared stewardship, permanent bans, appeals and succession.

### F4 — Ranking and discovery, after a ranking ADR

Possible later work:

- Hot/Top/Discover;
- community search;
- transparent local ranking;
- abuse/Sybil tests for whichever public signals are chosen.

## Gate

The first Front Page prototype is ready for testing when:

- a person can create/join a public community, read, post and reply from the phone while the node does the network work;
- a community owner can add a co-moderator who explicitly accepts;
- owner/co-moderators can hide, restore and apply a 24-hour mute;
- a moderator cannot delete another person's original signed post;
- a stale/incomplete index is shown as stale/incomplete rather than as an empty community;
- personal filtering does not become a public moderation action;
- resource limits hold under floods;
- the same signed post may appear in multiple communities without changing authorship.
