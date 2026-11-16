//! The owner's Activity (ADR 0038 §3): one inbox on the node, so each of
//! their devices sees the same entries, read and unread. Entries are noted
//! where things happen (`Db::note_activity`); what each says is read here,
//! from where it's kept, when it's shown. One whose subject is gone (a like
//! taken back, a reply deleted, a follower gone) is dropped then.
//!
//! The keys: `reply:<reply>`, `liked:<post>`, `reshared:<post>`,
//! `followed:<node>`, `knows:<node>`, `published:<reply>`.

use anyhow::Result;
use iroh::{EndpointId, PublicKey};
use osp_proto::{
    ObjectId, Payload, Person, SignedObject,
    rpc::{
        ActivityEntry, ActivityPerson, ActivityWhat, ClientResponse, MAX_ACTIVITY_NAMES,
        MAX_ACTIVITY_PAGE, SentStatus,
    },
};

use crate::{db::Held, node::Core, trust::plain};

/// How much of a post's text an entry carries, in characters.
const POST_TEXT_CHARS: usize = 140;

impl Core {
    /// A page of the owner's Activity, the latest change first, and how many
    /// entries are unread in all.
    pub(crate) fn activity(
        &self,
        before: Option<u64>,
        limit: Option<u32>,
    ) -> Result<ClientResponse> {
        let limit = limit
            .unwrap_or(MAX_ACTIVITY_PAGE)
            .clamp(1, MAX_ACTIVITY_PAGE) as usize;
        let mut entries = Vec::new();
        let mut cursor = before;
        // Rows whose subject is gone are dropped and passed over, so a page
        // may take more than one read. `more` can be true with nothing left
        // to show, if what's left is gone; the next page then says so.
        let more = 'pages: loop {
            let rows = self.db.activity_rows(cursor, limit)?;
            let last = rows.len() < limit;
            for row in rows {
                if entries.len() == limit {
                    break 'pages true;
                }
                cursor = Some(row.seq);
                let Some(what) = self.activity_what(&row.key)? else {
                    self.db.forget_activity(&row.key)?;
                    continue;
                };
                entries.push(ActivityEntry {
                    key: row.key,
                    seq: row.seq,
                    at_ms: row.last_ms,
                    read: row.read,
                    what,
                });
            }
            if last {
                break false;
            }
        };
        Ok(ClientResponse::Activity {
            entries,
            unread: self.unread_activity()?,
            more,
        })
    }

    /// How many entries are unread, leaving out (and dropping) those whose
    /// subject is gone.
    pub(crate) fn unread_activity(&self) -> Result<u64> {
        let mut unread = 0;
        for row in self.db.unread_activity()? {
            match self.activity_what(&row.key)? {
                Some(_) => unread += 1,
                None => self.db.forget_activity(&row.key)?,
            }
        }
        Ok(unread)
    }

    /// What the entry `key` says now, if its subject is still there.
    fn activity_what(&self, key: &str) -> Result<Option<ActivityWhat>> {
        let Some((kind, subject)) = key.split_once(':') else {
            return Ok(None);
        };
        let id = || subject.parse::<ObjectId>().ok();
        let node = || subject.parse::<EndpointId>().ok();
        match kind {
            "reply" => id().map_or(Ok(None), |id| self.reply_entry(id)),
            "liked" => id().map_or(Ok(None), |id| self.liked_entry(id)),
            "reshared" => id().map_or(Ok(None), |id| self.reshared_entry(id)),
            "followed" => node().map_or(Ok(None), |n| self.followed_entry(n)),
            "knows" => node().map_or(Ok(None), |n| self.knows_entry(n)),
            "published" => id().map_or(Ok(None), |id| self.published_entry(id)),
            _ => Ok(None),
        }
    }

    fn reply_entry(&self, id: ObjectId) -> Result<Option<ActivityWhat>> {
        let Some(row) = self.db.received_reply(&id)? else {
            return Ok(None);
        };
        let waiting = match row.held {
            Held::Published | Held::Hidden => false,
            Held::Request => true,
            // The owner turned it away; nothing to tell them.
            Held::Declined => return Ok(None),
        };
        let Ok(Payload::Note(note)) = row.reply.signed.verify() else {
            return Ok(None);
        };
        let Some(post_text) = self.own_post_text(&row.reply.post)? else {
            return Ok(None);
        };
        Ok(Some(ActivityWhat::Reply {
            post: row.reply.post,
            post_text,
            reply: id,
            actor: row.reply.actor,
            name: name_in(&row.reply.profile),
            content: note.content,
            waiting,
        }))
    }

    fn liked_entry(&self, post: ObjectId) -> Result<Option<ActivityWhat>> {
        let likes = self.db.likes_of(&post)?;
        if likes.is_empty() {
            return Ok(None);
        }
        let Some(post_text) = self.own_post_text(&post)? else {
            return Ok(None);
        };
        Ok(Some(ActivityWhat::Liked {
            post,
            post_text,
            count: likes.len() as u64,
            people: likes
                .iter()
                .take(MAX_ACTIVITY_NAMES)
                .map(|l| ActivityPerson {
                    actor: l.actor,
                    name: name_in(&l.profile),
                })
                .collect(),
        }))
    }

    fn reshared_entry(&self, post: ObjectId) -> Result<Option<ActivityWhat>> {
        let mut resharers = self.db.resharers_in_order(&post)?;
        if resharers.is_empty() {
            return Ok(None);
        }
        let Some(post_text) = self.own_post_text(&post)? else {
            return Ok(None);
        };
        // The latest first.
        resharers.sort_by_key(|(_, ms)| std::cmp::Reverse(*ms));
        let mut people = Vec::new();
        for (actor, _) in resharers.iter().take(MAX_ACTIVITY_NAMES) {
            let name = self
                .db
                .voucher_profile(actor)?
                .map(|p| name_in(&p))
                .unwrap_or_default();
            people.push(ActivityPerson {
                actor: *actor,
                name,
            });
        }
        Ok(Some(ActivityWhat::Reshared {
            post,
            post_text,
            count: resharers.len() as u64,
            people,
        }))
    }

    fn followed_entry(&self, node: EndpointId) -> Result<Option<ActivityWhat>> {
        if !self.db.is_follower(&node)? {
            return Ok(None);
        }
        Ok(Some(ActivityWhat::Followed {
            node,
            person: self.whose_node(&node)?,
        }))
    }

    fn knows_entry(&self, node: EndpointId) -> Result<Option<ActivityWhat>> {
        let Some(row) = self.db.trust_row(&node)? else {
            return Ok(None);
        };
        Ok(Some(ActivityWhat::KnowsYou {
            node,
            person: ActivityPerson {
                actor: row.actor,
                name: plain(&row.name),
            },
            note: plain(&row.note),
        }))
    }

    fn published_entry(&self, id: ObjectId) -> Result<Option<ActivityWhat>> {
        let Some(sent) = self.db.sent_reply(&id)? else {
            return Ok(None);
        };
        if sent.status != SentStatus::Published {
            return Ok(None);
        }
        let Some(Ok(Payload::Note(note))) = sent.signed.as_ref().map(|s| s.verify()) else {
            return Ok(None);
        };
        Ok(Some(ActivityWhat::ReplyPublished {
            post: sent.post,
            reply: id,
            creator: sent.creator,
            creator_name: self.db.profile_name(&sent.creator)?.map(|n| plain(&n)),
            content: note.content,
        }))
    }

    /// Whose a follower's node is, only where the node knows for sure: the
    /// owner follows someone whose signed profile lists it, or it asked to
    /// be trusted and sent its owner's profile. Never a guess from names
    /// or timing.
    fn whose_node(&self, node: &EndpointId) -> Result<Option<ActivityPerson>> {
        for follow in self.db.follows()? {
            if follow.addr.id == *node {
                return Ok(Some(self.person(follow.actor)?));
            }
        }
        Ok(self.db.trust_row(node)?.map(|row| ActivityPerson {
            actor: row.actor,
            name: plain(&row.name),
        }))
    }

    fn person(&self, actor: PublicKey) -> Result<ActivityPerson> {
        let name = self.db.profile_name(&actor)?.unwrap_or_default();
        Ok(ActivityPerson {
            actor,
            name: plain(&name),
        })
    }

    /// The start of one of the owner's posts' text, if it's still there.
    fn own_post_text(&self, post: &ObjectId) -> Result<Option<String>> {
        let me = self.signer.identity();
        Ok(match self.db.outbox_object(post)?.map(|o| o.verify()) {
            Some(Ok(Payload::Note(n))) if n.actor == me => {
                Some(n.content.chars().take(POST_TEXT_CHARS).collect())
            }
            _ => None,
        })
    }
}

/// The name in a signed profile, made plain; empty if it isn't one.
fn name_in(profile: &SignedObject) -> String {
    match profile.verify() {
        Ok(Payload::Person(Person { name, .. })) => plain(&name),
        _ => String::new(),
    }
}
