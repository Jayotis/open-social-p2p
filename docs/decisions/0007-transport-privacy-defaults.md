# ADR 0007: Transport privacy defaults, private messages and direct media

**Status:** Accepted (2026-10-21). Implementation is phased; relay-only behaviour must be validated (see below).

## Context

- A direct peer-to-peer connection lets each side see the other's IP address ([doc 05](../05-security-privacy-and-governance.md)).
- A relay hides each peer's address from the other, but the relay operator sees both, and every relayed byte costs the operator bandwidth.
- Tor onion services hide addresses from everyone, but add latency, and the Tor Project discourages bulk file sharing over its volunteer-run network.
- Peer-to-peer video only saves home and relay bandwidth when peers connect directly. Routing video through relays moves the cost to whoever runs the relays.

iroh connects directly where it can and falls back to relays ([ADR 0002](0002-rust-and-iroh.md)). It also has an experimental Tor transport (`iroh-tor-transport`), which needs a separately running Tor daemon.

## Options considered

1. **Direct for everything.** Fastest and cheapest. Every peer you exchange anything with sees your IP.
2. **Private for everything.** All traffic through relays or Tor. Strongest privacy, but slow video and large relay costs.
3. **Split:** messages private by default, media direct by default.

## Decision

Option 3.

**Control plane (private by default).** This covers the social protocol (`osp/social/0`), address lookup and discovery. It uses relay-only connections, so nodes you only exchange messages with never learn your IP address. Tor is to be evaluated for this plane later.

**Media plane (direct by default).** This covers content transfer (iroh-blobs). It uses direct connections where possible. Setup explains this in plain words: *people your node exchanges media with can see your internet address.* A **private media** option switches media to relay-only too, at the cost of slower video and more relay bandwidth.

**Phone to its own home node.** Direct paths are allowed on both planes, because both ends belong to the same person. The phone's address is seen only by its own node (and a relay, when one is used).

**VPNs.** Running the node behind a VPN stays supported and hides the home address on both planes. The maintainer's desktop runs this way.

## Consequences

- **The default still reveals your IP to everyone your node exchanges media with directly.** That includes any stranger who downloads a post from your node, and every follower in the swarm your node trades copies with ([doc 13](../13-hosting-roles-and-follower-swarm.md)). Someone who gets your post from another follower's node learns that node's address, not yours. This is the same exposure as BitTorrent. The private control plane mainly protects two groups: people who turn on private media, and anyone who only exchanges messages with you. People must be told this plainly; it must not be implied that the default hides their address.
- **Relay operators see who talks to whom** (both addresses, not content). During development these are n0's public relays; project or community relays, and self-hosting instructions, are needed before a public release.
- **Relay cost grows with private-media use.** Relays may need rate limits for media, which would make private media slower still.

## Privacy modes to evaluate

| Mode | Hides your address from peers | Who still learns it | Cost |
| --- | --- | --- | --- |
| **Direct** | No | Every peer you exchange data with | Fastest |
| **Relay-only** | Yes | The relay operator, along with who you talk to and when | Relay bandwidth, some latency |
| **Onion-routed** (iroh's experimental Tor transport) | Yes, including from the relay | Much harder for anyone; traffic correlation remains possible | Latency and throughput costs; volunteer Tor capacity |

None of these modes is anonymous in the strong sense, and none may be described that way.

**Requirements for any private mode:**

- **Fail closed.** If a private connection can't be made, the node reports an error. It never silently falls back to a direct connection.
- **Verify the whole application, not one setting.** Addresses can leak through discovery records, `WhoHosts` replies, logs and earlier connections, not only through the transport. The validation test must inspect all of them.

## Implementation phases

1. **Node-wide private mode:** relay-only for everything. iroh 1.4's endpoint builder can drop its IP transports (`clear_ip_transports()`), which should leave only relay paths. To validate: confirm with a packet capture that peers never learn the node's address.
2. **Separate planes:** either two endpoints (a relay-only control endpoint with its own key, and a direct media endpoint, both listed in the signed profile), or per-connection path restrictions if iroh supports them. To evaluate.
3. **Tor for the control plane:** evaluate `iroh-tor-transport` once it and iroh's custom-transport API are stable.

## Validation and revisit conditions

- Nodes on different networks run in private mode while traffic, discovery records, `WhoHosts` replies and relay behaviour are inspected. No peer learns a node's address through the application, and connections fail closed.
- Measure video start time and throughput, relayed against direct, on the maintainer's test machines (including the desktop behind ProtonVPN).
- Estimate relay bandwidth cost per active user under realistic private-media uptake.
- Revisit if relay-only video proves good enough to make private media the default, or if Tor integration matures.
