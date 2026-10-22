# Relay-only mode: turning it on and checking it

In relay-only mode the node connects only through its relays, so other nodes, and your own phone, never learn its IP address. The relay operator still sees it, along with who the node talks to and when. Everything, video included, passes through the relay, which is slower. The decision is in [ADR 0007](../decisions/0007-transport-privacy-defaults.md). The automated tests are in [`relay_only.rs`](../../crates/osp-node/tests/relay_only.rs), against real relays on loopback.

## Turn it on

In the node's `config.toml`:

```toml
[network]
relay = "default"        # or your own relays; relay-only needs some
relay_only = true        # don't also set `bind`
```

Restart the node (`systemctl --user restart osp`). Then `osp status` shows:

- `Bound:` with nothing after it: the node has no IP sockets;
- `Relay:` with the node's home relay;
- `NAT: no UDP; every connection goes through the relay`.

To turn it off, remove the line (or set it to `false`) and restart.

## Check that nothing connects directly

No root needed:

- On the node: `ss -tunap | grep '"osp"'` lists only TCP connections on port 443, to the relay and the DNS publishing service (`dns.iroh.link` with n0's defaults). There are no UDP sockets.
- On another node that follows it: `osp status` shows the relay-only node as `relay`.
- In the app: the status screen (tap the dot) says the connection goes through a relay. Play a video to see how it feels.

## With a packet capture

This needs `tcpdump` and root. Start the capture before switching the node, so its startup is included. Leave out SSH if you're using it:

```sh
sudo tcpdump -i any -n -s 128 -Z "$USER" -w relay-only.pcap 'not port 22'
```

`-s 128` keeps only packet headers. Switch the node, use it from another node and a phone for a few minutes, then stop the capture with Ctrl-C. Reading the file needs no root. Replace `NODE`, `PEER` and the switch time with your own:

```sh
# Anything the node's machine sent straight to a peer after the switch: expect nothing.
tcpdump -r relay-only.pcap -n -tttt 'src host NODE and dst host PEER and not arp' | awk '$2 > "HH:MM:SS"'
# iroh's address discovery with relays (port 7842): expect none after the switch.
tcpdump -r relay-only.pcap -n -tttt 'udp port 7842' | awk '$2 > "HH:MM:SS"'
# All UDP the machine sent after the switch, by destination: expect only other software (VPN, DNS, mDNS).
tcpdump -r relay-only.pcap -n -tttt udp | awk '$2 > "HH:MM:SS" && $4 == "Out" {print $3, $8}' | sort | uniq -c | sort -rn
```

Packets *to* the node from a phone paired while the node was in direct mode are expected: the phone still tries the local address from its pairing code. Nothing answers them. Pairing the phone again drops that address.

## Results to record

Results here are public, so describe networks in general terms. Keep details of your own machines and network in `notes/`.

| Date | Node network | Peers | Path seen by peers | Capture | Notes |
| --- | --- | --- | --- | --- | --- |
| 2026-10-22 | home network, behind a VPN, node as a user service, n0 relays | another node and two Android phones on the same home network | relay, about 110 ms idle, up to 600 ms while a video streamed | after the switch: nothing sent directly to the peers, no UDP from the node, only TCP to its relay and DNS publishing | video played and seeked through the relay. Both phones still sent a few packets to the node's old local address; nothing answered. Switched back to direct afterwards: peers direct again at 3–5 ms |

## Known limitations

- The relay operator sees the node's address and who it talks to. With n0's default relays, that's n0, which also runs the DNS publishing service.
- A peer whose relay isn't in this node's config can't be reached at all in relay-only mode. The error names the relay.
- Through n0's public relays each node gets about 1 MB/s, and other requests slow down while a video streams (see the [Stage 1 guide](stage1-manual.md)).
- In direct mode, a relay from someone else's address must be public `https`. A host name that resolves to a private address isn't caught.
- The phone app has no relay-only setting of its own: when the node is relay-only, the phone reaches it through the relay too, even at home.
