# ADR 0024: Watching a found post without showing who you are

**Status:** Proposed (2026-11-09). Approved in principle by the maintainer the same day, and revised after that review; stays Proposed until the implementation is agreed. Settles what [ADR 0015](0015-nodes-meet-first.md) left open ("a found post's media opens only once its author is followed").

Revised 2026-11-09, after the maintainer's review:

- The 10 minutes govern reuse for new requests. They don't end a transfer under way, which has its own idle and resource limits.
- Every step of opening a found post now has a stated identity, and none may fall back to the viewer's own.
- A source that can't be reached privately fails plainly, rather than costing privacy.
- Limits on strangers hold across all throwaway keys at once, with tests of key churn.

## Context

Alice installs OSP, follows nobody, and finds Kim's video through a directory ([ADR 0015](0015-nodes-meet-first.md)). Today she can't watch it: her node knows no sizes and no hosts for a found post's files, and it asks only the nodes it follows who hosts a file. The maintainer wants public posts viewable without telling the creator who watched (2026-11-09), along the whole path, not just the first request.

What would identify Alice today, if her node simply asked Kim's node, as it does for posts it follows:

1. **The request.** `WhoHosts` goes over Tor from Alice's public endpoint. Tor checks the key of the node that dials, so Kim's node knows exactly which node asked about which file.
2. **The download.** It comes from Alice's media endpoint. Its key is stable, and its binding to her public ID ([ADR 0014](0014-finding-people-and-private-messages.md)) is given to every node she follows. It's also named in `WhoHosts` replies to other followers when she shares copies. So even when Kim's node hasn't seen that binding, the endpoint is one pseudonym across everything Alice watches. Anyone who has seen the binding, now or later, can turn it into her identity. Kim would learn it the day Alice follows her.
3. **The relay.** A media endpoint has no IP address of its own, so the download goes through the relay Kim's media endpoint uses, n0's for now. That relay sees Alice's node's IP address and which endpoint she fetched from, as it does for every relayed transfer today.

So a fix for the request alone leaves the download identifying her.

## Options considered

1. **Ask from the public identity, and say so.** Smallest. Kim learns, by public ID, who watched what.
2. **A throwaway identity for the request only.** The download still comes from Alice's stable media endpoint: point 2 above.
3. **A throwaway endpoint for the whole path, with no request from Alice's identity at all.** Kim's profile carries her media endpoint's binding. Alice's node checks it, and fetches through the relay from a fresh, relay-only endpoint made for this viewing. Kim's node sees an unknown endpoint fetch a public file, as any stranger's.
4. **Media over Tor.** Hides the relay's view too. Too slow for video ([ADR 0014](0014-finding-people-and-private-messages.md#evidence-m4s-first-tests-2026-10-31)).

## Decision

Option 3, for public posts, from public sources only: the creator, and hosting resharers ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)).

### Media bindings in profiles

- A node with a media endpoint puts that endpoint's binding in its signed profile, beside its public node IDs. A media endpoint is relay-only and has no address to give, so publishing it shows only which relay it uses. Its ID already goes to anyone it serves.
- A direct endpoint's binding is never published.
- Alice's node checks a binding as any: both keys signed it, and the public ID is one the newest signed profile it holds lists. One that fails, or comes from an older profile than one it holds, is ignored.

### The viewing endpoint

- **A new key, relay-only** like a media endpoint, with no IP sockets. It's never bound to Alice's public ID, never given a binding, and never named to anyone.
- **One author at a time.** A thumbnail and its video, and that author's other posts, share one. Another author's post gets another.
- **New requests for 10 minutes** after it was made (a starting value). After that, anything new from that author gets a new key. A transfer already under way isn't cut off at the 10 minutes. It goes on until it ends, or until it has sent nothing for 30 s (idle), or until the viewer stops.
- **Resource limits on Alice's node:** at most 4 viewing endpoints at once (a starting value). A fifth waits for one to finish, rather than reusing one for a different author. Each lives at most an hour, whatever it's doing, and is dropped once its transfers end.
- **What it sends:** the question of who hosts a file, to a public source's media endpoint ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)), and iroh-blobs requests for the hashes of a post Alice's node holds, signed by its author. Nothing else.

### Every step of opening a found post, and which identity it uses

| Step | Comes from | Identity used |
| --- | --- | --- |
| The post and its author's profile | The directory's listing, already on Alice's node, read under its machine name ([ADR 0015](0015-nodes-meet-first.md)) | none: nothing new is sent |
| File sizes and hashes | The signed post itself | none |
| The thumbnail | The source's media endpoint, through the relay | the viewing endpoint |
| Who else hosts it | The creator's media endpoint, and the directory listings Alice's node holds ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)) | the viewing endpoint |
| The media | Those sources' media endpoints, through the relay | the viewing endpoint |
| Retries, a part taken over by a faster source, a busy source, moving to the next source | The same public sources only | the viewing endpoint, or a new one if its 10 minutes are up |

