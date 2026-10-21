# Open Feed and Recommendation Algorithm

**Status: Proposal for review — no code or working ranking system exists yet.**

Open Social P2P's content discovery and feed ordering must be **open source, independently implementable, explainable, and under the user's control**. The social network must not depend on an opaque ranking service operated by one company.

This document describes the *content-ranking algorithm*. It is separate from the [peer-performance algorithm](03-peer-distribution-and-scoring.md), which assigns media chunks to sources based on locally measured connection speed and reliability.

## Feed modes

The Flutter mobile app and web client should offer clear modes:

| Feed | Proposed behavior |
| --- | --- |
| **Following — Latest** | Strict chronological posts from followed accounts; no engagement-based reordering |
| **Following — Suggested** | Optional ranking of eligible followed-account posts for personal relevance |
| **Trending** | Public posts and topics gaining attention within a selected time window, based on distributed evidence |
| **Discover** | Public posts from outside the user's follows, with topic controls and diversity/exploration |
| **Communities** | Group posts according to membership, permissions and selected community sorting |
| **Videos** | Video posts and channels with the same transparent discovery controls |

Users should be able to switch modes at any time. Chronological mode is a first-class feature, not an obscure opt-out.

## Two different questions

1. **What's trending on the network?** Generate a tentative list of posts/topics that show *recent, meaningful increases in attention* from eligible public activity.
2. **What should appear in my feed?** Filter eligible content through the person's choices, follows, blocks, selected topics, language and feed mode, then order it transparently.

A fast media source, large amount of uploaded data, or high replica count is **not**, on its own, evidence that a post is interesting or endorsed. Network health and human attention must remain distinct.

## Potential public popularity signals

Discuss and evaluate the following inputs; weights are deliberately **not selected**:

| Candidate signal | Proposed role | Major limitation |
| --- | --- | --- |
| Unique public resharers | Meaningful distribution/explicit share | Can be farmed by accounts and servers |
| Public replies and conversations | Activity and conversation growth | Argument and harassment can also drive replies |
| Public likes/reactions | Weak approval/engagement hint | Easy to coordinate artificially |
| Changes in activity over recent time windows | Detect emerging rather than merely historically large posts | Small samples cause noisy rankings |
| Diversity of contributing independent servers or communities | Potential evidence of broader spread | Servers and identities can be fabricated; not a proof |
| Opt-in, privacy-preserving aggregated media retrieval demand | Supporting evidence of interest and resource needs | Downloads, auto-preloads, replays and bots are not endorsements |
| Voluntary public peer hosting/replication | Help estimate availability, not approval | Mirrors can be automatic; popularity must not equal replica count |

Never use raw bytes, transfer speed, number of chunks, direct IP addresses, or the presence of a user's personal desktop node as a default ranking boost. A user without the companion app should not be disadvantaged.

## A proposed transparent ranking pipeline

1. **Eligibility:** Only consider public content the viewer is allowed to see. Respect blocks, mutes, moderation and requested topic/language constraints before ranking.
2. **De-duplication:** Group reposts under the canonical original object when possible; account for cross-server references and media aliases without confusing different authors' work.
3. **Estimate activity:** Gather eligible public social interactions and optional aggregate demand signals; deduplicate repeat events where possible. Treat unverified or incomplete data as uncertain.
4. **Determine trend:** Compare recent activity against an earlier baseline and account for age, low-sample noise, and uncertainty, so content gaining attention can surface.
5. **Apply user preferences:** Follow relationships, explicit topic choices, hidden topics, language preferences and chosen feed mode. Personal preferences remain on-device where feasible.
6. **Balance and explore:** Avoid infinite repetition of the same creator/topic; include a configurable opportunity for new voices rather than only established popular accounts.
7. **Explain:** Provide a human-readable reason such as "Trending among public posts followed by this community" or "Shared by accounts you follow". Never suggest a number is global if only part of the network was observed.

The eventual mathematical formula, normalizations, parameters and default weights must be reviewed publicly and versioned in this repository. We are *not* adopting arbitrary numerical weights before we can test abuse and accuracy.

## Local preference and privacy

Prefer the user device to combine user-selected preferences with candidate lists; publish as little personal viewing history as possible. Do not require centralized profiles of each user's searches, watching behavior or clicks.

The system may still need limited social infrastructure to make recommendations practical. Document exactly which information is processed on-device, on the user's home node, on federated servers, or on optional coordination nodes before release.

No hidden paid boost, secret suppression formula or preferential visibility based on hosting capacity should be part of default organic feeds. Sponsored content, if ever supported, must be visibly labeled and separately governed.

## Open algorithm requirements

- Publish the complete implementable algorithm and its defaults, including filtering, scoring, decay, source selection, tie-breaking and diversity rules.
- Publish configuration formats, explanation categories, version history and reproducible test fixtures once development begins.
- Allow independent client implementations and alternative ranking choices; no single server should have exclusive control of the feed.
- Provide a visible **Why am I seeing this?** explanation and controls to disable particular recommendation sources.
- Changes to ranking logic must be documented, reviewable and supported by tests for fairness, manipulation and privacy regression.
- Publish aggregate evaluation methods but never release people's private viewing history or credentials.

## Risks that must be researched

- Coordinated amplification, bots, sockpuppets, purchased engagement, Sybil identities and malicious servers.
- Mistaking outrage/harassment for positive interest.
- Feedback loops where the algorithm causes the popularity it measures.
- Excluding smaller communities, new users or posts on servers with poor connectivity.
- Federation incompleteness: no server sees every public interaction on the entire Internet.
- Leakage of private followers, browsing history, audience membership or geographic location through popularity reporting.
- Misleading exact counts from incomplete samples.

## Evaluation before any launch

Develop privacy-preserving test fixtures and simulation scenarios for:

- A new post with a genuine rise in discussion versus an old post with a large historical total.
- A coordinated bot farm versus diverse genuine engagement.
- A post that receives a million automatic chunk requests but no meaningful social activity.
- A small community's locally interesting content versus a very large global account.
- Loss of major peers and missing federated servers without showing fake precision.
- Explicit feed controls overriding trending recommendations.
- Rapidly spreading harmful content where moderation and personal blocks must be applied separately from popularity.

## Standards and examples to research

- [W3C ActivityPub specification](https://www.w3.org/TR/activitypub/) — public social activities and federation.
- [Mastodon trends API](https://docs.joinmastodon.org/methods/trends/) — a comparative existing feature, not a proposed final scoring algorithm.
- [libp2p Gossipsub specification](https://github.com/libp2p/specs/blob/master/pubsub/gossipsub/gossipsub-v1.1.md) — a *transport-layer* reference; its peer scoring is not a social-content ranking model.

See [Distributed Trending Signals](09-distributed-trending-signals.md) for the proposed P2P data collection and privacy design.
