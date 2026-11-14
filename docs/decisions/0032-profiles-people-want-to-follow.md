# ADR 0032: Profiles people want to follow

**Status:** Proposed (2026-11-14). Step P2 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). Adds to the `Person` object of [ADR 0003](0003-native-social-protocol-v0.md) and to the client protocol of [ADR 0008](0008-device-pairing-and-client-protocol.md).

## Context

A `Person` today has a name (200 bytes at most), a summary (2,000 bytes), whether a directory may list it, its nodes and its media endpoints. It is set at `osp init` and can't be changed from the phone. Someone found in the Universe is a name and a key: nothing says whether they're worth following.

## Decision

### 1. Two new fields in `Person`

- **`icon`**, a profile picture: one image attachment, as a post's (its hash, type and size). The node makes it from the photo the phone sends, as it makes a photo post's image ([ADR 0005](0005-media-profile-v1.md)): WebP, square, 400 px, at most 64 KiB, metadata stripped.
- **`attachment`**, up to 4 links, each a name (32 bytes at most) and an `https` URL (256 bytes at most). This is the form Mastodon uses for profile links (AS2 `PropertyValue`), so Stage 2's bridge can carry them. Links are shown as plain links and aren't checked. Checking a domain is separate (below).

Both are optional. A `Person` is signed over its exact bytes, and nodes ignore fields they don't know, so an older node reads a new profile without the picture and links.

### 2. Who gets the picture, and how

The picture is a file like any other, so it goes where profiles go:

- **Followers** get it with the profile, as they get a post's media.
- **Directories** list only its hash. A listing stays the same size, give or take a few bytes.
- **Anyone else** fetches it from the person's media endpoint through a viewing identity ([ADR 0024](0024-watching-found-posts-without-your-identity.md)), so Jimmy's node never learns that Kim looked at his profile. Kim's node fetches a picture only when Kim opens the profile, or when the Universe shows that person up close ([ADR 0034](0034-a-universe-in-levels.md)). That's at most 200 new pictures an hour, kept with found posts' media and evicted with it.

### 3. Editing on the phone

Two new client requests:

- **`SetProfile { name, summary, links, icon }`** signs and publishes a new `Person`, as `osp init` does now. The picture comes up as a resumable upload ([ADR 0008](0008-device-pairing-and-client-protocol.md)). Followers get the new profile on their next sync, and directories on their next read.
- **`Profile { actor }`** returns what the node knows of one person: their profile, their posts, media and reshares that it holds, whether Kim follows them, and whether they're among the people Kim knows.

`osp profile` does the same from the command line.

### 4. The profile page

A profile shows the picture, name, summary and links, then their posts, a grid of their media, and their reshares. Its actions are **Follow**, and in plain words ([ADR 0031](0031-app-shell-home-universe-you.md)) the trusted-people actions of [ADR 0013](0013-trusted-followers.md): "I know Jimmy" (trust someone you follow) and "Ask Jimmy to confirm he knows you" (ask for trust).

**Counts.** Who follows whom isn't public in OSP, so Kim's node can't know how many followers Jimmy has, and doesn't guess. The alpha shows:

- on **your own** profile: how many people follow you and how many you follow (your node knows both);
- on **anyone's**: how many posts your node holds of theirs, and, on posts you've shared, how many people reshared them (already shown).

No follower counts for other people. This also leaves out a popularity number the roadmap wanted decided separately.

### 5. A verified domain, later

Checking a domain (M4's plan: the profile names a website, the website names the key) isn't built. When it is, the profile shows it. It isn't part of the Product Alpha's gate.

## Options considered

- **A picture inside the profile, as bytes.** Simpler, but a profile is limited to 128 KiB, a directory listing would grow by 64 KiB a person, and every change would resend it.
- **A picture fetched from a directory.** Fewer fetches from home nodes, but directories would then hold files, which is ADR 0026's and ADR 0027's territory.
- **Public follower counts, signed by the person's node.** Possible, but they'd be the person's claim, they'd be hard to check, and they add a popularity number. Left out.

## Consequences

- **Privacy.** Kim's picture is as public as her name. A picture can identify someone more than a name can; the edit screen says it's public before Kim chooses one. Fetching others' pictures never uses Kim's identity.
- **Load.** One more small fetch per person viewed up close, through viewing endpoints that have limits already. The hourly cap bounds it.
- **Compatibility.** Additive. Nodes before this version show no picture or links.

## Gate

Discovering a person, opening their profile, understanding who they are and following them is a complete journey in the app, with no command line. Tests: a profile with a picture and links round-trips through a directory and a follow; an older `Person` without them still verifies; a picture is fetched through a viewing identity only; a picture over 64 KiB or a link that isn't `https` is refused.
