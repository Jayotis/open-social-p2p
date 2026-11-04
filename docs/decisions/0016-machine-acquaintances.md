# ADR 0016: Machine acquaintances, reciprocity and referrals

**Status:** Proposed (2026-11-04). Not built. Follows on from [ADR 0015](0015-nodes-meet-first.md), whose rules stand: its step 3 is this record's first stage, its local record grows here, and its statements get more detail. Direct connections are in [ADR 0018](0018-earned-direct-connections.md), not here.

## Context

OSP has two kinds of relationship.

- **People** find people through follow codes, public profiles and posts, reshares and searches. That is the social layer.
- **Machines** find machines through greetings, repeated technical contact and introductions from other machines. This isn't the owners' follow graph, and it must not grow into a full map of the network.

ADR 0015 built a machine name, `Hello` and a directory's map of machines. Its step 3 brings `Known`, the stages, a local record and limits. It doesn't say what counts as useful, what a machine does with what it learned, or how a warning is handled.

Questions to settle:

1. How do machines learn of others without a giant directory?
2. What counts as an acquaintance, a useful interaction and a good referral?
3. What is that knowledge used for, and where does it stop?
4. How are warnings checked and corrected without bans?

## Options considered

- **A shared directory of machines, with a shared score.** Easy lookups, but heavy to copy, a public map of relationships, and a target for fake machines. Rejected.
- **Counting `Hello`, messages, votes or raw bytes as credit.** Simple, and simple to fake with a script. Rejected.
- **Ranked lists of creators passed between machines.** This mixes machines with people's tastes. It belongs above this layer, in a decision of its own. Rejected here.
- **Machines serving media under their machine names, and earning direct connections that way.** Considered at length. A machine hosts what its owner follows, so it would say what [ADR 0015](0015-nodes-meet-first.md) says a machine never says. A follower would also have to tell each creator its machine key, which ties the two names together. And the evidence a machine can show is cheap. Rejected: see [ADR 0018](0018-earned-direct-connections.md).
- **A small, local set of acquaintances, each node keeping its own record.** Chosen.

## Decision

### Three layers

| Layer | Owns | Doesn't own |
| --- | --- | --- |
| Social | Signed authors, profiles and posts, follows, trust between people | Machine reliability, routing |
| Machine | Machine names, greetings, signed records, referrals, the local record, statements | Follows, popularity, media, anyone's address |
| Media | Endpoints bound to a public ID ([ADR 0014](0014-finding-people-and-private-messages.md)), verified downloads, [ADR 0011](0011-measuring-peers.md)'s record, earned direct connections | Machine names |

One computer runs all three. Their names and records aren't combined because of that. A machine name is a lasting pseudonym, not a promise of anonymity.

### After a greeting

- `Hello` stays as built. Either side may start it, and meeting at the same moment doesn't count twice.
- **What a record offers is a claim,** not proof of service, skill or independence.
- **A greeting proves the machine answered, once.** It earns at most a small note that it was there.
- **Conversations have a purpose,** a limit and an end. There's no regular chatter and no open gossip network. Being busy, offline, or without a record isn't bad behaviour.
- A machine says only what ADR 0015 lets it say. It never says what comes from its owner's follows, feed, media, searches or peers.

### Meeting more machines

Every node keeps a small set of machines it knows, not a table of the world: ADR 0015's 256 machines, 6 new dials an hour and a check-in about once a day, each a starting value.

- **A node meets others by asking those it knows.** A asks B with `Known`, gets up to 20 signed records, and may dial C. C is unknown until A has met it. B carries nothing after that.
- **`Known` stays random.** Choosing which records to return would show what's in the private record. A machine doesn't ask for a machine by name either, as that shows what it wants.
- **A "hop" is only another dial,** so there's no message that travels through the network and no traversal to build. How far nodes get is measured as dials needed, within the limit above.

Six degrees isn't a guarantee, and being close isn't being trusted.

### The local record

Each node keeps, for each machine it knows, these things apart:

| Measure | What it saw for itself | Used for |
| --- | --- | --- |
| Reachability | Answers over time | When to try again |
| Integrity | Records that were valid and turned out current | Whom to ask first |
| Usefulness | New, correct records it needed; introductions that worked | Whom to check in with, and keep when full |
| Reciprocity | Help received and help given, kept apart | Who is answered first when busy |
| Referral accuracy | Whether what it said held up | How much to rely on it next time |

