# ADR 0028: Replies enter through a validated social path

**Status:** Proposed (2026-11-11). This is a coding contract, not only a direction. The thresholds below are starting hypotheses fixed before measurement. Build the substrate and the simulation from them; do not enable automatic six-degree admission by default until the simulation gate in this record is met and the maintainer accepts the result.

Adds public replies/comments to the native social protocol before federation. Builds on [ADR 0013](0013-trusted-followers.md), [ADR 0015](0015-nodes-meet-first.md), [ADR 0016](0016-machine-acquaintances.md), [ADR 0018](0018-earned-direct-connections.md), [ADR 0022](0022-lists-and-introductions-are-earned.md), [ADR 0024](0024-watching-found-posts-without-your-identity.md), [ADR 0019](0019-taking-posts-and-media-down.md), [doc 10](../10-exploration-divergence-and-scouts.md) and [doc 12](../12-peer-reciprocity-and-recognition.md).

This record adopts **Hunter** as the product term for the role previously called **Scout**. Active product/proposal docs on this branch use Hunter; historical reviews and accepted ADR wording need not be rewritten merely for terminology. New code and new records use `hunter` / `Hunter`.

## Context

A public post needs conversation, but an unrestricted comment endpoint would let every new identity on the Internet send content to a creator's home node. Following cannot be the gate: following is deliberately open, and identities are cheap.

The opposite rule, "only people the creator already follows or trusts may comment", is too closed. A creator should be able to hear from an established creator or Hunter outside their immediate circle. The useful person may be several social steps away: a friend knows someone who knows someone who knows them.

OSP already separates several kinds of evidence:

- **Human trust:** the owner follows someone or explicitly marks a trusted follower (ADR 0013).
- **First-hand network work:** a node credits only work it checked itself (ADRs 0018 and 0022).
- **Public social recognition:** creator and Hunter recognition in doc 12 comes from signed public reshares and is computed by the viewer's own node, not claimed by the subject and not assigned centrally.
- **Private reading:** ADR 0024 lets a person fetch public media through a viewing endpoint without presenting their public identity to the creator.

Replies use all four ideas without collapsing them into one score.

The rules are:

> **Reading is open and private. Speaking is public and attributable. The creator's own node decides admission from evidence it can verify.**

There is no global influencer list, no central reputation service, and no universal "approved commenter" badge.

## Definitions

**Root post:** the original signed `Payload::Note` being discussed. A reshare (`Announce`) is never a thread root.

**Reply:** a signed `Payload::Note` whose existing `inReplyTo` field names the root post.

**Creator:** the identity that signed the root post.

**Hunter:** the discovery role previously called Scout: a person whose public reshares repeatedly find work early that later receives independent public reshares.

**Anchor:** a human relationship local to the creator's node. In v1, anchors are identities the owner follows and identities the owner has explicitly trusted under ADR 0013. Network peers, direct contacts, Known machines and worker-qualified nodes are **not** anchors merely because of their network state.

**Public social edge:** evidence that two public identities had a public curation relationship: a valid signed `Announce` by one identity of a post by the other. For path finding only, that edge is treated as undirected. Direction still matters for Creator/Hunter recognition.

**Social distance:** the number of edges from the creator identity to the commenter. A local follows/trusted relationship from the creator to an anchor is one edge. Each public social edge is one edge. Maximum distance for automatic validation is 6.

**Independent paths:** two paths from creator to commenter that share no intermediate public identity. They may share only the creator and commenter. Therefore their first hop from the creator is necessarily different.

**Recognition level:** Creator level or Hunter level, 0 through 5, computed locally from signed public activity below. A person never supplies their own level.

## Decision

### Replies are existing Notes

Do not add a comment content type.

A v1 reply is:

```text
Payload::Note(Note {
    actor: commenter,
    content,
    published,
    attachment: [],
    in_reply_to: Some(root_post),
})
```

Rules:

- `in_reply_to` names the canonical root `Note`, never an `Announce`.
- If the user presses Reply on a reshare, resolve it to the original Note first.
- Reply content is UTF-8 text, non-empty after trimming, at most **4,000 bytes**.
- Attachments are forbidden in v1.
- Replies to replies are forbidden in v1: the target must be a top-level Note with `in_reply_to == None`.
- The reply is signed by the commenter's normal public profile identity. Machine identities, viewing keys, hosting keys and directory keys cannot author replies.

