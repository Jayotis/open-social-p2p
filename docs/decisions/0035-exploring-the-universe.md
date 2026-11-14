# ADR 0035: Exploring the Universe

**Status:** Proposed (2026-11-14). Step P5 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). Replaces M5's planned Explore lanes and search screen. Uses [ADR 0034](0034-a-universe-in-levels.md)'s levels and the node's own index ([ADR 0014](0014-finding-people-and-private-messages.md)).

## Context

Someone with no follows has an empty Home, and today only a search box to start from. To use search, you have to know what to look for. M5 planned browsing lanes (recent posts, new people, videos, reshares). The Universe can be those lanes, placed on a map.

## Decision

### 1. Layers

The map has one geography, and layers that change what lights up. The phone asks for a layer with the snapshot ([ADR 0033](0033-what-the-universe-may-show.md)).

| Layer | Lights up |
| --- | --- |
| **People** | people and groups (the default) |
| **Posts** | where recent public posts are |
| **Video** | where public video is |
| **Shared** | where reshares are spreading |
| **Network** | machines, hosting and transfers ([ADR 0036](0036-showing-what-the-network-does.md)) |

Trending, hunters, overlooked work, topics and different perspectives (docs [09](../09-distributed-trending-signals.md) and [10](../10-exploration-divergence-and-hunters.md)) come after the alpha. Each needs its own work on ranking and abuse.

### 2. Tapping opens what's there

Tapping a group opens a sheet of its people and its recent posts, newest first. Tapping a person opens their profile ([ADR 0032](0032-profiles-people-want-to-follow.md)). A found post's media opens through a viewing identity, as now ([ADR 0024](0024-watching-found-posts-without-your-identity.md)).

The order is always plain: newest first, or by the node's own measure of what it holds. No ranking learns from what Kim looks at.

### 3. Search on the map

Search stays on the node, as now, so nothing Kim types leaves the house. A search does two things at once:

1. it shows the results as today, separated into people, posts and video;
2. it lights up the regions that hold them, and offers to fly there.

Each result says the region it's in, so the phone knows where to look. Kim can switch between the map and the plain list at any time.

### 4. A newcomer's first look

On a fresh node, the first directory read (50 s on the clean-install test) brings the people a directory lists and their latest posts. That's what the first Universe shows. If the node has found no one yet, the map says so plainly ("Your node is looking for people"), and offers an invite code to paste instead.

## Options considered

- **Explore as lists** (M5's lanes). Simple and familiar, but it would be a second way to explore, beside the map.
- **The map only.** It's the signature feature, but lists are faster to read and work better with screen readers. Both stay, and each links to the other.

## Consequences

- The Product Alpha needs real public content to explore. The lab nodes post their own, and testers bring theirs. Nothing is made up.
- What a newcomer sees depends on which directories their node reads. That's ADR 0015's design; ADR 0027's several directories, after the alpha, widen it.

## Gate

A new person with no follows finds people and posts they find interesting by browsing, without knowing what to search for. Checked on a fresh node against the lab's world. Tests: every search result carries its region; each layer's lit regions match what the node holds.
