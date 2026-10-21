# Exploration, Divergent Perspectives, and Discovery Scouts

**Status:** Architecture and product proposal for public review. No ranking model, discovery attribution system or app feature is implemented.

## The purpose

Open Social P2P must not become another network in which popular posts always become more popular, people see only what resembles their recent clicks, and unfamiliar creators never get exposure.

The design should promote **user agency, curiosity, useful disagreement, discovery of neglected material, and recognition of thoughtful curators**. It must not impose an ideological profile on users or equate controversy with quality.

This document extends [the open feed algorithm](08-open-feed-algorithm.md) and [distributed trend signals](09-distributed-trending-signals.md). The **personal home-hosted web search engine is not part of this project**.

## Distinguish four independent ideas

Do not conflate these properties:

1. **Freshness:** The original content was published recently.
2. **Underexposure:** Few people in the *observed and eligible portion of the network* have been shown or engaged with the content. Low observed counts do not prove low global counts.
3. **Novelty / unexpectedness:** Material is unlike the user's selected interests or the content they have already explored. This is relative to the viewer and may be desirable or undesirable.
4. **Perspective diversity:** Content offers a substantively different interpretation, method, experience or line of argument about a topic. It is not synonymous with factual reliability, offensiveness, polarization or popularity.

An old, obscure post can be worth discovering. A new post can repeat an old idea. A popular post can offer a perspective rarely seen in one community. Each deserves separate treatment.

## User-facing discovery areas

### Following — Latest

Always offer an unranked chronological view of the eligible posts the person follows. The discovery engine must not interfere with this mode.

### Uncharted / Hidden Gems

An intentional exploration lane for public material with little *observed* exposure, including both newly posted content and older overlooked posts. Search for relevance by topics the user requests, not simply by negative popularity. Sample across small communities and servers. Ensure a user can browse without asking the algorithm to build a detailed behavioral profile.

Suggested filters: **Just Published**, **Overlooked**, **Outside My Usual Topics**, **Small Communities**, and **Surprise Me**.

### Different Perspectives / Crossroads

An opt-in way to compare perspectives, approaches and experiences relevant to a user-selected issue or interest. Prefer **topic-centered contrasting viewpoints**, explanatory context and constructive disagreement over inflammatory bait.

Possible interactions:
- "Show another approach to this problem."
- "What are the main disagreements about this topic?"
- "Show thoughtful critiques and responses."
- "Show how different communities discuss it."

Avoid guessing a user's political, religious or other sensitive beliefs to classify or target them. Do not permanently label accounts as belonging to fixed ideological camps. Respect their choice not to receive a category of content.

**Diversity does not mean manufactured symmetry:** two views are not automatically equally supported by evidence. Existing safety, moderation, topic relevance and provenance checks still apply. Do not reward harassment, fabricated claims or outrage merely because it is oppositional.

### Trailblazers / Discovery Scouts

Let people intentionally browse introductions and recommendations from members with a demonstrated history of finding, contextualizing and sharing interesting work *early*. Promote conscientious curation rather than follower count or self-declared expertise.

A scout may be an enthusiastic hobbyist, community member, independent researcher, artist, specialist or librarian of links. The label is about their discovery activity, **not their truthfulness, credibility as an expert, or authority over others**.

## Discovery attribution: creators first, scouts second

An illustrative chain:

- Alice publishes a public video. Alice is and remains the **creator**.
- Bob reshares Alice's video with context before it has much observed attention in Bob's community.
- Other independent accounts discover and discuss it through Bob's public share.
- The interface may show "Found through Bob" or "Introduced to this community by Bob", always naming Alice as the creator.

**Do not say "first person in the world to discover"** without impossible-to-guarantee global evidence. A user can only be an early discoverer *within an observable, declared scope and timeframe*. Federated services cannot see everything.

Potential attribution fields for later specification: canonical original post/author ID, reshare activity and its signed actor reference, observation window and scope, pointer to the introducing post, and an uncertainty indicator. Actor signatures alone do not prove global chronology. Use canonical post provenance and guard against forged timestamps, deletions and cross-server duplicates.

Preserve an introduction path where technically feasible, but **do not expose a detailed viewer/share graph** or private relationship trail. Never publicize who merely viewed a file.

## Measuring a scout's contribution (ideas, not adopted weights)

Potential evidence:
- How many distinct public works were introduced early within a defined community.
- Whether those works later earned *independent* attention after the introduction.
- Whether a scout regularly introduces work from independent creators rather than repeatedly boosting their own material.
- How often the introduced material broadens the mix of topics and communities encountered.
- Voluntary feedback on usefulness and context, rather than follower count alone.

Hard problems:
- Correlation is not proof that an introduction caused later interest.
- Multiple independent people can discover the same work; attribution should be non-exclusive where evidence warrants.
- Disconnected servers and deleted posts produce incomplete records.
- Self-reshares, paid promotion rings and bot networks can manufacture apparent success.
- A numerical "influencer rank" can quickly become a monopoly, status contest or harassment target.

