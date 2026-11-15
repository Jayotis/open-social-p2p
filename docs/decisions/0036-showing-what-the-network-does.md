# ADR 0036: Showing what the network does

**Status:** Accepted (2026-11-14), the maintainer's decision the day it was proposed. Step P6 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). Replaces M5's planned My Node screen. Adds events to [ADR 0033](0033-what-the-universe-may-show.md)'s allowlist.

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

*Built (2026-11-15, P6 part 2):* the node keeps them in `events` (migration 35), and gives a device those after the `seq` it holds. Events about one thing within an hour of the first are summed into one, which comes again with a new `seq`. A fetch is an event only when the node fetched by itself, ahead of time or to keep a copy: media Kim opened is what she watched, which the Universe never shows ([ADR 0033](0033-what-the-universe-may-show.md) §4). The author's node is asked last, so a copy often sends it all before anyone asks the author's node. So "kept available by a copy" also counts a last sync with the author that failed. `osp events` prints them. The fields are reviewed in ADR 0033.

*Built (2026-11-15, P6 part 3):* the phone asks for events with the Universe, since the highest `seq` it holds, and draws each one once, one at a time, about three seconds each. A summed one coming again isn't drawn again. Only events from the last hour are drawn: the first look shows what just happened, and the rest of the day is listed. A ring opens around people a machine's list just brought, or round the public sky's edge when none of them is on the map. A reshare is a line from the author out to whoever reshared. A fetch flows into Kim's star from its host's star, or from the edge of the view when it came from another node, and a copy's flow has its own colour. A served copy flows out to the edge. "The edge" stands for no place: its direction comes from the post's ID, so the same post comes the same way. Machines aren't placed on the map yet, so a machine met or faded brightens or dims the machines chip. Someone an event names who isn't on the map draws nothing. A line under the map says what's being drawn ("Your node sent 2.0 MB of Jimmy's post to another node"), and tapping it, or **What it did today** in the node's summary, lists the day's events. Tests: an idle world draws nothing and schedules no frame, each event is drawn once, and the words pass the word list.

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

*Built (2026-11-15, P6 part 1):* "this month" became **the last 30 days**, which is what's counted: every byte of media the node sends another node, by UTC day, kept for 31 days (`served_days`). That's its own posts' media, copies it keeps for others, everything. **Hosting** counts the posts the node reshared with hosting (`posts_hosted`, as reviewed in [ADR 0033](0033-what-the-universe-may-show.md)). Copies a follower that shares keeps aren't counted post by post, so they show only in what was served. **Space** is the media the node holds plus its database. These come in the node's status, not the snapshot, so the allowlist doesn't change. The machines list starts with a few sentences on what machines are and what the node does with them ([ADR 0015](0015-nodes-meet-first.md)), promising nothing about reputation, and leads to this summary.

## Options considered

- **A traffic monitor:** connections, peers, transfer rates. Accurate, but it's a packet analyser, and draws machines next to transfers in ways that would link them to people.
- **A static node screen** (M5's plan). Honest, but it doesn't show the network working.

## Consequences

- The node keeps one more small table of events. It goes in no backup, and nothing is sent anywhere because of it.
- Live flows show the owner when their node served a copy. Only the owner sees that, and it names a post, not who fetched it.

## Gate

The node's summary describes what it did in plain words only: the word-list test of [ADR 0031](0031-app-shell-home-universe-you.md) passes on it, and each line matches the node's own counts in `osp-sim`. Whether people who didn't build OSP can then say what their node does is checked in P11 ([ADR 0030](0030-product-alpha-before-testers.md)). Tests: each event type is produced by a real action in `osp-sim`; an idle world produces no events; no event holds a field outside the allowlist.
