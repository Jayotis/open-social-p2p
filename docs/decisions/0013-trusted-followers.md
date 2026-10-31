# ADR 0013: Trusted followers, asked for and allowed on the phone

**Status:** Accepted (2026-10-31).

## Context

A node names followers holding a copy of its media to its other followers, so they fetch from each other instead of from it ([doc 13](../13-hosting-roles-and-follower-swarm.md)). Up to 6 are named at a time, from up to 100 per file. Which ones it names is its own choice, made only from what it saw itself: first those it heard from in the last few minutes, those reached directly before those through a relay, at random within each ([ADR 0003](0003-native-social-protocol-v0.md)). A downloader fetches from the first 3 named at once.

Following is open: any node that knows ours can follow it, and node IDs cost nothing. So the node never takes another node's word about a follower ([doc 03](../03-peer-distribution-and-scoring.md), [doc 12](../12-peer-reciprocity-and-recognition.md) principles 1 and 2), and nothing a follower can do on its own gets it named ahead of the others: a crowd of fake followers could do the same.

But the owner knows some of their followers: family, friends, people they work with, or people they've come to trust online over the years. That trust is between people, and the open network can't fake it. Over time it could mean more than hosting: posts only trusted followers see, messages, direct connections. [Doc 12](../12-peer-reciprocity-and-recognition.md) already lets the owner give extra weight to people they follow, under contention.

Questions to settle:

1. What does being trusted mean, and what does it grant?
2. Who decides that a follower is trusted, and how?
3. What does the owner see, and what can they check?
4. What does trust change now?
5. How is the owner protected from a flood of requests?

## Options considered

**Who decides.**

- (a) **Automatic, from conditions:** a follower that reveals its internet address, accepts direct connections, has a profile and posts. Each condition is cheap for a fake: a rented server has an address and accepts direct connections, and a script can post. A crowd meeting them would be named first, which is the crowding-out the hosting check and the half-and-half list were built against.
- (b) **The owner, on their own:** `osp trust add <name>` for someone they follow. No new message, and the owner already checked who it is when they added the follow code. But it works only for people the owner follows, and a friend can't ask.
- (c) **The follower asks, the owner allows:** the follower's node sends a request; the owner sees who it is and allows or declines it on their phone. Fake followers can fill the list of requests, burying a friend's.

**What trust grants.**

- One flag that every feature reads: simple, but a later feature could take it as permission for something the owner never agreed to, such as showing their home address.
- **The relationship, with each use decided separately:** the owner trusts a person, and each feature that uses trust has its own rule or setting.

**Requiring an internet address.** Showing a home address could be a condition for asking. But relay-only mode exists to hide it ([ADR 0007](0007-transport-privacy-defaults.md)), and M4 hides it behind Tor; requiring it would leave out the people most careful about privacy. Whether a node reaches ours directly is shown to the owner instead, and already counts in which followers are named first.

## Decision

Options (c) and (b) together, and the relationship with each use decided separately.

**What it means.** A trusted follower is a person the owner has chosen to trust, recorded on the owner's node against that person's identity and the nodes the owner allowed for it. Being trusted grants nothing on its own. Each use of it is a rule or setting of its own, decided in its own right, and the owner is told what it does before it applies. This ADR decides one use, hosting (below). Later uses each get their own decision:

- serving trusted followers first when busy ([doc 12](../12-peer-reciprocity-and-recognition.md), part 2);
- posts only trusted followers see, and messages;
- direct connections in M4. The [roadmap](../06-roadmap.md) already lets each node choose who it connects to directly: anyone, people it follows, contacts it approved, or nobody. Trusted followers can become one of those choices, off until the owner picks it, and the app says first that trusted followers may then learn the home address. The node's connections are authenticated and limited, so an address mostly lets someone flood the home connection, and shows roughly where it is and which provider it uses. Sharing it with a friend is reasonable, but only when the owner knows.

