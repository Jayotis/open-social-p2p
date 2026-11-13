# Replies on real nodes

**Status:** [ADR 0028](../decisions/0028-replies-let-in-by-the-creator.md)'s test on real nodes, run once on 2026-11-13 ([results](#results)). Each step's expected result is the ADR's.

The automated checks are in `crates/osp-node/tests/replies.rs`, `tests/replying.rs` and the app's `test/replies_test.dart`. This run checks the same things between real nodes, over Tor, with the app on phones.

## The setup

Three home nodes on one home network, running main as of 2026-11-13 (ADR 0028 steps 1 and 2). Messages between nodes go over Tor (ADR 0014).

- **Kim** posts. Kim's node follows Alice and nobody else, and trusts nobody. Kim's phone, an Android phone, is paired with it.
- **Alice** is someone Kim follows. Her phone, a second Android phone, is paired with her node.
- **Jimmy** is a stranger: his node follows Kim, but Kim doesn't follow or trust him.

Every node's reply policy was the default, `network`.

## The steps

| # | Who | Does | Expected |
| --- | --- | --- | --- |
| 1 | Kim | Posts (CLI) | |
| 2 | Alice | Replies (CLI) | Published at once |
| 3 | Jimmy | Replies (CLI) | Goes to Kim's requests; Kim's badge counts it |
| 4 | A reader | Reads the thread (CLI, app) | Gets the published replies; the creator's node learns nothing about who reads |
| 5 | Jimmy | Replies while Kim's node is stopped | Kept, tried again, and arrives once after Kim's node is back |
| 6 | Kim | Publishes Jimmy's reply from the app's requests | In the thread; Jimmy's node learns it's published |
| 7 | Alice | Replies from the app | Published, and shown in the thread |
| 8 | Jimmy | Deletes his published reply (CLI) | Gone from the thread |
| 9 | Kim | Publishes and allows Jimmy's next one (app); Jimmy replies again | The new reply is published without asking |
| 10 | Kim | Hides a reply (app) | Gone for readers; dimmed and marked hidden for Kim; nothing deleted |
| 11 | Kim | Sets the post's replies to Nobody (app); Jimmy replies | Refused as closed; the reply box goes from Kim's thread |
| 12 | Kim | Deletes the post (app, after the one-minute undo) | Readers' nodes drop the post and its thread |

## Results

Every expected result was met. Times are from the command, or the tap, to the result.

| # | Result |
| --- | --- |
| 2 | Published in 2 s |
| 3 | Answered "waiting for the creator to look" in 1 s. Kim's requests gave the reason "you don't follow them; they follow you", and `osp status` and the app's menu badge counted 1 |
| 4 | From Alice's node, 0.6 s; from Jimmy's, 0.6 s. Kim's node logged nothing about the reads |
| 5 | With Kim's node stopped, `osp reply` waited 90 s for the first try, then said "not answered yet; your node tries again, in 59s". Kim's node was back 1 min 40 s after the reply. The two tries that followed failed: Tor couldn't reach Kim's node for about 15 minutes after its restart (Tor's guards were reporting failing circuits, and Kim's own syncs failed too), so the next try was set for 30 minutes later. `osp sync` on Jimmy's node tried at once, 14 minutes after the reply, and it arrived once, among Kim's requests |
| 6 | Published at the tap. Jimmy's next read of the thread showed it, and his node then listed it as published |
| 7 | Published at once; the thread showed it after the reload, below the keyboard |
| 8 | Kim's node had Jimmy's `Delete` 1.5 s after the command, and readers no longer got the reply. Jimmy's node forgot it once Kim's had the `Delete` |
| 9 | "Publish and allow" at the tap; Jimmy's next reply was published in 2 s |
| 10 | As expected. Jimmy's node still lists the hidden reply as "waiting for the creator to look": it learns a reply is published only by seeing it in the thread |
| 11 | Jimmy's reply was answered "the post takes no replies". Readers saw "Who may reply: nobody (off)" |
| 12 | Kim's node deleted the post 60 s after the tap, and Alice's node had dropped it within 4 s more. A reader's node that has dropped the post can't ask for its thread (it doesn't know the post), so on real nodes the post's own `Delete` reaches readers before the thread's answer would. The thread's answer carrying the `Delete` is checked in `tests/replies.rs` |

Not run on real nodes, and left to the automated tests: sending the same reply twice, a deleted reply sent again, the limits under a flood, and checking that a reader's thread read sends nothing from its public endpoints.

### Found on the way

- **Fixed:** the requests screen's four buttons didn't fit on one row of a phone, so they stacked in a column. They now wrap onto two rows, as the post card's buttons do.
- **Fixed: retries over a bad Tor stretch.** A creator's node that came back while Tor couldn't reach it cost the replier the 1- and 5-minute tries, and the next was 30 minutes away. Now, once a sync with the creator's node works again, the replier's node sends the replies waiting for them at once.
- **Fixed: replies to a deleted post.** The replier's node kept listing them as published or waiting after the post's `Delete` arrived. They now stand as "the post was deleted", with nothing more to try.
- **`osp reply` on the node itself** waits for the first try's answer, up to 2 minutes. The app waits at most 10 s, then shows the reply as being sent.
- One phone's clock was an hour ahead, so every time it showed was an hour out. Not the app's doing.