The existing `Note` format remains suitable for later ActivityPub translation.

### Replies do not go into the ordinary outbox

Current clients parse `inReplyTo` but do not use it to separate replies from feed posts. If replies were appended to the ordinary outbox now, current clients would show them as top-level posts.

Therefore v1 replies have separate delivery and persistence.

They do not appear as:

- ordinary Following-feed entries;
- directory "latest posts";
- ordinary post search results;
- reshares;
- hosting commitments.

A later profile "Replies" view can deliberately expose them without changing the signed object.

## Post reply policy

Every root post has one of three policies:

| Policy | Admission |
| --- | --- |
| `off` | No new replies |
| `circle` | Explicitly allowed commenters, identities the creator follows, and trusted followers |
| `network` | `circle`, first-hand worker admission, and the recognition/path validation below |

The default for a new public post is **`network`**.

Before automatic recognition passes this ADR's simulation gate, `network` still accepts circle and first-hand-worker paths, but recognition/path candidates that would otherwise auto-publish are placed in **Request** instead. The feature flag starts false:

```toml
[comments]
recognition_auto = false
default_policy = "network"
```

The maintainer may change those defaults after the gate; that is recorded in this ADR before merge.

## Transport

### Submitting a reply: public social endpoint

Submission is an intentional public act and goes to the creator's normal authenticated social endpoint, currently `osp/social/2`, over whatever transport that endpoint uses (Tor by default in M4). It does **not** require a direct-IP connection.

Add:

```rust
SocialRequest::SubmitReply {
    reply: SignedObject,
    profile: SignedObject,
}
```

The remote endpoint ID must be one of the nodes named by the supplied signed profile. The profile actor must equal the reply actor.

Add:

```rust
SocialResponse::ReplyResult {
    state: ReplySubmissionState,
}

enum ReplySubmissionState {
    Published,
    AwaitingApproval,
    NotPublished,
    Closed,
    Deleted,
}
```

`Blocked` is deliberately not a wire state. A blocked identity receives `NotPublished`, as does a rejected identity, so the protocol is not a block-list oracle.

Submitting the same signed reply ID again is idempotent and returns its current externally visible state.

### Deleting one's reply

Add:

```rust
SocialRequest::DeleteReply {
    delete: SignedObject,
    profile: SignedObject,
}
```

The `Delete` must be signed by the actor of the stored reply, name that reply object ID, and the supplied current profile must belong to that actor and name the connecting node.

The creator's node removes an accepted or pending reply, stores its tombstone, and returns `Ok`. Repeating the same valid Delete is idempotent.

The phone/CLI may continue to expose one user action called Delete; the home node extends its existing delete handling so an owned outgoing reply can use this path instead of the ordinary outbox path.

### Reading a thread: viewing endpoint

Reading a public thread must not identify the reader to the creator.

Add a protocol on the creator's relay-only media endpoint:

```text
osp/replies/0
```

Use a viewing endpoint as ADR 0024 does.

Request:

```rust
struct RepliesRequest {
    post: ObjectId,
    after: u64,
    limit: u16,
}
```

Response:

```rust
enum RepliesResponse {
    Replies {
        entries: Vec<ReplyEntry>,
        latest: u64,
        open: bool,
    },
    Deleted {
        delete: SignedObject,
    },
    Error {
        message: String,
    },
}

struct ReplyEntry {
    seq: u64,
    reply: SignedObject,
    profile: SignedObject,
}
```

Rules:

- Entries are accepted, non-hidden replies only.
- Entries are returned oldest first where `seq > after`.
- `limit` is clamped to **50**.
- Encoded response is at most **256 KiB**; stop before the next entry would cross it.
- Request frame is at most **4 KiB**.
- At most **16** reply-read requests are served at once across the media endpoint.
- The endpoint does not perform recognition, graph searches or network lookups.
- If the root post has been deleted, return the creator's signed `Delete`.
- A reader's public profile ID is never required or logged merely for reading.

## Hard validation of a submitted reply

Run these checks before social admission:

1. `reply.verify()` succeeds and yields `Payload::Note`.
2. The reply is not signed by a key this node knows the identity revoked.
3. The supplied profile verifies as `Payload::Person`.
4. Profile actor == reply actor.
5. The connecting endpoint is listed by that profile.
6. `in_reply_to == Some(root_post)`.
7. The root object exists in this creator's own outbox, verifies as a top-level Note, and its actor is this node's identity.
8. The root is not tombstoned.
9. Reply attachments are empty.
10. Reply content, after trimming, is not empty and is at most 4,000 bytes.
11. The per-node, per-actor and per-post bounds below are not exceeded.

