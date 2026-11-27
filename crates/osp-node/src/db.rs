//! Node metadata in SQLite: objects, outbox, follows, blob classes and known hosts.
//!
//! Blob contents live in the iroh-blobs store (see `store.rs`); this database
//! only records which blobs we have and why.

use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use iroh::{EndpointAddr, EndpointId, PublicKey};
use osp_proto::{
    Announce, BlobHash, BoundEndpoint, Delete, EarlierKeys, EndpointBinding, EndpointKind, Key,
    Note, ObjectId, Payload, Person, SignedObject,
    machine::{ListingEntry, ListingItem, MAX_LISTING_PAGE_BYTES, MachineRecord},
    rpc::{FollowInfo, PeerCounts, StorageInfo, Upload, UploadId},
    universe::{EVENT_HOURS, Event, EventWhat},
};
use rusqlite::{Connection, OptionalExtension, params};
use tracing::info;

use crate::{events, meet, peers, trust};

use opened::NOT_OPENED;

mod activity;
mod blocks;
pub(crate) mod carriers;
mod hide;
mod likes;
mod opened;
mod profiles;
mod replies;
mod reply_backup;
mod sent_replies;
mod universe;
mod vouches;
pub(crate) use carriers::{DAY_MS, Probationer, SUGGESTION_DAYS};
pub use hide::Applied as HideApplied;
pub(crate) use likes::{HeldLike, SentLike};
pub(crate) use replies::{Choice, Held, ReceivedReply, ReplyRow};
pub(crate) use reply_backup::ReplyTables;
pub(crate) use sent_replies::SentReply;
pub(crate) use vouches::{GivenVouch, HeldVouch, NewVouch, SideVouch, VouchTables};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS objects (
    id           TEXT PRIMARY KEY,
    actor        TEXT NOT NULL,
    kind         TEXT NOT NULL,
    published_ms INTEGER NOT NULL,
    received_ms  INTEGER NOT NULL,
    signed       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS objects_actor ON objects(actor, kind);

-- Our own identity's published objects, in order.
CREATE TABLE IF NOT EXISTS outbox (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id  TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS follows (
    actor      TEXT PRIMARY KEY,
    node_addr  TEXT NOT NULL,
    cursor     INTEGER NOT NULL DEFAULT 0,
    last_sync  INTEGER,
    last_error TEXT
);

CREATE TABLE IF NOT EXISTS followers (
    node      TEXT PRIMARY KEY,
    node_addr TEXT NOT NULL,
    since     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS profiles (
    actor  TEXT PRIMARY KEY,
    name   TEXT NOT NULL,
    signed TEXT NOT NULL
);

-- Why we keep each blob: own > replica > prefetch > cache (ADR 0003).
CREATE TABLE IF NOT EXISTS blobs (
    hash        TEXT PRIMARY KEY,
    class       TEXT NOT NULL,
    size        INTEGER NOT NULL,
    added_ms    INTEGER NOT NULL,
    accessed_ms INTEGER NOT NULL
);

-- Nodes believed to serve a blob, learned from authors, hosting reshares and WhoHosts.
CREATE TABLE IF NOT EXISTS hosts (
    blob      TEXT NOT NULL,
    node      TEXT NOT NULL,
    node_addr TEXT NOT NULL,
    seen_ms   INTEGER NOT NULL,
    PRIMARY KEY (blob, node)
);

-- Paired phones (milestone M2).
CREATE TABLE IF NOT EXISTS devices (
    node      TEXT PRIMARY KEY,
    name      TEXT,
    paired_ms INTEGER NOT NULL
);
";

/// A paired device: its ID, name, when it was paired (RFC 3339) and whether
/// the review code paired it.
pub type DeviceRow = (EndpointId, Option<String>, String, bool);

/// Changes to the schema above, applied in order to every database, old or
/// new. SQLite's `user_version` records how many have run. Only ever append.
const MIGRATIONS: &[&str] = &[
    // 1: when each follower last answered or subscribed, so long-silent ones can be dropped.
    "ALTER TABLE followers ADD COLUMN seen_ms INTEGER",
    // 2: the size stored posts declare for each blob, so a download the
    // phone asks for can be stopped once it passes that. Filled in from
    // the posts already stored by `migrate`.
    "CREATE TABLE declared_sizes (blob TEXT PRIMARY KEY, size INTEGER NOT NULL)",
    // 3: followers still to be told that this identity moved to this node (ADR 0009).
    "ALTER TABLE followers ADD COLUMN tell_moved INTEGER NOT NULL DEFAULT 0",
    // 4: what we know of each identity's signing keys (ADR 0010): its newest
    // key certificate, and the date of the newest one that revoked earlier keys.
    "CREATE TABLE identity_keys (
        actor             TEXT PRIMARY KEY,
        cert              TEXT NOT NULL,
        cert_ms           INTEGER NOT NULL,
        revoked_before_ms INTEGER
    )",
    // 5: objects of followed identities dropped because a key that signed
    // them was revoked, to ask their node for again (ADR 0010), with when
    // each first arrived.
    "CREATE TABLE refetch (
        id          TEXT PRIMARY KEY,
        actor       TEXT NOT NULL,
        received_ms INTEGER NOT NULL
    )",
    // 6: what this node measured about each peer node (ADR 0011): whether it
    // ever contacted the peer itself, first and last contact, and the faded
    // counts as JSON (`PeerCounts`), so a measure can be added without a
    // migration. Counts were last faded at the last contact.
    "CREATE TABLE peers (
        node     TEXT PRIMARY KEY,
        ours     INTEGER NOT NULL,
        first_ms INTEGER NOT NULL,
        last_ms  INTEGER NOT NULL,
        counts   TEXT NOT NULL
    );
    CREATE INDEX peers_by_contact ON peers(ours, last_ms);",
    // 7: the follower swarm (doc 13): whether each follower serves what it
    // fetches ahead of time to anyone, and which followers told us they
    // hold a copy of one of our blobs, last when.
    "ALTER TABLE followers ADD COLUMN shares INTEGER NOT NULL DEFAULT 0;
    CREATE TABLE swarm (
        blob    TEXT NOT NULL,
        node    TEXT NOT NULL,
        seen_ms INTEGER NOT NULL,
        PRIMARY KEY (blob, node)
    );",
    // 8: where each object sits in the feed: when it was published, except
    // for a followed author's post held back while its media is on its way
    // (`insert_object_held`), which takes its place when it's shown.
    "ALTER TABLE objects ADD COLUMN feed_ms INTEGER NOT NULL DEFAULT 0;
    UPDATE objects SET feed_ms = published_ms;",
    // 9: who posted each blob, so a download can leave the author's own
    // nodes until last when it fetches from several at once (doc 13).
    // Filled in from the posts already stored by `migrate`.
    "ALTER TABLE declared_sizes ADD COLUMN author TEXT",
    // 10: uploads from devices that can be resumed after a broken
    // connection (ADR 0008): the post's text and files (JSON `Upload`s),
    // how far it got, and the post ID or the reason it failed.
    "CREATE TABLE uploads (
        device     TEXT NOT NULL,
        id         TEXT NOT NULL,
        content    TEXT NOT NULL,
        files      TEXT NOT NULL,
        state      TEXT NOT NULL,
        outcome    TEXT,
        updated_ms INTEGER NOT NULL,
        PRIMARY KEY (device, id)
    )",
    // 11: trusted followers (ADR 0013): each node that asked to be trusted
    // or was trusted, the identity and name of the profile that listed it,
    // the note it sent, whether it is waiting, allowed or declined, and
    // when it asked (never, if trusted without a request) and was answered.
    "CREATE TABLE trust (
        node        TEXT PRIMARY KEY,
        actor       TEXT NOT NULL,
        name        TEXT NOT NULL,
        note        TEXT NOT NULL,
        state       TEXT NOT NULL,
        asked_ms    INTEGER,
        answered_ms INTEGER
    );
    CREATE INDEX trust_by_actor ON trust(actor);",
    // Other nodes' media and direct endpoints: which node each serves for,
    // from when, and the binding and address as they came (ADR 0014). And the
    // nodes allowed a direct connection by the rule `approved`.
    "CREATE TABLE endpoints (
        endpoint TEXT PRIMARY KEY,
        public   TEXT NOT NULL,
        kind     TEXT NOT NULL,
        since_ms INTEGER NOT NULL,
        bound    TEXT NOT NULL
    );
    CREATE INDEX endpoints_by_public ON endpoints(public, kind);
    CREATE TABLE direct_contacts (
        node     TEXT PRIMARY KEY,
        added_ms INTEGER NOT NULL
    );",
    // 13: the endpoint a follower said it may be named by to other nodes, as
    // a host of our media, and how to reach it (ADR 0014).
    "ALTER TABLE followers ADD COLUMN host TEXT",
    // 14: a directory's lists (ADR 0015). `listed`: who asked to be listed,
    // the node read for them, where its outbox had got to, and when they asked
    // and it was last tried and answered. `map`: machines that answered when
    // greeted back, and when they last greeted and were greeted. `listing`:
    // what readers are given, one row per person or machine as JSON
    // (`ListingItem`), which takes a new number whenever it changes.
    "CREATE TABLE listed (
        actor    TEXT PRIMARY KEY,
        node     TEXT NOT NULL,
        latest   INTEGER NOT NULL DEFAULT 0,
        asked_ms INTEGER NOT NULL,
        tried_ms INTEGER NOT NULL DEFAULT 0,
        read_ms  INTEGER
    );
    CREATE TABLE map (
        machine    TEXT PRIMARY KEY,
        heard_ms   INTEGER NOT NULL,
        greeted_ms INTEGER NOT NULL
    );
    CREATE TABLE listing (
        seq  INTEGER PRIMARY KEY AUTOINCREMENT,
        key  TEXT NOT NULL UNIQUE,
        item TEXT NOT NULL
    );",
    // 15: what this node took from directories (ADR 0015). The profiles and
    // posts themselves are in `profiles` and `objects`, as those of anyone it
    // heard of through a reshare. These say which are only there because of a
    // directory, so they stay capped and go first: who was found, through which
    // directory, when, and how many bytes it came to; which posts; and how far
    // this node has read each directory.
    "CREATE TABLE found_people (
        actor     TEXT PRIMARY KEY,
        directory TEXT NOT NULL,
        read_ms   INTEGER NOT NULL,
        bytes     INTEGER NOT NULL
    );
    CREATE TABLE found_posts (
        id    TEXT PRIMARY KEY,
        actor TEXT NOT NULL
    );
    CREATE INDEX found_posts_by_actor ON found_posts(actor);
    CREATE TABLE directories_read (
        directory TEXT PRIMARY KEY,
        cursor    INTEGER NOT NULL
    );",
    // 16: the machines this node's machine name knows (ADR 0015's step 3),
    // and its record of each, which never leaves the node: its latest record
    // as it signed it, where that first came from, when it was heard of, met,
    // last dialled and last answered, dials in a row unanswered, and faded
    // counts as JSON (`meet::Counts`), last faded at `faded_ms`. And when
    // each of the last hour's new machines was dialled, and whether that dial
    // went to the source heard from less (ADR 0016's rule 2).
    "CREATE TABLE machines (
        machine     TEXT PRIMARY KEY,
        record      TEXT NOT NULL,
        source      TEXT NOT NULL,
        heard_ms    INTEGER NOT NULL,
        met_ms      INTEGER,
        tried_ms    INTEGER NOT NULL,
        tries       INTEGER NOT NULL,
        answered_ms INTEGER,
        counts      TEXT NOT NULL,
        faded_ms    INTEGER NOT NULL
    );
    CREATE TABLE machine_dials (
        at_ms      INTEGER NOT NULL,
        less_heard INTEGER NOT NULL
    );",
    // 17: posts and reshares deleted by their authors (ADR 0019), kept for
    // good: each one's ID and author, the signed `Delete` and its own ID, and
    // when it arrived here, as the date in it is only its signer's word.
    // And what each stored post or reshare refers to: the files a post names,
    // the post a reshare points at, so a `Delete` finds them without reading
    // every object. A trigger forgets an object's refs with it, however it goes.
    "CREATE TABLE tombstones (
        id        TEXT PRIMARY KEY,
        actor     TEXT NOT NULL,
        delete_id   TEXT NOT NULL UNIQUE,
        signed      TEXT NOT NULL,
        received_ms INTEGER NOT NULL
    );
    CREATE TABLE refs (
        ref TEXT NOT NULL,
        id  TEXT NOT NULL,
        PRIMARY KEY (ref, id)
    );
    CREATE INDEX refs_by_id ON refs(id);
    CREATE TRIGGER refs_go_with_objects AFTER DELETE ON objects
    BEGIN
        DELETE FROM refs WHERE id = old.id;
    END;",
    // 18: which reader got as far as each directory's cursor
    // (`directory::READER`). One from before passed over entries a later one
    // takes, so a later one reads that directory again from the start.
    "ALTER TABLE directories_read ADD COLUMN reader INTEGER NOT NULL DEFAULT 0;",
    // 19: whether each of the last hour's dials went to a machine that greeted
    // this one unasked (ADR 0016).
    "ALTER TABLE machine_dials ADD COLUMN greeted_back INTEGER NOT NULL DEFAULT 0;",
    // 20: the media each post a directory brought includes, and the size it
    // declares, so it can be watched through a viewing endpoint (ADR 0024).
    // Its rows come and go with `found_posts`. A post kept for another reason
    // moves to `declared_sizes`. Filled in as directories are read again.
    "CREATE TABLE found_blobs (
        blob TEXT NOT NULL,
        post TEXT NOT NULL,
        size INTEGER NOT NULL,
        PRIMARY KEY (blob, post)
    );
    CREATE INDEX found_blobs_by_post ON found_blobs(post);",
    // 21: reshares in directories' lists (ADR 0025). For a reader: a found
    // reshare's files are kept under the reshare (`post`), with the post they
    // belong to (`note`), and `found_hosts` holds the found reshares a
    // directory marked hosting. For a directory: how each listed reshare's
    // checks of the resharer's copy went, and whether it's listed as hosting.
    "ALTER TABLE found_blobs ADD COLUMN note TEXT;
    CREATE TABLE found_hosts (
        via   TEXT PRIMARY KEY,
        note  TEXT NOT NULL,
        actor TEXT NOT NULL
    );
    CREATE INDEX found_hosts_by_note ON found_hosts(note);
    CREATE TABLE reshare_checks (
        announce   TEXT PRIMARY KEY,
        actor      TEXT NOT NULL,
        post       TEXT NOT NULL,
        passed     INTEGER NOT NULL DEFAULT 0,
        fails      INTEGER NOT NULL DEFAULT 0,
        checked_ms INTEGER NOT NULL,
        hosting    INTEGER NOT NULL DEFAULT 0,
        claims     INTEGER NOT NULL DEFAULT 1
    );
    CREATE INDEX reshare_checks_by_post ON reshare_checks(post);
    CREATE INDEX reshare_checks_by_actor ON reshare_checks(actor);",
    // 22: for an author's node, the resharers that told it they host one of
    // its posts (ADR 0025), each of the post's files a row: the resharer's
    // node, identity and `Announce`, its media endpoint's binding (JSON) from
    // its profile, whether that says it may be listed, and how this node's
    // checks of its copy went. A row is made only once a check passed.
    "CREATE TABLE resharer_hosts (
        blob         TEXT NOT NULL,
        node         TEXT NOT NULL,
        post         TEXT NOT NULL,
        actor        TEXT NOT NULL,
        announce     TEXT NOT NULL,
        media        TEXT NOT NULL,
        discoverable INTEGER NOT NULL,
        fails        INTEGER NOT NULL DEFAULT 0,
        checked_ms   INTEGER NOT NULL,
        PRIMARY KEY (blob, node)
    );
    CREATE INDEX resharer_hosts_by_post ON resharer_hosts(post);",
    // The files of posts this node reshared with hosting (ADR 0025's step
    // 5), and the bytes it has sent others of each, for `osp status`. The
    // count stays on the node.
    "CREATE TABLE shared_served (
        blob  TEXT PRIMARY KEY,
        post  TEXT NOT NULL,
        bytes INTEGER NOT NULL DEFAULT 0
    );",
    // 24: followers' nodes the owner removed, whose subscriptions are refused
    // until the owner allows them again, and when.
    "CREATE TABLE removed_followers (
        node       TEXT PRIMARY KEY,
        removed_ms INTEGER NOT NULL
    );",
    // 25: the search index (`SEARCH_SCHEMA`) gained each post's date. It's
    // built again when the database is opened.
    "DROP TABLE IF EXISTS search_text; DROP TABLE IF EXISTS search_items;",
    // 26: replies to this node's posts (ADR 0028), kept apart from `objects` so
    // they never reach the outbox, the feed, search or a directory's lists.
    // Each post's policy, where its owner set one. Each reply received: the
    // post it answers, its replier and the node that sent it, whether it's
    // published, hidden, a request or declined, its place in the thread (given
    // each time it's published, so readers further on still get it), the
    // signed reply and the profile that came with it, and when it arrived.
    // Repliers the owner allowed or blocked. And repliers' `Delete`s of
    // replies held here, so an old copy can't come back.
    "CREATE TABLE reply_policies (
        post   TEXT PRIMARY KEY,
        policy TEXT NOT NULL
    );
    CREATE TABLE received_replies (
        id          TEXT PRIMARY KEY,
        post        TEXT NOT NULL,
        actor       TEXT NOT NULL,
        node        TEXT NOT NULL,
        state       TEXT NOT NULL,
        thread_seq  INTEGER UNIQUE,
        signed      TEXT NOT NULL,
        profile     TEXT NOT NULL,
        received_ms INTEGER NOT NULL
    );
    CREATE INDEX received_replies_by_post ON received_replies(post, thread_seq);
    CREATE INDEX received_replies_by_actor ON received_replies(actor, post);
    CREATE INDEX received_replies_by_state ON received_replies(state, received_ms);
    CREATE TABLE repliers (
        actor  TEXT PRIMARY KEY,
        choice TEXT NOT NULL,
        at_ms  INTEGER NOT NULL
    );
    CREATE TABLE reply_deletes (
        reply       TEXT PRIMARY KEY,
        post        TEXT NOT NULL,
        actor       TEXT NOT NULL,
        signed      TEXT NOT NULL,
        received_ms INTEGER NOT NULL
    );
    CREATE INDEX reply_deletes_by_post ON reply_deletes(post);",
    // 27: the owner's replies to others' posts (ADR 0028), kept from when
    // they're signed and sent to the creator's node until it answers. The
    // post and its creator, and the node to send to when no newer profile
    // names one. The signed reply, gone once the owner deletes it, and then
    // their `Delete` while that's being sent. Where it stands, and what the
    // creator's node said if it refused. Tries so far, when to try next
    // (none once answered), when this sending began (tries stop 30 days
    // after), and when it was signed.
    "CREATE TABLE sent_replies (
        id            TEXT PRIMARY KEY,
        post          TEXT NOT NULL,
        creator       TEXT NOT NULL,
        creator_node  TEXT NOT NULL,
        signed        TEXT,
        delete_signed TEXT,
        state         TEXT NOT NULL,
        error         TEXT,
        tries         INTEGER NOT NULL DEFAULT 0,
        next_try_ms   INTEGER,
        since_ms      INTEGER NOT NULL,
        sent_ms       INTEGER NOT NULL
    );
    CREATE INDEX sent_replies_next_try ON sent_replies(next_try_ms);",
    // 28: who vouched for this node's owner (ADR 0029). Each vouch: a reshare
    // of one of the owner's posts, or a found-early credit from the author of
    // a post the owner reshared; the post, the voucher, the signed object,
    // when it was first and last heard of, and the post's date, by which the
    // oldest go first. Each voucher's newest signed profile, so their node
    // can be reached. The credits this node's owner gave its posts' early
    // finders, and whether each finder's node has it. And which of the
    // owner's reshares their authors' nodes were told of.
    "CREATE TABLE vouches (
        kind     TEXT NOT NULL,
        post     TEXT NOT NULL,
        voucher  TEXT NOT NULL,
        signed   TEXT NOT NULL,
        first_ms INTEGER NOT NULL,
        heard_ms INTEGER NOT NULL,
        post_ms  INTEGER NOT NULL,
        PRIMARY KEY (kind, post, voucher)
    );
    CREATE INDEX vouches_by_post_ms ON vouches(kind, post_ms);
    CREATE INDEX vouches_by_voucher ON vouches(voucher);
    CREATE TABLE voucher_profiles (
        actor        TEXT PRIMARY KEY,
        profile      TEXT NOT NULL,
        published_ms INTEGER NOT NULL
    );
    CREATE TABLE credits_given (
        post   TEXT NOT NULL,
        finder TEXT NOT NULL,
        signed TEXT NOT NULL,
        told   INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (post, finder)
    );
    CREATE TABLE told_reshares (
        announce TEXT PRIMARY KEY
    );",
    // 29: this node's owner's side (ADR 0029): who vouched for them, who
    // vouched for those, and so on, 3 steps back. Each voucher once, at the
    // nearest step found, with the signed vouch that places them. Replaced
    // whole at each daily gathering, whose time is kept so a restart doesn't
    // gather again early.
    "CREATE TABLE side_vouches (
        voucher TEXT PRIMARY KEY,
        step    INTEGER NOT NULL,
        signed  TEXT NOT NULL
    );
    CREATE TABLE side_gathered (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        ms INTEGER NOT NULL
    );",
    // 30: a stranger's reply and who vouched for them (ADR 0029): whether
    // their node was asked to show its owner's side (0 not asked, 1 asked,
    // 2 shown), and what the meeting with the owner's world found, as JSON.
    "ALTER TABLE received_replies ADD COLUMN side_asked INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE received_replies ADD COLUMN found TEXT;",
    // 31: a person's posts and reshares by date, for their profile's pages
    // (ADR 0032).
    "CREATE INDEX objects_by_actor_date ON objects(actor, kind, published_ms, id)",
    // 32: the picture of each profile held (ADR 0032): its file, the size the
    // profile declares, and whose it is, so a device can open it and the node
    // knows where it comes from. Kept with `profiles`.
    "CREATE TABLE icons (
        actor TEXT PRIMARY KEY,
        blob  TEXT NOT NULL,
        size  INTEGER NOT NULL
    );
    CREATE INDEX icons_by_blob ON icons(blob);",
    // 33: the Universe's places (ADR 0034 §4): each group's lasting ID and
    // where each group and person sits, so they outlast a restart. One
    // row, as JSON. Rebuilt from the public lines if lost.
    "CREATE TABLE universe_places (
        id    INTEGER PRIMARY KEY CHECK (id = 0),
        saved BLOB NOT NULL
    )",
    // 34: the bytes of media this node sent other nodes, by UTC day, for its
    // owner's summary (ADR 0036 §3). Days past `SERVED_DAYS` are dropped.
    "CREATE TABLE served_days (
        day   INTEGER PRIMARY KEY,
        bytes INTEGER NOT NULL
    )",
    // 35: what the node did or saw lately, for its owner's devices to draw
    // (ADR 0036 §1, `events`): `what` is the event as JSON, `subject` what
    // it's about, so events about one thing are summed. And the first post
    // stored that declares each blob, so an event can name it.
    "CREATE TABLE events (
        seq      INTEGER PRIMARY KEY AUTOINCREMENT,
        subject  TEXT NOT NULL,
        first_ms INTEGER NOT NULL,
        last_ms  INTEGER NOT NULL,
        count    INTEGER NOT NULL,
        what     TEXT NOT NULL
    );
    CREATE INDEX events_by_subject ON events(subject, first_ms);
    ALTER TABLE declared_sizes ADD COLUMN post TEXT;",
    // 36: the paths from the owner to a stranger who replied, as the meeting
    // of their side found them (ADR 0037, `connected::Path` as JSON), so
    // "How are we connected?" shows what let the reply in.
    "ALTER TABLE received_replies ADD COLUMN paths TEXT",
    // 37: likes (ADR 0038). On the creator's node, each person's newest like
    // of each of its owner's posts: its ID, the node that sent it, the signed
    // like and the profile that came with it, its date (never later than
    // its arrival), and when it arrived. On the liker's node, one row per
    // post the owner liked or took a like back from: the post's creator and
    // the node to send to when no newer profile names one, the like the
    // creator's node may hold, the signed `Like` or `Undo` to send and its
    // ID, whether it's a like, tries so far, when to try next (none once
    // answered), when this sending began (tries stop 7 days after), and what
    // the creator's node said if it refused.
    "CREATE TABLE likes (
        post         TEXT NOT NULL,
        actor        TEXT NOT NULL,
        id           TEXT NOT NULL,
        node         TEXT NOT NULL,
        signed       TEXT NOT NULL,
        profile      TEXT NOT NULL,
        published_ms INTEGER NOT NULL,
        received_ms  INTEGER NOT NULL,
        PRIMARY KEY (post, actor)
    );
    CREATE INDEX likes_by_id ON likes(id);
    CREATE INDEX likes_by_post_date ON likes(post, published_ms);
    CREATE TABLE sent_likes (
        post         TEXT PRIMARY KEY,
        creator      TEXT NOT NULL,
        creator_node TEXT NOT NULL,
        like_id      TEXT NOT NULL,
        signed       TEXT NOT NULL,
        sent_id      TEXT NOT NULL,
        liked        INTEGER NOT NULL,
        tries        INTEGER NOT NULL DEFAULT 0,
        next_try_ms  INTEGER,
        since_ms     INTEGER NOT NULL,
        error        TEXT
    );
    CREATE INDEX sent_likes_next_try ON sent_likes(next_try_ms);",
    // 38: the owner's Activity (ADR 0038 §3, `db::activity`): one row for
    // each thing it's about, by its key, and whether the owner read it. A
    // change takes the row a new `seq`, so the latest is first. What each
    // says is read from where that's kept.
    "CREATE TABLE activity (
        seq      INTEGER PRIMARY KEY AUTOINCREMENT,
        key      TEXT NOT NULL UNIQUE,
        first_ms INTEGER NOT NULL,
        last_ms  INTEGER NOT NULL,
        read     INTEGER NOT NULL
    );",
    // 39: opened posts (ADR 0039 §4, `db::opened`): a post the owner opened
    // from an OSP code, its author not followed. The post is in `objects`
    // and its media in `found_blobs`, as a found post's are; this marks it,
    // until when it's kept, and its size with its media. `opened_people`
    // are those whose profile is held only because of an opened post: no
    // list shows them.
    "CREATE TABLE opened_posts (
        id        TEXT PRIMARY KEY,
        actor     TEXT NOT NULL,
        opened_ms INTEGER NOT NULL,
        until_ms  INTEGER NOT NULL,
        bytes     INTEGER NOT NULL
    );
    CREATE INDEX opened_posts_by_actor ON opened_posts(actor);
    CREATE INDEX opened_posts_by_until ON opened_posts(until_ms);
    CREATE TABLE opened_people (
        actor TEXT PRIMARY KEY
    );",
    // What the owner blocked (ADR 0042 §5): a person, a post or a file,
    // by its hex ID. `name` is a person's name when they were blocked.
    "CREATE TABLE blocks (
        kind   TEXT NOT NULL,
        target TEXT NOT NULL,
        name   TEXT,
        at_ms  INTEGER NOT NULL,
        PRIMARY KEY (kind, target)
    );",
    // A person's signed profile as it was when they were blocked, out of
    // every list, so unblocking puts it back and they can be followed again.
    "ALTER TABLE blocks ADD COLUMN profile TEXT;",
    // The project's hide list (ADR 0042 §6): its entries in force, each a
    // hash of a post's ID or a file's hash; the list as last applied, signed,
    // for phones, and the owner, whose own posts it never hides; and the posts
    // it hides, kept out of every table that shows them, for a retraction.
    "CREATE TABLE hide_list (
        kind TEXT NOT NULL,
        h    TEXT NOT NULL,
        PRIMARY KEY (kind, h)
    );
    CREATE TABLE hide_state (
        one        INTEGER PRIMARY KEY CHECK (one = 1),
        key        TEXT NOT NULL,
        seq        INTEGER NOT NULL,
        published  TEXT NOT NULL,
        entries    INTEGER NOT NULL,
        fetched_ms INTEGER NOT NULL,
        signed     TEXT NOT NULL,
        owner      TEXT NOT NULL
    );
    CREATE TABLE hidden_objects (
        id          TEXT PRIMARY KEY,
        kind        TEXT NOT NULL,
        actor       TEXT NOT NULL,
        signed      TEXT NOT NULL,
        received_ms INTEGER NOT NULL
    );",
    // People the owner excepted from the hide list: their posts aren't
    // hidden here, though their listed files are never served on.
    "CREATE TABLE hide_exceptions (
        actor TEXT PRIMARY KEY,
        at_ms INTEGER NOT NULL
    );",
    // The posts a block of a post or a file dropped, by ID only, so undoing
    // it asks their authors' nodes for them again (through `refetch`).
    "CREATE TABLE block_dropped (
        kind        TEXT NOT NULL,
        target      TEXT NOT NULL,
        id          TEXT NOT NULL,
        actor       TEXT NOT NULL,
        received_ms INTEGER NOT NULL,
        PRIMARY KEY (kind, target, id)
    );",
    // When each machine in the table started, by the earliest of what its
    // records said and when it was first seen, so a later record can't make
    // it new again (ADR 0043's signed start).
    "CREATE TABLE machine_starts (
        machine       TEXT PRIMARY KEY,
        started_ms    INTEGER NOT NULL,
        first_seen_ms INTEGER NOT NULL
    );",
    // What this node, as a viewer, notes of the carriers its circuits went
    // through (ADR 0017's credit, `db::carriers`): the days each finished a
    // job for it, those it ever finished one with, and its probationers.
    "CREATE TABLE carrier_jobs (
        machine TEXT NOT NULL,
        day     INTEGER NOT NULL,
        PRIMARY KEY (machine, day)
    );
    CREATE TABLE carriers_worked (
        machine  TEXT PRIMARY KEY,
        first_ms INTEGER NOT NULL
    );
    CREATE TABLE carrier_probation (
        machine  TEXT PRIMARY KEY,
        since_ms INTEGER NOT NULL,
        last_day INTEGER NOT NULL
    );",
    // Work offers (`db::carriers`, ADR 0043 part 4d-2): the machines whose
    // first middle job for this node it may offer to those who ask, kept
    // apart from their work (`carrier_jobs`), and the machines offered to
    // this node by those it holds at standing.
    "CREATE TABLE carrier_tips (
        machine  TEXT PRIMARY KEY,
        first_ms INTEGER NOT NULL,
        last_ms  INTEGER NOT NULL,
        handed   INTEGER NOT NULL DEFAULT 0,
        ended    INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE carrier_suggestions (
        machine    TEXT PRIMARY KEY,
        descriptor TEXT NOT NULL,
        from_machine TEXT NOT NULL,
        until_ms   INTEGER NOT NULL
    );",
    // What this node carried for others each UTC day, both ways, so carrying
    // keeps within the month's budget (ADR 0043 part 5).
    "CREATE TABLE carried_days (
        day   INTEGER PRIMARY KEY,
        bytes INTEGER NOT NULL
    );",
    // A pairing code for an app reviewer (`osp pair --for-review`): it lasts
    // days, not minutes, so it outlives restarts, and pairs a few devices.
    // Those devices are marked, so ending the review unpairs them.
    "CREATE TABLE review_pairing (
        secret_hash  TEXT PRIMARY KEY,
        expires_ms   INTEGER NOT NULL,
        devices_left INTEGER NOT NULL
    );
    ALTER TABLE devices ADD COLUMN review INTEGER NOT NULL DEFAULT 0;",
    // People who said in a profile a directory gave that they may not be
    // listed, and its date (ADR 0014): forgetting them took that profile
    // too, so this keeps a directory that's behind from listing them again.
    "CREATE TABLE withdrawn_people (
        actor        TEXT PRIMARY KEY,
        published_ms INTEGER NOT NULL,
        noted_ms     INTEGER NOT NULL
    );
    CREATE INDEX withdrawn_people_by_noted ON withdrawn_people(noted_ms);",
    // A vouch is its own signed object, and expires (ADR 0029 section 7).
    // Held: the `Vouch` valid longest from each voucher and kind for this
    // node's owner. Given: the owner's own, one per person and kind, when
    // each is renewed, and whether that person's node has it. Credits given
    // keep only who was credited for which post: the credit itself is a
    // given vouch. Found-early credits held as rows of `vouches`, and sides
    // gathered from reshares, no longer count, so they go.
    "CREATE TABLE held_vouches (
        voucher        TEXT NOT NULL,
        kind           TEXT NOT NULL,
        signed         TEXT NOT NULL,
        valid_until_ms INTEGER NOT NULL,
        heard_ms       INTEGER NOT NULL,
        PRIMARY KEY (voucher, kind)
    );
    CREATE INDEX held_vouches_by_until ON held_vouches(valid_until_ms);
    CREATE TABLE given_vouches (
        subject        TEXT NOT NULL,
        kind           TEXT NOT NULL,
        signed         TEXT NOT NULL,
        valid_until_ms INTEGER NOT NULL,
        renew_ms       INTEGER NOT NULL,
        told           INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (subject, kind)
    );
    CREATE TABLE credits_kept (
        post   TEXT NOT NULL,
        finder TEXT NOT NULL,
        PRIMARY KEY (post, finder)
    );
    INSERT INTO credits_kept (post, finder)
        SELECT post, finder FROM credits_given ORDER BY rowid;
    DROP TABLE credits_given;
    ALTER TABLE credits_kept RENAME TO credits_given;
    DELETE FROM vouches WHERE kind = 'found_early';
    DELETE FROM side_vouches;
    DELETE FROM side_gathered;",
];

