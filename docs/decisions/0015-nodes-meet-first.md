# ADR 0015: Nodes meet first: machine names, and what they may say

**Status:** Accepted (2026-11-01). Changes how [ADR 0014](0014-finding-people-and-private-messages.md)'s directories are used, and comes before its invited alpha. [ADR 0011](0011-measuring-peers.md)'s rule that a node's record of its peers stays private doesn't change.

## Context

Today every relationship between nodes starts with a person: a follow code, a request to be trusted, a pairing. A node waits to be told.

A node is on all day, while its owner sleeps, works and sees friends. It could spend that time getting to know other nodes, learning which are useful and what's out there, so that its owner has something to look at when they come back. The social part starts with the nodes, and people benefit from what their nodes have learned, not the other way round.

Two things stand in the way.

**Anything a node says under its public ID, a person says.** That ID leads to a profile. Asking "do you know anyone who posts about photography?" tells a long-term contact what its owner likes, and answering tells them whom its owner knows. [ADR 0014](0014-finding-people-and-private-messages.md) deferred asking other nodes for that reason, and because with few follows there's nobody much to ask.

**Fake nodes cost nothing.** [Doc 12](../12-peer-reciprocity-and-recognition.md) removed shared credit because fake accounts and rented servers could make or buy it, and [ADR 0013](0013-trusted-followers.md) rejected automatic trust because every condition is cheap to fake. Over Tor it's harder still: nothing shows that 100 nodes aren't one computer.

Since ADR 0014, a node reached over Tor needs only an ID, which reveals no address. So a node can have a second ID with no person behind it.

Questions to settle:

1. Who do nodes talk as?
2. What may they say, and what stays at home?
3. What do directories hold, and how are they used?
4. What can be learned about a machine, and what is done with it?
5. How can a relationship between machines become one between people?

## Options considered

- **Talking under the public ID.** No new key, but everything said is said by a person. Rejected.
- **A new key for each conversation.** Hides the most, but no relationship can form, and relationships are the point. ADR 0014's search key is this; it has its use, and comes back with word search (below).
- **A second Tor process for the second ID.** The two would be harder to match up, as they wouldn't share Tor's circuits and would fail apart, but at the cost of another process on every node. Not now; see "Revisit".
- **An always-on overlay,** such as `iroh-gossip`, with each node holding connections to a set of neighbours. Tor circuits held open all day, to keep a membership list nobody yet needs. Rejected for now.
- **Sharing reputation from the start.** With a handful of nodes there's nothing to investigate, and it rests on the problem of fake nodes, which needs the simulator first. Later, with the rules below.

## Decision

### A machine name

- **Every node on Tor has a second identity on Tor: its machine name.** It's a fresh random key, `machine.key`, derived from nothing else. No binding ties it to the public ID, and nothing sent under the public ID mentions it.
- **It serves nothing else.** It has its own endpoint, its own onion service on the node's Tor, and its own protocol, `osp/machine/1`. The machine endpoint answers no other protocol, and no other endpoint answers this one, so a mistake in one can't answer for the other.
- **It's shown as words made from the key,** never a name the owner picks, which could give them away.
- **It isn't in the backup.** A restored node starts again among machines as a stranger, and the owner can replace the key at any time, losing its history.
- **On by default on a node with Tor,** said plainly at `osp init`, and off with `[machine] enabled = false`. [ADR 0012](0012-sharing-prefetched-copies.md) asks at setup because sharing copies reveals whom someone follows. This reveals no person, but it does have a cost; see "Consequences".

### What stays at home

- **A machine says what it has, not what it wants.** No question carries an interest. Whatever is of interest to its owner is worked out at home, from what the machine has gathered.
- **A machine says only what the machine itself learned:** never anything from its owner's follows, feed, media, trusted followers or peer records. Saying "this node's media was bad" says its machine fetched from that node, and a pattern of those can identify a person. So it doesn't say that.
- **Conversations are events, not chatter.** It greets a machine it has just heard of, swaps records, checks in now and then, and closes the connection. Times are random, so they don't line up with the node's posts and syncs.