**Asking.** The owner of a node that follows someone asks from the phone (on that person's profile: "Ask to be a trusted follower") or with `osp trust ask <name>`. Their node sends `AskTrust { profile, note }` over `osp/social/1`: its owner's signed profile, which must list this node, and a short note of at most 200 characters, such as "Sam, from the climbing club". Only a follower may ask.

**Trusting someone you follow.** The owner can trust a person they follow without a request: on that person's profile ("Trust") or with `osp trust add <name>`. The owner's node has that person's signed profile from following them, and trusts the nodes it lists that follow the owner. This covers a friend whose request is buried among fake ones.

**What the owner sees.** `Status` counts the requests waiting, so the app shows a badge, and `TrustRequests` lists them, newest first. The signed profile shows only that the request comes from that identity, so the name and the note are shown as what the person says, not as checked. For each request:

- the name and note, as their claims;
- whether the owner follows that identity, which is the check: the owner added that follow code themselves. Otherwise the app says the owner doesn't follow this person, and that to be sure, they can get the person's follow code in person or through another channel, follow them, and then allow;
- whether that person is already trusted on another node;
- how long the node has followed, whether it reached ours directly or through a relay lately, and how many of our files it holds a copy of.

The owner allows or declines each (`AnswerTrust`), and can take trust back later (`Untrust`). The same on the node: `osp trust requests`, `osp trust allow <name>`, `osp trust decline <name>`, `osp trust add <name>`, `osp trust remove <name>`, `osp trust list`.

**What trust changes now.** Only which followers are named as hosts. It's the owner's preference, not a claim that the video spreads faster.

- Each answer names at most 3 trusted followers among its 6, picked at random from those heard from lately; the other places are filled as now. If too few other followers hold a copy to fill them, more trusted followers do (added 2026-10-31, while building it). The 6 are then put in the usual order, direct before relayed, at random within each, so trusted followers aren't always among the first 3 that every downloader asks.
- A trusted follower not heard from lately gets no advantage: trust never outweighs signs that a node is offline. A slow one is left by the downloader like any slow host. When hosts are chosen by measured speed ([doc 13](../13-hosting-roles-and-follower-swarm.md), not built yet), that applies to trusted followers too.
- Of the 100 places kept for each file, up to 25 go to trusted followers holding it, so an owner who trusts many can't fill the list. The rest are split as now, between the followers that have followed longest and those noted last; a trusted follower past the 25 takes its chances there.

**Per person and per node.** Trust is for a person, applied to the nodes the owner allowed. A trusted person's new node isn't trusted on its own: its request shows that the person is trusted on another node, and the owner allows it with one tap, or trusts them again from their profile.

**Limits.**

- One request per node at a time; asking again while waiting replaces the note.
- At most 10 new requests an hour, and 50 waiting; when full, the oldest goes. Requests waiting 30 days go.
- A node declined is ignored for 30 days: its requests are dropped.
- Every request gets the same reply, that it arrived, whether it's new, a repeat, from a node declined or already allowed, or dropped by a limit. The asking node is never told the answer, so declining is quiet.

**Ending trust.** The owner ends it with `Untrust`. A node gives no notice when it stops following, so trust isn't taken away then. A node that stops asking for new posts is no longer heard from lately within 5 minutes, so it isn't preferred; after 30 days of silence it's no longer a follower, and its request, if waiting, goes. The trust itself stays, as the owner's judgment of the person, until the owner removes it.

**Privacy.** Trusted followers are named to the owner's other followers more often, so more of them can see that those friends follow the owner. `WhoHosts` already reveals followers this way ([doc 13](../13-hosting-roles-and-follower-swarm.md#whohosts-reveals-the-follower-list)); `osp trust allow` and the app say so before the owner allows. The note and the profile stay on the owner's node and phone.

**Data.** Database migration 11: a `trust` table with the node, its identity and name, the note, whether it is waiting, allowed or declined, and when it asked and was answered. Pending and trusted rows are listed in `osp peers` too.

## Consequences

- The owner can name people they trust first, and nobody gets trusted without the owner's yes.
- Trust is a relationship that later features can use, each under its own rule: being trusted never, on its own, shows the owner's address or grants anything else.
- A new message between nodes, three new client requests, a phone screen for requests, buttons to ask and to trust, and a database migration.
- The note is text from another node shown on the owner's phone: limited in length, shown as plain text, never as a link, and marked as what the person says.
- A crowd of fake followers can fill the request list, hiding a friend's request among them. The owner can still trust a friend they follow from that friend's profile.
- Nothing changes for a node whose owner trusts nobody.

## Validation and revisit conditions

- Unit tests: limits on requests (one per node, 10 an hour, 50 waiting, 30 days after a decline); every request gets the same reply; a request with a profile that doesn't list the node, or from a node that isn't a follower, is refused; at most 3 trusted followers of 6, not put ahead of others heard the same way; trusted followers not heard from lately aren't preferred; at most 25 of the 100 places kept for trusted followers.
- Integration tests with real nodes: a follower asks, the owner allows, and the follower is named; the owner trusts someone they follow without a request; a declined node asking again gets the same reply and isn't listed; trust removed is no longer used.
- One simulated home run with the 5 Mbit/s followers trusted: B8's marks met, and followers that aren't trusted still send copies.
- On the test nodes and a phone: B asks A, A's phone shows the request, and allowing it on the phone shows it in `osp trust list` on A.
- Revisit if fake requests turn out to bury real ones, or when M4's approved contacts are designed: trusted followers may become one of its choices, or the two one list.