/// Withdrawals from being listed kept (`withdrawn_people`), the newest: about
/// a megabyte. Identities are free, so a directory could list many.
const MAX_WITHDRAWN: usize = 10_000;

/// How many days of `served_days` are kept: the summary's 30, and today.
pub const SERVED_DAYS: i64 = 31;

/// The UTC day a time falls on, counted from 1970.
pub fn day_of(ms: i64) -> i64 {
    ms.div_euclid(86_400_000)
}

/// The search index (ADR 0014): a number for each post, by its object ID, and
/// each profile, by its identity, and FTS5's index of their words by that
/// number. The words themselves stay only in the signed objects. A post's
/// date (`objects.published_ms`) is kept by its number too, so a search can
/// order what it finds without looking each match up in `objects`; a
/// profile has none.
///
/// It's derived from `objects` and `profiles`, so it isn't in `MIGRATIONS`:
/// `Db::open` builds it whenever it's missing. To change it, say for another
/// tokenizer, a migration drops both tables.
const SEARCH_SCHEMA: &str = "
CREATE TABLE search_items (
    id           INTEGER PRIMARY KEY,
    key          TEXT NOT NULL UNIQUE,
    published_ms INTEGER
);
CREATE VIRTUAL TABLE search_text USING fts5(
    text,
    content = '',
    contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 2'
);
";

/// Words of a search that count. Far more than anyone types; the rest of a
/// long paste is ignored rather than making the node match all of it.
const MAX_SEARCH_WORDS: usize = 32;

/// Hosts remembered per blob. More than enough to find a copy; a peer
/// can't grow the table without bound by naming hosts.
const MAX_HOSTS_PER_BLOB: i64 = 20;

/// Followers remembered as holding a copy of one of our blobs. Plenty to
/// pick a few from.
const MAX_SWARM_PER_BLOB: i64 = 100;
/// Of those, the most kept for trusted followers (ADR 0013), so an owner
/// who trusts many can't fill the list (`add_swarm_member`).
const MAX_TRUSTED_SWARM_PER_BLOB: i64 = 25;

/// A blob's `accessed_ms` is updated at most this often, so a phone
/// seeking through a video doesn't write to the database on every request.
const TOUCH_EVERY_MS: i64 = 60_000;

const HOUR_MS: i64 = 60 * 60 * 1000;

/// Machines a directory's map holds. A full one loses whichever was heard from
/// longest ago, and machines check in often, so a crowd of new names passes
/// through without pushing out those still there.
const MAX_MAP: i64 = 10_000;

/// Someone a directory lists, and the node it reads for them (ADR 0015).
#[derive(Debug, Clone)]
pub struct Listed {
    pub actor: PublicKey,
    pub node: EndpointId,
    /// Where their outbox had got to when it was last read.
    pub latest: u64,
}

/// Why a blob is kept, which decides who it is served to.
/// A reshare a directory listed, checked by the reader (ADR 0025).
pub struct FoundReshare {
    pub announce: SignedObject,
    pub announce_payload: Payload,
    pub post: SignedObject,
    pub post_payload: Payload,
    /// The directory marked it hosting.
    pub hosting: bool,
}

/// How a directory's checks of one listed reshare's copy have gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReshareCheck {
    /// One has passed, ever.
    pub passed: bool,
    /// Failed in a row since the last that passed.
    pub fails: u32,
    pub checked_ms: i64,
}

/// A listed reshare that says it hosts its post, for the day's pick of the
/// post's hosts (ADR 0025).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostingClaim {
    pub announce: ObjectId,
    pub actor: PublicKey,
    /// When its resharer first asked to be listed; None if they aren't now.
    pub listed_ms: Option<i64>,
    /// Its copy's checks pass: one passed, and not 2 failed in a row since.
    pub passing: bool,
    /// Listed as hosting now.
    pub hosting: bool,
}

/// A resharer that told this node it hosts one of its posts (ADR 0025).
#[derive(Debug, Clone, PartialEq)]
pub struct ResharerHost {
    pub node: EndpointId,
    pub post: ObjectId,
    pub actor: PublicKey,
    pub announce: ObjectId,
    pub media: EndpointBinding,
    pub discoverable: bool,
    /// Checks failed in a row since the last that passed.
    pub fails: u32,
    pub checked_ms: i64,
}

/// What a `Delete` did on this node (`Db::apply_delete`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deleted {
    /// Whether this node held the post or reshare, and removed it.
    pub removed: bool,
    /// Files no post kept here names any more: to stop serving, and let go.
    pub files: Vec<BlobHash>,
    /// Whether one of the reshares that went was this node's owner's, so
    /// they pass the `Delete` on.
    pub ours: bool,
    /// For a hosting reshare that went, the files of the post it pointed
    /// at, which its resharer's nodes no longer host.
    pub unhosted: Vec<BlobHash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlobClass {
    /// Fetched when one of my devices opened it. Served only to them;
    /// evicted first.
    Cache,
    /// Fetched ahead of time because I follow its author or a resharer
    /// (doc 13). Served to anyone if I share prefetched copies
    /// (`swarm.share_prefetched`), otherwise only to my devices. Evicted
    /// after `Cache`.
    Prefetch,
    /// Media I chose to host by resharing. Served to anyone.
    Replica,
    /// Media of my own posts. Served to anyone; never evicted.
    Own,
}

impl BlobClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlobClass::Cache => "cache",
            BlobClass::Prefetch => "prefetch",
            BlobClass::Replica => "replica",
            BlobClass::Own => "own",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "cache" => BlobClass::Cache,
            "prefetch" => BlobClass::Prefetch,
            "replica" => BlobClass::Replica,
            "own" => BlobClass::Own,
            other => anyhow::bail!("unknown blob class {other}"),
        })
    }

    /// Whether anyone (not just my own devices) may download it, whatever
    /// the settings: hosting it is a promise.
    pub fn is_public(&self) -> bool {
        matches!(self, BlobClass::Own | BlobClass::Replica)
    }

    /// Whether anyone may download it now. Prefetched copies may be when
    /// the owner chose to share them.
    pub fn served_to_anyone(&self, share_prefetched: bool) -> bool {
        self.is_public() || (*self == BlobClass::Prefetch && share_prefetched)
    }

    /// Whether it counts against the cache quota and is evicted to make room.
    pub fn is_evictable(&self) -> bool {
        matches!(self, BlobClass::Cache | BlobClass::Prefetch)
    }
}

/// A followed identity and where we sync it from.
#[derive(Debug, Clone)]
pub struct Follow {
    pub actor: PublicKey,
    pub addr: EndpointAddr,
    pub cursor: u64,
    /// Whether a sync has ever completed a page from this follow.
    pub synced: bool,
}

/// What this node measured about one peer node (ADR 0011).
#[derive(Debug, Clone)]
pub struct PeerRow {
    pub node: EndpointId,
    /// Whether this node ever contacted it, rather than only the other way round.
    pub ours: bool,
    pub first_ms: i64,
    pub last_ms: i64,
    pub counts: PeerCounts,
}

/// A request to be a trusted follower, or a node trusted (ADR 0013).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustRow {
    pub node: EndpointId,
    /// The identity whose profile listed the node, and its name there.
    pub actor: PublicKey,
    pub name: String,
    /// Sent with the request; empty if trusted without one.
    pub note: String,
    /// When it asked, while waiting; when it was trusted, once allowed.
    pub at_ms: i64,
}

/// An upload from a paired device (`uploads.rs`).
#[derive(Debug, Clone)]
pub struct UploadRow {
    pub device: EndpointId,
    pub id: UploadId,
    pub content: String,
    pub files: Vec<Upload>,
    pub state: UploadState,
    /// When it last started, stopped or changed state.
    pub updated_ms: i64,
}

/// How far an upload got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadState {
    /// Waiting for the files, or the rest of them.
    Receiving,
    /// All the files are here; the post is being transcoded and published.
    Preparing,
    Posted(ObjectId),
    Failed(String),
}

impl UploadState {
    fn to_columns(&self) -> (&'static str, Option<String>) {
        match self {
            UploadState::Receiving => ("receiving", None),
            UploadState::Preparing => ("preparing", None),
            UploadState::Posted(id) => ("posted", Some(id.to_string())),
            UploadState::Failed(reason) => ("failed", Some(reason.clone())),
        }
    }

    fn from_columns(state: &str, outcome: Option<String>) -> Result<Self> {
        Ok(match (state, outcome) {
            ("receiving", _) => UploadState::Receiving,
            ("preparing", _) => UploadState::Preparing,
            ("posted", Some(id)) => UploadState::Posted(id.parse()?),
            ("failed", reason) => UploadState::Failed(reason.unwrap_or_default()),
            (other, _) => anyhow::bail!("unknown upload state {other}"),
        })
    }

    pub fn is_finished(&self) -> bool {
        matches!(self, UploadState::Posted(_) | UploadState::Failed(_))
    }
}

/// The most reshares shown with a found post: a directory lists up to 16
/// hosting each (ADR 0025).
pub const MAX_FOUND_RESHARES_SHOWN: u32 = 16;

/// A follower's node as stored (`Db::follower_rows`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowerRow {
    pub node: EndpointId,
    pub since_ms: i64,
    pub seen_ms: i64,
    pub shares: bool,
}

/// A follower holding a copy of a blob (`Db::swarm_members`).
#[derive(Debug, Clone)]
pub struct SwarmMember {
    pub addr: EndpointAddr,
    /// Trusted by the owner (ADR 0013).
    pub trusted: bool,
    /// The endpoint it may be named by to other nodes, if it has one.
    pub host: Option<BoundEndpoint>,
    /// When this node checked its copy and noted it.
    pub verified_ms: i64,
}

