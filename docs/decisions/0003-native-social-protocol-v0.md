# ADR 0003: Native node-to-node social protocol (v0)

**Status:** Accepted (2026-10-21). The protocol is now `osp/social/1`, since [ADR 0010](0010-signing-keys-and-revocation.md) changed how objects are signed; the requests below are unchanged.

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
  - `Subscribe`: "I follow you, notify me". The author node records the follower's node ID, and whether the follower shares what it prefetches (the follower swarm, [doc 13](../13-hosting-roles-and-follower-swarm.md)). A follower's node sends it again once a day, and after a restart.
  - `Notify { seq }`: a best-effort push hint so followers sync now instead of on their next poll. A node that doesn't follow the author answers with an error.
  - `WhoHosts { hash }`: which nodes this node knows to be hosting a blob. To its followers, the author's node also names a few followers holding a copy (doc 13).
  - `Hosting { hash }`: "I hold a verified copy of this media of your posts, and serve it to anyone", from a follower that shares what it prefetches. Its author's node can then name it in `WhoHosts`.
  - `Moved { profile, addr }`: "the identity you follow now lives on this node", sent after a node takes an identity over ([ADR 0009](0009-identity-backup-and-recovery.md)).
  - `GetObjects { ids }`: up to 50 objects from this node's own outbox, by ID. Followers ask for them after the identity revokes a signing key, for copies signed by the new key ([ADR 0010](0010-signing-keys-and-revocation.md)).
- **Limits on incoming requests.** Anyone who knows a node's ID can connect, so a node limits what each request may cost. A request frame is at most 64 KiB; replies, such as outbox pages, may use the full 16 MiB. A node serves at most 32 requests at once from all peers together, and 4 on any one connection, each within 60 seconds. A request over a limit gets a "busy" error without being read. A connection may have at most 32 streams open, and one-way streams aren't allowed, since no protocol uses them.
- **Limits on followers and addresses.** Anyone can subscribe, and new identities cost nothing, so:
  - a node address (in `Subscribe`, `WhoHosts` replies or a follow ticket) may list at most 16 relays and IP addresses;
  - a node keeps at most 10,000 followers and accepts at most 100 new ones an hour. Someone refused still sees new posts by polling; they only miss the push until their node subscribes again the next day;
  - a follower not heard from for 30 days is forgotten. Answering a `Notify` or subscribing again counts as being heard from;
  - followers are notified 16 at a time, 15 seconds each, so ones that don't answer don't delay the rest;
  - a node remembers at most 20 hosts per blob, and takes at most 20 from one `WhoHosts` reply. The hosts a reply names are used for that download only, not remembered, so followers an author's node names to its followers aren't passed on to anyone else;
  - an author's node names at most 6 followers holding a copy in one `WhoHosts` reply, picked at random from the last 100 that said so and still share, and only to followers, each of which gets at most 60 such replies an hour. Strangers hear only of hosts that are public by choice: the author's node and resharers that host. `Hosting` is accepted only from a follower that shares, for media of the author's own posts.
