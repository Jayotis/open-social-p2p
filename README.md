# Open Social P2P

**A proposed open-source, decentralized social network with user-owned media and peer-to-peer distribution.**

**Status: Concept and architecture planning. No production application has been developed.**

## The idea

Create a familiar alternative to Facebook, X and YouTube without requiring a single company to store and serve everyone's photos and videos.

Members should be able to sign up normally, connect with people, publish posts, join communities and watch videos **without learning about federation or configuring servers**.

A **Flutter mobile app** will be the primary iOS/Android interface. An **optional desktop companion** will let people host media on their own Linux, Windows or macOS computers. With permission, people who reshare public posts can retain and serve verified copies, helping popular content spread across many peers instead of overloading the original author.

Peers should choose sources according to **locally measured speed, latency, reliability and available capacity**, and respect bandwidth/storage limits. Security, IP-address privacy, audience controls, moderation and deletion limitations are essential design requirements.

**This repository is documentation-first. We are defining requirements and evaluating existing standards before writing application code.**

## Project documentation

| Document | Contents |
| --- | --- |
| [Vision and principles](docs/01-vision-and-principles.md) | Goals, user experience, non-goals and guiding values |
| [System architecture](docs/02-system-architecture.md) | Social layer, Flutter, media nodes, P2P transport and integrations |
| [Peer distribution and scoring](docs/03-peer-distribution-and-scoring.md) | Replication on reshare, fairness, verification and source selection |
| [App and companion requirements](docs/04-app-and-companion-requirements.md) | Mobile and desktop features and user journeys |
| [Security, privacy and governance](docs/05-security-privacy-and-governance.md) | Threat model, consent, moderation and policy requirements |
| [Development roadmap](docs/06-roadmap.md) | Research, proof of concept, integration, alpha and release milestones |
| [Contributing and open decisions](docs/07-contributing-and-open-decisions.md) | How to help and architectural questions needing review |

## Candidate technologies — not yet selected

- **Flutter:** planned iOS/Android client.
- **ActivityPub:** candidate federation standard.
- **Mastodon or Friendica:** possible social backends.
- **PeerTube:** possible video infrastructure.
- **libp2p, IPFS/Bitswap, WebRTC:** technologies to evaluate for P2P delivery, networking and verified chunks.

Not every federated service supports the same features. Combining these systems into one account and interface will require research and possibly custom integration.

## Key design questions

Can resharers voluntarily improve availability by seeding verified copies? How can local observations assign work fairly across faster and slower peers? How can we protect private groups, avoid revealing home network addresses, support users without computers, and manage abuse in a replicated network?

See the [open decisions](docs/07-contributing-and-open-decisions.md) to participate in those discussions.

## Separate search project

A personal search engine with a home-hosted crawler is a **separate idea and not part of Open Social P2P**.

## License

See [LICENSE](LICENSE) for the repository's current AGPL-3.0 license. Documentation licensing and third-party compatibility will be reviewed as the project develops.
