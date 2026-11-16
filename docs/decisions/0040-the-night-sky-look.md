# ADR 0040: The night sky look

**Status:** Accepted (2026-11-16), the maintainer's choices the day they were asked. Step P9 of the Product Alpha ([ADR 0030](0030-product-alpha-before-testers.md)). Refines the tokens [ADR 0031](0031-app-shell-home-universe-you.md) §4 set in P1.

## Context

After P8 the Universe looked like its own thing, but every other screen was stock Material 3, generated from a seed colour that came out purple. Post actions wrapped onto two lines, "Delete" sat beside Like and Reshare as a full-width text button, and light mode had nothing of the sky in it. P9's gate is that screenshots of Home, the Universe and a profile are recognisably OSP. (It first said "without the logo"; the maintainer dropped that on 2026-11-16, choosing the app's icon on every screen, §7.)

## Decision

### 1. Night first, light too

The Universe's night is the brand. Dark is the main look: the sky's navy behind every screen, cards a step lighter, **starlight** gold for what is yours (your star, the bell's count) and **dusk** blue for what you can do. Light is kept for phones set to it, using the same three colours on a pale ground, so it never falls back to stock purple. The Universe itself, with its bars, cards and buttons, is night in both themes.

Material's colour slots are filled by hand from the palette (`ospScheme`) rather than generated from a seed.

### 2. One typeface: Inter

The app bundles Inter (SIL OFL 1.1), Regular, Medium and SemiBold, cut down to Latin, about 430 KB in all. Text in other scripts falls back to the phone's own fonts. Names drawn on the map use it too. Android and iPhone then look the same, and the screenshot tests show real text.

### 3. Posts: icons with counts

Under a post, one row of icons with their counts (likes, replies, reshares), each with a spoken label for screen readers. It never wraps. Delete and the owner's other actions go in a "⋯" menu.

### 4. Tokens, not loose values

`theme.dart` holds colours (`Palette`, `ospScheme`, `OspColors` for what Material has no slot for, the map's line and star colours among them), type, spacing (`Space`), corners (`Corner`) and motion (`Motion`). `Motion.of` gives zero when the phone asks for reduced motion. Screens take their look from these.

### 5. Pictures of the look

`app/test/look_test.dart` draws Home, the Universe and a profile, light and dark, with the app's fonts, and compares them with `app/test/goldens/`. A change to the look on purpose updates the pictures in the same pull request, so a reviewer sees it.

### 6. Motion and access on the map

The Universe flies (`Motion.long`) and crossfades when what it draws changes (`Motion.medium`): a new region, a layer, a search, a connection. Under reduced motion all of it is zero: the camera jumps, the map changes at once, the pinch hint holds still, and what the node did is told in words under the map without moving on it.

For screen readers, each star, group and sky cell on screen is a button in words ("Kim. You follow them. 1 step from you"), and the map has Zoom in and Zoom out actions, since a pinch can't be made. Names drawn on the map grow with the phone's text size up to 1.5 times, past which they would cover the stars they name; everything else grows with it fully, and `look_test` draws Home, the Universe and a profile at twice the size. Names keep off their own star's lines where they can, and have a rim of the night so a line passing under them doesn't cross them out.

### 7. The app's icon in the top bar, lit as the node is

The app's icon, cut to its blue tile, sits beside search and the bell on Home, the Universe and You. It is in full colour while the node runs, fades in and out while the phone reaches it (holding still under reduced motion), and is grey when the node can't be reached. Its tooltip and spoken label say the state in words. Tapping it opens the node's summary, one tap from anywhere. The banner above Home stays for a problem that needs words. So every screenshot carries the brand, and the node's state is always in sight.

## Steps

1. The tokens, Inter, the Universe's night in both themes, and the pictures.
2. Posts: the card, avatars, media and the action row.
3. Profiles, You and Activity, and empty, loading and offline states.
4. The Universe's transitions, and accessibility: screen readers, large text, reduced motion.
5. The app's icon in the top bar as the node's status (§7).

## Consequences

- The app grows by about 430 KB of fonts.
- A change to the look shows up as changed pictures, which is the point, and costs a `--update-goldens` run.
- Names in scripts other than Latin are drawn in the phone's font beside Inter. Adding other scripts is a later choice, weighed against the download.
