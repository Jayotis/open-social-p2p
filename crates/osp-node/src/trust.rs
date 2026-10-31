//! Trusted followers (ADR 0013): followers ask to be trusted, and the owner
//! allows or declines; the owner can also trust someone they follow. Trust
//! is a relationship, a person and the nodes allowed for them, and grants
//! nothing on its own. Its only use so far is naming trusted followers as
//! hosts more often (`Heard::pick_hosts`, `Db::add_swarm_member`).

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{EndpointId, PublicKey};
use osp_proto::{
    Payload, SignedObject,
    rpc::{ClientResponse, Reached, TrustInfo},
    social::{MAX_TRUST_NOTE_CHARS, SocialRequest},
};
use tracing::{debug, info};

use crate::{
    db::{TrustRow, now_ms},
    node::Core,
    social::Contact,
};

/// New requests accepted an hour, at most. Anyone can follow, and node IDs
/// cost nothing, so this keeps a crowd of fakes from filling the list fast.
pub(crate) const MAX_NEW_REQUESTS_PER_HOUR: usize = 10;
/// Requests kept waiting for an answer, at most; when full, the oldest goes.
pub(crate) const MAX_WAITING: usize = 50;
/// A request not answered in this long goes.
pub(crate) const WAITING_KEPT: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// A node declined is ignored for this long: its requests are dropped.
pub(crate) const DECLINED_IGNORED: Duration = Duration::from_secs(30 * 24 * 60 * 60);

impl Core {
    /// Takes a request from `remote` to be a trusted follower. Refuses one
    /// that isn't valid: not from a follower, or with a profile that isn't
    /// signed or doesn't list `remote`. A valid one gets the same reply
    /// whether it's kept, a repeat, or dropped: from a node allowed or
    /// declined, or over a limit.
    pub(crate) fn asked_trust(
        &self,
        remote: EndpointId,
        profile: &SignedObject,
        note: &str,
    ) -> Result<()> {
        let note = check_note(note)?;
        ensure!(
            self.db.is_follower(&remote)?,
            "only a follower can ask to be trusted"
        );
        // Checked against the keys we know of, if we follow that identity
        // (ADR 0010). Otherwise only its signatures are: the key table is
        // for identities we follow and objects we keep.
        let following = self
            .db
            .follows()?
            .iter()
            .any(|f| f.actor == profile.signer());
        let payload = if following {
            self.verify_received(profile)?
        } else {
            profile.verify()?
        };
        let Payload::Person(person) = payload else {
            bail!("not a profile");
        };
        ensure!(
            person.nodes.contains(&remote),
            "the profile doesn't list your node"
        );
        ensure!(
            person.actor != self.signer.identity(),
            "a node of this identity can't ask to trust it"
        );
        let ask = TrustRow {
            node: remote,
            actor: person.actor,
            name: plain(&person.name),
            note,
            at_ms: now_ms(),
        };
        let kept = self
            .db
            .add_trust_request(&ask, self.sim.trust_requests_per_hour)?;
        debug!(
            node = %remote.fmt_short(),
            actor = %person.actor.fmt_short(),
            kept,
            "asked to be a trusted follower"
        );
        Ok(())
    }

    /// Requests waiting, newest first, and the nodes trusted, with what
    /// this node knows of each.
    pub(crate) fn trust_list(&self) -> Result<ClientResponse> {
        let now = now_ms();
        let requests = self.db.trust_requests(now)?;
        let trusted = self.db.trusted()?;
        let followed: HashSet<PublicKey> = self.db.follows()?.iter().map(|f| f.actor).collect();
        let instant = Instant::now();
        let info = |row: &TrustRow| -> Result<TrustInfo> {
            let reached = match self
                .heard
                .contact(&row.node, instant, self.sim.heard_lately)
            {
                Contact::Direct => Some(Reached::Direct),
                Contact::Relayed => Some(Reached::Relay),
                Contact::NotLately => None,
            };
            Ok(TrustInfo {
                node: row.node,
                actor: row.actor,
                name: row.name.clone(),
                note: row.note.clone(),
                secs_ago: secs_since(row.at_ms, now),
                followed: followed.contains(&row.actor),
                trusted_elsewhere: trusted
                    .iter()
                    .any(|t| t.actor == row.actor && t.node != row.node),
                follower_secs: self
                    .db
                    .follower_since(&row.node)?
                    .map(|since| secs_since(since, now)),
                reached,
                copies: self.db.swarm_copies(&row.node)?,
            })
        };
        Ok(ClientResponse::Trust {
            requests: requests.iter().map(info).collect::<Result<_>>()?,
            trusted: trusted.iter().map(info).collect::<Result<_>>()?,
        })
    }

