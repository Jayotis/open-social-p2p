# ADR 0014: Finding people and content, with messages between nodes over Tor

**Status:** Accepted (2026-11-01). Decides [ADR 0007](0007-transport-privacy-defaults.md)'s phases 2 and 3. Changed before building (2026-11-01): one stream between a pair of nodes, used both ways once the node that dialled proves its key, instead of one dialled by each ("Tor", below).

## Context

Today people find each other only through follow codes, and a post reaches someone only through whom they follow and what those people reshare. Milestone M4 lets people find others and their public posts ([roadmap](../06-roadmap.md)), and this record answers [doc 07](../07-contributing-and-open-decisions.md)'s open decision 5: who provides discovery, and what it reveals.

Two things make it more than a search box.

**No single service should hold the index, see every search and decide the ranking.** Those are three powers, and a search engine holds all three. They can be split: the node searches what it already has at home; directories, which anyone can run, return matches; and the node orders the results itself.

**Once strangers can find a node, any of them can connect to it and learn its internet address.** Today a node connects directly unless its owner turns on relay-only mode ([ADR 0007](0007-transport-privacy-defaults.md) phase 1), which was fine while only people holding a follow code could reach it. ADR 0007 planned to separate messages from media (phase 2) and to try Tor for messages (phase 3). M4 needs both first.

Questions to settle:

1. How are people and posts found, and by whom?
2. How do messages between nodes avoid revealing addresses, and how is Tor run?
3. Who may connect to a node directly, for media?
4. Which of M3's timers change for messages that take seconds?
5. What does the node index, and how are directories run and asked?
6. How does someone know a profile is the person it names?

## Evidence: M4's first tests (2026-10-31)

**Tor between two nodes.** Two home nodes on different networks, one behind a VPN, each with its own Tor 0.4.9 (started as the user, with a password cookie on its control port), talked through iroh 1.4.1 and `iroh-tor-transport` 0.1.0 only.

- It works with nothing but the other node's ID: the onion address comes from it. Tor started from nothing in 10 s.
- **A first connection took 11 to 24 s** (14 s in the middle, eight tries, all on the first attempt). Each direction sets up its own circuit, to the other node's onion service, so both nodes' services must be up.
- **Round trips** of a small request were 1 to 1.5 s in the middle; on one connection over 30 minutes, mostly 1 to 3.5 s, with spikes up to 19 s. A new connection to a node already reached took about 1 s.
- **Bulk speed depends on the circuit:** 1 MB took 3.4 s on a good circuit and 150 s on a poor one. Fine for messages, not for media.
- **Nothing went direct:** the nodes had no UDP socket and only local TCP connections to their Tor, and Tor connected only to Tor relays.
- **A Tor restart ends the node's onion service** for good: the transport doesn't set it up again. After the node set it up again, the other node got through 1 to 1.5 minutes later.
- **The transport sends each packet from a task of its own,** so packets can leave out of order. A copy sending each peer's packets in order was faster in 14 of 15 paired comparisons, by about a quarter to two fifths.
- **Its control-port library (`torut`) brings two crates with security advisories** (curve25519-dalek 3.2, ed25519-dalek 1.0), so our `cargo audit` would fail. It's used only to talk to Tor's control port and for the onion key's format.

**Search on the node.** SQLite's FTS5 is in our build (SQLite 3.54). On a copy of a real node's posts, words, prefixes and phrases match, accents fold (`cafe` finds "Café"), hashtags match without the `#`, and a deleted row drops out of results at once. Chinese and Japanese text isn't split into words: a whole sentence matches, a word inside it doesn't. With a million made-up posts (182 MB of text), the index took 112 MB and 7 s to build, and the 50 best matches for a common word came in 10 to 74 ms.

