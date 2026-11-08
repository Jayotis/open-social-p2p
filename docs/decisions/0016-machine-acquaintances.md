# ADR 0016: Machine acquaintances, reciprocity and referrals

**Status:** Accepted (2026-11-08); proposed 2026-11-04. Follows on from [ADR 0015](0015-nodes-meet-first.md), whose rules stand: its step 3 is this record's first stage, its local record grows here, and its statements get more detail. Direct connections are in [ADR 0018](0018-earned-direct-connections.md), not here. Stage A, with the three rules in "Keeping discovery open", was decided on 2026-11-05 and built with ADR 0015's step 3 (2026-11-06), the share of dials for sources heard from less being 2 of the 6 an hour ([results](../testing/meeting.md)). Stage A2, newcomers' turns, a greeting first and the crowd limit, was built on 2026-11-08, and missed its gate: in [the newcomer run](../testing/meeting.md#newcomers), 34 of 50 newcomers were met by 10 established nodes and 40 were passed on, short of marks 5 and 6; one operator's names not yet known were within the 40 places of those who greet at the end, but not throughout. It was merged all the same, as newcomers were met and passed on far more than before it, and stage A3 builds on it. Stage A3, introductions, with [ADR 0021](0021-introduced-machines-count-among-those-who-greet.md)'s changes, was built on 2026-11-08 and missed its gate in two runs ([results](../testing/meeting.md#introductions-results)): newcomers were met by a median of 8 established nodes, against 12 with A2 alone; at the end, nodes held newcomers through an introduction 37 times and one operator's names 378 times; its names not yet known reached 48 on one node. It was merged on 2026-11-08 with introductions off by default, as they helped one operator more than newcomers, one of this record's reasons to revisit them ("Validation"); each part can be turned on in `config.toml`. The later stages aren't built.

