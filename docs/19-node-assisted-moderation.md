# Node-Assisted Moderation

**Status:** Planning document. No AI model, script system or automatic community moderation is approved by this document alone.

OSP's home node is a good place to help its owner keep out spam and unwanted material. It is always on, already holds the person's local social state, and has more room to work than the phone.

The basic rule is:

> **A helper can suggest. The node and the person remain in control.**

A helper can be a simple rule, a script, a classifier or an AI model.

## Personal filtering

The owner's node may locally help with things such as:

- blocks and mutes;
- duplicate/repeated spam;
- unwanted words, topics or links;
- warnings/blurring for content the owner chose to treat cautiously;
- a local review queue.

These choices affect **this person's node**. They do not remove the content from OSP or create a public accusation about its author.

A person should be able to see why something was hidden and undo their own local filter.

## Helping community moderators

For the first public-community prototype in [ADR 0041](decisions/0041-public-communities-and-stewards.md), a moderator's node may help by:

- grouping duplicate reports;
- finding repeated/link spam;
- matching a clear community rule;
- putting suspicious items near the top of the moderation queue;
- suggesting hide, restore or a 24-hour mute.

By default, the moderator approves the action.

Later, a moderator may explicitly allow a bounded automatic action for obvious, reversible cases. That should be learned from real moderation use rather than assumed now.

## What helpers never decide

Scripts/AI do not decide whether a signature is valid or whether someone has permission under the protocol.

They may not independently:

- sign as the owner;
- delete another person's post;
- add/remove community moderators;
- change community rules;
- create a permanent community ban;
- transfer/close a community;
- change OSP governance.

Core protocol checks remain ordinary deterministic node code.

## Keep signing keys away from helpers

A helper should not receive the owner's signing key.

The flow is:

```text
incoming content
      |
 moderation helper
      |
   suggestion
      |
 node policy
      |
local filter OR ask the person
      |
node performs an allowed signed action
```

The node policy decides:

- which helpers may run;
- what they may inspect;
- CPU/RAM/time limits;
- what may happen locally without asking;
- what always needs a person;
- how many automatic actions may happen in a period.

If a helper crashes or behaves badly, the node must keep doing its normal social/network work.

## One interface for rules and AI

Do not build an API tied to one AI vendor or model.

A useful local result can be as simple as:

```text
target: <post>
reason: repeated_link
suggestion: review
helper: local-spam-v1
```

An AI model may also provide a confidence/explanation. A simple exact rule does not need one.

These are local results unless the person/node policy turns one into an allowed action.

## Local first

Where practical, personal filtering should work on the person's own node.

That keeps reading/preferences at home and works while the phone is off.

Possible helpers include:

- word/regex rules;
- URL/domain lists;
- duplicate/similarity checks;
- language/topic classifiers;
- local text/image/video models.

AI is optional. OSP must still work on a modest Home node with every model switched off.

Library or Studio hardware may have room for heavier helpers, but gets no extra authority from doing so.

## External services

An optional cloud moderation service can be considered later, but enabling it must clearly say what leaves the node.

A community moderator must not be able to silently make every reader's node send viewed content to an outside AI service.

## Public moderation vs personal filtering

The app should say which one happened.

Examples:

- **Hidden by your spam filter**
- **Muted by you**
- **Removed from this community by a moderator**
- **Held for moderator review by an automatic rule**

A local AI score is not itself a public moderation action.

## Privacy

Reports can contain sensitive information.

If helpers examine reports, give them only what they need. Do not publish raw reports merely because a resulting hide/mute is public.

Any future private moderator case store needs its own access/encryption design.

## Testing

Test at least:

- spam designed to trigger false positives;
- content that tells an AI model to ignore its instructions;
- a bad helper trying to read files/keys it should not access;
- a helper that hangs or uses too much memory/CPU;
- helper unavailable/off;
- two helpers disagree;
- automatic rules hide too much too quickly;
- a false positive is reversed;
- disabling helpers leaves OSP fully usable.

## Build in stages

1. **Simple personal filters:** existing blocks/mutes plus duplicate/link/word rules.
2. **Local helper interface:** one result format, limits and a review queue.
3. **Optional local models:** sandbox/resource limits, advisory by default.
4. **Community assistance:** only after ADR 0041's first community prototype exists.
5. **Bounded automation:** only after real moderation use shows which reversible actions are safe to automate.

## Gate

Assisted moderation is ready when:

- OSP works normally with it all switched off;
- helpers cannot access unrestricted signing authority;
- personal filtering cannot accidentally become public moderation;
- automatic community actions, if enabled, are bounded and reversible;
- failures cannot stop the node's core work;
- the person can understand and undo important local outcomes.
