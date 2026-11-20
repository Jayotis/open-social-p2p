# ADR 0043: OSP's own media plane, built before the alpha

**Status:** Accepted (2026-11-18), the maintainer's decision after P11's failure journeys. Amends [ADR 0017](0017-one-hop-forwarding.md), which is now built in P11 and with seed carriers, and [ADR 0030](0030-product-alpha-before-testers.md) §3, which listed ADR 0017 among the records not built before the Product Alpha. Amended 2026-11-20, the maintainer's decision: every node carries from the middle by default (§8), and relays fall back in the order of [ADR 0044](0044-project-relays-for-the-alpha.md) (§1). Not built.

## Context

Media between nodes without a direct link goes through n0's public relays, which are meant for development and hobby use ([ADR 0017](0017-one-hop-forwarding.md#context)). In P11's failure journeys ([product-alpha-journey.md](../testing/product-alpha-journey.md#failures-p11-part-6-2026-11-18)), a follower fetching a 108.5 MB video after its creator's node restarted reached it only through a public relay, at a 4.4 s round trip and about 0.25 MB/s. At that speed a 10-minute video takes about 7 minutes to arrive. An alpha whose videos crawl tests the relay, not OSP.

ADR 0017 already describes the network meant to replace it: nodes carry encrypted media, each end picks the hop next to it, and newcomers carry only in the middle. It was to be built after the alpha, and only if a simulation showed it worth building. It also lets an end use a hop only once that hop has earned it, through ADR 0018's direct links or finished jobs on 3 days in 30. On a new network nobody has, so every transfer would stay on n0 for weeks.

## Decision

1. **Relays are a fallback, not OSP's media network.** When no hop can carry a transfer, it goes through the project's relays ([ADR 0044](0044-project-relays-for-the-alpha.md)), and through n0's only when those can't. Sustained n0 relay performance is treated as inadequate for OSP video. Relays still coordinate hole punches.
2. **ADR 0017's carrying is built in P11, before the Product Alpha.** Its stage A no longer decides whether to build. The simulator sizes the limits instead: upload shares, how often a newcomer gets a middle turn, and how much a carrier holding 10% of hops sees.
3. **Never over Tor.** Video doesn't go over Tor ([ADR 0017](0017-one-hop-forwarding.md#options-considered)). Tor still carries the small requests that set a transfer up, as it does today.
4. **The path, as in ADR 0017.** The viewer V picks a hop next to it (P), and the host H names one next to it (Q): H → Q → P → V. If both ends pick the same machine, it's one hop. On a newcomer's turn a machine with no standing goes between them, and sees neither end's address.
5. **Encrypted end to end,** from V's media endpoint to H's, with a layer for each hop as ADR 0017 says. No hop can see or change what it carries. The layers come from a reviewed design, picked in a design note before any code.
6. **Seed carriers may sit next to an end from the start.** The installer passes a short list of carriers, `carriers.txt`, from the release, as it passes `directories.txt`. It is never built into the software ([the first directory is temporary](0027-directory-bootstrap-list-and-mirror.md)). A machine on that list may be the hop next to a viewer or a host from day one. Everyone else starts in the middle and earns a place next to an end as ADR 0017 says: jobs finished for that node on 3 different days in the last 30. Once a node has earned hops of its own, it picks those before seed carriers.
7. **A host's hop** may be a direct peer, as in ADR 0017, or a seed carrier. Until ADR 0018 is built, that means a seed carrier.
8. **Every node carries, from the middle** (amended 2026-11-20; it was ADR 0017's question). New nodes carry as middle hops by default, and sit next to an end only once they've earned it (§6). Setup tells the owner, as [ADR 0017](0017-one-hop-forwarding.md#told-at-setup) says, and asks nothing. The owner controls the resources: upload share, jobs at once, what happens on a metered connection, and a pause under Advanced for troubleshooting. Letting the hops your node picks see its address is a separate privacy rule: it no longer waits for ADR 0018's tests, and is asked as its own question:

   > Your node can fetch and send video through other nodes it picks, which is much faster than the public fallback. The node next to yours sees your internet address and which node you're fetching from, as the fallback does today, but never what's inside.

   Yes is recommended. A node that says no keeps its own leg on n0's relays. ADR 0018 asks its own question when it's built.

### Who sees what

| Machine | Sees | Doesn't see |
| --- | --- | --- |
| A seed carrier next to V | V's address, which host V reaches, the timing and the number of bytes | V's account, H's address, the contents |
| A seed carrier next to H | H's address, whose node it is, the timing and the number of bytes | V's address, the contents |
| A newcomer in the middle | The two hops' addresses, the timing and the number of bytes | Either end's address, the contents |
| n0's relay today, for comparison | Both ends' addresses and the timing, on every transfer it carries | The contents |

**Harm, likelihood and effort.** A seed carrier's operator learns the addresses of the nodes that picked it, and which hosts they fetch from. That's less than n0 sees today, but it's spread across carriers rather than one company. To collect addresses, an attacker has to get onto the list, which the maintainer controls, or run carriers that do real work for weeks before any end picks them. Even then each one sees only the ends that picked it. Seeing both ends of a transfer needs both ends' hops, about p² of transfers for an operator with a share p of the hops next to ends. Stage D measures it.

## Building it, in P11

Each mark is set before its run.

| Stage | Work | Gate |
| --- | --- | --- |
| Design | A short note picking the layer construction from a reviewed design, the wire format, and how a hop checks it is on an OSP path and not a general proxy: [carrier circuits](0043-design-note-carrier-circuits.md), accepted 2026-11-20 | Note reviewed; nothing built before |
| Sizing | `osp-sim`: B8 with every node carrying within its limits, seed carriers, newcomers, and the project's relays then n0 as fallback, from zero standing | Limits and the newcomer rate set from it |
| B | Seed list, picking hops, newcomers' middle turns, the layers, carrying on by default with its controls (upload share, jobs at once, metered connections, the pause), busy, credit for finished jobs, setup telling the owner, the question on the node's own address, fallback to the project's relays then n0 | On the lab nodes, a video goes H → Q → P → V and on a newcomer's turn through four hops. In a packet capture, neither end learns the other's address, the viewer's hop learns no account, and a newcomer learns neither end's address. A skipped hop breaks the transfer. A new node carries in the middle with no setup answer, within its limits, and its owner's own traffic is unharmed |
| C | ADR 0017's hostile tests: loops, floods, dropping, skipping a hop, naming a host that didn't pick it, asking for addresses, a newcomer trying to get next to an end | No open proxy, no amplification, no stranger gets an end's address, failures fall back to relays |
| D | A real run on the home machines and test VMs, the same 108.5 MB video as P11 part 6 | Throughput and time to start playing set as marks before the run, measured against the project relay and n0 |

## Consequences

- **P11 grows by the largest piece of network work since M4,** and the Product Alpha comes later.
- **Home upload:** each byte through two hops is uploaded from three homes, about 3.1 with newcomers' turns ([ADR 0017](0017-one-hop-forwarding.md#consequences)). Every node has bounded carrying resources (an upload share and jobs at once) and always puts its owner's own traffic first.
- **Seed carriers' operators** answer for what their connection sends, as every carrier does, and see the addresses of the ends that pick them.
- **Every machine shows its address** under its machine name to the machines next to it when it carries, never tied to a person ([ADR 0017](0017-one-hop-forwarding.md#consequences), changing ADR 0015).
- **ADR 0022** follows carrying as the roadmap says. When strangers' `Known` lists close is decided once stage C passes.
- **Licensing:** the design note names the `hpke` crate (MIT or Apache-2.0), pinned when building starts.

## Revisit when

- the lab run misses its marks, or carrying harms an ordinary home connection;
- carrying materially harms ordinary home connections, or people often need to pause it for troubleshooting;
- testers turn down the question on their node's own address;
- the seed list grows past a few machines the maintainer knows, so it needs a rule of its own;
- ADR 0018 is built, so hosts' hops come from direct peers again.
