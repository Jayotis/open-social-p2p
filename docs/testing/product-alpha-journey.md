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

## Run 2: arm64, `v0.1.0-alpha.7`, 2026-11-18

- **Node:** a fresh Debian 13 (arm64) virtual machine, emulated (no KVM), 4 cores, 2 GB: like a Raspberry Pi 4, only slower. A new user with sudo; no Tor, ffmpeg or OSP before.
- **Install:** as in Run 1, from the alpha.7 draft's files. Answers: a name, sharing yes, listing **no** (so the test account stays out of the directories), hide list yes.
- **Phone:** Pixel 4 XL, Android, the release APK (build 436) installed after removing the app and its data.
- **Network:** the alpha's directories, after Run 1's leftover test identities were dropped and the demo accounts (Kim, Jimmy, A) were given bios and posts.

| # | Step | Expected | Seen | Time |
|---|------|----------|------|------|
| 1 | Install the node | as in Run 1 | As expected; `Health: no problems`, Tor at 45% when shown | 3 min 13 s in all, almost all of it apt under emulation; `osp init` with questions a few seconds |
| 2 | Install the app and pair | welcome, terms, hide list, *Pair with your node* | As expected. Paired with the pasted code; the app asked for notifications, then opened on the Universe | about 3 min after the service started |
| 3 | Make a profile | | About line set in the app. The picture was set with `osp edit-profile --picture`, to keep the phone's own photos out of the picker; it shows on the profile and on Robin's own posts | |
| 4 | Enter the Universe | | Opened at once | |
| 5 | Watch it find people | | The three demo accounts with the lines between them, and "found 3 people, listed by …"; 14 machines heard of | under 4 min from install |
| 6 | Browse | | Kim's bio and listed posts before following | |
| 7 | Open a profile | | Kim: bio, Follow, *How are we connected?*, 5 listed posts | |
| 8 | Follow | | Followed Kim; her profile filled to 12 posts, 5 with media; a 1-minute video streamed and played | |
| 9 | Home fills | | Kim's posts and Robin's reshare | |
| 10 | Post | | A text post from the app. A photo and a 5-second video posted from the node with `osp post -a`: transcoded to 720p and shown in the app | photo 12 s, video 33 s (emulated) |
| 11 | Like | | In Kim's Activity at once | |
| 12 | Reply | waits for the author's yes | "Waiting for Kim to look at it" in the app; "waiting for you" in Kim's Activity | |
| 13 | Reshare | | "Share, and help keep it available": the photo (274 KB) fetched and kept; "Reshared by Robin, who hosts a copy" on Home | |
| 14 | Look at the network | | Own star: *Your node*, phone reaches it, running 10 min, no problems | |
| 15 | Come back after a restart | | VM rebooted: the service started without a login; app force-closed and reopened: Home at once with everything there | `osp status` answered 77 s after the reboot |

Run 1's findings: own pictures now show on own posts, and the directories bring only the demo accounts.

### Found

- **The hide-list question comes twice.** The installer asks it, then the app asks again before pairing, as it can't know the node's answer yet. The node keeps the installer's answer and the app's goes unused, so someone who answers differently in the app isn't told their answer didn't count. The app could ask after pairing instead, only when the node wasn't asked (it already does for a paired phone).
- **A reshare doesn't show on its button.** After resharing, the post's reshare icon looks the same; the like heart turns blue when liked.
- **The letter circle changes colour.** Robin's circle is yellow everywhere but teal on *Edit your profile*.
- **Not covered:** choosing a photo in the app (picker), for the reason in step 3.
