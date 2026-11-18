# Product Alpha journey on clean installs (P11 part 5)

The roadmap's P11 journey, from installing the node to coming back after a restart, run on a computer that has never had OSP and a phone with the app freshly installed. Each step records what should happen, what was seen, and how long it took.

## Run 1: x86_64, `v0.1.0-alpha.6`, 2026-11-18

- **Node:** a fresh Debian 13 (amd64) virtual machine, KVM, 4 cores, 2 GB. A new user with sudo; no Tor, ffmpeg or OSP before.
- **Install:** the draft release's files, copied in; `install.sh` run with `OSP_BASE=file://…` pointing at them. Otherwise exactly as [docs/install.md](../install.md).
- **Phone:** Pixel 4 XL, Android, the release APK (build 435) installed on an app with no data.
- **Network:** the alpha's directories (two). Behind NAT; the node reports a varying public address, so peers mostly reach it through the relay.

| # | Step | Expected | Seen | Time |
|---|------|----------|------|------|
| 1 | Install the node | apt adds Tor, ffmpeg, heif-convert; `osp` checked against `SHA256SUMS`; name, sharing and listing questions, 24 words and three back; service installed; `osp status` | As expected. `Health: no problems`; Tor at 5% when shown, as the closing note warns. | apt 16 s, `osp init` with questions 37 s; about 1 min from `sudo` to a running node |
| 2 | Install the app and pair | APK installs; first start says it needs a node, then the terms, then *Pair with your node*; `osp pair` QR scanned; the app opens on Home | As expected. Paired 2 min after the service started; `osp devices` shows the phone, connected | — |
| 3 | Make a profile | photo and an about line from You | Both set; `osp profile` shows them | |
| 4 | Enter the Universe | | Opened; people appeared "fairly fast" | |
| 5 | Watch it find people | the Universe fills as the node finds people through directories | Both directories answered within 2 min of start (2 and 3 people); everyone's name shown; Kim's listed posts and media open without following her. A came in through Kim's reshares. 13 machines heard of, one met | under 2 min to the first people |
| 6 | Browse | | | |
| 7 | Open a profile | | | |
| 8 | Follow | | Followed Kim and both Jimmys | |
| 9 | Home fills | | Kim's posts on Home after following | |
| 10 | Post | | Posted a photo | |
| 11 | Like | the author's Activity shows it | In Kim's Activity at once | |
| 12 | Reply | a stranger's reply waits for the author's yes | In Kim's Activity as "waiting for you" (she doesn't follow alex) | |
| 13 | Reshare | | Reshared one of Kim's posts; its media (10.6 MB) fetched from her node | |
| 14 | Look at the network | reached from your own star without Advanced; plain words | Tapped the home star in the Universe: *Your node* with what it did today; Advanced details useful | |
| 15 | Come back after a restart | node and app restarted; the app reconnects; follows, posts and Home are still there | VM rebooted: the service started without a login. App force-closed and reopened: reconnected at once, no questions; everything still there | VM back in 25 s; app instant |

### Found

- **Your own picture isn't on your posts.** Post cards and the people list show only the first letter, for everyone: a post doesn't carry its author's picture. Strangers' letters are deliberate (ADR 0032 §2: their picture is fetched only when their profile is opened), but your own picture and those of people you follow are already on your node, so showing them costs nothing.
- **Most people a newcomer meets are empty test accounts.** Of the 6 people the directories brought, 4 (two Jimmys, `osptest`, `osptester1`) are test identities with no posts. The Universe looks emptier than the network is. Before outside testers, they should leave the directories or post.

- **PATH note after install.** On Debian the installer ends with "`~/.local/bin` isn't on your PATH: run it as `~/.local/bin/osp`, or add that folder to PATH". Debian's `~/.profile` already adds that folder at login when it exists, so logging in again (a new terminal or SSH session) is enough. The note should say that first.
