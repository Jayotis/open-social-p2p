# ADR 0012: Sharing prefetched copies is asked at setup

**Status:** Accepted (2026-10-24). Built (2026-10-24): `osp init` asks, with `--share` and `--no-share` to answer without asking.

## Context

A home node fetches new posts from the people it follows ahead of time, so they open at once on the owner's phone ([ADR 0001](0001-home-node-architecture.md)). The follower swarm ([doc 13](../13-hosting-roles-and-follower-swarm.md)) serves those copies to other nodes, so a popular post doesn't swamp its author's home connection. In the [B8 test](../testing/b8-creator-upload.md#results) with 300 followers sharing, the creator sent 20 copies of a new video instead of 300.

Sharing is the `share_prefetched` setting under `[swarm]` in `config.toml`. Phase 1 built it off by default while it was tried out. Doc 13 says setup should ask, explain what sharing reveals, and recommend yes, and that a short decision record should come first, since this changes a privacy default.

What sharing reveals, and costs:

- **Who you follow.** A node that downloads a copy from yours learns that you follow the post's author, or someone who reshared it. The author's node, or a hosting resharer's, may name your node to its other followers as having a copy, a few at a time (ADR 0003). Most social networks show follows anyway, but here it is shown to whoever asks.
- **Your internet address.** Media connections are direct by default ([ADR 0007](0007-transport-privacy-defaults.md)), so a node downloading from yours sees your address, unless relay-only mode is on.
- **Upload.** Serving copies uses upload bandwidth, within `max_upload_mbps` under `[serving]`.
- **Not what you watched.** Copies are fetched because you follow someone, not because you opened anything. Media you open from outside your follows stays in a private cache and is never shared.

The swarm only helps if most followers share. Most people keep defaults, so a switch that is off unless found in a file does little.

Questions to settle:

1. Is sharing on for a node whose owner was never asked?
2. How is the owner asked?
3. What happens to existing nodes?

## Options considered

1. **On unless the owner says no.** Setup asks, and pressing Enter means yes. Scripted setups share too. The swarm grows fastest, but people would reveal who they follow without having said yes.
2. **Asked, yes recommended.** Setup explains what sharing reveals and asks for a yes or no, recommending yes. Scripted setups say which with a flag. A node never asked doesn't share. Everyone who shares said so.
3. **Opt-in, not recommended.** Setup mentions sharing and how to turn it on, and leaves it off. The swarm stays small, and doc 13's B8 result wouldn't hold for real audiences.

## Decision

Option 2.

- **`osp init` asks**, both for a new identity and for `--restore`, before anything is created. It explains what sharing reveals in the words above, recommends yes, and waits for "yes" or "no". The answer is written to `config.toml`.
- **`--share` or `--no-share`** answers without asking, for scripts and test nodes. Without either, and with no answer (input closed), nothing is created, and the message names the two options.
- **The built-in default stays off.** A `config.toml` without the setting, such as one from before this change, doesn't share. Existing nodes keep what their file says; upgrading never turns sharing on.
- **Changing it later:** edit `share_prefetched` under `[swarm]` in `config.toml` and restart the node. A node that stops sharing tells the nodes it follows when it next subscribes (within a day, or at once after the restart), and they stop naming it. The copies it holds stay, served only to its own devices.

## Consequences

- **Everyone who shares said yes,** after being told what it reveals. Nobody shares by upgrading or by skipping a question.
- **The swarm depends on people saying yes.** If too few do, popular posts fall back on their authors, as in the B8 baseline. The recommendation, and how the question reads, matter.
- **Setup has one more question.** A scripted setup that passes neither flag stops with a message instead of guessing.
- **No protocol or database change.** Nodes already tell the nodes they follow whether they share (`Subscribe`), and followers that stop sharing stop being named.

## Validation and revisit conditions

- Tests: `osp init` writes `share_prefetched = true` for "yes" or `--share`, and `false` for "no" or `--no-share`; with no answer and no flag, it creates nothing; the question is asked for `--restore` too.
- Revisit when the phone app gains settings (Stage 3, "hosting preferences"): the question then belongs in the app's setup, and the setting on a settings screen. Revisit too if real use shows most people saying no: then the explanation, or the design, needs another look.
