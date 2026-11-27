//! Vouches (ADR 0029). Reshares of this node's owner's posts, told by the
//! resharers' nodes: a record of who reshared, for the "reshared by" count
//! and found-early credits, not a vouch (section 7). The signed `Vouch`es
//! others gave the owner, held until they run out. The owner's own vouches,
//! renewed while they stand. The credits the owner gave, and which of the
//! owner's reshares their authors' nodes were told of.

use anyhow::{Context, Result};
use iroh::{EndpointAddr, EndpointId, PublicKey};
use osp_proto::{ObjectId, Payload, SignedObject};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::Db;

use osp_proto::VouchKind;

fn kind_str(kind: VouchKind) -> &'static str {
    match kind {
        VouchKind::Reshare => "reshare",
        VouchKind::FoundEarly => "found_early",
    }
}

fn kind_from(s: &str) -> Option<VouchKind> {
    match s {
        "reshare" => Some(VouchKind::Reshare),
        "found_early" => Some(VouchKind::FoundEarly),
        _ => None,
    }
}

/// Deletes the profiles of vouchers left with nothing kept of theirs.
const DROP_UNUSED_PROFILES: &str = "DELETE FROM voucher_profiles
     WHERE actor NOT IN (SELECT voucher FROM vouches)
       AND actor NOT IN (SELECT voucher FROM held_vouches)";

/// A reshare of one of the owner's posts to keep.
pub(crate) struct NewVouch<'a> {
    pub post: ObjectId,
    pub voucher: PublicKey,
    pub signed: &'a SignedObject,
    /// The date of the post, by which the oldest go first.
    pub post_ms: i64,
    pub now_ms: i64,
    /// The resharer's profile, kept if newer than the one kept: their node
    /// is reached through it.
    pub profile: &'a SignedObject,
    pub profile_ms: i64,
}

/// A signed `Vouch` someone gave the owner, to hold.
pub(crate) struct HeldVouch<'a> {
    pub voucher: PublicKey,
    pub kind: VouchKind,
    pub signed: &'a SignedObject,
    pub valid_until_ms: i64,
    pub now_ms: i64,
    pub profile: &'a SignedObject,
    pub profile_ms: i64,
}

/// One of the owner's own vouches for someone.
#[derive(Debug, Clone)]
pub(crate) struct GivenVouch {
    pub subject: PublicKey,
    pub kind: VouchKind,
    pub signed: SignedObject,
    pub valid_until_ms: i64,
    /// When it's renewed, if it still stands.
    pub renew_ms: i64,
    /// Whether the subject's node has it.
    pub told: bool,
}

