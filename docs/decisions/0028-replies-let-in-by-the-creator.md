# ADR 0028: Replies, let in by the creator's node

**Status:** Proposed (2026-11-11). Trimmed 2026-11-12 to what M5 builds first: replies, a creator's circle let in directly, and a request inbox for everyone else. Letting strangers in automatically by creator or hunter level and by social paths up to six steps long is left to a later record (see [Later](#later-letting-strangers-in-automatically)).

Builds on [ADR 0013](0013-trusted-followers.md) (trusted followers), [ADR 0019](0019-taking-posts-and-media-down.md) (deleting), [ADR 0024](0024-watching-found-posts-without-your-identity.md) (viewing endpoints), and the [roadmap](../06-roadmap.md)'s M5, which moves replies forward from Stage 2.

## Context

A public post needs conversation. But an open reply endpoint would let any new identity on the internet write to a creator's home node, and identities cost nothing. Following can't be the gate either, as anyone may follow.

The opposite, "only people the creator follows may reply", shuts out everyone else. A useful stranger should have some way in.

So: **reading is open and private, replying is public and signed, and the creator's own node decides what it shows.**

## Decision

### A reply is an ordinary Note

There's no new content type. A reply is a signed `Note` whose `in_reply_to` names the post it answers. The field exists already and nothing uses it yet.

- It answers a post (the **root**), never a reshare. Pressing Reply on a reshare answers the post it shares.
- Text only, not empty after trimming, at most **4,000 bytes**. No attachments.
- No replies to replies yet: the root must have no `in_reply_to` of its own.
- Signed by the replier's public identity, never a machine name, viewing key or hosting key.

### Replies stay out of the outbox

Today's clients would show a reply in someone's outbox as a post of its own. So a reply goes only to the root's creator, and is kept apart. It isn't in anyone's feed, a directory's latest posts, search results, reshares or hosting. A profile's "Replies" view can come later without changing the signed object.

### Each post's policy

| Policy | Who's let in |
| --- | --- |
| `off` | Nobody |
| `circle` | Those the creator follows, their trusted followers, and those the creator chose to allow |
| `network` | The circle directly; anyone else goes to the creator's requests |

New posts default to `network`. Set in `config.toml`:

```toml
[replies]
default_policy = "network"
```

### What the creator's node decides, in order

Kim posts. Alice is someone Kim follows. Jimmy is a stranger.

1. **The checks below fail:** refused.
2. **The policy is `off`:** closed.
3. **Kim blocked the replier:** refused, with the same answer as any other refusal, so nobody can find out they're blocked.
4. **Kim allowed them, follows them (Alice), or they're a trusted follower:** published.
5. **The policy is `circle`:** refused.
6. **Otherwise (Jimmy):** goes to Kim's requests.

Following Kim gets Jimmy nothing. Kim can publish his reply once, publish it and allow him from then on, decline it, or block him.

### Checks on a reply

Before deciding anything, the node checks that:

1. the reply verifies as a `Note`, not signed by a key its identity revoked;
2. the profile sent with it verifies, is the replier's, and names the node that's connecting;
3. it answers a post in this node's own outbox, by this node's identity, that hasn't been deleted;
4. it has no attachments, and its text is within the limit;
5. none of the limits below is reached.

A reply that fails is refused.

### Sending a reply

Sending is a public act, so it goes to the creator's ordinary social endpoint (`osp/social/2`), over Tor as other messages do (ADR 0014). It doesn't need a direct connection.

- **`SubmitReply { reply, profile }`** answers with the reply's state: `Published`, `AwaitingApproval`, `NotPublished`, `Closed` or `Deleted`. Sending the same reply again gives its current state.
- **`DeleteReply { delete, profile }`** carries the replier's own signed `Delete` (ADR 0019). The creator's node drops the reply and keeps the `Delete`, so an old copy can't come back. Deleting one's reply is the same Delete as for a post, from the person's point of view.

A reply is stored on the replier's node as soon as it's signed. If the creator's node can't be reached, it's tried again after 1 minute, 5, 30, 2 hours, 12 hours, then daily, for 30 days. `osp sync` tries at once. Any answer from the creator's node ends the tries. The reply's ID keeps a retry from making a second copy.

### Reading a thread

Reading mustn't tell the creator who's reading. So threads are read through a viewing endpoint, as ADR 0024's found posts are, on a new protocol on the creator's media endpoint, `osp/replies/0`:

- **Asked:** a post, and where the reader got to (`after`), as a directory's lists are read.
- **Answered:** published replies, oldest first, each with the replier's profile, at most **50** a page and **256 KiB**. Or the creator's signed `Delete`, if the post is gone.
- At most **16** reads served at once. Nothing about the reader is asked for or logged.

### What the creator can do

- With a request: **publish once**, **publish and allow from now on**, **decline**, or **block**. Requests are private, and go after 30 days.
- With a published reply: **hide** it from the thread, or show it again. **Block** the replier, which hides their replies on Kim's posts and refuses new ones.
- Hiding isn't deleting: the creator never signs a `Delete` for someone else's words.
- Allowing someone to reply gives them nothing else: not trusted-follower status, a direct connection, or standing.
- Deleting the root closes its thread. Its replies can then be dropped, and readers get the creator's `Delete`.