**No step falls back to Alice's own endpoints.** Her node's usual ways of fetching (asking the nodes it follows, `WhoHosts` over Tor, its media or direct endpoint) aren't used for a found post that its owner hasn't followed or shared. That's a separate code path, not the usual one with a flag, so a fallback can't reach the usual one by mistake.

### When no public source can be reached privately

The download fails, and Alice is told why in plain words: no public source could be reached without showing who she is. It never fills in quietly from her own identity. She can try again later, or choose to follow the author, a person's act that then uses her own endpoints as any follow does.

### What follows from watching

- **The copy stays private:** it's `cache`, served to Alice's devices only ([ADR 0012](0012-sharing-prefetched-copies.md)). Watching publishes nothing. Sharing is a separate act ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)).
- **What Alice's node records,** about the sources, not about her: each source's download outcomes, kept under the source's public ID in its private [ADR 0011](0011-measuring-peers.md) record, so a source that keeps failing is tried last or skipped. Nothing about the viewing key is kept.
- **When Alice follows Kim,** her node goes back to using its own endpoints for Kim's posts, as for any follow. **When she shares one,** the share is public anyway, and her node may finish fetching it with its own endpoints ([ADR 0025](0025-shared-posts-are-found-and-hosted.md)).
- **Only public posts.** Posts for trusted followers ([ADR 0013](0013-trusted-followers.md)) are never fetched this way.

### Limits at the source, across all throwaway keys

A viewer can make keys at will, so a source's limits on strangers count all of them together, never one key at a time:

- **Transfers:** strangers already share half a node's places for large files (`store.rs`), whatever key they come from. This doesn't change.
- **Connections:** at most a fixed number of connections at once from endpoints the node doesn't know (32, a starting value), in aggregate.
- **Who-hosts questions:** at most a fixed number an hour from unknown endpoints, in aggregate (600, a starting value), and at most one a minute from any one key.
- **When they're full,** a stranger is refused as busy, as now, and followers and the node's own devices are unaffected.

## Consequences

**What Kim's node learns:** that some endpoint it doesn't know fetched a public file, and when. Not the public ID, not the media endpoint, not an IP address. It can count fetches.

**What it can't hide:**

- **The relay** sees the viewing endpoint and Alice's node's IP address, which endpoint it fetched from, and how much. n0 runs the relays for now. That's the same as for every relayed transfer today ([ADR 0007](0007-transport-privacy-defaults.md)), and it's why relays run by the community stay an objective. The relay doesn't learn Alice's public ID from this.
- **Timing.** If Alice follows Kim a minute after watching, Kim's node may guess the two were one node. A node can't prevent guesses from timing.
- **The relay URL.** A viewing endpoint uses the same home relay as Alice's media endpoint. On n0's shared relays that says little. On a relay few people use, it narrows who it was.
- **Within one viewing endpoint's life,** Kim's node can link Alice's fetches of Kim's own posts to one viewer, not to who she is.
- **It's not anonymity.** Someone watching both Alice's connection and Kim's relay can match the traffic. Nothing may describe this as anonymous.

**For sources:** a crowd of throwaway keys gets no more than strangers' share in total, and builds no standing.

**Cost:** a new relay connection for each viewing endpoint, about a second, and one key pair. The profile grows by one binding, about 200 bytes. Playback may start a little later than for a followed post.

**What changes elsewhere:** the profile gains a field, an extension to AS2, ignored by older nodes. ADR 0014's rule that the media endpoint's ID goes to anyone the node serves becomes "goes to anyone": it was already public in effect.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | The media binding in the profile, checked on receipt | Tests: a profile with a forged or mismatched binding is ignored; one from an older profile is ignored; a direct binding is never published |
| 2 | Viewing endpoints and the separate path for found posts | Tests, with every request each of the node's endpoints sends recorded: opening a found post (thumbnail, sources, media, a retry after a busy source, a source that stops partway, and moving to the next) sends nothing from the public, media or direct endpoint and no Tor request. With every public source unreachable, it fails with the plain message and sends nothing from them either. A transfer under way past 10 minutes finishes; a new request after 10 minutes comes from a new key; an idle one is dropped after 30 s. A fifth author waits for a free endpoint. Trusted followers' posts aren't fetched |
| 3 | Limits at the source, across keys | Tests: 200 viewers with fresh keys get no more than strangers' share of places, the connection limit and the hourly question limit in total; one key asking every second is held to one a minute; followers and devices still get their places throughout |
| 4 | Real nodes A, B and C, C as directory | Alice's node, following nobody, plays Kim's found video over the relay. Kim's `osp peers` shows only an unknown endpoint, and her logs show no public ID or media endpoint of Alice's |

**Revisit** if a relay of our own changes what relays see, or if fetching through viewing endpoints proves too slow to start.
