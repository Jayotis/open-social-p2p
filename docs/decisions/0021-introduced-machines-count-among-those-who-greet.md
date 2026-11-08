# ADR 0021: A machine introduced counts among those who greet

**Status:** Accepted (2026-11-08), while building [ADR 0016](0016-machine-acquaintances.md)'s stage A3. It changes two rules of ADR 0016's introductions; the rest stand. Built with stage A3 (2026-11-08). The second rule, and a lower bar for asking, were added the same day after [stage A3's first run](../testing/meeting.md#introductions) missed its gate. The second run missed it too ([results](../testing/meeting.md#introductions-results)): at the end, nodes held newcomers through an introduction 37 times and one operator's names 378 times, and newcomers were met by fewer nodes than with stage A2 alone. So introductions are off by default (2026-11-08); these rules apply when a node turns them on.

## Context

In ADR 0016, a machine, Jimmy, can ask one it has met, Alice, to introduce it to Kim, a machine whose record Alice passed on. Kim accepts if she has known Alice for 3 days and Alice's records checked out, at most one an hour, and then dials Jimmy in the hour's turn for those who greet. ADR 0016 also counts Jimmy "against its introducer's source, as a record Alice passed on would": a place in Alice's share of Kim's table, "instead of the crowded places of those who greet".

Building it showed a way round stage A2's crowd limit. Neither Alice nor Kim can tell an operator's name from a newcomer, so an operator's names can ask honest machines to introduce them. Each one accepted would hold a place in an honest machine's share, outside the 40 places of those who greet. Once met, it would be a source of its own, with up to 32 places for what it passes on, as is any machine an honest one passed on. Kim's one an hour would be the only brake: up to 24 more of the operator's names a day on each node, where the crowd limit allows 40 in all.

## Decision

**A machine accepted through an introduction counts against those who greet,** as if it had greeted Kim, and is **dialled first** in the hour's turn for those who greet, ahead of machines that only greeted. What it passes on counts against those who greet too, until it's known (stage A2's stand-in) or has carried jobs Kim checked (stage B), as for any machine that greeted. While it waits for that dial it keeps its place, even one taken in turn; the dial comes within the hour, as Kim accepts one an hour.

**Why:** an introduction is another machine's word, not work Kim checked herself. Under ADR 0016's rule that checked work, not testimony, lets a machine out of the places of those who greet (2026-11-08), a reference gets a machine only a turn at carrying sooner. An introduction gets it a turn at being met sooner. That is what newcomers lacked in stage A2's runs: they kept their places, with a median of 23 established nodes holding each, but waited for the dial ([results](../testing/meeting.md#newcomers)).

**What this replaces,** in ADR 0016's "Introductions": "A machine met through an introduction counts against its introducer's source, as a record Alice passed on would, so one introducer fills at most its quarter of Kim's table", and, in "What it gets Jimmy", "and to count it under Alice's share of her table instead of the crowded places of those who greet".

**An introducer still among those who greet introduces no one** (2026-11-08, after stage A3's first run, the maintainer's choice). Kim accepts an introduction only from a machine she doesn't count among those who greet: one she heard of from another machine or a directory, or, under stage A2's stand-in, one she knows. In the first run, nodes held 670 of the operator's names through introductions by the time its first name could be known. Once met, each was passed on by honest machines outside the 40 places, and the operator's names not yet known reached 56 on one node.

**Why:** the same as above. An introduction is the introducer's word, and a machine that came in by greeting, with no work Kim checked, has no word with her yet. Otherwise a crowd's names, each greeting and answering, introduce one another.

**What this replaces,** in ADR 0016's "Introductions": "Nothing here is earned: it takes no standing, and no history beyond having met the machine asked", for the introducer only. Jimmy still needs nothing, and nor does Alice, beyond how Kim came to hear of her. Most of a node's machines came through other machines' records, so most honest introducers qualify.

**What stands:** Jimmy names only a machine whose record Alice gave it. The limits stay: Alice passes on 3 requests a day for one machine asking and 12 in all, and Kim accepts one an hour from a machine she has known for 3 days. Alice gets the same answer whatever Kim decides. Requests carry IDs, expiries and Jimmy's signature, and each part can be turned off in `config.toml`. When Kim dials Jimmy, what happens counts for or against Alice, as it would for a record Alice passed on.

## Consequences

- Jimmy gets a place among those who greet and a dial sooner, not a place in Alice's share. An introduction can't carry a crowd past the 40 places.
- A crowd's names can't introduce one another until they're let out of those places. They can still ask honest machines to introduce them, using up introducers' limits and Kim's one an hour, and with them the hour's turn for those who greet. Stage A3's run measures this.
- **Settled while building, as starting values:**
  - **When a machine asks:** while fewer than 10 machines it has met have greeted it in the last week, met before or after they greeted, so newcomers ask and established machines don't. Only machines it met count, so strangers greeting it can't make it stop. 10 is the newcomer run's bar for a start. The first bar, 32, counting only machines met before their greeting, kept established machines asking for days in stage A3's first run, and their requests used up the introducers' limits.
  - **How it asks:** in a conversation it's having anyway, right after `Known`, naming one of the machines just passed on, and each machine at most once a week. Asking adds no circuits.
  - **How Alice passes it on:** in her next round, in a conversation of its own, alongside the round's others so it doesn't hold up her dials.
  - **"Nothing Alice passed on has failed her checks"** is read as it is for being known: one record that didn't check out is forgiven, as a machine can go for good between being passed on and being dialled.
  - **Kept in memory only:** limits and requests seen. After a restart, a machine may ask a while longer, and its limits start afresh.

## Validation

ADR 0016's stage A3 gate, unchanged: [the newcomer run](../testing/meeting.md#introductions), once, with newcomers asking. Its marks: newcomers met by more established nodes than with A2 alone; one operator's names, introducing one another, held within A2's mark; and honest newcomers still introduced while the operator uses up introducers' limits.

**Revisit** if a machine introduced loses its place before it's dialled, or if the turn for those who greet goes mostly to a crowd's introductions.