/// SQLite connection shared by the node's tasks.
///
/// Queries are short, so a plain mutex is fine; a busy node could move to a
/// connection pool later.
pub struct Db {
    conn: Mutex<Connection>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn).with_context(|| format!("updating {}", path.display()))?;
        build_search_index(&conn)
            .with_context(|| format!("building the search index of {}", path.display()))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves SQLite itself consistent, so keep going.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- objects ----

    /// Stores a verified object. Returns false if we already had it.
    pub fn insert_object(&self, obj: &SignedObject, payload: &Payload) -> Result<bool> {
        self.insert(obj, payload, now_ms(), None)
    }

    /// Like `insert_object`, for an object that first arrived at `received`:
    /// a copy fetched again after a revocation takes the place of the one
    /// that was dropped, so it doesn't count as new today (ADR 0010).
    pub fn insert_object_received(
        &self,
        obj: &SignedObject,
        payload: &Payload,
        received: i64,
    ) -> Result<bool> {
        self.insert(obj, payload, received, None)
    }

    /// Like `insert_object`, for a post kept out of the feed until
    /// `until_ms`, or until `show_held` is called for it, while its media
    /// is on its way.
    pub fn insert_object_held(
        &self,
        obj: &SignedObject,
        payload: &Payload,
        until_ms: i64,
    ) -> Result<bool> {
        self.insert(obj, payload, now_ms(), Some(until_ms))
    }

    fn insert(
        &self,
        obj: &SignedObject,
        payload: &Payload,
        received: i64,
        held_until: Option<i64>,
    ) -> Result<bool> {
        // Kept apart, never among objects: an outbox or a feed never holds
        // one (ADR 0038).
        ensure!(
            !matches!(payload, Payload::Like(_) | Payload::Undo(_)),
            "a like isn't kept among objects"
        );
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let is_new = insert_object(&tx, obj, payload, received, held_until)?;
        if let (true, Payload::Note(note)) = (is_new, payload) {
            declare_sizes(&tx, note, Some(&obj.id()))?;
        }
        if let (true, Payload::Announce(a)) = (is_new, payload) {
            note_reshare(&tx, a, now_ms())?;
        }
        // One a directory brought now has another reason to be here.
        if !is_new {
            found_post_is_kept(&tx, &obj.id().to_string())?;
        }
        tx.commit()?;
        Ok(is_new)
    }

    /// How many objects by `actor` arrived at or after `since_ms`.
    pub fn objects_received_since(&self, actor: &PublicKey, since_ms: i64) -> Result<usize> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM objects WHERE actor = ?1 AND received_ms >= ?2",
            params![actor.to_string(), since_ms],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    pub fn get_object(&self, id: &ObjectId) -> Result<Option<SignedObject>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT signed FROM objects WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|j| serde_json::from_str(&j).context("corrupt stored object"))
            .transpose()
    }

    /// Stores `obj` in place of the object with its ID: the same payload,
    /// signed again by a newer key (ADR 0010).
    pub fn replace_signed(&self, obj: &SignedObject) -> Result<()> {
        let conn = self.conn();
        let (id, json) = (obj.id().to_string(), serde_json::to_string(obj)?);
        conn.execute(
            "UPDATE objects SET signed = ?2 WHERE id = ?1",
            params![id, json],
        )?;
        // One of our deletes is kept in its tombstone too (ADR 0019).
        conn.execute(
            "UPDATE tombstones SET signed = ?2 WHERE delete_id = ?1",
            params![id, json],
        )?;
        Ok(())
    }

    /// Posts and reshares by the given identities, newest first: by when
    /// they were published, or, for one that was held back, when it was
    /// shown, so it lands above what the person may have scrolled past.
    pub fn feed(&self, actors: &[PublicKey], limit: u32) -> Result<Vec<SignedObject>> {
        if actors.is_empty() {
            return Ok(vec![]);
        }
        let placeholders = vec!["?"; actors.len()].join(",");
        let now = now_ms();
        let sql = format!(
            "SELECT signed FROM objects
             WHERE kind IN ('Note', 'Announce') AND actor IN ({placeholders}) AND feed_ms <= {now}
               AND {NOT_OPENED}
             ORDER BY feed_ms DESC LIMIT {limit}"
        );
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let actors: Vec<String> = actors.iter().map(|a| a.to_string()).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(actors), |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    /// Shows posts held back from the feed now, at the top: their media
    /// arrived, or won't. One shown already stays where it is.
    pub fn show_held(&self, ids: &[ObjectId]) -> Result<()> {
        let now = now_ms();
        let conn = self.conn();
        for id in ids {
            conn.execute(
                "UPDATE objects SET feed_ms = ?2 WHERE id = ?1 AND feed_ms > ?2",
                params![id.to_string(), now],
            )?;
        }
        Ok(())
    }

    /// Shows every post held back from the feed now. Returns how many.
    pub fn show_all_held(&self) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE objects SET feed_ms = ?1 WHERE feed_ms > ?1",
            params![now_ms()],
        )?)
    }

    // ---- deletes (ADR 0019) ----

    /// Acts on `delete`, the verified signed form of `d`. If this node holds
    /// the post or reshare it names, it must be `d.actor`'s: it goes, with a
    /// post's reshares, from objects, the outbox, the feed, search and found
    /// posts, and the post's files go unless another post kept here names
    /// them. A tombstone is kept if something went, or with `always`, as for
    /// a `Delete` in an outbox this node syncs, so an old copy arriving later
    /// is refused. `None` if it was deleted already, or wasn't here and
    /// isn't to be kept. `me` is this node's owner, whose reshares say so.
    pub fn apply_delete(
        &self,
        delete: &SignedObject,
        d: &Delete,
        always: bool,
        me: &PublicKey,
    ) -> Result<Option<Deleted>> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let target = d.object.to_string();
        let already: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM tombstones WHERE id = ?1",
                params![target],
                |row| row.get(0),
            )
            .optional()?;
        if already.is_some() {
            return Ok(None);
        }
        let held: Option<(String, String)> = tx
            .query_row(
                "SELECT actor, signed FROM objects WHERE id = ?1",
                params![target],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((actor, _)) = &held {
            ensure!(
                *actor == d.actor.to_string(),
                "a delete of a post by someone else, refused"
            );
        } else if !always {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO tombstones (id, actor, delete_id, signed, received_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                target,
                d.actor.to_string(),
                delete.id().to_string(),
                serde_json::to_string(delete)?,
                now_ms()
            ],
        )?;
        let mut deleted = Deleted {
            removed: held.is_some(),
            files: Vec::new(),
            ours: false,
            unhosted: Vec::new(),
        };
        if let Some((_, json)) = held {
            let obj: SignedObject = serde_json::from_str(&json).context("corrupt stored object")?;
            let mut gone = vec![target.clone()];
            let note = match obj.verify() {
                Ok(Payload::Note(note)) => Some(note),
                Ok(Payload::Announce(a)) if a.hosting && a.object_actor != d.actor => {
                    let reshared: Option<String> = tx
                        .query_row(
                            "SELECT signed FROM objects WHERE id = ?1",
                            params![a.object.to_string()],
                            |row| row.get(0),
                        )
                        .optional()?;
                    let reshared = reshared.and_then(|json| {
                        let obj: SignedObject = serde_json::from_str(&json).ok()?;
                        match obj.verify() {
                            Ok(Payload::Note(note)) => Some(note),
                            _ => None,
                        }
                    });
                    deleted.unhosted = reshared.iter().flat_map(|n| n.blobs()).collect();
                    None
                }
                _ => None,
            };
            if note.is_some() {
                let mut stmt = tx.prepare(
                    "SELECT refs.id, objects.actor FROM refs JOIN objects ON objects.id = refs.id
                     WHERE refs.ref = ?1 AND objects.kind = 'Announce'",
                )?;
                let reshares = stmt.query_map(params![target], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for reshare in reshares {
                    let (id, actor) = reshare?;
                    deleted.ours |= actor == me.to_string();
                    gone.push(id);
                }
            }
            drop_objects(
                &tx,
                &gone,
                note.iter().flat_map(|n| n.blobs()),
                &mut deleted.files,
            )?;
            opened::settle_opened_person(&tx, &d.actor.to_string())?;
        }
        tx.commit()?;
        Ok(Some(deleted))
    }

    /// Whether `obj`'s author deleted it, or, for a reshare, the post it
    /// points at.
    pub fn is_deleted(&self, obj: &SignedObject, payload: &Payload) -> Result<bool> {
        is_deleted(&self.conn(), &obj.id(), payload)
    }

    /// Whether a post kept here names `blob`, or a profile held here gives
    /// it as its picture.
    pub fn is_named(&self, blob: &BlobHash) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM refs WHERE ref = ?1
                 UNION ALL SELECT 1 FROM icons WHERE blob = ?1 LIMIT 1",
                params![blob.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Stops hosting `blobs` for others, now that the owner deleted the
    /// reshare that promised to host them (ADR 0019): those in `ahead` are
    /// kept as fetched ahead of time, as a follow would have fetched them,
    /// the rest as cache.
    pub fn stop_hosting(&self, blobs: &[BlobHash], ahead: &[BlobHash]) -> Result<()> {
        let conn = self.conn();
        for blob in blobs {
            let class = match ahead.contains(blob) {
                true => BlobClass::Prefetch,
                false => BlobClass::Cache,
            };
            conn.execute(
                "UPDATE blobs SET class = ?2 WHERE hash = ?1 AND class = 'replica'",
                params![blob.to_string(), class.as_str()],
            )?;
        }
        Ok(())
    }

    /// Whether `post` by `author` comes to this node through a follow: its
    /// author is followed, or someone followed reshared it.
    pub fn comes_through_a_follow(&self, post: &ObjectId, author: &PublicKey) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM follows WHERE actor = ?2)
                 OR EXISTS (SELECT 1 FROM refs
                            JOIN objects ON objects.id = refs.id
                            JOIN follows ON follows.actor = objects.actor
                            WHERE refs.ref = ?1 AND objects.kind = 'Announce')",
            params![post.to_string(), author.to_string()],
            |row| row.get(0),
        )?)
    }

    /// The posts the owner reshared, each with its author, from the outbox.
    pub fn own_reshares(&self, me: &PublicKey) -> Result<Vec<(ObjectId, PublicKey)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT objects.signed FROM outbox JOIN objects ON objects.id = outbox.id
             WHERE objects.kind = 'Announce' AND objects.actor = ?1",
        )?;
        let rows = stmt.query_map(params![me.to_string()], |row| row.get::<_, String>(0))?;
        let mut reshares = Vec::new();
        for json in rows {
            let obj: SignedObject =
                serde_json::from_str(&json?).context("corrupt stored object")?;
            if let Ok(Payload::Announce(a)) = obj.verify() {
                reshares.push((a.object, a.object_actor));
            }
        }
        Ok(reshares)
    }

    /// When this node heard that `id`'s author deleted it, if it did.
    #[cfg(test)]
    pub fn deleted_at(&self, id: &ObjectId) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT received_ms FROM tombstones WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Whether `id`'s author deleted it.
    pub fn is_tombstoned(&self, id: &ObjectId) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM tombstones WHERE id = ?1",
                params![id.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The signed `Delete` of `id`, if its author deleted it and it's `actor`'s.
    pub fn delete_of(&self, id: &ObjectId, actor: &PublicKey) -> Result<Option<SignedObject>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT signed FROM tombstones WHERE id = ?1 AND actor = ?2",
                params![id.to_string(), actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|j| serde_json::from_str(&j).context("corrupt tombstone"))
            .transpose()
    }

    /// A signed `Delete` this node keeps, by its own ID, as a forward carries it.
    pub fn delete_by_id(&self, delete_id: &ObjectId) -> Result<Option<SignedObject>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT signed FROM tombstones WHERE delete_id = ?1",
                params![delete_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|j| serde_json::from_str(&j).context("corrupt tombstone"))
            .transpose()
    }

    // ---- our outbox ----

    pub fn append_outbox(&self, id: &ObjectId) -> Result<u64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO outbox (id) VALUES (?1)",
            params![id.to_string()],
        )?;
        Ok(conn.last_insert_rowid() as u64)
    }

    pub fn outbox_since(&self, since: u64, limit: u32) -> Result<Vec<(u64, SignedObject)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT outbox.seq, objects.signed FROM outbox
             JOIN objects ON objects.id = outbox.id
             WHERE outbox.seq > ?1 ORDER BY outbox.seq LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![since as i64, limit], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|r| {
            let (seq, json) = r?;
            Ok((seq as u64, serde_json::from_str(&json)?))
        })
        .collect()
    }

    /// An object in our own outbox, by ID.
    pub fn outbox_object(&self, id: &ObjectId) -> Result<Option<SignedObject>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT objects.signed FROM outbox
                 JOIN objects ON objects.id = outbox.id
                 WHERE outbox.id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|j| serde_json::from_str(&j).context("corrupt stored object"))
            .transpose()
    }

    pub fn outbox_latest(&self) -> Result<u64> {
        let seq: Option<i64> = self
            .conn()
            .query_row("SELECT MAX(seq) FROM outbox", [], |row| row.get(0))?;
        Ok(seq.unwrap_or(0) as u64)
    }

    // ---- follows ----

    /// Follows `actor` at `addr`, or updates the address of a follow. A
    /// follow that moves to a different node starts over like a new one,
    /// since that node numbers its outbox from 1 (ADR 0009).
    pub fn upsert_follow(&self, actor: &PublicKey, addr: &EndpointAddr) -> Result<()> {
        let conn = self.conn();
        let current: Option<String> = conn
            .query_row(
                "SELECT node_addr FROM follows WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let same_node = match current {
            Some(json) => serde_json::from_str::<EndpointAddr>(&json)?.id == addr.id,
            None => false,
        };
        let sql = if same_node {
            "UPDATE follows SET node_addr = ?2 WHERE actor = ?1"
        } else {
            // Replacing the row resets its position, sync time and error.
            "INSERT OR REPLACE INTO follows (actor, node_addr) VALUES (?1, ?2)"
        };
        conn.execute(
            sql,
            params![actor.to_string(), serde_json::to_string(addr)?],
        )?;
        Ok(())
    }

    /// Stops following `actor`, and forgets what was still to be fetched
    /// again from its node.
    pub fn remove_follow(&self, actor: &PublicKey) -> Result<bool> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let n = tx.execute(
            "DELETE FROM follows WHERE actor = ?1",
            params![actor.to_string()],
        )?;
        tx.execute(
            "DELETE FROM refetch WHERE actor = ?1",
            params![actor.to_string()],
        )?;
        tx.commit()?;
        Ok(n > 0)
    }

    pub fn follows(&self) -> Result<Vec<Follow>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT actor, node_addr, cursor, last_sync IS NOT NULL FROM follows")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        rows.map(|r| {
            let (actor, addr, cursor, synced) = r?;
            Ok(Follow {
                actor: actor.parse()?,
                addr: serde_json::from_str(&addr)?,
                cursor: cursor as u64,
                synced,
            })
        })
        .collect()
    }

    pub fn follow_infos(&self) -> Result<Vec<FollowInfo>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT follows.actor, profiles.name, cursor, last_sync, last_error
             FROM follows LEFT JOIN profiles ON profiles.actor = follows.actor",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        rows.map(|r| {
            let (actor, name, cursor, last_sync, last_error) = r?;
            Ok(FollowInfo {
                actor: actor.parse()?,
                name,
                cursor: cursor as u64,
                last_sync: last_sync.map(format_ms),
                last_error,
            })
        })
        .collect()
    }

    pub fn set_follow_synced(&self, actor: &PublicKey, cursor: u64) -> Result<()> {
        // Never move the cursor backwards if two syncs overlap.
        self.conn().execute(
            "UPDATE follows SET cursor = MAX(cursor, ?2), last_sync = ?3, last_error = NULL
             WHERE actor = ?1",
            params![actor.to_string(), cursor as i64, now_ms()],
        )?;
        Ok(())
    }

    pub fn set_follow_error(&self, actor: &PublicKey, error: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE follows SET last_error = ?2 WHERE actor = ?1",
            params![actor.to_string(), error],
        )?;
        Ok(())
    }

    // ---- followers ----

    /// Records a follower, or updates the address of one we have. Either
    /// way it counts as heard from now. A follower subscribing here knows
    /// where we are, so it needn't be told we moved.
    pub fn add_follower(&self, addr: &EndpointAddr, shares: bool) -> Result<()> {
        self.conn().execute(
            "INSERT INTO followers (node, node_addr, since, seen_ms, shares) VALUES (?1, ?2, ?3, ?3, ?4)
             ON CONFLICT(node) DO UPDATE SET node_addr = excluded.node_addr,
                 seen_ms = excluded.seen_ms, tell_moved = 0, shares = excluded.shares",
            params![
                addr.id.to_string(),
                serde_json::to_string(addr)?,
                now_ms(),
                shares
            ],
        )?;
        Ok(())
    }

    /// Whether a follower said it shares what it fetches ahead of time.
    pub fn follower_shares(&self, node: &EndpointId) -> Result<bool> {
        let shares: Option<bool> = self
            .conn()
            .query_row(
                "SELECT shares FROM followers WHERE node = ?1",
                params![node.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(shares == Some(true))
    }

    /// Marks every follower as still to be told that we moved (ADR 0009).
    pub fn mark_followers_to_tell_moved(&self) -> Result<usize> {
        Ok(self
            .conn()
            .execute("UPDATE followers SET tell_moved = 1", [])?)
    }

    pub fn followers_to_tell_moved(&self) -> Result<Vec<EndpointAddr>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT node_addr FROM followers WHERE tell_moved = 1")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    /// Notes that a follower heard we moved.
    pub fn told_moved(&self, node: &EndpointId) -> Result<()> {
        self.conn().execute(
            "UPDATE followers SET tell_moved = 0, seen_ms = ?2 WHERE node = ?1",
            params![node.to_string(), now_ms()],
        )?;
        Ok(())
    }

    pub fn is_follower(&self, node: &EndpointId) -> Result<bool> {
        let found: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM followers WHERE node = ?1",
                params![node.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Notes that a follower's node answered.
    pub fn follower_seen(&self, node: &EndpointId) -> Result<()> {
        self.conn().execute(
            "UPDATE followers SET seen_ms = ?2 WHERE node = ?1",
            params![node.to_string(), now_ms()],
        )?;
        Ok(())
    }

    /// How many followers first subscribed at or after `since_ms`.
    pub fn new_followers_since(&self, since_ms: i64) -> Result<usize> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM followers WHERE since >= ?1",
            params![since_ms],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    /// Forgets followers not heard from since `before_ms`. Returns how many.
    pub fn drop_silent_followers(&self, before_ms: i64) -> Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        // A node we follow as well asks for them again at its next sync.
        tx.execute(
            "DELETE FROM endpoints WHERE public IN
             (SELECT node FROM followers WHERE COALESCE(seen_ms, since) < ?1)",
            params![before_ms],
        )?;
        let n = tx.execute(
            "DELETE FROM followers WHERE COALESCE(seen_ms, since) < ?1",
            params![before_ms],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// Records the endpoint a follower may be named by to other nodes.
    pub fn set_follower_host(&self, node: &EndpointId, host: Option<&BoundEndpoint>) -> Result<()> {
        let host = host.map(serde_json::to_string).transpose()?;
        self.conn().execute(
            "UPDATE followers SET host = ?2 WHERE node = ?1",
            params![node.to_string(), host],
        )?;
        Ok(())
    }

    /// The endpoint a follower may be named by to other nodes.
    pub fn follower_host(&self, node: &EndpointId) -> Result<Option<BoundEndpoint>> {
        let host: Option<Option<String>> = self
            .conn()
            .query_row(
                "SELECT host FROM followers WHERE node = ?1",
                params![node.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(host
            .flatten()
            .map(|host| serde_json::from_str(&host))
            .transpose()?)
    }

    pub fn followers(&self) -> Result<Vec<EndpointAddr>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT node_addr FROM followers")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    /// Each follower's node, when it first subscribed and was last heard
    /// from, and whether it shares what it fetches ahead of time; oldest first.
    pub fn follower_rows(&self) -> Result<Vec<FollowerRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT node, since, COALESCE(seen_ms, since), shares FROM followers ORDER BY since",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        rows.map(|r| {
            let (node, since_ms, seen_ms, shares) = r?;
            Ok(FollowerRow {
                node: node.parse()?,
                since_ms,
                seen_ms,
                shares,
            })
        })
        .collect()
    }

    /// Removes a follower's node: forgets it as a follower, with its
    /// endpoints, the copies it said it holds and any trust, and refuses its
    /// subscriptions until `allow_follower`. Returns whether it was a follower.
    pub fn remove_follower(&self, node: &EndpointId) -> Result<bool> {
        let node = node.to_string();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let was = tx.execute("DELETE FROM followers WHERE node = ?1", params![node])? > 0;
        // A node we follow as well asks for them again at its next sync.
        tx.execute("DELETE FROM endpoints WHERE public = ?1", params![node])?;
        tx.execute("DELETE FROM swarm WHERE node = ?1", params![node])?;
        tx.execute("DELETE FROM trust WHERE node = ?1", params![node])?;
        tx.execute(
            "INSERT OR IGNORE INTO removed_followers (node, removed_ms) VALUES (?1, ?2)",
            params![node, now_ms()],
        )?;
        tx.commit()?;
        Ok(was)
    }

    /// Lets a removed follower's node subscribe again. Returns whether it
    /// was removed.
    pub fn allow_follower(&self, node: &EndpointId) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM removed_followers WHERE node = ?1",
            params![node.to_string()],
        )?;
        Ok(n > 0)
    }

    /// Whether the owner removed this node as a follower.
    pub fn is_removed_follower(&self, node: &EndpointId) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM removed_followers WHERE node = ?1",
                params![node.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The nodes removed as followers, oldest first.
    pub fn removed_followers(&self) -> Result<Vec<EndpointId>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT node FROM removed_followers ORDER BY removed_ms")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|r| Ok(r?.parse()?)).collect()
    }

    // ---- identity keys (ADR 0010) ----

    /// Checks the key certificate `cert`, dated `cert_ms`, against what we
    /// know of its identity's keys, and keeps it if it's the newest we've
    /// seen. Refuses a key that a newer certificate revoked. Returns true
    /// if this certificate revoked the keys before it.
    pub fn check_key(&self, cert: &SignedObject, key: &Key, cert_ms: i64) -> Result<bool> {
        let conn = self.conn();
        let actor = key.actor.to_string();
        let known: Option<(i64, Option<i64>)> = conn
            .query_row(
                "SELECT cert_ms, revoked_before_ms FROM identity_keys WHERE actor = ?1",
                params![actor],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let revoked_before = known.and_then(|(_, before)| before);
        ensure!(
            revoked_before.is_none_or(|before| cert_ms >= before),
            "signed with a key that {} has revoked",
            key.actor.fmt_short()
        );
        if known.is_some_and(|(newest, _)| cert_ms <= newest) {
            return Ok(false);
        }
        let revokes = key.earlier_keys == EarlierKeys::Revoked;
        let revoked_before = if revokes {
            Some(cert_ms)
        } else {
            revoked_before
        };
        conn.execute(
            "INSERT INTO identity_keys (actor, cert, cert_ms, revoked_before_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(actor) DO UPDATE SET cert = excluded.cert,
                 cert_ms = excluded.cert_ms, revoked_before_ms = excluded.revoked_before_ms",
            params![actor, serde_json::to_string(cert)?, cert_ms, revoked_before],
        )?;
        Ok(revokes)
    }

    /// Whether `obj` was signed by a key its identity has since revoked.
    pub fn signed_by_revoked_key(&self, obj: &SignedObject) -> Result<bool> {
        let before: Option<Option<i64>> = self
            .conn()
            .query_row(
                "SELECT revoked_before_ms FROM identity_keys WHERE actor = ?1",
                params![obj.signer().to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(before
            .flatten()
            .is_some_and(|before| signed_before(obj, before)))
    }

    /// Deletes the stored objects of `actor` signed by keys certified before
    /// `before_ms`, which a newer certificate revoked (ADR 0010), and its
    /// stored profile if one of those keys signed it. With `refetch`, keeps
    /// each deleted object's ID and when it arrived, to ask the identity's
    /// node for a copy signed by its new key. Returns how many it deleted.
    pub fn drop_revoked(&self, actor: &PublicKey, before_ms: i64, refetch: bool) -> Result<usize> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let mut dropped = Vec::new();
        {
            let mut stmt =
                tx.prepare("SELECT id, received_ms, signed FROM objects WHERE actor = ?1")?;
            let rows = stmt.query_map(params![actor.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (id, received, json) = row?;
                let obj: SignedObject =
                    serde_json::from_str(&json).context("corrupt stored object")?;
                if signed_before(&obj, before_ms) {
                    dropped.push((id, received));
                }
            }
        }
        for (id, received) in &dropped {
            tx.execute("DELETE FROM objects WHERE id = ?1", params![id])?;
            tx.execute("DELETE FROM found_posts WHERE id = ?1", params![id])?;
            tx.execute("DELETE FROM found_blobs WHERE post = ?1", params![id])?;
            tx.execute("DELETE FROM opened_posts WHERE id = ?1", params![id])?;
            remove_from_search(&tx, id)?;
            if refetch {
                tx.execute(
                    "INSERT OR IGNORE INTO refetch (id, actor, received_ms) VALUES (?1, ?2, ?3)",
                    params![id, actor.to_string(), received],
                )?;
            }
        }
        // A thief's deletes go too (ADR 0019): the post they named is asked
        // for again, and comes back if the owner's node still has it.
        let mut undone = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT id, signed FROM tombstones WHERE actor = ?1")?;
            let rows = stmt.query_map(params![actor.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, json) = row?;
                let delete: SignedObject =
                    serde_json::from_str(&json).context("corrupt tombstone")?;
                if signed_before(&delete, before_ms) {
                    undone.push(id);
                }
            }
        }
        for id in &undone {
            tx.execute("DELETE FROM tombstones WHERE id = ?1", params![id])?;
            if refetch {
                tx.execute(
                    "INSERT OR IGNORE INTO refetch (id, actor, received_ms) VALUES (?1, ?2, ?3)",
                    params![id, actor.to_string(), now_ms()],
                )?;
            }
        }
        let profile: Option<String> = tx
            .query_row(
                "SELECT signed FROM profiles WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(json) = profile
            && signed_before(&serde_json::from_str(&json)?, before_ms)
        {
            tx.execute(
                "DELETE FROM profiles WHERE actor = ?1",
                params![actor.to_string()],
            )?;
            profiles::forget_icon(&tx, &actor.to_string())?;
            remove_from_search(&tx, &actor.to_string())?;
        }
        // Their hosting reshares of this node's posts aren't named any more.
        // Only each reshare's ID is kept here, not which key signed it, so
        // all of them go: a thief's would pass the rechecks by serving the
        // file. Their own node signs its reshares again with the new key and
        // tells this one again, and is noted again then (ADR 0025, 0010).
        tx.execute(
            "DELETE FROM resharer_hosts WHERE actor = ?1",
            params![actor.to_string()],
        )?;
        tx.commit()?;
        Ok(dropped.len())
    }

    /// Up to `limit` objects of `actor` still to ask its node for again,
    /// with when each first arrived.
    pub fn refetch_due(&self, actor: &PublicKey, limit: usize) -> Result<Vec<(ObjectId, i64)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id, received_ms FROM refetch WHERE actor = ?1 LIMIT ?2")?;
        let rows = stmt.query_map(params![actor.to_string(), limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.map(|r| {
            let (id, received) = r?;
            Ok((id.parse()?, received))
        })
        .collect()
    }

    /// Forgets objects that were asked for again, whether they came back or not.
    pub fn refetch_done(&self, ids: &[ObjectId]) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        for id in ids {
            tx.execute("DELETE FROM refetch WHERE id = ?1", params![id.to_string()])?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---- profiles ----

    /// Stores `obj`, the signed form of `person`, in place of the identity's
    /// profile. Every sync brings the profile again, usually unchanged, and
    /// then nothing is written.
    pub fn upsert_profile(&self, person: &Person, obj: &SignedObject) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        upsert_profile(&tx, person, obj)?;
        tx.commit()?;
        Ok(())
    }

    pub fn profile(&self, actor: &PublicKey) -> Result<Option<SignedObject>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT signed FROM profiles WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|j| serde_json::from_str(&j).context("corrupt stored profile"))
            .transpose()
    }

    /// The media endpoints the newest profile held for `actor` gives, those
    /// that check out (ADR 0024). An older profile's never replaces it, so
    /// neither do its bindings.
    pub fn media_endpoints_of(&self, actor: &PublicKey) -> Result<Vec<EndpointBinding>> {
        Ok(match self.profile(actor)?.map(|obj| obj.verify()) {
            Some(Ok(Payload::Person(person))) if person.actor == *actor => {
                person.media_endpoints().cloned().collect()
            }
            _ => vec![],
        })
    }

    pub fn profile_name(&self, actor: &PublicKey) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT name FROM profiles WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?)
    }

    // ---- a directory's lists (ADR 0015) ----

    /// The node read for someone listed, if they are.
    pub fn listed_node(&self, actor: &PublicKey) -> Result<Option<EndpointId>> {
        let node: Option<String> = self
            .conn()
            .query_row(
                "SELECT node FROM listed WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(node.map(|node| node.parse()).transpose()?)
    }

    pub fn listed_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM listed", [], |row| row.get(0))?;
        Ok(n as usize)
    }

    /// How many people first asked to be listed at or after `since_ms`.
    pub fn new_listings_since(&self, since_ms: i64) -> Result<usize> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM listed WHERE asked_ms >= ?1",
            params![since_ms],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    /// Lists someone, read through `node`. If they're listed through another
    /// node already, reads this one instead, and soon.
    pub fn add_listed(&self, actor: &PublicKey, node: &EndpointId) -> Result<()> {
        self.conn().execute(
            "INSERT INTO listed (actor, node, asked_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(actor) DO UPDATE SET node = excluded.node, latest = 0, tried_ms = 0",
            params![actor.to_string(), node.to_string(), now_ms()],
        )?;
        Ok(())
    }

    /// Those listed who haven't been tried since `tried_before_ms`, the longest
    /// waiting first.
    pub fn listed_due(&self, tried_before_ms: i64) -> Result<Vec<Listed>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT actor, node, latest FROM listed WHERE tried_ms <= ?1 ORDER BY tried_ms",
        )?;
        let rows = stmt.query_map(params![tried_before_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (actor, node, latest) = row?;
            Ok(Listed {
                actor: actor.parse()?,
                node: node.parse()?,
                latest: latest as u64,
            })
        })
        .collect()
    }

    pub fn set_listed_tried(&self, actor: &PublicKey, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE listed SET tried_ms = ?2 WHERE actor = ?1",
            params![actor.to_string(), now],
        )?;
        Ok(())
    }

    /// Someone's node answered, and nothing they're listed with changed.
    pub fn set_listed_read(&self, actor: &PublicKey, latest: u64, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE listed SET latest = ?2, read_ms = ?3 WHERE actor = ?1",
            params![actor.to_string(), latest as i64, now],
        )?;
        Ok(())
    }

    /// What readers are given for someone listed.
    pub fn listed_item(&self, actor: &PublicKey) -> Result<Option<ListingItem>> {
        let item: Option<String> = self
            .conn()
            .query_row(
                "SELECT item FROM listing WHERE key = ?1",
                params![person_listing_key(actor)],
                |row| row.get(0),
            )
            .optional()?;
        item.map(|item| serde_json::from_str(&item).context("corrupt listing"))
            .transpose()
    }

    /// Someone's node answered, and this is what readers are to be given for
    /// them. It takes a new number only if it changed, so readers aren't sent
    /// the same thing again.
    pub fn put_listed(
        &self,
        actor: &PublicKey,
        item: &ListingItem,
        latest: u64,
        now: i64,
    ) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE listed SET latest = ?2, read_ms = ?3 WHERE actor = ?1",
            params![actor.to_string(), latest as i64, now],
        )?;
        put_listing(&tx, &person_listing_key(actor), item)?;
        tx.commit()?;
        Ok(())
    }

    /// Drops someone from the lists, telling readers they're gone. Says whether
    /// they were listed.
    pub fn drop_listed(&self, actor: &PublicKey) -> Result<bool> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let dropped = drop_listed(&tx, actor)?;
        tx.commit()?;
        Ok(dropped)
    }

    /// Drops those whose nodes haven't answered since `before_ms`, or ever,
    /// having asked before it. Returns how many.
    pub fn drop_silent_listed(&self, before_ms: i64) -> Result<usize> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let silent: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT actor FROM listed WHERE COALESCE(read_ms, asked_ms) < ?1")?;
            let rows = stmt.query_map(params![before_ms], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for actor in &silent {
            drop_listed(&tx, &actor.parse()?)?;
        }
        tx.commit()?;
        Ok(silent.len())
    }

    /// Notes a machine that greeted this directory. If it's on the map, says
    /// when it was last greeted back.
    pub fn heard_machine(&self, machine: &EndpointId, now: i64) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "UPDATE map SET heard_ms = ?2 WHERE machine = ?1 RETURNING greeted_ms",
                params![machine.to_string(), now],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// A machine answered when greeted back: it goes on the map with its
    /// record. If the map is full, it takes the place of the one heard from
    /// longest ago.
    pub fn put_machine(
        &self,
        machine: &EndpointId,
        record: &MachineRecord,
        now: i64,
    ) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO map (machine, heard_ms, greeted_ms) VALUES (?1, ?2, ?2)
             ON CONFLICT(machine) DO UPDATE SET heard_ms = ?2, greeted_ms = ?2",
            params![machine.to_string(), now],
        )?;
        let item = ListingItem::Machine {
            record: record.clone(),
        };
        put_listing(&tx, &machine_listing_key(machine), &item)?;
        let oldest: Vec<String> = {
            let mut stmt =
                tx.prepare("SELECT machine FROM map ORDER BY heard_ms DESC LIMIT -1 OFFSET ?1")?;
            let rows = stmt.query_map(params![MAX_MAP], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for machine in &oldest {
            forget_machine(&tx, machine)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Takes machines not heard from since `before_ms` off the map. Readers
    /// aren't told: a machine's record says how old it is.
    pub fn forget_silent_machines(&self, before_ms: i64) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let silent: Vec<String> = {
            let mut stmt = tx.prepare("SELECT machine FROM map WHERE heard_ms < ?1")?;
            let rows = stmt.query_map(params![before_ms], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for machine in &silent {
            forget_machine(&tx, machine)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// How many people and machines a directory holds.
    pub fn directory_counts(&self) -> Result<(u64, u64)> {
        let conn = self.conn();
        let count = |table: &str| -> Result<u64> {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            let n: i64 = conn.query_row(&sql, [], |row| row.get(0))?;
            Ok(n as u64)
        };
        Ok((count("listed")?, count("map")?))
    }

    /// A page of the lists: entries numbered above `since`, in order, stopping
    /// before `MAX_LISTING_PAGE_BYTES`. Also the newest number ever given, which
    /// never goes down, though the row that had it may have gone.
    pub fn listing_since(&self, since: u64, limit: u32) -> Result<(Vec<ListingEntry>, u64)> {
        let conn = self.conn();
        let latest: Option<i64> = conn
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'listing'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let mut stmt =
            conn.prepare("SELECT seq, item FROM listing WHERE seq > ?1 ORDER BY seq LIMIT ?2")?;
        let mut rows = stmt.query(params![since as i64, limit])?;
        let mut entries = Vec::new();
        let mut bytes = 0;
        while let Some(row) = rows.next()? {
            let item: String = row.get(1)?;
            bytes += item.len();
            if !entries.is_empty() && bytes > MAX_LISTING_PAGE_BYTES {
                break;
            }
            let mut item: ListingItem = serde_json::from_str(&item).context("corrupt listing")?;
            // What the hide list holds isn't given out (ADR 0042 §6).
            if let ListingItem::Person {
                posts, reshares, ..
            } = &mut item
            {
                posts.retain(|p| !hide_held_object(&conn, p));
                reshares.retain(|r| {
                    !hide_held_object(&conn, &r.announce) && !hide_held_object(&conn, &r.post)
                });
            }
            entries.push(ListingEntry {
                seq: row.get::<_, i64>(0)? as u64,
                item,
            });
        }
        Ok((entries, latest.unwrap_or(0) as u64))
    }

    // ---- the machines this node knows (ADR 0015's step 3) ----

    /// The machines stored, and when the last hour's new ones were dialled. A
    /// row that doesn't read back is left out.
    pub fn machines(&self) -> Result<(Vec<meet::Entry>, Vec<meet::Dial>)> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT machine, record, source, heard_ms, met_ms, tried_ms, tries, answered_ms,
                    counts, faded_ms
             FROM machines",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                (
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ),
                row.get::<_, String>(8)?,
                row.get(9)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (machine, record, source, times, counts, faded_ms) = row?;
            let (heard_ms, met_ms, tried_ms, tries, answered_ms) = times;
            let entry = (|| -> Result<meet::Entry> {
                let machine: EndpointId = machine.parse()?;
                let record: MachineRecord = serde_json::from_str(&record)?;
                let info = record.verify()?;
                ensure!(info.machine == machine, "a record of another machine");
                Ok(meet::Entry {
                    machine,
                    published_ms: osp_proto::parse_rfc3339_ms(&info.published)
                        .context("undated")?,
                    record,
                    source: meet::Source::from_db(&source)?,
                    heard_ms,
                    met_ms,
                    tried_ms,
                    tries,
                    answered_ms,
                    counts: serde_json::from_str(&counts)?,
                    faded_ms,
                })
            })();
            match entry {
                Ok(entry) => entries.push(entry),
                Err(e) => tracing::warn!("leaving out a machine stored that doesn't read: {e:#}"),
            }
        }
        let mut stmt = conn
            .prepare("SELECT at_ms, less_heard, greeted_back FROM machine_dials ORDER BY at_ms")?;
        let dials = stmt
            .query_map([], |row| {
                Ok(meet::Dial {
                    at_ms: row.get(0)?,
                    less_heard: row.get(1)?,
                    greeted_back: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok((entries, dials))
    }

    /// Writes what changed in the table, all at once.
    pub fn save_machines(&self, changes: &meet::Changes) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        for e in &changes.put {
            tx.execute(
                "INSERT OR REPLACE INTO machines (machine, record, source, heard_ms, met_ms,
                    tried_ms, tries, answered_ms, counts, faded_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    e.machine.to_string(),
                    serde_json::to_string(&e.record)?,
                    e.source.to_db(),
                    e.heard_ms,
                    e.met_ms,
                    e.tried_ms,
                    e.tries,
                    e.answered_ms,
                    serde_json::to_string(&e.counts)?,
                    e.faded_ms,
                ],
            )?;
        }
        for machine in &changes.removed {
            tx.execute(
                "DELETE FROM machines WHERE machine = ?1",
                params![machine.to_string()],
            )?;
        }
        for start in &changes.starts {
            tx.execute(
                "INSERT OR REPLACE INTO machine_starts (machine, started_ms, first_seen_ms)
                 VALUES (?1, ?2, ?3)",
                params![
                    start.machine.to_string(),
                    start.started_ms,
                    start.first_seen_ms
                ],
            )?;
        }
        for machine in &changes.starts_removed {
            tx.execute(
                "DELETE FROM machine_starts WHERE machine = ?1",
                params![machine.to_string()],
            )?;
        }
        if let Some(dials) = &changes.dials {
            tx.execute("DELETE FROM machine_dials", [])?;
            for dial in dials {
                tx.execute(
                    "INSERT INTO machine_dials (at_ms, less_heard, greeted_back)
                     VALUES (?1, ?2, ?3)",
                    params![dial.at_ms, dial.less_heard, dial.greeted_back],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets every machine known, for a new machine name.
    pub fn forget_machines(&self) -> Result<()> {
        self.conn().execute_batch(
            "DELETE FROM machines; DELETE FROM machine_dials; DELETE FROM machine_starts;",
        )?;
        Ok(())
    }

    /// When the machines seen started. A row that doesn't read is left out.
    pub fn machine_starts(&self) -> Result<Vec<meet::Start>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT machine, started_ms, first_seen_ms FROM machine_starts")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut starts = Vec::new();
        for row in rows {
            let (machine, started_ms, first_seen_ms) = row?;
            match machine.parse() {
                Ok(machine) => starts.push(meet::Start {
                    machine,
                    started_ms,
                    first_seen_ms,
                }),
                Err(e) => tracing::warn!("leaving out a machine's start that doesn't read: {e}"),
            }
        }
        Ok(starts)
    }

    // ---- what came from directories (ADR 0015) ----

    pub fn is_following(&self, actor: &PublicKey) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM follows WHERE actor = ?1)",
            params![actor.to_string()],
            |row| row.get(0),
        )?)
    }

    /// How far this node has read a directory's lists: 0 if only a reader
    /// before `reader` got that far, as it passed over entries this one takes.
    pub fn directory_cursor(&self, directory: &EndpointId, reader: u32) -> Result<u64> {
        let cursor: Option<i64> = self
            .conn()
            .query_row(
                "SELECT cursor FROM directories_read WHERE directory = ?1 AND reader >= ?2",
                params![directory.to_string(), reader],
                |row| row.get(0),
            )
            .optional()?;
        Ok(cursor.unwrap_or(0) as u64)
    }

    pub fn set_directory_cursor(
        &self,
        directory: &EndpointId,
        cursor: u64,
        reader: u32,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO directories_read (directory, cursor, reader)
             VALUES (?1, ?2, ?3)",
            params![directory.to_string(), cursor as i64, reader],
        )?;
        Ok(())
    }

    /// Keeps someone found through `directory`: `posts`, in place of those it
    /// brought before, and `profile` unless the one stored is newer. The caller
    /// has checked them. A post that's here for another reason stays as it is.
    #[cfg(test)]
    pub fn keep_found(
        &self,
        directory: &EndpointId,
        person: &Person,
        profile: &SignedObject,
        posts: &[(SignedObject, Payload)],
        now: i64,
    ) -> Result<bool> {
        self.keep_found_with(directory, person, profile, posts, &[], now)
    }

    /// The same, with their reshares (ADR 0025): each reshare and the post it
    /// points at are kept as found, and one the directory marked hosting
    /// names its resharer as a source of that post's media. Returns whether
    /// they're new here: the node held no profile of theirs.
    pub fn keep_found_with(
        &self,
        directory: &EndpointId,
        person: &Person,
        profile: &SignedObject,
        posts: &[(SignedObject, Payload)],
        reshares: &[FoundReshare],
        now: i64,
    ) -> Result<bool> {
        let actor = person.actor.to_string();
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let held: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM profiles WHERE actor = ?1)",
            params![actor],
            |row| row.get(0),
        )?;
        let earlier: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM found_posts WHERE actor = ?1")?;
            let rows = stmt.query_map(params![actor], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let ids: Vec<String> = posts
            .iter()
            .map(|(obj, _)| obj.id().to_string())
            .chain(
                reshares
                    .iter()
                    .flat_map(|r| [r.announce.id().to_string(), r.post.id().to_string()]),
            )
            .collect();
        for id in earlier.iter().filter(|id| !ids.contains(id)) {
            drop_found_post(&tx, id)?;
        }
        let mut bytes = serde_json::to_vec(profile)?.len();
        for (obj, payload) in posts {
            bytes += serde_json::to_vec(obj)?.len();
            let id = obj.id().to_string();
            if insert_object(&tx, obj, payload, now, None)? {
                tx.execute(
                    "INSERT INTO found_posts (id, actor) VALUES (?1, ?2)",
                    params![id, actor],
                )?;
            }
            let found: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM found_posts WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?;
            if let (Some(_), Payload::Note(note)) = (found, payload) {
                for (blob, size) in note.sized_blobs() {
                    tx.execute(
                        "INSERT OR IGNORE INTO found_blobs (blob, post, size) VALUES (?1, ?2, ?3)",
                        params![blob.to_string(), id, size as i64],
                    )?;
                }
            }
        }
        for r in reshares {
            bytes += serde_json::to_vec(&r.announce)?.len() + serde_json::to_vec(&r.post)?.len();
            keep_found_reshare(&tx, r, &actor, now)?;
        }
        if profile_is_newer(&tx, person)? {
            upsert_profile(&tx, person, profile)?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO found_people (actor, directory, read_ms, bytes)
             VALUES (?1, ?2, ?3, ?4)",
            params![actor, directory.to_string(), now, bytes as i64],
        )?;
        tx.commit()?;
        Ok(!held)
    }

    /// Someone's own profile, dated `published_ms`, says they may not be
    /// listed: whatever directory brought them, what was found of them goes,
    /// and the date is kept, so an older profile a directory still lists
    /// can't bring them back. The caller has checked this node holds no later
    /// profile of theirs. Only the newest `MAX_WITHDRAWN` are kept.
    pub fn found_person_withdrew(&self, actor: &PublicKey, published_ms: i64) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let actor_text = actor.to_string();
        tx.execute(
            "INSERT INTO withdrawn_people (actor, published_ms, noted_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT (actor) DO UPDATE SET
                 published_ms = MAX(published_ms, excluded.published_ms),
                 noted_ms = excluded.noted_ms",
            params![actor_text, published_ms, now_ms()],
        )?;
        tx.execute(
            "DELETE FROM withdrawn_people WHERE actor IN (
                 SELECT actor FROM withdrawn_people ORDER BY noted_ms DESC, actor
                 LIMIT -1 OFFSET ?1)",
            params![MAX_WITHDRAWN as i64],
        )?;
        let found: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM found_people WHERE actor = ?1)",
            params![actor_text],
            |row| row.get(0),
        )?;
        if found {
            drop_found_person(&tx, actor)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The date of the latest profile in which someone said they may not be
    /// listed, as `found_person_withdrew` kept it.
    pub fn withdrawn_at(&self, actor: &PublicKey) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT published_ms FROM withdrawn_people WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// A later profile lets someone be listed again.
    pub fn forget_withdrawal(&self, actor: &PublicKey) -> Result<()> {
        self.conn().execute(
            "DELETE FROM withdrawn_people WHERE actor = ?1",
            params![actor.to_string()],
        )?;
        Ok(())
    }

    /// Drops what `directory` brought of someone, if that's where they came
    /// from. A directory can't take away what another brought.
    pub fn forget_found(&self, directory: &EndpointId, actor: &PublicKey) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let from_it: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM found_people WHERE actor = ?1 AND directory = ?2)",
            params![actor.to_string(), directory.to_string()],
            |row| row.get(0),
        )?;
        if from_it {
            drop_found_person(&tx, actor)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Drops all that `directory` brought, to read it again from the start.
    pub fn forget_directory(&self, directory: &EndpointId) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let people: Vec<String> = {
            let mut stmt = tx.prepare("SELECT actor FROM found_people WHERE directory = ?1")?;
            let rows = stmt.query_map(params![directory.to_string()], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for actor in &people {
            drop_found_person(&tx, &actor.parse()?)?;
        }
        tx.execute(
            "DELETE FROM directories_read WHERE directory = ?1",
            params![directory.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Someone found is followed now: what came of them stays, as any follow's
    /// posts do, and no longer counts as found.
    pub fn found_person_is_followed(&self, actor: &PublicKey) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let ids: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM found_posts WHERE actor = ?1")?;
            let rows = stmt.query_map(params![actor.to_string()], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for id in &ids {
            found_post_is_kept(&tx, id)?;
        }
        opened::opened_author_is_followed(&tx, actor)?;
        tx.execute(
            "DELETE FROM found_people WHERE actor = ?1",
            params![actor.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The found post `blob` belongs to, when that's all it belongs to here:
    /// its author's, found through a directory, not followed, and not shared
    /// by this node's owner, which takes it off the found posts (ADR 0024).
    /// Its media is fetched only through viewing endpoints. None for any
    /// other blob, which is fetched the usual way.
    pub fn found_only_post(&self, blob: &BlobHash) -> Result<Option<(ObjectId, Note)>> {
        // A post kept here for good says so: it's fetched the usual way.
        let kept: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM declared_sizes WHERE blob = ?1",
                params![blob.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if kept.is_some() {
            return Ok(None);
        }
        let posts: Vec<String> = {
            let conn = self.conn();
            let mut stmt =
                conn.prepare("SELECT COALESCE(note, post) FROM found_blobs WHERE blob = ?1")?;
            let rows = stmt.query_map(params![blob.to_string()], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for id in posts {
            let id: ObjectId = id.parse()?;
            let Some(obj) = self.get_object(&id)? else {
                continue;
            };
            if let Ok(Payload::Note(note)) = obj.verify()
                && note.blobs().any(|b| b == *blob)
                && !self.is_following(&note.actor)?
            {
                return Ok(Some((id, note)));
            }
        }
        Ok(None)
    }

    /// Whether `blob` is named here only by posts a directory brought, whatever
    /// became of them: such a blob is never fetched the usual way (ADR 0024).
    pub fn only_found_names(&self, blob: &BlobHash) -> Result<bool> {
        let conn = self.conn();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM found_blobs WHERE blob = ?1 LIMIT 1",
                params![blob.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let kept: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM declared_sizes WHERE blob = ?1",
                params![blob.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some() && kept.is_none())
    }

    /// The resharers a directory marked as hosting `post` (ADR 0025), from
    /// the listings this node holds.
    pub fn found_hosts_of(&self, post: &ObjectId) -> Result<Vec<PublicKey>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT DISTINCT actor FROM found_hosts WHERE note = ?1")?;
        let rows = stmt.query_map(params![post.to_string()], |row| row.get::<_, String>(0))?;
        rows.map(|actor| Ok(actor?.parse()?)).collect()
    }

    /// The reshares of `post` that came with it through directories: those
    /// marked hosting, and those of a post with files. A reshare of a post
    /// with only words, not hosting, isn't linked to it here. Newest first,
    /// at most `MAX_FOUND_RESHARES_SHOWN`.
    pub fn found_reshares_of(&self, post: &ObjectId) -> Result<Vec<SignedObject>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT signed FROM objects WHERE id IN (
                 SELECT via FROM found_hosts WHERE note = ?1
                 UNION SELECT post FROM found_blobs WHERE note = ?1)
             ORDER BY published_ms DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![post.to_string(), MAX_FOUND_RESHARES_SHOWN], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|r| serde_json::from_str(&r?).context("corrupt stored object"))
            .collect()
    }

    // ---- a directory's checks of listed reshares (ADR 0025) ----

    pub fn reshare_check(&self, announce: &ObjectId) -> Result<Option<ReshareCheck>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT passed, fails, checked_ms FROM reshare_checks WHERE announce = ?1",
                params![announce.to_string()],
                |row| {
                    Ok(ReshareCheck {
                        passed: row.get::<_, i64>(0)? != 0,
                        fails: row.get::<_, i64>(1)? as u32,
                        checked_ms: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Notes how a check of `actor`'s reshare `announce` of `post` went.
    pub fn note_reshare_check(
        &self,
        announce: &ObjectId,
        actor: &PublicKey,
        post: &ObjectId,
        passed: bool,
        now: i64,
    ) -> Result<ReshareCheck> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO reshare_checks (announce, actor, post, passed, fails, checked_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(announce) DO UPDATE SET
                 passed = MAX(passed, excluded.passed),
                 fails = CASE WHEN excluded.passed = 1 THEN 0 ELSE fails + 1 END,
                 checked_ms = excluded.checked_ms",
            params![
                announce.to_string(),
                actor.to_string(),
                post.to_string(),
                passed as i64,
                (!passed) as i64,
                now
            ],
        )?;
        drop(conn);
        self.reshare_check(announce)?
            .context("a reshare check just noted is missing")
    }

    /// Notes a listed reshare that doesn't claim to host, so a `Delete` of its
    /// post finds it. It's never checked.
    pub fn note_plain_reshare(
        &self,
        announce: &ObjectId,
        actor: &PublicKey,
        post: &ObjectId,
        now: i64,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO reshare_checks (announce, actor, post, checked_ms, claims)
             VALUES (?1, ?2, ?3, ?4, 0)",
            params![
                announce.to_string(),
                actor.to_string(),
                post.to_string(),
                now
            ],
        )?;
        Ok(())
    }

    /// Puts `item` in place of someone's listing, as it is, for readers.
    pub fn replace_listed_item(&self, actor: &PublicKey, item: &ListingItem) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        put_listing(&tx, &person_listing_key(actor), item)?;
        tx.commit()?;
        Ok(())
    }

    /// Says whether `announce` is listed as hosting, for counting a post's.
    pub fn set_reshare_hosting(&self, announce: &ObjectId, hosting: bool) -> Result<()> {
        self.conn().execute(
            "UPDATE reshare_checks SET hosting = ?2 WHERE announce = ?1",
            params![announce.to_string(), hosting as i64],
        )?;
        Ok(())
    }

    /// The listed reshares of `post` that say they host it (ADR 0025), each
    /// with when its resharer first asked to be listed, whether its checks
    /// pass (one passed, and not `fails_unmark` failed in a row since), and
    /// whether it's listed as hosting now.
    pub fn hosting_claims(&self, post: &ObjectId, fails_unmark: u32) -> Result<Vec<HostingClaim>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT reshare_checks.announce, reshare_checks.actor, listed.asked_ms,
                    reshare_checks.passed = 1 AND reshare_checks.fails < ?2,
                    reshare_checks.hosting
             FROM reshare_checks LEFT JOIN listed ON listed.actor = reshare_checks.actor
             WHERE reshare_checks.post = ?1 AND reshare_checks.claims = 1",
        )?;
        let rows = stmt.query_map(params![post.to_string(), fails_unmark], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (announce, actor, listed_ms, passing, hosting) = row?;
            Ok(HostingClaim {
                announce: announce.parse()?,
                actor: actor.parse()?,
                listed_ms,
                passing,
                hosting,
            })
        })
        .collect()
    }

    /// The posts with a listed reshare that says it hosts them.
    pub fn posts_with_hosting_claims(&self) -> Result<Vec<ObjectId>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT DISTINCT post FROM reshare_checks WHERE claims = 1")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|post| post?.parse()).collect()
    }

    /// Forgets the checks of `actor`'s reshares that aren't listed any more.
    pub fn forget_reshare_checks(&self, actor: &PublicKey, keep: &[ObjectId]) -> Result<()> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT announce FROM reshare_checks WHERE actor = ?1")?;
        let ids: Vec<String> = stmt
            .query_map(params![actor.to_string()], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let keep: Vec<String> = keep.iter().map(|id| id.to_string()).collect();
        for id in ids.iter().filter(|id| !keep.contains(id)) {
            conn.execute(
                "DELETE FROM reshare_checks WHERE announce = ?1",
                params![id],
            )?;
        }
        Ok(())
    }

    /// Whether any of `actor`'s listed reshares was last checked before
    /// `before_ms`.
    pub fn reshare_checks_due(&self, actor: &PublicKey, before_ms: i64) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM reshare_checks
                 WHERE actor = ?1 AND claims = 1 AND checked_ms < ?2 LIMIT 1",
                params![actor.to_string(), before_ms],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }

    /// Who has a reshare of `post` listed. Their checks of it go.
    pub fn take_reshares_of(&self, post: &ObjectId) -> Result<Vec<PublicKey>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT DISTINCT actor FROM reshare_checks WHERE post = ?1")?;
        let actors: Vec<String> = stmt
            .query_map(params![post.to_string()], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        conn.execute(
            "DELETE FROM reshare_checks WHERE post = ?1",
            params![post.to_string()],
        )?;
        actors.iter().map(|a| Ok(a.parse()?)).collect()
    }

    /// The owner keeps a post that came from a directory, as by resharing it.
    pub fn found_post_stays(&self, id: &ObjectId) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        found_post_is_kept(&tx, &id.to_string())?;
        tx.commit()?;
        Ok(())
    }

    /// Drops those read longest ago until what's left is within the limits.
    /// Returns how many went.
    pub fn trim_found(&self, max_people: usize, max_bytes: u64) -> Result<usize> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let (mut people, mut bytes): (i64, i64) = tx.query_row(
            "SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM found_people",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let mut dropped = 0;
        if people as usize > max_people || bytes as u64 > max_bytes {
            let oldest: Vec<(String, i64)> = {
                let mut stmt =
                    tx.prepare("SELECT actor, bytes FROM found_people ORDER BY read_ms")?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for (actor, size) in oldest {
                if people as usize <= max_people && bytes as u64 <= max_bytes {
                    break;
                }
                drop_found_person(&tx, &actor.parse()?)?;
                people -= 1;
                bytes -= size;
                dropped += 1;
            }
        }
        tx.commit()?;
        Ok(dropped)
    }

    /// The directory someone was found through, if they were.
    pub fn found_through(&self, actor: &PublicKey) -> Result<Option<EndpointId>> {
        let directory: Option<String> = self
            .conn()
            .query_row(
                "SELECT directory FROM found_people WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(directory.map(|directory| directory.parse()).transpose()?)
    }

    /// The directory a post came from, if it's here only because of one.
    pub fn post_found_through(&self, id: &ObjectId) -> Result<Option<EndpointId>> {
        let directory: Option<String> = self
            .conn()
            .query_row(
                "SELECT directory FROM found_posts
                 JOIN found_people ON found_people.actor = found_posts.actor
                 WHERE found_posts.id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(directory.map(|directory| directory.parse()).transpose()?)
    }

    pub fn found_count(&self) -> Result<u64> {
        let n: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM found_people", [], |row| row.get(0))?;
        Ok(n as u64)
    }

    /// Notes `blob` as a file of `post`, which this node reshared with
    /// hosting, so what it serves of it is counted.
    pub fn note_shared(&self, blob: &BlobHash, post: &ObjectId) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO shared_served (blob, post) VALUES (?1, ?2)",
            params![blob.to_string(), post.to_string()],
        )?;
        Ok(())
    }

    /// Adds `bytes` sent of `blob` to its count, if it's a file of a post
    /// this node reshared with hosting.
    pub fn count_shared_served(&self, blob: &BlobHash, bytes: u64) -> Result<()> {
        self.conn().execute(
            "UPDATE shared_served SET bytes = bytes + ?2 WHERE blob = ?1",
            params![blob.to_string(), bytes as i64],
        )?;
        Ok(())
    }

    /// How many posts this node reshared with hosting, and the bytes it has
    /// sent others of their files.
    pub fn shared_served(&self) -> Result<(u64, u64)> {
        let (posts, bytes): (i64, i64) = self.conn().query_row(
            "SELECT COUNT(DISTINCT post), COALESCE(SUM(bytes), 0) FROM shared_served",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((posts as u64, bytes as u64))
    }

    // ---- search (ADR 0014) ----

    /// Stored posts matching what a person typed: the best match first, and
    /// among equals the newest.
    ///
    ///
    /// Every other request waits while it runs, and a common word can match
    /// most posts, so it ranks the matches by what the index holds (their
    /// words and dates), and reads only the best from `objects`.
    pub fn search_posts(&self, words: &str, limit: u32) -> Result<Vec<SignedObject>> {
        self.search(
            words,
            limit,
            "SELECT objects.signed FROM (
               SELECT search_items.key AS key, bm25(search_text) AS rank,
                      search_items.published_ms AS published_ms
               FROM search_text
               JOIN search_items ON search_items.id = search_text.rowid
               WHERE search_text MATCH ?1 AND search_items.published_ms IS NOT NULL
               ORDER BY rank, published_ms DESC LIMIT ?2
             ) AS best
             JOIN objects ON objects.id = best.key
             ORDER BY best.rank, best.published_ms DESC",
        )
    }

    /// Stored profiles other than `me`'s, with when their newest post or
    /// reshare here was published: newest first, then those with none, by
    /// name.
    pub fn people(&self, me: &PublicKey, limit: u32) -> Result<Vec<(SignedObject, Option<u64>)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT profiles.signed,
                        (SELECT MAX(published_ms) FROM objects
                         WHERE objects.actor = profiles.actor
                           AND objects.kind IN ('Note', 'Announce') AND {NOT_OPENED}) AS newest
                 FROM profiles WHERE profiles.actor != ?1
                   AND profiles.actor NOT IN (SELECT actor FROM opened_people)
                 ORDER BY newest IS NULL, newest DESC, profiles.name LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![me.to_string(), limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })?;
        rows.map(|r| {
            let (json, newest) = r?;
            let obj = serde_json::from_str(&json).context("corrupt stored profile")?;
            Ok((obj, newest.map(|ms| ms.max(0) as u64)))
        })
        .collect()
    }

    /// When `actor`'s newest post or reshare here was published.
    pub fn newest_ms(&self, actor: &PublicKey) -> Result<Option<u64>> {
        let newest: Option<i64> = self.conn().query_row(
            &format!(
                "SELECT MAX(published_ms) FROM objects
                 WHERE actor = ?1 AND kind IN ('Note', 'Announce') AND {NOT_OPENED}"
            ),
            params![actor.to_string()],
            |row| row.get(0),
        )?;
        Ok(newest.map(|ms| ms.max(0) as u64))
    }

    /// Stored profiles matching what a person typed: the best match first.
    pub fn search_profiles(&self, words: &str, limit: u32) -> Result<Vec<SignedObject>> {
        self.search(
            words,
            limit,
            "SELECT profiles.signed FROM search_text
             JOIN search_items ON search_items.id = search_text.rowid
             JOIN profiles ON profiles.actor = search_items.key
             WHERE search_text MATCH ?1
             ORDER BY bm25(search_text), profiles.name LIMIT ?2",
        )
    }

    fn search(&self, words: &str, limit: u32, sql: &str) -> Result<Vec<SignedObject>> {
        let Some(query) = search_query(words) else {
            return Ok(vec![]);
        };
        let conn = self.conn();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![query, limit], |row| row.get::<_, String>(0))?;
        rows.map(|r| serde_json::from_str(&r?).context("corrupt stored object"))
            .collect()
    }

    // ---- blobs ----

    pub fn blob_class(&self, hash: &BlobHash) -> Result<Option<BlobClass>> {
        Ok(self.blob(hash)?.map(|(class, _)| class))
    }

    /// A blob's class and size, if we hold it.
    pub fn blob(&self, hash: &BlobHash) -> Result<Option<(BlobClass, u64)>> {
        let row: Option<(String, i64)> = self
            .conn()
            .query_row(
                "SELECT class, size FROM blobs WHERE hash = ?1",
                params![hash.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(class, size)| Ok((BlobClass::parse(&class)?, size as u64)))
            .transpose()
    }

    /// Records that we hold a blob. A blob's class only ever moves up
    /// (cache -> prefetch -> replica -> own), never down.
    pub fn put_blob(&self, hash: &BlobHash, class: BlobClass, size: u64) -> Result<()> {
        let conn = self.conn();
        let current: Option<String> = conn
            .query_row(
                "SELECT class FROM blobs WHERE hash = ?1",
                params![hash.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let class = match current.map(|c| BlobClass::parse(&c)).transpose()? {
            Some(existing) => existing.max(class),
            None => class,
        };
        let now = now_ms();
        conn.execute(
            "INSERT INTO blobs (hash, class, size, added_ms, accessed_ms) VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(hash) DO UPDATE SET class = excluded.class, accessed_ms = excluded.accessed_ms",
            params![hash.to_string(), class.as_str(), size as i64, now],
        )?;
        Ok(())
    }

    /// Forgets a blob, whatever its class.
    pub fn remove_blob(&self, hash: &BlobHash) -> Result<()> {
        self.conn().execute(
            "DELETE FROM blobs WHERE hash = ?1",
            params![hash.to_string()],
        )?;
        Ok(())
    }

    /// Every blob we hold, with its class.
    pub fn blobs(&self) -> Result<Vec<(BlobHash, BlobClass)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT hash, class FROM blobs")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|r| {
            let (hash, class) = r?;
            Ok((hash.parse()?, BlobClass::parse(&class)?))
        })
        .collect()
    }

    /// Total size of the blobs kept under `class`.
    pub fn class_bytes(&self, class: BlobClass) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT COALESCE(SUM(size), 0) FROM blobs WHERE class = ?1",
            params![class.as_str()],
            |row| row.get(0),
        )?;
        Ok(n as u64)
    }

    /// Total size of the blobs under the cache quota: cached and prefetched.
    pub fn evictable_bytes(&self) -> Result<u64> {
        let n: i64 = self.conn().query_row(
            "SELECT COALESCE(SUM(size), 0) FROM blobs WHERE class IN ('cache', 'prefetch')",
            [],
            |row| row.get(0),
        )?;
        Ok(n as u64)
    }

    /// Notes that one of my devices just used a blob, so the cache keeps
    /// it longer.
    pub fn touch_blob(&self, hash: &BlobHash) -> Result<()> {
        let now = now_ms();
        self.conn().execute(
            "UPDATE blobs SET accessed_ms = ?2 WHERE hash = ?1 AND accessed_ms < ?3",
            params![hash.to_string(), now, now - TOUCH_EVERY_MS],
        )?;
        Ok(())
    }

    /// Forgets cached blobs, then prefetched ones, each in the order they
    /// were used, longest ago first, until at least `bytes` are freed or
    /// none are left (doc 13). Returns them, so their data can be released.
    /// Never touches other classes.
    pub fn evict_cache(&self, bytes: u64) -> Result<Vec<BlobHash>> {
        let conn = self.conn();
        let mut evicted = Vec::new();
        let mut freed = 0;
        {
            let mut stmt = conn.prepare(
                "SELECT hash, size FROM blobs WHERE class IN ('cache', 'prefetch')
                 ORDER BY class = 'prefetch', accessed_ms, added_ms",
            )?;
            let mut rows = stmt.query([])?;
            while freed < bytes
                && let Some(row) = rows.next()?
            {
                evicted.push(row.get::<_, String>(0)?.parse::<BlobHash>()?);
                freed += row.get::<_, i64>(1)? as u64;
            }
        }
        for hash in &evicted {
            conn.execute(
                "DELETE FROM blobs WHERE hash = ?1",
                params![hash.to_string()],
            )?;
        }
        Ok(evicted)
    }

    /// The largest size any stored post declares for a blob. The blob's
    /// real size is fixed by its hash, so a smaller claim elsewhere is
    /// simply wrong, and it mustn't stop the honest post's media loading.
    /// The size the posts here declare for `blob`: those kept, or else those
    /// a directory brought, which may be watched (ADR 0024).
    pub fn declared_size(&self, blob: &BlobHash) -> Result<Option<u64>> {
        let conn = self.conn();
        let size: Option<i64> = conn
            .query_row(
                "SELECT size FROM declared_sizes WHERE blob = ?1",
                params![blob.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let size = match size {
            Some(size) => Some(size),
            None => conn.query_row(
                "SELECT COALESCE(
                     (SELECT MAX(size) FROM found_blobs WHERE blob = ?1),
                     (SELECT MAX(size) FROM icons WHERE blob = ?1))",
                params![blob.to_string()],
                |row| row.get(0),
            )?,
        };
        Ok(size.map(|s| s as u64))
    }

    /// Who posted `blob`, from the posts we have, or whose picture it is.
    pub fn blob_author(&self, blob: &BlobHash) -> Result<Option<PublicKey>> {
        let author: Option<Option<String>> = self
            .conn()
            .query_row(
                "SELECT COALESCE(
                     (SELECT author FROM declared_sizes WHERE blob = ?1),
                     (SELECT actor FROM icons WHERE blob = ?1 LIMIT 1))",
                params![blob.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(author.flatten().map(|a| a.parse()).transpose()?)
    }

    /// Counts media bytes sent to another node on `day`, and drops days past
    /// `SERVED_DAYS`.
    pub fn count_served(&self, day: i64, bytes: u64) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO served_days (day, bytes) VALUES (?1, ?2)
             ON CONFLICT(day) DO UPDATE SET bytes = bytes + excluded.bytes",
            params![day, bytes as i64],
        )?;
        conn.execute(
            "DELETE FROM served_days WHERE day <= ?1",
            params![day - SERVED_DAYS],
        )?;
        Ok(())
    }

    /// Whose media `blob` is, and the first post stored that includes it,
    /// for an event: a profile picture has no post.
    pub fn media_names(&self, blob: &BlobHash) -> Result<(Option<PublicKey>, Option<ObjectId>)> {
        let Some(author) = self.blob_author(blob)? else {
            return Ok((None, None));
        };
        let post: Option<Option<String>> = self
            .conn()
            .query_row(
                "SELECT COALESCE(
                     (SELECT post FROM declared_sizes WHERE blob = ?1),
                     (SELECT post FROM found_blobs WHERE blob = ?1 LIMIT 1))",
                params![blob.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok((Some(author), post.flatten().map(|p| p.parse()).transpose()?))
    }

    /// The resharer whose node `node` is noted as hosting `blob` (ADR 0025),
    /// by their own signed reshare.
    pub fn hosting_resharer(
        &self,
        blob: &BlobHash,
        node: &EndpointId,
    ) -> Result<Option<PublicKey>> {
        let actor: Option<String> = self
            .conn()
            .query_row(
                "SELECT actor FROM resharer_hosts WHERE blob = ?1 AND node = ?2 LIMIT 1",
                params![blob.to_string(), node.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(actor.map(|a| a.parse()).transpose()?)
    }

    /// The nodes `actor`'s profile here names.
    pub fn nodes_of(&self, actor: &PublicKey) -> Result<Vec<EndpointId>> {
        Ok(match self.profile(actor)?.map(|p| p.verify()) {
            Some(Ok(Payload::Person(person))) => person.nodes,
            _ => Vec::new(),
        })
    }

    /// Whether the last sync with `actor`, someone followed, failed.
    pub fn last_sync_failed(&self, actor: &PublicKey) -> Result<bool> {
        let failed: Option<bool> = self
            .conn()
            .query_row(
                "SELECT last_error IS NOT NULL FROM follows WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(failed.unwrap_or(false))
    }

    /// Adds an event that just happened (ADR 0036 §1).
    pub fn add_event(&self, what: EventWhat) -> Result<()> {
        self.add_event_at(what, now_ms())
    }

    pub fn add_event_at(&self, what: EventWhat, now: i64) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        events::add(&tx, what, now)?;
        tx.commit()?;
        Ok(())
    }

    /// The events after `since`, oldest first.
    pub fn events(&self, since: Option<u64>) -> Result<Vec<Event>> {
        self.events_at(since, now_ms())
    }

    pub fn events_at(&self, since: Option<u64>, now: i64) -> Result<Vec<Event>> {
        events::since(&self.conn(), since, now)
    }

    /// The media bytes sent to other nodes over the 30 days up to `today`.
    pub fn served_lately(&self, today: i64) -> Result<u64> {
        let bytes: i64 = self.conn().query_row(
            "SELECT COALESCE(SUM(bytes), 0) FROM served_days WHERE day > ?1 AND day <= ?2",
            params![today - 30, today],
            |row| row.get(0),
        )?;
        Ok(bytes as u64)
    }

    /// The database's own size, in bytes.
    pub fn size_bytes(&self) -> Result<u64> {
        let size: i64 = self.conn().query_row(
            "SELECT page_count * page_size FROM pragma_page_count, pragma_page_size",
            [],
            |row| row.get(0),
        )?;
        Ok(size as u64)
    }

    pub fn storage_summary(&self) -> Result<Vec<StorageInfo>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT class, COUNT(*), COALESCE(SUM(size), 0) FROM blobs GROUP BY class ORDER BY class",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StorageInfo {
                class: row.get(0)?,
                blobs: row.get::<_, i64>(1)? as u64,
                bytes: row.get::<_, i64>(2)? as u64,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- hosts ----

    /// Records a host of a blob, keeping only the most recently seen few.
    /// Forgets `node` as a host of each of `blobs`.
    pub fn forget_hosts(&self, blobs: &[BlobHash], node: &EndpointId) -> Result<()> {
        let conn = self.conn();
        for blob in blobs {
            conn.execute(
                "DELETE FROM hosts WHERE blob = ?1 AND node = ?2",
                params![blob.to_string(), node.to_string()],
            )?;
        }
        Ok(())
    }

    pub fn add_host(&self, blob: &BlobHash, addr: &EndpointAddr) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO hosts (blob, node, node_addr, seen_ms) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(blob, node) DO UPDATE SET node_addr = excluded.node_addr, seen_ms = excluded.seen_ms",
            params![
                blob.to_string(),
                addr.id.to_string(),
                serde_json::to_string(addr)?,
                now_ms()
            ],
        )?;
        conn.execute(
            "DELETE FROM hosts WHERE blob = ?1 AND node NOT IN
             (SELECT node FROM hosts WHERE blob = ?1 ORDER BY seen_ms DESC LIMIT ?2)",
            params![blob.to_string(), MAX_HOSTS_PER_BLOB],
        )?;
        Ok(())
    }

    /// Every node known to host some blob.
    pub fn host_nodes(&self) -> Result<Vec<EndpointId>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT DISTINCT node FROM hosts")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|r| Ok(r?.parse()?)).collect()
    }

    /// Known hosts of a blob, most recently seen first.
    pub fn hosts(&self, blob: &BlobHash) -> Result<Vec<EndpointAddr>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT node_addr FROM hosts WHERE blob = ?1 ORDER BY seen_ms DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![blob.to_string(), MAX_HOSTS_PER_BLOB], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    // ---- the follower swarm (doc 13) ----

    /// Whether `node` is noted as holding a copy of `blob`.
    pub fn in_swarm(&self, blob: &BlobHash, node: &EndpointId) -> Result<bool> {
        let found: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM swarm WHERE blob = ?1 AND node = ?2",
                params![blob.to_string(), node.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Notes that a follower holds a copy of `blob`. Noting it again
    /// changes nothing. Only so many are kept for each blob: up to
    /// `MAX_TRUSTED_SWARM_PER_BLOB` trusted followers that share, those
    /// that have followed us longest (ADR 0013); then, of the others, those
    /// that still share first, half the followers that have followed us
    /// longest, so a crowd of new followers can't push them out, and half
    /// those noted last, so followers that got it later, with their uploads
    /// free, are named too. A new one is always kept, as the last noted.
    pub fn add_swarm_member(&self, blob: &BlobHash, node: &EndpointId) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO swarm (blob, node, seen_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(blob, node) DO NOTHING",
            params![blob.to_string(), node.to_string(), now_ms()],
        )?;
        conn.execute(
            "WITH members AS (
                 SELECT s.node, f.shares IS 1 AS sharing, f.since, s.seen_ms,
                     t.node IS NOT NULL AS trusted
                 FROM swarm s LEFT JOIN followers f ON f.node = s.node
                     LEFT JOIN trust t ON t.node = s.node AND t.state = 'allowed'
                 WHERE s.blob = ?1),
             trusted AS (
                 SELECT node FROM members WHERE trusted AND sharing
                 ORDER BY since, seen_ms LIMIT ?2),
             others AS (
                 SELECT * FROM members WHERE node NOT IN (SELECT node FROM trusted)),
             longest AS (
                 SELECT node FROM others ORDER BY sharing DESC, since, seen_ms
                 LIMIT (?3 - (SELECT COUNT(*) FROM trusted)) / 2),
             latest AS (
                 SELECT node FROM others WHERE node NOT IN (SELECT node FROM longest)
                 ORDER BY sharing DESC, seen_ms DESC
                 LIMIT ?3 - (SELECT COUNT(*) FROM trusted) - (SELECT COUNT(*) FROM longest))
             DELETE FROM swarm WHERE blob = ?1
                 AND node NOT IN (SELECT node FROM trusted)
                 AND node NOT IN (SELECT node FROM longest)
                 AND node NOT IN (SELECT node FROM latest)",
            params![
                blob.to_string(),
                MAX_TRUSTED_SWARM_PER_BLOB,
                MAX_SWARM_PER_BLOB
            ],
        )?;
        Ok(())
    }

    /// The followers, other than `except`, that hold a copy of `blob` and
    /// still say they share, in random order.
    pub fn swarm_members(&self, blob: &BlobHash, except: &EndpointId) -> Result<Vec<SwarmMember>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT f.node_addr, t.node IS NOT NULL, f.host, s.seen_ms
             FROM swarm s JOIN followers f ON f.node = s.node
                 LEFT JOIN trust t ON t.node = s.node AND t.state = 'allowed'
             WHERE s.blob = ?1 AND f.shares = 1 AND s.node != ?2
             ORDER BY RANDOM()",
        )?;
        let rows = stmt.query_map(params![blob.to_string(), except.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        rows.map(|r| {
            let (addr, trusted, host, verified_ms) = r?;
            Ok(SwarmMember {
                addr: serde_json::from_str(&addr)?,
                trusted,
                host: host.map(|h| serde_json::from_str(&h)).transpose()?,
                verified_ms,
            })
        })
        .collect()
    }

    // ---- hosting resharers, for an author's node (ADR 0025) ----

    /// Notes a resharer whose copy of `host.post` passed its check, for each
    /// of `blobs`, the post's files. One noted before keeps its checks, and
    /// gets the endpoint and listing choice of its newest profile.
    pub fn note_resharer_host(&self, host: &ResharerHost, blobs: &[BlobHash]) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let media = serde_json::to_string(&host.media)?;
        for blob in blobs {
            tx.execute(
                "INSERT INTO resharer_hosts
                     (blob, node, post, actor, announce, media, discoverable, fails, checked_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(blob, node) DO UPDATE SET
                     actor = excluded.actor, announce = excluded.announce,
                     media = excluded.media, discoverable = excluded.discoverable",
                params![
                    blob.to_string(),
                    host.node.to_string(),
                    host.post.to_string(),
                    host.actor.to_string(),
                    host.announce.to_string(),
                    media,
                    host.discoverable as i64,
                    host.fails as i64,
                    host.checked_ms,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Whether `node` is noted as a hosting resharer of `post`.
    pub fn is_resharer_host(&self, post: &ObjectId, node: &EndpointId) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM resharer_hosts WHERE post = ?1 AND node = ?2 LIMIT 1",
                params![post.to_string(), node.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }

    /// How many resharers are noted as hosting `post`.
    pub fn resharer_hosts_of(&self, post: &ObjectId) -> Result<usize> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(DISTINCT node) FROM resharer_hosts WHERE post = ?1",
            params![post.to_string()],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    /// The resharers to name as hosts of `blob` to anyone: they may be
    /// listed, and fewer than `max_fails` checks in a row failed.
    pub fn named_resharers(&self, blob: &BlobHash, max_fails: u32) -> Result<Vec<ResharerHost>> {
        self.resharer_rows(
            "WHERE blob = ?1 AND discoverable = 1 AND fails < ?2",
            params![blob.to_string(), max_fails as i64],
        )
    }

    /// Hosting resharers last checked before `before_ms`, one row per post.
    pub fn resharer_rechecks_due(&self, before_ms: i64) -> Result<Vec<(ResharerHost, BlobHash)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT post, node, MIN(blob) FROM resharer_hosts
             WHERE checked_ms < ?1 GROUP BY post, node",
        )?;
        let due: Vec<(String, String, String)> = stmt
            .query_map(params![before_ms], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        drop(conn);
        let mut out = Vec::new();
        for (post, node, blob) in due {
            let rows = self.resharer_rows(
                "WHERE post = ?1 AND node = ?2 AND blob = ?3",
                params![post, node, blob],
            )?;
            if let Some(row) = rows.into_iter().next() {
                out.push((row, blob.parse()?));
            }
        }
        Ok(out)
    }

    /// Notes how a recheck of `node`'s copy of `post` went.
    pub fn note_resharer_check(
        &self,
        post: &ObjectId,
        node: &EndpointId,
        passed: bool,
        now: i64,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE resharer_hosts SET checked_ms = ?3,
                 fails = CASE WHEN ?4 = 1 THEN 0 ELSE fails + 1 END
             WHERE post = ?1 AND node = ?2",
            params![post.to_string(), node.to_string(), now, passed as i64],
        )?;
        Ok(())
    }

    /// Forgets `node` as a hosting resharer of `post`, or with no node, every
    /// resharer of it.
    pub fn forget_resharer_hosts(&self, post: &ObjectId, node: Option<&EndpointId>) -> Result<()> {
        let conn = self.conn();
        match node {
            Some(node) => conn.execute(
                "DELETE FROM resharer_hosts WHERE post = ?1 AND node = ?2",
                params![post.to_string(), node.to_string()],
            )?,
            None => conn.execute(
                "DELETE FROM resharer_hosts WHERE post = ?1",
                params![post.to_string()],
            )?,
        };
        Ok(())
    }

    fn resharer_rows(
        &self,
        filter: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<ResharerHost>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT node, post, actor, announce, media, discoverable, fails, checked_ms
             FROM resharer_hosts {filter} ORDER BY node"
        ))?;
        let rows = stmt.query_map(args, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })?;
        rows.map(|r| {
            let (node, post, actor, announce, media, discoverable, fails, checked_ms) = r?;
            Ok(ResharerHost {
                node: node.parse()?,
                post: post.parse()?,
                actor: actor.parse()?,
                announce: announce.parse()?,
                media: serde_json::from_str(&media)?,
                discoverable: discoverable != 0,
                fails: fails as u32,
                checked_ms,
            })
        })
        .collect()
    }

    /// How many blobs `node` is noted as holding a copy of.
    pub fn swarm_copies(&self, node: &EndpointId) -> Result<usize> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM swarm WHERE node = ?1",
            params![node.to_string()],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    // ---- trusted followers (ADR 0013) ----

    /// Records a request from `ask.node` to be a trusted follower, within
    /// the limits: one per node, a repeat replacing the note; none from a
    /// node allowed, or declined in the last `trust::DECLINED_IGNORED`; at
    /// most `per_hour` new ones an hour; and `trust::MAX_WAITING` waiting,
    /// the oldest going to make room. `ask.at_ms` is when it came. Returns
    /// whether it was kept.
    pub fn add_trust_request(&self, ask: &TrustRow, per_hour: usize) -> Result<bool> {
        let now = ask.at_ms;
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        forget_old_trust_requests(&tx, now)?;
        let node = ask.node.to_string();
        let state: Option<String> = tx
            .query_row(
                "SELECT state FROM trust WHERE node = ?1",
                params![node],
                |row| row.get(0),
            )
            .optional()?;
        let kept = match state.as_deref() {
            Some("waiting") => {
                tx.execute(
                    "UPDATE trust SET actor = ?2, name = ?3, note = ?4 WHERE node = ?1",
                    params![node, ask.actor.to_string(), ask.name, ask.note],
                )?;
                true
            }
            Some(_) => false,
            None => {
                let this_hour: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM trust WHERE asked_ms >= ?1",
                    params![now - HOUR_MS],
                    |row| row.get(0),
                )?;
                let fits = (this_hour as usize) < per_hour;
                if fits {
                    tx.execute(
                        "DELETE FROM trust WHERE state = 'waiting' AND node NOT IN
                         (SELECT node FROM trust WHERE state = 'waiting'
                          ORDER BY asked_ms DESC LIMIT ?1)",
                        params![trust::MAX_WAITING as i64 - 1],
                    )?;
                    tx.execute(
                        "INSERT INTO trust (node, actor, name, note, state, asked_ms)
                         VALUES (?1, ?2, ?3, ?4, 'waiting', ?5)",
                        params![node, ask.actor.to_string(), ask.name, ask.note, now],
                    )?;
                }
                fits
            }
        };
        tx.commit()?;
        Ok(kept)
    }

    /// Requests to be a trusted follower still waiting, newest first.
    pub fn trust_requests(&self, now: i64) -> Result<Vec<TrustRow>> {
        let conn = self.conn();
        forget_old_trust_requests(&conn, now)?;
        trust_rows(
            &conn,
            "SELECT node, actor, name, note, asked_ms FROM trust
             WHERE state = 'waiting' ORDER BY asked_ms DESC",
        )
    }

    /// How many requests to be a trusted follower are waiting.
    pub fn trust_request_count(&self, now: i64) -> Result<usize> {
        let conn = self.conn();
        forget_old_trust_requests(&conn, now)?;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM trust WHERE state = 'waiting'",
            [],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    /// What `node` said when it asked to be trusted, or was trusted, in any
    /// state: whose node it is, and the note.
    pub fn trust_row(&self, node: &EndpointId) -> Result<Option<TrustRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT node, actor, name, note, COALESCE(asked_ms, answered_ms, 0) FROM trust
             WHERE node = ?1",
        )?;
        let row = stmt
            .query_row(params![node.to_string()], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .optional()?;
        row.map(|(actor, name, note, at_ms)| {
            Ok(TrustRow {
                node: *node,
                actor: actor.parse()?,
                name,
                note,
                at_ms,
            })
        })
        .transpose()
    }

    /// The nodes trusted, the most recently trusted first.
    pub fn trusted(&self) -> Result<Vec<TrustRow>> {
        trust_rows(
            &self.conn(),
            "SELECT node, actor, name, note, answered_ms FROM trust
             WHERE state = 'allowed' ORDER BY answered_ms DESC",
        )
    }

    /// Allows or declines the request waiting from `node`. False if none is.
    pub fn answer_trust(&self, node: &EndpointId, allow: bool, now: i64) -> Result<bool> {
        let state = if allow { "allowed" } else { "declined" };
        let n = self.conn().execute(
            "UPDATE trust SET state = ?2, answered_ms = ?3 WHERE node = ?1 AND state = 'waiting'",
            params![node.to_string(), state, now],
        )?;
        Ok(n > 0)
    }

    /// Trusts `nodes` as `actor`'s, whether or not they asked, or were declined.
    pub fn trust_nodes(
        &self,
        actor: &PublicKey,
        name: &str,
        nodes: &[EndpointId],
        now: i64,
    ) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        for node in nodes {
            tx.execute(
                "INSERT INTO trust (node, actor, name, note, state, answered_ms)
                 VALUES (?1, ?2, ?3, '', 'allowed', ?4)
                 ON CONFLICT(node) DO UPDATE SET actor = excluded.actor, name = excluded.name,
                     state = 'allowed', answered_ms = excluded.answered_ms",
                params![node.to_string(), actor.to_string(), name, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Takes trust back from `actor` on every node. Returns on how many.
    pub fn untrust(&self, actor: &PublicKey) -> Result<usize> {
        Ok(self.conn().execute(
            "DELETE FROM trust WHERE actor = ?1 AND state = 'allowed'",
            params![actor.to_string()],
        )?)
    }

    /// When `node` first followed us, if it does.
    pub fn follower_since(&self, node: &EndpointId) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT since FROM followers WHERE node = ?1",
                params![node.to_string()],
                |row| row.get(0),
            )
            .optional()?)
    }

    // ---- other nodes' endpoints (ADR 0014) ----

    /// Keeps a checked endpoint of another node, in place of what it had of
    /// that kind, unless what it had is dated later. Says whether it did.
    pub fn keep_endpoint(&self, bound: &BoundEndpoint) -> Result<bool> {
        let binding = &bound.binding;
        let (public, kind) = (binding.public.to_string(), kind_name(binding.kind));
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let later: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM endpoints WHERE public = ?1 AND kind = ?2 AND since_ms > ?3",
                params![public, kind, binding.since_ms as i64],
                |row| row.get(0),
            )
            .optional()?;
        if later.is_some() {
            return Ok(false);
        }
        tx.execute(
            "DELETE FROM endpoints WHERE public = ?1 AND kind = ?2",
            params![public, kind],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO endpoints (endpoint, public, kind, since_ms, bound)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                binding.endpoint.to_string(),
                public,
                kind,
                binding.since_ms as i64,
                serde_json::to_string(bound)?
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// The node a direct endpoint serves for, if one of them told us.
    pub fn direct_endpoint_owner(&self, endpoint: &EndpointId) -> Result<Option<EndpointId>> {
        let public: Option<String> = self
            .conn()
            .query_row(
                "SELECT public FROM endpoints WHERE endpoint = ?1 AND kind = ?2",
                params![endpoint.to_string(), kind_name(EndpointKind::Direct)],
                |row| row.get(0),
            )
            .optional()?;
        Ok(public.map(|p| p.parse()).transpose()?)
    }

    /// The node any endpoint we know of serves for: what's measured of the
    /// endpoint, and what it may do, is that node's.
    pub fn endpoint_owner(&self, endpoint: &EndpointId) -> Result<Option<EndpointId>> {
        let public: Option<String> = self
            .conn()
            .query_row(
                "SELECT public FROM endpoints WHERE endpoint = ?1",
                params![endpoint.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(public.map(|p| p.parse()).transpose()?)
    }

    /// A node's endpoint of `kind`, and how to reach it, as it told us.
    pub fn endpoint_of(
        &self,
        public: &EndpointId,
        kind: EndpointKind,
    ) -> Result<Option<BoundEndpoint>> {
        let bound: Option<String> = self
            .conn()
            .query_row(
                "SELECT bound FROM endpoints WHERE public = ?1 AND kind = ?2",
                params![public.to_string(), kind_name(kind)],
                |row| row.get(0),
            )
            .optional()?;
        Ok(bound.map(|b| serde_json::from_str(&b)).transpose()?)
    }

    /// Forgets a node's endpoint of `kind`, once the node stops offering it.
    pub fn forget_endpoint(&self, public: &EndpointId, kind: EndpointKind) -> Result<()> {
        self.conn().execute(
            "DELETE FROM endpoints WHERE public = ?1 AND kind = ?2",
            params![public.to_string(), kind_name(kind)],
        )?;
        Ok(())
    }

    /// Every endpoint we know of, to find them again after a restart.
    pub fn endpoints(&self) -> Result<Vec<BoundEndpoint>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT bound FROM endpoints")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|bound| Ok(serde_json::from_str(&bound?)?))
            .collect()
    }

    // ---- nodes allowed a direct connection by the rule `approved` ----

    pub fn add_direct_contact(&self, node: &EndpointId) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO direct_contacts (node, added_ms) VALUES (?1, ?2)",
            params![node.to_string(), now_ms()],
        )?;
        Ok(())
    }

    pub fn remove_direct_contact(&self, node: &EndpointId) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM direct_contacts WHERE node = ?1",
            params![node.to_string()],
        )?;
        Ok(n > 0)
    }

    pub fn is_direct_contact(&self, node: &EndpointId) -> Result<bool> {
        let found: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM direct_contacts WHERE node = ?1",
                params![node.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Oldest first.
    pub fn direct_contacts(&self) -> Result<Vec<EndpointId>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT node FROM direct_contacts ORDER BY added_ms")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|node| Ok(node?.parse()?)).collect()
    }

    /// The direct endpoint a node told us of, to close connections with it.
    pub fn direct_endpoint_of(&self, public: &EndpointId) -> Result<Option<EndpointId>> {
        let endpoint: Option<String> = self
            .conn()
            .query_row(
                "SELECT endpoint FROM endpoints WHERE public = ?1 AND kind = ?2",
                params![public.to_string(), kind_name(EndpointKind::Direct)],
                |row| row.get(0),
            )
            .optional()?;
        Ok(endpoint.map(|e| e.parse()).transpose()?)
    }

    // ---- devices ----

    /// Records a paired device, or for one paired before, its name and the
    /// new pairing time. A device
    /// paired with the review code is marked as the reviewer's; pairing again
    /// with an ordinary code makes it the owner's.
    pub fn add_device(&self, node: &EndpointId, name: &str, review: bool) -> Result<()> {
        self.conn().execute(
            // A device pairing again, say after its app forgot the node, was
            // paired now: `osp pair` sees it by the time.
            "INSERT INTO devices (node, name, paired_ms, review) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(node) DO UPDATE SET name = excluded.name, review = excluded.review,
                 paired_ms = excluded.paired_ms",
            params![node.to_string(), name, now_ms(), review],
        )?;
        Ok(())
    }

    /// Makes `secret_hash` the review code, replacing any before it.
    pub fn set_review_pairing(
        &self,
        secret_hash: &blake3::Hash,
        expires_ms: i64,
        devices: u32,
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM review_pairing", [])?;
        tx.execute(
            "INSERT INTO review_pairing (secret_hash, expires_ms, devices_left) VALUES (?1, ?2, ?3)",
            params![secret_hash.to_hex().as_str(), expires_ms, devices],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The review code, if one is still usable at `now_ms`: its secret's
    /// hash, when it expires and how many more devices it pairs. One that
    /// expired is forgotten.
    pub fn review_pairing(&self, now_ms: i64) -> Result<Option<(blake3::Hash, i64, u32)>> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM review_pairing WHERE expires_ms <= ?1 OR devices_left <= 0",
            params![now_ms],
        )?;
        let row = conn
            .query_row(
                "SELECT secret_hash, expires_ms, devices_left FROM review_pairing",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, u32>(2)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(hash, expires, left)| Ok((blake3::Hash::from_hex(&hash)?, expires, left)))
            .transpose()
    }

    /// One more device paired with the review code; it's forgotten once it
    /// has paired as many as it was made for.
    pub fn use_review_pairing(&self, secret_hash: &blake3::Hash) -> Result<()> {
        let conn = self.conn();
        let hash = secret_hash.to_hex();
        conn.execute(
            "UPDATE review_pairing SET devices_left = devices_left - 1 WHERE secret_hash = ?1",
            params![hash.as_str()],
        )?;
        conn.execute("DELETE FROM review_pairing WHERE devices_left <= 0", [])?;
        Ok(())
    }

    /// Ends a review: forgets the review code, and returns whether there was
    /// one and the devices paired with it, for unpairing.
    pub fn end_review(&self) -> Result<(bool, Vec<EndpointId>)> {
        let conn = self.conn();
        let code = conn.execute("DELETE FROM review_pairing", [])? > 0;
        let mut stmt = conn.prepare("SELECT node FROM devices WHERE review = 1")?;
        let devices = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|node| Ok(node?.parse()?))
            .collect::<Result<_>>()?;
        Ok((code, devices))
    }

    pub fn remove_device(&self, node: &EndpointId) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM devices WHERE node = ?1",
            params![node.to_string()],
        )?;
        Ok(n > 0)
    }

    /// Paired devices with their names, pairing times and whether the
    /// review code paired them, oldest first.
    pub fn devices(&self) -> Result<Vec<DeviceRow>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT node, name, paired_ms, review FROM devices ORDER BY paired_ms")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        rows.map(|r| {
            let (node, name, paired, review) = r?;
            Ok((node.parse()?, name, format_ms(paired), review))
        })
        .collect()
    }

    pub fn is_device(&self, node: &EndpointId) -> Result<bool> {
        let found: Option<i64> = self
            .conn()
            .query_row(
                "SELECT 1 FROM devices WHERE node = ?1",
                params![node.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    // ---- uploads from devices ----

    /// Records a new upload as receiving. False if the device already has
    /// one with this ID.
    pub fn add_upload(
        &self,
        device: &EndpointId,
        id: &UploadId,
        content: &str,
        files: &[Upload],
        now: i64,
    ) -> Result<bool> {
        let n = self.conn().execute(
            "INSERT OR IGNORE INTO uploads (device, id, content, files, state, updated_ms)
             VALUES (?1, ?2, ?3, ?4, 'receiving', ?5)",
            params![
                device.to_string(),
                id.to_string(),
                content,
                serde_json::to_string(files)?,
                now
            ],
        )?;
        Ok(n > 0)
    }

    pub fn upload(&self, device: &EndpointId, id: &UploadId) -> Result<Option<UploadRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT device, id, content, files, state, outcome, updated_ms FROM uploads
             WHERE device = ?1 AND id = ?2",
        )?;
        stmt.query_row(params![device.to_string(), id.to_string()], upload_columns)
            .optional()?
            .map(parse_upload)
            .transpose()
    }

    /// Every upload, oldest change first.
    pub fn uploads(&self) -> Result<Vec<UploadRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT device, id, content, files, state, outcome, updated_ms FROM uploads
             ORDER BY updated_ms, rowid",
        )?;
        let rows = stmt.query_map([], upload_columns)?;
        rows.map(|r| parse_upload(r?)).collect()
    }

    /// Changes an upload's state, and when it last changed.
    pub fn set_upload_state(
        &self,
        device: &EndpointId,
        id: &UploadId,
        state: &UploadState,
        now: i64,
    ) -> Result<()> {
        let (state, outcome) = state.to_columns();
        self.conn().execute(
            "UPDATE uploads SET state = ?3, outcome = ?4, updated_ms = ?5
             WHERE device = ?1 AND id = ?2",
            params![device.to_string(), id.to_string(), state, outcome, now],
        )?;
        Ok(())
    }

    pub fn remove_upload(&self, device: &EndpointId, id: &UploadId) -> Result<bool> {
        let n = self.conn().execute(
            "DELETE FROM uploads WHERE device = ?1 AND id = ?2",
            params![device.to_string(), id.to_string()],
        )?;
        Ok(n > 0)
    }

    /// Forgets finished uploads that last changed before `before_ms`.
    pub fn forget_finished_uploads(&self, before_ms: i64) -> Result<usize> {
        Ok(self.conn().execute(
            "DELETE FROM uploads WHERE state IN ('posted', 'failed') AND updated_ms < ?1",
            params![before_ms],
        )?)
    }

    /// The nodes each stored profile lists, with the profile's identity and name.
    pub fn profile_nodes(&self) -> Result<Vec<(PublicKey, String, Vec<EndpointId>)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT name, signed FROM profiles")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut found = Vec::new();
        for row in rows {
            let (name, json) = row?;
            let obj: SignedObject =
                serde_json::from_str(&json).context("corrupt stored profile")?;
            if let Ok(Payload::Person(person)) = obj.verify() {
                found.push((person.actor, name, person.nodes));
            }
        }
        Ok(found)
    }

    // ---- peers (ADR 0011) ----

    /// Adds to what this node measured about a peer node, after fading what
    /// was there to `now`. `ours` says this node made the contact. Paired
    /// devices are the owner's own, not peers, and aren't recorded. A new
    /// row, or one joining the group of nodes this node contacted, makes
    /// room by dropping the row in its group with the oldest contact; rows
    /// with no contact for `peers::FORGET_AFTER` go too.
    pub fn update_peer(
        &self,
        node: &EndpointId,
        ours: bool,
        now: i64,
        update: impl FnOnce(&mut PeerCounts),
    ) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let id = node.to_string();
        let device: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM devices WHERE node = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        if device.is_some() {
            return Ok(());
        }
        let known: Option<(bool, i64, String)> = tx
            .query_row(
                "SELECT ours, last_ms, counts FROM peers WHERE node = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (was_ours, mut counts) = match known {
            Some((was_ours, last_ms, json)) => (
                Some(was_ours),
                peers::fade(serde_json::from_str(&json)?, now - last_ms),
            ),
            None => (None, PeerCounts::default()),
        };
        update(&mut counts);
        let ours = ours || was_ours == Some(true);
        if was_ours != Some(ours) {
            // New to this group.
            tx.execute(
                "DELETE FROM peers WHERE last_ms < ?1",
                params![now - peers::FORGET_AFTER.as_millis() as i64],
            )?;
            let in_group: i64 = tx.query_row(
                "SELECT COUNT(*) FROM peers WHERE ours = ?1",
                params![ours],
                |row| row.get(0),
            )?;
            if in_group >= peers::MAX_PER_GROUP as i64 {
                tx.execute(
                    "DELETE FROM peers WHERE node =
                     (SELECT node FROM peers WHERE ours = ?1 ORDER BY last_ms LIMIT 1)",
                    params![ours],
                )?;
            }
        }
        tx.execute(
            "INSERT INTO peers (node, ours, first_ms, last_ms, counts) VALUES (?1, ?2, ?3, ?3, ?4)
             ON CONFLICT(node) DO UPDATE SET
                 ours = excluded.ours, last_ms = excluded.last_ms, counts = excluded.counts",
            params![id, ours, now, serde_json::to_string(&counts)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// What this node measured about each peer node, faded to `now`, most
    /// recent contact first. Forgets peers not in contact for `peers::FORGET_AFTER`.
    pub fn peers(&self, now: i64) -> Result<Vec<PeerRow>> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM peers WHERE last_ms < ?1",
            params![now - peers::FORGET_AFTER.as_millis() as i64],
        )?;
        let mut stmt = conn.prepare(
            "SELECT node, ours, first_ms, last_ms, counts FROM peers ORDER BY last_ms DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        rows.map(|r| {
            let (node, ours, first_ms, last_ms, json) = r?;
            Ok(PeerRow {
                node: node.parse()?,
                ours,
                first_ms,
                last_ms,
                counts: peers::fade(serde_json::from_str(&json)?, now - last_ms),
            })
        })
        .collect()
    }

    pub fn follower_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM followers", [], |row| row.get(0))?;
        Ok(n as usize)
    }
}

