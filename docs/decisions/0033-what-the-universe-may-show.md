# ADR 0033: What the Universe may show

**Status:** Proposed (2026-11-14). Step P3 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). A new client request of [ADR 0008](0008-device-pairing-and-client-protocol.md); the node-to-node protocol is unchanged.

## Context

The Universe ([ADR 0034](0034-a-universe-in-levels.md)) draws what Kim's node knows: people, their posts, the machines it has met, and what it serves. That knowledge is spread across the node's tables, and some of it was given to Kim's node on terms. Circle nodes answer `PlaceWithin` only for their followers, and never pass it on ([ADR 0029](0029-vouchers-within-six-steps.md)). The peer record never leaves the node ([ADR 0011](0011-measuring-peers.md)). Machine names are tied to no person ([ADR 0015](0015-nodes-meet-first.md)).

Two dangers:

- the app reading the node's database directly, so every column becomes something the phone shows, and a screenshot then shares it;
- the Universe asking the network for more, so the map has something to draw.

## Decision

### 1. One request, built from an allowlist

The phone asks **`Universe { level, region, layers, since }`** and gets a **snapshot** ([ADR 0034](0034-a-universe-in-levels.md) sets its size). It goes only to Kim's paired devices, as every client request does. The node builds it from the fields below and nothing else. A test fails if a snapshot holds a field that isn't on this list.

### 2. The core rule

> The Universe shows what Kim's node already knows and is allowed to show Kim. It never makes the node publish anything new, or ask anyone anything new, just so there's something to draw.

Fetching public material through a viewing identity, as Kim would by opening it (profile pictures up close, [ADR 0032](0032-profiles-people-want-to-follow.md)), is allowed. It tells nobody who Kim is.

### 3. What it may show

**People**

| Field | From | Why it may be shown |
| --- | --- | --- |
| identity key, name, picture hash | profiles the node holds | public, signed by them |
| steps from Kim: 1 (her circle), 2 (whom her circle reshared), 3 (from her gathered side) | follows, trust, outboxes, `side_vouches` | Kim's own follows, and public signed acts |
| follows them, is someone Kim knows | `follows`, `trust` | Kim's own data |
| found through a directory, in words | `found_people` | already shown today |
| public edges: Alice reshared Jimmy, Jimmy credited Carol early | reshares in outboxes, `vouches`, `side_vouches` | public signed acts |
| posts held, media held, last post (to the day) | `objects` | public; used for brightness |

**Machines**

| Field | From | Why it may be shown |
| --- | --- | --- |
| machine name, in words | `machines`, `map` | made to be shown; tied to no person |
| state: heard of, met, answered lately, faded | `machines` | coarse, Kim's own record |
| runs a directory | directory lists read | public role |
| work with Kim's node, in bands (none, some, a lot) | peer record | Kim's own record, banded, never a score |

**Content:** public posts, media and reshares the node holds or found, each placed with its author.

**Network:** totals only. Copies served, bytes served, posts hosted, people found through directories, as counts over a day or a week. And the recent events of [ADR 0036](0036-showing-what-the-network-does.md).

### 4. What it never shows

- **Addresses:** IP addresses, direct endpoint IDs, relay URLs, onion addresses. Advanced shows the node's own endpoint IDs, as now; the Universe doesn't.
- **Where anyone is:** no location, guessed or otherwise, and no time zones.
- **What anyone watched,** Kim included: found posts opened through viewing identities stay unlinked to her ([ADR 0024](0024-watching-found-posts-without-your-identity.md)).
- **Scores:** the peer record's numbers per machine. Bands only, as above.
- **What was given on terms:** `PlaceWithin` answers. [ADR 0037](0037-how-are-we-connected.md) shows them as a gap of known length, never as the people in it.
- **A machine next to a person.** Kim's node knows a followed person's node keys, and separately the machine names it has met. The Universe draws people and machines in separate layers, with no line between them, and never tries to match them up. The one exception is Kim's own star, which is her node.
- **Requests' notes** (trust, replies), outside Activity.

### 5. Kim's own data stays hers

Kim's followers, the people she knows and her own counts are hers to see on her phone. They aren't on this list's "never", but they are on the share card's ([ADR 0039](0039-sharing-your-universe.md)), which has its own, stricter allowlist.

## Options considered

- **The app reads the database** (through an export or SQL over the client protocol). Fastest to build, and no allowlist at all.
- **A request per table** (people, machines, peers). Each looks harmless, and the phone joins them. The allowlist is clearer in one place.
- **This: one snapshot, from one allowlist, with a test.**

## Consequences

- Each new thing the Universe shows needs a line in this record. That's deliberate.
- Screenshots of the Universe show only names that are public already. That doesn't apply to Kim's follows and the people she knows, who are drawn at step 1. The share card covers that case ([ADR 0039](0039-sharing-your-universe.md)), but an ordinary screenshot can't be stopped. The first run says so once: "the people near you here are the people you follow."

## Gate

A privacy review of every field, recorded in this ADR's table before P4 builds on it. Tests: the snapshot's schema matches the allowlist; a snapshot of a simulated world contains no endpoint ID, IP address or relay URL anywhere in its bytes; opening the Universe sends nothing new between nodes, apart from public material fetched through a viewing identity (counted in `osp-sim`).
