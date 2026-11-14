//! What a profile page shows of a person (ADR 0032): how many of their
//! posts, media and reshares this node holds, and one page of them at a time.

use anyhow::{Context, Result};
use iroh::PublicKey;
use osp_proto::{ObjectId, SignedObject, rpc::ProfileItemKind};
use rusqlite::{OptionalExtension, params};

use super::Db;

/// A post with media is a `Note` that names a file. A note's only refs are
/// its files (`add_refs`).
const HAS_MEDIA: &str = "EXISTS (SELECT 1 FROM refs WHERE refs.id = objects.id)";

/// How many of a person's things this node holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProfileCounts {
    pub posts: u64,
    pub media: u64,
    pub reshares: u64,
}

impl Db {
    /// How many of `actor`'s posts, posts with media, and reshares are here.
    pub fn profile_counts(&self, actor: &PublicKey) -> Result<ProfileCounts> {
        let sql = format!(
            "SELECT COALESCE(SUM(kind = 'Note'), 0),
                    COALESCE(SUM(kind = 'Note' AND {HAS_MEDIA}), 0),
                    COALESCE(SUM(kind = 'Announce'), 0)
             FROM objects WHERE actor = ?1 AND kind IN ('Note', 'Announce')"
        );
        let (posts, media, reshares): (i64, i64, i64) =
            self.conn()
                .query_row(&sql, params![actor.to_string()], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
        Ok(ProfileCounts {
            posts: posts as u64,
            media: media as u64,
            reshares: reshares as u64,
        })
    }

    /// Up to `limit` of `actor`'s things of `kind`, newest first, published
    /// before `before`, one of theirs. Two published in the same millisecond
    /// are ordered by ID, so a page never skips or repeats one.
    pub fn profile_items(
        &self,
        actor: &PublicKey,
        kind: ProfileItemKind,
        before: Option<&ObjectId>,
        limit: u32,
    ) -> Result<Vec<SignedObject>> {
        let conn = self.conn();
        let actor = actor.to_string();
        let which = match kind {
            ProfileItemKind::Posts => "kind = 'Note'".to_string(),
            ProfileItemKind::Media => format!("kind = 'Note' AND {HAS_MEDIA}"),
            ProfileItemKind::Reshares => "kind = 'Announce'".to_string(),
        };
        // `objects_by_actor_date` holds them in this order, so a page is
        // read straight from it, however many there are.
        let (after, from) = match before {
            None => ("", None),
            Some(id) => {
                let ms: i64 = conn
                    .query_row(
                        "SELECT published_ms FROM objects
                         WHERE id = ?1 AND actor = ?2 AND kind IN ('Note', 'Announce')",
                        params![id.to_string(), actor],
                        |row| row.get(0),
                    )
                    .optional()?
                    .context("that post isn't here any more; start from the newest")?;
                (
                    "AND (published_ms, id) < (?3, ?4)",
                    Some((ms, id.to_string())),
                )
            }
        };
        let sql = format!(
            "SELECT signed FROM objects
             WHERE actor = ?1 AND {which} {after}
             ORDER BY published_ms DESC, id DESC LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let read = |row: &rusqlite::Row| row.get::<_, String>(0);
        let rows: Vec<String> = match from {
            None => stmt
                .query_map(params![actor, limit], read)?
                .collect::<rusqlite::Result<_>>()?,
            Some((ms, id)) => stmt
                .query_map(params![actor, limit, ms, id], read)?
                .collect::<rusqlite::Result<_>>()?,
        };
        rows.iter()
            .map(|r| serde_json::from_str(r).context("corrupt stored object"))
            .collect()
    }

    /// Whether `actor` is trusted on any of their nodes (ADR 0013): someone
    /// this node's owner knows.
    pub fn is_trusted(&self, actor: &PublicKey) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM trust WHERE actor = ?1 AND state = 'allowed')",
            params![actor.to_string()],
            |row| row.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;
    use osp_proto::{Note, Payload};

    use super::*;

    /// Kim's `n`th post, published `n` seconds in, or two at once.
    fn note(kim: &SecretKey, n: u64, ms: u64) -> (SignedObject, Payload) {
        let payload = Payload::Note(Note {
            actor: kim.public(),
            content: format!("post {n}"),
            published: osp_proto::rfc3339_from_ms(ms),
            attachment: vec![],
            in_reply_to: None,
        });
        (SignedObject::sign(kim, &payload).unwrap(), payload)
    }

    #[test]
    fn ten_thousand_posts_are_counted_and_paged_through_without_a_gap() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let kim = SecretKey::from_bytes(&[3; 32]);
        let start = 1_700_000_000_000;
        // Each insert commits; without waiting for the disk, these take a moment.
        db.conn().execute_batch("PRAGMA synchronous = OFF").unwrap();
        let mut dates = std::collections::HashMap::new();
        for n in 0..10_000u64 {
            // Every tenth pair published in the same millisecond.
            let ms = start + n * 1000 - (n % 10 == 1) as u64 * 1000;
            let (obj, payload) = note(&kim, n, ms);
            db.insert_object(&obj, &payload).unwrap();
            dates.insert(obj.id(), ms);
        }

        let counts = db.profile_counts(&kim.public()).unwrap();
        assert_eq!(
            counts,
            ProfileCounts {
                posts: 10_000,
                media: 0,
                reshares: 0
            }
        );
        let nobody = SecretKey::from_bytes(&[4; 32]).public();
        assert_eq!(
            db.profile_counts(&nobody).unwrap(),
            ProfileCounts::default()
        );

        let mut seen = std::collections::HashSet::new();
        let mut before = None;
        let mut last_ms = u64::MAX;
        loop {
            let page = db
                .profile_items(&kim.public(), ProfileItemKind::Posts, before.as_ref(), 50)
                .unwrap();
            assert!(page.len() <= 50);
            for obj in &page {
                // Not verified: in a debug build 10,000 signatures take a minute.
                let ms = dates[&obj.id()];
                assert!(ms <= last_ms, "out of order");
                last_ms = ms;
                assert!(seen.insert(obj.id()), "repeated");
            }
            match page.last() {
                Some(obj) if page.len() == 50 => before = Some(obj.id()),
                _ => break,
            }
        }
        assert_eq!(seen.len(), 10_000);
    }
}