### Stages

| Stage | How it's reached | What it means |
| --- | --- | --- |
| Heard of | A signed record of the machine arrived | Kept, within limits. It may be dialled |
| Met | It answered | The two swap records of machines they've met. A local record of it starts |
| Known | A month of answers, and records that checked out | Checked in with first, and kept when the table is full |
| Introduced | Both owners said yes | Later; see below |

**Being known grants nothing:** no access, no preference for media, no say over anyone, as with trusted followers in ADR 0013. A machine is never assumed to belong to anyone the owner trusts.

### Messages

`osp/machine/1` starts with three requests, each with a capped reply:

- **`Hello`:** each side sends its machine record: its ID, what it offers (being a directory, so far) and a date, signed by its own key.
- **`Known`:** up to 20 records, picked at random from machines it has met lately. It doesn't say how or when.
- **`Listing { since, limit }`:** a directory's lists, below, in pages, as an outbox is read. Other machines say they aren't directories.

A request a machine doesn't know gets an error rather than a closed connection, so messages can be added later.

### Directories: the first acquaintance, and the map

This changes how ADR 0014's directories are used. Being listed, its limits, and who runs them stay as they are.

- **A directory holds two lists.** The map is the machine records of machines that greeted it and answered when it greeted them back. The people are those who asked to be listed: each one's signed profile, and their latest posts. The map holds no people, and neither list says who talks to whom.
- **The directory list that comes with a node is its first set of machines.** A directory's public role means it can say openly which machine name is its own.
- **A node fetches the listing and searches at home.** It reads `Listing` in the background under its machine name, checks every signature, and adds what arrives to its own index. So search works offline, and no word anyone types leaves the node. This replaces sending words to a directory for now. ADR 0014's search request and search key come back when a directory is too large to copy.
- **It starts with each listed person's profile and last 5 posts,** roughly 1 KB a profile. These stay apart from what its owner follows: they never enter the feed, they're capped, and they go first when space is short. Results say where they came from.
- **Asking to be listed goes from the public endpoint,** never the machine name, and not at the same moment as `Hello`.
- **Consent travels with the record.** The signed profile says whether it may be listed and passed on. A later profile can take that back. So a directory can check it, and later any machine can.
- **Following is still a person's act,** and as in ADR 0014 the node first fetches the profile from the author's own node.
- **Machines that have met keep meeting when the directory is off.**

### The local record: where reputation begins

- Each node keeps a record of each machine it has met: whether it answered, and whether the records it passed on checked out, meaning valid, current and answering when dialled. It fades as in ADR 0011, and as there, **it never leaves the node.**
- It says whether a machine is an honest and useful contact. It says nothing about media, which goes through the media endpoint, tied to a public ID. ADR 0011's record of public nodes stays apart from it.
- It decides whom to check in with first, and whom to keep when the table is full. A reliable computer isn't a trustworthy person, so nothing here is shown as a judgement of anyone.

### Limits

Starting values, for the simulator to adjust:

- **At most 256 machines kept.** Half the places go to those known longest, as in [doc 13](../13-hosting-roles-and-follower-swarm.md)'s swarm list, so a crowd of new names can't push them out. Records from one source can fill at most a quarter of the rest.
- **At most 6 new machines dialled an hour,** and each known machine checked in with about once a day.
- **Only signed records are dialled.** A machine signs its own record, so nobody can make up a record for a machine that never made one. Replaying a real one costs at most one of those dials.

### Later, with rules set now

None of this is built by this record. The rules are here so the steps above don't get in their way.

**Local states for peers.** Reliable, unknown, questionable, quarantined, worked out from ADR 0011's counts, for choosing whom to fetch from and serve. It needs no message. Poor performance isn't malice: a failing disk looks like a bad node. So only verified bad data can lead to quarantine, and everything fades.

**Statements.** A signed statement from one machine about another:

- It's a new, deliberate object with its own scope and expiry, not ADR 0011's record, which stays private. Doc 12's removal of shared credit stands: a statement earns nobody anything.
- A machine's statement covers only what it saw of another machine. Anything about media goes only between nodes whose owners trust each other (ADR 0013).
- It's passed on as the signed original, unchanged, and counted by who signed it, so 100 forwards are one witness. A retraction exists from the first version, and travels with it.
- Two reports from machines that look independent start a node's own check, and change nothing by themselves. Two keys can be one person, and over Tor nobody can tell, so counting keys proves nothing. Only a node's own evidence changes what it does, and a violation it verified itself acts at once.
- There's no network-wide score or ban. It steers whom to try first, never who gets access, and it never hides anyone's posts. Media is checked by hash anyway, so a wrong opinion costs time, not safety.
- First the simulator: 300 nodes, some malicious, some colluding under many names, one with a failing disk, measuring how often an innocent node is marked down.

**Introductions.** A machine and a person become linked only when both owners say yes: the node proposes, and people decide. Otherwise a patient watcher could behave well for a month and learn who everyone is. It must leave nothing the other side can show to a third party, so ADR 0014's bindings won't do, since they're built to be checked by anyone. It stays between the two nodes, and it can't be undone, as anyone can repeat what they learned. The way of proving it is chosen when this is built, from reviewed designs, and isn't invented here.

## Consequences

**Privacy.**

- A machine name shows that a machine runs this software, when it's on, and which machines it has met. A directory's map, or someone collecting `Known` replies, can map machines, not people, until something links them.
- **It isn't anonymous, and nothing may describe it so.** The machine name and the public ID share one Tor and come and go together, so someone reaching both can match them over time. Whatever a machine says, and an introduced contact, can link them too. That's why it's stated at setup and can be turned off.
- A directory sees which machine fetched its listing and when, and no search words, where under ADR 0014 it would have seen them. It can still leave people out, and a node reading one directory sees only what that directory gives it.

**Operational.**

- One more onion service, and some background traffic. How many Tor circuits it adds is measured in step 3.
- A node holds some strangers' public posts, within a cap.
- With a handful of nodes all reading one directory, meeting adds little: the alpha shows that it works and what it costs, not what it finds. It's worth more as the network grows.

**Security.** A node now dials machines it heard of from strangers, which ADR 0014's public endpoint never does, but only from signed records, within the limits above, over Tor, and on an endpoint that serves nothing else. Since being known grants nothing, fake machines gain nothing.

**Licensing.** No new dependency.

## Building it

These take the place of ADR 0014's step 4, and come before its invited alpha. Each gate is met before the next step starts.

| Step | Work | Gate |
| --- | --- | --- |
| 1 | The machine key, its onion service, `osp/machine/1` with `Hello` | It answers nothing else, and is back after a Tor restart; the leak test below passes |
| 2 | A directory's two lists; the node fetching and searching at home; a profile summary and consent in the profile; following someone found | ADR 0014's: a node with no follows finds a stranger and follows them. No typed word leaves the node |
| 3 | `Known`, the stages, the local record, the limits | In `osp-sim` with Tor-like delays: a node left alone meets others with no human act, including with the directory off; one hostile node can't fill its table; circuits per node measured |

Later, each with its own gate: local states, statements and retractions, then introductions.

## Validation

- **Leak test,** added to ADR 0014's: no public message, profile, follow code, `WhoHosts` reply, directory listing or log line ties a machine name to a public ID, and changing a node's follows changes no machine reply.
- **Search:** a stranger's profile and posts fetched from a directory are found offline; they stay out of the feed, and revoked ones drop out as in ADR 0014.
- **`osp-sim`:** step 3's gate, and that a crowd of fake names can't push out machines known longest.
- **The invited alpha,** as in ADR 0014: a few people find each other through the directory, follow and watch, without swapping follow codes.

**Revisit** if a machine name proves easy to match to its public ID in practice, which may mean a second Tor process; if the circuits cost too much; if a directory grows too large to copy, which brings word search back; or if the map of machines shows more than expected.
