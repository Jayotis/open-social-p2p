# Tor: turning it on and checking it

Messages between nodes are moving onto Tor's onion services, so that nodes that find each other never learn each other's address ([ADR 0014](../decisions/0014-finding-people-and-private-messages.md)). This is its first step: with Tor on, the node starts and watches its own Tor, and answers other nodes' requests through it as well as directly. What it sends still goes as before, except `osp tor ping`. The transport is in [`osp-tor`](../../crates/osp-tor/); the node's tests are in [`tor.rs`](../../crates/osp-node/tests/tor.rs).

Tor hides addresses; it doesn't make anyone anonymous.

## Turn it on

1. Install Tor: `sudo apt install tor` on Debian or Ubuntu. The node runs its own Tor from that program, with its settings and data in the `tor` folder of the node's home. It doesn't use or change the system's Tor service, which you can turn off if nothing else needs it: `sudo systemctl disable --now tor`.
2. In the node's `config.toml`. Tor takes the [media and direct endpoints](direct-endpoint.md) too, as media and phones can't go over it, and the node won't start without them:

   ```toml
   [tor]
   enabled = true
   # binary = "/usr/bin/tor"   # if it isn't on the PATH

   [endpoints]
   enabled = true
   direct = "nobody"
   ```

   **This changes what the node can reach.** Messages then go only through Tor, so it can reach only nodes with Tor on, and they can reach it only that way. Its follow code gives its ID and nothing else. **Phones paired before must pair again**, as they now connect to the direct endpoint.

3. Restart the node (`systemctl --user restart osp`). Then `osp status` shows a line like `Tor: ready, onion service published, 0 streams`. Connecting to the Tor network takes about 10 seconds the first time, and publishing the node's onion service a few more.

If Tor stops, the node starts it again by itself, after a pause that grows if it keeps stopping, and `osp status` says why it stopped until then.

## Reach another node

The other node needs Tor on too. Its node ID is enough, as the onion address follows from it:

```sh
osp tor ping NODE_ID
```

The first time, it says how long connecting took: usually 10 to 25 seconds, but [longer at a node that has just started](#a-first-contact-can-take-minutes). After that, it reuses the connection, as the node does, and a request takes about a second.

## Check it recovers

With a ping every few seconds from one node to the other, stop either node's Tor, as a crash would. Find it by its settings file, so the system's Tor, if it runs, is left alone:

```sh
pkill -f "$HOME/.local/share/osp/tor/torrc"
```

The node starts Tor again within seconds (`osp status` says `restarted once`), and pings get through again within about 20 seconds with nothing done by hand. A node whose own Tor restarted is answered as soon as it reconnects; one reaching a node whose Tor restarted waits until that node's onion service is published again.

## Check nothing goes direct

No root needed:

- `ss -tnp | grep '"tor"'` on either node: the node's Tor connects only to Tor relays (usually port 443 or 9001), never to the other node's address.
- `ss -tunap | grep '"osp"'`: connections over Tor are only to `127.0.0.1`, the node's own Tor. The node's other connections are its media and direct endpoints': to relays, to its own phones, and to peers both rules allow.

## Follow and fetch, all of it

An ignored test runs two nodes as they're meant to run, with messages over the Tor network and media through n0's public relays:

```sh
OSP_TOR=/usr/bin/tor cargo test -p osp-node --test tor following_and_fetching -- --ignored --nocapture
```

In its one run so far, on 2026-11-01, it took 17 s from following to holding a photo, with both nodes on the same machine: connections were over Tor for messages and through a relay for media, and none direct.

## A first contact can take minutes

Usually a first connection takes 8 to 25 seconds. **A node dialling another whose onion service was published seconds earlier sometimes waits minutes.** Six runs on 2026-11-01 with a new key each time took 8.4, 9.8, 14.5, 22.2, 112.5 and 112.6 s. Three runs reusing a key published minutes before took 8.9, 199 and 215 s.

Tor gives no error during the wait: its answer to the SOCKS request just doesn't come. Dropping the request and making a new one every 30 s made no difference in those runs, so the node doesn't do that.

It rarely matters. A node dials the nodes it follows as soon as its Tor is ready, and they've been up for a while, so they answer in seconds; a connection either node dialled carries messages both ways. It takes a stranger dialling a node that has just started. Nothing is lost meanwhile: the node keeps trying in the background.
