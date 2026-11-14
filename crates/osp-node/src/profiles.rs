//! Profiles people want to follow (ADR 0032): editing the owner's from a
//! device, and what a profile page shows of anyone's.

use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{PublicKey, endpoint::RecvStream};
use osp_proto::{
    BlobRef, ObjectId, Payload, ProfileLink,
    rpc::{ClientResponse, IconChange, MAX_PROFILE_PAGE, ProfileInfo, ProfileItemKind, Upload},
    types::{MAX_NAME_BYTES, MAX_PROFILE_LINKS, MAX_SUMMARY_BYTES},
};
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

use crate::{
    config::Config,
    db::BlobClass,
    media,
    node::{Core, ProfileFields, own_profile},
};

/// Largest photo a device may send for a profile picture. The app shrinks
/// it to about 1,024 pixels first, a few hundred kilobytes.
pub(crate) const MAX_ICON_UPLOAD: u64 = 16 << 20;
/// How long the photo may take to arrive.
const ICON_UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// What `SetProfile` asked for, checked and tidied: names and links trimmed,
/// an empty summary none.
struct Edit {
    name: String,
    summary: Option<String>,
    links: Vec<ProfileLink>,
}

fn check_edit(name: &str, summary: Option<&str>, links: &[ProfileLink]) -> Result<Edit> {
    let name = name.trim();
    ensure!(!name.is_empty(), "a profile needs a name");
    ensure!(
        name.len() <= MAX_NAME_BYTES,
        "the name is longer than {MAX_NAME_BYTES} bytes"
    );
    let summary = summary.map(str::trim).filter(|s| !s.is_empty());
    ensure!(
        summary.is_none_or(|s| s.len() <= MAX_SUMMARY_BYTES),
        "the summary is longer than {MAX_SUMMARY_BYTES} bytes"
    );
    ensure!(
        links.len() <= MAX_PROFILE_LINKS,
        "a profile can have at most {MAX_PROFILE_LINKS} links"
    );
    let links: Vec<ProfileLink> = links
        .iter()
        .map(|l| ProfileLink {
            name: l.name.trim().to_string(),
            value: l.value.trim().to_string(),
        })
        .collect();
    for link in &links {
        link.check()?;
    }
    Ok(Edit {
        name: name.to_string(),
        summary: summary.map(String::from),
        links,
    })
}

impl Core {
    /// The owner's name, as their profile gives it now.
    pub(crate) fn own_name(&self) -> String {
        match self.profile().verify() {
            Ok(Payload::Person(person)) => person.name,
            _ => self.config.name.clone(),
        }
    }

    /// Answers `SetProfile`: makes the picture, if there's a new one, then
    /// signs and stores the new profile, serves it from now on, and writes
    /// it to `config.toml`, so a restart keeps it. A device's photo is read
    /// from `recv`; the local socket has none.
    pub(crate) async fn set_profile(
        &self,
        name: &str,
        summary: Option<&str>,
        links: &[ProfileLink],
        icon: IconChange,
        recv: Option<&mut RecvStream>,
    ) -> Result<ProfileInfo> {
        let edit = check_edit(name, summary, links)?;
        let _editing = self.profile_edit.lock().await;
        let Payload::Person(current) = self.profile().verify()? else {
            bail!("this node's own profile isn't a profile");
        };
        let icon = match icon {
            IconChange::Keep => current.icon.clone(),
            IconChange::Remove => None,
            IconChange::Upload { file } => {
                let recv = recv.context("a photo can only be sent from a paired device")?;
                Some(self.receive_icon(&file, recv).await?)
            }
            IconChange::File { path } => {
                ensure!(
                    recv.is_none(),
                    "a device sends its photo with the request, not a path"
                );
                Some(self.make_icon(&path).await?)
            }
        };
        let fields = ProfileFields {
            name: edit.name,
            summary: edit.summary,
            links: edit.links,
            icon,
            discoverable: current.discoverable,
        };
        let media = current.media_endpoints.first().cloned();
        let obj = own_profile(&self.signer, &fields, self.endpoint.id(), media, &self.db)?;
        *self.own_profile.write().unwrap_or_else(|e| e.into_inner()) = obj;
        if let Err(e) = self.save_profile_fields(&fields) {
            warn!("the new profile is published, but saving it to config.toml failed: {e:#}");
        }
        if let Some(old) = &current.icon
            && fields.icon.as_ref().is_none_or(|new| new.blob != old.blob)
        {
            self.db.remove_blob(&old.blob)?;
            self.store.let_go(&[old.blob]).await;
        }
        info!("published a new profile");
        self.profile_info(&self.signer.identity()).await
    }

    /// Writes the profile's fields to `config.toml`, as it is on the disk
    /// now, so other changes made there since the node started stay.
    fn save_profile_fields(&self, fields: &ProfileFields) -> Result<()> {
        let path = self.home.config();
        let mut config = match path.exists() {
            true => Config::load(&path)?,
            false => self.config.clone(),
        };
        config.name = fields.name.clone();
        config.summary = fields.summary.clone();
        config.links = fields.links.clone();
        config.icon = fields.icon.clone();
        config.save(&path)
    }

