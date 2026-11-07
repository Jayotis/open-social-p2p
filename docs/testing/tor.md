# Tor: turning it on and checking it

Messages between nodes are moving onto Tor's onion services, so that nodes that find each other never learn each other's address ([ADR 0014](../decisions/0014-finding-people-and-private-messages.md)). With Tor on, the node starts and watches its own Tor, and messages between nodes go only through it, while media and phones use the node's other endpoints. The transport is in [`osp-tor`](../../crates/osp-tor/); the node's tests are in [`tor.rs`](../../crates/osp-node/tests/tor.rs).

Tor hides addresses; it doesn't make anyone anonymous.

## Turn it on

A node made by `osp init` has it on already, unless it was made with `--no-tor`; `osp init` says if Tor isn't installed. For a node made before:

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

   **This changes what the node can reach.** Messages then go only through Tor, so it can reach only nodes with Tor on, and they can reach it only that way. Its follow code gives its ID and nothing else. **Phones paired before must pair again**, as they now connect to the direct endpoint: in the app, Home node status, then Forget this home node, then `osp pair`.

3. Restart the node (`systemctl --user restart osp`). Then `osp status` shows a line like `Tor: ready, onion service published, 0 streams`. Connecting to the Tor network takes about 10 seconds the first time, and publishing the node's onion service a few more.

If Tor stops, the node starts it again by itself, after a pause that grows if it keeps stopping, and `osp status` says why it stopped until then.

## Reach another node

The other node needs Tor on too. Its node ID is enough, as the onion address follows from it:

```sh
osp tor ping NODE_ID
```

