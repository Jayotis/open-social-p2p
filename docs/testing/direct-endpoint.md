# The media and direct endpoints: turning them on and checking them

A node gets two endpoints beside its main one ([ADR 0014](../decisions/0014-finding-people-and-private-messages.md), build step 2). Each has a key of its own, and a signed binding ties it to the node's public ID.

- **Media:** relays only, no IP sockets. It serves media to anyone, and giving out its ID reveals only which relay it uses.
- **Direct:** IP and relays. It serves media and the phones' protocol, to the node's own phones and to peers its rule allows, and refuses everyone else before they can learn an address.

Neither carries messages between nodes. With them on, media and phones go only through them, and the node's main endpoint carries only messages. **Its address is hidden only with [Tor](tor.md) on too:** without it, that main endpoint still connects directly. The tests are in [`endpoints.rs`](../../crates/osp-node/tests/endpoints.rs) and [`on_endpoints.rs`](../../crates/osp-node/tests/on_endpoints.rs).

## Turn them on

In the node's `config.toml`:

```toml
[endpoints]
enabled = true
direct = "nobody"   # or "anyone", "follows" (people you follow), "approved"
```

Restart the node (`systemctl --user restart osp`). `osp status` then shows a line like `Endpoints: direct 1a2b3c4d5e (allows nobody), media 6f7a8b9c0d`.

- A connection is direct only when both sides' rules allow it. Your own phones always may.
- Under `approved`, `osp direct allow <node id>` adds a node by its full public node ID, `osp direct list` shows them, and `osp direct remove <node id>` takes one off and closes any connection with it. What it learned of your address can't be taken back.
- Phones pair with the direct endpoint, which through a relay takes an app that offers the pass described below. **Phones paired before must pair again.**
- A node fetches media from another's direct endpoint, from its own, only if its own rule allows that node and that node gave it the endpoint. Otherwise it fetches from its media endpoint, so a node that allows nobody shows its address to nobody, even a node that allows anyone.
- A follower is named to other nodes as a host by its media endpoint, or by its direct endpoint if its rule is `anyone`. A direct endpoint given to one node because the rule allows that node is never passed on.

## Why the refusal happens on the first packet

The direct endpoint has to refuse a stranger who somehow has its ID, and do so before the stranger learns an address. Measured on 2026-11-01 with iroh 1.4.1: a node with four IP addresses, a client that only knew its ID and a relay on loopback, 30 tries each way. A longer run of the first, 100 tries, gave 392 addresses of a possible 400.

| How the node refuses | Handshake finished | Addresses the client was told |
| --- | --- | --- |
| iroh's `after_handshake` hook | 30 of 30 | 120 |
| In the protocol handler | 30 of 30 | 116 |
| A filter on the first packet, by the sender the relay names | 0 of 30 | 0 |

iroh sends a node's addresses in `ADD_ADDRESS` frames as soon as it registers a connection, which it does before the hook runs. So a refusal at that point always loses the race, or nearly always. Only a refusal before the handshake is safe, and by then the node hasn't seen the client's key.

What it has seen is the relay's word. A packet through a relay comes with the sender's ID, which the relay checked when that sender connected to it. Only the relay's operator could forge it, and the operator sees the node's address anyway when the node connects to the relay.

**Two cases go further than the sender's ID.**

- **A phone that is pairing** isn't known to the node yet. It offers a pass among the ALPNs in its first packet: the hash of the pairing ticket's secret. A node that doesn't expect it ignores it.
- **Someone who comes straight to an IP address** names nobody, and its key isn't known until the handshake. It already has that address, so it goes on to the handshake and is refused there. **By then it may have been told the node's other addresses**: in the test, all four, each time. It gets nothing else. iroh gives no way of preventing this yet; refusing every connection from an IP address would also refuse allowed peers and phones that already know the way. So someone who knows one of a node's addresses and its port can learn its others.

## What the tests check

The tests watch what iroh's client side is told, as the node's side can't show it: each counts the addresses a node registered for an endpoint. A node let through is told, so the count is known to work.

- A stranger holding the direct ID is refused and told nothing, with or without a ticket waiting.
- A phone pairs through a relay with its ticket's pass. A made-up or used pass opens nothing. Once paired it needs none; once unpaired it's refused.
- Nodes that allow each other connect, and media is served. Either side's rule alone isn't enough, whichever side refuses.
- Each rule hands the direct endpoint to whom it says.
- A binding for another node, of another kind, with another endpoint's address, or dated far ahead is refused, whether offered or in a reply.
- The media endpoint serves a stranger and tells it no address.
- Neither endpoint takes messages between nodes, and the media endpoint doesn't take phones.

`on_public_relays`, an ignored test, does the main ones on n0's relays:

```sh
cargo test -p osp-node --test endpoints on_public_relays -- --ignored --nocapture
```

In its one run so far, a stranger was refused in about a second and told nothing, and a paired phone was told six addresses.