A failure is `NotPublished` or the existing protocol `Error` for malformed/oversized input. It never enters recognition.

Store the newest valid profile supplied for an accepted/requested commenter, subject to the same profile freshness and key-revocation rules as other profiles.

## Admission states

The creator's node computes:

```text
Allow
Request
Reject
Block
```

Wire mapping:

| Local result | Wire result |
| --- | --- |
| Allow | Published |
| Request | AwaitingApproval |
| Reject | NotPublished |
| Block | NotPublished |
| replies closed | Closed |
| root deleted | Deleted |

### Deterministic admission order

Implement the decision in this order. A later test may tune constants, but code must not reorder the classes silently.

```text
validate_reply(commenter, root):

1. hard protocol checks
   fail -> Reject

2. root policy == off
   -> Reject/Closed

3. commenter is explicitly blocked
   -> Block

4. commenter is explicitly allowed for comments
   -> Allow

5. creator follows commenter
   -> Allow

6. commenter is a trusted follower
   -> Allow

7. root policy == circle
   -> Reject

8. creator's own node has worker qualification for commenter
   -> Allow

9. compute creator_level, hunter_level
   recognition = max(creator_level, hunter_level)

10. find shortest local social path d, maximum 6

11. automatic recognition admission, only when comments.recognition_auto == true:
    d <= 2 and recognition >= 1
        -> Allow
    d <= 4 and recognition >= 2
        -> Allow
    d <= 6 and recognition >= 3
        and two independent paths exist
        -> Allow

12. request rather than discard when:
    d <= 2
        -> Request
    d <= 4 and recognition >= 1
        -> Request
    d <= 6 and recognition >= 2
        -> Request
    no path and recognition >= 3
        -> Request

13. otherwise
    -> Reject
```

When `recognition_auto == false`, every case in step 11 falls through to Request.

A graph search that hits a resource bound is **inconclusive**, never proof. It may yield Request if the recognition rule above qualifies; it must not yield Allow.

## Direct human relationships

The following qualify under both `circle` and `network`:

- the creator follows the commenter;
- the commenter is an ADR 0013 trusted follower;
- the creator explicitly chose "Allow future comments" for that identity.

Merely following the creator is not enough. Following is open.

An explicit comment permission is for comments only. It does not create trusted-follower status, direct-connection permission, machine standing, private-post access or a social-graph anchor.

## First-hand worker admission

Under `network`, a commenter also qualifies if the creator's own node holds first-hand worker standing for a node in that commenter's current signed profile at ADR 0018/0022's layer-2 bar:

- checked work on at least **3 different days in the last 30**;
- no bad data;
- still above the existing faded threshold for keeping that level.

Only this creator's own record counts.

A Known machine does not qualify. Another node's report does not qualify. A public creator/Hunter level does not turn into worker standing.

Until ADR 0018's work layers are represented in the running node, this test simply returns false. Do not build a second reply-only imitation of worker standing.

Worker admission allows the worker identity itself to comment. It does **not** make that identity an anchor for validating other commenters.

## The local social graph

The six-degree check must use evidence the node can actually prove. OSP does not publish everyone's follow graph, so v1 does not pretend that it can traverse private follows.

### Root and private anchor edges

The graph root is the creator's own public identity at distance 0.

For path search only, add one local edge from the creator to:

- every identity the creator follows;
- every identity the creator has explicitly trusted under ADR 0013.

These private edges never leave the creator's node.

Explicit comment permission, worker standing, machine acquaintance, direct contact and directory co-membership do not create anchor edges.

### Public social edges

Every valid signed `Announce` observed by the node creates evidence between:

- resharer identity;
- original post author identity.

For **path finding**, treat that evidence as an undirected edge. This says only that a public curation relationship was observed; it does not claim friendship or reciprocal trust.

For **recognition**, keep the real direction: resharer -> creator.

An edge is current for **180 days from the node's local first-seen time**, not from a timestamp supplied by the actor.

When the last stored Announce supporting an edge is deleted/tombstoned or ages past 180 days, that edge no longer participates in new paths.

### Persistence for public evidence

Add a migration:

