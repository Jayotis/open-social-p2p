# ADR 0029: Vouchers within six steps of the creator

**Status:** Accepted (2026-11-12; amended 2026-11-13: a replier's side is sent only when asked for, who answers the circle question, and that every meeting counts through a person in the circle, not Kim's node; and after its `osp-sim` round, that meeting points count only as far as their chains to the replier are separate, and a meeting's own work may take 150 ms on an SBC-sized node, and the table asks 4 points at 4 steps and 6 at 5 or 6; amended 2026-11-27: a vouch is its own signed object, one per voucher and person, issued 10 minutes after a reshare and valid 30 days unless renewed, [section 7](#7-a-vouch-is-separate-from-a-reshare-and-expires)). Takes up what [ADR 0028](0028-replies-let-in-by-the-creator.md) left for later: letting strangers' replies in without the creator having to look. On by default once its `osp-sim` round passes (the maintainer, 2026-11-12). Uses [doc 12](../12-peer-reciprocity-and-recognition.md)'s creator and hunter recognition in a narrow form, for this one use; doc 12's levels and their other uses stay a proposal.

## Context

Under ADR 0028, Kim's node publishes replies from people Kim follows or trusts (her **circle**), and sends everyone else to her requests.

The case this record is for: Jimmy is a popular creator or hunter in another community. He comes across Kim's post and replies. Kim wants that: his reply brings her attention. But he's not in her circle, and she may not see her requests for days.

Somewhere within six steps of Kim's circle, someone has probably vouched for Jimmy: reshared him, or credited him with finding their work. Kim's node should find that, and let him in.

Two things make a plain walk of six steps the wrong way to do it:

- **Six steps reach nearly everyone,** spammers too. One careless reshare five steps out would open Kim's replies to whoever got it, and to every throwaway identity that one vouches for.
- **Walking at reply time is costly.** Each reply would send requests over Tor to many nodes, each passing it on, and each learning that Kim is checking Jimmy.

So both sides do their part ahead of time, in the background, and they meet in the middle when the reply arrives. And the farther out the meeting, the more separate places in Kim's world must vouch for Jimmy. Being popular is what gets someone in from far away.

## Terms

- **A vouch** is a signed act by one person for another, valid for 30 days unless renewed ([section 7](#7-a-vouch-is-separate-from-a-reshare-and-expires)). There are two kinds:
  - **For resharing:** Bob reshared Jimmy's posts in the last 90 days.
  - **A found-early credit:** the author of a post credits Jimmy with being one of the first to reshare it (below). This is how a hunter is vouched for.

  Until the 2026-11-27 amendment, the reshare itself was the vouch, and lasted for good.
- **Steps** count vouches outward from Kim. Kim's circle is 1 step. Whoever the circle vouched for is 2. And so on.
- **Kim's side** is everyone within 3 steps of Kim that her node, or her circle's nodes, can place from what they already hold.
- **Jimmy's side** is the people who vouched for Jimmy, those who vouched for them, and those who vouched for *them*: up to 3 steps back from Jimmy.

## Decision

### 1. An author hears of every reshare, and credits early finders

Today a resharer tells the author's node only of reshares with hosting (ADR 0025). From now on it tells of every one: `Reshared { announce, profile }` on the social endpoint. The author's node keeps **who vouched for its owner**:

- one `Announce` per resharer per post, and from 10 minutes after it, the resharer's `Vouch` (section 7), one per resharer;
- at most 10,000 in all, dropping those of the oldest posts first;
- at most 200 new resharers an hour, past which new ones are refused, so a flood can't push out the real ones.

The author's node hears reshares in the order they come. When a post has **10 different resharers within 7 days** of its first, the author's node credits each of the first 3 with finding it early, and keeps which post each was for. The **found-early credit** it sends each finder's node is a `Vouch` of kind `found_early` (section 7), which that node holds. Order is the author's own: a finder can't backdate their way in.

### 2. Jimmy's node gathers his side, in the background

Once a day, Jimmy's node asks the nodes of those who vouched for him: "who vouched for you?" Each answers from the vouches it holds that are still valid (section 7), at most 32, newest first, each with its voucher's signed profile so Jimmy's node can reach their node. Then it does the same one step further back.

It keeps at most **256 vouches** in all, nearest first. These are public acts, so any node answers anyone, limited to 10 questions an hour from one node.

This is the nodes getting to know each other while their owners do other things. Jimmy's node comes to know the nodes of those who vouched for him.

### 3. A reply shows Jimmy's side when asked

Jimmy's node sends his reply with ADR 0028's `SubmitReply`, as now. Most replies come from Kim's circle and need nothing more. When Kim's node would send the reply to her requests and vouchers are on, it answers "show who vouched for you", and Jimmy's node sends his side in a second request: up to 256 signed vouches, about 150 KB (this one request may be up to 256 KiB; every other stays within 64 KiB). Kim's node checks each one: it verifies, it isn't signed by a key Kim's node knows was revoked, and it's a vouch for whom it says.

Kim's node only knows revocations of identities it holds. A revoked key of someone far out still vouches until Kim's node learns of it: one meeting point of the 3 to 10 needed, so low harm.

### 4. Kim's node looks for where the two sides meet

**First, on its own.** Kim's node holds the outboxes of those Kim follows, so it can place:

- **1 step:** her circle;
- **2 steps:** whoever her circle reshared in the last 90 days.

It looks for those people among Jimmy's side.

**Then, if that isn't enough, it asks Kim's circle.** It asks up to **16 nodes** of people in her circle: trusted followers first, then those it's followed longest. Each gets one question: "which of these identities can you place within 2 steps of your owner?", with the identities of Jimmy's side (not the vouches). Each answers from what it already holds: which ones, and at how many steps from its owner.

- A node only answers nodes whose owner follows its owner, or whom its owner follows, at most 60 questions an hour. So Alice's node answers Kim's if Kim follows Alice, or if Alice follows Kim (as each of Kim's trusted followers does). Alice's node never learns whether Kim trusted her (ADR 0013).
- No node it asks asks anyone else. The question goes one step out and stops.
- Alice's node answering is Alice's node vouching: Kim chose to follow her. A meeting through Alice is at 1 + Alice's steps.

So Kim's side reaches 3 steps, Jimmy's side 3 steps back, and together they find vouchers up to **6 steps** apart.

### 5. How many meetings let a reply in

Each **meeting point** is a different person on Kim's side found in Jimmy's side.

What counts is **how many different meeting points** there are, not how many vouches, and only as far as their **chains to Jimmy are separate**: Kim's node counts the most meeting points it can join to Jimmy by chains of vouches that share nobody but Jimmy. Throwaway identities behind one careless reshare all meet Kim's side at that one person, so they count as one. So does everyone who vouched for the careless person: they're on Jimmy's side too, 3 steps back, but every chain from them to the operator goes through the careless person. The `osp-sim` round found that one (2026-11-13): there, the operator met Kim's world at 5 points of the 10 needed, and a careless person better vouched-for near Kim would have let it in.

Each meeting is **through someone in Kim's circle**: Alice herself, whoever Alice reshared, and whatever Alice's node answers are all through Alice, whether Kim's node found them or Alice's did. Kim's node isn't a place of its own, so one careless friend never counts twice.

| Jimmy's distance from Kim | Meeting points needed | Reached through |
| --- | ---: | --- |
| 2 or 3 steps | 3 | Kim's node, or her circle |
| 4 steps | 4 | at least 2 different people in her circle |
| 5 or 6 steps | 6 | at least 2 different people in her circle |

Until the `osp-sim` round, the table asked 5 points at 4 steps and 10 at 5 or 6, counting every different person. Counted by separate chains, popular strangers far away met Kim's world at 4 to 8 points, and 14 of 30 got in. Each separate chain needs a different real person's vouch, so fewer are needed for the same trust; with this table 27 of 30 did, and no throwaway reply ([results](../testing/vouchers.md#results)).

At or above the mark, under `network`, the reply is **published**. Below it, the reply goes to Kim's requests with what was found: "4 steps away, 3 people in your circle's world vouched for them".

A popular creator or hunter has many vouchers in many places, so from far away they still meet Kim's world at many points. Someone with one lucky reshare meets it at one.

It's on by default once its `osp-sim` round passes, and can be turned off in `config.toml`:

```toml
[replies]
vouchers = true
```

The setting only says whether Kim's node lets replies in this way. Recording reshares, giving credits, answering "who vouched for you" and gathering one's own side always run, since other people's replies depend on them.

### 6. What it's for, and not

Getting in through vouchers lets a reply in. It doesn't buy bandwidth, a direct connection, a place among sources, a place in a feed, private posts or trusted-follower status. A reply hidden or blocked changes nobody's standing on the network. Doc 12's rule that recognition never buys service holds.

What Kim's node found isn't shown to anyone but Kim, and isn't a score of Jimmy that others see. Each creator's node finds its own.

### 7. A vouch is separate from a reshare, and expires

Resharing a post, vouching for a person and keeping a relationship with them are related, but not the same. Bob may delete a reshare because the post is out of date, because he's tidying his profile, or because he no longer agrees with that one post. None of those means he no longer stands behind Jimmy. If Bob finds Jimmy is a spammer or a pirate, he blocks him.

Until this amendment, the signed reshare was the vouch, and Jimmy's node kept it for good: deleting it, or blocking Jimmy, changed nothing. So a vouch is now its own signed object, and it expires instead of being taken back:

- **`Vouch { actor, subject, kind, valid_until }`**, signed by Bob's node for Bob (`actor`), as every object is ([ADR 0010](0010-signing-keys-and-revocation.md)), for Jimmy (`subject`). `kind` is `reshare` or `found_early`. One per voucher, person and kind: Bob's 40 reshares of Jimmy's posts are one vouch. The voucher's node sends it to Jimmy's with `Vouch { vouch, profile }` on the social endpoint.
- **Issued 10 minutes after the reshare**, if it still stands. A reshare deleted within that time never vouches, so a mis-tap costs nothing. The reshare goes to Bob's followers at once as before; only the vouch waits.
- **Valid 30 days.** Bob's node renews it around day 20, at a random time within a few days, so Bob's node can be off for about 10 days without a lapse. Jimmy can't extend it: he can't sign as Bob.
- **Renewed while Bob reshared Jimmy in the last 90 days and hasn't blocked him.** A reshare deleted later still counts until its vouch runs out; a reshare older than 90 days no longer renews it. Unfollowing Jimmy changes nothing: it isn't distrust.
- **Blocking Jimmy stops issuing and renewing** Bob's vouches for him. Nothing is sent, as with every block. Jimmy's vouch from Bob runs out within 30 days.
- **Found-early credits work the same way:** the post's author's node signs one for 30 days and renews it, unless the author blocks the finder.
- **Kim's node checks `valid_until` against its own clock** and counts no vouch that ran out. It asks nobody. Jimmy's node drops expired vouches, and "who vouched for you" answers only valid ones.

No message takes a vouch back, and nobody but Kim's node judges one. That keeps step 4's rule: nothing about a reply goes further than one step from Kim's node.

10 minutes and 30 days are starting values, not measured. Once real nodes run, count the renewals each node sends a day before changing them.

Kim's own side (step 4: whoever her circle reshared in the last 90 days) is read from her circle's outboxes, not from vouches. It already ends after 90 days, or when Bob deletes the reshare, and stays as it is.

## What it reveals, and the risks it leaves

- **Alice's node learns** that someone with Jimmy's side replied to Kim, from the identities it's asked about. It's likely public soon anyway, if the reply is published. Low harm.
- **Kim learns** which of up to 256 identities Alice's node places, which says a little about whom Alice follows or reshared. Kim follows Alice, and the identities are ones Jimmy chose. Low harm.
- **"Who vouched for you"** can be asked by anyone, which makes a person's reshares easier to collect. They're public in the resharers' outboxes already; the limit keeps it slow.
- **Getting a spam reply in from far away** takes 6 meeting points on separate chains, through 2 people in Kim's circle. That means getting reshares from 6 real people near Kim's world, none of whom reaches Jimmy through another, which is about what being popular is. Through one careless reshare, it can't be done, however well vouched-for the careless person is. Unlikely, at real effort, for one reply until Kim blocks it.
- **Renewals show liveness, and their end shows a block.** Jimmy's node learns about once a month that Bob's node is up. If Bob's renewals stop while his reshare still stands, Jimmy can tell he was probably blocked, within about 30 days. The block sends nothing; the random renewal time only blurs when. Accepted (the maintainer, 2026-11-27): those likely to notice are spammers, and they learn only that one voucher dropped them.
- **Getting in from close** (3 steps) takes 3 meeting points, the cheapest way. It needs three separate people near Kim's circle to vouch. That's the same bar as being reshared by three of her circle's friends.

## Options considered

- **A walk at reply time, each node asking the next.** The cost and spread of telling, above, for every reply.
- **Nodes that work together pass each other the vouches they hold** (suggested in review, 2026-11-12). Passing on everything tells a peer whom one's owner follows. Passing on only the vouches for one's owner places nobody. Jimmy's node gathering his own side does what was wanted.
- **Taking a vouch back with a message to Jimmy's node** (2026-11-27). Jimmy's node holds the vouch, so a spammer's node would ignore it. Expiry needs no message, and Jimmy can't undo it.
- **Kim's node asking Bob's whether he still vouches** (2026-11-27). A walk at reply time again, and it tells Bob's node that Kim is checking Jimmy.
- **Counting vouches, not meeting points.** Throwaway identities behind one careless reshare would count each.
- **Two steps only, 3 resharers placed by Kim's node** (this record's first draft, 2026-11-12). Safe, but a popular stranger from another community wouldn't reach Kim's world in two steps, and that's the case this record is for.
- **Six steps with two independent paths** (ADR 0028's first draft). Distinct meeting points, from at least two of Kim's circle, are the same idea in a form that a few hundred carried vouches can show.

## Consequences

- Popular creators and hunters can reply on posts outside their community and be published without the creator looking. Others reach requests, with what was found.
- Every reshare costs one message to its author. Each person one vouches for costs a renewal about every 20 days, and each found-early credit one too. Posts that take off give their first finders a credit.
- Nodes talk more in the background: once a day to those who vouched for their owner, and one question to up to 16 of the circle's nodes for a reply that needs it.
- Nothing about a reply goes further than one step from Kim's node.

## Building it

After ADR 0028's replies:

1. `Reshared`, the record of who vouched, `FoundEarly`. The node view's "reshared by" count.
2. Gathering one's side, and answering "who vouched for you".
3. Carrying it with a reply, meeting on Kim's node, the question to her circle, the table.
4. One `osp-sim` round. It's on by default once that passes.
5. Section 7 (2026-11-27): the `Vouch` object, the 10-minute wait, renewal, expiry checked on Kim's node, and found-early credits that expire. Reshares stop counting as vouches.

## Checking it

Tests, at least:

- A reshare tells its author's node; a second by the same resharer of the same post replaces the first; the limits hold under a flood.
- A post's first 3 finders get credits once it has 10 resharers within 7 days, by the order the author's node heard them; a finder's claimed date changes nothing.
- A carried vouch that doesn't verify, is signed by a revoked key, or isn't for whom it says, doesn't count. Nor does one past its `valid_until`, or a bare reshare.
- A reshare deleted within 10 minutes issues no vouch; one deleted later keeps its vouch until it runs out, and isn't renewed once Bob has no reshare of Jimmy from the last 90 days.
- Blocking Jimmy stops Bob's node renewing his vouch, and sends nothing; unfollowing him doesn't stop it. The same for a found-early credit and its author's block.
- Renewal comes before expiry with the node off for up to 10 days.
- 200 throwaway identities vouching for each other and for Jimmy, behind one careless reshare by someone in Kim's world, give one meeting point.
- The table: at each distance, one short of the mark goes to requests; at the mark it's published; through only one person in Kim's circle, 4 steps and more go to requests.
- A circle node asked doesn't ask anyone else; it answers only nodes whose owner follows its owner or whom its owner follows, within the limit.
- A reply from the circle is published without its side being asked for; a stranger's side is asked for only with vouchers on.

One `osp-sim` round, a few hundred people in 10 communities, following and resharing:

- 30 popular creators and hunters, each vouched for by 30 or more people across 3 or more communities, replying 4 to 6 steps from the creator;
- 50 ordinary strangers replying;
- one operator with 200 throwaway identities, one careless reshare 5 steps out, and the identities vouching for each other and the operator.

**Pass:** no throwaway reply is published; at least 80% of the popular strangers' replies are published; a reply's meeting needs at most 16 questions and 150 ms of work on an SBC-sized node; gathering one's side costs under 100 requests a day.

The mark for work was 50 ms until the round (2026-11-13; [results](../testing/vouchers.md#results)). Nearly all of a meeting's work is checking the side's signatures, about 54 µs each on the desktop the round ran on, so a full side of 256 can't be checked in 50 ms on an SBC one at a time. A creator's node checks each waiting request's side once, and keeps at most 100 waiting, so a flood costs an SBC seconds, not more. The maintainer raised it to 150 ms.

On the test nodes: Kim follows Alice; Alice reshared Bob; Bob, through two more steps, vouches for people who vouched for Jimmy, at enough points for his distance; Jimmy's reply is published. Carol, vouched for only by throwaway identities behind one reshare, goes to requests.