**Messages over Tor in `osp-sim`** ([results](../testing/network-conditions.md#messages-over-tor)). B8's swarm run with Tor-like delays on messages, from the figures above, met every mark as before: the swarm barely notices. But 139 syncs failed because a first connection may take 25 s and the node gives up at 20 s; 4 of 300 followers weren't told of the post within 15 s; and a node following and fetching at once took about 30 s longer, for the first connection. The 3 s the node waits for each node it follows when the phone opens media isn't reached by B8; it's shorter than any first connection over Tor.

## Options considered

**Finding.**

- **One index service** (as AT Protocol's app views): simple, and the "Google problem" again.
- **Asking the nodes you follow** (as Secure Scuttlebutt replicates friends of friends): private, but with a handful of users nobody follows enough people for it to find anything. Deferred.
- **Keyword search over a DHT** (as Kad in eMule): its keyword search was flooded with spam and poisoned results, while its exact lookups worked. Rejected for keywords.
- **The node's own index, plus directories anyone can run, plus exact lookups.** eD2K's many servers were directories; fake servers appeared, which signatures fix, and spying servers, which hiding who searched fixes.

**Private messages.**

- **A relay-only endpoint for messages:** works today (phase 1), but the relay operator sees who talks to whom, and each relayed byte costs them.
- **Tor's onion services:** no relay of ours, and nobody on the path sees both ends. Slower, and Tor is someone else's network, run by volunteers. The test showed it's quick enough for messages.
- **A forwarding network of our own:** needs people to run it, and there aren't any yet. Later.

**Running Tor.** The system's Tor service needs an administrator to install and set it up, and a group for its control port. Arti (Tor in Rust, embeddable) needs no separate program, but how far its onion-service hosting has come isn't checked. The node starting its own Tor from the distribution's package needs neither.

**Keys.** One key for every endpoint would let anyone with the public ID connect through a relay and punch through to the node's address. So the endpoints that reveal different things need different keys.

## Decision

### Three ways of finding

1. **The node's own index:** what it has already received, searched at home, so nothing leaves the house.
2. **Directories:** opt-in nodes that index the public posts and profiles of nodes that ask to be listed, for strangers and topics.
3. **Exact lookups:** a domain, or an identity key, to its node.

Not in M4: asking the nodes you follow, keyword search over a DHT, doc 08's ranking, trending signals.

### Three endpoints, each with its own key

| Endpoint | Reached through | Its ID goes to | Carries |
| --- | --- | --- | --- |
| **Public** | Tor only | the signed profile, follow codes, directories, everyone | messages between nodes (`osp/social`) |
| **Media** | relays only, no IP sockets | anyone the node serves media to | media (iroh-blobs) |
| **Direct** | IP and relays | the node's own phones, and peers the rule below allows | media; the phones' protocol |

- **The public endpoint keeps the node's current key,** since that ID is already in signed profiles, follow codes and followers' tables. The media and direct endpoints get new keys. Phones, which can't run Tor, get the direct endpoint in the pairing ticket, so phones paired before must pair again once.
- **Nothing about the public endpoint is published** outside Tor: no DNS or Mainline record. It has no address to give away.
- **The media endpoint is relay-only mode** from phase 1, for one endpoint: it has nothing to punch through to, so handing out its ID reveals only which relay it uses. Media for peers not allowed a direct connection goes through it, on n0's public relays for now.
- **Fail closed.** The direct and media endpoints don't serve messages between nodes, so if Tor is down, messages wait; they never go direct.

**How the endpoints are tied to the person.** The identity signs its profile, and the profile lists its public node IDs ([ADR 0010](0010-signing-keys-and-revocation.md)). The public endpoint's key signs a short **binding** for each of its other endpoints: this media (or direct) endpoint serves for this public node, from this date. A node passes the binding with the endpoint's ID, so whoever receives it, first-hand or from a third node, can check it as far as the identity. Media is checked by its hash anyway, so a wrong endpoint can't serve wrong content; the binding stops one node passing off another's endpoint as its own, and lets each endpoint's measurements ([ADR 0011](0011-measuring-peers.md)) and trust ([ADR 0013](0013-trusted-followers.md)) be counted against the right node.

- The follows and followers tables keep the public ID as the node's name, with its bindings next to it. `WhoHosts` names followers by their public ID with a media (or direct) binding; `Hosting`, the swarm list and peer records follow suit.
- **Handing out the direct endpoint isn't enough to protect it:** a peer that's allowed could pass its ID on. So the direct endpoint itself refuses any node that isn't its own phone or allowed by the rule below, and it must do so before revealing an address. Whether iroh reveals any address before a connection can be refused is to be tested before building on it; if it does, the direct endpoint is given only to the phones until that's solved.

### Who may connect directly

Each node sets a rule for direct connections: anyone, people I follow, contacts I approved, or nobody. A connection is direct only when **both** sides' rules allow it; otherwise media goes through the relays.

- A node hands its direct endpoint only to peers its rule allows, when they ask over Tor. Its own side of the rule decides which of its own endpoints it dials from, so a node that allows nobody fetches from its media endpoint and its address stays hidden even from a node that allows everyone.
- Following someone, or knowing them a long time, never grants it on its own. An address once given can't be taken back.
- Followers in the swarm are named to other followers in `WhoHosts` by their media endpoint, or their direct endpoint if their rule allows anyone. That is the same disclosure as today, now chosen.
- **The default rule is decided by measuring** (below), before the invited alpha.

### Tor

- **The node starts and watches its own Tor** from the distribution's `tor` package: its own data folder in the node's home, local ports, and a password cookie on the control port. If Tor stops, the node starts it again and sets up its onion service again, and says so in `osp status`. Linux first.
- **We carry our own copy of `iroh-tor-transport`** in the workspace ([`osp-tor`](../../crates/osp-tor/)), with these changes, and offer them to its makers: **one stream per pair of nodes, used both ways**, on which the node that dialled first proves its key by signing a challenge from the other; packets to each peer sent in order; the onion service set up again after Tor restarts, with reconnects spaced out and the state shown; a small control-port client of our own instead of `torut`, so the advisories don't come with it; and limits on streams and dials. It keeps upstream's licences, MIT or Apache-2.0, so the changes can go back to it.
- **Why one stream, proved:** upstream's frames name their sender without proof, and each node sends on a stream it dialled itself, to the other's onion service. So a pair holds two circuits, a first contact waits for both onion services, and a peer can make a node dial any onion address by naming it. The node that dialled already knows whom it reached, as Tor checks the onion address; once it has proved its key, the other node answers on the same stream. A pair then holds one circuit, a first contact needs only the other node's onion service, a node never dials anyone because a packet named them, and a node whose own Tor restarted is answered again as soon as it dials out, without waiting a minute or so for its onion service to be found again. It's a wire format of our own, named by the stream's first bytes.
- **Recovering from a Tor restart is a requirement,** not polish: a home node runs unattended, and one that stops talking until someone restarts it isn't ready for the alpha.
- iroh's custom transports are outside its semver guarantees, so an iroh update may need changes here, as with `osp-sim`'s network.
- Arti stays the likely way to package Tor later, once its onion-service hosting is checked.

### Waiting for messages that take seconds

M3's timers were set for direct connections. Raising them isn't the whole answer: what changes is how the node waits. Work falls into two kinds.

- **Background work:** syncing, telling followers of a post, `Hosting` checks, telling a node we hold a copy, directory refreshes. It's queued and never holds up a post or the app; it reuses open connections, and retries are spaced out. A first connection is given up to 60 s instead of 20 (the test's took up to 24 s), and one given up on still readies the circuits for the next try. Telling followers of a post gives each up to 60 s, a few at once; one missed still gets the post at its next sync.
- **Someone waiting:** following, searching, opening media on the phone. The app shows that the node is connecting through Tor, the person can cancel, and each has an overall limit. Searches show answers from the directories that reply, as they reply.
- **Opening media the node doesn't have** asks all the nodes it follows at once, not one after another, and waits only for those it already has a connection to, a round trip or two (`ASK_FIRST_WAIT` from 3 s to about 5 s). A first connection isn't waited for; it's readied for next time.
- The request timeout (30 s) and a follow sync's limit (120 s) stay; neither was reached.
- **The measure is the whole journey:** in `osp-sim`, how long from posting until followers hold the video, with the Tor-like delays, as B8 already counts, not whether a timer was raised enough.

### The node's own index

- FTS5 tables, **contentless with `contentless_delete=1`**: the text stays only in the signed objects, so results and snippets are read from those. Tokenizer `unicode61` with accents folded; `trigram` later for Chinese and Japanese.
- **A mapping table gives each searchable thing a stable integer** (`INTEGER PRIMARY KEY`), the FTS rowid: a post by its object ID, a profile by its identity, so a newer profile takes the old one's place. The `objects` table's own rowid won't do: its key is text, so `VACUUM` may renumber it.
- Filled when a post or profile is stored, in the same transaction; removed when its object is dropped (a revocation, ADR 0010). It's derived data: rebuilt from the objects after a restore, or if missing.
- Results in plain order: text relevance (BM25), then newest. Searching works offline.

### Directories

- **A directory is an opt-in role** any node can take on, like doc 13's preservation node. The maintainer runs the first one, so the first users find someone; anyone can run another.
- **Being listed is opt-in.** A node asks a directory to list it. The directory then follows it as a follower does, through the outbox it already serves, and indexes its profile and public posts in its own FTS5 tables. Revocations reach it the same way. A node that asks to be dropped is dropped, with what was indexed of it. Directories cap new listings per hour and new posts per listed node per day, as nodes already cap followers and posts.
- **A search** is one request with the words, answered with at most 20 matching posts and profiles **and at most 64 kB**, since bulk speed over Tor varies so much. Each result is the author's signed object as stored, so nothing a directory writes needs trusting; a reply that would pass the size limit stops early and says so. The asking node checks every signature, so a directory can leave things out but can't fake them. It asks two or three directories, merges the answers, drops duplicates, orders them itself, and shows which directories each came from.
- **Results can be stale.** Each says when the directory last synced its author. A signature proves who wrote something, not that it's still current: before following, the node fetches the profile from the author's own node, and a post the author has since dropped isn't shown from a directory's copy once the node knows.
- **Searches go over Tor and aren't made with the node's public identity:** they come from a key of their own, replaced now and then. It needs no onion service, as a directory answers on the stream the search came on. A directory still sees the words, their timing, and the same key for as long as it lasts, so it may link searches made with one key; replacing keys limits that, it doesn't prevent it.
- **Finding directories:** a list of directory IDs comes with the node; later, directories can name others.

### Exact lookups and verified domains

- **By domain.** A creator can name a website in their signed profile. The website serves `https://<domain>/.well-known/osp.json` naming the identity key and its public node IDs. Typing the domain finds the node with no follow code and no search.
- **The node checks it,** fetching the file over Tor and caching it, and the phone never fetches it. If it matches, the app shows "verified: example.com"; whether that's the right website is for people to judge. Tor resolves addresses but not TXT records, so it's a file, not a DNS record. A site that blocks Tor gets "couldn't check", not "not verified".
- **Rechecked now and then.** "No longer verified" when the file goes. If the domain starts naming a different key, followers are warned, not moved.
- **Optional.** A profile without a domain works as before, marked as not verified.
- **By identity key:** a directory returns the profile listing that identity's nodes. A lookup through Mainline's DHT would have to go over Tor, which carries no UDP; later.

## Consequences

**Privacy.**

- Strangers learn a node's public ID, its profile and its public posts, never its address, unless both rules allow a direct connection.
- Directories see the words searched, when, and the searching key, not the node's public identity. They may link searches made with one key. A directory returning at most 20 has chosen which 20: that's why a node asks more than one.
- The relays carrying media from nodes that hide their address see both ends of those transfers, as in ADR 0007.
- Tor hides addresses; it doesn't make anyone anonymous. Someone watching both ends' traffic can still match it up. Nothing may describe this as anonymous.

**Operational.**

- Every node depends on Tor for messages. Tor restarting takes messages away for a minute or two.
- Each pair of nodes talking uses one circuit, from whichever dialled. How many stay open at once, and what they cost a node with hundreds of followers, isn't known: to measure before a larger alpha.
- Messages take seconds instead of milliseconds. Background sync hides most of it; a first contact takes up to half a minute.
- The maintainer runs the first directory, a stand-in until others do.
- **Media from nodes that hide their address still costs someone bandwidth.** Tor solves private first contact, not the cost of sending video privately. n0's public relays carry it for now; relays and forwarding run by the community are an objective of their own after M4, and the media path stays replaceable meanwhile.

**Security.** A node dials only nodes it means to reach, and nodes that proved their key on a stream that has since closed: a packet naming someone can't make it dial them. A domain's server sees that someone fetched its file over Tor, not who.

**Licensing.** Tor (BSD) runs as a separate program. The copied transport is MIT/Apache-2.0, compatible with the project's AGPL-3.0.

## Building it

In order, each step passing its gate before the next starts. Local search doesn't depend on the others and can be built alongside.

| Step | Work | Gate |
| --- | --- | --- |
| 1 | Our copy of the Tor transport, with its fixes; the node starting and watching its own Tor | Survives Tor restarts on either side and reconnects on its own; nothing goes direct |
| 2 | The three endpoints and their bindings; the direct endpoint refusing nodes not allowed | Tests that bindings are checked and refused nodes learn no address |
| 3 | Messages, `WhoHosts`, `Hosting`, the swarm, trusted followers and peer records on the new endpoints; the waiting above | All their tests pass; B8 in `osp-sim` with the Tor-like delays meets its marks |
| 3b | The node's own index and a search screen | Finds posts and profiles offline; revoked posts drop out; rebuilt after a restore |
| 4 | One directory, the maintainer's | A node with no follows finds a stranger and follows them |
| 5 | More directories, lookup by key and by domain | One directory off, one tampering, one returning stale results, a domain naming a new key: each handled |
| 6 | The invited alpha | Below |

## Validation

- **Leak test,** like phase 1's: no stranger learns the node's address through its profile, follow code, lookups, `WhoHosts` replies, directory listings or logs. A packet capture shows messages going only to Tor.
- **`osp-sim`:** B8's swarm run passes its marks with Tor-like delays on messages, with the changed timers.
- **Measuring media's default:** B8's swarm run with 0, 20 and 50% of followers fetching through relays, counting the bytes relayed and the creator's copies. It needs a simulated relay first.
- **Search:** posts and profiles are found offline; revoked posts drop out.
- **Directories:** a node with no follows finds a stranger and follows them; with two directories and one switched off, search still works; forged or stale records are rejected.
- **The invited alpha:** a few people, on their own Linux machines, install the node with Tor, find each other through the directory, follow and watch, without swapping follow codes or showing strangers their address.

**Revisit** if Tor becomes too slow or unreliable for messages, once arti can host onion services, once there are enough users to run a forwarding network of our own, or if a directory's view of searches proves too much.
