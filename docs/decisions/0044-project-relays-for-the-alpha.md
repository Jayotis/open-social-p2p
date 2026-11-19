# ADR 0044: Project relays for the alpha, locked while testing

**Status:** Proposed (2026-11-19), drafted at the maintainer's request, for the maintainer's review. Amends [ADR 0043](0043-the-media-plane-before-the-alpha.md): the alpha's media goes through relays the project runs, and volunteer carrying is built after the alpha. Carries out [ADR 0007](0007-transport-privacy-defaults.md#consequences)'s "project or community relays are needed before a public release". Not built.

## Context

Media between nodes without a direct link goes through a relay. Until now that has been n0's public relays, and they slow OSP down on purpose. In the lab, n0's relay logged that it was rate-limiting the sending node ("outbound relay traffic is being throttled") while a 108.5 MB video crawled through it. The same video went about 4 times faster through a relay the project ran on a rented server, and the cap then was the test line, not the relay. An alpha on n0's relays would feel broken, and testers who leave because of it tell others.

[ADR 0043](0043-the-media-plane-before-the-alpha.md) answered with volunteer carrying before the alpha, starting from seed carriers: volunteers the maintainer knows, listed by the installer. The project has no such volunteers yet, and asking people to carry strangers' traffic from home is a hard sell before there's anything to show them. A relay on a rented server needs nobody's home connection. It costs about €10 a month, which the maintainer pays.

A relay is not carrying. Its operator sees both ends' addresses on every transfer it carries, as n0 does today. Hiding addresses from the operator still needs ADR 0017's hops run by different people. This ADR doesn't change that goal. It changes the order.

## Decision

1. **The alpha uses project relays.** The project runs one relay on a rented server under the project's domain, and later a second for when the first is down. They are iroh's own relay server, unchanged. n0's relays stay only for nodes set up without the project's relays.

2. **Named by the installer, never built in.** The installer writes the relays into the node's `config.toml`, as it writes the directories. The `osp` binary keeps no default ([ADR 0015](0015-nodes-meet-first.md), [the first directory is temporary](0027-directory-bootstrap-list-and-mirror.md)). Once ADR 0027's signed list is built, it names the relays too, so a relay can be added or replaced without a new release.

3. **Locked to OSP nodes while testing.** The relay accepts only clients that present a shared token. Strangers can't use its bandwidth, and nobody outside the test can tie up the project's only relay. The installer writes the token, and the node sends it to its own relays. A paired phone gets it from its node in the pairing code, so it can reach its node through the relay. The token is never in the repository or in public text. It keeps strangers off the relay's bandwidth. It isn't a secret against testers, who all have it.

4. **Limits on every connection.** Even with the lock, each client may send at most 4 MB/s into the relay (bursts up to 8 MB), and the relay accepts at most 10 new connections a second. These are starting values, raised if they slow real use.

5. **Carrying comes after the alpha.** ADR 0043's carrying (and ADR 0017's hops and newcomers' middle turns) is built after the Product Alpha, with the design note still first. ADR 0043 §1 stands: n0's relays aren't OSP's media network. The project's relays aren't meant to be either. They're a bridge until there are enough people to carry.

6. **The directory may move to the same server.** The first directory runs on a home machine that also does other work, and its fallback isn't always on. The relay's server is always on. Moving the first directory there is an option, decided on its own, not part of this record.

### Who sees what

| Machine | Sees | Doesn't see |
| --- | --- | --- |
| The project's relay (the maintainer) | Both ends' addresses on each transfer it carries, which endpoints talk, the timing and the number of bytes | The contents, which are encrypted end to end |
| n0's relay, before this record | The same, for every OSP transfer | The contents |
| A node at the other end | Nothing about the other end's address | |

**Harm, likelihood and effort.** The relay's operator can list which home addresses fetch from which endpoints. That's what n0 can do today, moved to the maintainer, whom testers already trust with the directory, the hide list and the app. Anyone else needs the server itself: its login is by key only, and its firewall opens only the relay's ports and SSH. A relay few people use says more about each transfer than n0's shared ones ([ADR 0024](0024-watching-found-posts-without-your-identity.md#consequences)), but in the alpha every tester knows who runs it.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | `[network] relay_token`: sent to the node's own relays (iroh's `RelayConfig::with_auth_token`) from every endpoint the node runs | A lab node with the token reaches the locked relay, one without is refused, and a video goes between two lab nodes through it |
| 2 | The installer writes the relays and the token, as it writes the directories | A fresh install on a test machine uses the project relay with no hand edits |
| 3 | The pairing code carries the node's relays and token. The phone uses them instead of n0's | A phone on mobile data plays a video from its node through the relay. Pairing on the home network still works |
| 4 | ADR 0027's signed list names relays, when 0027 is built | A relay added to the list reaches nodes without a release |

The token is a starting point. If it leaks beyond testers, it's changed on the relay and in the installer, and nodes get the new one with their next update.

## Consequences

- **Testers' videos move at about the speed of the slower end's upload,** not n0's limit.
- **The maintainer runs a lasting service,** for now. It can be turned off: nothing is built in, and the list can point elsewhere.
- **A cost of about €10 a month for each relay,** and the server's traffic allowance (20 TB a month) bounds what it can carry. Past it, the provider bills by the terabyte.
- **One relay is one point of failure.** If it's down, nodes reach each other only directly or not at all, until the second relay is added or the node falls back to n0 (a later choice).
- **Nodes without the token can't reach a node whose home is the project relay,** except directly. During the test that's the point. Lifting the lock is decided before any public release.
- **The phone and the node change together.** The pairing code's format changes, so an older app can't use the relay until it's updated.

## How relays retire

Relays carry less as the network grows:

- **People who work together go direct.** Jimmy follows Kim and hosts her posts. After checked work for each other, their nodes earn a direct link ([ADR 0018](0018-earned-direct-connections.md)) and stop using relays between them.
- **Strangers go through volunteers.** When Alice opens a found post from Kim, neither may learn the other's address, so that traffic never goes direct. It moves to carriers run by different people ([ADR 0017](0017-one-hop-forwarding.md)), where no single machine sees both ends.
- **The relays become a fallback,** for pairs whose routers can't connect directly and nobody to carry. They're turned off when there are enough users to carry the rest, the rule the first directory follows.

## Options considered

1. **Stay on n0's relays.** No work, no cost. Throttled, and the alpha feels broken.
2. **Seed carriers first (ADR 0043).** Better privacy from day one, but it needs volunteers the project doesn't have, and the most network work since M4 before anyone can test.
3. **Project relays now, carrying after (this record).** Fast media within days, at the cost of the maintainer seeing both ends, as n0 does now.

## Revisit when

- the relay's traffic nears its allowance, or a second relay is needed for load;
- the token leaks, or the lock is lifted before a public release;
- carrying is built, so strangers' traffic moves off the relays;
- the first directory moves, or a tester asks who can see what.