/// An `uploads` row as stored: device, id, content, files, state, outcome, updated_ms.
type UploadColumns = (String, String, String, String, String, Option<String>, i64);

fn upload_columns(row: &rusqlite::Row<'_>) -> rusqlite::Result<UploadColumns> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn parse_upload(columns: UploadColumns) -> Result<UploadRow> {
    let (device, id, content, files, state, outcome, updated_ms) = columns;
    Ok(UploadRow {
        device: device.parse()?,
        id: id.parse()?,
        content,
        files: serde_json::from_str(&files)?,
        state: UploadState::from_columns(&state, outcome)?,
        updated_ms,
    })
}

/// Forgets requests to be a trusted follower waiting since before
/// `trust::WAITING_KEPT`, or from a node that no longer follows us, and
/// declines older than `trust::DECLINED_IGNORED`, so that node may ask again.
fn forget_old_trust_requests(conn: &Connection, now: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM trust WHERE (state = 'waiting'
             AND (asked_ms < ?1 OR node NOT IN (SELECT node FROM followers)))
             OR (state = 'declined' AND answered_ms < ?2)",
        params![
            now - trust::WAITING_KEPT.as_millis() as i64,
            now - trust::DECLINED_IGNORED.as_millis() as i64
        ],
    )?;
    Ok(())
}

