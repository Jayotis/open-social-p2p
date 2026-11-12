# ADR 0029: Who reshared you, carried with your reply

**Status:** Proposed (2026-11-12). Takes up what [ADR 0028](0028-replies-let-in-by-the-creator.md) left for later: letting some strangers' replies in without the creator having to look. Uses [doc 12](../12-peer-reciprocity-and-recognition.md)'s creator level in a narrower form, for this one use. Doc 12's hunter level and its other uses stay a proposal.

## Context

Under ADR 0028, Kim's node publishes replies from people Kim follows or trusts, and sends everyone else to Kim's requests. A stranger worth hearing waits for Kim to look.

The first draft of ADR 0028 tried to let such strangers in by their creator level and their distance from Kim through public reshares. It ran into two problems:

- **Kim's node sees few reshares.** It holds what the people Kim follows reshare, and what directories list. So for most strangers it has no evidence either way.
- **Reshares are cheap to fake in one direction.** Anyone can reshare anything. If Jimmy's level counted every reshare of his posts, three throwaway identities would raise it. If being placed near Kim counted Jimmy's reshares *of* Kim's circle, resharing one of Alice's posts would do it.

What's hard to fake is the other direction: **someone Kim already knows chose to reshare Jimmy.** This record gets that evidence to Kim's node, without Kim's node asking anyone, and counts only that.

## Decision

### 1. An author is told of every reshare

Today a resharer tells the author's node only of a reshare with hosting (ADR 0025's `Hosting` request). From now on it tells of every reshare: `Reshared { announce, profile }` on the social endpoint, over Tor like other messages. It's one message a reshare, sent as the hosting one is.

A reshare is already public, in the resharer's outbox. Telling the author says nothing new about the resharer. It gives the author's node a record of **who reshared its owner's posts**, each one an `Announce` signed by the resharer.

The author's node keeps:

- one `Announce` per resharer per post, the newest;
- at most **10,000** in all, dropping those of the oldest posts first;
- at most **200 new resharers an hour**. Past that, new ones are refused, so a flood can't push out the real ones.

This record is also what the app's node view can show: "reshared by 41 people this month".

### 2. A reply carries some of it

With a reply (ADR 0028's `SubmitReply`), Jimmy's node sends up to **32** of those signed `Announce`s, each by a different resharer, newest first, hosting ones first. It's Jimmy's own record of who reshared him, which only he could have gathered, and which nobody can forge: each one is signed by its resharer.

Jimmy's node chooses what to send, so it could leave some out. That only hurts Jimmy's chances.

### 3. Kim's node counts only resharers it can place

Kim's node checks each carried `Announce`: it verifies, it's a reshare of a post by Jimmy, and its signing key isn't revoked. Then it counts **the resharers it can place**, using only what it already holds:

- **One step:** people Kim follows, and Kim's trusted followers.
- **Two steps:** people that someone at one step reshared in the last 90 days. Kim's node holds those reshares already, from the outboxes of those Kim follows.

Jimmy's **standing with Kim** is how many different placed people reshared him in the last 90 days, by the carried `Announce`s and any Kim's node already holds. A hosting reshare counts 1.5, as in doc 12.

Jimmy's own throwaway identities count for nothing, however many reshare him: none of them is placed. To count, a reshare has to come from someone Kim follows, or someone they reshared.

### 4. Standing lets a reply in

Under ADR 0028's `network` policy, a reply from someone with no direct way in is:

- **published** when their standing with Kim is **3 or more**;
- sent to Kim's requests otherwise, as now, with the standing shown ("reshared by 2 people you follow or that they reshared").

The mark is set in `config.toml`, and 0 turns it off:

```toml
[replies]
publish_at_standing = 3
```

Standing is worked out when a reply arrives, from what's stored, and kept for an hour. It asks no other node anything.

### 5. What standing is for, and not

Standing lets a reply in. It doesn't buy bandwidth, a direct connection, a place among sources, a place in anyone's feed, private posts or trusted-follower status. A reply hidden or blocked doesn't change anyone's network standing. Doc 12's rule that recognition never buys service holds.

It isn't shown to anyone but Kim, and isn't a score of Jimmy that others can see. Each creator's node works out its own, from its own one and two steps.

## Options considered

- **Nodes that work together pass each other the reshares they hold** (suggested in review, 2026-11-12). A peer passing on all it holds tells the other whom its owner follows, as that's where most of it comes from. A peer passing on only reshares of its owner's posts tells nothing new about its owner, but nothing that places anyone either: being reshared by Alice's resharers says nothing about them. The reply carrying its own record gets the evidence where it's needed, when it's needed, and tells nobody anything they couldn't see.
- **Counting every reshare** (ADR 0028's first draft). Cheap to raise with throwaway identities, as above.
- **Kim's node asking around** when a reply comes, up to six steps. Every reply would cost Tor circuits on many nodes, and would tell them Kim is checking Jimmy.
- **Three steps instead of two.** Kim's node would need the outboxes of people it doesn't follow. Two steps already reach those Kim's follows reshare. A replier at three steps can still get in through requests.

## The risk this leaves

To get a reply published on Kim's post, someone needs three people Kim follows, or people they reshared, to reshare them first. With throwaway identities alone it can't be done. To do it, they'd have to get real people in Kim's circle to reshare spam, or take over their nodes. The payoff is one reply on one creator's posts until Kim blocks them. Unlikely, at real effort, for little.

A real person Kim follows who reshares carelessly lets in those they reshare. That's the point of the rule as much as its weakness: it's Kim's own circle's judgement, and Kim chose to follow them.

## Consequences

- Strangers that Kim's circle reshares get their replies published without Kim looking. Others still reach Kim's requests, with their standing shown.
- Every reshare costs one more message, to its author's node, which keeps a record of who reshared it.
- Nothing about a reply makes Kim's node ask another node anything.
- Kim's node never learns who Jimmy's node follows, only who reshared Jimmy, which is public.
- Doc 12's creator level is used here in a narrow form: counted only from placed resharers, seen only by the creator, for letting replies in. Its general form and the hunter level stay a proposal.

## Building it

After ADR 0028's replies:

1. `Reshared`, and the author's record of who reshared its posts, with its limits. The node view's count.
2. Carrying up to 32 with a reply; placing and counting on Kim's node; the mark.

## Checking it

Tests, at least:

- A reshare tells the author's node; a second one by the same resharer of the same post replaces the first; the hourly and total limits hold under a flood.
- A carried `Announce` that doesn't verify, isn't of the replier's post, or is signed by a revoked key, isn't counted.
- A resharer Kim follows counts; one that someone Kim follows reshared counts; one whose only tie is having reshared someone in Kim's circle doesn't.
- Any number of throwaway identities resharing Jimmy, and resharing Kim's circle, leave his standing at 0.
- Standing 3 publishes under `network`, not under `circle`; 2 goes to requests; the mark at 0 sends all to requests.
- Working out standing sends nothing to any other node.

One `osp-sim` round, before it's on by default: a few hundred people in communities, following and resharing; 50 strangers each replying to someone outside their community; one operator with 200 throwaway identities resharing each other, the operator, and the creators' circles. **Pass:** no throwaway reply is published; at least half of the strangers reshared by 3 or more people in a creator's circle are published; standing takes under 10 ms on an SBC-sized node.

On the test nodes: Kim follows Alice; Alice reshares Bob; Bob and two others Kim follows reshare Jimmy; Jimmy's reply is published, and Carol's, reshared only by throwaway identities, goes to requests.