```sql
CREATE TABLE social_evidence (
    announce     TEXT PRIMARY KEY,
    actor        TEXT NOT NULL,
    target_actor TEXT NOT NULL,
    post         TEXT NOT NULL,
    hosting      INTEGER NOT NULL,
    seen_ms      INTEGER NOT NULL
);
CREATE INDEX social_evidence_actor ON social_evidence(actor, seen_ms);
CREATE INDEX social_evidence_target ON social_evidence(target_actor, seen_ms);
CREATE INDEX social_evidence_post ON social_evidence(post, seen_ms);
```

Populate it whenever a verified `Announce` is stored and its referenced original Note is known.

Remove its row when that Announce is removed by a valid Delete. Prune rows older than 180 days.

Cap it at **100,000 rows**. If inserting would exceed that, delete the oldest rows first. Recognition/path validation using a pruned graph can become inconclusive but never grants admission from missing evidence.

Backfill this derived table from stored valid Announce objects at migration/open, as the search index is rebuilt from stored objects.

## Path search

Search the local graph only. A submitted comment triggers **no Tor fan-out and no recursive network query** in v1.

Use bounded breadth-first search:

- maximum distance: **6**;
- maximum public identities visited for one validation: **10,000**;
- maximum evidence edges examined: **50,000**;
- stop once the shortest path is known, except when a second independent path is required.

If a bound is hit before the answer is known, record `inconclusive`.

### Two independent paths

For distance 5–6 automatic admission, require two vertex-independent paths.

Implementation definition:

1. Find one shortest path P1 of at most 6 edges.
2. Remove every intermediate public identity in P1 (keep creator and commenter).
3. Search again to depth 6.
4. A path found is P2 and counts as independent.

This deliberately requires a different first-hop anchor/relationship and prevents one intermediate hub from being counted twice.

Do not count two machine keys, two posts or two reshares by the same public identity as independent paths.

## Creator recognition

Recognition is computed by the creator's node from signed public activity it already stores. No network request is made during submission.

Window: **90 days by local first-seen time**.

For candidate identity C:

1. Find valid stored `Announce` evidence whose `target_actor == C`.
2. Exclude C resharing C.
3. Count each distinct resharer identity once across all C's posts.
4. That identity contributes **1.0**.
5. If at least one of that identity's qualifying reshares of C had `hosting == true`, it contributes **1.5** instead.

`creator_score` is the sum.

Starting levels:

| Creator level | creator_score |
| ---: | ---: |
| 0 | < 3 |
| 1 | >= 3 |
| 2 | >= 10 |
| 3 | >= 30 |
| 4 | >= 100 |
| 5 | >= 300 |

These thresholds are deliberately recorded before simulation. They are recognition bands, not follower counts.

## Hunter recognition

Use **Hunter** in new code.

Window: **90 days by local first-seen time**.

An eligible hunt is a valid stored `Announce` by candidate H of somebody else's root post.

For each eligible hunt:

- Let `t` be when this node first saw H's Announce.
- It is a **hit** when at least **3 other distinct identities**, excluding H and the root creator, are first seen resharing that same root post after `t` and no later than **7 days** after `t`.
- Later copies by the same identity count once.
- Claimed `published` times do not establish "early"; local first-seen time does.

Let:

- `hunts` = eligible hunts in the 90-day window;
- `hits` = those that became hits;
- `hit_rate = hits / hunts`.

Starting levels:

| Hunter level | Minimum hunts | Minimum hit rate |
| ---: | ---: | ---: |
| 0 | — | below level 1 |
| 1 | 5 | 20% |
| 2 | 10 | 30% |
| 3 | 20 | 40% |
| 4 | 40 | 50% |
| 5 | 80 | 60% |

The minimum-hunt column is a confidence floor, not a reward for volume. Beyond the floor, the level is driven by hit rate.

A person may have both levels. Comment admission uses:

```text
recognition = max(creator_level, hunter_level)
```

Do not add the levels together.

## Recognition cache

Recognition is derived data.

Cache per actor for at most **1 hour** and invalidate/recompute when relevant `social_evidence` changes if convenient. The signed Announce objects and local seen times are the source of truth.

Expose the evidence used to tests/debugging, not as a global public score:

```text
creator_score
creator_level
hunter_hunts
hunter_hits
hunter_level
computed_at
```

The future profile UI labels levels **"as seen from your network"**. No leaderboard is created.

