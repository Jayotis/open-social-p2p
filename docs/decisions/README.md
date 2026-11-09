# Architecture Decision Records

Each record captures one significant decision: the problem, the options considered, what was chosen and why, and when to revisit it. The format follows the template in [Contributing and open decisions](../07-contributing-and-open-decisions.md#architecture-decision-record-template).

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-home-node-architecture.md) | Home node architecture: phone as a thin client | Accepted; hosted-node plan superseded by 0006 |
| [0002](0002-rust-and-iroh.md) | Rust core on iroh and iroh-blobs | Accepted |
| [0003](0003-native-social-protocol-v0.md) | Native node-to-node social protocol (v0) | Accepted |
| [0004](0004-signed-objects-and-ids.md) | Signed objects and content identifiers | Accepted; who signs objects changed by 0010 |
| [0005](0005-media-profile-v1.md) | Media profile v1: 720p, low bitrate, metadata stripped | Accepted |
| [0006](0006-desktop-first-always-on-node.md) | Desktop-first, always-on personal node | Accepted |
| [0007](0007-transport-privacy-defaults.md) | Transport privacy defaults: private messages, direct media | Accepted; phases 2 and 3 decided by 0014 |
| [0008](0008-device-pairing-and-client-protocol.md) | Device pairing and the client protocol | Accepted |
| [0009](0009-identity-backup-and-recovery.md) | Identity backup and recovery | Accepted |
| [0010](0010-signing-keys-and-revocation.md) | Signing keys, and taking a stolen one back | Accepted |
| [0011](0011-measuring-peers.md) | Measuring peers | Accepted |
| [0012](0012-sharing-prefetched-copies.md) | Sharing prefetched copies is asked at setup | Accepted |
| [0013](0013-trusted-followers.md) | Trusted followers, asked for and allowed on the phone | Accepted |
| [0014](0014-finding-people-and-private-messages.md) | Finding people and content, with messages between nodes over Tor | Accepted; how directories are used and step 4 changed by 0015, and who may connect directly by 0018 once built |
| [0015](0015-nodes-meet-first.md) | Nodes meet first: machine names, and what they may say | Accepted; detail added and limits changed by 0016 to 0018; `Known` changed by 0022 |
| [0016](0016-machine-acquaintances.md) | Machine acquaintances, reciprocity and referrals | Accepted 2026-11-08; stages A, A2 and A3 built, A2's gate missed and merged all the same, A3's missed and merged with introductions off by default; introductions and references replaced by 0022; where a machine introduced counts, and who may introduce, changed by 0021 |
| [0017](0017-one-hop-forwarding.md) | Two hops for pairs without a direct link | Accepted 2026-11-08; not built |
| [0018](0018-earned-direct-connections.md) | Earned direct connections | Accepted 2026-11-08; not built |
| [0019](0019-taking-posts-and-media-down.md) | Taking posts and media down | Accepted |
| [0020](0020-sharing-media-you-open.md) | Sharing media you open, with peers that worked for you | Accepted 2026-11-08; not built |
| [0021](0021-introduced-machines-count-among-those-who-greet.md) | A machine introduced counts among those who greet | Accepted 2026-11-08; built with 0016's stage A3, whose gate was missed; introductions off by default; replaced by 0022 |
| [0022](0022-lists-and-introductions-are-earned.md) | Lists and introductions are earned | Accepted 2026-11-08; changes 0015's `Known`, makes 0016's introductions and references one feature, replaces 0021; built after carrying |
| [0023](0023-followers-keep-working-together.md) | Followers keep working together | Proposed 2026-11-08; adds to 0018's layer 1 and 0017's middle turns; measured first in 0017's stage A model |

Proposals under review that may become ADRs: [peer reciprocity and recognition](../12-peer-reciprocity-and-recognition.md) (its first phase, measuring, is ADR 0011) and [hosting roles and the follower swarm](../13-hosting-roles-and-follower-swarm.md) (its first phase is accepted, and whether to share copies is asked at setup, ADR 0012). See the [critical review](../reviews/2026-10-21-review-of-docs-12-and-13.md) for what blocks them.

A record marked *Accepted* can still be superseded. Write a new ADR that links back to it rather than rewriting history.

A [review of ADRs 0016 to 0019](../reviews/2026-11-04-review-of-adrs-0016-to-0019.md) checked their drafts against the code and outside sources, and they were revised after it. It changes no ADR's status.