    pub(crate) fn answer_trust(&self, node: &EndpointId, allow: bool) -> Result<()> {
        ensure!(
            self.db.answer_trust(node, allow, now_ms())?,
            "no request from {} is waiting",
            node.fmt_short()
        );
        info!(node = %node.fmt_short(), allow, "answered a request to be a trusted follower");
        Ok(())
    }

    /// Trusts someone we follow, on the nodes their profile lists that
    /// follow us. Their profile came from following them, so it's theirs.
    pub(crate) fn trust_followed(&self, actor: &PublicKey) -> Result<()> {
        ensure!(
            self.db.follows()?.iter().any(|f| f.actor == *actor),
            "you can only trust someone you follow this way; they can ask instead"
        );
        let profile = self
            .db
            .profile(actor)?
            .context("this node doesn't have their profile yet; try again after a sync")?;
        let Payload::Person(person) = profile.verify()? else {
            bail!("their stored profile isn't a profile");
        };
        let mut nodes = Vec::new();
        for node in &person.nodes {
            if self.db.is_follower(node)? {
                nodes.push(*node);
            }
        }
        ensure!(
            !nodes.is_empty(),
            "{}'s node doesn't follow you, so there's no node to trust yet",
            person.name
        );
        self.db
            .trust_nodes(actor, &plain(&person.name), &nodes, now_ms())?;
        info!(actor = %actor.fmt_short(), nodes = nodes.len(), "trusted someone followed");
        Ok(())
    }

    pub(crate) fn untrust(&self, actor: &PublicKey) -> Result<()> {
        let nodes = self.db.untrust(actor)?;
        ensure!(nodes > 0, "{} isn't trusted", actor.fmt_short());
        info!(actor = %actor.fmt_short(), nodes, "took trust back");
        Ok(())
    }

    /// Asks someone we follow to trust our owner. Their node subscribed us
    /// as a follower when we followed them; only a follower may ask.
    pub(crate) async fn ask_trust(&self, actor: &PublicKey, note: &str) -> Result<()> {
        let note = check_note(note)?;
        let follow = self
            .db
            .follows()?
            .into_iter()
            .find(|f| f.actor == *actor)
            .context("you can only ask someone you follow")?;
        let req = SocialRequest::AskTrust {
            profile: self.profile.clone(),
            note,
        };
        self.social_request(&follow.addr, &req).await?;
        Ok(())
    }
}

/// A note made plain, and checked for length.
fn check_note(note: &str) -> Result<String> {
    let note = plain(note);
    ensure!(
        note.chars().count() <= MAX_TRUST_NOTE_CHARS,
        "the note is longer than {MAX_TRUST_NOTE_CHARS} characters"
    );
    Ok(note)
}

/// Text from another node made safe to show as plain text: control
/// characters, which could move a terminal's cursor, and characters that
/// reverse the direction text is shown in, become spaces.
pub(crate) fn plain(text: &str) -> String {
    let reorders = |c: char| matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    text.chars()
        .map(|c| {
            if c.is_control() || reorders(c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn secs_since(ms: i64, now: i64) -> u64 {
    ((now - ms) / 1000).max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_and_names_are_shown_as_plain_text() {
        assert_eq!(
            plain("  Sam\u{1b}[2J, from\nthe club\u{202e} "),
            "Sam [2J, from the club"
        );
        assert_eq!(plain("Zoë 🧗"), "Zoë 🧗");
        assert!(check_note(&"é".repeat(MAX_TRUST_NOTE_CHARS)).is_ok());
        assert!(check_note(&"é".repeat(MAX_TRUST_NOTE_CHARS + 1)).is_err());
    }
}