The first time, it says how long connecting took: usually 10 to 25 seconds, but [longer at a node that has just started](#a-first-contact-can-take-minutes). After that, it reuses the connection, as the node does, and a request takes about a second.

## The machine name

A node with Tor on also has a machine name ([ADR 0015](../decisions/0015-nodes-meet-first.md)): a second identity on Tor, tied to no person, under which it will get to know other nodes. So far it can only greet another machine when asked. To turn it off:

```toml
[machine]
enabled = false
```

`osp status` says only `Machine: on`, and doesn't name it, as phones get that and people paste it, and it holds the node's public ID too. Keep the two apart when asking for help.

```sh
osp machine show            # its name: three words, and its ID
osp machine hello MACHINE   # greet another machine, by its ID
osp machine new             # with the node stopped: forget it, and make a new one at the next start
```

A greeting says how long connecting and asking took, and then closes the connection. The first one takes as long as any first contact over Tor, and so does one to a machine whose Tor has just restarted, which [can take minutes](#a-first-contact-can-take-minutes).

To check it's kept apart, with `PUBLIC` another node's ID and `MACHINE` that node's machine ID:

```sh
osp machine hello PUBLIC    # refused: a public ID doesn't answer as a machine
osp tor ping MACHINE        # refused: a machine name answers nothing else
journalctl --user -u osp | grep "$(osp machine show | tail -1 | cut -c1-10)"   # expect nothing
```

It isn't anonymous. Both IDs share one Tor and come and go together, so someone reaching both can match them over time.

## Directories

A directory lists people who asked to be listed, so that a node with no follows has someone to find ([ADR 0015](../decisions/0015-nodes-meet-first.md)). It holds each one's signed profile and latest 5 posts, and the records of machines that greeted it. A node reads a directory in the background under its machine name and keeps who it finds, so a search looks through them at home. No word anyone types leaves the node, and they stay out of the feed.

**To run one,** on a node with Tor on:

```toml
[directory]
serve = true
```

`osp status` then shows a line like `Directory: 3 people, 12 machines`. Its two names are what others need: its machine ID, from `osp machine show`, and its node ID, from `osp status`. Giving them out together is safe for a directory, whose role is public, and for nobody else.

**To read one, and to be listed by it:**

```toml
summary = "Builds oak tables"   # at the top of the file, by your name

[directory]
list_me = true                  # only if you want to be listed

[[directory.known]]
machine = "..."
node = "..."
```

Restart the node. `osp init` asks about `list_me` for a new node (`--list-me` or `--no-list-me` answers it). It says in your signed profile that a directory may list it and your latest posts, and that others may pass them on. Your node then asks each directory, from your public ID and never your machine name, at a time of its own within the hour, and again each day. To be dropped, turn it off and restart: the profile saying no takes it back, by the time the directory next reads your node, within the hour.

```sh
osp directory read       # read them now, rather than waiting
osp search oak           # says "found through directory" and which, by its machine name's words
osp follow IDENTITY      # the identity the search showed
```

Following asks their own node first, and goes no further unless it speaks for them: a directory's copy can be out of date. Like any first contact, it [can take minutes](#a-first-contact-can-take-minutes). `osp following` shows if it failed.

**How fresh it is.** A node reads its directories within 10 minutes of starting and then about every 6 hours, taking what changed since. After an update that takes a new kind of entry, it reads each from the start once. A directory reads each listed node once an hour. So a new post can take hours to be found, which is fine for finding people: once followed, posts come directly.

**What a directory sees:** which machine name read its lists, and when, and which public ID asked to be listed. It sees no search words, and nothing ties the two names together except what any machine and public ID on one Tor can show ([ADR 0015](../decisions/0015-nodes-meet-first.md#consequences)).

**Not yet:** the media of a post found through a directory opens only once you follow its author.

The whole of it over real Tor, which takes minutes: `OSP_TOR=/usr/bin/tor cargo test -p osp-node --test directory -- --ignored --nocapture`.

## Machines meeting

A node's machine name gets to know other machines on its own ([ADR 0015](../decisions/0015-nodes-meet-first.md), step 3). It starts with the machines on its directories' maps, greets a few new ones an hour, asks each one it meets for some of the machines it has met lately (`Known`), and checks in with each about once a day, at times of its own. Nothing its owner follows, posts or searches for changes what it says, and its record of the machines never leaves the node.

```sh
osp machine show    # its name, then "It knows 0 machines, has met 2 more and heard of 5."
```

A machine is **heard of** when a signed record of it arrives, **met** once it answers, and **known** after a month of answering with the records it passed on checking out. Being known grants nothing: known machines are checked in with first, and keep their places when the table is full.

**Its limits,** from ADR 0015 and the three rules ADR 0016 adds:

- At most 256 machines. Half the places go to those known longest, so a crowd of new names can't push them out.
- Records from one source fill at most a quarter of the rest, 32 places. A machine met still counts against whoever passed on its record, and all those that greet unasked count as one source, as names are free.
- 6 new machines dialled an hour, 2 of them from the sources that supplied the fewest of the table's machines.
- The directories in `config.toml` are never in the table, so a full one can't push them out. When none of the machines met answers and none is left to try, the node reads its directories again, no sooner than 10 minutes after the last time. Each time such a read brings nothing new, it waits twice as long, up to the usual 6 hours, so a node alone doesn't ask every 10 minutes for good.
- Only a machine never met gives up its place to a newcomer. One heard of that doesn't answer three dials, 6 hours apart, is forgotten, and so is one met that hasn't answered for 30 days.

`osp machine new` forgets them all with the name: the node starts again among machines as a stranger.

**What it costs:** each conversation over Tor builds about three circuits, two at the machine asking and one at the machine asked. [The simulator](meeting.md#results) estimates about 400 a day for each of 100 nodes; a count over real Tor is to come.

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

For more, a packet capture needs `tcpdump` and root. Start it before the node switches, so its startup is included, and leave out SSH if you're using it, and loopback, which is only the node talking to its own Tor. `-s 128` keeps only the headers:

```sh
sudo tcpdump -i any -n -s 128 -Z "$USER" -w tor.pcap 'not port 22 and not net 127.0.0.0/8 and not host ::1'
```

Use the nodes for a while, posting from one and fetching on another, then stop it with Ctrl-C. Replace `PEER` with another node's address:

```sh
# Anything between this machine and the other node: expect nothing once both have switched.
tcpdump -r tor.pcap -n -tttt 'host PEER and not arp'
# Everything on the node's own UDP ports, which `osp status` lists as Bound: expect relays' port 7842 and your phones.
tcpdump -r tor.pcap -n -tttt 'udp port PORT'
```

The capture has everything else the machine did too, so delete it afterwards.

We did this once, on 2026-11-01, on one of three test nodes on the same home network, for 11 minutes, while a post with a photo and a video reached the other two and two phones paired and played the video. The machine exchanged direct packets with both nodes before the switch. After it, there were only two from a node that hadn't switched yet, which were refused. The node's UDP went to relays and its phones, and from the moment each phone connected, a few packets went to other addresses, which by their timing are ones the phone offered. Only that one node was captured: the other two were checked with `ss`, which can miss short connections.

## Follow and fetch, all of it

An ignored test runs two nodes as they're meant to run, with messages over the Tor network and media through n0's public relays:

```sh
OSP_TOR=/usr/bin/tor cargo test -p osp-node --test tor following_and_fetching -- --ignored --nocapture
```

In its one run so far, on 2026-11-01, it took 17 s from following to holding a photo, with both nodes on the same machine: connections were over Tor for messages and through a relay for media, and none direct.

**Public relays may throttle media.** On the test nodes that day, sending a 3.7 MB video through n0's public relay made both ends log `The relay is rate-limiting this endpoint`, though it arrived. `osp-sim` has no relays, so its results don't show this: it's what ADR 0014's measurement of media's default is for.

## A first contact can take minutes

Usually a first connection takes 8 to 25 seconds. **A node dialling another whose onion service was published seconds earlier sometimes waits minutes.** Six runs on 2026-11-01 with a new key each time took 8.4, 9.8, 14.5, 22.2, 112.5 and 112.6 s. Three runs reusing a key published minutes before took 8.9, 199 and 215 s.

Tor gives no error during the wait: its answer to the SOCKS request just doesn't come. Dropping the request and making a new one every 30 s made no difference in those runs, so the node doesn't do that.

It rarely matters. A node dials the nodes it follows as soon as its Tor is ready, and they've been up for a while, so they answer in seconds; a connection either node dialled carries messages both ways. It takes a stranger dialling a node that has just started. Nothing is lost meanwhile: the node keeps trying in the background.
