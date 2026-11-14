# ADR 0031: The app shell: Home, Universe and You

**Status:** Proposed (2026-11-14). Step P1 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). Changes the app only; the client protocol ([ADR 0008](0008-device-pairing-and-client-protocol.md)) is unchanged.

## Context

Today the app opens on a feed, with search, people, trust, reply requests and node status reached from buttons around it. The node's state is the most prominent thing on screen: a banner, Tor state, endpoint IDs. A new person sees software for running a node, and an empty feed until they follow someone (since 2026-11-14, that feed points to search).

## Decision

### 1. Three places

| Place | What it's for |
| --- | --- |
| **Home** | The posts of people you follow and what they reshared, newest first: today's feed. |
| **Universe** | Exploring people, posts and the network as a map ([ADR 0034](0034-a-universe-in-levels.md), [ADR 0035](0035-exploring-the-universe.md)). |
| **You** | Your profile, your posts, the people you know, your node, settings. |

Three things are reachable from all of them:

- **Post:** one prominent button. It opens today's compose screen.
- **Activity:** a bell with a count, opening the inbox of [ADR 0038](0038-likes-and-activity.md). Until P8 it holds what exists now: reply requests and trust requests.
- **Your node:** tapping your own star at the centre of the Universe, or the node line in You, opens its summary ([ADR 0036](0036-showing-what-the-network-does.md)).

Search opens from the Universe, and from wherever a list of people or posts is shown.

### 2. Plain words in the normal journey

The screens people use every day name no mechanism. The words change, and the mechanisms behind them don't.

| Today | In the app |
| --- | --- |
| node, home node | your node (your home computer) |
| trusted follower, trust request | people you know; "Jimmy says he knows you" |
| allow trust | "Yes, I know Jimmy" |
| directory, found through `words` | found through `words` (one line, under Advanced details) |
| follow code, ticket | invite code |
| reply requests | replies waiting for you |
| reshare with hosting | share, and help keep it available |

**Advanced** keeps everything the status screen shows now: Tor, relay or direct, endpoint IDs, round-trip times, version, uptime, devices, logs, follow codes. Nothing is removed, only moved.

A test fails if a word from a short list (Tor, iroh, endpoint, onion, relay, RTT, actor, blob, directory, ticket) appears on an ordinary screen outside Advanced.

### 3. First run

After pairing:

1. The app shows the person's name and invite code: "This is you."
2. One screen says that their home computer is their node, and that it keeps working while the phone is off.
3. The Universe opens, with their star at the centre.
4. As the node reads its first directory (50 s on the clean-install test) and meets machines, people appear around it. The app says what's happening ("Your node is finding people"), from real events only ([ADR 0036](0036-showing-what-the-network-does.md)).
5. The person can open profiles and posts at once, with no follows.
6. A hint suggests following a few people. Home fills as they do.

An empty Home points to the Universe, not to search.

### 4. The look starts here

P1 sets the app's theme (colours, type, spacing, light and dark) as one set of tokens, the Universe's dark sky among them. P9 refines them rather than repainting screens built in the default Material look.

## Options considered

- **Five tabs (Home, Explore, Post, Activity, Profile),** the usual social app. Familiar, but Explore and the node's view would be two places, and the map would be a sixth.
- **Universe as the first tab.** It's the signature feature, but people come back to read what they follow. Home first, and the first run opens the Universe once.
- **This: three places, two global actions.**

## Consequences

- The app's screens are reorganised, and each old screen moves under one of the three places or under Advanced. Nothing on the node changes.
- Support gets harder in one way: a tester describing a problem won't know the words. Advanced has a "copy details for a bug report" button that copies the node's state without addresses.

## Gate

A new person understands where Home, the Universe, posting, Activity and their profile are, without knowing what Tor, iroh, an endpoint or a directory is. Checked with at least two people who didn't build OSP, watching them use the app, plus the word-list test above.
