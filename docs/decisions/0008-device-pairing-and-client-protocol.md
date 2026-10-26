# ADR 0008: Device pairing and the client protocol

**Status:** Accepted (2026-10-21)

## Context

The phone is a thin client of its owner's home node ([ADR 0001](0001-home-node-architecture.md)). Each phone has its own device key ([ADR 0004](0004-signed-objects-and-ids.md)), and the node must start at boot and report its health to the phone ([ADR 0006](0006-desktop-first-always-on-node.md)). Milestone M2 needs answers to five questions:

1. How does a phone prove it belongs to the node's owner, and how is that taken back?
2. What may a paired phone ask the node to do?
3. How does the phone's video player get media from the node?
4. How does the phone post a photo or video?
5. How does the phone learn whether the node is healthy?

## Options considered

**Pairing.** (a) A long-lived shared password typed into the phone. (b) A one-time secret in a QR code shown by the node, after which the phone is known by its device key. (c) Accepting any phone and asking the owner to approve it on the node.

**Media to the phone.** (a) iroh-blobs on the phone, downloading verified chunks from the node, plus a local server to feed the player. (b) Byte ranges streamed over the client protocol, behind a local server on the phone. (c) Downloading the whole file before playing.

## Decision

**Pairing: option (b).**

- `osp pair` on the node prints a **pairing ticket** as a QR code: the node's address and a 128-bit random secret. The ticket works **once**, within **10 minutes**. The node keeps only a hash of the secret, in memory.
- The address holds the node ID, its relay URL and its addresses on private networks, so a phone at home can connect directly even without internet. Public addresses are left out. That keeps the QR code small, and the phone learns them through the relay anyway.
- The phone dials the node ID from the ticket over `osp/client/0` and sends `Pair { secret, name }`. The QUIC handshake authenticates both ends by key: the phone knows it reached the node with that ID, and the node knows the phone holds its device key. The node records the device ID and name.
- From then on the device ID is the credential. **Every request checks it**, and an unknown device may only send `Pair`. `osp unpair` deletes the device and closes its connections. `osp devices` lists devices and shows which are connected.
- The check comes before the request is read, so an unknown device's request is held to 4 KiB, with at most 8 handled at once and 10 seconds each. A paired device's request may be up to 256 KiB (uploaded files follow it on the stream and don't count).

**What a device may do.** Every request the local `osp` tool can make, except:

- anything naming files on the node's own disk (`Post` attachment paths, `Fetch` with an output directory, `ExportBackup`, [ADR 0009](0009-identity-backup-and-recovery.md));
- managing devices (`NewPairing`, `Devices`, `Unpair`), which stays on the node itself.

A device's `Sync` returns once outboxes are synced. It doesn't wait for prefetching, because the phone can read media as soon as it needs it.

**Media to the phone: option (b).** `ReadBlob { blob, offset, len }` on `osp/client/0` returns the blob's size, then the raw bytes. On the phone, a small HTTP server on 127.0.0.1 turns the player's range requests into `ReadBlob`s, so seeking works. Other apps can reach 127.0.0.1 too, so every URL carries a random token.

**Media the node doesn't have yet streams to the phone.** The node starts fetching it, and replies once the byte at `offset` has arrived; the rest follow as they arrive, each checked against the hash before it's sent. The size in the reply is the one the post declares. Devices reading the same blob at once share one download, which carries on if they hang up, so the media ends up in the cache. If the fetch fails before the first byte arrives, the device gets the node's reason, as before. The app waits only for the start (`wait_for_media` in `osp-client`) before opening the player, because the player gives up if no bytes come for several seconds. The media server stops waiting for the node as soon as the player hangs up, after a seek for example. A node from before streaming replies only once it has the whole blob, so a newer app still works with it.

Why not iroh-blobs on the phone: the phone talks only to its own node, and that node already checked every chunk against the hash the author signed. The bytes then travel over a connection authenticated to that node. Option (b) keeps iroh-blobs and its store out of the app, and maps directly onto HTTP ranges. The `cache` serving rule (cached media goes only to paired devices, [ADR 0003](0003-native-social-protocol-v0.md)) stays in place for a future where a device fetches over iroh-blobs.

**Posting media.** A `Post` from a device lists its `uploads` (name and size), and the files' bytes follow the request on the same stream. The node writes them to a scratch directory, transcodes them like local attachments ([ADR 0005](0005-media-profile-v1.md)), publishes, and deletes the scratch copies. Limits: four attachments per post and 4 GB per file. Before reading any of the files, the node sets aside room for the bytes still to come, counted together with downloads in progress, and refuses the post if that would leave less than 2 GiB free on its disk (the level at which `status` warns, as for downloads). The room is given back when the stream ends, since the disk then shows what arrived. A post is also refused before any bytes are read when the node is already preparing four posts with media (ADR 0005).

**Resuming uploads.** A device names each post's upload with 32 random bytes (`upload` in `Post`). The node keeps the files it has received in `uploads/<device>-<id>/` in its home, and a row in its database records how far the upload got.

- If the connection breaks, the device sends `ResumeUpload`, which repeats the post's text and files, so the device never needs to know whether its `Post` arrived. The node replies `Resume { received }`, the bytes it holds counting the files back to back, and the device sends the rest. A node that doesn't know the upload starts it from byte 0. A new stream for an upload first stops any older one still receiving it (after a drop, the old one can linger until it times out).
- Once every byte is here, the node prepares the post whether or not the device is still connected, and records the post ID or the reason it failed. A resume after that gets the same reply, so **each upload is published at most once**.
- Uploads belong to their device. The media-post place and the disk room are taken by each stream, so a paused upload holds neither, only its partial files. A device keeps at most four unfinished uploads (starting a fifth drops the oldest). An unfinished upload untouched for 24 hours is dropped with its files; outcomes are kept for 30 days. `CancelUpload` drops an unfinished upload.
- The rows survive a restart of the node. At start, uploads whose files had all arrived are prepared, and folders that belong to no upload, as well as transcodes' scratch files, are deleted.
- A `Post` without `upload`, from an app older than resuming, behaves as before: a broken upload is dropped at once.

On the phone, `osp-client` saves the post (text, file paths, sizes and modification times) before sending it, and keeps it until the node publishes it or the owner discards it. When the connection breaks, or the bytes stop moving for 30 seconds, it reconnects after growing waits (1, 2, 5 and 10 seconds, then every 30) and resumes, after checking that no file changed. It gives up after **10 minutes without contact** with the node. That covers short breaks, such as a lift, a handover from Wi-Fi to mobile data or the home computer restarting; past that, something needs the owner's attention. The post is kept, so it can be carried on later.

In the Android app:

- When the owner presses Post, the picked files move from image_picker's copies in the cache folder, which Android may clear, into the app's own `uploads/` folder. Both are on the same disk, so this is a rename. A file outside the cache folder is copied instead, so the app never moves a file it doesn't own. The folder goes once the post is published or discarded; at start, folders that belong to no pending post are deleted.
- While a post is being sent, a **foreground service** (type `dataSync`) shows its progress in a notification and stops Android from stopping the app in the background. Android 13 and later ask the owner once whether the app may show notifications; the service runs either way. Swiping the app away from the recent apps stops the service, and the post then waits to be continued.
- The compose screen says "Waiting for your home node…" while it reconnects, and has a Cancel button, which discards the post on the phone and the node. Discarding waits at most 10 seconds for the node.
- A post that failed, or was cut off when the app stopped, stays on the phone. The compose screen offers Try again and Discard; once it's closed, the feed shows "A post didn't finish sending" with Continue and Discard. A post that gave up for lack of contact says whether the phone has no network or the home computer isn't answering.

**Health.** `Status` adds the node's version, uptime, number of devices, free disk space and a list of **problems in plain words**: no relay connection, no followed node reachable on the last sync, ffmpeg missing, or disk space low. `osp status` prints the same list.

**Always on.** `osp service install` installs a systemd **user** service that restarts on failure, and turns on lingering, so it starts at boot without anyone logging in. Root access is needed only if logind refuses lingering. A lock file stops a second node from starting on the same home.

**The phone's network.** The phone uses n0's relays and DNS lookup (resolve only), so it can find the node again if the node's relay changes. The phone never publishes its own address. Direct paths between phone and node are allowed ([ADR 0007](0007-transport-privacy-defaults.md)).

The phone side of all this lives in `crates/osp-client`. The Android app calls it through flutter_rust_bridge, and `cargo run -p osp-client --example phone` uses it from a computer.

## Consequences

- **The QR code is a password for 10 minutes.** Whoever uses it first becomes a paired device. `osp pair` says so, and shows the code only in the owner's terminal. The ticket also holds the node's private-network addresses.
- **A paired phone has full access** to the owner's node (post, follow, reshare) until it is unpaired. There are no read-only devices yet, and no per-device limits.
- **Unpairing stops new requests, not what the phone already has.** The node closes the phone's connections at once, but the saved feed and any media already buffered stay on the phone (a playing video carries on for up to a minute or so).
- **The device key lives in the phone's secure storage** (the `flutter_secure_storage` plugin). On Android it is encrypted with a key held in the Keystore, which can't be copied off the phone, so a copy of the app's files is useless elsewhere. It doesn't help against code running as the app on an unlocked phone, which can ask the Keystore to decrypt just as the app does. Older versions of the app kept the key as a plain file; it is moved into secure storage on the first start and the file deleted, so the phone stays paired. If secure storage can't decrypt the key (it clears itself then, after some resets), the app makes a new key, forgets its pairing and asks to pair again. On an iPhone the plugin would use the Keychain.
- **The phone trusts its own node** for media integrity. It doesn't re-verify BLAKE3 hashes itself.
- **Uploads send the original file.** A phone video can be hundreds of megabytes, which is costly on mobile data. The app should warn before posting on mobile data. Transcoding on the phone is a later option.
- **Seeking ahead of a download waits for it.** The node fetches a blob from start to end, so a seek past what has arrived waits until the download gets there. Asking for the sought part first is a later step, alongside the byte-range downloads in doc 13.
- **A post that declares the wrong size** can't be played while it streams: the phone is told the declared size, and the stream ends early if the blob turns out shorter. A larger blob is refused anyway (ADR 0003).
- **Interrupted uploads carry on** from where the node's copy ends, after a broken connection or a restart of the app or the node, for up to a day. A phone out of contact for 10 minutes stops trying and keeps the post. The Android app keeps sending in the background and offers to carry on after it restarts. The iPhone app, with its own background task, comes next.
- **An abandoned upload uses the node's disk for up to a day**: at most four per device, up to 4 GB each.

## Validation and revisit conditions

- `crates/osp-client/tests/phone.rs`: pairing, single-use tickets, refusal of unknown devices, device-only rules, posting a video, the feed, playback through the media server with ranges, revocation, a device key handed in from secure storage, a video the node is still fetching streaming to the phone (several requests sharing one download), and the reason reaching the phone when nobody can send it. `crates/osp-client/tests/resumed_uploads.rs`: a connection broken part way (in the second of two files) and after sending, while the node prepares; the app and the node restarting, with the node's copy checked byte for byte; giving up without contact; discarding; expiry, and none while receiving; a changed file; uploads belonging to their device; the four-upload limit; and an upload without an ID. `crates/osp-node/tests/uploads_at_start.rs`: what a node does at start with the uploads a stopped node left. `app/test/device_key_test.dart`: moving an old key file, and making a new key when the stored one is gone. `app/test/uploads_test.dart`: moving picked files into the app's folder (and copying files it doesn't own), and deleting a post's folder once it's done or left behind.
- Manual: the phone on home Wi-Fi and on mobile data pairs, reads the feed, plays a video and posts one ([M2 testing guide](../testing/m2-phone.md)).
- Revisit when: read-only or limited devices are needed; the phone needs to sign posts itself (identity-signed device grants, ADR 0004); or the phone fetches from other nodes.
