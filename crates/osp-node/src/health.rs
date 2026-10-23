//! What the node reports about its own health (ADR 0006): problems its owner
//! should know about, shown by `osp status` and in the phone app.

use std::path::Path;

use anyhow::Result;

use crate::{config::RelayChoice, db::BlobClass, keys::Signer, node::Core};

/// Warn when less than this is free on the node's disk.
pub(crate) const LOW_DISK_BYTES: u64 = 2 << 30;

/// Space available to the node's user on the filesystem holding `path`.
pub(crate) fn disk_free(path: &Path) -> Option<u64> {
    let stat = rustix::fs::statvfs(path).ok()?;
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

impl Core {
    /// Problems in plain words; empty when all is well.
    pub(crate) fn problems(
        &self,
        relay: Option<&str>,
        disk_free: Option<u64>,
    ) -> Result<Vec<String>> {
        let mut problems = Vec::new();
        if self.config.network.relay != RelayChoice::Disabled && relay.is_none() {
            problems.push(
                "not connected to a relay, so your phone can't reach this node from outside your home network"
                    .to_string(),
            );
        }
        // Some followed nodes being offline is normal; all of them suggests our network is down.
        let follows = self.db.follow_infos()?;
        if !follows.is_empty() && follows.iter().all(|f| f.last_error.is_some()) {
            problems.push(format!(
                "the last sync couldn't reach any of the {} people you follow; check this node's internet connection",
                follows.len()
            ));
        }
        if !self.media_tools {
            problems.push(format!(
                "{} isn't installed or doesn't run, so posts with photos or videos will fail",
                self.config.media.ffmpeg
            ));
        }
        if let Some(free) = disk_free
            && free < LOW_DISK_BYTES
        {
            problems.push(format!("only {} MB free on the node's disk", free >> 20));
        }
        if matches!(self.signer, Signer::Identity(_)) {
            problems.push(
                "this node still keeps your identity key, so anyone who copies its files can be you for good; \
                 stop the node and run `osp keys protect` to keep it only as your recovery words"
                    .to_string(),
            );
        }
        // Possible after lowering the quota. Hosted media isn't deleted to fit.
        let hosted = self.db.class_bytes(BlobClass::Replica)?;
        let quota = self.config.storage.replica_quota_mb << 20;
        if hosted > quota {
            problems.push(format!(
                "media you host for others takes {} MB, more than storage.replica_quota_mb ({} MB), so new hosting is refused",
                hosted >> 20,
                quota >> 20
            ));
        }
        Ok(problems)
    }
}
