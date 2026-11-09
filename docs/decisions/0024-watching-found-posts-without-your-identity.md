# ADR 0024: Watching a found post without showing who you are

**Status:** Proposed (2026-11-09), for review before any code. Settles what [ADR 0015](0015-nodes-meet-first.md) left open ("a found post's media opens only once its author is followed").

## Context

Alice installs OSP, follows nobody, and finds Kim's video through a directory ([ADR 0015](0015-nodes-meet-first.md)). Today she can't watch it: her node knows no sizes and no hosts for a found post's files, and it asks only the nodes it follows who hosts a file. The maintainer wants public posts viewable without telling the creator who watched (2026-11-09), along the whole path, not just the first request.

What would identify Alice today, if her node simply asked Kim's node, as it does for posts it follows:

1. **The request.** `WhoHosts` goes over Tor from Alice's public endpoint. Tor checks the key of the node that dials, so Kim's node knows exactly which node asked about which file.
2. **The download.** It comes from Alice's media endpoint. Its key is stable, and its binding to her public ID ([ADR 0014](0014-finding-people-and-private-messages.md)) is given to every node she follows and is named in `WhoHosts` replies to other followers when she shares copies. So even when Kim's node hasn't seen that binding, the endpoint is one pseudonym across everything Alice watches, and anyone who has seen the binding, now or later, can turn it into her identity. Kim would learn it the day Alice follows her.
3. **The relay.** A media endpoint has no IP address of its own, so the download goes through the relay Kim's media endpoint uses, n0's for now. That relay sees Alice's node's IP address and which endpoint she fetched from, as it does for every relayed transfer today.

So a fix for the request alone leaves the download identifying her.

## Options considered

1. **Ask from the public identity, and say so.** Smallest. Kim learns, by public ID, who watched what.
2. **A throwaway identity for the request only.** The download still comes from Alice's stable media endpoint: point 2 above.
3. **A throwaway endpoint for the whole path, with no request to Kim at all.** Kim's profile carries her media endpoint's binding. Alice's node checks it and downloads through the relay from a fresh, relay-only endpoint made for this viewing. Kim's node sees an unknown endpoint fetch a public file, as any stranger's.
4. **Media over Tor.** Hides the relay's view too. Too slow for video ([ADR 0014](0014-finding-people-and-private-messages.md#evidence-m4s-first-tests-2026-10-31)).

## Decision

Option 3, for public posts from the creator and from hosting resharers ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)).

- **The profile carries its media binding.** A node with a media endpoint puts that endpoint's binding in its signed profile, beside its public node IDs. A media endpoint is relay-only and has no address to give, so publishing it shows only which relay it uses. Its ID already goes to anyone it serves. A direct endpoint's binding is never published.
- **Alice's node checks it** as any binding: both keys signed it, and the public ID is one Kim's current signed profile lists. One that fails is ignored.
- **The download comes from a viewing endpoint:** a new key, relay-only like a media endpoint, with no IP sockets, never bound to Alice's public ID and never given a binding. It's used for one author's files, at most for 10 minutes or until the viewer stops (a starting value), then dropped. A thumbnail and its video share one; another author's post gets another. It sends nothing but iroh-blobs requests for the hashes of a post Alice's node holds, signed by its author, and the question of who hosts them ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)).
- **No message to Kim's node from Alice's identity,** over Tor or otherwise. A found post's files and their sizes come from the signed post itself, checked as every download is, chunk by chunk. The only question the viewing endpoint asks besides the download is which hosting resharers Kim's node names, through Kim's media endpoint ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)): public information, asked by a key that names nobody.
- **Only public sources.** The creator's media endpoint, and the media endpoints of hosting resharers ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)). Never a follower swarm's members, which Kim's node names only to her followers, and never a direct endpoint.
- **Only public posts.** Posts for trusted followers ([ADR 0013](0013-trusted-followers.md)) are never fetched this way.
- **What the viewing leaves behind** stays private: the copy is `cache`, served to Alice's devices only ([ADR 0012](0012-sharing-prefetched-copies.md)), as anything opened is. Watching publishes nothing. Sharing is a separate act ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)).
- **When Alice follows Kim,** her node goes back to using its own endpoints, as for any follow.

## Consequences

**What Kim's node learns:** that some endpoint it doesn't know fetched a public file, and when. Not the public ID, not the media endpoint, not an IP address. It can count fetches.

**What it can't hide:**

- **The relay** sees the viewing endpoint and Alice's node's IP address, which endpoint it fetched from, and how much. n0 runs the relays for now. That's the same as for every relayed transfer today ([ADR 0007](0007-transport-privacy-defaults.md)), and it's why relays run by the community stay an objective. The relay doesn't learn Alice's public ID from this.
- **Timing.** If Alice follows Kim a minute after watching, Kim's node may guess the two were one node. A node can't prevent guesses from timing.
- **The relay URL.** A viewing endpoint uses the same home relay as Alice's media endpoint. On n0's shared relays that says little. On a relay few people use, it narrows who it was.
- **Within the 10 minutes,** Kim's node can link Alice's fetches of Kim's own posts to one viewer, not to who she is.
- **It's not anonymity.** Someone watching both Alice's connection and Kim's relay can match the traffic. Nothing may describe this as anonymous.

**Limits on strangers** stay as they are: strangers share half a node's transfer places, so viewers with throwaway keys can't crowd out Kim's followers. A viewer making many keys gets no more than one stranger's share each, and builds no standing ([ADR 0011](0011-measuring-peers.md) records stay with the viewing endpoint and are dropped with it).

**Cost:** a new relay connection for each viewing, about a second, and one key pair. The profile grows by one binding, about 200 bytes.

**What changes elsewhere:** the profile gains a field, an extension to AS2, ignored by older nodes. ADR 0014's rule that the media endpoint's ID goes to anyone the node serves becomes "goes to anyone": it was already public in effect.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | The media binding in the profile, checked on receipt | A profile with a forged or mismatched binding is refused; a direct binding is never published |
| 2 | Viewing endpoints: relay-only, fresh key, dropped after use; found posts' files fetched through them | Tests: a node with no follows plays a found video; the creator's node records no known endpoint and no public ID for the fetch; a packet capture shows the viewing endpoint only to the relay; trusted followers' posts aren't fetched; one author's endpoint isn't reused for another |
| 3 | Real nodes A, B and C, C as directory | Alice's node, following nobody, plays Kim's found video over the relay; Kim's `osp peers` shows only an unknown endpoint |

**Revisit** if a relay of our own changes what relays see, or if fetching through viewing endpoints proves too slow to start.