- **Limits on syncing.** A followed node decides what its outbox says, so a reader bounds what it takes:
  - every outbox entry must have a higher sequence number than the reader's position, so each page moves the reader forward; otherwise that node's sync stops with an error;
  - an outbox page holds at most as many entries as asked for, and at most two bundled objects per entry (a reshared post and its author's profile). Bundled objects no reshare in the page points at are dropped. Entries that fail the object rules ([ADR 0004](0004-signed-objects-and-ids.md)) are skipped;
  - one sync takes at most 10 pages from a followed node, within 2 minutes, with 4 followed nodes synced at once;
  - a reader takes at most 500 new objects a day from one followed identity; the rest waits. A new follow starts with the latest 100 entries, not the whole history;
  - a node syncs at most every 5 seconds, however often it's notified;
  - media over 256 MiB isn't prefetched, and one sync prefetches at most 200 files and 1 GiB, smallest first, so thumbnails come before videos;
  - a prefetch refused as busy is tried again later, one file at a time, after about 1, 2, 4, 8, 15 and 15 minutes, each wait varied by up to half either way so followers refused together don't return together. After that the media is fetched when it's opened;
  - a new post or reshare whose media has a file over 1 MiB to prefetch is kept out of the feed until that file is on the node, so the person never taps a video that isn't there yet. It's shown anyway once the file won't come (the prefetch gave up, didn't fit the cache quota, or wasn't chosen in that sync), after 20 minutes, or when the node restarts. It then takes its place in the feed as of when it was shown, above posts that came meanwhile, not as of when it was published. Posts from before a follow, and the owner's own posts, are shown at once;
  - a node serving its outbox keeps each page under about 8 MiB of objects.
- **Follow tickets:** to follow someone you need their identity key and an address for their home node, encoded as a short string or QR code. There is no global directory yet.
- **Reshare and host:** an `Announce` of someone's post carries `hosting: true` when the resharer's node keeps a replica. Anyone who sees the `Announce` learns another provider for that media.
- **Serving rules** (enforced by the iroh-blobs request hook):
  - `own`: media of my own posts; served to anyone.
  - `replica`: media I chose to host by resharing; served to anyone.
  - `prefetch`: media my node fetched ahead of time from people I follow, and from the posts they reshared, so my phone loads it quickly. Served only to my paired devices, unless I share prefetched copies (`share_prefetched` under `[swarm]` in `config.toml`), when it is served to anyone. Sharing is the first piece of the follower swarm ([doc 13](../13-hosting-roles-and-follower-swarm.md)), off by default while it is tried out. It shows other nodes that I follow the post's author or a resharer, nothing about what I watched.
  - `cache`: media my node fetched when my phone opened it; served only to my paired devices, whatever the settings.
  - Anything else is refused.
- **Serving limits.** Anyone who knows a node's ID can download its public media, so a node limits how much it sends:
  - at most 8 transfers at once, at most 4 of them to strangers (nodes that neither follow me nor are my devices), and at most 2 to any one node. A crowd of strangers then can't crowd out my followers;
  - files of 1 MiB or less, such as thumbnails, have 8 places of their own outside those limits, apart from the 2 per node, so they aren't held up behind videos;
  - a request over a limit waits up to 10 seconds for a place, so a burst of requests from one node just takes turns. If no place frees up, it is refused as busy (iroh-blobs' `RateLimited`), and the requester tries the other hosts it knows, then asks the nodes it follows (`WhoHosts`);
  - an optional cap on sending speed for all transfers together: `max_upload_mbps` under `[serving]` in `config.toml`, off by default. About half the home's upload speed leaves room for everything else.
  - My own phone isn't affected: it reads media through its own connection to the node ([ADR 0008](0008-device-pairing-and-client-protocol.md)).
- **Storage quotas.** Followed nodes decide what their posts link to, so media other people posted has a quota, set under `[storage]` in `config.toml`: `cache_quota_mb` (default 5 GB) for `cache` and `prefetch` together, and `replica_quota_mb` (default 10 GB). Media of my own posts has no quota; it only has to fit on the disk.
  - Every download gets the size its post declares, and is stopped once more bytes than that arrive. When the phone asks for a blob, the node uses the largest size any stored post declares for it; a blob no stored post includes isn't downloaded.
  - Before a download, room is made for the declared size. A full cache evicts what my devices used longest ago: `cache` first, then `prefetch`. Replicas are never evicted, since hosting is a promise to others, so a reshare with hosting that doesn't fit is refused with a message; a reshare without hosting still works.
  - A download must leave at least 2 GiB free on the disk.
  - Evicting a blob removes its database row and its tag in the blob store; the iroh-blobs garbage collector deletes untagged blobs every 10 minutes. At startup the node makes the tags match the database and trims the cache to its quota. Hosted media over a lowered quota isn't deleted; the node reports it as a problem.
- **Stage 1 is public content only.** Private, followers-only and group content stays out until a reviewed design exists ([doc 05](../05-security-privacy-and-governance.md)).

## Consequences

- No interoperability with the fediverse in Stage 1. People on Mastodon cannot follow a home node yet.
- Follow relationships are visible to the followed person's node (as on any social network). The list of followers is not published.
- Discovery is manual (tickets and QR codes). Directories, DHT lookup and the ActivityPub bridge are Stage 2 topics.

## Validation and revisit conditions

- The Stage 1 test shows a post and its media reaching a third node after the author goes offline, via a resharer that hosts it.
- Revisit at Stage 2: design the ActivityPub bridge (a gateway with a public HTTPS address that translates for home nodes), or adopt an emerging standard if one suits home nodes better.