## Why recognition may admit comments but buys no network service

Doc 12 rejected Creator/Hunter recognition buying bandwidth priority because priority is zero-sum and virality should not buy network resources.

That still stands.

This ADR gives recognition one narrow use: whether a public identity may participate automatically in the creator's public conversation.

Recognition does not grant:

- private content;
- trusted-follower status;
- direct-IP access;
- worker standing;
- bandwidth priority;
- source preference;
- hosting preference;
- feed ranking.

A comment later hidden or blocked also does not automatically lower machine/network standing. Content moderation and transport integrity are separate records.

## Requests and creator moderation

A Request is stored privately and is not returned by `osp/replies/0`.

The creator can:

- **Publish once:** accept this reply only.
- **Publish and allow future:** accept it and put the actor in comment permission `allow`.
- **Decline:** remove the request; future replies are evaluated normally.
- **Block:** remove the request and set comment permission `block`.

For an accepted reply, the creator can later:

- **Hide from this thread:** stop serving it from the thread;
- **Unhide:** serve it again, if the reply/root are not deleted;
- **Block commenter:** hide their currently accepted replies on this creator's threads and refuse future submissions.

Hiding is not deletion. The creator never signs a Delete as another person.

A comment permission does not become a social-graph anchor.

Pending requests expire after **30 days** and are removed.

## Persistence

Add migrations equivalent to these tables (column names may follow existing style, but the information and uniqueness constraints are required):

```sql
CREATE TABLE reply_threads (
    post       TEXT PRIMARY KEY,
    policy     TEXT NOT NULL,
    updated_ms INTEGER NOT NULL
);

CREATE TABLE replies (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    id          TEXT NOT NULL UNIQUE,
    post        TEXT NOT NULL,
    actor       TEXT NOT NULL,
    signed      TEXT NOT NULL,
    profile     TEXT NOT NULL,
    state       TEXT NOT NULL, -- accepted | hidden | request
    received_ms INTEGER NOT NULL
);
CREATE INDEX replies_by_post ON replies(post, state, seq);
CREATE INDEX replies_by_actor ON replies(actor, received_ms);

CREATE TABLE reply_permissions (
    actor      TEXT PRIMARY KEY,
    state      TEXT NOT NULL, -- allow | block
    updated_ms INTEGER NOT NULL
);

CREATE TABLE outgoing_replies (
    id          TEXT PRIMARY KEY,
    post        TEXT NOT NULL,
    post_actor  TEXT NOT NULL,
    target_addr TEXT NOT NULL,
    signed      TEXT NOT NULL,
    profile     TEXT NOT NULL,
    state       TEXT NOT NULL, -- pending | published | request | not_published | closed | deleted
    tries       INTEGER NOT NULL,
    next_try_ms INTEGER,
    updated_ms  INTEGER NOT NULL
);

CREATE TABLE reply_tombstones (
    reply       TEXT PRIMARY KEY,
    actor       TEXT NOT NULL,
    delete_id   TEXT NOT NULL UNIQUE,
    signed      TEXT NOT NULL,
    received_ms INTEGER NOT NULL
);
```

Use a later current profile when retrying if the node has one; `target_addr` is the last usable destination so a temporary lack of directory/follow data does not discard the reply.

The creator's own root-post Delete is sufficient to close and remove the served thread. After that, rows in `replies` for that post may be reclaimed; the root tombstone prevents the thread returning. A reply author's own Delete is kept in `reply_tombstones` so an old reply cannot be accepted again while the root still exists.

## Bounds

Starting values:

| Resource | Limit |
| --- | ---: |
| Reply text | 4,000 bytes |
| Attachments | 0 |
| Replies returned/page | 50 |
| Encoded reply page | 256 KiB |
| Accepted + hidden replies/post | 2,000 |
| Requests waiting total | 100 |
| Requests waiting from one actor | 3 |
| New SubmitReply requests from one remote node/hour | 20 |
| New replies from one actor to one post/day | 20 |
| Reply-read requests served at once | 16 |
| Social evidence retained | 100,000 rows |
| Validation identities visited | 10,000 |
| Validation edges examined | 50,000 |
| Validation path depth | 6 |

When the request inbox is full, a new candidate that would become Request gets `NotPublished`; do not evict an older request to make room for a flood.

