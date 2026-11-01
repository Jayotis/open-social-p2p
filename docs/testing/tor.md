# Tor: turning it on and checking it

Messages between nodes are moving onto Tor's onion services, so that nodes that find each other never learn each other's address ([ADR 0014](../decisions/0014-finding-people-and-private-messages.md)). This is its first step: with Tor on, the node starts and watches its own Tor, and answers other nodes' requests through it as well as directly. What it sends still goes as before, except `osp tor ping`. The transport is in [`osp-tor`](../../crates/osp-tor/); the node's tests are in [`tor.rs`](../../crates/osp-node/tests/tor.rs).

Tor hides addresses; it doesn't make anyone anonymous.

## Turn it on

1. Install Tor: `sudo apt install tor` on Debian or Ubuntu. The node runs its own Tor from that program, with its settings and data in the `tor` folder of the node's home. It doesn't use or change the system's Tor service, which you can turn off if nothing else needs it: `sudo systemctl disable --now tor`.
2. In the node's `config.toml`:

   ```toml
   [tor]
   enabled = true
   # binary = "/usr/bin/tor"   # if it isn't on the PATH
   ```

3. Restart the node (`systemctl --user restart osp`). Then `osp status` shows a line like `Tor: ready, onion service published, 0 streams`. Connecting to the Tor network takes about 10 seconds the first time, and publishing the node's onion service a few more.

If Tor stops, the node starts it again by itself, after a pause that grows if it keeps stopping, and `osp status` says why it stopped until then.

## Reach another node

The other node needs Tor on too. Its node ID is enough, as the onion address follows from it:

```sh
osp tor ping NODE_ID
```

The first time, it says how long connecting took: 10 to 25 seconds, now and then a minute. After that, it reuses the connection, as the node does, and a request takes about a second.

## Check it recovers

With a ping every few seconds from one node to the other, stop either node's Tor, as a crash would. Find it by its settings file, so the system's Tor, if it runs, is left alone:

```sh
pkill -f "$HOME/.local/share/osp/tor/torrc"
```

The node starts Tor again within seconds (`osp status` says `restarted once`), and pings get through again within about 20 seconds with nothing done by hand. A node whose own Tor restarted is answered as soon as it reconnects; one reaching a node whose Tor restarted waits until that node's onion service is published again.

## Check nothing goes direct

No root needed:

- `ss -tnp | grep '"tor"'` on either node: the node's Tor connects only to Tor relays (usually port 443 or 9001), never to the other node's address.
- `ss -tunap | grep '"osp"'`: connections over Tor are only to `127.0.0.1`, the node's own Tor. The node's other connections, direct or through a relay, go on as before until messages move onto Tor.
