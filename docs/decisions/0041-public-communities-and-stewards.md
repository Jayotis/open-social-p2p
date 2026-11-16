# ADR 0041: Public communities, one owner and accepted co-moderators

**Status:** Proposed (2026-11-16). Post-Product-Alpha. This record deliberately decides only the first community prototype. Shared ownership, permanent community bans, appeals, succession and recovery wait for real community use.

Builds on [ADR 0004](0004-signed-objects-and-ids.md), [ADR 0010](0010-signing-keys-and-revocation.md), [ADR 0019](0019-taking-posts-and-media-down.md), [ADR 0028](0028-replies-let-in-by-the-creator.md) and the app/node split in [ADR 0001](0001-home-node-architecture.md).

The implementation plan is [doc 16](../16-front-page-and-communities.md). Assisted moderation is [doc 19](../19-node-assisted-moderation.md).

## Context

OSP needs public communities for the future Front Page.

The first useful questions are small:

- How does a public community have an identity?
- How does a post appear there without becoming the community's property?
- Who can keep obvious spam out?
- What does the phone do, and what does the owner's node do?

We do **not** yet know whether real communities need elections, recovery councils, permanent community bans or complicated shared governance. Building all of that before anyone uses a community would make the prototype larger without giving us evidence that the rules are right.

So the first version has one owner and optional co-moderators.

If an owner later abuses that power, they can remove a post from that community but cannot delete the author's post from OSP. People can leave and create another community. That limits the harm while we learn.

## Decision

### 1. A community has its own permanent ID

Creating a community makes one signed founding record.

It contains:

- the creator's OSP identity;
- the initial public name;
- the initial rules;
- a random value so two communities with the same name are still different.

The Community ID comes from that founding record and never changes.

A display name such as `c/prospecting` is not globally unique. Search/results must resolve to the Community ID, not the name alone.

Changing a name, icon, description or rules does not change the Community ID.

There is no shared community private key.

### 2. One owner

The person who creates the community is its **owner** in the first prototype.

The owner may:

- change the community's public name, description and rules;
- invite or remove co-moderators;
- hide/restore posts or replies from the community;
- mute someone from posting/replying in that community for at most 24 hours.

The owner cannot:

- edit another person's signed post;
- sign a Delete for another person;
- remove that post from unrelated OSP feeds, communities or nodes;
- gain authority over another person's identity.

This is intentionally simple. Shared ownership comes later if real use needs it.

### 3. Co-moderators must accept

The owner may invite another OSP identity to help moderate.

The invitation does nothing until that person explicitly accepts it with a signed action from their own node.

An accepted co-moderator may:

- hide a post/reply from this community;
- restore one;
- issue a community mute for at most 24 hours.

A co-moderator may **not**:

- add/remove other moderators;
- change community rules;
- transfer ownership;
- create a permanent community ban;
- close/archive the community.

The owner may remove a co-moderator.

### 4. Posts are placed, not copied into ownership

Kim writes a normal OSP post. Kim still owns it.

Putting it in a community creates a signed **community placement** that refers to:

- the Community ID;
- Kim's original post ID;
- who placed it;
- when it was placed.

The community does not rewrite Kim's post.

The same post may be placed in more than one community.

If Kim deletes the original under [ADR 0019](0019-taking-posts-and-media-down.md), cooperating community indexes stop showing it and keep the normal tombstone/evidence needed to stop stale copies coming back.

### 5. Public reading and joining

All communities in this first version are public.

Anyone may read a community.

"Joining" is a local subscription on the person's home node so the community appears in their Front Page/notifications. Joining does not give special posting rights and does not need to become a public membership list.

Private communities are a separate future design.

### 6. Discussions

Community discussion needs replies to replies, which [ADR 0028](0028-replies-let-in-by-the-creator.md) does not support today.

A follow-on protocol decision must define nested community replies.

The first design should preserve these rules:

- each reply is signed by its author;
- a reply names its parent, thread root and Community ID;
- the author can delete their own reply;
- community moderation may hide it from the community but cannot delete it for the author;
- reading a public thread should avoid revealing the reader's stable public identity merely because they opened it, following ADR 0024/0028's existing approach where practical.

### 7. Immediate, reversible moderation

The first prototype has only three community moderation actions:

