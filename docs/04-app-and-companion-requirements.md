# Application and Desktop Companion Requirements

**Status:** Product requirements draft, not a UI or implementation specification. The connection model below follows [ADR 0001](decisions/0001-home-node-architecture.md) and [ADR 0006](decisions/0006-desktop-first-always-on-node.md).

## Connection model

- The phone talks **only to its own home node**. The home node does the social and peer-to-peer networking, gathers content and serves it to the phone already transcoded.
- Pairing: the home node shows a QR code; the phone scans it once and is remembered as one of the person's devices. Devices can be revoked from the node.
- The connection works on the home network and away from home, without port forwarding (hole punching, or a relay as fallback).
- When the home node is offline the app shows cached content and says so plainly.
- A personal always-on node is required ([ADR 0006](decisions/0006-desktop-first-always-on-node.md)). The app shows prominently whether the home node is reachable.

## Flutter mobile application

### Onboarding

- Joining happens on the home node: installing it creates the identity locally. No central sign-up, password database or server selection.
- Guided identity backup and recovery, clear terms, and age/safety choices appropriate to the network.
- Friendly display names in the interface; full identity keys available when needed.
- The phone joins by pairing with the node (QR code), not by creating a separate account.

### Social experience

- Profiles, follows/connections, chronological feed and configurable discovery.
- Posts, replies, reshares, reactions, media attachments, notifications and reporting.
- Community/group features with explicit audience membership and moderation rights.
- Creator/video profiles and subscriptions, with smooth playback on typical mobile connections.
- Account export/migration capabilities and consistent identity display across services.
- Clear distinction between private, group-limited, unlisted and public content.

### Peer-hosting experience

- Show when the user's desktop node is paired, online, offline or storage-limited.
- Optional 'help host what I share' preference, never a precondition to resharing.
- Avoid silent large mobile downloads and excessive cellular data usage.
- A 'where this media comes from' transparency view without exposing sensitive home network details.
- Graceful placeholders for temporarily unavailable media.

## Home node (desktop companion) application

Target platforms: Linux first (the maintainer's test platform), then Windows and macOS. Implemented in Rust ([ADR 0002](decisions/0002-rust-and-iroh.md)); packaging is to be decided.

### Setup

- Guided installation and pairing with an existing account.
- Choose approved media folders; no arbitrary filesystem exposure.
- Work behind typical home routers without requiring manual port forwarding when feasible.
- Clear local controls, service start/stop and uninstall/data-removal pathways.

### Storage and publishing

- Import approved files without altering originals without permission.
- Local previews, thumbnails, formats and transcoding (as appropriate to hardware).
- Integrity checks, content-addressed references and recoverable metadata.
- Explicit publish and audience controls.
- Backup/export guidance: peer replication is **not** a guaranteed personal backup.

### Hosting and bandwidth

- Enable/disable voluntary hosting of eligible reshared content.
- Disk quota, upload-rate limit, allowed network types and schedule.
- Automatic eviction policy for cached replicas, preserving user originals.
- Active transfer monitor showing bytes sent, connected peers where privacy permits, and resource usage.
- Permissioned replication for private assets only after a separately reviewed security design.

### Reliability

- Resume transfers, recover after restarts, validate data and detect corrupt storage.
- Maintain accurate availability status while offline.
- Provide user-friendly diagnostics and logs with minimized sensitive metadata.
- Support energy-efficient operation and resource constraints.

## Accessibility and safety

Keyboard and screen-reader support, caption/subtitle support, media descriptions, reduced-motion options, reporting/blocking, meaningful consent and clear defaults are requirements for both apps.

## Definition of a usable early version

An ordinary person can install a home node, pair their phone, create a public media post from the phone, reshare someone else's post without any technical configuration, optionally help replicate it, and see it remain retrievable when its origin goes offline.
