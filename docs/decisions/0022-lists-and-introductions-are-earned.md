# ADR 0022: Lists and introductions are earned

**Status:** Proposed (2026-11-08), from the maintainer's decisions after [ADR 0016](0016-machine-acquaintances.md)'s stage A3 missed its gate. It changes [ADR 0015](0015-nodes-meet-first.md)'s `Known` reply, and makes ADR 0016's introductions and references one feature, replacing [ADR 0021](0021-introduced-machines-count-among-those-who-greet.md). Each change says what it replaces. Built after carrying, with ADR 0016's stage B. Until then, `Known` works as it does now, and introductions stay off by default.

## Context

In stage A3's runs ([results](../testing/meeting.md#introductions-results)), introductions helped one operator's names more than newcomers. Under ADR 0016's rules:

1. Jimmy, a new machine, contacts Alice, an established one. Alice sends him a list of up to 20 machines she has talked to lately (`Known`). She does that for any machine that contacts her.
2. Jimmy asks Alice to introduce him to Kim, one of the machines on that list.
3. Alice checks only that she has talked to Kim lately. She checks nothing about Jimmy: "Nothing here is earned" (ADR 0016).
4. Kim checks Alice: has Kim herself contacted Alice, 3 days ago or more? If so, she contacts Jimmy sooner.

Alice vouched for nobody. She passed on requests from anyone, so 100 names run by one operator did what newcomers did, only more often. They were held through introductions 378 times, against newcomers' 37. And Alice passed requests to machines she had contacted, which often hadn't contacted her, so Kim turned down over half of them.

The rule was wrong, not the run. An introduction means something only if Alice knows Jimmy from work he did for her, and Kim knows Alice the same way. A newcomer that knows nobody can still start. It carries encrypted traffic in the middle of other nodes' transfers, where it sees neither end ([ADR 0017](0017-one-hop-forwarding.md#who-picks)). That's how it meets machines: by finishing jobs for them.

Lists have the same problem. Any machine can ask any other for its list, all day. A crowd of names can collect the network's map that way and greet everyone on it. ADR 0015 made lists open so that nodes could find each other with no human act, before carrying was designed. Now that carrying gives newcomers a way in, lists can be earned too. The maintainer's earlier choices here were made to keep building; the failed runs are the reason to change them.

## Decision

### Work first

Every rule below rests on **work Alice's node checked itself,** as [ADR 0018](0018-earned-direct-connections.md#what-counts-as-work) defines it. That means transfers Jimmy's machine carried for her as a hop, each chunk checked, and, once Jimmy's node has shown Alice which machine is its own, downloads from it. Greetings, records, uptime, anything a machine says of itself, and another machine's word don't count.

**The bar is work on 3 different days, with no bad data:** ADR 0018's layer 2, and the bar a viewer already uses to pick the hop next to it (ADR 0017). It's a starting value for the simulator. Standing fades, as in ADR 0018, so a machine that stops working drops back below it.

### A newcomer that knows nobody

