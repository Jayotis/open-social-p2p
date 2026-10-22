# Stage 1 manual testing

How to run the prototype home node by hand, on one machine or across several, and what to record. The automated version of the main scenario is `cargo test` (see [`three_nodes.rs`](../../crates/osp-node/tests/three_nodes.rs)).

## Build

Requirements: Rust (stable) and ffmpeg (`sudo apt install ffmpeg`). ffmpeg is only needed on nodes that post media.

```sh
cargo build --release      # produces target/release/osp
```

A binary built on a newer distribution may not run on an older one, because glibc versions differ. Either build on each machine, or check the newest glibc the binary needs and copy it to machines that have at least that version (`ldd --version`):

```sh
objdump -T target/release/osp | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1
```

Copying is the easy route to a machine with no compiler.

## One machine, scripted

```sh
scripts/dev-three-nodes.sh                 # loopback only, works offline
scripts/dev-three-nodes.sh --internet      # through n0's public relays
scripts/dev-three-nodes.sh my-video.mp4    # post your own clip
```

The script starts three daemons (alice, bob, carol), runs the publish → reshare-and-host → origin-offline → fetch scenario, and prints what Carol received (resolution, bitrate, leftover metadata). Node homes and logs stay in `.osp-dev/`.

## Running a node by hand

```sh
osp init --name "Your name"        # creates ~/.local/share/osp
osp daemon                         # leave running, or `osp service install` (see the M2 guide)

# in another terminal
osp invite                         # ticket + QR code for others to follow you
osp follow <ticket>
osp post "Hello" --attach clip.mp4 # transcoded to 720p, metadata stripped
osp feed
osp reshare <post-id> --host       # reshare and keep/serve a copy
osp fetch <post-id> --out ~/Downloads
osp status                         # relay, NAT, connections (direct/relay), storage
```

Use `--home DIR` (or `OSP_HOME=DIR`) to run several nodes on one machine. Add `--json` to any command for machine-readable output. Logging detail: `RUST_LOG=osp_node=debug osp daemon`.

## Cross-machine scenario

| Role | Network |
| --- | --- |
| A, author (later paired with the phone) | Behind a VPN |
| B, resharer and host | Home network |
| C, viewer | Home network |

1. On each machine: `osp init --name A|B|C`, then start the daemon.
2. B: `osp follow <A's ticket>`. C: `osp follow <B's ticket>`.
3. A: `osp post "cross-machine test" --attach clip.mp4`.
4. B: `osp sync && osp feed`, then `osp reshare <id> --host`.
5. Stop A's daemon (Ctrl-C).
6. C: `osp sync && osp feed`, then `osp fetch <id> --out /tmp/osp-test`.
7. On B and C: `osp status`. Note whether each peer shows `direct` or `relay`, and the `NAT:` line. A public address that varies by destination usually means other nodes connect through the relay.

To time a transfer, run `follow` and `sync` together, e.g. `time (osp follow <ticket> && osp sync)`. `sync` waits until prefetched media has finished downloading, so `osp fetch` afterwards only copies the file out.

The RTT in `status` is QUIC's smoothed estimate. It climbs while a transfer runs and takes a minute or two to settle afterwards, so read it before or well after a big download. Through n0's public relays each uploading node gets about 1 MB/s (October 2026: two uploads from different nodes at once each held that rate, so the limit is per node, not the uplink). A queue builds behind that limit and adds close to a second of delay to everything else on the connection until the transfer ends. Direct connections bypass it, and a self-hosted relay can lift it.

When all three machines share a home network, the connections between them go direct over the LAN even with a VPN on, as long as the VPN allows local traffic. To test the path a remote peer would get:

- **Remote peer behind the VPN, no hardware changes:** on A, add `bind = "<VPN address>:0"` under `[network]` in `config.toml` and restart the daemon, using the VPN interface's IPv4 address from `ip -brief addr`. A then sends everything through the tunnel, and its LAN address is never offered. Follow A from fresh nodes on B and C (`--home ~/osp-vpntest`) so the media is downloaded again. Remove the `bind` line afterwards.
- Repeat that with the VPN's less strict NAT mode, if it has one, and see whether A's connections become direct.
- Run C on mobile data via a hotspot to test a fully remote path from a carrier network.

## Running a test node on a shared machine

If the machine already runs other services, run the node as its own user with resource limits. It doesn't need inbound ports:

```sh
sudo useradd --system --home-dir /var/lib/osp --create-home --shell /usr/sbin/nologin osp
sudo install -m 755 target/release/osp /usr/local/bin/osp
sudo -u osp OSP_HOME=/var/lib/osp osp init --name "C"
sudo tee /etc/systemd/system/osp-node.service >/dev/null <<'EOF'
[Unit]
Description=Open Social P2P home node (test)
After=network-online.target
Wants=network-online.target

[Service]
User=osp
Environment=OSP_HOME=/var/lib/osp
ExecStart=/usr/local/bin/osp daemon
Restart=on-failure
CPUQuota=25%
MemoryMax=512M
Nice=10
IOSchedulingClass=idle
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/osp
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
EOF
sudo systemctl daemon-reload
sudo systemctl enable --now osp-node
sudo -u osp OSP_HOME=/var/lib/osp osp status
```

Notes:

- Upload rate limits arrive in M3. Until then keep a shared machine in the viewer role (C). A viewer stores media in its `cache` class, which it serves only to its own paired devices, so it uploads nothing to strangers.
- Like most peer-to-peer software, iroh may ask the router for a port mapping (UPnP / NAT-PMP) if the router offers it. Nothing breaks if it doesn't.
- To remove everything: `sudo systemctl disable --now osp-node`, delete the unit file, `sudo userdel -r osp`.
- For a quick test without sudo, run it as your own user at low priority instead: `tmux new -d -s osp 'nice -n 10 ionice -c3 osp daemon'`.

## Results to record

Add a row per run. Results here are public, so describe networks in general terms. Keep details of your own machines and network in `notes/`, which git ignores.

| Date | A network | B network | C network | A↔B path | B↔C path | Time for C to fetch | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 2026-10-21 | one machine, loopback | same | same | direct | direct | < 1 s | `dev-three-nodes.sh` |
| 2026-10-21 | one machine, n0 relays, behind a VPN | same | same | direct | direct | < 1 s | `dev-three-nodes.sh --internet`; tickets carried only the relay URL |

## Known limitations of the M1 prototype

- Public posts only; no private or group content.
- Following is manual (tickets or QR codes); no directory or search.
- No storage quotas, eviction or upload rate limits yet (M3).
- Media is unavailable when the author's node and every host are offline.
- Development uses n0's public relays and DNS lookup ([ADR 0002](../decisions/0002-rust-and-iroh.md)).
- Not yet tested: HDR phone video, rotated photos, very long uploads.
