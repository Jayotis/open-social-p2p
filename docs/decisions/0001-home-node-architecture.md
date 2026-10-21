# ADR 0001: Home node architecture, phone as a thin client

**Status:** Accepted (2026-10-21). The plan for hosted nodes for people without a computer is superseded by [ADR 0006](0006-desktop-first-always-on-node.md): a personal node is required to join.

## Context

The earlier documents describe two products: a Flutter mobile app that is the main interface, and an optional desktop companion that stores and serves media. They leave open how the two divide the work, and how a phone reaches media on a home computer that sits behind a router (or a VPN).

Phones are a poor place to run a peer-to-peer node: background execution is restricted, batteries and mobile data are limited, and joining a public swarm exposes the phone's IP address. Desktops and small always-on boxes have none of those problems.

## Options considered

1. **Phone is a full peer.** It joins the swarm, fetches from many peers and talks to social servers itself. The companion only adds storage. This is the most work on the weakest device and leaks the phone's IP to everyone it fetches from.
2. **Phone talks to social servers directly; the desktop serves only media.** Social features keep working when the desktop is off, but the phone needs two networking stacks and is still exposed to social servers and peers.
3. **Phone talks only to its own home node.** The node does all social and media networking and the phone is a client of that one node.

## Decision

Option 3. **The desktop gathers content, the phone consumes it.**

- Each person has one **home node**. It holds their identity key, publishes their posts, follows other people, syncs their outboxes, prefetches media they are likely to watch, transcodes uploads and, if they opt in, hosts replicas of content they reshare.
- The phone pairs with its home node once (QR code) and from then on opens a single encrypted connection to it, by public key, from anywhere. On the home Wi-Fi this can be direct; away from home it goes through hole punching or a relay.
- **The home node is a role, not a particular machine.** It runs on a desktop, a small server such as a spare box, or (later) a hosted or community-run server for people who have no computer. This keeps the "no desktop required to join" principle from [Vision and principles](../01-vision-and-principles.md) without making the phone speak two protocols.

## Consequences

- **Privacy:** the phone's IP is seen only by its own home node (and any relay in between), not by the swarm or other people's nodes. Feed building and preferences stay on the person's own hardware, which matches the on-device preference in [doc 08](../08-open-feed-algorithm.md).
- **Battery and data:** the phone downloads finished, already-transcoded media from one place; no seeding from the phone.
- **Availability:** when the home node is offline, the phone shows what it has cached and cannot post. Hosted home nodes and a second paired node are the long-term answers; the prototype accepts the limitation.
- **Security:** the home node becomes a high-value target (it holds the identity key). Pairing, device revocation and key backup need careful design; see ADR 0004 for key separation.
- **Operational:** someone without a computer needs a hosted node. That is a service to run and pay for, recorded as an open question in [doc 07](../07-contributing-and-open-decisions.md).

## Validation and revisit conditions

- Stage 1 prototype: phone on mobile data reaches the desktop node behind home NAT and behind a VPN, and plays a video.
- Revisit if relayed phone-to-node traffic proves too slow or costly, or if a direct phone-to-network mode is needed for people whose home node is often offline.