| Action | Who may do it | Effect |
| --- | --- | --- |
| Hide a post/reply | Owner or accepted co-moderator | Stops listing it in this community |
| Restore a hidden post/reply | Owner or accepted co-moderator | Shows it again |
| Mute an identity | Owner or accepted co-moderator | Stops new community posts/replies for at most 24 hours |

These actions are signed and community-scoped.

A hide is **not** a Delete.

A mute expires automatically. The first prototype has **no permanent community ban**.

If moderators repeatedly disagree and toggle an item, that is useful evidence for the next governance design. Do not prebuild a voting system for it yet.

### 8. Assisted moderation

The owner's or moderator's node may use simple rules, scripts or optional AI to:

- find duplicate/link spam;
- group reports;
- prioritize a queue;
- suggest hide/restore/mute.

The helper is not a moderator.

It does not receive unrestricted signing keys.

By default a person approves a community action. Later, a moderator may explicitly allow bounded automatic actions that are routine and reversible, but that needs testing first.

See [doc 19](../19-node-assisted-moderation.md).

### 9. App, node and network responsibilities

Following [doc 15](../15-feature-responsibility-model.md):

- **App:** shows the community, rules, threads and moderation controls; captures what the person wants.
- **Home node:** stores subscriptions, verifies community records, builds the local community view, signs the owner's actions and keeps syncing while the phone is off.
- **Other nodes/indexes:** exchange signed public community records and may keep copies within their own limits.

The phone does not contact community indexes directly.

Hosting/indexing a community gives no moderation rights.

### 10. Community indexes

The first prototype needs enough indexing to let more than one node keep a public community readable.

An index may provide:

- community metadata/rules;
- placements;
- nested replies;
- moderation hides/restores/mutes.

Records remain signed by the people who created them. An index can omit or be stale, but cannot forge a valid author/moderator action.

The reader's node must show stale/incomplete state as such rather than treating a missing record as proof that nothing exists.

Detailed multi-index conflict/recovery rules should be added only when the simple index has been tested.

## Suggested first protocol records

Names are provisional:

- `CommunityCreate`
- `CommunityMetadata`
- `CommunityModeratorInvite`
- `CommunityModeratorAccept`
- `CommunityModeratorRemove`
- `CommunityPlacement`
- `CommunityModeration` (hide/restore/mute)
- a later nested-reply record/extension
- a bounded community index snapshot/page

Exact serialization, limits and transport still need review before implementation.

## First build

### F1 — identity and subscriptions

- create a public community;
- display it on another node;
- join/leave locally;
- owner changes public metadata/rules;
- invite/accept/remove a co-moderator.

### F2 — discussion and moderation

- place an existing post;
- nested replies;
- hide/restore;
- 24-hour mute;
- author Delete;
- simple replicated index/catch-up;
- phone -> own node -> other nodes test.

Then stop and learn from use before designing shared governance.

## Gate

The first prototype passes when:

- Alice creates a community and another node can verify it;
- Alice invites Bob as co-moderator and Bob must accept before he can moderate;
- Kim's ordinary signed post can be placed in the community without changing ownership;
- Alice or Bob can hide and restore it but cannot delete it for Kim;
- Alice or Bob can mute Jimmy for 24 hours and the mute expires;
- Kim deletes her original and the community stops showing it;
- another node can read the public community from signed records while Alice's node is offline;
- stale/incomplete index data is shown honestly;
- the phone only talks to its own node through the whole journey.

## Later

Only after real communities exist, decide whether OSP needs:

- several stewards sharing ownership;
- majority-approved permanent bans;
- appeals;
- ownership succession;
- recovery when owners/stewards disappear;
- handling of two conflicting signed governance histories;
- close/archive votes.

They are listed in [doc 16's Later section](../16-front-page-and-communities.md#later), and none is a prerequisite for F1–F2.

## Options rejected for the first prototype

- **Full multi-steward governance now:** too much untested machinery before any community exists.
- **Community moderator can Delete another author's post:** violates OSP content ownership.
- **Hosting/indexing grants moderation authority:** storage capacity is not social authority.
- **Automatic permanent bans from scripts/AI:** too much authority for an assistant.
- **A project-wide OSP moderator/master key:** creates the central control OSP is avoiding.
