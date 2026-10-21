# Application and Desktop Companion Requirements

**Status:** Product requirements draft, not a UI or implementation specification.

## Flutter mobile application

### Onboarding

- One normal registration process; no mandatory selection of servers.
- Email verification, account recovery, clear terms and age/safety choices appropriate to the service.
- Friendly usernames in the interface; full federated account addresses available when needed.
- No desktop installation required to register, post text or participate in discussions.

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

## Desktop companion application

Target platforms: Linux, Windows and macOS. Implementation language and packaging are to be decided.

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

An ordinary person can join through Flutter, create a public media post via a paired home node, reshare someone else's post without any technical configuration, optionally help replicate it, and see it remain retrievable when its origin goes offline.