**Recommendation:** begin with optional *discovery trails* and user-followable curator lists, not a universal public score or winner-takes-all "top influencer" leaderboard. If measurable scout recognition is eventually added, make methodology visible, allow correction and appeal, limit ranking feedback loops and test anti-manipulation carefully.

## How the algorithm could accommodate exploration

There should be *separate pools of candidates*, not a single engagement score deciding every slot:

- User-followed eligible content.
- Time-sensitive public discussion.
- Underexposed content, including older works.
- Topic-relevant contrasting perspectives from beyond the user's existing subscriptions.
- Explicitly chosen scouts, independent curators and smaller communities.

The open algorithm can allocate **user-configurable exploration space** to candidate pools, then check eligibility, source context, variety and duplication. No specific percentages or weights are adopted yet. Users can turn exploration off, increase it or choose a standalone exploration feed; recommendations should be explicitly explained.

The system should not covertly infer sensitive personal ideology. An opt-in "show alternatives to this post/topic" query is a safer initial design than building a permanent political or religious profile.

## Using the P2P network responsibly

The P2P network can help participants **discover publicly announced content references**, distribute independently verified introductions/summaries and replicate permitted media. It cannot provide reliable evidence that a post has not been seen anywhere else.

- Signed public introductions or bookmarks may be eligible discovery signals.
- Existing public ActivityPub follows, replies and reshares may be reused where allowed.
- Discovery candidates can be gossiped or subscribed to in topic/community channels; avoid broadcasting everyone's personal reading history.
- Nodes should not reveal viewers' home IP addresses or browsing data for ranking.
- Number of media hosts, bytes transferred and raw content downloads are **not** endorsements or a scouting reputation signal.
- A user without a desktop companion must have access to discovery features.
- Private-group posts, restricted-audience content and direct messages are excluded from public exploration.

P2P gossip distributes claims, not guarantees their accuracy. Authenticating a node or ActivityPub actor does not establish that one account represents one independent person.

## Fairness, anti-silo goals and choice

Exposure of unfamiliar perspectives should be possible, easy to select and meaningfully useful, **not forced confrontation**. The algorithm should support:
- Topic-focused explanations and contextual links.
- Discovering independently run communities, including those with small audiences.
- Exploring a topic from multiple disciplinary or community viewpoints, not only a binary conflict.
- Filtering mute/block lists and moderation exclusions before presentation.
- Avoiding repeatedly recommending the same source, viral account or inflammatory issue.
- Explicit opt-outs, language controls, content warnings and chronological-only mode.

Study whether users actually find useful material, not just whether posts trigger replies and time spent. Metrics should include creator coverage, new-creator exposure, unique sources, sustained user-reported discovery usefulness, respectful cross-community conversation and accuracy of provenance explanations.

## Threats and limitations

- Deliberately spreading low-quality or harmful content under the label "divergent".
- Gaming "first finder" status with controlled accounts, timestamp manipulation or coordinated boosting.
- Rewarding controversy, outrage and harassment instead of curiosity.
- Popular curators consolidating attention, recreating the original influencer economy.
- Misclassifying genuinely unfamiliar topics or user viewpoints.
- Revealing private interests, social relationships, community membership or reading history through discovery trails.
- Treating missing ActivityPub/P2P observations as proof that a work was never seen.

Moderation and factual-context safeguards are independent from ranking. Do not use the feed to secretly push particular political or ideological conclusions.

## Research and testing before implementation

1. Can public ActivityPub activities and opt-in public recommendations create useful discovery trails across servers?
2. How can "overlooked" be estimated from sparse federated observations without pretending global completeness?
3. Compare user-selected topic prompts with any automatic perspective classifier for privacy and usefulness.
4. Test new creators, old neglected works, niche languages, and small communities under a range of exploration settings.
5. Measure whether curated introductions create new exposure without imposing a winner-takes-all public ranking.
6. Simulate Sybil farms, reciprocal boosting, false timestamps and malicious gossip peers.
7. Test whether people can disable divergence and influence features completely while retaining a normal social app.

## Research references

- [Beyond-accuracy recommendation research on novelty and personal curiosity](https://arxiv.org/abs/1907.00119)
- [Popularity debiasing and coverage](https://arxiv.org/abs/2006.04275)
- [Bridging-based ranking design paper](https://www.belfercenter.org/sites/default/files/pantheon_files/files/publication/TAPP-Aviv_BridgingBasedRanking_FINAL_220518_0.pdf)
- [Prosocial ranking comparison study (2026)](https://arxiv.org/abs/2603.19626)
- [W3C ActivityPub recommendation](https://www.w3.org/TR/activitypub/)

These are design inputs, not proof that any proposed algorithm will prevent filter bubbles or meet privacy targets.