/// A vouch held for the owner, with its voucher's profile.
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
    /// Whether anything from `voucher` is kept: a reshare, or a vouch.
    pub fn knows_voucher(&self, voucher: &PublicKey) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM vouches WHERE voucher = ?1
                 UNION ALL SELECT 1 FROM held_vouches WHERE voucher = ?1 LIMIT 1",
                params![voucher.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }

    /// Keeps a reshare of one of the owner's posts, replacing one by the
    /// same resharer of the same post but keeping when that was first heard
    /// of, and with it the resharer's profile, at once. Then, past `max`,
    /// drops those of the oldest posts, and the profiles of vouchers left
    /// with nothing.
    pub fn add_vouch(&self, vouch: &NewVouch, max: usize) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO vouches (kind, post, voucher, signed, first_ms, heard_ms, post_ms)
             VALUES ('reshare', ?1, ?2, ?3, ?4, ?4, ?5)
             ON CONFLICT(kind, post, voucher) DO UPDATE SET
                 signed = excluded.signed, heard_ms = excluded.heard_ms",
            params![
                vouch.post.to_string(),
                vouch.voucher.to_string(),
                serde_json::to_string(vouch.signed)?,
                vouch.now_ms,
                vouch.post_ms,
            ],
        )?;
        keep_profile(&tx, &vouch.voucher, vouch.profile, vouch.profile_ms)?;
        tx.execute(
            "DELETE FROM vouches WHERE rowid IN (
                 SELECT rowid FROM vouches
                 ORDER BY post_ms DESC, first_ms DESC LIMIT -1 OFFSET ?1)",
            params![max as i64],
        )?;
        tx.execute(DROP_UNUSED_PROFILES, [])?;
        tx.commit()?;
        Ok(())
    }

    /// Whether `voucher` reshared `post`, as told.
    pub fn has_vouch(&self, post: &ObjectId, voucher: &PublicKey) -> Result<bool> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM vouches WHERE post = ?1 AND voucher = ?2",
                params![post.to_string(), voucher.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// How many people reshared `post`, as told.
    pub fn vouches_for(&self, post: &ObjectId) -> Result<usize> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM vouches WHERE post = ?1",
            params![post.to_string()],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    /// The resharers of `post` in the order this node first heard of them,
    /// with when: oldest first.
    pub fn resharers_in_order(&self, post: &ObjectId) -> Result<Vec<(PublicKey, i64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT voucher, first_ms FROM vouches WHERE post = ?1
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

    /// Forgets the reshares of one of the owner's posts, and the credits
    /// given for it, once it's deleted. Vouches already given run out.
    pub fn forget_vouches_of(&self, post: &ObjectId) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let post = post.to_string();
        tx.execute("DELETE FROM vouches WHERE post = ?1", params![post])?;
        tx.execute("DELETE FROM credits_given WHERE post = ?1", params![post])?;
        tx.execute(DROP_UNUSED_PROFILES, [])?;
        tx.commit()?;
        Ok(())
    }

    // ---- vouches others gave the owner ----

    /// Holds a vouch for the owner, unless one of the same voucher and kind
    /// valid as long or longer is held, and with it the voucher's profile.
    /// Then drops those that ran out, and past `max`, those that run out
    /// soonest. Says whether it's held.
    pub fn hold_vouch(&self, vouch: &HeldVouch, max: usize) -> Result<bool> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let changed = tx.execute(
            "INSERT INTO held_vouches (voucher, kind, signed, valid_until_ms, heard_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(voucher, kind) DO UPDATE SET
                 signed = excluded.signed, valid_until_ms = excluded.valid_until_ms,
                 heard_ms = excluded.heard_ms
             WHERE excluded.valid_until_ms > held_vouches.valid_until_ms",
            params![
                vouch.voucher.to_string(),
                kind_str(vouch.kind),
                serde_json::to_string(vouch.signed)?,
                vouch.valid_until_ms,
                vouch.now_ms,
            ],
        )?;
        keep_profile(&tx, &vouch.voucher, vouch.profile, vouch.profile_ms)?;
        tx.execute(
            "DELETE FROM held_vouches WHERE valid_until_ms <= ?1",
            params![vouch.now_ms],
        )?;
        tx.execute(
            "DELETE FROM held_vouches WHERE rowid IN (
                 SELECT rowid FROM held_vouches
                 ORDER BY valid_until_ms DESC LIMIT -1 OFFSET ?1)",
            params![max as i64],
        )?;
        tx.execute(DROP_UNUSED_PROFILES, [])?;
        tx.commit()?;
        Ok(changed > 0)
    }

    /// Drops the vouches held for the owner that ran out by `now_ms`.
    pub fn drop_expired_held(&self, now_ms: i64) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM held_vouches WHERE valid_until_ms <= ?1",
            params![now_ms],
        )?;
        tx.execute(DROP_UNUSED_PROFILES, [])?;
        tx.commit()?;
        Ok(())
    }

    /// Those whose vouches of `kind` for the owner are held and valid at
    /// `now_ms`, longest held first.
    pub fn held_from(&self, kind: VouchKind, now_ms: i64) -> Result<Vec<PublicKey>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT voucher FROM held_vouches WHERE kind = ?1 AND valid_until_ms > ?2
             ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![kind_str(kind), now_ms], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.parse()?);
        }
        Ok(out)
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

    /// Keeps whom the owner credited with finding `post` early.
    pub fn add_credits(&self, post: &ObjectId, finders: &[PublicKey]) -> Result<()> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        for finder in finders {
            tx.execute(
                "INSERT OR IGNORE INTO credits_given (post, finder) VALUES (?1, ?2)",
                params![post.to_string(), finder.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Whom the owner credited with finding `post` early, in order.
    pub fn credits_for(&self, post: &ObjectId) -> Result<Vec<PublicKey>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT finder FROM credits_given WHERE post = ?1 ORDER BY rowid")?;
        let rows = stmt.query_map(params![post.to_string()], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.parse()?);
        }
        Ok(out)
    }

    /// Everyone the owner credited with finding a post early, once each.
    pub fn credited_finders(&self) -> Result<Vec<PublicKey>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT finder FROM credits_given GROUP BY finder ORDER BY MIN(rowid)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.parse()?);
        }
        Ok(out)
    }

    // ---- the owner's own vouches ----

    /// The owner's vouch of `kind` for `subject`, if one is kept.
    pub fn given_vouch(&self, subject: &PublicKey, kind: VouchKind) -> Result<Option<GivenVouch>> {
        Ok(self
            .given_vouches_where(
                "subject = ?1 AND kind = ?2",
                params![subject.to_string(), kind_str(kind)],
            )?
            .pop())
    }

    /// The owner's vouches, oldest first.
    pub fn given_vouches(&self) -> Result<Vec<GivenVouch>> {
        self.given_vouches_where("1", params![])
    }

    /// The owner's vouches valid at `now_ms` whose subjects' nodes don't
    /// have them yet.
    pub fn untold_given_vouches(&self, now_ms: i64) -> Result<Vec<GivenVouch>> {
        self.given_vouches_where("told = 0 AND valid_until_ms > ?1", params![now_ms])
    }

    fn given_vouches_where(
        &self,
        filter: &str,
        args: impl rusqlite::Params,
    ) -> Result<Vec<GivenVouch>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT subject, kind, signed, valid_until_ms, renew_ms, told FROM given_vouches
             WHERE {filter} ORDER BY rowid"
        ))?;
        let rows = stmt.query_map(args, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, bool>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (subject, kind, signed, valid_until_ms, renew_ms, told) = row?;
            let Some(kind) = kind_from(&kind) else {
                continue;
            };
            out.push(GivenVouch {
                subject: subject.parse()?,
                kind,
                signed: serde_json::from_str(&signed).context("corrupt stored vouch")?,
                valid_until_ms,
                renew_ms,
                told,
            });
        }
        Ok(out)
    }

    /// Keeps the owner's new or renewed vouch, untold.
    pub fn put_given_vouch(&self, vouch: &GivenVouch) -> Result<()> {
        self.conn().execute(
            "INSERT INTO given_vouches (subject, kind, signed, valid_until_ms, renew_ms, told)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(subject, kind) DO UPDATE SET
                 signed = excluded.signed, valid_until_ms = excluded.valid_until_ms,
                 renew_ms = excluded.renew_ms, told = excluded.told",
            params![
                vouch.subject.to_string(),
                kind_str(vouch.kind),
                serde_json::to_string(&vouch.signed)?,
                vouch.valid_until_ms,
                vouch.renew_ms,
                vouch.told,
            ],
        )?;
        Ok(())
    }

    /// Notes that the subject's node has the owner's vouch `signed`, unless
    /// it was renewed meanwhile.
    pub fn given_vouch_told(&self, signed: &SignedObject) -> Result<()> {
        self.conn().execute(
            "UPDATE given_vouches SET told = 1 WHERE signed = ?1",
            params![serde_json::to_string(signed)?],
        )?;
        Ok(())
    }

    /// Forgets the owner's vouches that ran out by `now_ms`.
    pub fn drop_expired_given(&self, now_ms: i64) -> Result<()> {
        self.conn().execute(
            "DELETE FROM given_vouches WHERE valid_until_ms <= ?1",
            params![now_ms],
        )?;
        Ok(())
    }

    /// The owner's reshares in the outbox: each post's author, and the
    /// reshare's date.
    pub fn own_reshare_dates(&self, me: &PublicKey) -> Result<Vec<(PublicKey, i64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT objects.signed, objects.published_ms
             FROM outbox JOIN objects ON objects.id = outbox.id
             WHERE objects.kind = 'Announce' AND objects.actor = ?1",
        )?;
        let rows = stmt.query_map(params![me.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (json, published_ms) = row?;
            let obj: SignedObject = serde_json::from_str(&json).context("corrupt stored object")?;
            // Checked when it was stored; only its contents are needed here.
            if let Ok(Payload::Announce(a)) = serde_json::from_slice(obj.payload_bytes()) {
                out.push((a.object_actor, published_ms));
            }
        }
        Ok(out)
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

    /// The vouch heard of last from each voucher for the owner, of those
    /// valid at `now_ms`, with the voucher's profile, newest first, at most
    /// `limit` vouchers. Those without a kept profile are left out.
    pub fn newest_vouches(&self, now_ms: i64, limit: usize) -> Result<Vec<KeptVouch>> {
        let conn = self.conn();
        // SQLite takes the other columns from the row with the largest `heard_ms`.
        let mut stmt = conn.prepare(
            "SELECT v.voucher, v.signed, p.profile, MAX(v.heard_ms) AS newest
             FROM held_vouches v JOIN voucher_profiles p ON p.actor = v.voucher
             WHERE v.valid_until_ms > ?1
             GROUP BY v.voucher ORDER BY newest DESC, MAX(v.rowid) DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![now_ms, limit as i64], |row| {
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

    /// Everyone who reshared the owner's posts or holds a found-early credit
    /// for them valid at `now_ms`, most recently heard of first: how many
    /// of their reshares are kept, whether they credit the owner, and their
    /// kept profile.
    pub fn vouchers(&self, now_ms: i64) -> Result<Vec<Voucher>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT v.voucher, SUM(v.reshare), SUM(v.credit), p.profile, MAX(v.heard) AS heard
             FROM (SELECT voucher, 1 AS reshare, 0 AS credit, heard_ms AS heard FROM vouches
                   UNION ALL
                   SELECT voucher, 0, 1, heard_ms FROM held_vouches
                   WHERE kind = 'found_early' AND valid_until_ms > ?1) v
             LEFT JOIN voucher_profiles p ON p.actor = v.voucher
             GROUP BY v.voucher ORDER BY heard DESC",
        )?;
        let rows = stmt.query_map(params![now_ms], |row| {
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

    /// Everything kept of who reshared the owner's posts or vouched for the
    /// owner, and what the owner gave and told, for a backup. Not the side:
    /// a restored node gathers it again. Nor the owner's own vouches: a
    /// restored node signs them again.
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
        let mut stmt = conn.prepare("SELECT post, finder FROM credits_given ORDER BY rowid")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (post, finder) = row?;
            out.credits_given.push(CreditRecord {
                post: post.parse()?,
                finder: finder.parse()?,
            });
        }
        let mut stmt = conn.prepare(
            "SELECT voucher, kind, signed, valid_until_ms, heard_ms FROM held_vouches
             ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        for row in rows {
            let (voucher, kind, signed, valid_until_ms, heard_ms) = row?;
            out.held.push(HeldRecord {
                voucher: voucher.parse()?,
                kind,
                signed: serde_json::from_str(&signed).context("corrupt stored vouch")?,
                valid_until_ms,
                heard_ms,
            });
        }
        let mut stmt = conn.prepare("SELECT announce FROM told_reshares")?;
        for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
            out.told_reshares.push(row?.parse()?);
        }
        Ok(out)
    }

    /// Puts back what `vouch_tables` gave, into a new node's database. A
    /// reshare, vouch or profile that doesn't verify, or names someone other
    /// than its signer, is skipped, and so is a vouch that ran out. Returns
    /// how many vouches were put back.
    pub fn restore_vouch_tables(&self, tables: &VouchTables) -> Result<usize> {
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        let signed_by = |obj: &SignedObject, who: &PublicKey| {
            obj.verify().is_ok_and(|payload| payload.actor() == *who)
        };
        let mut vouches = 0;
        for v in &tables.vouches {
            // A reshare is signed by its resharer.
            if v.kind != "reshare" || !signed_by(&v.signed, &v.voucher) {
                tracing::warn!("skipping a reshare in the backup that doesn't check out");
                continue;
            }
            tx.execute(
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
        let now = super::now_ms();
        for h in &tables.held {
            let checks = match h.signed.verify() {
                Ok(Payload::Vouch(v)) => {
                    v.actor == h.voucher
                        && kind_from(&h.kind) == Some(v.kind)
                        && osp_proto::parse_rfc3339_ms(&v.valid_until) == Some(h.valid_until_ms)
                }
                _ => false,
            };
            if !checks {
                tracing::warn!("skipping a vouch in the backup that doesn't check out");
                continue;
            }
            if h.valid_until_ms <= now {
                continue;
            }
            vouches += tx.execute(
                "INSERT OR IGNORE INTO held_vouches
                     (voucher, kind, signed, valid_until_ms, heard_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    h.voucher.to_string(),
                    h.kind,
                    serde_json::to_string(&h.signed)?,
                    h.valid_until_ms,
                    h.heard_ms,
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
            tx.execute(
                "INSERT OR IGNORE INTO credits_given (post, finder) VALUES (?1, ?2)",
                params![c.post.to_string(), c.finder.to_string()],
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

/// Someone who reshared the owner's posts or credits the owner: how many
/// reshares of theirs are kept, whether they hold a found-early credit
/// (1) or not (0), and their kept profile.
pub(crate) struct Voucher {
    pub actor: PublicKey,
    pub reshares: u32,
    pub credits: u32,
    pub profile: Option<SignedObject>,
}

/// What a backup holds of vouches (ADR 0029), as the tables keep them.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct VouchTables {
    /// Reshares of the owner's posts.
    pub vouches: Vec<VouchRecord>,
    pub voucher_profiles: Vec<VoucherProfileRecord>,
    pub credits_given: Vec<CreditRecord>,
    pub told_reshares: Vec<ObjectId>,
    /// Vouches others gave the owner.
    #[serde(default)]
    pub held: Vec<HeldRecord>,
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
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct HeldRecord {
    voucher: PublicKey,
    kind: String,
    signed: SignedObject,
    valid_until_ms: i64,
    heard_ms: i64,
}

/// Keeps a voucher's profile, if it's newer than the one kept.
fn keep_profile(
    conn: &rusqlite::Connection,
    actor: &PublicKey,
    profile: &SignedObject,
    published_ms: i64,
) -> Result<()> {
    conn.execute(
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

#[cfg(test)]
mod tests {
    use iroh::SecretKey;
    use osp_proto::{Announce, Payload, Vouch};

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
            post,
            voucher: who.public(),
            signed: &signed,
            post_ms,
            now_ms,
            profile: &signed,
            profile_ms: now_ms,
        };
        db.add_vouch(&vouch, max).unwrap();
    }

    fn resharers(db: &Db) -> Vec<ObjectId> {
        let conn = db.conn();
        let mut stmt = conn
            .prepare("SELECT post FROM vouches ORDER BY first_ms, rowid")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|p| p.unwrap().parse().unwrap())
            .collect()
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
            resharers(&db),
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
        assert_eq!(db.vouches_for(&ObjectId::of(b"post")).unwrap(), 2);
    }

    /// Holds `who`'s vouch for someone, valid until `until`, heard at `now`.
    fn hold(db: &Db, who: &SecretKey, until: i64, now: i64, max: usize) -> bool {
        let signed = SignedObject::sign(
            who,
            &Payload::Vouch(Vouch {
                actor: who.public(),
                subject: SecretKey::generate().public(),
                kind: VouchKind::Reshare,
                valid_until: osp_proto::rfc3339_from_ms(until as u64),
                published: osp_proto::rfc3339_from_ms(now as u64),
            }),
        )
        .unwrap();
        let profile = SignedObject::sign(
            who,
            &Payload::Announce(Announce {
                actor: who.public(),
                object: ObjectId::of(b"stands in for a profile"),
                object_actor: who.public(),
                published: osp_proto::now_rfc3339(),
                hosting: false,
            }),
        )
        .unwrap();
        db.hold_vouch(
            &HeldVouch {
                voucher: who.public(),
                kind: VouchKind::Reshare,
                signed: &signed,
                valid_until_ms: until,
                now_ms: now,
                profile: &profile,
                profile_ms: now,
            },
            max,
        )
        .unwrap()
    }

    #[test]
    fn a_held_vouch_is_replaced_only_by_one_valid_longer_and_goes_when_it_runs_out() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let (bob, carol, dan) = (
            SecretKey::generate(),
            SecretKey::generate(),
            SecretKey::generate(),
        );
        assert!(hold(&db, &bob, 2_000, 100, 10));
        // An older one sent again changes nothing; a renewal replaces it.
        assert!(!hold(&db, &bob, 1_500, 110, 10));
        assert!(hold(&db, &bob, 3_000, 120, 10));
        assert!(hold(&db, &carol, 1_000, 130, 10));
        let held = |now| db.held_from(VouchKind::Reshare, now).unwrap();
        assert_eq!(held(500), [bob.public(), carol.public()]);
        // Carol's ran out: not answered, then dropped with her profile.
        assert_eq!(held(1_000), [bob.public()]);
        assert_eq!(db.newest_vouches(1_000, 10).unwrap().len(), 1);
        db.drop_expired_held(1_000).unwrap();
        assert!(db.voucher_profile(&carol.public()).unwrap().is_none());
        assert!(!db.knows_voucher(&carol.public()).unwrap());
        // Past the most, those that run out soonest go.
        assert!(hold(&db, &dan, 5_000, 140, 1));
        assert_eq!(held(500), [dan.public()]);
    }
}
