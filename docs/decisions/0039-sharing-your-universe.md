# ADR 0039: Sharing your Universe

**Status:** Proposed (2026-11-14). Step P10 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). The one thing the app makes to leave it. Stricter than [ADR 0033](0033-what-the-universe-may-show.md)'s allowlist, which is for Kim's own phone.

## Context

People share what's distinctive. A picture of Kim's part of the network, with what her node did, is something no other social app can make, and it shows others what OSP is. It is also an image that leaves OSP for good, so it must hold only what is public, or what Kim chose to show.

## Decision

### 1. The card

**Share my Universe** makes an image like this:

> **Open Social P2P**
> 68 people discovered · 14 direct connections
> 6 separate paths into the wider network
> 1.8 GB served peer to peer

and a drawing of Kim's part of the Universe.

Its counts are: people her node knows; her follows and the people she knows, as one number; separate chains from her side ([ADR 0037](0037-how-are-we-connected.md)); bytes served, rounded to two figures.

### 2. Built from its own allowlist

The node builds the card's data with its own request (`ShareCard`), not from a screenshot of the Universe:

- **Kim's follows aren't shown by name.** They aren't public in OSP, and a card names nobody unless Kim ticks that person. Everyone else is an unnamed point.
- **No machines.** No machine names, and no Network layer.
- **No addresses, IDs or times.** Totals are rounded, and nothing says when.
- **No gaps' contents, no requests, no Activity.**

### 3. Kim sees exactly what's shared

The card is shown full size before it goes anywhere, with the people she named listed under it. It leaves the phone only through the phone's own share sheet, when Kim taps it.

### 4. Sharing a profile, a post, or OSP itself

A profile or post is shared as a copyable **OSP code**, as text, with a line of plain words in front for people who don't have OSP:

- **A profile:** `osp:person/<invite code>`, today's follow code with a prefix, with the person's name.
- **A post:** `osp:post/<post ID>?by=<invite code>`, with its first lines. Kim's node opens the post from its author, or as a found post if Kim doesn't follow them.
- **OSP itself:** the install link. Until there's a project site, that's the install guide on the repository's host.

In the alpha, the app takes an OSP code pasted anywhere it takes an invite code, and offers to open one found on the clipboard. The app also registers `osp:` as a link scheme, so tapping a code where links work opens it, but nothing depends on that. Web links that open the app (Android App Links, iOS universal links) need a domain the project controls, and come later.

There is no web page for a profile or post: OSP has no server to put one on.

## Options considered

- **Share a screenshot.** Phones do that anyway. A made card can leave out what a screenshot can't.
- **No card.** Safer, but it gives up the one thing people would show others.

## Consequences

- Kim can still screenshot the Universe, and that shows her follows at step 1. The first run says so ([ADR 0033](0033-what-the-universe-may-show.md)).
- A card can't be checked by whoever sees it. It's Kim's claim, like any screenshot.

## Gate

A generated card or link can't disclose anything that isn't already public or that Kim didn't approve. Tests: on a simulated world, a card's data holds no name Kim didn't tick, no machine, no ID, no address and no time; its totals are rounded; the preview is the image that's shared, byte for byte; an OSP code holds only an invite code, a post ID and the prefix, and a malformed one is refused with a plain message.
