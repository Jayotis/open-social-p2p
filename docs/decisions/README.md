# Architecture Decision Records

Each record captures one significant decision: the problem, the options considered, what was chosen and why, and when to revisit it. The format follows the template in [Contributing and open decisions](../07-contributing-and-open-decisions.md#architecture-decision-record-template).

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-home-node-architecture.md) | Home node architecture: phone as a thin client | Accepted; hosted-node plan superseded by 0006 |
| [0002](0002-rust-and-iroh.md) | Rust core on iroh and iroh-blobs | Accepted |
| [0003](0003-native-social-protocol-v0.md) | Native node-to-node social protocol (v0) | Accepted |
| [0004](0004-signed-objects-and-ids.md) | Signed objects and content identifiers | Accepted |
| [0005](0005-media-profile-v1.md) | Media profile v1: 720p, low bitrate, metadata stripped | Accepted |
| [0006](0006-desktop-first-always-on-node.md) | Desktop-first, always-on personal node | Accepted |
| [0007](0007-transport-privacy-defaults.md) | Transport privacy defaults: private messages, direct media | Accepted |
| [0008](0008-device-pairing-and-client-protocol.md) | Device pairing and the client protocol | Accepted |
| [0009](0009-identity-backup-and-recovery.md) | Identity backup and recovery | Accepted; partly built |

Proposals under review that may become ADRs: [peer reciprocity and recognition](../12-peer-reciprocity-and-recognition.md) and [hosting roles and the follower swarm](../13-hosting-roles-and-follower-swarm.md). See the [critical review](../reviews/2026-10-21-review-of-docs-12-and-13.md) for what blocks them.

A record marked *Accepted* can still be superseded. Write a new ADR that links back to it rather than rewriting history.
