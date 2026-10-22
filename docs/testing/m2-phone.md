# M2 manual testing: phone and always-on node

How to run a home node as a service, pair a phone with it and check that the phone works at home and away. The decisions behind this are in [ADR 0008](../decisions/0008-device-pairing-and-client-protocol.md). The automated version is `cargo test -p osp-client` (see [`phone.rs`](../../crates/osp-client/tests/phone.rs)).

Build and install the node first, as in the [Stage 1 guide](stage1-manual.md#build).

## Run the node as a service

```sh
install -m 755 target/release/osp ~/.local/bin/osp
osp init --name "Your name"      # once, if this machine has no node yet
osp service install              # starts now, and at every boot
osp status                       # Version, uptime, devices and any problems
journalctl --user -u osp -f      # logs
```

`osp service install` writes `~/.config/systemd/user/osp.service` and turns on lingering, so the node starts at boot without anyone logging in. If logind refuses, it prints the `sudo loginctl enable-linger` command to run. Stop a node you started by hand first: only one node may run on a home, and a second one exits straight away with "already running". To remove the service, run `osp service uninstall`. The node's data stays.

To check: reboot without logging in, then from another machine (or from the phone) confirm the node answers.

## Pair a device

```sh
osp pair          # QR code + ticket; waits until the phone has paired
osp devices       # names, IDs, and whether each is connected now
osp unpair NAME   # by name, or by the start of the device ID
```

The code works once, within 10 minutes. Whoever uses it first becomes a paired device, so don't share it or show it on a screen others can photograph.

## The Android app

Build it as described in [`app/README.md`](../../app/README.md). Then install it in one of two ways:

- **Over USB:** turn on *Developer options → USB debugging* on the phone, connect it, accept the prompt on the phone, then run `adb install -r app/build/app/outputs/flutter-apk/app-release.apk`.
- **By copying the file:** copy the APK to the phone and open it. Android asks to allow installs from that app (Files, say) once.

First run: tap *Scan QR code* and scan the code from `osp pair`. If the camera can't read the terminal, run `osp pair --no-qr` and paste the text into *Paste the code instead*, for example by sending it to yourself.

What the app shows:

- **The dot in the top bar:** green means the node answered recently, amber means it reports a problem, red means it can't be reached. Tap it for details: direct or relay, round-trip time, the node's version, uptime, disk space and problems, and a diagnostics log.
- **When the node can't be reached:** a red banner, and the feed as last saved. Posting is disabled until the node is back.
- **On mobile data:** the app asks before playing a video over 10 MB or uploading over 20 MB.

## Without the app: a pretend phone

The example program `phone` runs the app's networking code on a computer. Run it on a different machine, or a different network, to test the path a phone away from home would take.

```sh
cargo build --release -p osp-client --example phone
P=target/release/examples/phone
$P pair <ticket>                     # the text line printed by `osp pair`
$P status                            # node health, plus direct or relay and RTT
$P feed
$P post "Hello from the phone" clip.mp4
$P get <blob hash> out.mp4           # through the local media server, like the player
$P forget
```

`--dir DIR` keeps several pretend phones apart. On the same machine as the node, the connection is always direct, even with `--bind 127.0.0.1:0`. To test a relayed path, run the pretend phone on another network (for example a laptop on a phone hotspot).

## Scenarios

| # | Phone network | What to do | Expect |
| --- | --- | --- | --- |
| 1 | Home Wi-Fi | Pair, open the feed, play a video, post a photo and a video | `status` on the phone says `direct`; video starts within a couple of seconds |
| 2 | Mobile data | The same | `relay` or `direct` (after hole punching); playback works at 720p |
| 3 | Switch Wi-Fi ↔ mobile data while a video plays | Watch | Playback stalls briefly at most; the connection survives or reconnects |
| 4 | Any | Stop the node (`systemctl --user stop osp`) | The phone says the node is unreachable and shows the saved feed |
| 5 | Any | `osp unpair` the phone | Its next request fails with "isn't paired"; pairing again works |
| 6 | Any | Reboot the node's machine, don't log in | The phone reaches the node again within a minute or two of boot |

## Results to record

Results here are public, so describe networks in general terms. Keep details of your own machines and network in `notes/`.

| Date | Node network | Phone network | Path (phone ↔ node) | Video start | Post (size, time) | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| 2026-10-21 | one machine, n0 relays | pretend phone, same machine | direct, 2 ms | — | 11 MB upload + transcode: 1.4 s | `phone` example; 0.8 MB rendition read back with matching BLAKE3 |

## Known limitations

- A paired device has full access until unpaired; no read-only devices.
- The phone uploads the original file; there is no phone-side transcoding or resume yet.
- Media the node hasn't fetched yet is downloaded in full by the node before the phone gets the first byte.
- The app doesn't keep media on the phone: when the node is unreachable, the saved feed shows text and layout, but no pictures or video.
- Android may stop the app in the background, which interrupts an upload. Keep the app open until the post is sent.
- The app looks up DNS with public resolvers rather than the phone's own settings. It still works, but networks that block outside DNS will break relay connections.