    /// Reads a device's photo from its request's stream into the work
    /// directory, then makes the picture from it.
    async fn receive_icon(&self, file: &Upload, recv: &mut RecvStream) -> Result<BlobRef> {
        ensure!(file.size > 0, "the photo is empty");
        ensure!(
            file.size <= MAX_ICON_UPLOAD,
            "the photo is too big for a profile picture; choose a smaller one"
        );
        let work = self.home.work();
        std::fs::create_dir_all(&work)?;
        let ext = Path::new(&file.name)
            .extension()
            .and_then(|e| e.to_str())
            .filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or("bin");
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = work.join(format!("icon-upload-{}-{n}.{ext}", std::process::id()));
        let received = tokio::time::timeout(ICON_UPLOAD_TIMEOUT, async {
            let mut out = tokio::fs::File::create(&path).await?;
            let mut taken = tokio::io::AsyncReadExt::take(recv, file.size);
            let n = tokio::io::copy(&mut taken, &mut out).await?;
            out.flush().await?;
            ensure!(n == file.size, "the photo stopped part way; send it again");
            anyhow::Ok(())
        })
        .await
        .context("the photo took too long to arrive; send it again")
        .and_then(|r| r);
        let made = match received {
            Ok(()) => self.make_icon(&path).await,
            Err(e) => Err(e),
        };
        let _ = std::fs::remove_file(&path);
        made
    }

    /// Makes a profile picture from a photo and keeps it as our own file.
    async fn make_icon(&self, photo: &Path) -> Result<BlobRef> {
        ensure!(
            self.media_tools,
            "the home node can't make pictures: ffmpeg isn't installed"
        );
        let made = {
            let _turn = self.transcode_lock.lock().await;
            media::profile_picture(&self.config.media, photo, &self.home.work()).await?
        };
        let imported = self.store.import(&made, BlobClass::Own).await;
        if let Some(job_dir) = made.parent() {
            let _ = std::fs::remove_dir_all(job_dir);
        }
        let (blob, size) = imported?;
        Ok(BlobRef {
            blob,
            size,
            media_type: "image/webp".to_string(),
        })
    }

    /// Answers `Profile`: the newest profile held of `actor`, and what this
    /// node holds of them, counted, never listed.
    pub(crate) async fn profile_info(&self, actor: &PublicKey) -> Result<ProfileInfo> {
        let me = self.signer.identity();
        let obj = match *actor == me {
            true => self.profile(),
            false => self
                .db
                .profile(actor)?
                .with_context(|| format!("this node holds no profile of {}", actor.fmt_short()))?,
        };
        let Payload::Person(person) = obj.verify()? else {
            bail!(
                "the stored profile of {} isn't a profile",
                actor.fmt_short()
            );
        };
        ensure!(
            person.actor == *actor,
            "the stored profile is someone else's"
        );
        let icon_local = match &person.icon {
            Some(icon) => self.store.has(&icon.blob).await?,
            None => false,
        };
        let counts = self.db.profile_counts(actor)?;
        let own = *actor == me;
        Ok(ProfileInfo {
            icon_local,
            followed: !own && self.db.is_following(actor)?,
            known: !own && self.db.is_trusted(actor)?,
            posts: counts.posts,
            media: counts.media,
            reshares: counts.reshares,
            followers: match own {
                true => Some(self.db.follower_count()? as u64),
                false => None,
            },
            following: match own {
                true => Some(self.db.follows()?.len() as u64),
                false => None,
            },
            person,
        })
    }

    /// Answers `ProfileItems`: one page, and the `before` for the next one
    /// when the page was full. A reshare of a post that isn't here is left
    /// out, but still moves the page on.
    pub(crate) async fn profile_items(
        &self,
        actor: &PublicKey,
        kind: ProfileItemKind,
        before: Option<&ObjectId>,
        limit: u32,
    ) -> Result<ClientResponse> {
        let limit = limit.clamp(1, MAX_PROFILE_PAGE);
        let page = self.db.profile_items(actor, kind, before, limit)?;
        let next = match page.len() == limit as usize {
            true => page.last().map(|obj| obj.id()),
            false => None,
        };
        let mut items = Vec::new();
        for obj in &page {
            if let Some(item) = self.feed_item(obj).await? {
                items.push(self.with_found(item)?);
            }
        }
        Ok(ClientResponse::ProfileItems { items, next })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(name: &str, value: &str) -> ProfileLink {
        ProfileLink {
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn edits_are_tidied_and_checked() {
        let edit = check_edit(
            "  Kim ",
            Some("   "),
            &[link(" Site ", " https://example.org/kim ")],
        )
        .unwrap();
        assert_eq!(edit.name, "Kim");
        assert_eq!(edit.summary, None);
        assert_eq!(edit.links, [link("Site", "https://example.org/kim")]);

        assert!(check_edit(" ", None, &[]).is_err());
        assert!(check_edit("Kim", None, &[link("Site", "http://example.org")]).is_err());
        assert!(check_edit("Kim", None, &[link("", "https://example.org")]).is_err());
        let five = vec![link("a", "https://example.org"); 5];
        assert!(check_edit("Kim", None, &five).is_err());
    }
}