/// `trust` rows from `sql`, which selects node, actor, name, note and a time.
fn trust_rows(conn: &Connection, sql: &str) -> Result<Vec<TrustRow>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    rows.map(|r| {
        let (node, actor, name, note, at_ms) = r?;
        Ok(TrustRow {
            node: node.parse()?,
            actor: actor.parse()?,
            name,
            note,
            at_ms,
        })
    })
    .collect()
}

/// Whether `person` is dated after the profile stored for them, or none is.
fn profile_is_newer(conn: &Connection, person: &Person) -> Result<bool> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT signed FROM profiles WHERE actor = ?1",
            params![person.actor.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(stored) = stored else {
        return Ok(true);
    };
    let stored: SignedObject = serde_json::from_str(&stored).context("corrupt stored profile")?;
    Ok(match stored.verify() {
        Ok(known) => {
            osp_proto::parse_rfc3339_ms(&person.published)
                > osp_proto::parse_rfc3339_ms(known.published())
        }
        Err(_) => true,
    })
}

fn upsert_profile(conn: &Connection, person: &Person, obj: &SignedObject) -> Result<()> {
    let actor = person.actor.to_string();
    // Someone the owner blocked isn't kept, so no list shows them.
    if blocks::blocks_account(conn, &actor)? {
        return Ok(());
    }
    // Held for another reason now, not only for an opened post. Opening
    // one marks them again after this.
    opened::opened_person_is_kept(conn, &actor)?;
    let signed = serde_json::to_string(obj)?;
    let stored: Option<String> = conn
        .query_row(
            "SELECT signed FROM profiles WHERE actor = ?1",
            params![actor],
            |row| row.get(0),
        )
        .optional()?;
    if stored.as_ref() == Some(&signed) {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO profiles (actor, name, signed) VALUES (?1, ?2, ?3)
         ON CONFLICT(actor) DO UPDATE SET name = excluded.name, signed = excluded.signed",
        params![actor, person.name, signed],
    )?;
    profiles::record_icon(conn, person)?;
    index_for_search(conn, &actor, &profile_text(person), None)
}

/// Stores a signed object, and its words for a search. Returns false if it was
/// already here.
fn insert_object(
    conn: &Connection,
    obj: &SignedObject,
    payload: &Payload,
    received: i64,
    held_until: Option<i64>,
) -> Result<bool> {
    // Its author deleted it, or the post it reshares: an old copy stays out.
    if is_deleted(conn, &obj.id(), payload)? {
        return Ok(false);
    }
    // The owner blocked it, its author or a file it names (ADR 0042 §5).
    if blocks::blocks_object(conn, &obj.id(), payload)? {
        return Ok(false);
    }
    // Replies are kept apart (ADR 0028), never where a feed, an outbox, search
    // or a directory's lists would find them.
    ensure!(
        !matches!(payload, Payload::Note(note) if note.is_reply()),
        "a reply isn't stored as a post"
    );
    // The project's hide list holds it (ADR 0042 §6): kept aside, for a
    // retraction to bring back. After the check above, so a reply, which a
    // retraction couldn't put back here, is never kept aside.
    if hide::hides_object(conn, &obj.id(), payload)? {
        hide::keep_aside(conn, obj, payload, received)?;
        return Ok(false);
    }
    // Clamp author-claimed time so a future-dated post can't pin itself to the top.
    let published = osp_proto::parse_rfc3339_ms(payload.published())
        .unwrap_or(received)
        .min(received);
    let id = obj.id().to_string();
    let n = conn.execute(
        "INSERT OR IGNORE INTO objects
             (id, actor, kind, published_ms, received_ms, signed, feed_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            payload.actor().to_string(),
            payload.kind(),
            published,
            received,
            serde_json::to_string(obj)?,
            held_until.unwrap_or(published)
        ],
    )?;
    if n > 0 {
        add_refs(conn, &id, payload)?;
        if let Payload::Note(note) = payload {
            index_for_search(conn, &id, &note.content, Some(published))?;
        }
    }
    Ok(n > 0)
}

/// Takes the objects `gone` from objects, the outbox, the feed, search and
/// found posts, with replies to them, then each of `blobs` no post kept here
/// names any more, adding those to `files`: the caller lets them go from the
/// store. Used by a delete (ADR 0019) and a block (ADR 0042 §5).
fn drop_objects(
    tx: &Connection,
    gone: &[String],
    blobs: impl IntoIterator<Item = BlobHash>,
    files: &mut Vec<BlobHash>,
) -> Result<()> {
    for id in gone {
        tx.execute("DELETE FROM objects WHERE id = ?1", params![id])?;
        tx.execute("DELETE FROM outbox WHERE id = ?1", params![id])?;
        tx.execute("DELETE FROM found_posts WHERE id = ?1", params![id])?;
        tx.execute("DELETE FROM found_blobs WHERE post = ?1", params![id])?;
        tx.execute("DELETE FROM opened_posts WHERE id = ?1", params![id])?;
        tx.execute("DELETE FROM refetch WHERE id = ?1", params![id])?;
        remove_from_search(tx, id)?;
        replies::forget_replies_to(tx, id)?;
    }
    for blob in blobs {
        let blob_hex = blob.to_string();
        let named: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM refs WHERE ref = ?1 LIMIT 1",
                params![blob_hex],
                |row| row.get(0),
            )
            .optional()?;
        if named.is_some() || files.contains(&blob) {
            continue;
        }
        forget_blob(tx, &blob_hex)?;
        files.push(blob);
    }
    Ok(())
}

/// Whether the hide list holds `obj`, which a directory is about to give
/// out. One that doesn't verify is left to the reader to refuse.
fn hide_held_object(conn: &Connection, obj: &SignedObject) -> bool {
    obj.verify()
        .is_ok_and(|p| hide::hides_object(conn, &obj.id(), &p).unwrap_or(false))
}

/// Forgets a file: its record, who hosts it, its swarm and declared size.
fn forget_blob(tx: &Connection, blob_hex: &str) -> Result<()> {
    for table in ["blobs", "hosts", "swarm", "declared_sizes"] {
        let column = if table == "blobs" { "hash" } else { "blob" };
        tx.execute(
            &format!("DELETE FROM {table} WHERE {column} = ?1"),
            params![blob_hex],
        )?;
    }
    Ok(())
}

