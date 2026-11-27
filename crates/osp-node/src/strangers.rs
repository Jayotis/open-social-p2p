//! Limits on strangers at the media endpoint (ADR 0024 step 3).
//!
//! Alice watches Kim's found post from a viewing endpoint: a new key, which
//! Kim's node can't tell from anyone else's. Anyone can make keys at will, so
//! a limit per key would hold nobody back. These count all strangers together:
//! connections at once, and questions of who hosts a file an hour, besides one
//! a minute from any one key about any one file. Not one a minute from a key
//! at all: a viewing endpoint is kept for an author's posts for 10 minutes,
//! and asks about each file it fetches, a thumbnail then its video.
//! Followers and the node's own devices aren't held to them. Transfers have
//! their own share for strangers (`store.rs`).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use iroh::{
    EndpointId,
    endpoint::{Connection, VarInt},
    protocol::{AcceptError, ProtocolHandler},
};
use osp_proto::BlobHash;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::debug;

use crate::node::Core;

/// Strangers' connections to the media endpoint at once, all together.
pub(crate) const MAX_CONNECTIONS: usize = 32;
/// Strangers' questions of who hosts a file, an hour, all together.
pub(crate) const MAX_QUESTIONS_PER_HOUR: u32 = 600;
/// The least time between two questions from one key about one file.
pub(crate) const QUESTION_EVERY: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(60 * 60);
/// What a stranger's connection is closed with when there's no room for it.
pub(crate) const BUSY: VarInt = VarInt::from_u32(503);

pub(crate) struct Strangers {
    connections: Arc<Semaphore>,
    questions: Mutex<Questions>,
}

/// Questions let through this hour, and when each key last asked one about
/// each file.
struct Questions {
    since: Option<Instant>,
    asked: u32,
    /// Only questions let through, so at most an hour's worth.
    last: HashMap<(EndpointId, BlobHash), Instant>,
}

impl Default for Strangers {
    fn default() -> Self {
        Self {
            connections: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
            questions: Mutex::new(Questions {
                since: None,
                asked: 0,
                last: HashMap::new(),
            }),
        }
    }
}

impl Strangers {
    /// A place for one more stranger's connection, held while it lasts, if
    /// there's one.
    pub(crate) fn connection(&self) -> Option<OwnedSemaphorePermit> {
        self.connections.clone().try_acquire_owned().ok()
    }

    /// Whether a question from `key` about `blob` at `now` is let through,
    /// counting it if so: at most one each `every` from a key about a file,
    /// and `MAX_QUESTIONS_PER_HOUR` from all of them.
    pub(crate) fn question(
        &self,
        key: EndpointId,
        blob: BlobHash,
        now: Instant,
        every: Duration,
    ) -> bool {
        let mut q = self.questions.lock().unwrap_or_else(|e| e.into_inner());
        if q.since
            .is_none_or(|since| now.duration_since(since) >= HOUR)
        {
            q.since = Some(now);
            q.asked = 0;
        }
        if q.last.len() as u32 >= MAX_QUESTIONS_PER_HOUR {
            q.last.retain(|_, at| now.duration_since(*at) < every);
        }
        if q.last
            .get(&(key, blob))
            .is_some_and(|at| now.duration_since(*at) < every)
            || q.asked >= MAX_QUESTIONS_PER_HOUR
        {
            return false;
        }
        q.asked += 1;
        q.last.insert((key, blob), now);
        true
    }
}

impl Core {
    /// Whether `endpoint` is one of this node's followers or devices, not a
    /// stranger.
    pub(crate) fn knows_peer(&self, endpoint: EndpointId) -> bool {
        crate::store::is_follower_or_device(&self.db, Some(endpoint))
    }
}

/// A handler on the media endpoint that holds a stranger's connection to its
/// share, `MAX_CONNECTIONS`, for as long as it lasts.
#[derive(Clone)]
pub(crate) struct Limited<H> {
    core: Arc<Core>,
    inner: H,
}

impl<H> Limited<H> {
    pub(crate) fn new(core: Arc<Core>, inner: H) -> Self {
        Self { core, inner }
    }
}

impl<H> std::fmt::Debug for Limited<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Limited")
    }
}

impl<H: ProtocolHandler> ProtocolHandler for Limited<H> {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let _place = match self.core.knows_peer(conn.remote_id()) {
            true => None,
            false => match self.core.strangers.connection() {
                Some(place) => Some(place),
                None => {
                    debug!("strangers' connections are full: one more refused");
                    conn.close(BUSY, b"busy");
                    return Ok(());
                }
            },
        };
        self.inner.accept(conn).await
    }

    async fn shutdown(&self) {
        self.inner.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;

    fn key() -> EndpointId {
        SecretKey::generate().public()
    }

    fn blob(byte: u8) -> BlobHash {
        BlobHash::from_bytes([byte; 32])
    }

    /// `strangers.question`, at `secs` after `start`, a minute apart.
    fn ask(strangers: &Strangers, key: EndpointId, file: u8, start: Instant, secs: u64) -> bool {
        let at = start + Duration::from_secs(secs);
        strangers.question(key, blob(file), at, QUESTION_EVERY)
    }

    #[test]
    fn one_key_asks_about_a_file_once_a_minute() {
        let strangers = Strangers::default();
        let (alice, start) = (key(), Instant::now());
        assert!(ask(&strangers, alice, 1, start, 0));
        // Asking every second about it: held to one a minute.
        for secs in 1..60 {
            assert!(!ask(&strangers, alice, 1, start, secs), "{secs} s");
        }
        assert!(ask(&strangers, alice, 1, start, 60));
        // Its next file, the video after the thumbnail, at once.
        assert!(ask(&strangers, alice, 2, start, 60));
    }

    #[test]
    fn fresh_keys_share_an_hours_questions() {
        let strangers = Strangers::default();
        let start = Instant::now();
        for i in 0..MAX_QUESTIONS_PER_HOUR {
            let at = start + Duration::from_millis(u64::from(i));
            let asked = strangers.question(key(), blob(1), at, QUESTION_EVERY);
            assert!(asked, "question {i}");
        }
        assert!(!ask(&strangers, key(), 1, start, 59 * 60));
        assert!(ask(&strangers, key(), 1, start, 60 * 60));
        // The keys kept stay an hour's worth at most.
        let kept = strangers.questions.lock().unwrap().last.len() as u32;
        assert!(kept <= MAX_QUESTIONS_PER_HOUR + 1, "{kept}");
    }

    #[test]
    fn connections_are_shared_by_all_strangers() {
        let strangers = Strangers::default();
        let held: Vec<_> = (0..MAX_CONNECTIONS)
            .map(|_| strangers.connection().expect("a place"))
            .collect();
        assert!(strangers.connection().is_none());
        drop(held);
        assert!(strangers.connection().is_some());
    }
}
