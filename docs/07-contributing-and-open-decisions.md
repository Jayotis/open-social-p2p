# Contributing and Open Decisions

**Status:** A Stage 1 prototype exists ([roadmap](06-roadmap.md)). Nothing is production-ready.

## How to contribute now

Read the [vision](01-vision-and-principles.md) and [architecture](02-system-architecture.md). Help identify missing requirements, compare credible existing projects, propose test scenarios, document abuse/privacy risks, or draft decision records.

Useful GitHub issue formats:

- **Problem:** What user problem or architectural risk is being addressed?
- **Proposed approach:** What alternatives could solve it?
- **Tradeoffs:** Reliability, privacy, cost, federation compatibility, maintenance burden.
- **Acceptance criteria:** How would we know it works?
- **Sources/references:** Existing standards, software, measurements or research.

Decisions should be recorded openly, with alternatives and reasons, before significant implementation work begins.

## Major open decisions

1. ~~Mastodon, Friendica, another ActivityPub engine, or a purpose-built thin social layer?~~ Decided: a native protocol for Stage 1, federation through an ActivityPub bridge in Stage 2 ([ADR 0003](decisions/0003-native-social-protocol-v0.md)). How the bridge works is open.
2. What exactly is a portable identity across social services and media nodes?
3. How are creator-authenticated post/media manifests represented and updated?
4. ~~What is the best P2P transport approach for native desktop, Flutter mobile and browser clients?~~ Decided for desktop and mobile: iroh and iroh-blobs ([ADR 0002](decisions/0002-rust-and-iroh.md)). Browser clients are open.
5. ~~Who provides peer discovery, and what information is revealed?~~ Decided for M4: the node's own index, directories anyone can run, and exact lookups by domain or key, with messages between nodes over Tor ([ADR 0014](decisions/0014-finding-people-and-private-messages.md)). Media's default, who may connect directly, is set by measuring before M4's invited alpha.
6. How will peer scoring balance throughput, responsiveness, reliability and upload fairness?
7. Which replication and eviction defaults are safe, understandable and useful?
8. What media is publicly cacheable, and what are the enforcement boundaries for private groups?
9. How will users without an always-on machine participate? For now they don't: joining means running a node ([ADR 0006](decisions/0006-desktop-first-always-on-node.md)), and hosted nodes would need a new decision record.
10. How will copyright notices, safety reporting and moderation operate in a replicated network?
11. What parts must remain available when the original device and all known replicas are offline?
12. What minimum central costs remain, and who pays for relay traffic?

## Architecture decision record template

Add a record under [`docs/decisions/`](decisions/) with:

- Decision title and status: proposed / accepted / superseded.
- Context and specific problem.
- Options considered and factual evidence.
- Decision and rationale.
- Security, privacy, operational and licensing consequences.
- Validation plan and conditions for revisiting it.

## Scope reminder

This repository covers the social platform, Flutter app, companion and media P2P network. A separate home-hosted personal web search crawler is **not** part of this project.
