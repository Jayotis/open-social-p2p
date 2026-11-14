//! Who vouched for this node's owner (ADR 0029): reshares of their posts,
//! told by the resharers' nodes, and found-early credits from the authors of
//! posts they reshared. Also the credits the owner gave, and which of the
//! owner's reshares their authors' nodes were told of.

use anyhow::{Context, Result};
use iroh::{EndpointAddr, EndpointId, PublicKey};
use osp_proto::{ObjectId, Payload, SignedObject};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::Db;

/// What kind of vouch a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VouchKind {
    /// Someone reshared one of the owner's posts.
    Reshare,
    /// The author of a post the owner reshared credits them with finding it early.
    FoundEarly,
}

impl VouchKind {
    fn as_str(self) -> &'static str {
        match self {
            VouchKind::Reshare => "reshare",
            VouchKind::FoundEarly => "found_early",
        }
    }
}

/// A vouch to keep.
pub(crate) struct NewVouch<'a> {
    pub kind: VouchKind,
    pub post: ObjectId,
    pub voucher: PublicKey,
    pub signed: &'a SignedObject,
    /// The date of the post, by which the oldest go first.
    pub post_ms: i64,
    pub now_ms: i64,
}

/// A credit this node's owner gave an early finder of one of their posts.
#[derive(Debug, Clone)]
pub(crate) struct GivenCredit {
    pub post: ObjectId,
    pub finder: PublicKey,
    pub signed: SignedObject,
}

/// A vouch kept for the owner, with its voucher's profile.
#[derive(Debug, Clone)]
pub(crate) struct KeptVouch {
    pub voucher: PublicKey,
    pub signed: SignedObject,
    pub profile: SignedObject,
}

/// One person on the owner's side (ADR 0029): `voucher`, `step` steps back
/// from the owner, placed by the signed vouch `signed`.
#[derive(Debug, Clone)]
pub(crate) struct SideVouch {
    pub voucher: PublicKey,
    pub step: u8,
    pub signed: SignedObject,
}

impl Db {
    /// Whether any vouch from `voucher` is kept.
    pub fn knows_voucher(&self, voucher: &PublicKey) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM vouches WHERE voucher = ?1 LIMIT 1",
                params![voucher.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }

    /// Keeps a vouch, replacing one of the same kind by the same voucher for
    /// the same post but keeping when that was first heard of. Then, past
    /// `max` of that kind, drops those of the oldest posts, and the profiles
    /// of vouchers left with none.
    pub fn add_vouch(&self, vouch: &NewVouch, max: usize) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let kind = vouch.kind.as_str();
        tx.execute(
            "INSERT INTO vouches (kind, post, voucher, signed, first_ms, heard_ms, post_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)
             ON CONFLICT(kind, post, voucher) DO UPDATE SET
                 signed = excluded.signed, heard_ms = excluded.heard_ms",
            params![
                kind,
                vouch.post.to_string(),
                vouch.voucher.to_string(),
                serde_json::to_string(vouch.signed)?,
                vouch.now_ms,
                vouch.post_ms,
            ],
        )?;
        tx.execute(
            "DELETE FROM vouches WHERE rowid IN (
                 SELECT rowid FROM vouches WHERE kind = ?1
                 ORDER BY post_ms DESC, first_ms DESC LIMIT -1 OFFSET ?2)",
            params![kind, max as i64],
        )?;
        tx.execute(
            "DELETE FROM voucher_profiles
             WHERE actor NOT IN (SELECT voucher FROM vouches)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// How many vouches of `kind` there are for `post`: for a reshare, how
    /// many people reshared it.
    pub fn vouches_for(&self, kind: VouchKind, post: &ObjectId) -> Result<usize> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM vouches WHERE kind = ?1 AND post = ?2",
            params![kind.as_str(), post.to_string()],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    /// The resharers of `post` in the order this node first heard of them,
    /// with when: oldest first.
    pub fn resharers_in_order(&self, post: &ObjectId) -> Result<Vec<(PublicKey, i64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT voucher, first_ms FROM vouches WHERE kind = 'reshare' AND post = ?1
             ORDER BY first_ms, rowid",
        )?;
        let rows = stmt.query_map(params![post.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (voucher, first_ms) = row?;
            out.push((voucher.parse()?, first_ms));
        }
        Ok(out)
    }

    /// Forgets the vouches by reshare for one of the owner's posts, and the
    /// credits given for it, once it's deleted.
    pub fn forget_vouches_of(&self, post: &ObjectId) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let post = post.to_string();
        tx.execute(
            "DELETE FROM vouches WHERE kind = 'reshare' AND post = ?1",
            params![post],
        )?;
        tx.execute("DELETE FROM credits_given WHERE post = ?1", params![post])?;
        tx.execute(
            "DELETE FROM voucher_profiles
             WHERE actor NOT IN (SELECT voucher FROM vouches)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Keeps a voucher's profile, if it's newer than the one kept.
    pub fn keep_voucher_profile(
        &self,
        actor: &PublicKey,
        profile: &SignedObject,
        published_ms: i64,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT INTO voucher_profiles (actor, profile, published_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(actor) DO UPDATE SET
                 profile = excluded.profile, published_ms = excluded.published_ms
             WHERE excluded.published_ms > voucher_profiles.published_ms",
            params![
                actor.to_string(),
                serde_json::to_string(profile)?,
                published_ms
            ],
        )?;
        Ok(())
    }

    /// A voucher's newest profile kept here.
    pub fn voucher_profile(&self, actor: &PublicKey) -> Result<Option<SignedObject>> {
        let json: Option<String> = self
            .conn()
            .query_row(
                "SELECT profile FROM voucher_profiles WHERE actor = ?1",
                params![actor.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|j| serde_json::from_str(&j).context("corrupt stored profile"))
            .transpose()
    }

    // ---- credits the owner gave ----

    /// Whether the owner gave credits for `post` already.
    pub fn credited(&self, post: &ObjectId) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM credits_given WHERE post = ?1 LIMIT 1",
                params![post.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }

    /// Keeps the credits just signed for a post's early finders, untold.
    pub fn add_credits(&self, credits: &[GivenCredit]) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        for c in credits {
            tx.execute(
                "INSERT OR IGNORE INTO credits_given (post, finder, signed) VALUES (?1, ?2, ?3)",
                params![
                    c.post.to_string(),
                    c.finder.to_string(),
                    serde_json::to_string(&c.signed)?
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Credits whose finders' nodes haven't been told yet.
    pub fn untold_credits(&self) -> Result<Vec<GivenCredit>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT post, finder, signed FROM credits_given WHERE told = 0 ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (post, finder, signed) = row?;
            out.push(GivenCredit {
                post: post.parse()?,
                finder: finder.parse()?,
                signed: serde_json::from_str(&signed).context("corrupt stored credit")?,
            });
        }
        Ok(out)
    }

    /// The credits given for `post`, told or not.
    pub fn credits_for(&self, post: &ObjectId) -> Result<Vec<GivenCredit>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT finder, signed FROM credits_given WHERE post = ?1 ORDER BY rowid")?;
        let rows = stmt.query_map(params![post.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (finder, signed) = row?;
            out.push(GivenCredit {
                post: *post,
                finder: finder.parse()?,
                signed: serde_json::from_str(&signed).context("corrupt stored credit")?,
            });
        }
        Ok(out)
    }

    /// The posts vouches of `kind` are for.
    pub fn vouch_posts(&self, kind: VouchKind) -> Result<Vec<ObjectId>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT post FROM vouches WHERE kind = ?1 ORDER BY first_ms, rowid")?;
        let rows = stmt.query_map(params![kind.as_str()], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.parse()?);
        }
        Ok(out)
    }

    /// Notes that a finder's node has its credit.
    pub fn credit_told(&self, post: &ObjectId, finder: &PublicKey) -> Result<()> {
        self.conn().execute(
            "UPDATE credits_given SET told = 1 WHERE post = ?1 AND finder = ?2",
            params![post.to_string(), finder.to_string()],
        )?;
        Ok(())
    }

    // ---- the owner's reshares, told to their authors ----

    /// Whether the owner's reshare `announce` was told to its author's node.
    pub fn reshare_told(&self, announce: &ObjectId) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM told_reshares WHERE announce = ?1",
                params![announce.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }

    pub fn note_reshare_told(&self, announce: &ObjectId) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO told_reshares (announce) VALUES (?1)",
            params![announce.to_string()],
        )?;
        Ok(())
    }

    // ---- the owner's side ----

    /// The newest vouch of each voucher for the owner, with the voucher's
    /// profile, newest first, at most `limit` vouchers. Those without a kept
    /// profile are left out.
    pub fn newest_vouches(&self, limit: usize) -> Result<Vec<KeptVouch>> {
        let conn = self.conn();
        // SQLite takes the other columns from the row with the largest `first_ms`.
        let mut stmt = conn.prepare(
            "SELECT v.voucher, v.signed, p.profile, MAX(v.first_ms) AS newest
             FROM vouches v JOIN voucher_profiles p ON p.actor = v.voucher
             GROUP BY v.voucher ORDER BY newest DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (voucher, signed, profile) = row?;
            out.push(KeptVouch {
                voucher: voucher.parse()?,
                signed: serde_json::from_str(&signed).context("corrupt stored vouch")?,
                profile: serde_json::from_str(&profile).context("corrupt stored profile")?,
            });
        }
        Ok(out)
    }

    /// Replaces the owner's side with a newly gathered one, gathered at `now_ms`.
    pub fn replace_side(&self, side: &[SideVouch], now_ms: i64) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM side_vouches", [])?;
        for v in side {
            tx.execute(
                "INSERT OR IGNORE INTO side_vouches (voucher, step, signed) VALUES (?1, ?2, ?3)",
                params![
                    v.voucher.to_string(),
                    v.step,
                    serde_json::to_string(&v.signed)?
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO side_gathered (id, ms) VALUES (0, ?1)
             ON CONFLICT(id) DO UPDATE SET ms = excluded.ms",
            params![now_ms],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The owner's side as last gathered, nearest first.
    pub fn side(&self) -> Result<Vec<SideVouch>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT voucher, step, signed FROM side_vouches ORDER BY step, rowid")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u8>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (voucher, step, signed) = row?;
            out.push(SideVouch {
                voucher: voucher.parse()?,
                step,
                signed: serde_json::from_str(&signed).context("corrupt stored vouch")?,
            });
        }
        Ok(out)
    }

    /// When the owner's side was last gathered.
    pub fn side_gathered_ms(&self) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row("SELECT ms FROM side_gathered WHERE id = 0", [], |row| {
                row.get(0)
            })
            .optional()?)
    }

    /// Who reshared whom since `since_ms`, among those the owner follows: each
    /// reshare as (resharer, author of the post), by the reshares' own dates.
    pub fn followed_reshared_since(&self, since_ms: i64) -> Result<Vec<(PublicKey, PublicKey)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT signed FROM objects
             WHERE kind = 'Announce' AND published_ms >= ?1
               AND actor IN (SELECT actor FROM follows)",
        )?;
        let rows = stmt.query_map(params![since_ms], |row| row.get::<_, String>(0))?;
        let mut reshares = Vec::new();
        for json in rows {
            let obj: SignedObject =
                serde_json::from_str(&json?).context("corrupt stored object")?;
            // Checked when it was stored; only its contents are needed here.
            if let Ok(Payload::Announce(a)) = serde_json::from_slice(obj.payload_bytes()) {
                reshares.push((a.actor, a.object_actor));
            }
        }
        Ok(reshares)
    }

    /// Those the owner follows, each with the node followed, those followed
    /// longest first.
    pub fn followed_longest(&self) -> Result<Vec<(PublicKey, EndpointId)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT actor, node_addr FROM follows ORDER BY rowid")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (actor, addr) = row?;
            let addr: EndpointAddr = serde_json::from_str(&addr)?;
            out.push((actor.parse()?, addr.id));
        }
        Ok(out)
    }

    /// Whether the owner's outbox holds a reshare of `post`.
    pub fn owner_reshared(&self, me: &PublicKey, post: &ObjectId) -> Result<bool> {
        Ok(self
            .own_reshares(me)?
            .iter()
            .any(|(reshared, _)| reshared == post))
    }

    /// Each voucher, most recently heard of first, with how many reshares
    /// and found-early credits of theirs are kept, and their kept profile.
    pub fn vouchers(&self) -> Result<Vec<Voucher>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT v.voucher,
                    SUM(v.kind = 'reshare'), SUM(v.kind = 'found_early'),
                    p.profile, MAX(v.heard_ms) AS heard
             FROM vouches v LEFT JOIN voucher_profiles p ON p.actor = v.voucher
             GROUP BY v.voucher ORDER BY heard DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, u32>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (voucher, reshares, credits, profile) = row?;
            let profile = profile
                .map(|p| serde_json::from_str(&p).context("corrupt stored profile"))
                .transpose()?;
            out.push(Voucher {
                actor: voucher.parse()?,
                reshares,
                credits,
                profile,
            });
        }
        Ok(out)
    }

    // ---- backups ----

    /// Everything kept of who vouched for the owner and what the owner gave
    /// and told, for a backup. Not the side: a restored node gathers it again.
    pub fn vouch_tables(&self) -> Result<VouchTables> {
        let conn = self.conn();
        let mut out = VouchTables::default();
        let mut stmt = conn.prepare(
            "SELECT kind, post, voucher, signed, first_ms, heard_ms, post_ms
             FROM vouches ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        for row in rows {
            let (kind, post, voucher, signed, first_ms, heard_ms, post_ms) = row?;
            out.vouches.push(VouchRecord {
                kind,
                post: post.parse()?,
                voucher: voucher.parse()?,
                signed: serde_json::from_str(&signed).context("corrupt stored vouch")?,
                first_ms,
                heard_ms,
                post_ms,
            });
        }
        let mut stmt = conn.prepare("SELECT profile, published_ms FROM voucher_profiles")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (profile, published_ms) = row?;
            out.voucher_profiles.push(VoucherProfileRecord {
                profile: serde_json::from_str(&profile).context("corrupt stored profile")?,
                published_ms,
            });
        }
        let mut stmt =
            conn.prepare("SELECT post, finder, signed, told FROM credits_given ORDER BY rowid")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        for row in rows {
            let (post, finder, signed, told) = row?;
            out.credits_given.push(CreditRecord {
                post: post.parse()?,
                finder: finder.parse()?,
                signed: serde_json::from_str(&signed).context("corrupt stored credit")?,
                told,
            });
        }
        let mut stmt = conn.prepare("SELECT announce FROM told_reshares")?;
        for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
            out.told_reshares.push(row?.parse()?);
        }
        Ok(out)
    }

    /// Puts back what `vouch_tables` gave, into a new node's database. A vouch,
    /// profile or credit that doesn't verify, or names someone other than its
    /// signer, is skipped. Returns how many vouches were put back.
    pub fn restore_vouch_tables(&self, tables: &VouchTables) -> Result<usize> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let signed_by = |obj: &SignedObject, who: &PublicKey| {
            obj.verify().is_ok_and(|payload| payload.actor() == *who)
        };
        let mut vouches = 0;
        for v in &tables.vouches {
            // A reshare is signed by its resharer, a found-early credit by
            // the author who gave it: either way, the voucher.
            let known = [VouchKind::Reshare, VouchKind::FoundEarly]
                .iter()
                .any(|k| k.as_str() == v.kind);
            if !known || !signed_by(&v.signed, &v.voucher) {
                tracing::warn!("skipping a vouch in the backup that doesn't check out");
                continue;
            }
            vouches += tx.execute(
                "INSERT OR IGNORE INTO vouches
                     (kind, post, voucher, signed, first_ms, heard_ms, post_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    v.kind,
                    v.post.to_string(),
                    v.voucher.to_string(),
                    serde_json::to_string(&v.signed)?,
                    v.first_ms,
                    v.heard_ms,
                    v.post_ms,
                ],
            )?;
        }
        for p in &tables.voucher_profiles {
            let Ok(Payload::Person(person)) = p.profile.verify() else {
                tracing::warn!("skipping a voucher's profile in the backup that doesn't check out");
                continue;
            };
            tx.execute(
                "INSERT OR IGNORE INTO voucher_profiles (actor, profile, published_ms)
                 VALUES (?1, ?2, ?3)",
                params![
                    person.actor.to_string(),
                    serde_json::to_string(&p.profile)?,
                    p.published_ms
                ],
            )?;
        }
        for c in &tables.credits_given {
            if c.signed.verify().is_err() {
                tracing::warn!("skipping a credit in the backup that doesn't check out");
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO credits_given (post, finder, signed, told)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    c.post.to_string(),
                    c.finder.to_string(),
                    serde_json::to_string(&c.signed)?,
                    c.told
                ],
            )?;
        }
        for announce in &tables.told_reshares {
            tx.execute(
                "INSERT OR IGNORE INTO told_reshares (announce) VALUES (?1)",
                params![announce.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(vouches)
    }
}

/// Someone who vouched for the owner: how many reshares and found-early
/// credits of theirs are kept, and their kept profile.
pub(crate) struct Voucher {
    pub actor: PublicKey,
    pub reshares: u32,
    pub credits: u32,
    pub profile: Option<SignedObject>,
}

/// What a backup holds of vouches (ADR 0029), as the tables keep them.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct VouchTables {
    pub vouches: Vec<VouchRecord>,
    pub voucher_profiles: Vec<VoucherProfileRecord>,
    pub credits_given: Vec<CreditRecord>,
    pub told_reshares: Vec<ObjectId>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct VouchRecord {
    kind: String,
    post: ObjectId,
    voucher: PublicKey,
    signed: SignedObject,
    first_ms: i64,
    heard_ms: i64,
    post_ms: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct VoucherProfileRecord {
    profile: SignedObject,
    published_ms: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CreditRecord {
    post: ObjectId,
    finder: PublicKey,
    signed: SignedObject,
    told: bool,
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;
    use osp_proto::{Announce, Payload};

    use super::*;

    fn reshare(db: &Db, who: &SecretKey, post: &[u8], post_ms: i64, now_ms: i64, max: usize) {
        let post = ObjectId::of(post);
        let signed = SignedObject::sign(
            who,
            &Payload::Announce(Announce {
                actor: who.public(),
                object: post,
                object_actor: SecretKey::generate().public(),
                published: osp_proto::now_rfc3339(),
                hosting: false,
            }),
        )
        .unwrap();
        let vouch = NewVouch {
            kind: VouchKind::Reshare,
            post,
            voucher: who.public(),
            signed: &signed,
            post_ms,
            now_ms,
        };
        db.add_vouch(&vouch, max).unwrap();
        db.keep_voucher_profile(&who.public(), &signed, now_ms)
            .unwrap();
    }

    #[test]
    fn past_the_most_those_of_the_oldest_posts_go_with_their_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (old, mid, new) = (
            SecretKey::generate(),
            SecretKey::generate(),
            SecretKey::generate(),
        );
        reshare(&db, &old, b"old post", 1_000, 10, 2);
        reshare(&db, &mid, b"mid post", 2_000, 20, 2);
        reshare(&db, &new, b"new post", 3_000, 30, 2);
        assert_eq!(
            db.vouch_posts(VouchKind::Reshare).unwrap(),
            [ObjectId::of(b"mid post"), ObjectId::of(b"new post")]
        );
        assert!(!db.knows_voucher(&old.public()).unwrap());
        assert!(db.voucher_profile(&old.public()).unwrap().is_none());
        assert!(db.voucher_profile(&mid.public()).unwrap().is_some());
    }

    #[test]
    fn a_resharer_telling_again_keeps_when_it_was_first_heard_of() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (a, b) = (SecretKey::generate(), SecretKey::generate());
        reshare(&db, &a, b"post", 1_000, 10, 100);
        reshare(&db, &b, b"post", 1_000, 20, 100);
        reshare(&db, &a, b"post", 1_000, 30, 100);
        let order = db.resharers_in_order(&ObjectId::of(b"post")).unwrap();
        assert_eq!(order, [(a.public(), 10), (b.public(), 20)]);
        assert_eq!(
            db.vouches_for(VouchKind::Reshare, &ObjectId::of(b"post"))
                .unwrap(),
            2
        );
    }
}
