# ADR 0042: Content rules, reports and the project's hide list

**Status:** Accepted (2026-11-17), the maintainer's choices on scope, reports and the hide list, and the rest as drafted with them. Part of P11 ([ADR 0030](0030-product-alpha-before-testers.md)), needed before outside testers on either phone. Answers [ADR 0019](0019-taking-posts-and-media-down.md)'s open question of who publishes the first list. Step 1 built (2026-11-17): the terms before pairing and Report. Step 2 built (2026-11-17): `osp block` and Block in the app; unblocking a person puts back their profile as it was, so they can be followed again. Step 3's node side built (2026-11-17): the list's format, `osp-list` to sign it, the node's daily fetch over Tor from `https://osp2p.com/lists/hide.json`, applying it on the node and in a directory's lists, and `osp hide-list on|off` ([how to check it](../testing/tor.md#the-hide-list)). The project's key is built in (2026-11-17), and the first list, signed and empty, is on the site. Amended (2026-11-17), by the maintainer: the list is asked about at setup, not on by default (§6), and the owner can make exceptions by person. Built on the node: the question in `osp init`, exceptions (`osp hide-list except`), listed replies left out of threads, and the signed list passed to phones. Next: the app's question, Advanced, and its own check.

## Context

The iPhone app goes through Apple's review before anyone outside the project can install it from TestFlight. Apple's rules for apps that show what their users post ask for a reporting mechanism with timely responses, a way to block people, a way to filter objectionable material, and published contact details.

OSP has none of these for posts today. The app can block someone only from the replies to your own posts ([ADR 0028](0028-replies-let-in-by-the-creator.md)). ADR 0019 designed the rest and built only its first step, deleting your own posts:

- `osp block <post, file or account>` (its step 2) isn't built;
- it said anyone may publish a signed list of files that nodes subscribe to by choice, and left open who publishes the first one.

Answering App Store Connect's questions made one rule explicit: explicit sexual material isn't permitted content. It is a rule, not "happens rarely".

## Decision

### 1. One set of rules for all official OSP software

The rules apply the same way to the official Android and iPhone apps, the node software the project releases, and any directory or other service the project runs. There is no cleaner view for one app store while the other shows something else.

They aren't a rule for the whole network. A node someone runs from changed software, or with the list turned off, stays under its owner's control, as ADR 0019 says. Only the author can delete a post across the network.

### 2. What the project doesn't permit

Narrow on purpose:

- explicit sexual material: pornography, and explicit sexual acts or genitals shown in full;
- prolonged graphic or sadistic realistic violence;
- any sexual material involving minors;
- intimate images shared without the consent of the person in them;
- credible threats of violence against a person, and material made to incite it.

A report alone never puts anything on the list. Ordinary speech, however controversial, isn't covered, and the list isn't for disagreements.

### 3. Terms of use, agreed at first start

The first time the app starts, before pairing, it shows short terms of use, and the person agrees before going on:

- post only what you made, or have the right to share;
- you answer for what you post. Posts are public, other nodes keep copies, and a delete reaches the nodes that cooperate, not copies someone saved;
- §2's rules, and that reports are reviewed;
- your node is yours: there's no account with the project, and the project can't change what's on your node.

The full text is at `https://osp2p.com/terms`, and the README and the install guide carry the same words. The app keeps which version was agreed to; a changed version is shown again, once.

### 4. Reports go through the project's site, not the network

Each post and profile in the app has **Report**. It opens `https://osp2p.com/report` in the browser with the post's or account's ID filled in. The form asks which rule it breaks, with an optional note and an optional email for an answer.

- Reports never travel through the social protocol, so a report can't be used to probe someone's node, and it works when the reporter's own node is down.
- The site's report page, and the README, give the project's published contact address.
- The reporter's node ID isn't asked for or sent. The site's host sees the reporter's IP address, as for any web page.
- The maintainer reviews each report, aiming to act within 24 hours.

### 5. Four tools, kept apart

| Tool | Who | What it does |
| --- | --- | --- |
| **Delete** | The author | Erases their own signed post on every node that cooperates (ADR 0019) |
| **Block** | Each owner | Their node won't fetch, serve or show a post, file or account. Nothing is sent, and it can be undone |
| **The project's hide list** | The project | Official software, set up as released, won't fetch, serve or show what's on it |
| **Delisting** | Project-run directories | They stop listing an account or post, so nobody finds it through them |

### 6. The project's hide list

The project publishes the first list, in ADR 0019's form: signed, each entry dated and retractable, and each entry a hash of the hash, so the list doesn't double as a catalogue. It has two kinds of entries:

- **a file,** by its hash: any post naming that file;
- **a post,** by its ID, for a post without media or one whose media was encoded differently.

The project's nodes and directories apply it too: a directory drops a listed post from its listings, and delists an account that exists mainly to post what §2 doesn't permit.

**Asked at setup.** Official software asks the owner whether to use the list when the node is set up: `osp init`, and the app's first start, after the terms and before pairing. "Use the list" is the highlighted answer, "Not now" the other. Until the owner answers, the list is off, on new nodes and on nodes that were running before this was built; the app asks at its next start. *(Amended 2026-11-17; it said "official builds subscribe by default".)*

With the list on, the node fetches it once a day over Tor, so fetching tells the project nothing about the node. It then:

- refuses to fetch or serve a listed file, and drops its copies;
- leaves listed posts out of Home, the Universe, search and Activity, and listed replies out of threads, both those it reads and those it serves;
- passes the signed list to its paired phones.

The official apps check the list's signature against the project's key, which is built into them, and hide listed posts too. The owner can turn the list off on their own node under Advanced. The app then says, under Advanced, that the project's list isn't applied.

**Exceptions by person.** The owner can tell their node not to hide one person's posts: from that person's profile ("Don't hide their posts"), or `osp hide-list except`. The list then leaves that person's posts, and reshares of them, in view on the owner's node and phones, and their listed files may be fetched for the owner to see. The node still never passes a listed file on to anyone, whoever posted it. Advanced lists the people excepted, to take an exception back. The owner's own posts are never hidden, so they need no exception.

**The key.** The list is signed by a project key kept off every node and off the release machines. It isn't the maintainer's own identity. A new key comes with a new release.

### 7. Appeals and removal

Anyone, the author included, can ask through the same form for an entry to be looked at again. An entry that doesn't meet §2 is retracted, and nodes show the post again after their next daily fetch. Retractions stay in the list, so a node that missed one learns it.

## Options considered

- **Delisting and blocking only.** Each owner can protect themselves, but nobody acts on a report beyond directories, and a newcomer's first Home can show anything. Not enough for an official alpha.
- **An Apple-only filter.** Two products with two sets of rules, and Android testers see what reviewers never did. Rejected (§1).
- **Reports through the network,** a message from the reporter's node to a project node. It keeps everything in OSP, but adds a request strangers can send, needs the reporter's node up, and gives no published contact. Rejected (§4).
- **A broad moderation list,** covering whatever is reported as offensive. The project would be judging speech for every official node. Rejected (§2).
- **This decision.** The mechanism ADR 0019 already chose, with the project as its first publisher, a narrow list and a published path for reports.

## Consequences

**People.** What official software shows follows one published set of rules. An author whose post is listed keeps it on their own node and can appeal. Someone who disagrees with the list can turn it off on their node.

**Privacy.** Fetching the list over Tor reveals nothing about the node. The list reveals what the project hides, only to someone who already has the file or post. A report reveals the reporter's IP address to the site's host and, if given, their email to the maintainer; nothing about it reaches the reported person.

**Security.** A list not signed by the project's key is refused. A stolen key could hide posts, never change or delete them; a new release replaces the key. Someone can report falsely, but a report alone never lists anything (§2).

**Operational.** One maintainer reads reports, so the 24-hour aim holds while reports are few. The list is small and fetched once a day. The site needs a form and somewhere to keep reports, beyond its static pages.

**Changes to accepted records.**

- ADR 0019's open question (step 3 of "What each owner decides") is answered: the project publishes the first list, and official software asks each owner at setup whether to use it.
- ADR 0019's position, "no central service ... the software reports nothing to the project": official software now fetches one list from the project, and the project runs a report form. The software still sends the project nothing about anyone, and the maintainer still can't change or remove anything on anyone's node.
- ADR 0019's list gains post entries alongside files.
- ADR 0019's `osp block` is built here, for accounts as well as posts and files.

**Licensing.** No new dependency expected.

## Building it

| Step | Work | Gate |
| --- | --- | --- |
| 1 | The terms at first start (§3), on the site, in the README and the install guide; **Report** in the app; the report form and its contact on the site | The terms are agreed before pairing, and again once after they change; a report from the app reaches the maintainer with the right ID |
| 2 | Block: `osp block` and the app, for an account, a post or a file | A blocked account's posts aren't fetched, served or shown, and no message leaves the node, after a restart too |
| 3 | The hide list: format, signing tool, the node's daily fetch over Tor, applying it on the node, in directories and in the app, the Advanced switch | A listed file isn't fetched or served and its post isn't shown; a retraction brings it back; a list signed by another key is refused; the list holds no plain hash; turning it off works |

All three before external TestFlight testing or a release to testers.

## Revisit when

- reports outgrow one person's day;
- someone else offers to publish a list people want;
- re-encoding to dodge file entries becomes common;
- a community's moderators ([ADR 0041](0041-public-communities-and-stewards.md)) need lists of their own.
