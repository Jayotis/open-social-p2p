# Node-Assisted Moderation

**Status:** Design and planning document. No AI model, script API or automatic community moderation is approved by this document alone.

OSP's home node is well placed to help its owner filter spam and unwanted material because it is persistent, has more compute/storage than the phone, and already applies the owner's social state before presenting content.

Assistance must remain subordinate to human authority and protocol rules.

> **A moderation assistant may classify, prioritize or recommend. It gains no social or governance authority merely by running.**

This applies to hand-written rules, scripts, statistical classifiers and AI models.

See [doc 15](15-feature-responsibility-model.md), [ADR 0019](decisions/0019-taking-posts-and-media-down.md), [ADR 0041](decisions/0041-public-communities-and-stewards.md) and the feed filtering requirements in [doc 08](08-open-feed-algorithm.md).

## Three scopes

### 1. Personal moderation

The owner's node may locally:

- block/mute identities, posts, files, words or topics;
- suppress duplicate/repeated spam;
- warn or blur material based on the owner's preferences;
- reduce recommendations of unwanted topics;
- hold suspicious content in a local review queue;
- refuse to fetch/serve content under the owner's local policy where existing protocol rules permit.

These are local decisions. They do not create public moderation records and do not tell another node what it must show.

A local classifier result is not a fact about the author.

### 2. Community steward assistance

For communities under ADR 0041, a steward's node may help:

- group duplicate reports;
- detect repeated/link spam;
- identify likely rule matches;
- prioritize a moderation queue;
- summarize context;
- suggest hide, reinstate or temporary-restriction actions;
- prepare the exact action for the steward to review.

By default, the steward explicitly approves the public moderation action.

A community may later authorize **bounded automatic execution of routine, reversible actions**. Such automation is still attributed to a steward/policy and is never counted as a steward itself.

### 3. Network/protocol validity

No AI or script decides cryptographic validity.

Signature checking, governance quorum, author deletion, rate-limit accounting and protocol permissions are deterministic node code. An assistant may not override them.

## Assistant interface

Do not make a special API only for today's AI models. Define a general moderation-assistant result.

Conceptually:

```text
target: <local object reference>
classification: suspicious
confidence: 0.93
reasons:
  - duplicate_content
  - repeated_link
suggested_action: hold_for_review
assistant: local-spam-v1
```

The stable interface should support:

- target/reference;
- one or more reason/category codes;
- confidence where meaningful;
- suggested action;
- assistant/version identity;
- optional human-readable explanation;
- evidence references that do not expose unrelated private data.

A deterministic rule may omit confidence.

Assistant outputs are **local advisory records** unless a separate policy engine authorizes an action.

## Policy engine and signing boundary

Assistants must not receive unrestricted access to OSP signing keys.

The path is:

```text
content/event
    |
assistant(s)
    |
suggestion
    |
node moderation policy
    |
allowed local action OR request for human approval
    |
OSP node signs a scoped protocol action only when permitted
```

The policy engine enforces:

- which assistants may run;
- which content they may inspect;
- CPU/RAM/time budget;
- which local actions can happen automatically;
- whether community automation is enabled;
- maximum automatic action rate;
- what requires human approval;
- what is prohibited regardless of recommendation.

A compromised assistant should not be able to sign a governance proposal, permanent ban or author Delete.

## Community automation boundary

ADR 0041's human governance remains authoritative.

Automation may eventually be authorized for actions such as:

- hold an obvious duplicate flood for review;
- hide a post that exactly matches a signed community rule/blocklist;
- temporarily rate-limit repeated identical replies.

These actions must be:

- routine and reversible;
- explicitly enabled by an authorized steward/community policy;
- rate/volume bounded;
- recorded as automated;
- attributable to the steward/policy that authorized the automation;
- reviewable/reversible by stewards.

Automation may **never** independently:

- add/remove stewards;
- count toward quorum;
- change community rules/governance;
- permanently ban a participant;
- archive/close a community;
- forge/delete another author's content;
- resolve a governance fork.

Permanent bans still require ADR 0041's steward majority.

## Local models first

Where practical, personal moderation should be able to run entirely on the owner's node.

Benefits:

- browsing history/preferences need not leave the home;
- no central classifier gets a view of the person's feed;
- classification can continue while the phone is off;
- different people may choose different moderation preferences.

Potential local tools include:

- regex/keyword rules;
- URL/domain lists;
- duplicate/similarity detection;
- language/topic classifiers;
- text models;
- image/video classifiers.

A feature must work without AI. Low-power Home nodes can use simpler rules or disable expensive assistants.

Hunter/Creator hardware may run stronger local models without gaining network authority.

## External moderation services

A node may later support an optional external provider only with explicit owner configuration.

Before enabling it, the UI should state:

- what content/data leaves the node;
- who receives it;
- whether prompts/results are retained according to that service's terms;
- whether media files or only derived text/features are sent;
- how to disable/delete local configuration.

Cloud assistance is not required for OSP's core moderation capability.

Community stewards must not silently cause every member's node to upload viewed content to a third party.

## Transparency and overrides

For personal filtering, the app should offer useful explanations such as:

- Hidden by your spam filter.
- Blurred because you chose to warn on graphic imagery.
- Reduced because you asked to see less of this topic.

Users can inspect/override local decisions.

For community actions:

- distinguish "removed from this community" from "hidden by your node";
- show when a moderation action was automated under a steward-authorized policy;
- preserve the signed community audit record;
- never imply an AI system itself is a steward.

## Reports and private evidence

Reports may contain private/sensitive data.

Assistants processing reports should receive only what they need. Public community moderation records contain bounded reason codes and safe explanations, not the raw private report.

If OSP builds a restricted steward case store, its encryption/access model needs separate review before implementation.

## Ranking separation

Moderation/filtering occurs before recommendation ranking.

An item being classified as safe does not make it interesting. An item receiving high engagement does not bypass moderation.

Assistant scores are not organic popularity signals unless a future public algorithm explicitly and safely defines such use.

## Abuse and failure cases

Test at least:

- spammer tries to trigger false positives against others;
- prompt injection/content instructs an AI assistant to ignore policy;
- malicious model/script tries to access signing keys or filesystem;
- assistant hangs or consumes excessive memory/CPU;
- classifier becomes unavailable;
- two assistants disagree;
- steward automation hides too much content quickly;
- a model update changes decisions unexpectedly;
- poisoned community blocklist/rules;
- external provider unavailable/leaks metadata;
- false positive reversed without affecting the author's original content.

The node must continue its core social/network duties when moderation assistance fails.

## Implementation stages

### A1 — deterministic personal filters

Use existing blocks/mutes plus bounded keyword/domain/duplicate rules. No model runtime required.

### A2 — assistant API and local review queue

Define the stable local assistant result/interface and policy engine. Add explanations and overrides in the app.

### A3 — optional local models

Support sandboxed/resource-bounded local classifiers. Keep them advisory by default.

### A4 — steward assistance

Integrate assistants with community report/moderation queues after Front Page governance exists.

### A5 — bounded automatic community actions

Only after real moderation workload exists and tests show safe, reversible automation. Permanent/community-governance actions remain human-quorum controlled.

## Gate

Assisted moderation is ready when:

- disabling every assistant leaves OSP fully usable;
- assistants cannot access unrestricted signing authority;
- local filtering never becomes a public moderation record accidentally;
- automated community actions are explicitly bounded, attributable and reversible;
- permanent bans/governance remain human steward decisions;
- failure/resource exhaustion cannot disable the node's core responsibilities;
- users can understand and override important local outcomes.
