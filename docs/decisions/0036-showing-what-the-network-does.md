# ADR 0036: Showing what the network does

**Status:** Proposed (2026-11-14). Step P6 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). Replaces M5's planned My Node screen. Adds events to [ADR 0033](0033-what-the-universe-may-show.md)'s allowlist.

## Context

What makes OSP different is invisible: Kim's node serves Jimmy's video to a stranger, fetches a post from a resharer while its creator is offline, and meets machines at night. Today that shows up only in `osp peers`, logs and the status screen's numbers.

## Decision

### 1. Events, from things that happened

The node keeps a short list of recent events (the last 24 hours, at most 500), and the phone asks for those since its last look (`UniverseEvents { since }`). Each one is something the node did or saw:

| Event | Drawn as |
| --- | --- |
| a directory read brought N new people | a region appearing, or growing |
| a reshare of a post Kim's node knows | a line branching into another group |
| media fetched from a host | a flow from the host's region to Kim's star |
| a copy served to another node | a flow out from Kim's star |
| media fetched from a copy while its creator's node was unreachable | the flow, marked "kept available by a copy" |
| a machine met, or faded | a point in the Network layer brightening or fading |

The host of a fetch is drawn only as a person when it is one publicly, as a resharer listed as hosting ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)). Otherwise it's a machine in the Network layer, or just "another node."

Busy events are summed (for example "served 14 copies of Jimmy's video in the last hour"), and drawn once.

### 2. Nothing made up

Every animation comes from an event or an honest total. With no events, the map is still. There's no demo mode in a release build, and a test checks that the phone draws no flow it wasn't sent.

### 3. Kim's node, in a sentence or two

Tapping Kim's star, or the node line in You, opens a summary:

> **Online**
> Hosting 11 shared posts · Served 324 MB to the network this month
> Following 8 people · 12 followers
> Using 1.4 GB of space for OSP

Each line comes from the node's own counts: posts it hosts as a resharer or a follower that shares, bytes served from the peer record, follows, followers, the store's size. Below it is **Advanced**, holding the status screen as it is today: Tor, relay or direct, version, uptime, node IDs, resources, diagnostics ([ADR 0031](0031-app-shell-home-universe-you.md)).

When the node is offline, the summary says when it was last seen, and the Universe shows the last snapshot, marked as such.

## Options considered

- **A traffic monitor:** connections, peers, transfer rates. Accurate, but it's a packet analyser, and draws machines next to transfers in ways that would link them to people.
- **A static node screen** (M5's plan). Honest, but it doesn't show the network working.

## Consequences

- The node keeps one more small table of events. It goes in no backup, and nothing is sent anywhere because of it.
- Live flows show the owner when their node served a copy. Only the owner sees that, and it names a post, not who fetched it.

## Gate

A person who didn't build OSP can say in their own words what their node does for the network, without knowing the transport underneath. Tests: each event type is produced by a real action in `osp-sim`; an idle world produces no events; no event holds a field outside the allowlist.