When a thread has 2,000 accepted/hidden replies, new automatic admissions become Request if there is request capacity; otherwise `NotPublished`. Existing replies are not silently discarded to make room.

These values are starting limits and may change after measurement, by an ADR amendment.

## Offline creator and retry

Creating a reply succeeds locally once it is signed and stored in `outgoing_replies`, even if the creator cannot be reached.

Retry schedule after a failed connection/timeout:

```text
1 minute
5 minutes
30 minutes
2 hours
12 hours
24 hours
then every 24 hours
```

Stop after **30 days** unless the user explicitly retries.

`osp sync` and an app manual retry may trigger an immediate attempt without resetting the age.

Definitive `Published`, `AwaitingApproval`, `NotPublished`, `Closed` or `Deleted` stops automatic submission retries.

A restart must preserve the queue and must not produce duplicate accepted replies: the signed reply ID is the idempotency key.

## Deletion behavior

### Commenter deletes their reply

The commenter's node signs the existing `Payload::Delete` naming the reply.

Locally it stores that tombstone immediately and stops presenting the outgoing reply as live.

It sends `DeleteReply` to the root creator, with the commenter's current profile, and retries on the same schedule if the creator is offline.

The creator verifies:

- the Delete signature;
- Delete actor == stored reply actor;
- Delete object == stored reply ID;
- supplied profile actor == Delete actor;
- connecting node is in the profile.

Then it drops accepted/hidden/request state for that reply and stores the reply tombstone.

An old copy of the reply cannot resurrect it.

### Creator deletes the root post

No new reply is accepted.

`osp/replies/0` returns the root creator's signed Delete.

The creator may reclaim all reply rows held only for that thread. This does not forge Deletes for the commenters' objects.

## Client RPC

Add client requests/responses equivalent to:

```rust
ClientRequest::Reply {
    post: ObjectId,
    content: String,
}

ClientRequest::Replies {
    post: ObjectId,
    after: u64,
    limit: u16,
}

ClientRequest::ReplyRequests

ClientRequest::AnswerReplyRequest {
    reply: ObjectId,
    decision: ReplyDecision,
}

enum ReplyDecision {
    PublishOnce,
    PublishAndAllow,
    Decline,
    Block,
}

ClientRequest::SetReplyPolicy {
    post: ObjectId,
    policy: ReplyPolicy,
}

enum ReplyPolicy {
    Off,
    Circle,
    Network,
}

ClientRequest::HideReply {
    reply: ObjectId,
    hidden: bool,
}

ClientRequest::SetCommentPermission {
    actor: PublicKey,
    permission: Option<CommentPermission>,
}

enum CommentPermission {
    Allow,
    Block,
}
```

Extend existing `Delete { id }` so an owned outgoing reply ID routes to `DeleteReply`.

Responses must expose:

- outgoing reply ID;
- local delivery state;
- thread page;
- pending requests with commenter profile and recognition/path explanation;
- current policy.

The UI must not need to parse protocol enums or inspect SQLite.

## Explanation returned to the creator UI

For every Request, store/compute an explanation containing only local facts:

```text
creator_level
hunter_level
shortest_path_length: Option<u8>
independent_paths: u8   // 0, 1 or 2+
worker_qualified: bool
followed: bool
trusted: bool
```

The app may render, for example:

- "Creator level 3 as seen from your network; 4 social steps away."
- "Hunter level 2; one path through your network."
- "You follow this person."
- "Trusted follower."
- "Node has done checked work for yours on 3 days."

Do not expose the intermediate path identities in the normal UI. The graph is for validation, not a relationship-tracing product.

Debug/test output may show paths.

## CLI

Add, or provide equivalent developer commands:

```text
osp reply <post-id> <text>
osp replies <post-id>
osp reply-requests
osp reply-allow <reply-id>
osp reply-decline <reply-id>
osp reply-block <reply-id>
osp replies-policy <post-id> off|circle|network
osp recognition <actor>
osp social-path <actor>
```

`osp recognition` and `osp social-path` are local diagnostic commands. They make no network requests.

## Hunter terminology

New implementation names use `Hunter`, `hunter_level`, `hunter_hits`, etc.

Do not add new identifiers named `Scout`.

Active proposal/product docs use Hunter. Historical reviews and accepted or superseded ADR text may retain Scout so their original reasoning remains readable.

## Deliberately not in v1

### Manual comment referrals