/// Whether `id`'s author deleted it, or, for a reshare, the post it points at.
fn is_deleted(conn: &Connection, id: &ObjectId, payload: &Payload) -> Result<bool> {
    let tombstoned = |id: &ObjectId| -> Result<bool> {
        Ok(conn
            .query_row(
                "SELECT 1 FROM tombstones WHERE id = ?1",
                params![id.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    };
    Ok(tombstoned(id)? || matches!(payload, Payload::Announce(a) if tombstoned(&a.object)?))
}

/// Notes what a stored object refers to: the files a post names, and the post
/// a reshare points at.
fn add_refs(conn: &Connection, id: &str, payload: &Payload) -> Result<()> {
    let refs: Vec<String> = match payload {
        Payload::Note(note) => note.blobs().map(|blob| blob.to_string()).collect(),
        Payload::Announce(a) => vec![a.object.to_string()],
        _ => Vec::new(),
    };
    for r in refs {
        conn.execute(
            "INSERT OR IGNORE INTO refs (ref, id) VALUES (?1, ?2)",
            params![r, id],
        )?;
    }
    Ok(())
}

/// Fills in `refs` from the posts and reshares stored before it existed.
fn backfill_refs(conn: &Connection) -> Result<()> {
    let mut stmt =
        conn.prepare("SELECT id, signed FROM objects WHERE kind IN ('Note', 'Announce')")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, json) = row?;
        let obj: SignedObject = serde_json::from_str(&json).context("corrupt stored object")?;
        // Checked when it arrived; one that fails today's checks names nothing.
        if let Ok(payload) = obj.verify() {
            add_refs(conn, &id, &payload)?;
        }
    }
    Ok(())
}

/// A post that came from a directory is kept for another reason now, so it's
/// no longer capped or dropped with those. Only now does this node say what
/// media it includes: with no node to fetch it from, there's no use saying so
/// before, and these come and go.
fn found_post_is_kept(conn: &Connection, id: &str) -> Result<()> {
    opened::opened_post_is_kept(conn, id)?;
    let n = conn.execute("DELETE FROM found_posts WHERE id = ?1", params![id])?;
    conn.execute("DELETE FROM found_blobs WHERE post = ?1", params![id])?;
    if n > 0 {
        let signed: String = conn.query_row(
            "SELECT signed FROM objects WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        let obj: SignedObject = serde_json::from_str(&signed).context("corrupt stored object")?;
        if let Payload::Note(note) = obj.verify()? {
            declare_sizes(conn, &note, Some(&obj.id()))?;
        }
    }
    Ok(())
}

/// Removes a post that came from a directory and has no other reason to be here.
fn drop_found_post(conn: &Connection, id: &str) -> Result<()> {
    // A post a found reshare still points at stays, as that reshare's.
    let pointed_at: Option<String> = conn
        .query_row(
            "SELECT found_posts.actor FROM refs
             JOIN found_posts ON found_posts.id = refs.id
             WHERE refs.ref = ?1 AND refs.id != ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(owner) = pointed_at {
        conn.execute(
            "UPDATE found_posts SET actor = ?2 WHERE id = ?1",
            params![id, owner],
        )?;
        return Ok(());
    }
    let owner: Option<String> = conn
        .query_row(
            "SELECT actor FROM found_posts WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    // What a reshare points at, before its refs go with it.
    let reshared: Vec<String> = {
        let mut stmt = conn.prepare("SELECT ref FROM refs WHERE id = ?1")?;
        let rows = stmt.query_map(params![id], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    conn.execute("DELETE FROM found_posts WHERE id = ?1", params![id])?;
    conn.execute("DELETE FROM found_blobs WHERE post = ?1", params![id])?;
    conn.execute("DELETE FROM found_hosts WHERE via = ?1", params![id])?;
    conn.execute("DELETE FROM objects WHERE id = ?1", params![id])?;
    remove_from_search(conn, id)?;
    // The post a found reshare brought goes with it, unless something else
    // here holds it.
    for post in reshared {
        let same_owner: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM found_posts WHERE id = ?1 AND actor IS ?2",
                params![post, owner],
                |row| row.get(0),
            )
            .optional()?;
        if same_owner.is_some() {
            drop_found_post(conn, &post)?;
        }
    }
    Ok(())
}

/// Keeps a reshare a directory listed, and the post it points at, as found
/// under `actor`, the resharer's listing.
fn keep_found_reshare(conn: &Connection, r: &FoundReshare, actor: &str, now: i64) -> Result<()> {
    let (announce, note) = (r.announce.id().to_string(), r.post.id().to_string());
    for (obj, payload, id) in [
        (&r.post, &r.post_payload, &note),
        (&r.announce, &r.announce_payload, &announce),
    ] {
        if insert_object(conn, obj, payload, now, None)? {
            conn.execute(
                "INSERT INTO found_posts (id, actor) VALUES (?1, ?2)",
                params![id, actor],
            )?;
        }
    }
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM found_posts WHERE id = ?1",
            params![announce],
            |row| row.get(0),
        )
        .optional()?;
    if found.is_none() {
        return Ok(());
    }
    if let Payload::Note(post) = &r.post_payload {
        for (blob, size) in post.sized_blobs() {
            conn.execute(
                "INSERT OR IGNORE INTO found_blobs (blob, post, size, note)
                 VALUES (?1, ?2, ?3, ?4)",
                params![blob.to_string(), announce, size as i64, note],
            )?;
        }
    }
    match r.hosting {
        true => conn.execute(
            "INSERT OR REPLACE INTO found_hosts (via, note, actor) VALUES (?1, ?2, ?3)",
            params![announce, note, actor],
        )?,
        false => conn.execute("DELETE FROM found_hosts WHERE via = ?1", params![announce])?,
    };
    Ok(())
}

/// Removes everything a directory brought of someone. Their profile goes unless
/// something else here needs it: they're followed, or a reshare holds their post.
fn drop_found_person(conn: &Connection, actor: &PublicKey) -> Result<()> {
    let actor_text = actor.to_string();
    let ids: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM found_posts WHERE actor = ?1")?;
        let rows = stmt.query_map(params![actor_text], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for id in &ids {
        drop_found_post(conn, id)?;
    }
    conn.execute(
        "DELETE FROM found_people WHERE actor = ?1",
        params![actor_text],
    )?;
    let n = conn.execute(
        "DELETE FROM profiles WHERE actor = ?1
             AND NOT EXISTS (SELECT 1 FROM follows WHERE actor = ?1)
             AND NOT EXISTS (SELECT 1 FROM objects WHERE actor = ?1)",
        params![actor_text],
    )?;
    if n > 0 {
        profiles::forget_icon(conn, &actor_text)?;
        remove_from_search(conn, &actor_text)?;
    } else {
        opened::profile_only_for_opened(conn, &actor_text)?;
    }
    forget_key_if_unused(conn, actor)
}

fn person_listing_key(actor: &PublicKey) -> String {
    format!("person:{actor}")
}

fn machine_listing_key(machine: &EndpointId) -> String {
    format!("machine:{machine}")
}

/// Puts `item` in the lists under `key`, in place of what was there, with a new
/// number. An item that hasn't changed keeps its number.
fn put_listing(conn: &Connection, key: &str, item: &ListingItem) -> Result<()> {
    let item = serde_json::to_string(item)?;
    let stored: Option<String> = conn
        .query_row(
            "SELECT item FROM listing WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    if stored.as_ref() != Some(&item) {
        conn.execute(
            "INSERT OR REPLACE INTO listing (key, item) VALUES (?1, ?2)",
            params![key, item],
        )?;
    }
    Ok(())
}

fn drop_listed(conn: &Connection, actor: &PublicKey) -> Result<bool> {
    conn.execute(
        "DELETE FROM reshare_checks WHERE actor = ?1",
        params![actor.to_string()],
    )?;
    let n = conn.execute(
        "DELETE FROM listed WHERE actor = ?1",
        params![actor.to_string()],
    )?;
    if n > 0 {
        put_listing(
            conn,
            &person_listing_key(actor),
            &ListingItem::Gone { actor: *actor },
        )?;
        forget_key_if_unused(conn, actor)?;
    }
    Ok(n > 0)
}

fn forget_machine(conn: &Connection, machine: &str) -> Result<()> {
    conn.execute("DELETE FROM map WHERE machine = ?1", params![machine])?;
    conn.execute(
        "DELETE FROM listing WHERE key = ?1",
        params![format!("machine:{machine}")],
    )?;
    Ok(())
}

/// Forgets what's known of a stranger's keys, once nothing here needs it, so
/// strangers coming and going can't fill the table (`verify_received`).
fn forget_key_if_unused(conn: &Connection, actor: &PublicKey) -> Result<()> {
    conn.execute(
        "DELETE FROM identity_keys WHERE actor = ?1
             AND NOT EXISTS (SELECT 1 FROM listed WHERE actor = ?1)
             AND NOT EXISTS (SELECT 1 FROM found_people WHERE actor = ?1)
             AND NOT EXISTS (SELECT 1 FROM follows WHERE actor = ?1)
             AND NOT EXISTS (SELECT 1 FROM profiles WHERE actor = ?1)
             AND NOT EXISTS (SELECT 1 FROM objects WHERE actor = ?1)",
        params![actor.to_string()],
    )?;
    Ok(())
}

/// Runs the migrations this database hasn't had yet, each in one transaction.
/// How the `endpoints` table names a kind of endpoint.
fn kind_name(kind: EndpointKind) -> &'static str {
    match kind {
        EndpointKind::Media => "media",
        EndpointKind::Direct => "direct",
    }
}

fn migrate(conn: &Connection) -> Result<()> {
    let done: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(done as usize) {
        let version = i as i64 + 1;
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        // `declared_sizes` has had all its columns since 9, and every
        // database gets there in the same call.
        if version == 9 {
            backfill_declared_sizes(&tx)?;
        }
        if version == 17 {
            backfill_refs(&tx)?;
        }
        if version == 32 {
            profiles::backfill_icons(&tx)?;
        }
        if version == 35 {
            backfill_declared_posts(&tx)?;
        }
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

/// Whether a key certified before `before_ms` signed `obj`. Never true of
/// an object the identity key signed itself: that key is never revoked.
fn signed_before(obj: &SignedObject, before_ms: i64) -> bool {
    let Some(cert) = obj.certificate() else {
        return false;
    };
    match cert.verify() {
        Ok(Payload::Key(key)) => {
            osp_proto::parse_rfc3339_ms(&key.published).is_some_and(|cert_ms| cert_ms < before_ms)
        }
        _ => false,
    }
}

/// Records the sizes a post declares for its media and thumbnails, and who
/// posted them. If two people post the same file, the first is kept.
/// Notes the sizes `note` declares for its blobs, and `post`, its ID, as
/// the first post to declare each: none before migration 35.
fn declare_sizes(conn: &Connection, note: &Note, post: Option<&ObjectId>) -> Result<()> {
    for (blob, size) in note.sized_blobs() {
        let (blob, size, author) = (blob.to_string(), size as i64, note.actor.to_string());
        match post {
            Some(post) => conn.execute(
                "INSERT INTO declared_sizes (blob, size, author, post) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(blob) DO UPDATE SET size = MAX(size, excluded.size),
                     author = COALESCE(author, excluded.author),
                     post = COALESCE(post, excluded.post)",
                params![blob, size, author, post.to_string()],
            )?,
            None => conn.execute(
                "INSERT INTO declared_sizes (blob, size, author) VALUES (?1, ?2, ?3)
                 ON CONFLICT(blob) DO UPDATE SET size = MAX(size, excluded.size),
                     author = COALESCE(author, excluded.author)",
                params![blob, size, author],
            )?,
        };
    }
    Ok(())
}

/// Names the first post stored that declares each blob, for the posts
/// stored before `declared_sizes` had them.
fn backfill_declared_posts(conn: &Connection) -> Result<()> {
    let mut stmt =
        conn.prepare("SELECT signed FROM objects WHERE kind = 'Note' ORDER BY received_ms")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for json in rows {
        let obj: SignedObject = serde_json::from_str(&json?).context("corrupt stored object")?;
        if let Ok(Payload::Note(note)) = obj.verify() {
            for (blob, _) in note.sized_blobs() {
                conn.execute(
                    "UPDATE declared_sizes SET post = ?2 WHERE blob = ?1 AND post IS NULL",
                    params![blob.to_string(), obj.id().to_string()],
                )?;
            }
        }
    }
    Ok(())
}

/// A reshare that just arrived is an event (ADR 0036 §1) if it was made in
/// the last day, of a post this node holds: a follow or a restore that
/// brings old reshares isn't someone resharing now.
fn note_reshare(tx: &Connection, a: &Announce, now: i64) -> Result<()> {
    let day = (EVENT_HOURS * 60 * 60 * 1000) as i64;
    let recent = osp_proto::parse_rfc3339_ms(&a.published)
        .is_some_and(|at| at > now - day && at <= now + day);
    let held: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM objects WHERE id = ?1)",
        params![a.object.to_string()],
        |row| row.get(0),
    )?;
    if recent && held {
        let what = EventWhat::Reshared {
            from: a.actor,
            author: a.object_actor,
            post: a.object,
        };
        events::add(tx, what, now)?;
    }
    Ok(())
}

/// Fills in `declared_sizes` from the posts stored before it existed.
fn backfill_declared_sizes(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT signed FROM objects WHERE kind = 'Note'")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for json in rows {
        let obj: SignedObject = serde_json::from_str(&json?).context("corrupt stored object")?;
        // Checked when it arrived; one that fails today's checks gets no sizes.
        if let Ok(Payload::Note(note)) = obj.verify() {
            declare_sizes(conn, &note, None)?;
        }
    }
    Ok(())
}

/// The words of a profile that a search looks through.
fn profile_text(person: &Person) -> String {
    match &person.summary {
        Some(summary) => format!("{}\n{summary}", person.name),
        None => person.name.clone(),
    }
}

/// Puts `text` in the search index for one post or profile, in place of what
/// the index held for it. `published_ms` is a post's date, `None` for a
/// profile.
fn index_for_search(
    conn: &Connection,
    key: &str,
    text: &str,
    published_ms: Option<i64>,
) -> Result<()> {
    let id: i64 = conn.query_row(
        "INSERT INTO search_items (key, published_ms) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET published_ms = excluded.published_ms
         RETURNING id",
        params![key, published_ms],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO search_text (rowid, text) VALUES (?1, ?2)",
        params![id, text],
    )?;
    Ok(())
}

fn remove_from_search(conn: &Connection, key: &str) -> Result<()> {
    let id: Option<i64> = conn
        .query_row(
            "DELETE FROM search_items WHERE key = ?1 RETURNING id",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = id {
        conn.execute("DELETE FROM search_text WHERE rowid = ?1", params![id])?;
    }
    Ok(())
}

/// Builds the search index if it's missing, from the posts and profiles
/// stored: on a database from before it, or one it was dropped from.
fn build_search_index(conn: &Connection) -> Result<()> {
    let built: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name = 'search_text')",
        [],
        |row| row.get(0),
    )?;
    if built {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(SEARCH_SCHEMA)?;
    let mut indexed = 0;
    {
        let mut stmt = tx.prepare(
            "SELECT signed, published_ms FROM objects
             WHERE kind = 'Note' AND id NOT IN (SELECT id FROM opened_posts)
             UNION ALL SELECT signed, NULL FROM profiles
             WHERE actor NOT IN (SELECT actor FROM opened_people)",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })?;
        for row in rows {
            let (json, published_ms) = row?;
            let obj: SignedObject = serde_json::from_str(&json).context("corrupt stored object")?;
            // Checked when it arrived; one that fails today's checks can't be
            // shown, so it isn't indexed.
            match obj.verify() {
                Ok(Payload::Note(note)) => {
                    index_for_search(&tx, &obj.id().to_string(), &note.content, published_ms)?;
                }
                Ok(Payload::Person(person)) => {
                    index_for_search(&tx, &person.actor.to_string(), &profile_text(&person), None)?;
                }
                _ => continue,
            }
            indexed += 1;
        }
    }
    tx.commit()?;
    if indexed > 0 {
        info!(indexed, "built the search index");
    }
    Ok(())
}

/// What a person typed, as an FTS5 query: every word has to start a word in
/// the text, so `table` finds "tables". FTS5 has a query syntax of its own,
/// so each word goes in quotes, which leaves `OR`, `NEAR(`, `text:` and `*`
/// plain words. `None` if nothing was typed.
fn search_query(words: &str) -> Option<String> {
    let words: Vec<String> = words
        .split_whitespace()
        .take(MAX_SEARCH_WORDS)
        // SQLite reads a NUL as the end of the query, leaving a quote open.
        .map(|word| word.replace(char::is_control, ""))
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{}\"*", word.replace('"', "\"\"")))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

fn format_ms(ms: i64) -> String {
    let t = UNIX_EPOCH + std::time::Duration::from_millis(ms.max(0) as u64);
    humantime::format_rfc3339_seconds(t).to_string()
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;
    use osp_proto::{Attachment, EndpointBinding};

    use super::*;

    #[test]
    fn blob_class_only_moves_up() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let h = BlobHash::from_bytes([7; 32]);
        db.put_blob(&h, BlobClass::Cache, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Cache));
        db.put_blob(&h, BlobClass::Prefetch, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Prefetch));
        db.put_blob(&h, BlobClass::Cache, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Prefetch));
        db.put_blob(&h, BlobClass::Replica, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Replica));
        db.put_blob(&h, BlobClass::Prefetch, 10).unwrap();
        assert_eq!(db.blob_class(&h).unwrap(), Some(BlobClass::Replica));
    }

    #[test]
    fn the_swarm_lists_followers_that_share_in_random_order_and_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let blob = BlobHash::from_bytes([5; 32]);
        let sharing: Vec<EndpointAddr> = (0..3).map(|_| some_addr()).collect();
        for addr in &sharing {
            db.add_follower(addr, true).unwrap();
            db.add_swarm_member(&blob, &addr.id).unwrap();
        }
        // One that doesn't share, and one that isn't a follower any more.
        let quiet = some_addr();
        db.add_follower(&quiet, false).unwrap();
        let gone = some_addr();
        for node in [quiet.id, gone.id] {
            db.add_swarm_member(&blob, &node).unwrap();
        }
        assert!(db.follower_shares(&sharing[0].id).unwrap());
        assert!(!db.follower_shares(&quiet.id).unwrap());
        assert!(!db.follower_shares(&gone.id).unwrap());

        let members = |except: &EndpointId| -> Vec<EndpointId> {
            let members = db.swarm_members(&blob, except).unwrap();
            members.into_iter().map(|m| m.addr.id).collect()
        };
        let listed = members(&sharing[0].id);
        assert_eq!(listed.len(), 2, "{listed:?}");
        assert!(listed.contains(&sharing[1].id) && listed.contains(&sharing[2].id));
        assert_eq!(members(&quiet.id).len(), 3);
        let other = BlobHash::from_bytes([6; 32]);
        assert!(db.swarm_members(&other, &quiet.id).unwrap().is_empty());
        // Not always in the same order, so the same ones aren't always named.
        let first = members(&quiet.id)[0];
        assert!((0..50).any(|_| members(&quiet.id)[0] != first));

        for _ in 0..MAX_SWARM_PER_BLOB + 5 {
            let addr = some_addr();
            db.add_follower(&addr, true).unwrap();
            db.add_swarm_member(&blob, &addr.id).unwrap();
        }
        let kept: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM swarm", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, MAX_SWARM_PER_BLOB);
    }

    #[test]
    fn a_full_swarm_keeps_the_longest_followers_and_those_noted_last() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let blob = BlobHash::from_bytes([5; 32]);
        // A follower that shares, following us since `day`.
        let follower = |day: i64| {
            let addr = some_addr();
            db.add_follower(&addr, true).unwrap();
            let since = params![addr.id.to_string(), day * DAY_MS];
            let sql = "UPDATE followers SET since = ?2 WHERE node = ?1";
            db.conn().execute(sql, since).unwrap();
            addr
        };
        // Notes a follower now, then dates it `order`, so those noted
        // later are noted after it. Says whether it was kept.
        let add = |addr: &EndpointAddr, order: i64| {
            db.add_swarm_member(&blob, &addr.id).unwrap();
            let sql = "UPDATE swarm SET seen_ms = ?2 WHERE node = ?1";
            let seen = params![addr.id.to_string(), order];
            db.conn().execute(sql, seen).unwrap();
            db.in_swarm(&blob, &addr.id).unwrap()
        };
        let kept = |addrs: &[EndpointAddr]| -> Vec<bool> {
            addrs
                .iter()
                .map(|a| db.in_swarm(&blob, &a.id).unwrap())
                .collect()
        };
        let count = || -> i64 {
            let sql = "SELECT COUNT(*) FROM swarm WHERE blob = ?1";
            let key = params![blob.to_string()];
            db.conn().query_row(sql, key, |row| row.get(0)).unwrap()
        };
        // Full, noted in the order they followed.
        let members: Vec<EndpointAddr> = (1..=MAX_SWARM_PER_BLOB).map(follower).collect();
        for (i, member) in members.iter().enumerate() {
            assert!(add(member, i as i64 + 1));
        }

        // A follower following for less time than all of them is kept, as
        // the last noted, in place of the one noted longest ago of the
        // half kept for that.
        let half = (MAX_SWARM_PER_BLOB / 2) as usize;
        let late = follower(MAX_SWARM_PER_BLOB + 1);
        assert!(add(&late, 1_000));
        assert_eq!(kept(&members[half..half + 1]), [false]);
        // One following for longer than all of them takes a place among
        // those that have followed longest; the newest of those moves to
        // the other half, in place of the one noted longest ago there.
        let early = follower(0);
        assert!(add(&early, 1_001));
        assert_eq!(kept(&members[half - 1..half]), [false]);
        assert_eq!(count(), MAX_SWARM_PER_BLOB);

        // A crowd of new followers takes only the half noted last.
        let crowd: Vec<EndpointAddr> = (0..60).map(|d| follower(1_000 + d)).collect();
        for (i, newcomer) in crowd.iter().enumerate() {
            assert!(add(newcomer, 2_000 + i as i64));
        }
        assert!(db.in_swarm(&blob, &early.id).unwrap());
        assert!(kept(&members[..half - 1]).iter().all(|&k| k));
        assert!(kept(&crowd[..10]).iter().all(|&k| !k));
        assert!(kept(&crowd[10..]).iter().all(|&k| k));
        assert_eq!(count(), MAX_SWARM_PER_BLOB);

        // A member that stopped sharing gives up its place to anyone, in
        // either half.
        db.add_follower(&members[0], false).unwrap();
        assert!(add(&follower(2_000), 3_000));
        assert!(!db.in_swarm(&blob, &members[0].id).unwrap());
        db.add_follower(&crowd[59], false).unwrap();
        assert!(add(&follower(2_001), 3_001));
        assert_eq!(kept(&crowd[58..]), [true, false]);
        assert_eq!(count(), MAX_SWARM_PER_BLOB);

        // Noted again, a member keeps the time it was first noted.
        let seen = |node: &EndpointId| -> i64 {
            let sql = "SELECT seen_ms FROM swarm WHERE blob = ?1 AND node = ?2";
            let keys = params![blob.to_string(), node.to_string()];
            db.conn().query_row(sql, keys, |row| row.get(0)).unwrap()
        };
        let sql = "UPDATE swarm SET seen_ms = 1 WHERE node = ?1";
        db.conn()
            .execute(sql, params![early.id.to_string()])
            .unwrap();
        db.add_swarm_member(&blob, &early.id).unwrap();
        assert_eq!(seen(&early.id), 1);
    }

    #[test]
    fn a_full_swarm_keeps_up_to_a_quarter_for_trusted_followers() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let blob = BlobHash::from_bytes([5; 32]);
        // A follower that shares, following us since `day`, noted as
        // holding a copy at `order`, and trusted or not.
        let member = |day: i64, order: i64, trusted: bool| {
            let addr = some_addr();
            db.add_follower(&addr, true).unwrap();
            let sql = "UPDATE followers SET since = ?2 WHERE node = ?1";
            let since = params![addr.id.to_string(), day * DAY_MS];
            db.conn().execute(sql, since).unwrap();
            if trusted {
                let actor = iroh::SecretKey::generate().public();
                db.trust_nodes(&actor, "a friend", &[addr.id], 0).unwrap();
            }
            db.add_swarm_member(&blob, &addr.id).unwrap();
            let sql = "UPDATE swarm SET seen_ms = ?2 WHERE node = ?1";
            let seen = params![addr.id.to_string(), order];
            db.conn().execute(sql, seen).unwrap();
            addr
        };
        let kept = |addrs: &[EndpointAddr]| -> usize {
            addrs
                .iter()
                .filter(|a| db.in_swarm(&blob, &a.id).unwrap())
                .count()
        };
        let members: Vec<EndpointAddr> = (1..=MAX_SWARM_PER_BLOB)
            .map(|day| member(day, day, false))
            .collect();
        // Trusted followers that followed later than all of them, and then
        // a crowd of new followers.
        let trusted: Vec<EndpointAddr> =
            (0..30).map(|i| member(200 + i, 1_000 + i, true)).collect();
        let crowd: Vec<EndpointAddr> = (0..60).map(|i| member(300 + i, 2_000 + i, false)).collect();

        // A quarter for the trusted followers that have followed longest;
        // of the rest, half for followers that have followed longest, and
        // half for the crowd, noted last. Trusted ones past the quarter
        // took their chances there, and the crowd came later.
        let longest = (MAX_SWARM_PER_BLOB - MAX_TRUSTED_SWARM_PER_BLOB) as usize / 2;
        let quarter = MAX_TRUSTED_SWARM_PER_BLOB as usize;
        assert_eq!(kept(&trusted[..quarter]), quarter);
        assert_eq!(kept(&trusted[quarter..]), 0);
        assert_eq!(kept(&members[..longest]), longest);
        assert_eq!(kept(&members[longest..]), 0);
        assert_eq!(
            kept(&crowd),
            MAX_SWARM_PER_BLOB as usize - quarter - longest
        );
        let in_swarm = db.swarm_members(&blob, &crowd[0].id).unwrap();
        assert_eq!(in_swarm.iter().filter(|m| m.trusted).count(), quarter);
    }

    #[test]
    fn requests_to_be_trusted_are_limited() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let follower = || {
            let addr = some_addr();
            db.add_follower(&addr, true).unwrap();
            addr.id
        };
        let ask = |node: EndpointId, note: &str, at_ms: i64| {
            let row = TrustRow {
                node,
                actor: iroh::SecretKey::generate().public(),
                name: "Sam".to_string(),
                note: note.to_string(),
                at_ms,
            };
            db.add_trust_request(&row, trust::MAX_NEW_REQUESTS_PER_HOUR)
                .unwrap()
        };
        let waiting = |now: i64| -> Vec<EndpointId> {
            db.trust_requests(now)
                .unwrap()
                .iter()
                .map(|r| r.node)
                .collect()
        };
        let start = 100 * DAY_MS;

        // One per node: asking again replaces the note, and keeps its place.
        let sam = follower();
        assert!(ask(sam, "hi", start));
        assert!(ask(sam, "Sam, from the climbing club", start + 1));
        let requests = db.trust_requests(start + 1).unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].note, "Sam, from the climbing club");
        assert_eq!(requests[0].at_ms, start);

        // Ten new ones an hour; a repeat isn't new.
        for i in 1..10 {
            assert!(ask(follower(), "", start + i));
        }
        assert!(!ask(follower(), "", start + 10));
        assert!(ask(sam, "again", start + 11));
        assert_eq!(db.trust_request_count(start + 11).unwrap(), 10);

        // Fifty waiting: the oldest go, and the newest come first.
        let mut last = sam;
        for hour in 1..=5 {
            for i in 0..9 {
                last = follower();
                assert!(ask(last, "", start + hour * HOUR_MS + 10 + i));
            }
        }
        let now = start + 6 * HOUR_MS;
        assert_eq!(db.trust_request_count(now).unwrap(), trust::MAX_WAITING);
        assert!(!waiting(now).contains(&sam));
        assert_eq!(waiting(now)[0], last);

        // Declined, a node's requests are dropped for 30 days.
        let carol = waiting(now)[1];
        assert!(db.answer_trust(&carol, false, now).unwrap());
        assert!(!db.answer_trust(&carol, true, now).unwrap());
        let later = now + 6 * HOUR_MS;
        assert!(!ask(carol, "please", later));
        assert!(!waiting(later).contains(&carol));
        // Allowed, its requests change nothing.
        let dave = waiting(later)[0];
        assert!(db.answer_trust(&dave, true, later).unwrap());
        assert!(!ask(dave, "me again", later));
        assert_eq!(db.trusted().unwrap()[0].node, dave);
        assert!(!waiting(later).contains(&dave));

        // A request waiting 30 days goes; a node declined then may ask again.
        let month = later + trust::WAITING_KEPT.as_millis() as i64;
        assert!(waiting(month).is_empty());
        assert!(ask(carol, "please", month));
        assert_eq!(waiting(month), [carol]);
        // So does one from a node that no longer follows us, forgotten as silent.
        db.drop_silent_followers(i64::MAX).unwrap();
        assert!(waiting(month).is_empty());
    }

    fn some_addr() -> EndpointAddr {
        EndpointAddr::new(iroh::SecretKey::generate().public())
    }

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    /// Adds `n` peers to one group, the first with the oldest contact.
    fn add_peers(db: &Db, ours: bool, n: usize, from_ms: i64) -> Vec<EndpointId> {
        let conn = db.conn();
        let tx = conn.unchecked_transaction().unwrap();
        let mut nodes = Vec::new();
        for i in 0..n {
            let node = some_addr().id;
            tx.execute(
                "INSERT INTO peers (node, ours, first_ms, last_ms, counts) VALUES (?1, ?2, ?3, ?3, '{}')",
                params![node.to_string(), ours, from_ms + i as i64],
            )
            .unwrap();
            nodes.push(node);
        }
        tx.commit().unwrap();
        nodes
    }

    fn peer(db: &Db, node: &EndpointId, now: i64) -> Option<PeerRow> {
        db.peers(now).unwrap().into_iter().find(|p| p.node == *node)
    }

    fn count(c: &mut PeerCounts) {
        c.requests += 1.0;
    }

    #[test]
    fn peers_are_kept_in_two_groups_of_limited_size() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let now = now_ms();
        let max = peers::MAX_PER_GROUP;
        let ours = add_peers(&db, true, max, now - 1000 * max as i64);
        let theirs = add_peers(&db, false, max, now - 2000 * max as i64);

        // A new node this node contacted takes the place of the one in its
        // group with the oldest contact, however old the other group's are.
        let new = some_addr().id;
        db.update_peer(&new, true, now, count).unwrap();
        let rows = db.peers(now).unwrap();
        assert_eq!(rows.iter().filter(|p| p.ours).count(), max);
        assert_eq!(rows.iter().filter(|p| !p.ours).count(), max);
        assert!(peer(&db, &ours[0], now).is_none());
        assert!(peer(&db, &ours[1], now).is_some());
        assert!(peer(&db, &theirs[0], now).is_some());

        // A stranger the node then contacts moves into that group, and
        // makes room there too.
        db.update_peer(&theirs[5], true, now, count).unwrap();
        assert!(peer(&db, &theirs[5], now).unwrap().ours);
        assert!(peer(&db, &ours[1], now).is_none());
        let rows = db.peers(now).unwrap();
        assert_eq!(rows.iter().filter(|p| p.ours).count(), max);
        assert_eq!(rows.iter().filter(|p| !p.ours).count(), max - 1);

        // A node contacting us later doesn't leave that group, and a known
        // node doesn't push anyone out.
        db.update_peer(&theirs[5], false, now, count).unwrap();
        assert!(peer(&db, &theirs[5], now).unwrap().ours);
        assert!(peer(&db, &ours[2], now).is_some());

        // A new stranger fills the free place without pushing anyone out,
        // and the next one pushes out the stranger with the oldest contact.
        db.update_peer(&some_addr().id, false, now, count).unwrap();
        assert!(peer(&db, &theirs[0], now).is_some());
        db.update_peer(&some_addr().id, false, now, count).unwrap();
        assert!(peer(&db, &theirs[0], now).is_none());
        assert!(peer(&db, &ours[2], now).is_some());
    }

    #[test]
    fn a_nodes_later_endpoint_replaces_the_earlier_one() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let public = SecretKey::generate();
        let bound = |endpoint: &SecretKey, kind, since_ms| BoundEndpoint {
            binding: EndpointBinding::new(&public, endpoint, kind, since_ms),
            addr: EndpointAddr::new(endpoint.public()),
        };
        let (first, second) = (SecretKey::generate(), SecretKey::generate());

        assert!(
            db.keep_endpoint(&bound(&first, EndpointKind::Direct, 100))
                .unwrap()
        );
        let owner = db.direct_endpoint_owner(&first.public()).unwrap();
        assert_eq!(owner, Some(public.public()));
        assert_eq!(
            db.direct_endpoint_of(&public.public()).unwrap(),
            Some(first.public())
        );

        // An older one, resent, changes nothing.
        assert!(
            !db.keep_endpoint(&bound(&second, EndpointKind::Direct, 99))
                .unwrap()
        );
        assert_eq!(db.direct_endpoint_owner(&second.public()).unwrap(), None);

        // A later one takes its place, so the old one is let in no more.
        assert!(
            db.keep_endpoint(&bound(&second, EndpointKind::Direct, 101))
                .unwrap()
        );
        assert_eq!(db.direct_endpoint_owner(&first.public()).unwrap(), None);
        assert_eq!(
            db.direct_endpoint_of(&public.public()).unwrap(),
            Some(second.public())
        );

        // A media endpoint is never taken for a direct one, nor replaces it.
        let media = SecretKey::generate();
        assert!(
            db.keep_endpoint(&bound(&media, EndpointKind::Media, 200))
                .unwrap()
        );
        assert_eq!(db.direct_endpoint_owner(&media.public()).unwrap(), None);
        assert_eq!(
            db.direct_endpoint_of(&public.public()).unwrap(),
            Some(second.public())
        );
    }

    #[test]
    fn direct_contacts_are_kept_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let node = SecretKey::generate().public();
        assert!(!db.is_direct_contact(&node).unwrap());
        assert!(!db.remove_direct_contact(&node).unwrap());
        db.add_direct_contact(&node).unwrap();
        db.add_direct_contact(&node).unwrap();
        assert!(db.is_direct_contact(&node).unwrap());
        assert_eq!(db.direct_contacts().unwrap(), [node]);
        assert!(db.remove_direct_contact(&node).unwrap());
        assert!(!db.is_direct_contact(&node).unwrap());
    }

    #[test]
    fn devices_and_long_silent_peers_arent_kept() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let now = now_ms();
        let phone = some_addr().id;
        db.add_device(&phone, "phone", false).unwrap();
        db.update_peer(&phone, true, now, count).unwrap();
        assert!(db.peers(now).unwrap().is_empty());

        let forget = peers::FORGET_AFTER.as_millis() as i64;
        let silent = add_peers(&db, true, 2, now - forget - 1);
        let recent = add_peers(&db, true, 1, now - forget + DAY_MS);
        // Listing forgets the silent ones...
        assert!(peer(&db, &silent[0], now).is_none());
        assert!(peer(&db, &recent[0], now).is_some());
        // ...and so does a new row.
        let silent = add_peers(&db, false, 1, now - forget - 1);
        db.update_peer(&some_addr().id, false, now, count).unwrap();
        let left: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM peers WHERE node = ?1",
                params![silent[0].to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
    }

    /// A phone that forgot its node and paired again with the same key
    /// (seen 2026-11-23): one device, paired at the new time.
    #[test]
    fn a_device_pairing_again_is_paired_at_the_new_time() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let phone = some_addr().id;
        db.add_device(&phone, "phone", false).unwrap();
        db.conn()
            .execute("UPDATE devices SET paired_ms = paired_ms - ?1", [DAY_MS])
            .unwrap();
        let first = db.devices().unwrap();
        db.add_device(&phone, "phone", false).unwrap();
        let again = db.devices().unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].0, phone);
        assert_ne!(again[0].2, first[0].2);
    }

    #[test]
    fn peer_counts_fade_before_they_are_added_to_and_when_read() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let start = now_ms() - 28 * DAY_MS;
        let node = some_addr().id;
        db.update_peer(&node, true, start, |c| c.requests += 4.0)
            .unwrap();
        db.update_peer(&node, false, start + 14 * DAY_MS, count)
            .unwrap();
        let row = peer(&db, &node, start + 28 * DAY_MS).unwrap();
        // 4 faded to 2, plus 1, all faded by half again.
        assert!((row.counts.requests - 1.5).abs() < 1e-9, "{row:?}");
        assert_eq!(row.first_ms, start);
        assert_eq!(row.last_ms, start + 14 * DAY_MS);
        assert!(row.ours);
    }

    #[test]
    fn a_database_from_before_migrations_is_updated_and_keeps_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let old = some_addr();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO followers (node, node_addr, since) VALUES (?1, ?2, 1)",
                params![old.id.to_string(), serde_json::to_string(&old).unwrap()],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert!(db.is_follower(&old.id).unwrap());
        db.follower_seen(&old.id).unwrap();
        drop(db);
        // Opening again doesn't run the migrations twice.
        let db = Db::open(&path).unwrap();
        assert_eq!(db.follower_count().unwrap(), 1);
    }

    #[test]
    fn silent_followers_are_dropped_and_new_ones_counted() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (quiet, chatty) = (some_addr(), some_addr());
        db.add_follower(&quiet, false).unwrap();
        db.add_follower(&chatty, false).unwrap();
        let start = now_ms();
        assert_eq!(db.new_followers_since(start - 60_000).unwrap(), 2);
        // Only the quiet one was last heard from long ago.
        db.conn()
            .execute(
                "UPDATE followers SET since = 1, seen_ms = 1 WHERE node = ?1",
                params![quiet.id.to_string()],
            )
            .unwrap();
        assert_eq!(db.new_followers_since(start - 60_000).unwrap(), 1);
        assert_eq!(db.drop_silent_followers(start - 60_000).unwrap(), 1);
        assert!(!db.is_follower(&quiet.id).unwrap());
        assert!(db.is_follower(&chatty.id).unwrap());
    }

    #[test]
    fn followers_are_told_of_a_move_until_they_hear_or_subscribe() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (a, b, c) = (some_addr(), some_addr(), some_addr());
        for f in [&a, &b, &c] {
            db.add_follower(f, false).unwrap();
        }
        assert!(db.followers_to_tell_moved().unwrap().is_empty());
        assert_eq!(db.mark_followers_to_tell_moved().unwrap(), 3);
        db.told_moved(&a.id).unwrap();
        // Subscribing to this node means b already knows where we are.
        db.add_follower(&b, false).unwrap();
        let left: Vec<_> = db.followers_to_tell_moved().unwrap();
        assert_eq!(left.iter().map(|f| f.id).collect::<Vec<_>>(), [c.id]);
    }

    #[test]
    fn a_follow_that_moves_to_another_node_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let actor = iroh::SecretKey::generate().public();
        let node = some_addr();
        db.upsert_follow(&actor, &node).unwrap();
        db.set_follow_synced(&actor, 50).unwrap();
        db.set_follow_error(&actor, "unreachable").unwrap();

        // The same node at a new address keeps its place.
        let relay: iroh::RelayUrl = "https://relay.example.org".parse().unwrap();
        db.upsert_follow(&actor, &node.clone().with_relay_url(relay))
            .unwrap();
        let [follow] = &db.follows().unwrap()[..] else {
            panic!("one follow expected")
        };
        assert_eq!((follow.cursor, follow.synced), (50, true));

        // Another node starts again from the latest entries.
        let other = some_addr();
        db.upsert_follow(&actor, &other).unwrap();
        let [follow] = &db.follows().unwrap()[..] else {
            panic!("one follow expected")
        };
        assert_eq!(follow.addr.id, other.id);
        assert_eq!((follow.cursor, follow.synced), (0, false));
        assert_eq!(db.follow_infos().unwrap()[0].last_error, None);
    }

    fn note_with_media(
        key: &iroh::SecretKey,
        media: &[(BlobHash, u64)],
    ) -> (SignedObject, Payload) {
        let attachment = media
            .iter()
            .map(|&(blob, size)| osp_proto::Attachment {
                kind: osp_proto::MediaKind::Video,
                media_type: "video/mp4".into(),
                blob,
                size,
                width: 1,
                height: 1,
                duration_ms: None,
                name: None,
                thumbnail: None,
                profile: "v1-720p".into(),
            })
            .collect();
        let payload = Payload::Note(Note {
            actor: key.public(),
            content: "media".into(),
            published: osp_proto::now_rfc3339(),
            attachment,
            in_reply_to: None,
        });
        (SignedObject::sign(key, &payload).unwrap(), payload)
    }

    #[test]
    fn declared_sizes_come_from_new_posts_and_posts_stored_before() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let key = iroh::SecretKey::generate();
        let (old, new) = (BlobHash::from_bytes([1; 32]), BlobHash::from_bytes([2; 32]));
        {
            // A database from before migration 2, holding a post with media.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            let (obj, _) = note_with_media(&key, &[(old, 500)]);
            conn.execute(
                "INSERT INTO objects (id, actor, kind, published_ms, received_ms, signed)
                 VALUES (?1, ?2, 'Note', 1, 1, ?3)",
                params![
                    obj.id().to_string(),
                    key.public().to_string(),
                    serde_json::to_string(&obj).unwrap()
                ],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.declared_size(&old).unwrap(), Some(500));
        assert_eq!(db.declared_size(&new).unwrap(), None);
        assert_eq!(db.blob_author(&old).unwrap(), Some(key.public()));

        let (obj, payload) = note_with_media(&key, &[(new, 300)]);
        db.insert_object(&obj, &payload).unwrap();
        assert_eq!(db.declared_size(&new).unwrap(), Some(300));
        // Another post can't shrink it, only raise it.
        let (obj, payload) = note_with_media(&key, &[(new, 10), (old, 900)]);
        db.insert_object(&obj, &payload).unwrap();
        assert_eq!(db.declared_size(&new).unwrap(), Some(300));
        assert_eq!(db.declared_size(&old).unwrap(), Some(900));
        // The first to post it stays its author.
        let other = iroh::SecretKey::generate();
        let (obj, payload) = note_with_media(&other, &[(new, 300)]);
        db.insert_object(&obj, &payload).unwrap();
        assert_eq!(db.blob_author(&new).unwrap(), Some(key.public()));
        assert_eq!(
            db.blob_author(&BlobHash::from_bytes([3; 32])).unwrap(),
            None
        );
    }

    #[test]
    fn a_database_from_before_authors_learns_who_posted_each_blob() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let key = iroh::SecretKey::generate();
        let blob = BlobHash::from_bytes([1; 32]);
        {
            // A database from before migration 9, holding a post with media.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            for sql in &MIGRATIONS[..8] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 8).unwrap();
            let (obj, _) = note_with_media(&key, &[(blob, 500)]);
            conn.execute(
                "INSERT INTO objects (id, actor, kind, published_ms, received_ms, signed)
                 VALUES (?1, ?2, 'Note', 1, 1, ?3)",
                params![
                    obj.id().to_string(),
                    key.public().to_string(),
                    serde_json::to_string(&obj).unwrap()
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO declared_sizes (blob, size) VALUES (?1, 500)",
                params![blob.to_string()],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.declared_size(&blob).unwrap(), Some(500));
        assert_eq!(db.blob_author(&blob).unwrap(), Some(key.public()));
    }

    #[test]
    fn held_posts_join_the_feed_on_top_when_shown_or_when_their_time_is_up() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let key = iroh::SecretKey::generate();
        let ids = |db: &Db| -> Vec<ObjectId> {
            let feed = db.feed(&[key.public()], 10).unwrap();
            feed.iter().map(|o| o.id()).collect()
        };
        let post = |text: &str| {
            let payload = Payload::Note(Note {
                actor: key.public(),
                content: text.into(),
                published: osp_proto::now_rfc3339(),
                attachment: vec![],
                in_reply_to: None,
            });
            (SignedObject::sign(&key, &payload).unwrap(), payload)
        };
        // Times are in milliseconds; keep steps that must be ordered apart.
        let pause = || std::thread::sleep(std::time::Duration::from_millis(5));
        let far = now_ms() + DAY_MS;
        let (first, payload) = post("first");
        assert!(db.insert_object_held(&first, &payload, far).unwrap());
        let (second, payload) = post("second");
        db.insert_object(&second, &payload).unwrap();
        assert_eq!(ids(&db), vec![second.id()]);

        // Shown above the post that came after it, and only moved once.
        pause();
        db.show_held(&[first.id()]).unwrap();
        assert_eq!(ids(&db), vec![first.id(), second.id()]);
        pause();
        let (third, payload) = post("third");
        db.insert_object(&third, &payload).unwrap();
        db.show_held(&[first.id()]).unwrap();
        assert_eq!(ids(&db), vec![third.id(), first.id(), second.id()]);

        // One whose time is up is shown without being asked.
        let (fourth, payload) = post("fourth");
        db.insert_object_held(&fourth, &payload, now_ms() + 50)
            .unwrap();
        assert_eq!(ids(&db).len(), 3);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(ids(&db)[0], fourth.id());

        // And a restart shows them all.
        let (fifth, payload) = post("fifth");
        db.insert_object_held(&fifth, &payload, far).unwrap();
        assert_eq!(db.show_all_held().unwrap(), 1);
        assert_eq!(ids(&db)[0], fifth.id());
    }

    #[test]
    fn eviction_takes_cache_blobs_used_longest_ago_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let [own, replica, stale, recent, prefetched, prefetched_later] =
            [1u8, 2, 3, 4, 5, 6].map(|b| BlobHash::from_bytes([b; 32]));
        db.put_blob(&own, BlobClass::Own, 100).unwrap();
        db.put_blob(&replica, BlobClass::Replica, 100).unwrap();
        db.put_blob(&stale, BlobClass::Cache, 100).unwrap();
        db.put_blob(&recent, BlobClass::Cache, 100).unwrap();
        db.put_blob(&prefetched, BlobClass::Prefetch, 100).unwrap();
        db.put_blob(&prefetched_later, BlobClass::Prefetch, 100)
            .unwrap();
        db.conn()
            .execute("UPDATE blobs SET accessed_ms = 1", [])
            .unwrap();
        // Used longest ago, but prefetched copies go after cached ones.
        db.conn()
            .execute(
                "UPDATE blobs SET accessed_ms = 0 WHERE class = 'prefetch'",
                [],
            )
            .unwrap();
        db.touch_blob(&recent).unwrap();
        db.touch_blob(&prefetched_later).unwrap();
        assert_eq!(db.class_bytes(BlobClass::Cache).unwrap(), 200);
        assert_eq!(db.evictable_bytes().unwrap(), 400);

        assert_eq!(db.evict_cache(0).unwrap(), vec![]);
        assert_eq!(db.evict_cache(1).unwrap(), vec![stale]);
        assert_eq!(db.blob_class(&stale).unwrap(), None);
        assert_eq!(db.evict_cache(1).unwrap(), vec![recent]);
        assert_eq!(db.evict_cache(1).unwrap(), vec![prefetched]);
        // Asking for more than the cache holds empties the cache, no more.
        assert_eq!(db.evict_cache(1 << 30).unwrap(), vec![prefetched_later]);
        assert_eq!(db.evictable_bytes().unwrap(), 0);
        assert_eq!(db.blob_class(&own).unwrap(), Some(BlobClass::Own));
        assert_eq!(db.blob_class(&replica).unwrap(), Some(BlobClass::Replica));
    }

    fn signed_reshare(
        key: &SecretKey,
        post: &SignedObject,
        author: PublicKey,
    ) -> (SignedObject, Payload) {
        let payload = Payload::Announce(osp_proto::Announce {
            actor: key.public(),
            object: post.id(),
            object_actor: author,
            published: osp_proto::now_rfc3339(),
            hosting: false,
        });
        (SignedObject::sign(key, &payload).unwrap(), payload)
    }

    fn signed_delete(key: &SecretKey, object: ObjectId) -> (SignedObject, Delete) {
        let d = Delete {
            actor: key.public(),
            object,
            published: osp_proto::now_rfc3339(),
        };
        let obj = SignedObject::sign(key, &Payload::Delete(d.clone())).unwrap();
        (obj, d)
    }

    #[test]
    fn a_delete_takes_the_post_its_reshares_and_the_files_nothing_else_names() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (alice, bob, carol, dave) = (
            SecretKey::generate(),
            SecretKey::generate(),
            SecretKey::generate(),
            SecretKey::generate(),
        );
        let (only, shared) = (BlobHash::from_bytes([1; 32]), BlobHash::from_bytes([2; 32]));
        let (post, payload) = note_with_media(&alice, &[(only, 10), (shared, 10)]);
        assert!(db.insert_object(&post, &payload).unwrap());
        let (other, other_payload) = note_with_media(&dave, &[(shared, 10)]);
        db.insert_object(&other, &other_payload).unwrap();
        for blob in [only, shared] {
            db.put_blob(&blob, BlobClass::Prefetch, 10).unwrap();
        }
        let (bobs, bobs_payload) = signed_reshare(&bob, &post, alice.public());
        db.insert_object(&bobs, &bobs_payload).unwrap();
        let (mine, my_payload) = signed_reshare(&carol, &post, alice.public());
        db.insert_object(&mine, &my_payload).unwrap();
        assert!(db.is_named(&only).unwrap());

        // Carol's node hears Alice deleted it.
        let (delete, d) = signed_delete(&alice, post.id());
        let deleted = db
            .apply_delete(&delete, &d, false, &carol.public())
            .unwrap();
        assert_eq!(
            deleted,
            Some(Deleted {
                removed: true,
                files: vec![only],
                ours: true,
                unhosted: Vec::new(),
            })
        );
        for id in [post.id(), bobs.id(), mine.id()] {
            assert_eq!(db.get_object(&id).unwrap(), None);
        }
        assert_eq!(db.blob_class(&only).unwrap(), None);
        assert!(!db.is_named(&only).unwrap());
        // Dave's post names the other file, so it stays.
        assert_eq!(db.blob_class(&shared).unwrap(), Some(BlobClass::Prefetch));
        assert!(db.get_object(&other.id()).unwrap().is_some());
        assert!(
            db.search_posts("media", 10)
                .unwrap()
                .iter()
                .all(|o| o.id() != post.id())
        );

        // Once is enough, and an old copy, or a reshare of it, stays out.
        assert_eq!(
            db.apply_delete(&delete, &d, false, &carol.public())
                .unwrap(),
            None
        );
        assert!(!db.insert_object(&post, &payload).unwrap());
        assert!(!db.insert_object(&bobs, &bobs_payload).unwrap());
        assert_eq!(db.get_object(&post.id()).unwrap(), None);
        assert_eq!(db.delete_by_id(&delete.id()).unwrap(), Some(delete.clone()));
        assert_eq!(
            db.delete_of(&post.id(), &alice.public()).unwrap(),
            Some(delete)
        );
        assert_eq!(db.delete_of(&post.id(), &bob.public()).unwrap(), None);
    }

    #[test]
    fn blocking_a_person_drops_what_is_held_of_them_and_keeps_them_out() {
        use osp_proto::rpc::BlockTarget;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = Db::open(&path).unwrap();
        // Alice's node; Kim is blocked; Bob reshared Kim's post, and so
        // did Alice.
        let (alice, kim, bob) = (
            SecretKey::generate(),
            SecretKey::generate(),
            SecretKey::generate(),
        );
        let file = BlobHash::from_bytes([7; 32]);
        let (post, payload) = note_with_media(&kim, &[(file, 10)]);
        db.insert_object(&post, &payload).unwrap();
        db.put_blob(&file, BlobClass::Prefetch, 10).unwrap();
        let (bobs, bobs_payload) = signed_reshare(&bob, &post, kim.public());
        db.insert_object(&bobs, &bobs_payload).unwrap();
        let (hers, her_payload) = signed_reshare(&alice, &post, kim.public());
        db.insert_object(&hers, &her_payload).unwrap();
        let bobs_own = store_note(&db, &bob, "oak", EARLIER);
        let (person, profile) = signed_person(&kim, "Kim", None);
        db.upsert_profile(&person, &profile).unwrap();
        db.upsert_follow(&kim.public(), &some_addr()).unwrap();

        let kim_block = BlockTarget::Account {
            actor: kim.public(),
        };
        let dropped = db.block(&kim_block, &alice.public()).unwrap();
        assert_eq!(dropped.posts, 2);
        assert_eq!(dropped.files, vec![file]);
        assert_eq!(db.get_object(&post.id()).unwrap(), None);
        assert_eq!(db.get_object(&bobs.id()).unwrap(), None);
        assert_eq!(db.blob_class(&file).unwrap(), None);
        // Alice's own reshare goes only if she deletes it; Bob's own post stays.
        assert!(db.get_object(&hers.id()).unwrap().is_some());
        assert!(db.get_object(&bobs_own).unwrap().is_some());
        assert_eq!(db.profile(&kim.public()).unwrap(), None);
        assert!(db.follows().unwrap().is_empty());
        assert_eq!(
            db.replier_choice(&kim.public()).unwrap(),
            Some(Choice::Blocked)
        );
        let listed = db.blocks().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].target, kim_block);
        assert_eq!(listed[0].name.as_deref(), Some("Kim"));

        // Kept out, after a restart too: posts, reshares and the profile.
        drop(db);
        let db = Db::open(&path).unwrap();
        assert!(!db.insert_object(&post, &payload).unwrap());
        assert!(!db.insert_object(&bobs, &bobs_payload).unwrap());
        db.upsert_profile(&person, &profile).unwrap();
        assert_eq!(db.profile(&kim.public()).unwrap(), None);
        // Blocking again drops nothing more.
        assert_eq!(
            db.block(&kim_block, &alice.public()).unwrap(),
            blocks::Dropped::default()
        );

        assert!(db.unblock(&kim_block).unwrap());
        assert!(!db.unblock(&kim_block).unwrap());
        assert_eq!(db.replier_choice(&kim.public()).unwrap(), None);
        assert!(db.insert_object(&post, &payload).unwrap());
        assert!(db.blocks().unwrap().is_empty());
    }

    #[test]
    fn blocking_a_post_or_a_file_drops_the_posts_it_names() {
        use osp_proto::rpc::BlockTarget;
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (alice, kim, bob) = (
            SecretKey::generate(),
            SecretKey::generate(),
            SecretKey::generate(),
        );
        let me = alice.public();

        // A post, and a reshare of it; Kim's other post stays.
        let (post, payload) = signed_note(&kim, "oak", EARLIER);
        db.insert_object(&post, &payload).unwrap();
        let (bobs, bobs_payload) = signed_reshare(&bob, &post, kim.public());
        db.insert_object(&bobs, &bobs_payload).unwrap();
        let other = store_note(&db, &kim, "ash", EARLIER);
        let dropped = db.block(&BlockTarget::Post { id: post.id() }, &me).unwrap();
        assert_eq!(dropped.posts, 2);
        assert_eq!(db.get_object(&post.id()).unwrap(), None);
        assert!(db.get_object(&other).unwrap().is_some());
        assert!(!db.insert_object(&bobs, &bobs_payload).unwrap());

        // A file: Kim's post naming it goes, and the file with it, even
        // though Alice's own post names it too, which stays.
        let file = BlobHash::from_bytes([9; 32]);
        let (kims, kims_payload) = note_with_media(&kim, &[(file, 10)]);
        db.insert_object(&kims, &kims_payload).unwrap();
        let (own, own_payload) = note_with_media(&alice, &[(file, 10)]);
        db.insert_object(&own, &own_payload).unwrap();
        db.put_blob(&file, BlobClass::Own, 10).unwrap();
        let target = BlockTarget::File { hash: file };
        let dropped = db.block(&target, &me).unwrap();
        assert_eq!(dropped.posts, 1);
        assert_eq!(dropped.files, vec![file]);
        assert_eq!(db.get_object(&kims.id()).unwrap(), None);
        assert!(db.get_object(&own.id()).unwrap().is_some());
        assert_eq!(db.blob_class(&file).unwrap(), None);
        assert!(db.is_blocked(&target).unwrap());
        assert!(!db.insert_object(&kims, &kims_payload).unwrap());
    }

    #[test]
    fn unblocking_a_post_or_a_file_asks_for_what_it_dropped_again() {
        use osp_proto::rpc::BlockTarget;
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (alice, kim) = (SecretKey::generate(), SecretKey::generate());
        let me = alice.public();

        // Kim's post with a file; Alice blocks the file, then unblocks it.
        let file = BlobHash::from_bytes([7; 32]);
        let (post, payload) = note_with_media(&kim, &[(file, 10)]);
        db.insert_object(&post, &payload).unwrap();
        let target = BlockTarget::File { hash: file };
        assert_eq!(db.block(&target, &me).unwrap().posts, 1);
        assert!(db.refetch_due(&kim.public(), 10).unwrap().is_empty());
        assert!(db.unblock(&target).unwrap());
        let due = db.refetch_due(&kim.public(), 10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, post.id());
        // Kim's node sends it back, and it's kept.
        assert!(db.insert_object(&post, &payload).unwrap());
        db.refetch_done(&[post.id()]).unwrap();

        // Blocked twice over, by itself and by its file: undoing one block
        // leaves the other keeping it out if a copy comes.
        db.block(&BlockTarget::Post { id: post.id() }, &me).unwrap();
        db.block(&target, &me).unwrap();
        assert!(db.unblock(&target).unwrap());
        assert!(!db.insert_object(&post, &payload).unwrap());

        // A person's block asks for nothing again: following them does.
        let other = store_note(&db, &kim, "ash", EARLIER);
        db.refetch_done(&[post.id()]).unwrap();
        let account = BlockTarget::Account {
            actor: kim.public(),
        };
        db.block(&account, &me).unwrap();
        assert!(db.unblock(&account).unwrap());
        assert!(db.refetch_due(&kim.public(), 10).unwrap().is_empty());
        assert_eq!(db.get_object(&other).unwrap(), None);
    }

    #[test]
    fn only_a_posts_author_can_delete_it_and_a_reshares_only_the_resharer() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (alice, bob) = (SecretKey::generate(), SecretKey::generate());
        let me = SecretKey::generate().public();
        let (post, payload) = signed_note(&alice, "oak", EARLIER);
        db.insert_object(&post, &payload).unwrap();
        let (reshare, reshare_payload) = signed_reshare(&bob, &post, alice.public());
        db.insert_object(&reshare, &reshare_payload).unwrap();

        let (forged, d) = signed_delete(&bob, post.id());
        assert!(db.apply_delete(&forged, &d, true, &me).is_err());
        assert!(db.get_object(&post.id()).unwrap().is_some());
        assert!(!db.is_tombstoned(&post.id()).unwrap());

        // Bob deleting his reshare takes only that.
        let (delete, d) = signed_delete(&bob, reshare.id());
        let deleted = db.apply_delete(&delete, &d, false, &me).unwrap().unwrap();
        assert!(deleted.removed && !deleted.ours && deleted.files.is_empty());
        assert_eq!(db.get_object(&reshare.id()).unwrap(), None);
        assert!(db.get_object(&post.id()).unwrap().is_some());
    }

    #[test]
    fn a_delete_in_an_outbox_is_kept_for_a_post_never_held() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let alice = SecretKey::generate();
        let me = SecretKey::generate().public();
        let (post, payload) = signed_note(&alice, "oak", EARLIER);
        let (delete, d) = signed_delete(&alice, post.id());

        // Through a forward, one for a post not held is let pass.
        assert_eq!(db.apply_delete(&delete, &d, false, &me).unwrap(), None);
        assert!(!db.is_tombstoned(&post.id()).unwrap());
        // From the author's outbox, it's kept, and a stale copy later is refused.
        let before = now_ms();
        let kept = db.apply_delete(&delete, &d, true, &me).unwrap().unwrap();
        assert!(!kept.removed);
        assert!(db.is_tombstoned(&post.id()).unwrap());
        // When it came is noted here, whatever date it gives itself.
        let at = db.deleted_at(&post.id()).unwrap().unwrap();
        assert!(at >= before && at <= now_ms());
        assert!(!db.insert_object(&post, &payload).unwrap());
    }

    #[test]
    fn a_database_from_before_deletes_learns_what_each_post_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let key = SecretKey::generate();
        let blob = BlobHash::from_bytes([1; 32]);
        {
            // A database from before migration 17, holding a post with media.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            for sql in &MIGRATIONS[..16] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 16).unwrap();
            let (obj, _) = note_with_media(&key, &[(blob, 500)]);
            conn.execute(
                "INSERT INTO objects (id, actor, kind, published_ms, received_ms, signed)
                 VALUES (?1, ?2, 'Note', 1, 1, ?3)",
                params![
                    obj.id().to_string(),
                    key.public().to_string(),
                    serde_json::to_string(&obj).unwrap()
                ],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert!(db.is_named(&blob).unwrap());
    }

    #[test]
    fn hosts_per_blob_are_capped() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let blob = BlobHash::from_bytes([3; 32]);
        for _ in 0..MAX_HOSTS_PER_BLOB + 5 {
            db.add_host(&blob, &some_addr()).unwrap();
        }
        // Inserts can share a millisecond; make the order unambiguous.
        db.conn()
            .execute("UPDATE hosts SET seen_ms = 1", [])
            .unwrap();
        let latest = some_addr();
        db.add_host(&blob, &latest).unwrap();
        let hosts = db.hosts(&blob).unwrap();
        assert_eq!(hosts.len(), MAX_HOSTS_PER_BLOB as usize);
        assert!(hosts.contains(&latest));
        let stored: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM hosts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, MAX_HOSTS_PER_BLOB);
    }

    fn signed_note(key: &SecretKey, content: &str, published: &str) -> (SignedObject, Payload) {
        let payload = Payload::Note(Note {
            actor: key.public(),
            content: content.into(),
            published: published.into(),
            attachment: vec![],
            in_reply_to: None,
        });
        (SignedObject::sign(key, &payload).unwrap(), payload)
    }

    fn store_note(db: &Db, key: &SecretKey, content: &str, published: &str) -> ObjectId {
        let (obj, payload) = signed_note(key, content, published);
        assert!(db.insert_object(&obj, &payload).unwrap());
        obj.id()
    }

    fn signed_person(key: &SecretKey, name: &str, summary: Option<&str>) -> (Person, SignedObject) {
        let person = Person {
            actor: key.public(),
            name: name.into(),
            summary: summary.map(String::from),
            discoverable: false,
            nodes: vec![],
            media_endpoints: vec![],
            published: osp_proto::now_rfc3339(),
            icon: None,
            attachment: Vec::new(),
        };
        let obj = SignedObject::sign(key, &Payload::Person(person.clone())).unwrap();
        (person, obj)
    }

    fn a_machine_record(byte: u8) -> (EndpointId, MachineRecord) {
        let key = SecretKey::from_bytes(&[byte; 32]);
        let record = MachineRecord::new(&key, vec![], "2026-11-01T00:00:00Z".into()).unwrap();
        (key.public(), record)
    }

    fn map_has(db: &Db, machine: &EndpointId) -> bool {
        db.heard_machine(machine, 0).unwrap().is_some()
    }

    fn numbers(db: &Db, since: u64) -> Vec<u64> {
        let (entries, _) = db.listing_since(since, 100).unwrap();
        entries.iter().map(|entry| entry.seq).collect()
    }

    #[test]
    fn a_page_of_the_lists_stops_at_its_size_with_at_least_one_entry() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        assert_eq!(db.listing_since(0, 10).unwrap().1, 0);
        let key = SecretKey::from_bytes(&[1; 32]);
        let (_, profile) = signed_person(&key, "Alice", None);
        let (note, _) = signed_note(&key, &"x".repeat(9_000), EARLIER);
        let item = ListingItem::Person {
            profile,
            posts: vec![note; 2],
            reshares: vec![],
        };
        for byte in 2..12 {
            let actor = SecretKey::from_bytes(&[byte; 32]).public();
            db.put_listed(&actor, &item, 1, 0).unwrap();
        }

        let (page, latest) = db.listing_since(0, 100).unwrap();
        assert!((1..10).contains(&page.len()), "{} entries", page.len());
        let bytes: usize = page
            .iter()
            .map(|entry| serde_json::to_vec(&entry.item).unwrap().len())
            .sum();
        assert!(bytes <= MAX_LISTING_PAGE_BYTES, "{bytes} bytes");
        assert_eq!(latest, 10);
        assert_eq!(db.listing_since(0, 2).unwrap().0.len(), 2);
        // The rest come with the next page, in order.
        let rest = numbers(&db, page.last().unwrap().seq);
        assert_eq!(rest[0], page.last().unwrap().seq + 1);
    }

    #[test]
    fn an_entry_takes_a_new_number_only_if_it_changed() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let key = SecretKey::from_bytes(&[1; 32]);
        let actor = key.public();
        let (_, profile) = signed_person(&key, "Alice", None);
        let (note, _) = signed_note(&key, "A post", EARLIER);
        db.add_listed(&actor, &actor).unwrap();
        let bare = ListingItem::Person {
            profile: profile.clone(),
            posts: vec![],
            reshares: vec![],
        };
        db.put_listed(&actor, &bare, 1, 0).unwrap();
        assert_eq!(numbers(&db, 0), [1]);
        db.put_listed(&actor, &bare, 2, 0).unwrap();
        assert_eq!(numbers(&db, 0), [1]);
        let with_post = ListingItem::Person {
            profile,
            posts: vec![note],
            reshares: vec![],
        };
        db.put_listed(&actor, &with_post, 3, 0).unwrap();
        assert_eq!(numbers(&db, 0), [2]);
        assert_eq!(db.listed_due(0).unwrap()[0].latest, 3);
    }

    #[test]
    fn the_newest_number_never_goes_down() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (machine, record) = a_machine_record(1);
        db.put_machine(&machine, &record, 100).unwrap();
        db.forget_silent_machines(101).unwrap();
        let (entries, latest) = db.listing_since(0, 10).unwrap();
        assert!(entries.is_empty());
        assert_eq!(latest, 1);
    }

    #[test]
    fn those_silent_too_long_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (answered, asked_lately, never) = (
            SecretKey::from_bytes(&[1; 32]).public(),
            SecretKey::from_bytes(&[2; 32]).public(),
            SecretKey::from_bytes(&[3; 32]).public(),
        );
        for actor in [answered, asked_lately, never] {
            db.add_listed(&actor, &actor).unwrap();
        }
        let now = now_ms();
        db.conn()
            .execute(
                "UPDATE listed SET asked_ms = ?1 WHERE actor != ?2",
                params![now - 1000, asked_lately.to_string()],
            )
            .unwrap();
        db.set_listed_read(&answered, 0, now).unwrap();

        assert_eq!(db.drop_silent_listed(now - 500).unwrap(), 1);
        assert_eq!(db.listed_node(&never).unwrap(), None);
        assert_eq!(db.listed_count().unwrap(), 2);
        let (entries, _) = db.listing_since(0, 10).unwrap();
        assert_eq!(entries[0].item, ListingItem::Gone { actor: never });

        // Nor do those never listed leave a mark.
        assert!(!db.drop_listed(&never).unwrap());
        assert_eq!(db.listing_since(0, 10).unwrap().0.len(), 1);

        let (quiet, record) = a_machine_record(4);
        let (heard, other) = a_machine_record(5);
        db.put_machine(&quiet, &record, now - 1000).unwrap();
        db.put_machine(&heard, &other, now - 1000).unwrap();
        db.heard_machine(&heard, now).unwrap();
        db.forget_silent_machines(now - 500).unwrap();
        assert!(!map_has(&db, &quiet));
        assert!(map_has(&db, &heard));
        let (entries, _) = db.listing_since(0, 10).unwrap();
        assert!(matches!(
            entries.last().unwrap().item,
            ListingItem::Machine { .. }
        ));
        assert_eq!(db.directory_counts().unwrap(), (2, 1));
    }

    #[test]
    fn a_full_map_loses_whichever_was_heard_from_longest_ago() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        {
            let conn = db.conn();
            let tx = conn.unchecked_transaction().unwrap();
            for i in 0..MAX_MAP {
                tx.execute(
                    "INSERT INTO map (machine, heard_ms, greeted_ms) VALUES (?1, ?2, 0)",
                    params![format!("{i:064x}"), 1000 + i],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let (oldest, record) = a_machine_record(1);
        db.put_machine(&oldest, &record, 10).unwrap();
        assert_eq!(db.directory_counts().unwrap().1 as i64, MAX_MAP);
        assert!(!map_has(&db, &oldest));

        // Something with a real place pushes out the longest quiet.
        let (newer, record) = a_machine_record(2);
        db.put_machine(&newer, &record, 1_000_000).unwrap();
        assert!(map_has(&db, &newer));
        assert_eq!(db.directory_counts().unwrap().1 as i64, MAX_MAP);
    }

    /// Someone found through a directory, with one post, as of `now`.
    fn found(db: &Db, directory: &EndpointId, byte: u8, text: &str, now: i64) -> PublicKey {
        let key = SecretKey::from_bytes(&[byte; 32]);
        let (person, profile) = signed_person(&key, "Someone", None);
        let (note, payload) = signed_note(&key, text, EARLIER);
        db.keep_found(directory, &person, &profile, &[(note, payload)], now)
            .unwrap();
        key.public()
    }

    #[test]
    fn those_found_are_trimmed_oldest_first_and_their_keys_go_with_them() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let directory = SecretKey::from_bytes(&[9; 32]).public();
        let oldest = found(&db, &directory, 1, "maple first", 100);
        let middle = found(&db, &directory, 2, "maple second", 200);
        let newest = found(&db, &directory, 3, "maple third", 300);
        db.conn()
            .execute(
                "INSERT INTO identity_keys (actor, cert, cert_ms) VALUES (?1, '', 0)",
                params![oldest.to_string()],
            )
            .unwrap();
        assert_eq!(db.found_count().unwrap(), 3);

        // Within limits, nothing goes.
        assert_eq!(db.trim_found(3, u64::MAX).unwrap(), 0);
        assert_eq!(found_posts(&db, "maple").len(), 3);

        assert_eq!(db.trim_found(2, u64::MAX).unwrap(), 1);
        assert_eq!(found_posts(&db, "maple").len(), 2);
        assert!(db.profile(&oldest).unwrap().is_none());
        assert!(db.found_through(&oldest).unwrap().is_none());
        let keys: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM identity_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(keys, 0);

        // By bytes too: there's room for one of them.
        let (_, one): (i64, i64) = db
            .conn()
            .query_row(
                "SELECT actor, bytes FROM found_people WHERE actor = ?1",
                params![newest.to_string()],
                |row| Ok((0, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(db.trim_found(10, one as u64).unwrap(), 1);
        assert!(db.profile(&middle).unwrap().is_none());
        assert!(db.profile(&newest).unwrap().is_some());

        assert_eq!(db.trim_found(0, 0).unwrap(), 1);
        assert_eq!(db.found_count().unwrap(), 0);
        assert!(found_posts(&db, "maple").is_empty());
        assert!(found_people(&db, "someone").is_empty());
    }

    #[test]
    fn someone_followed_or_a_post_kept_is_no_longer_only_found() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let directory = SecretKey::from_bytes(&[9; 32]).public();

        // Following them: it stays through trimming, and now says its media.
        let key = SecretKey::from_bytes(&[1; 32]);
        let (person, profile) = signed_person(&key, "Followed", None);
        let blob = BlobHash::from_bytes([7; 32]);
        let with_media = Payload::Note(Note {
            actor: key.public(),
            content: "elm with a photo".into(),
            published: EARLIER.into(),
            attachment: vec![Attachment {
                kind: osp_proto::MediaKind::Image,
                media_type: "image/webp".into(),
                blob,
                size: 500,
                width: 1,
                height: 1,
                duration_ms: None,
                name: None,
                thumbnail: None,
                profile: "v1-720p".into(),
            }],
            in_reply_to: None,
        });
        let signed = SignedObject::sign(&key, &with_media).unwrap();
        db.keep_found(
            &directory,
            &person,
            &profile,
            &[(signed.clone(), with_media)],
            100,
        )
        .unwrap();
        // Its media can be watched through a viewing endpoint (ADR 0024), so
        // its size is known, but only as a found post's.
        assert_eq!(db.declared_size(&blob).unwrap(), Some(500));
        assert_eq!(db.blob_author(&blob).unwrap(), None);
        assert!(db.found_only_post(&blob).unwrap().is_some());
        assert_eq!(
            db.post_found_through(&signed.id()).unwrap(),
            Some(directory)
        );
        db.upsert_follow(&key.public(), &EndpointAddr::new(key.public()))
            .unwrap();
        db.found_person_is_followed(&key.public()).unwrap();
        assert_eq!(db.declared_size(&blob).unwrap(), Some(500));
        assert_eq!(db.blob_author(&blob).unwrap(), Some(key.public()));
        assert!(db.found_only_post(&blob).unwrap().is_none());
        assert_eq!(db.post_found_through(&signed.id()).unwrap(), None);
        db.trim_found(0, 0).unwrap();
        assert_eq!(found_posts(&db, "elm").len(), 1);
        assert_eq!(found_people(&db, "followed").len(), 1);

        // A post a reshare brings as well stays when the rest of them goes,
        // and their profile with it, as the reshare shows their name.
        let carried = found(&db, &directory, 2, "ash carried", 100);
        let (note, payload) = signed_note(&SecretKey::from_bytes(&[2; 32]), "ash carried", EARLIER);
        assert!(!db.insert_object(&note, &payload).unwrap());
        db.trim_found(0, 0).unwrap();
        assert_eq!(found_posts(&db, "ash").len(), 1);
        assert!(db.profile(&carried).unwrap().is_some());

        // So does one the owner reshares.
        let kept = SecretKey::from_bytes(&[3; 32]);
        found(&db, &directory, 3, "birch reshared", 100);
        let (note, _) = signed_note(&kept, "birch reshared", EARLIER);
        db.found_post_stays(&note.id()).unwrap();
        db.trim_found(0, 0).unwrap();
        assert_eq!(found_posts(&db, "birch").len(), 1);
    }

    #[test]
    fn a_directory_can_take_back_only_what_it_gave() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (one, other) = (
            SecretKey::from_bytes(&[8; 32]).public(),
            SecretKey::from_bytes(&[9; 32]).public(),
        );
        let actor = found(&db, &one, 1, "cedar", 100);
        db.forget_found(&other, &actor).unwrap();
        assert_eq!(found_posts(&db, "cedar").len(), 1);
        db.forget_found(&one, &actor).unwrap();
        assert!(found_posts(&db, "cedar").is_empty());
        assert!(db.profile(&actor).unwrap().is_none());

        // Starting over drops what it gave, and how far it was read.
        found(&db, &one, 1, "cedar", 100);
        let stays = found(&db, &other, 2, "cedar again", 100);
        db.set_directory_cursor(&one, 40, 1).unwrap();
        db.forget_directory(&one).unwrap();
        assert_eq!(db.directory_cursor(&one, 1).unwrap(), 0);
        assert_eq!(found_posts(&db, "cedar"), ["cedar again"]);
        assert!(db.found_through(&stays).unwrap().is_some());
    }

    #[test]
    fn a_withdrawal_drops_the_found_person_and_only_the_newest_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let directory = SecretKey::from_bytes(&[8; 32]).public();
        let actor = found(&db, &directory, 1, "rowan", 100);
        db.found_person_withdrew(&actor, 500).unwrap();
        assert!(found_posts(&db, "rowan").is_empty());
        assert!(db.found_through(&actor).unwrap().is_none());
        assert_eq!(db.withdrawn_at(&actor).unwrap(), Some(500));
        // An older one doesn't move the date back.
        db.found_person_withdrew(&actor, 400).unwrap();
        assert_eq!(db.withdrawn_at(&actor).unwrap(), Some(500));

        // A full table: one more, and the one noted longest ago goes.
        {
            let conn = db.conn();
            let tx = conn.unchecked_transaction().unwrap();
            for i in 0..MAX_WITHDRAWN as i64 - 1 {
                tx.execute(
                    "INSERT INTO withdrawn_people (actor, published_ms, noted_ms)
                     VALUES (?1, 1, ?2)",
                    params![format!("filler-{i}"), 1_000 + i],
                )
                .unwrap();
            }
            tx.execute(
                "UPDATE withdrawn_people SET noted_ms = 1 WHERE actor = ?1",
                params![actor.to_string()],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let newest = SecretKey::from_bytes(&[2; 32]).public();
        db.found_person_withdrew(&newest, 600).unwrap();
        assert_eq!(db.withdrawn_at(&actor).unwrap(), None);
        assert_eq!(db.withdrawn_at(&newest).unwrap(), Some(600));
        let kept: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM withdrawn_people", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(kept, MAX_WITHDRAWN as i64);

        db.forget_withdrawal(&newest).unwrap();
        assert_eq!(db.withdrawn_at(&newest).unwrap(), None);
    }

    #[test]
    fn a_later_reader_reads_a_directory_again_from_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let directory = SecretKey::from_bytes(&[8; 32]).public();
        {
            // A database from before migration 18, which got to 3.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            for sql in &MIGRATIONS[..17] {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 17).unwrap();
            conn.execute(
                "INSERT INTO directories_read (directory, cursor) VALUES (?1, 3)",
                params![directory.to_string()],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.directory_cursor(&directory, 0).unwrap(), 3);
        assert_eq!(db.directory_cursor(&directory, 1).unwrap(), 0);
        db.set_directory_cursor(&directory, 3, 1).unwrap();
        assert_eq!(db.directory_cursor(&directory, 1).unwrap(), 3);
        // And the one after that starts over again.
        assert_eq!(db.directory_cursor(&directory, 2).unwrap(), 0);
    }

    #[test]
    fn a_found_persons_older_profile_doesnt_replace_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let directory = SecretKey::from_bytes(&[9; 32]).public();
        let key = SecretKey::from_bytes(&[1; 32]);
        let dated = |name: &str, published: &str| {
            let person = Person {
                actor: key.public(),
                name: name.into(),
                summary: None,
                discoverable: true,
                nodes: vec![],
                media_endpoints: vec![],
                published: published.into(),
                icon: None,
                attachment: Vec::new(),
            };
            let signed = SignedObject::sign(&key, &Payload::Person(person.clone())).unwrap();
            (person, signed)
        };
        let (later, later_signed) = dated("Later", LATER);
        db.keep_found(&directory, &later, &later_signed, &[], 100)
            .unwrap();
        let (earlier, earlier_signed) = dated("Earlier", EARLIER);
        db.keep_found(&directory, &earlier, &earlier_signed, &[], 200)
            .unwrap();
        assert_eq!(found_people(&db, "later"), ["Later"]);
        assert!(found_people(&db, "earlier").is_empty());
        let (newest, newest_signed) = dated("Newest", "2030-01-01T00:00:00Z");
        db.keep_found(&directory, &newest, &newest_signed, &[], 300)
            .unwrap();
        assert_eq!(found_people(&db, "newest"), ["Newest"]);
    }

    #[test]
    fn a_media_endpoint_from_an_older_profile_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let directory = SecretKey::from_bytes(&[9; 32]).public();
        let key = SecretKey::from_bytes(&[1; 32]);
        let node = SecretKey::from_bytes(&[2; 32]);
        let dated = |media: u8, published: &str| {
            let media = SecretKey::from_bytes(&[media; 32]);
            let person = Person {
                actor: key.public(),
                name: "Kim".into(),
                summary: None,
                discoverable: true,
                nodes: vec![node.public()],
                media_endpoints: vec![EndpointBinding::new(
                    &node,
                    &media,
                    EndpointKind::Media,
                    1_000,
                )],
                published: published.into(),
                icon: None,
                attachment: Vec::new(),
            };
            let signed = SignedObject::sign(&key, &Payload::Person(person.clone())).unwrap();
            (person, signed)
        };
        let media_of = |db: &Db| -> Vec<EndpointId> {
            let bound = db.media_endpoints_of(&key.public()).unwrap();
            bound.iter().map(|b| b.endpoint).collect()
        };
        assert!(media_of(&db).is_empty());
        let (later, later_signed) = dated(3, LATER);
        db.keep_found(&directory, &later, &later_signed, &[], 100)
            .unwrap();
        let three = SecretKey::from_bytes(&[3; 32]).public();
        assert_eq!(media_of(&db), [three]);
        let (earlier, earlier_signed) = dated(4, EARLIER);
        db.keep_found(&directory, &earlier, &earlier_signed, &[], 200)
            .unwrap();
        assert_eq!(media_of(&db), [three]);
    }

    /// Kim's post with a photo, Jimmy's hosting reshare of it, and the
    /// listings that bring them (ADR 0025).
    struct Reshared {
        kim: SecretKey,
        jimmy: SecretKey,
        post: (SignedObject, Payload),
        blob: BlobHash,
        jimmy_person: Person,
        jimmy_profile: SignedObject,
        reshare: FoundReshare,
    }

    fn reshared() -> Reshared {
        let kim = SecretKey::from_bytes(&[1; 32]);
        let jimmy = SecretKey::from_bytes(&[2; 32]);
        let blob = BlobHash::from_bytes([5; 32]);
        let payload = Payload::Note(Note {
            actor: kim.public(),
            content: "walnut bowl".into(),
            published: EARLIER.into(),
            attachment: vec![Attachment {
                kind: osp_proto::MediaKind::Image,
                media_type: "image/webp".into(),
                blob,
                size: 700,
                width: 1,
                height: 1,
                duration_ms: None,
                name: None,
                thumbnail: None,
                profile: "v1-720p".into(),
            }],
            in_reply_to: None,
        });
        let post = (SignedObject::sign(&kim, &payload).unwrap(), payload);
        let announce = Payload::Announce(osp_proto::Announce {
            actor: jimmy.public(),
            object: post.0.id(),
            object_actor: kim.public(),
            published: LATER.into(),
            hosting: true,
        });
        let (jimmy_person, jimmy_profile) = signed_person(&jimmy, "Jimmy", None);
        let reshare = FoundReshare {
            announce: SignedObject::sign(&jimmy, &announce).unwrap(),
            announce_payload: announce,
            post: post.0.clone(),
            post_payload: post.1.clone(),
            hosting: true,
        };
        Reshared {
            kim,
            jimmy,
            post,
            blob,
            jimmy_person,
            jimmy_profile,
            reshare,
        }
    }

    #[test]
    fn a_found_hosting_reshare_names_its_resharer_and_goes_with_its_listing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let directory = SecretKey::from_bytes(&[9; 32]).public();
        let r = reshared();
        let post_id = r.post.0.id();
        db.keep_found_with(
            &directory,
            &r.jimmy_person,
            &r.jimmy_profile,
            &[],
            std::slice::from_ref(&r.reshare),
            100,
        )
        .unwrap();
        assert_eq!(db.declared_size(&r.blob).unwrap(), Some(700));
        let (id, note) = db.found_only_post(&r.blob).unwrap().unwrap();
        assert_eq!((id, note.actor), (post_id, r.kim.public()));
        assert_eq!(db.found_hosts_of(&post_id).unwrap(), [r.jimmy.public()]);

        // His next listing has no reshares: it and Kim's post go.
        db.keep_found_with(&directory, &r.jimmy_person, &r.jimmy_profile, &[], &[], 200)
            .unwrap();
        assert!(db.found_only_post(&r.blob).unwrap().is_none());
        assert!(db.found_hosts_of(&post_id).unwrap().is_empty());
        assert!(db.get_object(&post_id).unwrap().is_none());
        assert!(!db.only_found_names(&r.blob).unwrap());
    }

    #[test]
    fn a_post_a_found_reshare_points_at_stays_when_its_own_listing_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let directory = SecretKey::from_bytes(&[9; 32]).public();
        let r = reshared();
        let post_id = r.post.0.id();
        // Kim's own listing brings the post, and Jimmy's his reshare of it.
        let (kim_person, kim_profile) = signed_person(&r.kim, "Kim", None);
        db.keep_found(
            &directory,
            &kim_person,
            &kim_profile,
            std::slice::from_ref(&r.post),
            100,
        )
        .unwrap();
        db.keep_found_with(
            &directory,
            &r.jimmy_person,
            &r.jimmy_profile,
            &[],
            std::slice::from_ref(&r.reshare),
            100,
        )
        .unwrap();
        // Kim's next listing doesn't have it: Jimmy's reshare still holds it.
        db.keep_found(&directory, &kim_person, &kim_profile, &[], 200)
            .unwrap();
        assert!(db.get_object(&post_id).unwrap().is_some());
        assert!(db.found_only_post(&r.blob).unwrap().is_some());
        // And it goes with his.
        db.keep_found_with(&directory, &r.jimmy_person, &r.jimmy_profile, &[], &[], 300)
            .unwrap();
        assert!(db.get_object(&post_id).unwrap().is_none());
    }

    #[test]
    fn a_stale_directorys_reshare_of_a_deleted_post_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let directory = SecretKey::from_bytes(&[9; 32]).public();
        let r = reshared();
        let d = Delete {
            actor: r.kim.public(),
            object: r.post.0.id(),
            published: LATER.into(),
        };
        let delete = SignedObject::sign(&r.kim, &Payload::Delete(d.clone())).unwrap();
        let me = SecretKey::from_bytes(&[3; 32]).public();
        db.apply_delete(&delete, &d, true, &me).unwrap();
        db.keep_found_with(
            &directory,
            &r.jimmy_person,
            &r.jimmy_profile,
            &[],
            std::slice::from_ref(&r.reshare),
            100,
        )
        .unwrap();
        assert!(db.get_object(&r.post.0.id()).unwrap().is_none());
        assert!(db.get_object(&r.reshare.announce.id()).unwrap().is_none());
        assert!(db.found_only_post(&r.blob).unwrap().is_none());
        assert!(db.found_hosts_of(&r.post.0.id()).unwrap().is_empty());
    }

    #[test]
    fn reshare_checks_count_failures_in_a_row_after_one_passed() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (announce, post) = (ObjectId::from_bytes([1; 32]), ObjectId::from_bytes([2; 32]));
        let actor = SecretKey::from_bytes(&[3; 32]).public();
        let check = |passed, now| {
            db.note_reshare_check(&announce, &actor, &post, passed, now)
                .unwrap()
        };
        let failed_first = check(false, 1);
        assert!(!failed_first.passed);
        let passed = check(true, 2);
        assert!(passed.passed && passed.fails == 0);
        assert_eq!(check(false, 3).fails, 1);
        assert_eq!(check(false, 4).fails, 2);
        let back = check(true, 5);
        assert!(back.passed && back.fails == 0);
        assert!(db.reshare_checks_due(&actor, 6).unwrap());
        assert!(!db.reshare_checks_due(&actor, 5).unwrap());
        // A plain reshare is never due a check.
        let plain = ObjectId::from_bytes([4; 32]);
        let other = SecretKey::from_bytes(&[4; 32]).public();
        db.note_plain_reshare(&plain, &other, &post, 1).unwrap();
        assert!(!db.reshare_checks_due(&other, 100).unwrap());
        // Both listers of the post are found when it's deleted.
        let mut listers = db.take_reshares_of(&post).unwrap();
        listers.sort();
        let mut both = vec![actor, other];
        both.sort();
        assert_eq!(listers, both);
        assert!(db.reshare_check(&announce).unwrap().is_none());
    }

    /// The text of each post a search finds, in order.
    fn found_posts(db: &Db, words: &str) -> Vec<String> {
        db.search_posts(words, 50)
            .unwrap()
            .iter()
            .map(|obj| match obj.verify().unwrap() {
                Payload::Note(note) => note.content,
                other => panic!("found {other:?}"),
            })
            .collect()
    }

    /// The name on each profile a search finds, in order.
    fn found_people(db: &Db, words: &str) -> Vec<String> {
        db.search_profiles(words, 50)
            .unwrap()
            .iter()
            .map(|obj| match obj.verify().unwrap() {
                Payload::Person(person) => person.name,
                other => panic!("found {other:?}"),
            })
            .collect()
    }

    /// How many things the index itself holds under `word`. A search would
    /// also check each is still stored.
    fn indexed(db: &Db, word: &str) -> i64 {
        db.conn()
            .query_row(
                "SELECT COUNT(*) FROM search_text WHERE search_text MATCH ?1",
                params![word],
                |row| row.get(0),
            )
            .unwrap()
    }

    const EARLIER: &str = "2026-01-01T00:00:00Z";
    const LATER: &str = "2026-02-01T00:00:00Z";

    #[test]
    fn a_search_finds_words_their_starts_and_letters_without_accents() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let key = SecretKey::generate();
        let text = "The Final Cut of my #woodworking tables, at a Café in Zürich";
        store_note(&db, &key, text, EARLIER);
        store_note(&db, &key, "Something else entirely", EARLIER);
        for words in [
            "final",
            "FINAL",
            "table",
            "wood",
            "#woodworking",
            "cafe",
            "CAFÉ",
            "zurich",
            "  cut   final  ",
        ] {
            assert_eq!(found_posts(&db, words), [text], "searching for {words:?}");
        }
        // Every word has to be there.
        assert!(found_posts(&db, "final entirely").is_empty());
        assert!(found_posts(&db, "ood").is_empty());
    }

    #[test]
    fn a_search_shows_the_best_match_first_then_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let key = SecretKey::generate();
        let best = "oak oak oak oak";
        let long = "oak among a good many other words that have nothing to do with it at all";
        store_note(&db, &key, best, EARLIER);
        store_note(&db, &key, long, LATER);
        store_note(&db, &key, "pine, with nothing else to say", LATER);
        assert_eq!(found_posts(&db, "oak"), [best, long]);

        // The same words, so only the date tells these apart.
        let id_old = store_note(&db, &key, "elm table", EARLIER);
        let id_new = store_note(&db, &key, "elm table", LATER);
        let ids: Vec<ObjectId> = db
            .search_posts("elm", 50)
            .unwrap()
            .iter()
            .map(|obj| obj.id())
            .collect();
        assert_eq!(ids, [id_new, id_old]);
        assert_eq!(db.search_posts("elm", 1).unwrap()[0].id(), id_new);
    }

    #[test]
    fn what_someone_types_is_only_ever_words_to_look_for() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let key = SecretKey::generate();
        let both = "apple and pear";
        let apple = "just an apple";
        let pear = "just a pear";
        for text in [both, apple, pear] {
            store_note(&db, &key, text, EARLIER);
        }

        // FTS5's operators would change these; as words, nothing matches them.
        for words in [
            "apple OR pear",
            "apple NOT pear",
            "NOT apple",
            "apple AND pear AND nothing",
            "NEAR(apple pear)",
            "text:apple",
            "{text}:apple",
            "(apple OR pear)",
            "^just AND pear OR apple",
        ] {
            assert!(
                found_posts(&db, words).is_empty(),
                "searching for {words:?}"
            );
        }
        // Punctuation is only punctuation, on its own or stuck to a word.
        for words in [
            "\"", "\"\"", "\"\"\"", "*", "**", "(", ")", "()", ":", "^", "-", "+", "'", "--", ";",
            "\\", "\0", "\0\0", "\u{7}", "NEAR", "!!!", "\"*", "*\"",
        ] {
            found_posts(&db, words);
            found_people(&db, words);
        }
        for words in [
            "apple\"",
            "\"apple",
            "\"apple\"",
            "apple*",
            "*apple",
            "apple)",
            "(apple",
            "ap\0ple",
            "\0apple",
            "apple !!!",
            "!!! apple",
            "apple \"",
            "\" apple",
            "apple *",
        ] {
            let mut found = found_posts(&db, words);
            found.sort();
            assert_eq!(found, [both, apple], "searching for {words:?}");
        }
        // Nothing to look for.
        for words in ["", "   ", "\n\t", "\0", "\u{7} \u{1}"] {
            assert_eq!(search_query(words), None, "searching for {words:?}");
            assert!(found_posts(&db, words).is_empty());
        }

        // Very long input: past the limit on words, the rest doesn't count.
        let many = format!("apple {}", "z ".repeat(100_000));
        assert!(found_posts(&db, &many).is_empty());
        let many = format!("{}pear", "apple ".repeat(MAX_SEARCH_WORDS));
        let mut found = found_posts(&db, &many);
        found.sort();
        assert_eq!(found, [both, apple]);
        assert!(found_posts(&db, &"x".repeat(1 << 20)).is_empty());
    }

    #[test]
    fn what_a_post_says_is_only_ever_text() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let key = SecretKey::generate();
        let tricky = "a\0b \"unclosed (OR) NEAR( text:x '; DROP TABLE objects; --";
        store_note(&db, &key, tricky, EARLIER);
        // No text at all: a post of only media.
        store_note(&db, &key, "", EARLIER);
        assert_eq!(found_posts(&db, "unclosed"), [tricky]);
        assert_eq!(found_posts(&db, "drop table"), [tricky]);
    }

    #[test]
    fn a_post_stored_twice_is_found_once() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (obj, payload) = signed_note(&SecretKey::generate(), "once", EARLIER);
        assert!(db.insert_object(&obj, &payload).unwrap());
        assert!(!db.insert_object(&obj, &payload).unwrap());
        assert_eq!(found_posts(&db, "once"), ["once"]);
    }

    #[test]
    fn a_newer_profile_takes_the_old_ones_place_in_a_search() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (alice, bob) = (SecretKey::generate(), SecretKey::generate());
        let (person, obj) = signed_person(&alice, "Alice Archer", Some("Makes bows"));
        db.upsert_profile(&person, &obj).unwrap();
        let (person, obj) = signed_person(&bob, "Bob Baker", None);
        db.upsert_profile(&person, &obj).unwrap();
        assert_eq!(found_people(&db, "alice"), ["Alice Archer"]);
        assert_eq!(found_people(&db, "bows"), ["Alice Archer"]);
        assert_eq!(found_people(&db, "baker"), ["Bob Baker"]);
        // Posts and people are looked through apart.
        assert!(found_posts(&db, "alice").is_empty());
        store_note(&db, &bob, "a note about alice", EARLIER);
        assert_eq!(found_people(&db, "alice"), ["Alice Archer"]);

        let (person, obj) = signed_person(&alice, "Alice Smith", Some("Forges nails"));
        db.upsert_profile(&person, &obj).unwrap();
        assert_eq!(found_people(&db, "smith"), ["Alice Smith"]);
        assert_eq!(found_people(&db, "alice"), ["Alice Smith"]);
        assert!(found_people(&db, "archer").is_empty());
        assert!(found_people(&db, "bows").is_empty());
        let items: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM search_items", [], |row| row.get(0))
            .unwrap();
        // Both profiles, and Bob's note.
        assert_eq!(items, 3);

        // The same profile again, as every sync brings it, changes nothing.
        let before: i64 = db
            .conn()
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        db.upsert_profile(&person, &obj).unwrap();
        let after: i64 = db
            .conn()
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        assert_eq!(after, before);
    }

    /// A thief's delete goes with the rest of what a revoked key signed (ADR
    /// 0019), and the post it named is asked for again.
    #[test]
    fn a_delete_signed_by_a_key_revoked_since_is_undone() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let alice = SecretKey::generate();
        let me = SecretKey::generate().public();
        let stolen = osp_proto::CertifiedKey::certify(
            &alice,
            SecretKey::generate(),
            EARLIER.into(),
            EarlierKeys::Retired,
        )
        .unwrap();
        let (post, payload) = signed_note(&alice, "oak", EARLIER);
        db.insert_object(&post, &payload).unwrap();
        let d = Delete {
            actor: alice.public(),
            object: post.id(),
            published: EARLIER.into(),
        };
        let thiefs = SignedObject::sign_certified(&stolen, &Payload::Delete(d.clone())).unwrap();
        db.apply_delete(&thiefs, &d, true, &me).unwrap().unwrap();
        assert!(db.is_tombstoned(&post.id()).unwrap());

        let before = osp_proto::parse_rfc3339_ms(LATER).unwrap();
        db.drop_revoked(&alice.public(), before, true).unwrap();
        assert!(!db.is_tombstoned(&post.id()).unwrap());
        let due = db.refetch_due(&alice.public(), 10).unwrap();
        assert_eq!(
            due.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [post.id()]
        );
        // The owner's copy, fetched again, is taken.
        assert!(db.insert_object_received(&post, &payload, 1).unwrap());
    }

    /// Jimmy, whose key was stolen, revokes it: the hosting reshares of
    /// Kim's posts this node had noted of him aren't named any more, though
    /// the thief's node would pass the rechecks. Sam's stay.
    #[test]
    fn a_revoked_resharer_is_no_longer_named_as_a_host() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let post = signed_note(&SecretKey::generate(), "oak", EARLIER).0.id();
        let blob = BlobHash::from_bytes([7; 32]);
        let host = |actor: &SecretKey| {
            let key = SecretKey::generate();
            ResharerHost {
                node: SecretKey::generate().public(),
                post,
                actor: actor.public(),
                announce: post,
                media: EndpointBinding::new(actor, &key, EndpointKind::Media, 1),
                discoverable: true,
                fails: 0,
                checked_ms: now_ms(),
            }
        };
        let (jimmy, sam) = (SecretKey::generate(), SecretKey::generate());
        let (jimmys, sams) = (host(&jimmy), host(&sam));
        db.note_resharer_host(&jimmys, &[blob]).unwrap();
        db.note_resharer_host(&sams, &[blob]).unwrap();
        assert!(db.is_resharer_host(&post, &jimmys.node).unwrap());

        let before = osp_proto::parse_rfc3339_ms(LATER).unwrap();
        db.drop_revoked(&jimmy.public(), before, false).unwrap();
        assert!(!db.is_resharer_host(&post, &jimmys.node).unwrap());
        assert!(db.is_resharer_host(&post, &sams.node).unwrap());
    }

    #[test]
    fn what_a_revoked_key_signed_leaves_the_index_too() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let alice = SecretKey::generate();
        let certify = |published: &str, earlier| {
            osp_proto::CertifiedKey::certify(
                &alice,
                SecretKey::generate(),
                published.into(),
                earlier,
            )
            .unwrap()
        };
        let old = certify(EARLIER, EarlierKeys::Retired);
        let new = certify(LATER, EarlierKeys::Revoked);
        for (key, text) in [(&old, "oldkey post"), (&new, "newkey post")] {
            let payload = signed_note(&alice, text, EARLIER).1;
            let obj = SignedObject::sign_certified(key, &payload).unwrap();
            assert!(db.insert_object(&obj, &payload).unwrap());
        }
        let (person, _) = signed_person(&alice, "Oldkey Alice", None);
        let payload = Payload::Person(person.clone());
        let obj = SignedObject::sign_certified(&old, &payload).unwrap();
        db.upsert_profile(&person, &obj).unwrap();

        assert_eq!(indexed(&db, "oldkey"), 2);
        assert_eq!(found_people(&db, "alice"), ["Oldkey Alice"]);

        let before = osp_proto::parse_rfc3339_ms(LATER).unwrap();
        assert_eq!(db.drop_revoked(&alice.public(), before, false).unwrap(), 1);
        assert_eq!(indexed(&db, "oldkey"), 0);
        assert_eq!(indexed(&db, "newkey"), 1);
        assert_eq!(found_posts(&db, "post"), ["newkey post"]);
        assert!(found_people(&db, "alice").is_empty());
        let items: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM search_items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(items, 1);
    }

    #[test]
    fn a_post_taken_out_of_the_index_is_gone_even_after_a_vacuum() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let key = SecretKey::generate();
        let ids: Vec<ObjectId> = (0..20)
            .map(|i| store_note(&db, &key, &format!("post{i}th"), EARLIER))
            .collect();
        // Gaps in the objects table, for `VACUUM` to close.
        for id in ids.iter().step_by(2) {
            db.conn()
                .execute("DELETE FROM objects WHERE id = ?1", params![id.to_string()])
                .unwrap();
            remove_from_search(&db.conn(), &id.to_string()).unwrap();
        }
        db.conn().execute_batch("VACUUM").unwrap();
        for i in 0..20 {
            let found = found_posts(&db, &format!("post{i}th"));
            assert_eq!(found.len(), i % 2, "post {i}");
        }
        remove_from_search(&db.conn(), &ids[1].to_string()).unwrap();
        assert!(found_posts(&db, "post1th").is_empty());
        assert_eq!(found_posts(&db, "post3th"), ["post3th"]);
    }

    #[test]
    fn a_database_from_before_search_is_indexed_when_opened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let key = SecretKey::generate();
        let (note, _) = signed_note(&key, "written before search", EARLIER);
        let (_, profile) = signed_person(&key, "Early Bird", Some("Was here first"));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            for sql in MIGRATIONS {
                conn.execute_batch(sql).unwrap();
            }
            conn.pragma_update(None, "user_version", MIGRATIONS.len() as i64)
                .unwrap();
            let actor = key.public().to_string();
            conn.execute(
                "INSERT INTO objects (id, actor, kind, published_ms, received_ms, signed)
                 VALUES (?1, ?2, 'Note', 1, 1, ?3)",
                params![
                    note.id().to_string(),
                    actor,
                    serde_json::to_string(&note).unwrap()
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO profiles (actor, name, signed) VALUES (?1, 'Early Bird', ?2)",
                params![actor, serde_json::to_string(&profile).unwrap()],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(found_posts(&db, "written"), ["written before search"]);
        assert_eq!(found_people(&db, "early"), ["Early Bird"]);
        assert_eq!(found_people(&db, "first"), ["Early Bird"]);
        drop(db);

        // Opening again doesn't build it twice.
        let db = Db::open(&path).unwrap();
        assert_eq!(found_posts(&db, "written"), ["written before search"]);
        let (obj, payload) = signed_note(&key, "written after search", LATER);
        assert!(db.insert_object(&obj, &payload).unwrap());

        // Lost, it comes back from the posts and profiles stored.
        db.conn()
            .execute_batch("DROP TABLE search_text; DROP TABLE search_items;")
            .unwrap();
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(
            found_posts(&db, "written"),
            ["written after search", "written before search"]
        );
        assert_eq!(found_people(&db, "early"), ["Early Bird"]);
    }

    #[test]
    fn a_stored_post_that_no_longer_checks_out_isnt_indexed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = Db::open(&path).unwrap();
        let key = SecretKey::generate();
        store_note(&db, &key, "one good post", EARLIER);
        let bad = store_note(&db, &key, "one bad post", EARLIER);
        // Its row now holds different words under the same signature.
        let (obj, _) = signed_note(&key, "one bad post", EARLIER);
        let (changed, _) = signed_note(&key, "one changed post", EARLIER);
        let mut json = serde_json::to_value(obj).unwrap();
        json["payload"] = serde_json::to_value(changed).unwrap()["payload"].clone();
        db.conn()
            .execute(
                "UPDATE objects SET signed = ?2 WHERE id = ?1",
                params![bad.to_string(), json.to_string()],
            )
            .unwrap();
        db.conn()
            .execute_batch("DROP TABLE search_text; DROP TABLE search_items;")
            .unwrap();
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(found_posts(&db, "one"), ["one good post"]);
        assert_eq!(indexed(&db, "good"), 1);
        assert_eq!(indexed(&db, "changed"), 0);
    }

    #[test]
    fn served_counts_the_last_30_days_and_drops_older() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let today = day_of(now_ms());
        db.count_served(today - 40, 1000).unwrap();
        db.count_served(today - 30, 100).unwrap();
        db.count_served(today - 29, 10).unwrap();
        db.count_served(today, 1).unwrap();
        db.count_served(today, 2).unwrap();
        assert_eq!(db.served_lately(today).unwrap(), 13);
        let days: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM served_days", [], |r| r.get(0))
            .unwrap();
        // Day -40 was dropped; day -30 is kept, as `SERVED_DAYS` is 31.
        assert_eq!(days, 3);
        assert!(db.size_bytes().unwrap() > 0);
    }

    #[test]
    fn a_search_index_from_before_dates_is_built_again_when_opened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let key = SecretKey::generate();
        {
            let db = Db::open(&path).unwrap();
            store_note(&db, &key, "ash early", EARLIER);
            store_note(&db, &key, "ash later", LATER);
            // The index as it was before migration 25: no dates.
            db.conn()
                .execute_batch(
                    "DROP TABLE search_text; DROP TABLE search_items;
                     CREATE TABLE search_items (id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE);
                     CREATE VIRTUAL TABLE search_text USING fts5(
                         text, content = '', contentless_delete = 1,
                         tokenize = 'unicode61 remove_diacritics 2');
                     DROP TABLE reply_policies; DROP TABLE received_replies;
                     DROP TABLE repliers; DROP TABLE reply_deletes;
                     DROP TABLE sent_replies;
                     DROP TABLE vouches; DROP TABLE voucher_profiles;
                     DROP TABLE credits_given; DROP TABLE told_reshares;
                     DROP TABLE side_vouches; DROP TABLE side_gathered;
                     DROP INDEX objects_by_actor_date; DROP TABLE icons;
                     DROP TABLE universe_places; DROP TABLE served_days;
                     DROP TABLE events; ALTER TABLE declared_sizes DROP COLUMN post;
                     DROP TABLE likes; DROP TABLE sent_likes; DROP TABLE activity;
                     DROP TABLE opened_posts; DROP TABLE opened_people; DROP TABLE blocks;
                     DROP TABLE hide_list; DROP TABLE hide_state; DROP TABLE hidden_objects;
                     DROP TABLE hide_exceptions; DROP TABLE block_dropped;
                     DROP TABLE machine_starts; DROP TABLE carrier_jobs;
                     DROP TABLE carriers_worked; DROP TABLE carrier_probation;
                     DROP TABLE carrier_tips; DROP TABLE carrier_suggestions;
                     DROP TABLE carried_days;
                     DROP TABLE review_pairing; ALTER TABLE devices DROP COLUMN review;
                     DROP TABLE withdrawn_people;
                     DROP TABLE held_vouches; DROP TABLE given_vouches;
                     PRAGMA user_version = 24;",
                )
                .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(found_posts(&db, "ash"), ["ash later", "ash early"]);
        let undated: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM search_items WHERE published_ms IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(undated, 0);
    }

    // ---- opened posts (ADR 0039 §4) ----

    fn keep_opened_note(db: &Db, key: &SecretKey, text: &str, now: i64) -> ObjectId {
        let (obj, payload) = signed_note(key, text, EARLIER);
        let Payload::Note(note) = payload else {
            unreachable!()
        };
        let (person, profile) = signed_person(key, &format!("{text} author"), None);
        let kept = db
            .keep_opened(&obj, &note, Some((&person, &profile)), now)
            .unwrap();
        assert_eq!(kept, opened::Kept::Opened);
        obj.id()
    }

    #[test]
    fn opened_posts_go_after_their_time_then_the_least_lately_opened_beyond_the_caps() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (a, b, c) = (
            SecretKey::generate(),
            SecretKey::generate(),
            SecretKey::generate(),
        );
        let first = keep_opened_note(&db, &a, "first", 1_000);
        let second = keep_opened_note(&db, &b, "second", 2_000);
        let third = keep_opened_note(&db, &c, "third", 3_000);
        let all = u64::MAX;
        assert!(db.opened_to_drop(3_000, 10, all).unwrap().is_empty());
        assert_eq!(db.opened_to_drop(3_000, 2, all).unwrap(), [first]);
        assert_eq!(
            db.opened_to_drop(1_000 + opened::OPENED_FOR_MS, 10, all)
                .unwrap(),
            [first]
        );
        // A byte cap that holds one post keeps the latest opened.
        let one: i64 = db
            .conn()
            .query_row(
                "SELECT bytes FROM opened_posts WHERE id = ?1",
                params![third.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            db.opened_to_drop(3_000, 10, one as u64 + 10).unwrap(),
            [second, first]
        );
        // An ordinary post isn't an opened one.
        let ordinary = store_note(&db, &a, "ordinary", EARLIER);
        assert_eq!(db.opened_until(&ordinary).unwrap(), None);
    }

    #[test]
    fn an_opened_post_and_its_author_are_in_no_list_until_kept_for_another_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = Db::open(&path).unwrap();
        let me = SecretKey::generate().public();
        let alice = SecretKey::generate();
        let id = keep_opened_note(&db, &alice, "hazelnut", now_ms());
        let author = alice.public();
        let in_lists = |db: &Db| {
            (
                db.people(&me, 50).unwrap().len(),
                db.held_people(&me).unwrap().len(),
                db.search_posts("hazelnut", 10).unwrap().len(),
                db.search_profiles("hazelnut", 10).unwrap().len(),
                db.feed(&[author], 10).unwrap().len(),
                db.newest_ms(&author).unwrap().is_some(),
            )
        };
        assert_eq!(in_lists(&db), (0, 0, 0, 0, 0, false));
        // Nor when the search index is built again.
        db.conn()
            .execute_batch("DROP TABLE search_text; DROP TABLE search_items;")
            .unwrap();
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(in_lists(&db), (0, 0, 0, 0, 0, false));

        // It arrives another way, as from a follow's outbox: it's ordinary.
        let (obj, payload) = signed_note(&alice, "hazelnut", EARLIER);
        assert!(!db.insert_object(&obj, &payload).unwrap());
        assert_eq!(db.opened_until(&id).unwrap(), None);
        assert_eq!(in_lists(&db), (1, 1, 1, 1, 1, true));
        assert_eq!(db.drop_opened(&id).unwrap(), None);
    }

    #[test]
    fn dropping_an_opened_post_keeps_a_profile_held_for_another_reason() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let me = SecretKey::generate().public();
        let (alice, jimmy) = (SecretKey::generate(), SecretKey::generate());

        // Jimmy's profile was held before: it stays, and is listed throughout.
        let (person, profile) = signed_person(&jimmy, "Jimmy", None);
        db.upsert_profile(&person, &profile).unwrap();
        let his = keep_opened_note(&db, &jimmy, "pecan", now_ms());
        assert_eq!(db.people(&me, 50).unwrap().len(), 1);
        assert_eq!(db.drop_opened(&his).unwrap(), Some(vec![]));
        assert_eq!(db.people(&me, 50).unwrap().len(), 1);

        // A reshare of an opened post held here needs it: Remove leaves it,
        // an ordinary post now, and Jimmy's profile with it.
        let shared = keep_opened_note(&db, &jimmy, "walnut", now_ms());
        let carol = SecretKey::generate();
        let reshare = Payload::Announce(osp_proto::Announce {
            actor: carol.public(),
            object: shared,
            object_actor: jimmy.public(),
            published: LATER.into(),
            hosting: false,
        });
        let signed = SignedObject::sign(&carol, &reshare).unwrap();
        assert!(db.insert_object(&signed, &reshare).unwrap());
        assert_eq!(db.drop_opened(&shared).unwrap(), Some(vec![]));
        assert!(db.get_object(&shared).unwrap().is_some());
        assert_eq!(db.opened_until(&shared).unwrap(), None);
        assert!(db.profile(&jimmy.public()).unwrap().is_some());

        // Alice's came only with her opened post: it goes with it.
        let hers = keep_opened_note(&db, &alice, "almond", now_ms());
        assert!(db.profile(&alice.public()).unwrap().is_some());
        db.drop_opened(&hers).unwrap();
        assert!(db.profile(&alice.public()).unwrap().is_none());
        assert!(db.get_object(&hers).unwrap().is_none());
    }
}