1. Jimmy reads a directory's map (ADR 0015) and greets some of the machines on it. Greeting stays open to anyone, as it's what puts Jimmy in Alice's table, among those who greet ([ADR 0016](0016-machine-acquaintances.md#keeping-discovery-open)).
2. On 1 transfer in 10, Alice, fetching video through hops, puts a machine with no standing from her table in the middle ([ADR 0017](0017-one-hop-forwarding.md#who-picks)). Jimmy is one she can pick. He sees neither end.
3. Alice checks and credits each job Jimmy finishes. After work on 3 different days, Jimmy has standing with her.
4. Now Alice gives Jimmy her list, and he can ask her for an introduction.

A friend makes it quicker. A node approved by hand is a direct peer from the first day ([ADR 0018](0018-earned-direct-connections.md#a-fifth-rule)), so the friend's node can pick Jimmy to carry for it from the start, and that work counts like any other. Nobody has to know anyone to join.

### Lists go only to machines that worked for you

**Alice answers `Known` only for a machine with standing in her own record,** and for one machine no more than once a day (a starting value). Anyone else gets an empty list.

**What this replaces:** in ADR 0015, "Met: the two swap records of machines they've met" and a `Known` reply for any machine that asks. In ADR 0016, "A node meets others by asking those it knows".

**What stands:** the reply itself, up to 20 records picked at random from machines that answered in the last 7 days, never the one asking. Every limit on what a node takes in also stands: a quarter of the table for one source, the places of those who greet, and the crowd limit.

**Why:** a node doesn't trust a machine it doesn't know. A list is a map of whom Alice talks to. Given to anyone, it's how a crowd finds everyone to greet, and every stranger who asks costs Alice a reply. Given only for checked work, it costs the asker days of real upload.

**Directories are the exception.** A directory's map goes to anyone, as that's its public role, and it's where a newcomer that knows nobody starts. A directory can limit how often one machine reads it.

### Introductions are references

ADR 0016 had two features: introductions, for meeting, which needed no standing, and references, for standing, built later. They're now one, called an introduction.

1. **Jimmy asks Alice,** once he has standing with her. He names nobody.
2. **Alice decides from her own record of Jimmy,** with no human act: work on 3 different days, no bad data.
3. **Alice signs it.** It names both machines and says how long she has known Jimmy and the jobs he finished for her, in rough bands. It has a date and an expiry 14 days later, and carries Jimmy's own signed record. It says nothing about either owner, or what was carried, or for whom. This is ADR 0016's reference, unchanged.
4. **Alice sends it only to machines that picked her to carry for them,** at most 3 for one machine introduced and 12 a day in all (starting values), in a conversation they're having anyway. She tells Jimmy only whether she sent it. Those machines have Alice in their own records, because they chose her.
5. **Kim weighs it by her own record of Alice,** as ADR 0016 weighs references: only from a machine with carrying standing with her, and at most 8 from one signer at a time.

**What it gets Jimmy from Kim:**
- A turn in the middle of her transfers sooner. That's the low-trust job, where he sees neither end.
- Answered sooner when Kim is busy.
- Less first-hand work before a direct link (ADR 0018).

Kim holds his record among those who greet, as ADR 0021 said, until he has carried jobs she checked herself. He never gets a place next to her as a hop, her list, a direct link or access to anything that way. Those he earns from Kim first-hand, as he did from Alice.

**It goes one step.** Kim doesn't pass Alice's word on. She can introduce Jimmy herself only once he has standing with her. If Jimmy then fails Kim, with bad data or by dropping what he was picked to carry, she relies less on Alice's introductions (ADR 0016's referral accuracy). That fades, so one bad day doesn't shut Alice out for good.

**What this replaces:**
- **In ADR 0016:** "Nothing here is earned: it takes no standing"; "Jimmy names only a machine whose record Alice gave it"; "Alice passes it on only to a machine she has met"; Kim accepting from 3 days of knowing Alice, and dialling Jimmy in her newcomer turn; and references being "for standing, not for meeting".
- **ADR 0021, all of it:** a machine introduced still counts among those who greet, as above, but isn't dialled first, and work first covers its rule that an introducer still among those who greet introduces no one.

**Why:** an introduction is Alice's word about Jimmy, and her word means something only for work she checked. Kim gives it weight only because Alice did work Kim checked. Each step rests on work someone checked first-hand, and nothing travels further.

### What stands

- Greetings, the places of those who greet, the crowd limit, and ADR 0015's other limits.
- ADR 0015's introductions between a machine and a person, with both owners' yes. They're another thing.
- Each part can be turned off in `config.toml`.

## Consequences

- **Strangers get nothing but a greeting back.** Mapping the network now costs work on several days for every node mapped. One operator's names get honest nodes' lists only by carrying their traffic correctly, which costs real upload at each node. That's the price ADR 0016's rule of checked work, not testimony, sets.
- **Meeting follows traffic.** Lists and introductions flow only where nodes carried or served each other's media. A network with little video meets slowly. At the start, before anyone has carried, nodes meet only through directories and greetings. ADR 0015's gate, that nodes keep meeting with the directory off, now needs carrying in the same run.
- **Newcomers start slower than with open lists.** They earn their way, with days of carrying before a list. That's the intent, and how slow it is is what the simulator measures.
- **The stage A3 code is replaced** when this is built. Its signed requests, IDs, expiries and limits can be reused. Until then it stays off by default.
- **The invited alpha is unaffected.** It runs before carrying is built, so `Known` stays open, and with a handful of nodes all reading one directory, lists add little.
- **Privacy.** Fewer machines see a node's list. Kim learns that Alice worked with Jimmy, as a reference already told her. Alice learns which machines she sent it to, which she picked herself.

## Building it

After ADR 0016's stage B (carrying, and checked work letting a machine out of the places of those who greet), in the same simulator as [ADR 0017](0017-one-hop-forwarding.md#building-it)'s stage A. This replaces ADR 0016's stage A3 (built, off by default) and stage E (references).

| Step | Work | Gate |
| --- | --- | --- |
| 1 | `Known` only for machines with standing; introductions as above | In `osp-sim` with carrying, with marks set in the run's plan before it runs: a newcomer that knows only a directory gets a middle turn, standing with one node, its list and an introduction, each within a set time; strangers get no list; one operator's names get no honest node's list without carrying its jobs; established nodes keep meeting with the directory off |

## Validation

The gate above. Also reported: for each newcomer, the days from arriving to each step; the upload the operator spent for each list it got; and lists asked for and refused.

**Revisit** if newcomers take too long to get going, if a network with little traffic stops meeting, or if carrying honestly turns out cheap enough that a crowd gets lists anyway.
