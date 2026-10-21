# Critical review of docs 12 and 13

**Date:** 2026-10-21
**Reviews:** the first drafts of doc 12 ("Contribution accounting and network priority") and doc 13 ("Segment hosting, random assignment and content expiry"). Those drafts have since been replaced by [Peer reciprocity and recognition](../12-peer-reciprocity-and-recognition.md) and [Hosting roles, the follower swarm and expiry](../13-hosting-roles-and-follower-swarm.md). Each new draft has a "What was removed" section summarising the mechanisms criticised below.
**Purpose:** find every reason the designs could fail, before anything is built.

A second, independent review was carried out the same day. Its corrections are folded in below and summarised in [its own section](#second-review-corrections).

## How to read this

**Severity** (how bad the problem is):

- **Fatal unless fixed:** the design breaks one of its own goals or a project principle as written.
- **Serious:** likely to cause real harm or failure in practice.
- **Moderate:** a cost or weakness to plan around.

**Disposition** (where the finding stands now):

| Disposition | Meaning |
| --- | --- |
| **Resolved** | A specific design change addresses the failure |
| **Blocking** | Must be resolved before the named feature is built |
| **Needs evidence** | Requires testing, measurement or source checking |
| **Deferred** | Real concern, but the feature isn't in the first release |
| **Accepted risk** | Understood, documented and explicitly accepted |

## Decisions taken in response

Recorded on 2026-10-21 after this review and the second review:

1. **A personal node is required to join** ([ADR 0006](../decisions/0006-desktop-first-always-on-node.md)). The node is always on; the phone is its paired remote. Hosted nodes may come later.
2. **Messages private by default, media direct by default** ([ADR 0007](../decisions/0007-transport-privacy-defaults.md)), with an opt-in private-media mode.
3. **No public receipts and no global credit.** Nodes assess their own peers and use generous tit-for-tat when busy. Creators and scouts get levels others can check; hosts get a self-reported stat that grants nothing ([doc 12](../12-peer-reciprocity-and-recognition.md)).
4. **Popular content spreads through the follower swarm.** Followers' automatically prefetched copies are served publicly. Random blind hosting is dropped ([doc 13](../13-hosting-roles-and-follower-swarm.md)).

## The short version

Both first drafts solved problems a network gets at **scale** (viral load, freeloading, status competition), and introduced problems that appear on **day one** (privacy leaks, legal exposure, complexity). Two findings were fatal as written:

- public delivery receipts leak who watches whom (A1);
- "segments are fragments nobody can view" is false (B1).

Both are now resolved by removal.

## Part A: contribution accounting and priority (first draft of doc 12)

### A1. Receipts publish who watches whom. *Fatal unless fixed*

A receipt says "R received 40 MB from P's node on 21 October". It carries no post ID, but P is usually the creator R follows, or someone who reshared what R watched. Receipts exist to be shown to others, so collecting them from many providers gives a daily record, per person, of whose content they consume. [Doc 11](../11-collective-discovery-history-and-referrals.md) exists to prevent this.

The underlying tension: verifiable status needs attributable evidence (you must know who signed a receipt to tell real accounts from fake ones); privacy needs unattributable evidence. Unlinkable token schemes exist (see A12), but applying them to bandwidth is substantial research.

**Disposition: Resolved.** Public receipts are removed. Host credit is private to each node's ledger and never leaves it.

### A2. Priority is invisible while the network is small. *Serious, softened*

Priority only acts when a node is busy, so in a small network contributors and freeloaders would usually get identical service. The second review rightly softened this: even a tiny network saturates when a post draws attention from outside. The real criticism is that a complicated *global* incentive system is unproven and unnecessary until measurements justify it.

**Disposition: Resolved.** The global incentive system is removed. Local reciprocity acts only when a node is busy, and is enabled only after measurements show busy nodes (doc 12, phase 3).

### A3. Newcomers get the worst service at the worst moment. *Serious*

Busy periods are viral spikes, and viral spikes are when newcomers arrive. With no history, they get only the stranger share of a saturated node. A fixed share divided among hundreds of strangers is very little.

**Disposition: Needs evidence; Blocking for doc 12 phase 3.** The second draft gives strangers 30% of a busy node's upload, and the follower swarm adds sources. Strangers' time to first frame during a simulated spike must be measured before tit-for-tat is switched on.

### A4. Fake accounts can manufacture host and creator credit. *Serious*

Twenty identities on one machine can exchange real, correctly hashed content over loopback at no cost and sign genuine receipts. Hash verification proves the bytes were real, not that a real person benefited. Weighting by follows is weak because follows are cheap. Personalised PageRank needs a wide view of the follow graph that no home node has.

**Disposition: Resolved for bandwidth; Accepted risk for levels.** A node's private ledger records only what that node itself received, so a farm can only influence the nodes it actually trades with. As the second review notes, this is not proof that a real person benefited, which is fine: it is used only for that node's own decisions. Creator and scout levels can still be inflated by reshare rings. That risk is accepted because levels grant recognition only, never bandwidth or reach.

### A5. Status can be bought, which brings back centralisation. *Serious*

A rented gigabit server would out-earn every home user on global host credit, so the top status would be for sale and priority would flow to data centres.

**Disposition: Resolved.** There is no global host credit. The host stat is self-reported and buys nothing; a rented server can only earn reciprocity with the specific nodes it serves.

### A6. The slow lane falls mostly on phone-only people who just watch. *Serious*

Most people on social networks mostly watch. Under the first draft the slow lane fell on phone-only watchers, conflicting with [principle 9](../01-vision-and-principles.md) and with doc 08.

**Disposition: Resolved by ADR 0006.** There are no phone-only participants; everyone runs a node and can give back. **Residual Accepted risk:** people with slow upload speeds reciprocate less, so they get less from busy nodes. The stranger share and the absence of any penalty when nodes aren't busy limit the effect.

### A7. Hosted-node operators become powerful. *Serious*

An operator serving thousands of people would accumulate huge credit, or have to split it in ways nobody can check.

**Disposition: Deferred.** ADR 0006 has no hosted nodes at launch. Any future hosted-node ADR must address this.

### A8. Scout credit is gameable and depends on where you sit. *Moderate*

Bots can reshare everything instantly; rings can make each other's picks "take off"; "first seen" favours scouts near the viewer and on well-connected nodes.

**Disposition: Accepted risk.** Hit-rate scoring and first-seen timing limit the damage; levels grant recognition only.

### A9. A public "top host" badge is a target. *Moderate*

It invites envy and harassment, and publicly declares "I redistribute a lot of content", which makes the holder the first person to receive notices.

**Disposition: Accepted risk, reduced.** The host stat is optional and off the card unless the owner shows it.

### A10. Pressure will mount to let credit influence ranking. *Moderate*

Once standing is measurable, people will want it to buy reach, which rewards outrage and clickbait.

**Disposition: Resolved as a rule; ongoing governance.** Doc 12 states that peer assessment and levels never affect feed ranking. Changing that requires its own ADR.

### A11. Priority can be weaponised. *Moderate*

A peer with genuine high reciprocity could hog a node's capacity for hot content.

**Disposition: Accepted risk.** Logarithmic weighting, decay and the stranger share bound it, and it only affects the node being hogged.

### A12. History is not encouraging. *Moderate, reframed*

- BitTorrent's tit-for-tat was gamed by the strategic BitTyrant client, which got a median 70% speed-up ([Piatek et al., NSDI 2007](https://www.usenix.org/conference/nsdi-07/do-incentives-build-robustness-bittorrent)).
- Tribler's BarterCast targeted only "lazy" freeriders, and later analysis found it gave incentives to fewer of those than modelled ([Meulpolder et al., 2009](https://research.tue.nl/en/publications/bartercast-a-practical-approach-to-prevent-lazy-freeriding-in-p2p/)).
- Private trackers enforce upload ratios, but only through a central tracker.
- Filecoin and Swarm reached working incentives only by adding tokens and cryptographic proofs.

As the second review points out, these are cautionary examples, not proof that decentralised incentives are impossible. Standardised unlinkable tokens exist ([Privacy Pass, RFC 9576](https://www.rfc-editor.org/rfc/rfc9576)), though applying them to bandwidth rewards would be substantial design and anti-fraud work.

**Disposition: Accepted.** The second draft aims only at local, generous reciprocity, and tests it before relying on it.

## Part B: segment hosting and random assignment (first draft of doc 13)

### B1. Segments are not meaningless fragments. *Fatal unless fixed*

- Segment 0 of a faststart MP4 plays on its own.
- Every 1 MB segment of a v1-720p video contains keyframes, so stills can be extracted.
- A photo is a single segment.

Volunteers could unknowingly hold viewable illegal imagery.

This review proposed encrypting all media with the key in the signed post. **The second review showed that this does not solve the problem:** the key is published with the post, so anyone who can see the post can decrypt. It prevents casual inspection, not deliberate inspection, and no choice of encryption guarantees legal protection.

**Disposition: Resolved by removal.** Random blind hosting is dropped. Nodes host only whole files they fetched because of their own follows, reshares or preservation choices. The encryption recommendation is withdrawn.

### B2. Scattered segments make retention worse, not better. *Serious*

At 50% host uptime and 3 copies of each piece, an 11-segment video is available 23% of the time when its segments are scattered across different hosts, against 87.5% for 3 whole copies. The second review added an important qualification: placement, not splitting, is the cause. If the same hosts carry every segment, availability equals whole-file copies.

**Disposition: Resolved.** Hosting is of whole files; byte ranges are used only for faster parallel downloads.

### B3. Deniability on the hosting side is mostly theatre while downloads are visible. *Serious*

The nodes you download from see every request, which is a richer record than what you host.

**Disposition: Resolved by removal.** Hosting deniability is no longer claimed. Download visibility is stated plainly in ADR 0007: by default, peers you exchange media with see your address.

### B4. What you're assigned reveals your community. *Serious*

Gossiped assignments mostly come from your own social circles, revealing membership.

**Disposition: Resolved by removal.** Note that the follower swarm *does* reveal whom a node follows. That is disclosed at setup, is opt-out, and is the kind of information most social networks show anyway.

### B5. Cover costs the very resource we're trying to save. *Moderate*

Meaningful cover needed about 8 MB of assigned downloads per video watched, roughly 12 GB a month for a heavy viewer.

**Disposition: Resolved by removal.**

### B6. "Plausible deniability" has a poor legal track record. *Serious*

Statistical methods for identifying Freenet downloaders have been published ([Levine et al., 2017](https://people.cs.umass.edu/~liberato/home/publication/statistical-detection-of-downloaders-in-freenet), disputed by the Freenet developers).

**Disposition: Resolved.** No design document may claim legal protection for hosts.

### B7. Expiry can fail all at once. *Moderate*

- Every host may reach "14 days without requests" together while demand is still real.
- Each host may assume others still hold copies, so they all drop them.

**Disposition: Blocking for doc 13 phase 3; Needs evidence.** The second draft randomises the expiry day per node and requires a recent confirmation that 3 other copies exist. A simulation must show copies don't vanish together. The second review's correction is also adopted: the creator's copy is kept until the creator deletes it or storage fails, not "forever".

### B8. The first wave still hits the creator. *Serious*

A 10–20 Mbps home upload serves only about 6 to 13 simultaneous 1.5 Mbps streams. Worse, the prototype's own prefetching makes every follower's node fetch at once. This review suggested pushing copies to peers at publish time; the second review noted that this only moves the creator's upload cost earlier.

**Disposition: Needs evidence; Blocking for accepting doc 13.** The follower swarm, staggered prefetch and busy-redirect target this directly: early fetchers become sources for later ones. Measure the creator's total upload, with and without the swarm, on the test machines and in simulation.

### B9. Help-wanted notices can be abused. *Serious*

They allow flooding volunteers with junk, and deliberately planting illegal content on volunteers' machines.

**Disposition: Resolved by removal.**

### B10. Takedowns get harder. *Moderate*

**Disposition: Accepted risk.** Holders are known parties (followers, resharers, preservation nodes); cooperating nodes honour deletions; uncooperative copies can persist, as [doc 05](../05-security-privacy-and-governance.md) already says.

### B11. Small files gain nothing; many sources add overhead. *Moderate*

**Disposition: Accepted.** Parallel range downloads are used only for large files or saturated sources.

### B12. Bookkeeping grows faster than the content. *Moderate*

**Disposition: Deferred.** `WhoHosts` goes to the creator's node and to followed nodes for now. A distributed index is a later decision, if measurements require it.

### B13. Defaults decide everything. *Moderate*

**Disposition: Resolved.** Setup asks whether to share prefetched copies, explains what that reveals, recommends yes, and allows turning it off at any time.

## Part C: both designs together

### C1. Solving scale problems before there are users. *Serious*

**Disposition: Resolved.** The work is sequenced: swarm basics and peer measurement go with milestone M3; tit-for-tat and levels wait for measurements; the Android app (M2) is not blocked.

### C2. Several principles needed explicit amendment. *Serious*

**Disposition: Resolved.** ADR 0006 amends doc 01's non-goal about requiring a desktop. The second draft of doc 12 is consistent with docs 08, 10 and 11, and states where each applies.

### C3. Complexity budget. *Serious*

**Disposition: Resolved (reduced).** The second drafts remove:

- receipts and the weighting system;
- help-wanted gossip;
- segment hosting;
- media encryption;
- erasure coding.

What remains reuses components the prototype already has.

## Second review corrections

An independent review of the first drafts and of this document, on the same day, made these corrections, all adopted above:

1. **Encryption with the key in the post is not confidentiality** (B1).
2. **Priority under contention is zero-sum.** Calling it "positive, never punitive" was spin (A3, doc 12).
3. **The creator's copy can't be promised forever:** creators delete, and disks fail (B7).
4. **Placement, not splitting, causes the availability loss** (B2).
5. **Pre-seeding moves the creator's cost earlier; it doesn't remove it** (B8).
6. **A private ledger is not proof a real person benefited.** That's acceptable for local decisions only (A4).
7. **Even small networks saturate** when a post draws outside attention (A2).
8. **Historical failures are cautionary, not conclusive** (A12).

Its central question was: *"What minimum service must the network provide when virtually nobody is willing or able to host?"* ADR 0006 answers it differently than the second review expected: every participant runs an always-on node, and that is what keeps their own phone working.

Its two "settle before coding" decisions now stand as follows:

- **Portable identity.** Already in [ADR 0004](../decisions/0004-signed-objects-and-ids.md): the identity key is separate from node keys. Recovery is still to be designed.
- **Address privacy default.** Decided in ADR 0007.

### Follow-up review

A further review of the second drafts added:

- **`WhoHosts` can expose the follower graph** to outsiders, linked to addresses. Doc 13 now treats this as a privacy surface, with interim rules.
- **Relay-only is not anonymity.** The relay learns both ends and the timing. Private modes must fail closed, and must be verified across the whole application, including discovery records, replies and logs (ADR 0007).
- **The most important next experiment is B8** (the follower swarm): it is what distinguishes this network from an ordinary self-hosted server.

## Release gates

These are acceptance objectives, not results. **Numerical thresholds must be written down before measuring**, so results can't be interpreted after the fact.

| Item | Test | Success means | Gates |
| --- | --- | --- | --- |
| **A3** strangers during spikes | Simulate 300 followers plus hundreds of outside viewers, with slow and fast hosts | Newcomers get usable playback, not starved by established peers | Doc 12 phase 3 (tit-for-tat) |
| **B7** expiry failing together | Simulate 14-day expiry with stale host lists, simultaneous outages and shrinking demand | Copies don't vanish together because each host assumed the others kept one | Doc 13 phase 3 (expiry) |
| **B8** creator upload | A creator serving 300 followers directly, against staggered prefetch with the follower swarm | The creator's upload falls substantially, and followers still get posts promptly | Accepting doc 13 |
| **WhoHosts** exposure | Query as a stranger and as a follower; inspect what each learns | Strangers can't enumerate followers; followers see only a rate-limited sample | Doc 13 phase 1 |
| **ADR 0007** private mode | Nodes on different networks; inspect traffic, discovery records, `WhoHosts` replies and relays | No peer learns a node's address through the application; failures are closed, never silently direct | ADR 0007 phase 1 |
| **Identity recovery** | Simulate disk failure, a stolen phone, a lost desktop, and restoring onto new hardware | Identity and social data restored from an authorized backup; compromised devices revoked | Any public release |
