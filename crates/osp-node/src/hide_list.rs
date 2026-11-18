//! The project's hide list (ADR 0042 §6): fetched once a day over Tor, so
//! the site sees a Tor exit and never this node, checked against the
//! project's key, and applied. The owner can turn it off.

use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use anyhow::{Context, Result};
use iroh::PublicKey;
use osp_proto::{
    lists::{MAX_LIST_BYTES, SignedList},
    rpc::{HideException, HideListInfo},
};
use tracing::{info, warn};

use crate::{
    db::{HideApplied, now_ms},
    node::Core,
};

/// The project's list key, built into official builds (in `osp-proto`, so
/// the apps share it); `[lists] hide_key` overrides it.
pub const PROJECT_LIST_KEY: Option<&str> = Some(osp_proto::lists::PROJECT_KEY);

/// How often the list is fetched.
const FETCH_EVERY: Duration = Duration::from_secs(24 * 3600);
/// After a fetch that failed.
const RETRY_AFTER: Duration = Duration::from_secs(3600);
/// Longest a fetch takes, Tor's circuit included.
const FETCH_TIMEOUT: Duration = Duration::from_secs(180);
/// Tor keeps the list's streams on circuits of their own.
const ISOLATION: &str = "osp-lists";

impl Core {
    /// The key the list must be signed by, if any.
    pub(crate) fn hide_key(&self) -> Option<PublicKey> {
        self.config
            .lists
            .hide_key
            .or_else(|| PROJECT_LIST_KEY.and_then(|k| k.parse().ok()))
    }

    /// Checks a fetched list against the key and applies it, letting go of
    /// the files it holds.
    pub(crate) async fn apply_hide_list(&self, bytes: &[u8]) -> Result<HideApplied> {
        let key = self
            .hide_key()
            .context("no key to check the hide list by")?;
        anyhow::ensure!(
            self.hide_list_on.load(Ordering::Relaxed),
            "the hide list is off"
        );
        let signed = SignedList::from_json(bytes)?;
        let list = signed.verify(&key)?;
        let text = std::str::from_utf8(bytes).context("the list isn't text")?;
        let applied = self
            .db
            .apply_hide_list(&list, text, &self.signer.identity(), now_ms())?;
        self.store.let_go(&applied.files).await;
        info!(
            seq = list.seq,
            entries = list.active().count(),
            hidden = applied.hidden,
            shown = applied.shown,
            files = applied.files.len(),
            "applied the hide list"
        );
        Ok(applied)
    }

    pub(crate) fn hide_list_info(&self) -> Result<HideListInfo> {
        let state = self.db.hide_state()?;
        Ok(HideListInfo {
            on: self.hide_list_on.load(Ordering::Relaxed),
            key: self.hide_key(),
            seq: state.as_ref().map(|s| s.seq),
            published: state.as_ref().map(|s| s.published.clone()),
            entries: state.as_ref().map(|s| s.entries),
            fetched: state.map(|s| osp_proto::rfc3339_from_ms(s.fetched_ms.max(0) as u64)),
            asked: self.hide_list_asked.load(Ordering::Relaxed),
            exceptions: self
                .db
                .hide_exceptions()?
                .into_iter()
                .map(|(actor, name)| HideException { actor, name })
                .collect(),
            signed: self.db.hide_list_signed()?,
        })
    }

    /// Excepts `actor` from the list, or takes that back; what it hides
    /// changes at once.
    pub(crate) async fn set_hide_exception(&self, actor: &PublicKey, except: bool) -> Result<()> {
        anyhow::ensure!(
            *actor != self.signer.identity(),
            "your own posts are never hidden"
        );
        if let Some(applied) = self.db.set_hide_exception(actor, except, now_ms())? {
            self.store.let_go(&applied.files).await;
            info!(
                except,
                hidden = applied.hidden,
                shown = applied.shown,
                files = applied.files.len(),
                "changed a hide list exception"
            );
        }
        Ok(())
    }

    /// Turns the list on or off, here and in `config.toml`. Off, all it hid
    /// comes back; on, it's fetched soon.
    pub(crate) fn set_hide_list(&self, on: bool) -> Result<()> {
        let path = self.home.config();
        let mut config = match path.exists() {
            true => crate::config::Config::load(&path)?,
            false => self.config.clone(),
        };
        config.lists.hide = Some(on);
        config.save(&path)?;
        self.hide_list_asked.store(true, Ordering::Relaxed);
        self.hide_list_on.store(on, Ordering::Relaxed);
        if on {
            self.fetch_hide_list_now.notify_one();
        } else {
            let applied = self.db.clear_hide_list()?;
            info!(shown = applied.shown, "turned the hide list off");
        }
        Ok(())
    }

    async fn fetch_hide_list(&self) -> Result<HideApplied> {
        let tor = self
            .tor
            .as_ref()
            .context("Tor is off, and the list is fetched only through it")?;
        let bytes = tokio::time::timeout(FETCH_TIMEOUT, async {
            tor.ready().await;
            let socks = tor.socks().context("Tor hasn't given its SOCKS port")?;
            osp_tor::https::https_get(
                socks,
                ISOLATION,
                &self.config.lists.hide_url,
                MAX_LIST_BYTES,
            )
            .await
        })
        .await
        .context("fetching the hide list took too long")??;
        self.apply_hide_list(&bytes).await
    }
}

/// Fetches the list a day after the last one came, and again each day,
/// while it's on.
pub(crate) async fn fetch_loop(core: Arc<Core>) {
    if core.hide_key().is_none() || core.tor.is_none() {
        return;
    }
    let mut wait = Duration::ZERO;
    loop {
        if !core.hide_list_on.load(Ordering::Relaxed) {
            core.fetch_hide_list_now.notified().await;
            wait = Duration::ZERO;
            continue;
        }
        // A restart doesn't fetch again sooner than a day after the last.
        if let Ok(Some(state)) = core.db.hide_state() {
            let since = Duration::from_millis((now_ms() - state.fetched_ms).max(0) as u64);
            wait = wait.max(FETCH_EVERY.saturating_sub(since));
        }
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = core.fetch_hide_list_now.notified() => {}
        }
        if !core.hide_list_on.load(Ordering::Relaxed) {
            continue;
        }
        wait = match core.fetch_hide_list().await {
            Ok(_) => FETCH_EVERY,
            Err(e) => {
                warn!("the hide list wasn't fetched: {e:#}");
                RETRY_AFTER
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key that didn't parse would turn the list off without a word.
    #[test]
    fn project_key_parses() {
        let key = PROJECT_LIST_KEY.expect("the project has a list key");
        key.parse::<PublicKey>()
            .expect("PROJECT_LIST_KEY is a public key");
    }
}