Earlier discussion considered a trusted follower forwarding a stranger's exact comment to the creator. That is not part of the first build.

A stranger needs a way to ask the sponsor to do that. Giving every stranger an unrestricted referral-request inbox on the sponsor merely moves the unsolicited-write problem one hop away.

The six-degree validation above covers the important-outside-creator/Hunter case without creating that recursive ingress.

A later ADR may add deliberate referrals with their own admission rule.

### Also out of scope

- replies to replies / nested threads;
- media attachments in replies;
- likes, dislikes or emoji reactions;
- @mentions;
- push notifications;
- comment editing;
- comment ranking;
- using comment activity in feed ranking;
- replicated thread hosting;
- ActivityPub bridge work;
- active network crawling to validate a comment;
- global creator/Hunter leaderboards.

## Build order

### Step 0 — simulation fixture and recognition unit code

Before UI work, implement the pure recognition and path functions against deterministic fixtures.

Required pure interfaces, or equivalents:

```text
creator_recognition(actor, evidence, now) -> Recognition
hunter_recognition(actor, evidence, now) -> Recognition
social_path(actor, anchors, evidence, limits) -> PathResult
validate_reply(context) -> Admission
```

They must not perform network I/O.

### Step 1 — reply substrate

Build:

- separate reply persistence;
- `SubmitReply`;
- `DeleteReply`;
- outgoing retry;
- `off/circle/network`;
- direct followed/trusted/explicit permission admission;
- creator moderation;
- `osp/replies/0` viewing retrieval;
- CLI.

At this step recognition auto-admission remains off.

### Step 2 — recognition evidence and Hunter rename

Build `social_evidence`, backfill/pruning, Creator/Hunter calculations and diagnostics.

Do not use recognition to auto-publish yet.

### Step 3 — six-degree validation

Build bounded local path search, independent-path test, Request explanations, and the admission function exactly as above.

With `recognition_auto = false`, qualifying recognition goes to Request.

### Step 4 — simulation gate

Add an `osp-sim` scenario for reply admission.

Automatic recognition stays off unless the gate passes and the maintainer accepts the result.

### Step 5 — app

Add:

- Comments/Reply action on a post;
- thread screen;
- composer;
- outgoing pending/request state;
- creator request inbox;
- Publish once / Allow future / Decline / Block;
- Hide from thread;
- per-post Off/Circle/Network control.

No nested indentation in v1.

## Automated validation

At minimum:

1. A reply with no `inReplyTo` is not accepted as a reply.
2. A reply to an `Announce` is refused; client resolves a reshare to the root Note.
3. A reply to another reply is refused.
4. Attachments in a v1 reply are refused.
5. A connecting node not named in the commenter profile is refused.
6. A revoked signing key is refused.
7. Creator-followed commenter -> Allow.
8. Trusted follower -> Allow.
9. Person who merely follows the creator -> no special admission.
10. Explicit comment Allow -> Allow.
11. Explicit Block -> Block/NotPublished.
12. Circle policy refuses recognition/worker-only commenters.
13. Worker layer-2 qualification -> Allow once the shared worker implementation exists.
14. Known-machine status alone -> no admission.
15. Creator level thresholds exactly match the table.
16. Hunter level thresholds exactly match the table.
17. Hunter hit timing uses local first-seen time, not claimed publish time.
18. Self-reshares do not count toward Creator recognition.
19. One resharer counts once for Creator score; hosting changes its weight from 1.0 to 1.5, not once per post.
20. Public reshare edges are undirected for path search but directed for recognition.
21. Shortest path is bounded at six.
22. Two paths sharing an intermediate identity are not independent.
23. Two vertex-independent paths are independent.
24. d <= 2 + recognition >= 1 -> Allow only when recognition_auto is on; Request when off.
25. d <= 4 + recognition >= 2 -> same.
26. d <= 6 + recognition >= 3 + two independent paths -> same.
27. d 5–6 with only one path never auto-allows.
28. High recognition with no path -> Request, never automatic Allow.
29. A validation hitting visit/edge bounds never auto-allows.
30. Accepted reply appears in thread but not ordinary Feed/Search/directory listings.
31. Hidden reply disappears from thread without a forged Delete.
32. Commenter Delete removes accepted/requested reply and prevents replay.
33. Root Delete closes the thread and `osp/replies/0` returns the signed Delete.
34. Offline creator keeps outgoing reply pending; restart preserves it; later delivery occurs once.
35. Repeated SubmitReply is idempotent.
36. Reply-page count and byte limits hold.
37. Submit/rate/request/thread/storage limits hold under a flood.
38. Reading through the viewing endpoint sends no packet from the reader's public, media or direct endpoint in the simulated-network privacy test, following ADR 0024's style.

