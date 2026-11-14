# ADR 0030: A Product Alpha before testers

**Status:** Proposed (2026-11-14). Replaces the roadmap's M5 and the invited mechanism alpha as the next release gate. The decisions for each part are in ADRs [0031](0031-app-shell-home-universe-you.md) to [0039](0039-sharing-your-universe.md).

## Context

M0 to M4 and the first half of M5 built the network: a home node publishes, finds people through directories, follows, reshares, hosts and serves media, fetches from copies when the creator is offline, deletes, replies across Tor, and lets in strangers' replies when enough people near the creator vouched for them ([ADR 0029](0029-vouchers-within-six-steps.md)).

The plan was to hand that to a few invited people (the `v0.1.0-alpha.4` draft, 2026-11-14), then build the social product (M5). But the app still shows a mechanism: endpoints, Tor state, follow codes, a search box and a feed that is empty until you follow someone. Someone who didn't build OSP has no reason to install it, and nothing to show anyone else once they have. And with no testers yet, a mechanism alpha would test mostly the installer.

## Decision

### 1. The next release is the Product Alpha

The **Product Alpha** is the first version someone who didn't help build OSP would want to install, explore, use, show to someone else and keep running. It replaces both the invited mechanism alpha and M5 as the next gate.

M0 to M4 and M5's replies and vouchers become the **engineering foundation**. They stay as built, and the Product Alpha is built on them.

### 2. Three places, built in this order

The app has three places, **Home · Universe · You**, with posting and activity always reachable ([ADR 0031](0031-app-shell-home-universe-you.md)). The **Universe** is the network drawn as a zoomable map of people, content and machines. It is where people explore, it shapes the node's new requests, and it is how a newcomer with no follows finds someone. It isn't a decoration added at the end.

| Step | What | Decided in |
| --- | --- | --- |
| P0 | This record, and the roadmap reset | this ADR |
| P1 | The new app shell and first run | [0031](0031-app-shell-home-universe-you.md) |
| P2 | Profiles as social pages, edited on the phone | [0032](0032-profiles-people-want-to-follow.md) |
| P3 | The Universe request: what the node may show its owner | [0033](0033-what-the-universe-may-show.md) |
| P4 | The hierarchical Universe: clusters, zoom, stable places | [0034](0034-a-universe-in-levels.md) |
| P5 | The Universe as Explore, and search on the map | [0035](0035-exploring-the-universe.md) |
| P6 | Real network activity, and your node's summary | [0036](0036-showing-what-the-network-does.md) |
| P7 | "How are we connected?" | [0037](0037-how-are-we-connected.md) |
| P8 | Likes and one Activity inbox | [0038](0038-likes-and-activity.md) |
| P9 | The visual pass: screenshots that look like OSP | roadmap only |
| P10 | Sharing your Universe, a profile or a post | [0039](0039-sharing-your-universe.md) |
| P11 | Hardening: fresh-node journeys, devices, failures | roadmap only |
| P12 | The Product Alpha release and recruiting | roadmap only |

P9, P11 and P12 decide nothing new about the protocol or privacy, so they have gates in the roadmap and no record of their own.

### 3. What may interrupt the order

Network and protocol work interrupts it only for:

- a bug;
- a privacy or security problem;
- something the node must do for the product journey to work (for example avatars served to strangers, P2).

Not built before the Product Alpha, as before: ADR 0017's two hops, ADR 0018's earned direct links, ADR 0022's earned lists, ADR 0023, ADR 0026 and ADR 0027.

### 4. The current app stays as the reference

The working screens (pairing, posting, resumable uploads, feed, search, follows, reshares, playback, replies and their controls, trusted people, deletes, node health) stay working while the new shell replaces them one at a time. Nothing in the client protocol is removed while a screen still uses it. Protocol words and developer controls move under **Advanced** ([ADR 0031](0031-app-shell-home-universe-you.md)).

### 5. No tester release before the gate

The `v0.1.0-alpha.4` draft stays unpublished. It remains useful for clean-install checks on the lab machines. No release goes to testers until P11's gate is met.

### 6. What success looks like

The Product Alpha has worked if people who didn't build OSP:

- install it, and understand the idea;
- find others and their posts, and follow some;
- come back to Home, and post or reply;
- explore the Universe without being asked;
- understand that their node takes part;
- keep the node running after the scripted test;
- ask for improvements, not only report install failures.

The strongest sign is someone sharing a screenshot, or inviting another person, without being asked.

## Options considered

- **Ship the mechanism alpha now, build the product after.** Earlier outside feedback, but on the parts least likely to keep anyone. With no testers yet, it would test mostly the installer, and that is tested on the lab machines already.
- **Build M5's list as it was** (reactions, profiles, Explore lanes, a node screen). The same parts, without a centre. The Universe ties Explore, the node screen, cold start and the app's look together. Built as five separate screens, they'd need redoing.
- **This plan.** Testers come months later, but get something worth keeping.

## Consequences

- **Testers come later.** P3 to P7 are most of the work, and P4 is the largest single piece. Until P11, the lab machines and simulated peers do all the checking ([roadmap](../06-roadmap.md), P11).
- **The real Universe will be small.** A node keeps at most 64 MiB of what it found in directories (about 5,000 people), meets at most 256 machines and maps at most 10,000. The first testers will see tens of people. So the Universe has to look right with 20 people as well as with a million made-up ones ([ADR 0034](0034-a-universe-in-levels.md)).
- **Mechanism work slows to fixes.** Accepted records that aren't built (0017, 0018, 0022) wait longer. Media still goes through n0's relays for nodes that hide their address.
- **Privacy.** Every new view of the network goes through one allowlist on the node ([ADR 0033](0033-what-the-universe-may-show.md)), and the one thing made to leave the phone, the share card, has its own ([ADR 0039](0039-sharing-your-universe.md)).

## Revisit when

- someone offers to test before P11: a private round can run on what's built, without changing this order;
- P4 turns out much larger than planned: a flat Local view (P4's last two levels) can carry P5 to P8, with the higher levels after the alpha;
- a protocol change the product needs turns out to need its own record.