Revised 2026-11-07, and again on 2026-11-08 before it was accepted. Standing that a node earns by work another node checked itself now gives it service from that node, never access to content ("Service, not content"); a machine can ask one it has met to introduce it to another, one step at a time, which is how six degrees is reached ("Introductions"); references have a job, standing rather than meeting ("References"); and newcomers get turns at being met, with a limit on crowds, after [the newcomer run](../testing/meeting.md#newcomers). Each changed rule says what it replaced and why.

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
5. What does a newcomer get before it has earned anything?

## Options considered

- **A shared directory of machines, with a shared score.** Easy lookups, but heavy to copy, a public map of relationships, and a target for fake machines. Rejected.
- **Counting `Hello`, messages, votes or raw bytes as credit.** Simple, and simple to fake with a script. Rejected. Jobs a machine finished as a hop for a node, which that node checked itself, are another matter ("The local record").
- **Ranked lists of creators passed between machines.** This mixes machines with people's tastes. It belongs above this layer, in a decision of its own. Rejected here.
- **Machines serving media under their machine names, and earning direct connections that way.** Considered at length. A machine hosts what its owner follows, so it would say what [ADR 0015](0015-nodes-meet-first.md) says a machine never says. A follower would also have to tell each creator its machine key, which ties the two names together. And the evidence a machine can show is cheap. Rejected: see [ADR 0018](0018-earned-direct-connections.md).
- **Machines carrying encrypted media for others, credited by the node that received it.** Unlike hosting, a machine can't see what it carries, so it says nothing of its owner's tastes, and each byte costs real upload that the receiving node checked. Chosen in the revision, as [ADR 0017](0017-one-hop-forwarding.md)'s two hops.
- **Meeting only by dialling records passed on (`Known`).** Built in stage A, and kept. But a machine can't say "this is one I know", and a newcomer that greets is seldom met back ([the newcomer run](../testing/meeting.md#newcomers)). Introductions are added ("Introductions").
- **A signed reference as the way to meet.** It needs carrying first, so nobody could be vouched for until hops exist, and it mixes meeting a machine with earning standing from it. Kept for standing only ("References").
- **A small, local set of acquaintances, each node keeping its own record.** Chosen.

## Decision

### Three layers

| Layer | Owns | Doesn't own |
| --- | --- | --- |
| Social | Signed authors, profiles and posts, follows, trust between people | Machine reliability, routing |
| Machine | Machine names, greetings, signed records, referrals, the local record, statements, carrying encrypted media ([ADR 0017](0017-one-hop-forwarding.md)) | Follows, popularity, hosting media; addresses, except those of the nodes next to it while carrying |
| Media | Endpoints bound to a public ID ([ADR 0014](0014-finding-people-and-private-messages.md)), verified downloads, [ADR 0011](0011-measuring-peers.md)'s record, earned direct connections | Machine names |

One computer runs all three. Their names and records aren't combined because of that, except where a node shows a peer which machine is its own, to earn a direct link ([ADR 0018](0018-earned-direct-connections.md#what-counts-as-work)). A machine name is a lasting pseudonym, not a promise of anonymity.

### Service, not content

Three things are kept apart:

| | Who gets it | How | What it gives |
| --- | --- | --- | --- |
| Public posts and media | Anyone, a brand-new node included | Never earned | Finding and fetching them, through hops or relays when there's no direct link |
| A machine's standing with a node | Machines that did work that node checked itself | Jobs finished as a hop, checked; a reference gives a start | Picked as a hop, answered first when busy, part of what earns a direct link ([ADR 0018](0018-earned-direct-connections.md)), and for a machine that came in by greeting, a place outside the places of those who greet, by checked jobs alone ("Keeping discovery open") |
| Trusted followers ([ADR 0013](0013-trusted-followers.md)) | People the owner chose | A person's act | Hosting today, and the only gate on any private posts later |

Standing never entitles anyone to private posts, and having none never shuts anyone out of public ones, so no content is worth buying standing for. **A node is a proper computer on a proper internet connection** (2026-11-08, as [ADR 0006](0006-desktop-first-always-on-node.md)'s always-on node), so every node can carry; a phone is its owner's window onto their node, not a node. This replaces "a node that can't carry traffic, on a metered link or a small upload, is still a full member of the network": such a link is too rare to shape the rules around, and a rule that let those who can't carry get by on time alone would let a crowd of names do the same. An owner who pauses carrying still gets every public post. [ADR 0020](0020-sharing-media-you-open.md) tells peers with standing which public posts a node holds copies of, where its owner turned that on: a lead to public posts, not access to anything, and sent between nodes' public sides, not from machine names.

### Taking part isn't earned

A node with no history gets, from its first minute:

- every public post and video, as above;
- answers from machines that aren't busy, and the stranger share from those that are ([doc 12](../12-peer-reciprocity-and-recognition.md));
- turns at being met and passed on, and introductions through the machines it has met ("Introductions" and "Keeping discovery open", below);
- turns carrying in the middle of other nodes' transfers, where it sees neither end ([ADR 0017](0017-one-hop-forwarding.md#who-picks)), so it can start earning standing.

What's earned is service: being picked by hosts, priority when busy, direct links, and for a machine that came in by greeting, a place outside the crowded places of those who greet. A new name starts with nothing, the lowest any machine gets, so making new names never improves the service a node gets, as in doc 12.

### After a greeting

- `Hello` stays as built. Either side may start it, and meeting at the same moment doesn't count twice.
- **What a record offers is a claim,** not proof of service, skill or independence.
- **A greeting proves the machine answered, once.** It earns at most a small note that it was there.
- **Conversations have a purpose,** a limit and an end. There's no regular chatter and no open gossip network. Being busy, offline, or without a record isn't bad behaviour.
- A machine says only what ADR 0015 lets it say. It never says what comes from its owner's follows, feed, media, searches or peers, except a reference's rough count of jobs another machine finished for it ("References").

### Meeting more machines

Every node keeps a small set of machines it knows, not a table of the world: ADR 0015's 256 machines, 6 new dials an hour and a check-in about once a day, each a starting value.

- **A node meets others by asking those it knows.** A asks B with `Known`, gets up to 20 signed records, and may dial C. C is unknown until A has met it. B carries nothing after that.
- **`Known` stays random.** Choosing which records to return would show what's in the private record. A machine names another only in an introduction, and only one that the machine it asks passed on to it ("Introductions").
- **A step is a dial or an introduction.** An introduction goes one step, from the machine asked to the machine named, and no further, so no message travels through the network and there's no traversal to build. How far nodes get is measured as dials needed, within the limit above.

Six degrees isn't a guarantee, and being close isn't being trusted.

### Introductions

[ADR 0021](0021-introduced-machines-count-among-those-who-greet.md) changes two rules here (2026-11-08): a machine introduced counts against those who greet, not against its introducer's source, and is dialled first among them; and an introducer that Kim still counts among those who greet introduces no one.

Jimmy's machine has met Alice's, and Alice's knows Kim's. Jimmy asks Alice to introduce it to Kim, Alice passes the request on, and Kim decides. Six degrees is this, repeated: once Jimmy has met Kim, it can ask Kim the same way. Nothing here is earned: it takes no standing, and no history beyond having met the machine asked.

- **Jimmy names only a machine whose record Alice gave it** in `Known`, picked by the machine's own rules, at random or from sources heard from less, never from its owner's follows, feed or searches. So Alice learns which of her own records Jimmy picked, and nothing about its owner, and ADR 0015's "a machine says what it has, not what it wants" holds: no request carries an owner's interest.
- **Alice passes it on only to a machine she has met,** at most 3 a day for one machine asking and 12 a day in all (starting values). She tells Jimmy only whether she passed it on, with no reason.
- **Kim decides with no human act,** from her own record of Alice: she accepts if she has known Alice for at least 3 days (a starting value) and nothing Alice passed on has failed her checks. To accept, Kim greets Jimmy; to decline, she does nothing. Alice never learns the answer, and Jimmy can't tell a decline from Kim being busy or offline.
- **Accepted introductions come out of the 6 dials an hour,** taking the newcomer turn ("Keeping discovery open") ahead of machines that only greeted. They add no circuits, and Kim's own discovery isn't crowded out.
- **A machine met through an introduction counts against its introducer's source,** as a record Alice passed on would, so one introducer fills at most its quarter of Kim's table. An introducer first met through its own greeting counts against those who greet, as below.
- **What it gets Jimmy:** Jimmy has Kim's record, so it could greet Kim anyway, and the newcomer run showed what that gets a stranger: seldom being met back. An introduction gets Kim to dial Jimmy in her newcomer turn, ahead of machines that only greeted, and to count it under Alice's share of her table instead of the crowded places of those who greet. No standing, no priority, no address. If Jimmy then passes Kim bad records, Kim relies less on Alice's introductions ("Referral accuracy", below), and that fades as the record does, so one bad day doesn't shut Alice out for good.
- **Each request carries an ID, an expiry and Jimmy's signature,** and Alice passes on Jimmy's own signed record. Kim checks that record herself, drops a request she has seen before, and counts an introduction of Jimmy once, however many machines pass it on, so asking through several introducers doesn't get round her limit.
- **Each owner can turn it off,** asking, passing on or accepting, in `config.toml`.
- These link two machines, under machine names only. [ADR 0015](0015-nodes-meet-first.md)'s introductions, which link a machine to a person with both owners' yes, are another thing and don't change.

**Who learns what:** Alice, that Jimmy picked Kim from her records; Kim, that Alice knows Jimmy; Jimmy, whether Kim greeted. This replaces "a machine doesn't ask for a machine by name", whose reason was that a name shows what a node wants. A name picked by the machine's rules from Alice's own records shows only the pick.

### Keeping discovery open

**A crowd of fake machines can still crowd out honest ones.** Being known grants nothing, and standing comes only from work honest nodes checked themselves, so being known gets them no address and no access. What they can do is become the only voice a node hears: answering reliably under many names, they fill its table and use up its dials, and it stops meeting honest machines and directories. This is an eclipse, [shown on Bitcoin](https://www.usenix.org/conference/usenixsecurity15/technical-sessions/presentation/heilman). Here it takes a server running many names over Tor for weeks, and pays little: the node can't find others, and the operator sees which machines it talks to. Unlikely in the alpha; it matters more once discovery feeds anything else, and with the revision it feeds introductions and newcomers' turns in the middle of transfers ([ADR 0017](0017-one-hop-forwarding.md#who-picks)): a crowd that fills tables gets more of both.

ADR 0015's limits are the main defence: half of the 256 places stay with the machines known longest, and one source can fill at most a quarter of the rest. On top of those, three rules, built with ADR 0015's step 3:

- **Where a record came from stays counted** after its machine has been met, so meeting it doesn't free a place in its source's quarter.
- **Some of the six dials an hour go to records from sources that supplied few of the table's machines,** with no extra dials. The share is set before the simulator run.
- **Directories set in `config.toml` are kept apart** from machines met through others, so a full table can't push them out, and the node goes back to them when the rest fail.

**Newcomers.** [A run with 300 honest nodes](../testing/meeting.md#newcomers) found that newcomers arriving once tables were full met others but were seldom met back: by a median of 2 established nodes each, and none was passed on. A node dials what it heard of first, and a machine that greets is heard of last, so a newcomer lost its place before it was ever dialled. Three more rules, built for stage A2:

- **1 of the 6 dials an hour goes to a machine that greeted and hasn't been met,** picked at random. When the places for those who greet are full, or the table is, a machine that greets takes one of 8 places in turn, in place of the one that took a turn longest ago.
- **A crowd that came in by greeting stays within those places.** Records passed on by a machine first met through its greeting count against those who greet, not as a source of their own, until that machine has carried jobs for this node that this node checked itself. Until then it gets no place kept longest either. A reference doesn't let it out: it's another machine's word, not work this node checked, so it only gets the machine a turn at carrying sooner ("References"). Being known isn't enough: answering under many names costs one server little, while carrying costs real upload, and every node can carry ("Service, not content"). This replaces "until that machine is known" (2026-11-08). Carrying isn't built yet, so until it is, being known stands in, as in the runs below.
- **A greeting comes before what greeters pass on.** When those places are full, a machine that greets takes the place of a record passed on through one of them, one never met first, then the one heard of longest ago, and takes a turn only when none is left. A crowd's names pass on one another; a newcomer has none to pass on, so the crowd gives way.

The first rule alone, in the same run, got each newcomer met by 10 nodes instead of 2, and 30 of 50 passed on instead of none. But one operator gained more: 47 of its 100 names were held instead of 1, as its names passed on one another, each counted as a source with places of its own. A newcomer has no other names to pass on, so the second rule costs it nothing. It should also keep the dials for less-heard sources from favouring a crowd's many small sources.

Stage A2 ran all three, with being known standing in for checked work ([results](../testing/meeting.md#newcomers)). Each newcomer was met by a median of 12 established nodes, and 40 of 50 were passed on, short of marks 5 and 6 and still rising when the run ended: newcomers kept their places and waited for the dial back. A node still held a median of 50 of the operator's names, but 19 of them were known and let out by the stand-in. Those not yet known were at most 40 on any node at the end, and up to 58 during the run, let in by names already known and by honest machines (below).

**Honest machines pass a crowd's names on too.** A node that met one of the operator's names through its greeting passes it on like any machine it met, and a node that hears it from there counts it against that honest machine, not against those who greet: `Known` says nothing of how a machine was met, by design. Before any of the operator's names could be known, this put at most 4 of 300 nodes over the 40 places, with at most 46 of its names. It's stated as a limit: the crowd limit holds what came in through greetings, not what honest machines pass on.

**Introductions can carry a crowd in too:** an operator's names that a node has known for days, introducing the operator's new names. Under [ADR 0021](0021-introduced-machines-count-among-those-who-greet.md), each counts among those who greet, an introducer still among them introduces no one, and a node accepts at most one an hour. A crowd can also use up an introducer's daily limit, leaving honest newcomers waiting. Stage A3 measured both ([results](../testing/meeting.md#introductions-results)): the operator's names, asking every honest machine they greeted, were held through introductions 378 times at the end, against newcomers' 37, and newcomers were met by fewer nodes than with A2 alone.

Several names can still be one operator, so these make an eclipse harder, not impossible. A node whose only start is hostile is cut off from honest machines. Nothing here fixes that, and it's stated as a limit.

### The local record

Each node keeps, for each machine it knows, these things apart:

| Measure | What it saw for itself | Used for |
| --- | --- | --- |
| Reachability | Answers over time | When to try again |
| Integrity | Records that were valid and turned out current | Whom to ask first |
| Usefulness | New, correct records it needed; introductions that worked | Whom to check in with, and keep when full |
| Reciprocity | Help received and help given, kept apart | Who is answered first when busy |
| Referral accuracy | Whether what it said held up, introductions and references included | How much to rely on it next time |
| Carrying | Transfers it carried as a hop for this node ([ADR 0017](0017-one-hop-forwarding.md#credit)) that completed and passed the hash check, each above a minimum size, on different days; and those it dropped | Whom to pick as a hop; who is answered first when busy; part of what earns a direct link, once its owner shows the machine is theirs ([ADR 0018](0018-earned-direct-connections.md)) |

- **It stays on the node** and fades, as in ADR 0015. Two nodes can think differently of one machine.
- **There's no score, no tokens and no transferable credit.** Honesty and usefulness aren't interchangeable, and a slow link isn't malice.
- **A closed group convinces only itself.** Machines crediting one another gain nothing with a node that hasn't worked with them, as each node counts only its own experience. That protects standing, and the way out of the places of those who greet, which takes work the node checked. Within those places a crowd needs nobody's trust to fill them and take newcomers' turns ("Keeping discovery open").
- **These earn nothing:** a `Hello`, a duplicate, chatter, claimed uptime, a chain of references, jobs nobody checked. Hearing the same thing again is worth little or nothing.
- **Records and introductions that held up** decide whom to ask first and whom to keep, as before. **Only jobs carried earn service,** as each costs real upload. A reference (below) gives a start, not standing.
- **When not busy, a machine answers normally.** When busy it favours those with standing, and always keeps the stranger share for newcomers ([doc 12](../12-peer-reciprocity-and-recognition.md)).
- **Local states:** `reliable`, `unknown`, `questionable`, `quarantined`. Only abuse a node verified itself can quarantine. Marks are set from tests.

**What this decides:** whom to ask first, whom to keep, whom to answer first when busy, whom to pick as a hop, and, with ADR 0018, part of what earns a direct link. **What it never decides:** whether anyone can find or fetch a public post, who sees private posts (only [ADR 0013](0013-trusted-followers.md)'s trusted followers, a person's act), or anyone's address on its own.

This changes ADR 0015's "being known grants nothing", which was the whole rule until now. It kept fakes from gaining anything by answering greetings and passing on records, which cost nothing, and being known still grants nothing. Carrying isn't free: each job costs the carrier real upload, and the node crediting it checked every chunk. Doc 12 removed creator and scout standing buying priority, as priority is zero-sum and would reward virality with bandwidth. Here a node gives priority only for work done for itself, as doc 12's own generous tit-for-tat does, and the stranger share stays. A fake that carries real bytes for honest nodes is doing real work; what it can get for that is priced in [ADR 0018's check](0018-earned-direct-connections.md#validation).

### Statements

As ADR 0015 set out, with these details:

- A statement is new, deliberate, signed, narrow and expiring, and says only what the machine saw of another machine. ADR 0011's record isn't exported.
- It's passed on as the signed original, so ten forwards from one signer are one witness. Corrections and retractions exist from the first version.
- **Two first-hand reports from machines already known start a node's own check,** and change nothing by themselves. Two keys may be one person.
- That check can re-read records, try something harmless, ask for an explanation and ask a few established contacts. A timeout, a stale reply or a different opinion isn't misconduct.
- Only a node's own evidence changes what it does. There are no network-wide bans.

### References

A machine that has worked with another can sign a short reference for it. It gives the machines it's shown to a reason to try the one it names, and that one a chance to show it's reliable. It passes on no trust and no access. It's for standing, not for meeting, which is what introductions do. The first draft kept this for later, until it had a job: this is the job.

- **What it says:** both machine names, how many days the signer has known the other, the jobs it finished as a hop for the signer's node, checked there, in rough bands, a date, and an expiry 14 days later. Nothing about either owner, and nothing about what was fetched or from whom. It's the one thing a machine says that comes from its owner's media, as the carrying was the other machine's own work.
- **The machine named asks for it,** and shows up to 3 when it greets or is greeted. The signer doesn't learn where it's shown.
- **A node counts a reference only from a machine with carrying standing in its own record,** weighted by that record. One from a machine it knows only through another reference counts for nothing: references don't chain. Nodes reach six degrees through introductions and dials ("Introductions"); trust doesn't travel with them.
- **What it gets the machine named, from that node:** a turn in the middle of its transfers sooner ([ADR 0017](0017-one-hop-forwarding.md#who-picks)), answered sooner when busy, and less first-hand work before a direct link ([ADR 0018](0018-earned-direct-connections.md#what-counts-as-work)). Never a place next to it as a hop, never a direct link outright, never a way out of the places of those who greet ("Keeping discovery open"), and never access to anything.
- **If the machine named then fails that node,** with bad data or by dropping what it was picked to carry, the node relies less on its signer's references (referral accuracy, above).
- **A node counts at most 8 references from one signer at a time,** a starting value, so one careless or bought signer lifts only a few.

[Doc 12](../12-peer-reciprocity-and-recognition.md) removed "vouched" credit because fake accounts and rented servers could make or buy it. Here fakes can sign for each other, but a node counts only signers that did real work for it; a signer that sells references loses weight with each node where a buyer fails; and a reference gets only a start, after which the node's own record decides. What it shows, to the node it's shown to, is that two machines worked together, under machine names. Untested: whether references get newcomers going sooner, and what fake signers gain. Both are measured in ADR 0018's step 2 and its price check.

### Messages

`osp/machine/1` keeps its bounded single request and reply. Names below are sketches.

| Message | Status | Role |
| --- | --- | --- |
| `Hello` | Built | Meeting, with a signed record |
| `Listing` | Built | A directory's two lists |
| `Known` | ADR 0015 step 3 | Up to 20 random signed records |
| `Introduce`, `Introduction` | Stage A3 | Asking a machine to introduce one to a machine it passed on; passing that on, with the asker's signed record |
| `Statement`, `Correction` | Stage D | A warning, and its follow-up or retraction |
| `Reference` | Stage E | Asked of a machine by the one it would name; up to 3 shown with `Hello` |

Before any of these is built: signing domains, expiry, replay handling, frame limits and privacy tests. No media and no direct-connection negotiation goes through this protocol.

## Consequences

**Privacy.** A machine's records, its times and who it has met can link machines, and two names sharing one Tor can be matched over time, as ADR 0015 says. A statement, a reference or an introduction shows that one machine dealt with another, and an introduction shows the machine asked which of its records the asker picked. A machine that carries shows its address to the nodes next to it ([ADR 0017](0017-one-hop-forwarding.md)). Requests carry no searches or follows.

**Abuse.** Anyone can make machine keys, and an old name isn't an independent operator. Standing now gives service, so it's worth more to fakes than before. It comes only from work honest nodes checked themselves, and gives no access to content, so a fake gains priority and hop picks only by finishing real jobs. A crowd of names gets more newcomers' turns in the middle of transfers, which show it carriers' addresses and no end's, and can introduce its own names, each within its introducer's place; ADR 0018's price check and stage A3 measure these.

**Operational.** Peers, referrals and evidence all stay bounded. Discovery keeps working when a directory goes away. Stage A's Tor circuits: about 400 a node a day in `osp-sim` among 100 nodes, and over real Tor 55 a day for the machine name of a test node with two others to meet, most of them to keep its onion service up ([counts](../testing/tor.md#machines-meeting)). Each later stage adds to it.

**Changes to accepted records.** "Keeping discovery open" adds three rules to ADR 0015's limits. They're built with its step 3, and named in ADR 0015 then. The revision adds three more, built in stage A2 and named in ADR 0015's limits. One narrows ADR 0015's places kept for the machines known longest: a machine that came in by greeting gets one only once it has done work the node checked. The revision also changes ADR 0015's "being known grants nothing" as "The local record" says: being known still grants nothing, and jobs carried earn service. It answers doc 12's removal of vouched credit and of standing buying priority, above. Introductions keep ADR 0015's "a machine says what it has, not what it wants", as above, and replace this record's own "a machine doesn't ask for a machine by name". ADR 0013 doesn't change. The rest adds detail to ADR 0015.

**Licensing.** No new dependency.

## Building it

| Stage | Work | Gate |
| --- | --- | --- |
| A | ADR 0015 step 3: `Known`, stages, local record, limits, and the three rules in "Keeping discovery open" | ADR 0015's: nodes meet others with no human act, including with the directory off; one hostile node can't fill a table; circuits measured. Also: fake names under several keys, referring one another, can't push out the machines known longest |
| A2 | Newcomers' turns, a greeting first, and the crowd limit, with being known standing in for checked work ("Keeping discovery open") | [The newcomer run](../testing/meeting.md#newcomers), once: its marks 4 to 6 met, and one operator's names not yet known held within the 40 places of those who greet, as names known are let out by the stand-in. Missed on 2026-11-08, in two runs: marks 5 and 6, and the operator's mark met at the end only |
| A3 | Introductions, after A2, without waiting for hops | The newcomer run, once, with newcomers asking machines they met: newcomers met by more established nodes than with A2 alone; one operator's names, introducing one another, held within A2's mark; honest newcomers still introduced while the operator uses up introducers' limits. Missed on 2026-11-08, in two runs: all three, and marks 5 and 6. Merged with introductions off by default |
| B | The record's separate measures, fading, with carrying; checked work, not being known, lets a machine out of the places of those who greet | Valid referrals earn a place; greeting spam and claims earn nothing; jobs carried count only when the receiving node checked them, and dropped ones count against; the newcomer run, with an operator whose names answer but carry nothing: all its names, known or not, held within the 40 places of those who greet, beyond those honest machines passed on (A2's stated limit), and newcomers that carry let out, while a reference alone lets none out |
| C | Using it when busy | Newcomers still get answers and the stranger share; measured against fake machines |
| D | Statements, corrections and two-report checks | False reports and outages don't mark down innocent machines without a check |
| E | References | Signers can't be spoofed; references from machines without carrying standing, or through another reference, change nothing; ADR 0018's price check |

## Validation

In `osp-sim`: at least 300 ordinary nodes and 100 fake keys, several under one operator, with colluding referrals and introductions, offline machines, a failing disk, false accusations, a split network, returning newcomers, and newcomers arriving once tables are full. Measure dials to meet a machine, useful records per request, Tor circuits, database size, innocent machines marked down, and fairness to newcomers. ADR 0015's leak test still applies: nothing public ties a machine name to a public ID, and only a peer a node chose to show its machine to ([ADR 0018](0018-earned-direct-connections.md#what-counts-as-work)) learns it.

Stage A runs ADR 0015's gate once, with some hostile names under one operator referring one another, and reports how many honest machines each node kept. Circuits counted in `osp-sim` are an estimate; over real Tor, a test node's machine name built 55 a day with two others to meet ([counts](../testing/tor.md#machines-meeting)).

**Revisit** if acquaintances split into cliques, if one operator's names crowd out the rest, if introductions let a crowd in faster than newcomers, if the record shuts out newcomers, if references help fakes more than newcomers, or if two-report checks often mark down the innocent.
