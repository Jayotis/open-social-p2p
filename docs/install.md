# Installing a node (invited alpha)

For the invited alpha's testers. One script installs the `osp` program, Tor and ffmpeg, creates your node, and runs it as a service that starts at boot.

## What you need

- **Linux on x86_64 or arm64,** with systemd: Debian 12, Ubuntu 22.04, Raspberry Pi OS 12 (64-bit), or anything newer. A Raspberry Pi 4 or 5, or a similar single-board computer, is the usual home for a node ([ADR 0006](decisions/0006-desktop-first-always-on-node.md)): it's meant to stay on.
- **sudo,** to install Tor and ffmpeg. Elsewhere than Debian, Ubuntu and Raspberry Pi OS, install them yourself first (`tor`, `ffmpeg` and `ffprobe`, `heif-convert`).
- **Paper and a pen,** for your 24 recovery words.

## Installing

The maintainer sends you the release to use. Run this as yourself, not as root, with `vX.Y.Z` replaced by that release:

```sh
curl -fsSLO https://github.com/Jayotis/open-social-p2p/releases/download/vX.Y.Z/install.sh
bash install.sh
```

It then:

1. installs Tor, ffmpeg and heif-convert with apt, if they're missing;
2. downloads `osp` for your machine into `~/.local/bin`, and checks it against the release's checksums (a damaged download is refused);
3. creates your node in `~/.local/share/osp`. It asks your name (as others see it), whether to share copies of posts you follow and whether to be listed by directories, then shows your recovery words and asks for three of them back;
4. sets the node to read the alpha's directories, from the release's `directories.txt`. To change them later, edit `[[directory.known]]` in `~/.local/share/osp/config.toml`;
5. installs the service (`osp service install`) and shows `osp status`.

If `~/.local/bin` isn't on your PATH, run the program as `~/.local/bin/osp`.

For the first few minutes, while Tor makes its first connections, a search may find nobody, and `osp directory read` may say a directory didn't answer. Try again a minute later.

## Removing a follower

`osp followers list` shows the nodes that follow you, with a name where your node knows whose node it is: someone you follow back, or a node that asked to be trusted. A subscription itself doesn't say who it's from.

`osp followers remove <name or start of node ID>` stops a node being your follower:

- it's no longer told of your new posts, named to your other followers as holding copies, or trusted;
- its subscriptions are refused until `osp followers allow <node>`. Its node finds out the next time it subscribes (about once a day);
- **your posts stay public:** it can still read them, as anyone can, through your node or a directory;
- it's one node, not a person. Someone who makes a new node can follow you again from it.

## Updating

Run the newer release's `install.sh` the same way. It stops the node, replaces `osp` (keeping the old one as `osp.prev`), and starts it again. Your node and its settings are left as they are.

## Removing

```sh
osp service uninstall
rm ~/.local/bin/osp ~/.local/bin/osp.prev
```

Your node stays in `~/.local/share/osp` until you delete it. Delete it only if you have your recovery words, or no longer want the identity.

## What it doesn't do

- **The checksums catch a damaged download, not a forged one:** they come from the same place as the program. Getting `install.sh` from the release page over HTTPS is what you're trusting.
- **The directories are the maintainer's, for the alpha only.** They're set by the installer, not built into `osp` ([ADR 0015](decisions/0015-nodes-meet-first.md)). A signed list of directories that can change without a new release ([ADR 0027](decisions/0027-directory-bootstrap-list-and-mirror.md)) comes after the alpha.
- **Debian's `tor` package also starts a Tor service of its own.** The node doesn't use it: it runs its own Tor. You can stop that one with `sudo systemctl disable --now tor`.
