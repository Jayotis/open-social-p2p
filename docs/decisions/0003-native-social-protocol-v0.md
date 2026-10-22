# ADR 0003: Native node-to-node social protocol (v0)

**Status:** Accepted (2026-10-21)

## Context

The original architecture assumed an existing ActivityPub server (Mastodon or Friendica) as the social layer. ActivityPub identifies every actor by an HTTPS URL and delivers activities by HTTP POST to that actor's inbox. A home node behind a router or VPN has no public address to receive those POSTs without port forwarding or a tunnel, and the docs rule out requiring either for ordinary users.

## Options considered

1. **Run an ActivityPub server now** (GoToSocial, Mastodon) and attach P2P media to it. Federation from day one, but every person needs a publicly reachable server, which brings back central hosting, and Stage 1 would be spent learning another server's internals.
2. **Native node-to-node protocol over iroh**, using ActivityStreams 2.0 vocabulary for the objects, with an ActivityPub bridge added later.

## Decision

Option 2 for Stage 1.

- Objects use **ActivityStreams 2.0 vocabulary** (`Person`, `Note`, `Announce`, `published`, `attachment`, `inReplyTo` and so on) in plain JSON, signed as described in ADR 0004. They are not processed as JSON-LD. Using AS2 terms keeps a later ActivityPub bridge a translation job rather than a redesign.
- Each identity has an **outbox**: an ordered list of the signed objects it has published.
- Protocol `osp/social/0` (request/response, one QUIC stream per request, a length-prefixed JSON frame each way):
  - `GetProfile`: the signed `Person` record, which lists the identity's current home node IDs.
  - `GetOutbox { since }`: objects after sequence number `since`.
  - `Subscribe`: "I follow you, notify me". The author node records the follower's node ID. A follower's node sends it again once a day, and after a restart.
  - `Notify { seq }`: a best-effort push hint so followers sync now instead of on their next poll. A node that doesn't follow the author answers with an error.
  - `WhoHosts { hash }`: which nodes this node knows to be hosting a blob.
- **Limits on incoming requests.** Anyone who knows a node's ID can connect, so a node limits what each request may cost. A request frame is at most 64 KiB; replies, such as outbox pages, may use the full 16 MiB. A node serves at most 32 requests at once from all peers together, and 4 on any one connection, each within 60 seconds. A request over a limit gets a "busy" error without being read. A connection may have at most 32 streams open, and one-way streams aren't allowed, since no protocol uses them.
- **Limits on followers and addresses.** Anyone can subscribe, and new identities cost nothing, so:
  - a node address (in `Subscribe`, `WhoHosts` replies or a follow ticket) may list at most 16 relays and IP addresses;
  - a node keeps at most 10,000 followers and accepts at most 100 new ones an hour. Someone refused still sees new posts by polling; they only miss the push until their node subscribes again the next day;
  - a follower not heard from for 30 days is forgotten. Answering a `Notify` or subscribing again counts as being heard from;
  - followers are notified 16 at a time, 15 seconds each, so ones that don't answer don't delay the rest;
  - a node remembers at most 20 hosts per blob, and takes at most 20 from one `WhoHosts` reply.
- **Follow tickets:** to follow someone you need their identity key and an address for their home node, encoded as a short string or QR code. There is no global directory yet.
- **Reshare and host:** an `Announce` of someone's post carries `hosting: true` when the resharer's node keeps a replica. Anyone who sees the `Announce` learns another provider for that media.
- **Serving rules** (enforced by the iroh-blobs request hook):
  - `own`: media of my own posts; served to anyone.
  - `replica`: media I chose to host by resharing; served to anyone.
  - `cache`: media my node prefetched for my phone; served only to my paired devices.
  - Anything else is refused.
- **Stage 1 is public content only.** Private, followers-only and group content stays out until a reviewed design exists ([doc 05](../05-security-privacy-and-governance.md)).

## Consequences

- No interoperability with the fediverse in Stage 1. People on Mastodon cannot follow a home node yet.
- Follow relationships are visible to the followed person's node (as on any social network). The list of followers is not published.
- Discovery is manual (tickets and QR codes). Directories, DHT lookup and the ActivityPub bridge are Stage 2 topics.

## Validation and revisit conditions

- The Stage 1 test shows a post and its media reaching a third node after the author goes offline, via a resharer that hosts it.
- Revisit at Stage 2: design the ActivityPub bridge (a gateway with a public HTTPS address that translates for home nodes), or adopt an emerging standard if one suits home nodes better.
