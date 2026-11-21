# Installing OSP

OSP has two parts, installed in this order:

1. **Your node:** the `osp` program, on a computer at home that stays on. It holds your identity and does all the networking. The app doesn't work without one.
2. **The app,** on your phone: a remote control for your node.

Then you **pair** them, once. All three take about 15 minutes.

## What you need

- **For the node, Linux on x86_64 or arm64,** with systemd: Debian 12, Ubuntu 22.04, Raspberry Pi OS 12 (64-bit), or anything newer. A Raspberry Pi 4 or 5, or a similar single-board computer, is the usual home for a node ([ADR 0006](decisions/0006-desktop-first-always-on-node.md)): it's meant to stay on.
- **sudo** on that computer, to install Tor and ffmpeg. Elsewhere than Debian, Ubuntu and Raspberry Pi OS, install them yourself first (`tor`, `ffmpeg` and `ffprobe`, `heif-convert`).
- **An Android phone,** Android 7 or newer, 64-bit (nearly every phone since 2017). The iPhone app is in testing through TestFlight and not open to others yet.
- **Paper and a pen,** for your 24 recovery words.

Everything comes from the same release, on the [releases page](https://github.com/Jayotis/open-social-p2p/releases). Use the newest one, at the top; below, replace `vX.Y.Z` with its name.

## 1. The node

On the computer that will run your node, as yourself, not as root:

```sh
curl -fsSLO https://github.com/Jayotis/open-social-p2p/releases/download/vX.Y.Z/install.sh
bash install.sh
```

If the person who invited you gave you a relay token, have it ready: the installer asks for it (or set it first with `export OSP_RELAY_TOKEN=…`). The project's relays carry video between nodes that can't reach each other directly, and they let in only nodes with the token. Without it, the node uses n0's public relays, which are slower for video. Keep the token to yourself.

It then:

1. installs Tor, ffmpeg and heif-convert with apt, if they're missing;
2. downloads `osp` for your machine into `~/.local/bin`, and checks it against the release's checksums (a damaged download is refused);
3. creates your node in `~/.local/share/osp`. It asks your name (as others see it), whether to share copies of posts you follow, whether to be listed by directories, whether to use the project's hide list, your upload speed and whether the nodes yours picks may see its address (see [Carrying](#carrying) below), then shows your recovery words and asks for three of them back;
4. sets the node to read the alpha's directories, from the release's `directories.txt`. To change them later, edit `[[directory.known]]` in `~/.local/share/osp/config.toml`;
5. with a token, sets the node to use the project's relays, from the release's `relays.txt`, and n0's only while those let nobody in. They're `relay`, `relay_token` and `relay_fallback` under `[network]` in the same file;
6. sets the seed carriers (ADR 0043), from the release's `carriers.txt` if it has one: the nodes that may carry video next to yours from the start. They're `[[carrier.known]]` in the same file;
7. installs the service (`osp service install`), so the node starts at boot, and shows `osp status`.

If `~/.local/bin` isn't on your PATH, run the program as `~/.local/bin/osp`.

### Carrying

Your node carries encrypted video between other nodes, under its machine name, within the limits you set. This uses some of your upload, and builds your node's standing with the nodes it helps, which can get it direct links and quicker service. You can't see what's inside. The nodes next to yours see your internet address sending it, never who you are, and you answer for what your connection sends, as with anything else you do online. You can pause it under Advanced.

Carrying uses half your upload at most, always behind your own traffic, so setup asks your upload speed (a speed test shows it; a guess is fine). Skipped, it takes a slow home's 10 Mbit/s until you set it. `osp carrier` shows how it goes, and changes it:

- `osp carrier upload <Mbit/s>` sets your upload speed, and `osp carrier share <percent>` the share carrying may use;
- `osp carrier jobs <n>` sets how many it carries at once (2 to start);
- `osp carrier metered yes` stops it while your connection is metered;
- `osp carrier pause` stops it for troubleshooting, and `osp carrier resume` starts it again. A paused node still gets every post and video.

Setup also asks one question about your own video:

> Your node can fetch and send video through other nodes it picks, which is much faster than the public fallback. The node next to yours sees your internet address and which node you're fetching from, as the fallback does today, but never what's inside.

Yes is recommended. With no, or until you answer, your node's own video goes through the public fallback relays, as today. `osp hops yes` or `osp hops no` changes it at any time.

For the first few minutes, while Tor makes its first connections, a search may find nobody, and `osp directory read` may say a directory didn't answer. Try again a minute later.

## 2. The app

Download `osp-android-arm64.apk` from the same release, on the phone itself (open the releases page in its browser), or on a computer and then copy it to the phone. Open it, and allow installs from that app (your browser or Files) when Android asks; it asks once.

To check the download first, on a computer: `sha256sum osp-android-arm64.apk` must print the same as the APK's line in the release's `SHA256SUMS`.

The APK is signed by OSP. Android installs an update over it only when the update is signed with the same key, so a newer release's APK installs over it and keeps your pairing.

## 3. Pairing

The app first says what it needs, a node, then shows the [terms of use](terms.md) to agree to. Then it opens on *Pair with your node*. On your node's computer, run:

```sh
osp pair
```

It shows a QR code. In the app, tap *Scan QR code* and point the camera at it. `osp pair` says when the phone has paired, and the app opens on Home.

- The code works **once, within 10 minutes**, and whoever uses it first becomes a paired device, so don't share it or show it where others can photograph it.
- If the camera can't read the terminal, run `osp pair --no-qr`, send the text line to your phone, and use *Paste the code instead*.
- The phone doesn't have to be at home: it reaches the node from anywhere, faster on the same Wi-Fi.
- `osp devices` lists paired phones; `osp unpair <name>` forgets one.

## Removing a follower

`osp followers list` shows the nodes that follow you, with a name where your node knows whose node it is: someone you follow back, or a node that asked to be trusted. A subscription itself doesn't say who it's from.

`osp followers remove <name or start of node ID>` stops a node being your follower:

- it's no longer told of your new posts, named to your other followers as holding copies, or trusted;
- its subscriptions are refused until `osp followers allow <node>`. Its node finds out the next time it subscribes (about once a day);
- **your posts stay public:** it can still read them, as anyone can, through your node or a directory;
- it's one node, not a person. Someone who makes a new node can follow you again from it.

## Blocking

`osp block account <name or start of their identity>`, `osp block post <ID or OSP code>` or `osp block file <hash>` makes your node drop what it holds of that person, post or file, and stop fetching, serving and showing it. Blocking a person also stops following them and hides their replies on your posts. Nothing is sent: they aren't told.

- Your own posts and reshares stay; delete them to take them down.
- `osp blocks` lists what you blocked; `osp unblock account …` (or `post`, `file`) undoes one. What was dropped comes back only as your node fetches it again.

## Updating

**The node:** run the newer release's `install.sh` the same way. It stops the node, replaces `osp` (keeping the old one as `osp.prev`), and starts it again. Your node and its settings are left as they are.

**The app:** install the newer release's APK the same way, over the old one. It stays paired.

When the app says "your node doesn't know this request yet", the app is newer than the node: update the node.

## Removing

The app: uninstall it as any other; then `osp unpair <name>` on the node. The node:

```sh
osp service uninstall
rm ~/.local/bin/osp ~/.local/bin/osp.prev
```

Your node stays in `~/.local/share/osp` until you delete it. Delete it only if you have your recovery words, or no longer want the identity.

## What it doesn't do

- **The checksums catch a damaged download, not a forged one:** they come from the same place as the program. Getting `install.sh` from the release page over HTTPS is what you're trusting. The APK's signature does more for updates: Android refuses one not signed with the key of the app already installed.
- **The directories are the maintainer's, for the alpha only.** They're set by the installer, not built into `osp` ([ADR 0015](decisions/0015-nodes-meet-first.md)). A signed list of directories that can change without a new release ([ADR 0027](decisions/0027-directory-bootstrap-list-and-mirror.md)) comes after the alpha.
- **Debian's `tor` package also starts a Tor service of its own.** The node doesn't use it: it runs its own Tor. You can stop that one with `sudo systemctl disable --now tor`.
