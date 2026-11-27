# ADR 0044: Project relays as bootstrap and fallback while carrying is built

**Status:** Accepted (2026-11-19), the maintainer's decision after review. Amends [ADR 0043](0043-the-media-plane-before-the-alpha.md) without changing its timing: carrying is still built in P11, before the Product Alpha. This record adds relays the project runs, as the network's starting point and fallback while carrying grows. Carries out [ADR 0007](0007-transport-privacy-defaults.md#consequences)'s "project or community relays are needed before a public release". Not built.

## Context

Media between nodes without a direct link goes through a relay. Until now that has been n0's public relays, and they slow OSP down on purpose. In the lab, n0's relay logged that it was rate-limiting the sending node ("outbound relay traffic is being throttled") while a 108.5 MB video crawled through it. The same video went about 4 times faster through a relay the project ran on a rented server, and the cap then was the test line, not the relay.

[ADR 0043](0043-the-media-plane-before-the-alpha.md) builds carrying before the alpha. Carrying is part of OSP's architecture, not an optimization for later: new nodes contribute safely from the middle of transfers, and the network gradually stops depending on central relays. But on an empty network there's nobody to carry yet, and seed carriers need volunteers the project doesn't have. A relay on a rented server needs nobody's home connection. It costs about €10 a month, which the maintainer pays.

So the project's relay solves the empty network and gives a dependable fallback while carrying develops. It is not a reason to defer carrying. A relay is not carrying: its operator sees both ends of everything it carries, so hiding addresses from any one operator still needs hops run by different people.

## Decision

1. **The project runs relays.** One relay on a rented server under the project's domain, and later a second for when the first is down. They are iroh's own relay server, unchanged.

2. **Carrying keeps ADR 0043's timing.** It's built in P11, before the Product Alpha, with the design note first. Until enough nodes carry, the project's relays fill in. ADR 0043's seed carriers stay optional: a relay or a carrier the maintainer runs can be a seed, and volunteers join as they come.

3. **The order a transfer tries,** while carrying is built:

   1. **Direct,** when both sides' rules allow it ([ADR 0018](0018-earned-direct-connections.md)).
   2. **An OSP carrying path,** when one is available ([ADR 0043](0043-the-media-plane-before-the-alpha.md)).
   3. **The project's relays.**
   4. **n0's relays,** as an emergency fallback.
   5. **Fail,** with a plain message.

   A link between two hops that can't connect directly falls back in the same order, project relays before n0. n0 sees what the project's relay sees ([who sees what](#who-sees-what)), only slower, so keeping it as a last resort costs no privacy the project relay doesn't already cost.

4. **Project relays before n0, on purpose.** iroh picks a node's home relay by lowest latency among the relays it's given, not by order. Given both the project's relays and n0's, many nodes would pick n0 and be throttled again. So the node gives iroh only the project's relays while one of them answers, adds n0's when none does, and drops n0's again once one is back.

5. **Named by the installer, never built in.** The installer writes the relays into the node's `config.toml`, as it writes the directories. The `osp` binary keeps no default ([ADR 0015](0015-nodes-meet-first.md), [the first directory is temporary](0027-directory-bootstrap-list-and-mirror.md)). Once ADR 0027's signed list is built, it names the relays too, so a relay can be added or replaced without a new release.

6. **Locked by a shared token, only for the closed test.** The relay accepts only clients that present a shared token. The installer writes it, the node sends it to its own relays, and a paired phone gets it from its node in the pairing code. A phone paired before the relays or token change takes the new ones from its node, asking at most once an hour, as long as it can still reach the node: so a change runs old and new side by side until phones have it *(built 2026-11-26)*. The token is never in the repository or in public text. **This locking is only for the closed P11 and private TestFlight phase. It can't be relied on once the installer is publicly available:** anyone who downloads the installer can read the token. Before the installer is public, one of these must be true:

   - carrying has made the relay a fallback rather than the main path, so little depends on the lock;
   - the relay is deliberately public, with the abuse controls below;
   - access control has changed to something that doesn't depend on a secret everyone has.

7. **Limits on every connection.** Even with the lock, each client may send at most 4 MB/s into the relay (bursts up to 8 MB), and the relay accepts at most 10 new connections a second. These are starting values, raised if they slow real use.

8. **A hard traffic limit on the server.** The provider's traffic allowance isn't a limit: past it, the provider keeps carrying traffic and bills the overage. So the server counts its own billed traffic for the month and:

   - **alerts the maintainer** well before the allowance;
   - **refuses new relay traffic** before paid overage starts. Nodes then fall back as in point 3.

   The exact thresholds are set when it's deployed.

9. **The directory may move to the same server.** The first directory runs on a home machine that also does other work, and its fallback isn't always on. The relay's server is always on. Moving the first directory there is an option, decided on its own, not part of this record. *Done 2026-11-20:* the first directory now runs there as its own node, which serves the directory and nothing else, and the earlier one on a home machine is its fallback. It stays as temporary as before: its names come from the release's `directories.txt`, never from the program.

### Who sees what

| Machine | Sees | Doesn't see |
| --- | --- | --- |
| The project's relay (the maintainer) | Both ends' addresses on each transfer it carries, which endpoints talk, the timing and the number of bytes. Also which endpoints share an internet connection (below) | The contents, which are encrypted end to end |
| n0's relay, as a fallback and before this record | The same, for what it carries | The contents |
| A carrier next to an end ([ADR 0043](0043-the-media-plane-before-the-alpha.md#who-sees-what)) | That end's address and the other hop, never both ends | The contents |

**Endpoints tied together by their connection.** A node runs several endpoints that are meant to look unrelated: its public, media and machine endpoints, and the throwaway viewing endpoints of [ADR 0024](0024-watching-found-posts-without-your-identity.md). When they come from the same home address through the same relay, the relay can infer they belong to the same internet connection. A viewing endpoint still keeps the creator from learning who watched. But the relay's operator could infer it, by matching the viewing endpoint's address to the media endpoint that address also uses. n0's relays have the same view, but among a very large shared population, where the inference says less. With twenty alpha nodes on one OSP relay it says much more. This doesn't break ADR 0024's design: it's the relay's view, and carrying is what removes it. But testers should know who could make it: the maintainer.

**Harm, likelihood and effort.** The relay's operator can list which home addresses fetch what from which endpoints, and tie a node's endpoints together. That's what n0 can do today, moved to the maintainer, whom testers already trust with the directory, the hide list and the app. Anyone else needs the server itself: its login is by key only, and its firewall opens only the relay's ports and SSH. Carrying spreads this view across operators so that no single one sees both ends. That's why its timing stays.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | `[network] relay_token`, sent to the node's own relays (iroh's `RelayConfig::with_auth_token`) from every endpoint the node runs | A lab node with the token reaches the locked relay, one without is refused, and a video goes between two lab nodes through it |
| 2 | n0 as emergency fallback: n0's relays join the node's relays only while none of the project's answers | With the project relay stopped, two lab nodes still exchange a video through n0. Once it's back, both return to it within minutes |
| 3 | The server's traffic limit: an alert and a refusal of new relay traffic before overage | With a test threshold set low, the alert fires and new relay connections are refused, and nodes fall back as in step 2 |
| 4 | The installer writes the relays and the token, as it writes the directories | A fresh install on a test machine uses the project relay with no hand edits |
| 5 | The pairing code carries the node's relays and token. The phone uses them, with the same fallback | A phone on mobile data plays a video from its node through the relay. Pairing on the home network still works |
| 6 | ADR 0027's signed list names relays, when 0027 is built | A relay added to the list reaches nodes without a release |

Carrying's own stages are ADR 0043's. Its stage D is measured against the project relay as well as n0.

If the token leaks beyond testers, it's changed on the relay and in the installer, and nodes get the new one with their next update.

## Consequences

- **Testers' videos move at about the speed of the slower end's upload,** not n0's limit, from the first day.
- **The maintainer runs a lasting service,** for now. It can be turned off: nothing is built in, and the list can point elsewhere.
- **About €10 a month for each relay,** with traffic held below paid overage by the server's own limit.
- **No regression when the relay is down:** nodes fall back to n0, slower but working.
- **A small relay ties endpoints together** more than n0's shared ones do ([who sees what](#who-sees-what)), until carrying takes strangers' traffic.
- **Nodes without the token can't use the project relay.** During the closed test that's the point. They still reach others through n0, slowly.
- **The phone and the node change together.** The pairing code's format changes, so an older app can't use the relay until it's updated.

## How relays retire

Relays carry less as the network grows:

- **People who work together go direct.** Jimmy follows Kim and hosts her posts. After checked work for each other, their nodes earn a direct link ([ADR 0018](0018-earned-direct-connections.md)) and stop using relays between them.
- **Strangers go through carriers.** When Alice opens a found post from Kim, neither may learn the other's address, so that traffic never goes direct. It goes through hops run by different people ([ADR 0043](0043-the-media-plane-before-the-alpha.md)), where no single machine sees both ends. New nodes carry from the middle and earn places next to ends.
- **The relays become a fallback,** for pairs whose routers can't connect and nobody to carry. They're turned off when there are enough users to carry the rest, the rule the first directory follows.

## Options considered

1. **Stay on n0's relays until carrying is built.** No work, no cost. Throttled, and the alpha feels broken.
2. **Project relays instead of carrying for the alpha** (this record's first draft). Fast media soonest, but it makes a central relay the network's main path and leaves the privacy work for later.
3. **Project relays as bootstrap and fallback, carrying on ADR 0043's timing (this record).** Fast media from the start, a dependable fallback, and the network moving off the relays as carrying grows.

## Revisit when

- the installer is about to become publicly available (point 6);
- the relay's traffic nears its alert, or a second relay is needed for load;
- the token leaks;
- carrying is built, so strangers' traffic moves off the relays;
- the first directory moves, or a tester asks who can see what.