### Limits

| | Limit |
| --- | ---: |
| Reply text | 4,000 bytes |
| Published and hidden replies a post | 2,000 |
| Requests waiting, in all | 100 |
| Requests waiting from one identity | 3 |
| New replies from one node an hour | 20 |
| New replies from one identity to one post a day | 20 |
| Reads served at once | 16 |

When requests are full, a new one is refused. An old one isn't pushed out, or a flood would wipe out the real ones. When a thread is full, new replies go to requests while there's room.

**The risk this leaves:** at the 100-request cap, someone with many throwaway identities, each on its own node key, can fill Kim's requests in about an hour, so a real stranger's reply is refused until Kim clears them. The cost to them is scripting the keys; the gain is only that. Published replies and Kim's circle aren't touched. If it happens, Kim can set `circle` on a post, and the cap is revisited.

### Storage

New tables for: each post's policy; replies received, with their state (published, hidden or request) and the profile that came with them; each identity Kim allowed or blocked; replies being sent, with their tries; and replies' `Delete`s. Column names follow the existing tables' style.

### For the app and CLI

Client requests to: reply; read a thread; list requests and answer one; set a post's policy; hide or show a reply; allow or block someone. `Delete { id }` on one's own reply sends a `DeleteReply`. Each request comes with why it's there ("you don't follow them"), from what the node knows.

CLI: `osp reply <post> <text>`, `osp replies <post>`, `osp reply-requests` (`osp trust requests` is already trusted followers'), `osp reply-policy <post> off|circle|network`, and allow, decline, block and hide commands.

## Later: letting strangers in automatically

The draft of 2026-11-11 also let strangers in automatically when they were within six steps of the creator through public reshares, at a [creator or hunter level](../12-peer-reciprocity-and-recognition.md) high enough for that distance. It's left out for now, as:

- **The levels are still a proposal** (doc 12). Doc 12 counts only reshares by people within two follow steps of the one counting, and accepts that reshare rings can raise levels *because levels give recognition, never reach*. Letting levels admit replies gives them reach, so doc 12 has to be settled first, for that use.
- **A creator's node sees few reshares.** It holds reshares by those it follows, and what directories list. It doesn't see strangers' reshares of its own posts unless they host. So levels and paths worked out by Kim's node would mostly be empty, and a simulation has to model what each node really receives, not the whole network.
- **Rings are cheap.** With levels counting anyone, a stranger would need only a reshare of one of Alice's posts, which puts them two steps from Kim, and three throwaway identities resharing them to reach level 1. That's minutes of work for replies published unasked.

Letting in nodes that did checked work for the creator ([ADR 0018](0018-earned-direct-connections.md)'s layer 2) waits for that layer to be built.

A later ADR takes these up, with a simulation on each node's real view. The original draft is in the maintainer's notes.

**"Hunter"** is the name from now on for doc 10's "scout": someone whose reshares find posts early. New code and docs use it; accepted ADRs and reviews keep their wording.

## Not in this record

Replies to replies, attachments, reactions, mentions, notifications, editing replies, ranking replies or using them in ranking, other nodes hosting threads, and the ActivityPub bridge.

## Building it

1. **Replies:** the objects, `SubmitReply` and `DeleteReply`, sending with retries, policies, admission as above, requests, hiding and blocking, `osp/replies/0`, the CLI.
2. **The app:** a Reply action and composer, the thread, a reply's state while it's waiting, Kim's requests, and each post's policy.

## Checking it

Tests, at least:

- A reply to a reshare, to a reply, with attachments, from a node its profile doesn't name, or signed by a revoked key, is refused.
- Someone followed, a trusted follower, and someone allowed are published; someone who only follows the creator goes to requests under `network` and is refused under `circle`; someone blocked gets the same answer as any refusal.
- A published reply is in its thread but not in the feed, search or a directory's lists.
- Hiding takes it out of the thread without a `Delete`. The replier's `Delete` removes it, and sending it again doesn't bring it back. Deleting the root closes the thread and readers get the `Delete`.
- A reply to a creator who's offline is kept through a restart and arrives once.
- Sending the same reply twice gives one copy. The page, request and thread limits hold under a flood.
- Reading a thread sends nothing from the reader's public, media or direct endpoints, tested as ADR 0024's viewing is.

On the test nodes, with Kim, Alice (followed by Kim) and Jimmy (a stranger): Alice's reply is published; Jimmy's goes to requests and Kim publishes it; a reader reads the thread without its identity; Jimmy deletes his and it stays gone; Kim hides one, closes the thread, and deletes the post; Jimmy replies while Kim's node is off and it arrives after. Results in `docs/testing/replies.md`.

## Consequences

- No creator's node takes writes from anyone at all: strangers reach only a capped request inbox.
- A replier speaks as a signed public identity. Readers stay unseen.
- The creator decides what their node shows without being able to change anyone's words.
- Strangers worth hearing wait for the creator to look, until a later record lets some in automatically.
