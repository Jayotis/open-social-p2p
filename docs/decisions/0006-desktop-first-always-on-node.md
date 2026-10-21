# ADR 0006: Desktop-first, always-on personal node

**Status:** Accepted (2026-10-21). Supersedes the hosted-node plan in [ADR 0001](0001-home-node-architecture.md) and amends a non-goal in [Vision and principles](../01-vision-and-principles.md).

## Context

ADR 0001 made the phone a thin client of a home node, and planned hosted home nodes so that people without a computer could join. The earlier documents also listed "making a desktop computer mandatory to open a social account" as a non-goal.

On reflection, the home node is not an optional extra. It is the person's gateway to the network: it does the heavy networking, holds their media so the phone doesn't need the storage, keeps their phone's address private, and saves mobile data. A phone without its node would need more data, more storage and more exposure. That is the opposite of the experience this project is trying to offer.

## Options considered

1. **Phone-only accounts through hosted nodes from day one.** Widest audience, but someone must run, fund and moderate hosting for most users, which recreates the central provider this project exists to avoid.
2. **Personal node required; hosted nodes possible later.**
3. **Personal node required permanently.**

## Decision

Option 2.

- **Joining the network means installing the node** on a desktop or a small always-on computer. The node creates the person's identity locally ([ADR 0004](0004-signed-objects-and-ids.md)); there is no central sign-up and no password database. The phone is then paired with the node by QR code.
- **The identity belongs to the person, not the machine.** The identity key is separate from the machine's node key, so it can be backed up and moved to a replacement computer. Backup and recovery must be designed before any public release.
- **Always-on is an expected requirement.** The node starts at boot without anyone logging in, reconnects after network failures, and reports its health to the phone. The phone app shows prominently whether the home node is reachable. When it isn't, the phone shows cached content, and posting or fetching waits until the node is back.
- **Uptime rewards itself.** People keep their node running because their own phone depends on it. The network does not award points for uptime.
- **Suitable hardware:** an always-on desktop, a mini PC, or a single-board computer. Builds for 64-bit ARM Linux are planned and need testing. A laptop that travels with its owner is not a suitable home node.
- **The door stays open.** The node software keeps running unchanged on a server, so hosted nodes could be offered later. That would need a new ADR covering who runs them and who pays.

## Consequences

- **Simpler fairness:** every participant has a node, so everyone can host and give back. The phone-only "slow lane" problems in the [critical review](../reviews/2026-10-21-review-of-docs-12-and-13.md) (findings A6 and A7) largely disappear.
- **Narrower audience:** people who own only a phone cannot join at first. This is a deliberate trade-off.
- **Energy:** a desktop left on draws tens of watts; a mini PC or single-board computer draws a few. Setup should recommend low-power hardware.
- **Home connections:** some residential internet plans restrict running servers or cap monthly data. Setup should mention this, and upload limits must be easy to set.
- **Security:** an always-reachable node is a target. It needs hardening, least-privilege file access, and signed automatic updates.
- **Support burden:** people are running their own infrastructure, so installation, updates and recovery must be close to effortless.

## Validation and revisit conditions

- A nontechnical tester installs the node, pairs a phone and uses it for a week without help.
- Node uptime and reconnection behaviour are measured on the maintainer's test machines.
- Revisit if testing shows most interested people have no suitable always-on computer, or once hosted nodes have a credible funding and governance model.