- **It stays on the node** and fades, as in ADR 0015. Two nodes can think differently of one machine.
- **There's no score, no tokens and no transferable credit.** Honesty and usefulness aren't interchangeable, and a slow link isn't malice.
- **These earn nothing:** a `Hello`, a duplicate, chatter, claimed uptime, a chain of references, raw bytes. Hearing the same thing again is worth little or nothing.
- **When not busy, a machine answers normally.** When busy it may favour those it knows, and always leaves room for newcomers.
- **Local states:** `reliable`, `unknown`, `questionable`, `quarantined`. Only abuse a node verified itself can quarantine. Marks are set from tests.

**What this decides:** whom to ask first, whom to keep, whom to answer first when busy. **What it never decides:** access, anyone's address, or whether a person's posts are seen. ADR 0015's "being known grants nothing" stands.

### Statements

As ADR 0015 set out, with these details:

- A statement is new, deliberate, signed, narrow and expiring, and says only what the machine saw of another machine. ADR 0011's record isn't exported.
- It's passed on as the signed original, so ten forwards from one signer are one witness. Corrections and retractions exist from the first version.
- **Two first-hand reports from machines already known start a node's own check,** and change nothing by themselves. Two keys may be one person.
- That check can re-read records, try something harmless, ask for an explanation and ask a few established contacts. A timeout, a stale reply or a different opinion isn't misconduct.
- Only a node's own evidence changes what it does. There are no network-wide bans.

### Later: references and vouches

The idea: a machine names a few willing referees, each of whom signs that it agrees, for that subject and for a limited time, and answers narrow questions about what it saw. "Did this machine return valid records when you asked?" is a fair question. "Is its owner a good person?" isn't.

It's kept here but not decided, because it needs a job first. It can't grant access or replace first-hand evidence, so before building it, this must say which decision a reference would change. It also draws on a record that otherwise never leaves the node, and it shows whom a machine knows to anyone who asks. [Doc 12](../12-peer-reciprocity-and-recognition.md) removed "vouched" credit because fakes could make or buy it, so that reason has to be answered too. A referral could help with discovery, as a hint of whom to try first.

### Messages

`osp/machine/1` keeps its bounded single request and reply. Names below are sketches.

| Message | Status | Role |
| --- | --- | --- |
| `Hello` | Built | Meeting, with a signed record |
| `Listing` | Built | A directory's two lists |
| `Known` | ADR 0015 step 3 | Up to 20 random signed records |
| `Statement`, `Correction` | Proposed | A warning, and its follow-up or retraction |
| `References`, `AskReference` | Later | Only if the question above is answered |

Before any of these is built: signing domains, expiry, replay handling, frame limits and privacy tests. No media and no direct-connection negotiation goes through this protocol.

## Consequences

**Privacy.** A machine's records, its times and who it has met can link machines, and two names sharing one Tor can be matched over time, as ADR 0015 says. A statement shows that one machine dealt with another. Requests carry no searches or follows.

**Abuse.** Anyone can make machine keys, and an old name isn't an independent operator. Since this record grants no access, fakes gain little: at most they get asked first. That is why the stakes stay low.

**Operational.** Peers, referrals and evidence all stay bounded. Discovery keeps working when a directory goes away. How many Tor circuits this costs is measured in stage A, and each later stage adds to it.

**Changes to accepted records.** None. This adds detail to ADR 0015.

**Licensing.** No new dependency.

## Building it

| Stage | Work | Gate |
| --- | --- | --- |
| A | ADR 0015 step 3: `Known`, stages, local record, limits | ADR 0015's: nodes meet others with no human act, including with the directory off; one hostile node can't fill a table; circuits measured |
| B | The record's separate measures, fading | Valid referrals earn a place; greeting spam and claims earn nothing |
| C | Using it when busy | Newcomers still get answers; measured against fake machines |
| D | Statements, corrections and two-report checks | False reports and outages don't mark down innocent machines without a check |
| E | References, if they've found a job | Referees can't be spoofed; fake endorsements change nothing on their own |

## Validation

In `osp-sim`: at least 300 ordinary nodes and 100 fake keys, several under one operator, with colluding referrals, offline machines, a failing disk, false accusations, a split network and returning newcomers. Measure dials to meet a machine, useful records per request, Tor circuits, database size, innocent machines marked down, and fairness to newcomers. ADR 0015's leak test still applies: nothing ties a machine name to a public ID.

**Revisit** if acquaintances split into cliques, if one operator's names crowd out the rest, if the record shuts out newcomers, or if two-report checks often mark down the innocent.
