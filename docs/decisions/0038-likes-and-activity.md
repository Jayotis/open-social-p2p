# ADR 0038: Likes, and one Activity inbox

**Status:** Proposed (2026-11-14). Step P8 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). Takes up M5's planned reactions. A new object in the native protocol ([ADR 0003](0003-native-social-protocol-v0.md)), sent the way replies are ([ADR 0028](0028-replies-let-in-by-the-creator.md)).

## Context

Kim posts and hears nothing back unless someone replies or reshares, and even then she has to look in several places. Most feedback on a social network is smaller than a reply.

## Decision

### 1. One reaction: Like

A **`Like { object }`** is signed by the person who likes, as AS2's `Like`. Only one kind, for now. Whether OSP ever has a dislike, or more reactions, is a later product decision.

- **Sending.** Jimmy's node sends it to the creator's node on the social endpoint, as it sends a reply, and tries again until it's answered (at most 7 days, not a reply's 30). One like per person per post; a second replaces the first.
- **Taking it back.** A signed `Undo` of the like, sent the same way.
- **Limits.** A like publishes nothing on the creator's behalf, so it needs no admission. The creator's node takes at most 60 likes an hour from one node, 2,000 new likes an hour in all, and keeps at most 100,000, dropping those on the oldest posts first.
- **Who sees what.** The creator's node shows Kim who liked her posts. Anyone reading a post's thread (`osp/replies/0`, through a viewing endpoint) sees how many people liked it, not who. The likes are signed, so a later version could show who.

### 2. What a like is not

A like doesn't count toward a node's standing, a voucher ([ADR 0029](0029-vouchers-within-six-steps.md)), hosting priority, or the order of any feed. Each of those would make likes worth faking. Changing that needs its own record.

### 3. Activity

One inbox, kept on the node, so each of Kim's phones sees the same thing read and unread:

- replies to her posts, and replies waiting for her ([ADR 0028](0028-replies-let-in-by-the-creator.md));
- likes, grouped ("Jimmy and 3 others liked your post");
- reshares of her posts (`Reshared`, [ADR 0029](0029-vouchers-within-six-steps.md));
- new followers, and "Jimmy says he knows you" ([ADR 0013](0013-trusted-followers.md));
- her own replies published, as her node hears.

Each entry opens what it's about, and the words are those of a social app, not a log ([ADR 0031](0031-app-shell-home-universe-you.md)). A creator's node never tells a replier their reply was declined, and Activity doesn't either.

**Notifications.** On Android, the app checks its node in the background and notifies. iPhones need Apple's push service, which needs a server, and OSP has none. On an iPhone, Activity shows when the app is opened. Push for iPhones is left for later.

## Options considered

- **Several reactions from the start.** More expressive, more to decide, and each one needs words in every language.
- **Likes as public lists**, as on Mastodon. Every like would become a public record of who reads whom. Counts are enough for the alpha.
- **Likes that count toward standing or ranking.** That's exactly what likes farms are for. Left out.

## Consequences

- One new object type. Nodes older than this version refuse it, which is fine: a like goes only to the creator's node.
- The thread endpoint carries a count per post.

## Gate

Kim posts, and sees likes, replies and reshares come in, in one place, without checking several screens. Tests: a like and its undo across Tor; the limits per node and in all; a like changes no standing, vouch or feed order; two phones agree on what's read.