## Simulation gate for automatic recognition

Thresholds above are fixed before the run.

Create a deterministic scenario with at least:

- 500 honest public identities;
- at least 20 creators with a range of true reshare breadth;
- at least 20 Hunters with a range of hit rates;
- 10 communities with cross-community public reshare edges;
- candidates at distances 1 through 6;
- 200 Sybil identities controlled by one operator;
- reciprocal Sybil reshare rings;
- one Sybil cluster attached through only one honest intermediate;
- one Sybil cluster that spends real public reshare activity but has no second independent path;
- established creators/Hunters outside the creator's immediate community.

Report separately with recognition_auto hypothetically on:

1. honest Creator/Hunter candidates automatically allowed, by distance and level;
2. honest candidates sent to Request;
3. honest candidates rejected;
4. Sybil candidates automatically allowed;
5. Sybil candidates sent to Request;
6. graph work per validation: vertices and edges examined;
7. recognition evidence rows and database size;
8. effect of requiring the second independent path at distance 5–6.

**Starting marks:**

- >= 90% of honest level-3+ Creator/Hunter candidates at distance <= 4 are Allow or Request, not Reject.
- >= 80% of honest level-3+ Creator/Hunter candidates at distance 5–6 with two genuinely separate community paths are Allow.
- 0 candidates in a completely unanchored Sybil cluster are automatic Allow.
- <= 1% of Sybil candidates attached through only one honest bridge are automatic Allow.
- No validation exceeds the explicit visit/edge bounds.
- Increasing Sybil keys without adding new honest-anchor paths does not increase the number of independent paths.

If those marks fail, keep `recognition_auto = false`, record the results and amend the thresholds/rules before enabling it. Do not reinterpret the marks after the run.

## Real-node gate

After automated tests and the simulation:

Use at least four public identities on separate test nodes:

- Kim: root creator;
- Alice: Kim follows or trusts Alice;
- Jimmy: not followed/trusted by Kim, but constructed signed history gives him qualifying Creator or Hunter recognition and a path through the test graph;
- Carol: unrelated stranger without qualification.

Demonstrate:

1. Alice comments and publishes directly.
2. Jimmy is accepted or placed in Request exactly as the admission table predicts.
3. Carol does not auto-publish.
4. A reader opens the thread through `osp/replies/0` without using its public identity.
5. Jimmy deletes his reply and it does not return.
6. Kim hides a reply without creating a Delete by its author.
7. Kim closes the thread; a new submission receives Closed.
8. Kim deletes the root; the viewing endpoint returns Kim's signed Delete.
9. Bob/another node submits while Kim is offline; it survives restart and delivers once Kim returns.

Record the commands, object IDs, computed levels/path lengths and relevant privacy observations in:

`docs/testing/replies.md`

## Consequences

- OSP does not expose an anonymous write endpoint into every creator's node.
- A commenter speaks as a signed public identity.
- Immediate human relationships enter easily.
- Useful first-hand workers can earn entry from actual work with this creator.
- Established creators and Hunters can cross communities without already being in the creator's direct circle.
- Six degrees is based on locally provable public curation relationships plus the creator's private anchors, not a fictitious globally visible follow graph.
- Popularity never buys bandwidth, addresses, private content or machine standing.
- A partial local graph can miss a legitimate person; that becomes Request or rejection, never invented evidence.
- Comment validation itself causes no network fan-out in v1, so a submission cannot amplify into a six-hop Tor crawl.
- The creator retains final moderation of the thread their node serves without gaining the ability to rewrite another person's signed speech.

## Revisit

Revisit if:

- legitimate established creators/Hunters routinely fail because the local graph is too sparse;
- the 90/180-day windows are too short or long;
- the Creator/Hunter bands are easy to manufacture;
- the distance thresholds admit too many rings or exclude useful outsiders;
- the second-path rule at distance 5–6 is too strict;
- comment requests become noisy;
- users need a deliberate referral path;
- users want an explicit unrestricted-comments mode;
- active, privacy-preserving validation lookups become necessary;
- nested replies are ready to be designed.
