# osp-tor

Messages between Open Social P2P nodes over Tor ([ADR 0014](../../docs/decisions/0014-finding-people-and-private-messages.md)): an [iroh](https://github.com/n0-computer/iroh) transport whose packets travel on streams between Tor onion services, so neither node learns the other's address, and the node's own Tor, which it starts, watches and starts again when it stops.

A node's onion address follows from its node ID, since both are ed25519 keys, so one node needs nothing but another's ID to reach it.

## Where it comes from

It started from n0's [`iroh-tor-transport`](https://github.com/n0-computer/iroh-tor) 0.1.0, with these changes, which we mean to offer back:

- **One stream per pair of nodes, used both ways.** The node that accepts a stream sends a challenge, and the node that dialled signs it with its key. The node that dialled already knows whom it reached, as Tor checks the onion address. So the other node answers on the same stream: a pair needs one circuit, not one each way; a first contact needs one onion service, not both; a node that only dials needs none; and no node is made to dial anyone by a packet that names them. This wire format isn't upstream's.
- **Packets to a node leave in order,** on its stream, instead of each from a task of its own.
- **The onion service is set up again whenever Tor restarts,** and the node starts its own Tor again if it stops.
- **A control-port client of its own** instead of `torut`, which brought two crates with security advisories. It authenticates with SAFECOOKIE, so a program pretending to be Tor learns nothing.
- **Limits** on streams and dials, and pauses between failed dials to a node.

## Tests

`cargo test -p osp-tor` runs without Tor: the transport over loopback, with its streams cut as a Tor restart cuts them, and the supervisor against a fake Tor control port.

With Tor installed, one more test runs two nodes on this machine, each with its own Tor, through the Tor network, and kills each Tor in turn:

```sh
OSP_TOR=/usr/bin/tor cargo test -p osp-tor --test real_tor -- --ignored --nocapture
```

## Licence

As the code it started from, under either of

- the Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)), or
- the MIT licence ([LICENSE-MIT](LICENSE-MIT)),

at your option. The original is copyright 2025 N0, INC. The rest of Open Social P2P is under the AGPL-3.0 ([LICENSE](../../LICENSE)).
