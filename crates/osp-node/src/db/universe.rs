//! What the Universe reads (ADR 0033): the profiles held with what this node
//! holds of each, and the public signed acts between people. Only reads;
//! `universe.rs` picks the fields a snapshot may show.

use std::collections::HashMap;

use anyhow::{Context, Result};
use iroh::{EndpointId, PublicKey};
use osp_proto::SignedObject;
use rusqlite::{params, params_from_iter};

use super::{Db, NOT_OPENED, profiles::HAS_MEDIA};

/// A profile held, and what this node holds of its person.
pub(crate) struct HeldPerson {
    pub profile: SignedObject,
    pub posts: u64,
    pub media: u64,
    pub reshares: u64,
    pub newest_ms: Option<i64>,
}

impl Db {
    /// How many objects of `kind` (`Note`, `Announce`) each person published
    /// from `since_ms` on, as this node holds them, `me` included.
    pub fn recent_counts(&self, kind: &str, since_ms: i64) -> Result<Vec<(PublicKey, u64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT actor, COUNT(*) FROM objects
             WHERE kind = ?1 AND published_ms >= ?2 GROUP BY actor",
        )?;
        let rows = stmt
            .query_map(params![kind, since_ms], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(actor, n)| Some((actor.parse().ok()?, n as u64)))
            .collect())
    }

    /// The posts with media published from `since_ms` on. Unchecked here;
    /// the caller verifies each and reads its attachments.
    pub fn recent_media_posts(&self, since_ms: i64) -> Result<Vec<SignedObject>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT signed FROM objects
             WHERE kind = 'Note' AND published_ms >= ?1 AND {HAS_MEDIA}"
        ))?;
        let rows = stmt
            .query_map(params![since_ms], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.iter()
            .map(|json| serde_json::from_str(json).context("corrupt stored object"))
            .collect()
    }

    /// Every profile held other than `me`'s, with how many posts, posts with
    /// media and reshares of theirs are here, and their newest's date. That
    /// includes those who vouched for `me` (ADR 0029), whose profiles are
    /// kept apart. Those further back on `me`'s side came with profiles
    /// when it was gathered, but only their vouches are kept.
    pub fn held_people(&self, me: &PublicKey) -> Result<Vec<HeldPerson>> {
        let conn = self.conn();
        let sql = format!(
            "SELECT held.signed,
                    COALESCE(c.posts, 0), COALESCE(c.media, 0), COALESCE(c.reshares, 0),
                    c.newest
             FROM (
               SELECT actor, signed FROM profiles
               WHERE actor NOT IN (SELECT actor FROM opened_people)
               UNION ALL
               SELECT actor, profile FROM voucher_profiles
               WHERE actor NOT IN (SELECT actor FROM profiles
                                   WHERE actor NOT IN (SELECT actor FROM opened_people))
             ) AS held
             LEFT JOIN (
               SELECT actor,
                      SUM(kind = 'Note') AS posts,
                      SUM(kind = 'Note' AND {HAS_MEDIA}) AS media,
                      SUM(kind = 'Announce') AS reshares,
                      MAX(published_ms) AS newest
               FROM objects WHERE kind IN ('Note', 'Announce') AND {NOT_OPENED}
               GROUP BY actor
             ) AS c ON c.actor = held.actor
             WHERE held.actor != ?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![me.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (signed, posts, media, reshares, newest_ms) = row?;
            out.push(HeldPerson {
                profile: serde_json::from_str(&signed).context("corrupt stored profile")?,
                posts: posts as u64,
                media: media as u64,
                reshares: reshares as u64,
                newest_ms,
            });
        }
        Ok(out)
    }

    /// What this node holds of `me`: the same counts as `held_people`.
    pub fn own_counts(&self, me: &PublicKey) -> Result<(u64, u64, u64, Option<i64>)> {
        let sql = format!(
            "SELECT COALESCE(SUM(kind = 'Note'), 0),
                    COALESCE(SUM(kind = 'Note' AND {HAS_MEDIA}), 0),
                    COALESCE(SUM(kind = 'Announce'), 0),
                    MAX(published_ms)
             FROM objects WHERE actor = ?1 AND kind IN ('Note', 'Announce')"
        );
        let (posts, media, reshares, newest): (i64, i64, i64, Option<i64>) = self
            .conn()
            .query_row(&sql, params![me.to_string()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
        Ok((posts as u64, media as u64, reshares as u64, newest))
    }

    /// The public signed acts between people this node holds, from
    /// `since_ms` on: reshares in the outboxes and directories' lists it
    /// has, reshares and found-early credits vouching for its owner, the
    /// credits its owner gave, and the vouches that place its owner's side.
    /// Unchecked here; the caller verifies each.
    pub fn public_acts(&self, since_ms: i64) -> Result<Vec<SignedObject>> {
        let conn = self.conn();
        let mut out = Vec::new();
        let mut read = |sql: &str, since: Option<i64>| -> Result<()> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt
                .query_map(params_from_iter(since), |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for json in rows {
                out.push(serde_json::from_str(&json).context("corrupt stored object")?);
            }
            Ok(())
        };
        read(
            "SELECT signed FROM objects WHERE kind = 'Announce' AND published_ms >= ?1",
            Some(since_ms),
        )?;
        read(
            "SELECT signed FROM vouches WHERE heard_ms >= ?1",
            Some(since_ms),
        )?;
        // Neither keeps a date of its own: each act's date is in it.
        read("SELECT signed FROM credits_given", None)?;
        read("SELECT signed FROM side_vouches", None)?;
        Ok(out)
    }

    /// The Universe's places, as last saved.
    pub fn universe_places(&self) -> Result<Option<Vec<u8>>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT saved FROM universe_places WHERE id = 0")?;
        let mut rows = stmt.query([])?;
        Ok(match rows.next()? {
            Some(row) => Some(row.get(0)?),
            None => None,
        })
    }

    pub fn set_universe_places(&self, saved: &[u8]) -> Result<()> {
        self.conn().execute(
            "INSERT INTO universe_places (id, saved) VALUES (0, ?1)
             ON CONFLICT(id) DO UPDATE SET saved = excluded.saved",
            params![saved],
        )?;
        Ok(())
    }

    /// The directory each person a directory brought came from.
    pub fn found_directories(&self) -> Result<HashMap<PublicKey, EndpointId>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT actor, directory FROM found_people")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (actor, directory) = row?;
            out.insert(actor.parse()?, directory.parse()?);
        }
        Ok(out)
    }
}
