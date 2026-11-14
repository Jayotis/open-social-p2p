# ADR 0037: How are we connected?

**Status:** Proposed (2026-11-14). Step P7 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). Shows the paths [ADR 0029](0029-vouchers-within-six-steps.md) already works out; changes nothing in how replies are let in.

## Context

ADR 0029 lets Jimmy's reply in when enough people near Kim vouched for him, along separate chains. Kim sees the result ("published", or "found 2 of 4 points") but not why. The same paths answer a question people ask on any social network: how do I know this person?

## Decision

### 1. The question

On a profile, or on a person in the Universe, Kim can ask **How are we connected?** The Universe then lights the paths her node knows, for example:

1. Kim follows Alice. Alice reshared Carol. Carol reshared Jimmy.
2. Kim follows Bob. Bob reshared David. David credited Jimmy with finding his post early.
3. Kim knows Sarah. Sarah reshared Emma, Emma reshared Frank, and Frank reshared Jimmy.

Each path is written out in words, under the map.

### 2. Where the paths come from

- **Kim's world:** her circle, whom they reshared, and her gathered side, as ADR 0029 places them.
- **Jimmy's side:** the vouches his node showed when he replied (`ShowVouches`). If he hasn't replied, Kim's node may ask his node `WhoVouched` once, within that request's limits. Today a node asks it from its public endpoint, as part of gathering its own side. For this use it asks from a throwaway key over Tor instead, as viewing identities do ([ADR 0024](0024-watching-found-posts-without-your-identity.md)), so Jimmy's node learns that someone asked, not who. That's a question about public acts ([ADR 0033](0033-what-the-universe-may-show.md)'s core rule).

The paths come from the **same code** that admits replies, not from a copy of it. `Side::separate_chains` counts the separate chains today. It's changed to return the chains it counted, and admission keeps using their number. So what the map shows is what Kim's node would decide.

### 3. Three kinds of line

- **Known:** Kim's own follows, and the people she knows.
- **Signed evidence:** a reshare or a found-early credit her node checked.
- **A gap:** a step her node knows exists but can't see into. A circle node's `PlaceWithin` answer is one: "Alice's node places Carol within 2 steps." It's drawn as a dashed line of that length, with no one named inside, as the answer was given on terms ([ADR 0033](0033-what-the-universe-may-show.md)).

The map never draws a line it doesn't have. If there are no paths, it says "Your node hasn't found a connection," not "not connected."

## Options considered

- **A count only** ("3 connections"). Safe, but it's the part people want to see.
- **The shortest path only.** Easy to read, but ADR 0029 counts separate chains, and one path would hide why a reply got in.

## Consequences

- One more use for an existing public request, under its existing limits.
- Kim can see the chains that let a reply in, which also makes the voucher system something people can check for themselves.

## Gate

The paths shown agree with the node's own calculation in automated tests: on `osp-sim`'s voucher worlds, every reply let in has the paths the map is given, every gap is a `PlaceWithin` answer, and no path has a line that isn't known or signed.
