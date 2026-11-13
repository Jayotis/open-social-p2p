//! Reading a post's replies without showing who reads (ADR 0028).
//!
//! Kim's media endpoint answers `osp/replies/0`: the published replies to one
//! of her posts, a page at a time, each with the profile its replier sent, or
//! her `Delete` if the post is gone. Alice's node asks from a viewing endpoint
//! (ADR 0024), through the relay, so Kim's node sees a key it doesn't know,
//! as for any found post's media. Nothing about the reader is asked for or
//! kept.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, bail, ensure};
use iroh::{
    Endpoint, EndpointAddr, EndpointId,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use osp_proto::{
    Note, ObjectId, Payload, Person,
    framing::{read_frame_max, write_frame},
    rpc::{ClientResponse, ReplyInfo, ReplyStatus},
    social::{
        MAX_REQUEST_LEN, MAX_THREAD_PAGE, MAX_THREAD_PAGE_BYTES, REPLIES_ALPN, RepliesRequest,
        RepliesResponse, ThreadReply,
    },
};
use tokio::sync::Semaphore;
use tracing::debug;

use crate::{node::Core, trust::plain};

/// Most reads served at once, from everyone together. Viewing endpoints are
/// new keys, so a limit per key would hold nobody back.
const MAX_READS: usize = 16;
/// Longest one read may take, the request and the answer together.
const STREAM_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a reader waits for one of the creator's media endpoints.
const READ_WAIT: Duration = Duration::from_secs(20);
/// Largest page a reader takes. A page holds at least one reply, even one
/// past `MAX_THREAD_PAGE_BYTES` with its profile.
const MAX_PAGE_FRAME: u32 = 2 * MAX_THREAD_PAGE_BYTES as u32;

/// What the reader is told when the thread can't be read privately.
const NO_PRIVATE_THREAD: &str =
    "its replies couldn't be read without showing who you are; try again later";

/// Answers `REPLIES_ALPN` on the media endpoint.
#[derive(Clone)]
pub(crate) struct RepliesProtocol {
    core: Arc<Core>,
    reads: Arc<Semaphore>,
}

impl RepliesProtocol {
    pub fn new(core: Arc<Core>) -> Self {
        Self {
            core,
            reads: Arc::new(Semaphore::new(MAX_READS)),
        }
    }
}

impl std::fmt::Debug for RepliesProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RepliesProtocol")
    }
}

impl ProtocolHandler for RepliesProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        // One read per connection.
        let Ok((mut send, mut recv)) = conn.accept_bi().await else {
            return Ok(());
        };
        let resp = match self.reads.clone().try_acquire_owned() {
            Ok(_permit) => {
                let answer = async {
                    let req: RepliesRequest = read_frame_max(&mut recv, MAX_REQUEST_LEN).await?;
                    self.core.answer_replies(&req)
                };
                match tokio::time::timeout(STREAM_TIMEOUT, answer).await {
                    Ok(Ok(resp)) => resp,
                    Ok(Err(e)) => RepliesResponse::Error {
                        message: format!("{e:#}"),
                    },
                    Err(_) => return Ok(()),
                }
            }
            Err(_) => RepliesResponse::Error {
                message: crate::social::BUSY.to_string(),
            },
        };
        let _ = tokio::time::timeout(STREAM_TIMEOUT, async {
            write_frame(&mut send, &resp).await?;
            send.finish()?;
            // Wait for the reader to read it and close.
            conn.closed().await;
            anyhow::Ok(())
        })
        .await;
        Ok(())
    }
}

impl Core {
    /// A page of the published replies to `req.post`, one of this node's
    /// posts, after `req.after`; or its `Delete`.
    fn answer_replies(&self, req: &RepliesRequest) -> Result<RepliesResponse> {
        let me = self.signer.identity();
        if let Some(delete) = self.db.delete_of(&req.post, &me)? {
            return Ok(RepliesResponse::Deleted { delete });
        }
        match self.db.outbox_object(&req.post)?.map(|o| o.verify()) {
            Some(Ok(Payload::Note(note))) if note.actor == me && !note.is_reply() => {}
            _ => bail!("that isn't one of this node's posts"),
        }
        let after = i64::try_from(req.after).unwrap_or(i64::MAX);
        let rows = self.db.thread(&req.post, after, MAX_THREAD_PAGE + 1)?;
        let mut replies = Vec::new();
        let mut bytes = 0;
        let mut more = false;
        for row in rows {
            if replies.len() == MAX_THREAD_PAGE {
                more = true;
                break;
            }
            let reply = ThreadReply {
                seq: row.seq.context("a published reply has no place")?.max(0) as u64,
                reply: row.reply.signed,
                profile: row.reply.profile,
            };
            let size = serde_json::to_vec(&reply)?.len();
            if !replies.is_empty() && bytes + size > MAX_THREAD_PAGE_BYTES {
                more = true;
                break;
            }
            bytes += size;
            replies.push(reply);
        }
        Ok(RepliesResponse::Replies {
            policy: self.reply_policy(&req.post)?,
            replies,
            more,
        })
    }

    /// The replies to `post`, a page after `after`. One of the owner's own
    /// posts is read here; anyone else's from its creator's media endpoint,
    /// through a viewing endpoint, never this node's own.
    pub(crate) async fn read_thread(&self, post: &ObjectId, after: u64) -> Result<ClientResponse> {
        let (root, note) = self.resolve_note(post)?;
        ensure!(!note.is_reply(), "that's a reply; read the post it answers");
        if note.actor == self.signer.identity() {
            return self.own_thread(&root);
        }
        let authors = self.db.media_endpoints_of(&note.actor)?;
        ensure!(
            !authors.is_empty(),
            "{NO_PRIVATE_THREAD} (no profile held gives a media endpoint for its creator)"
        );
        let lease = self
            .viewing
            .lease(note.actor, || self.viewing_endpoint())
            .await
            .context(NO_PRIVATE_THREAD)?;
        let mut failed = anyhow!("its creator gives no media endpoint");
        for author in &authors {
            let ask = ask_replies(&lease.inner.endpoint, author.endpoint, root, after);
            match tokio::time::timeout(READ_WAIT, ask).await {
                Ok(Ok(resp)) => {
                    drop(lease);
                    return self.took_thread(resp, (&root, &note), after).await;
                }
                Ok(Err(e)) => failed = e,
                Err(_) => failed = anyhow!("its creator's media endpoint didn't answer in time"),
            }
        }
        Err(failed.context(NO_PRIVATE_THREAD))
    }

    /// What a creator's media endpoint answered, checked.
    async fn took_thread(
        &self,
        resp: RepliesResponse,
        (root, note): (&ObjectId, &Note),
        after: u64,
    ) -> Result<ClientResponse> {
        match resp {
            RepliesResponse::Replies {
                policy,
                replies,
                more,
            } => {
                ensure!(
                    replies.len() <= MAX_THREAD_PAGE,
                    "its creator's node sent more replies than a page holds"
                );
                let me = self.signer.identity();
                let mut last = after;
                let mut infos = Vec::new();
                for reply in &replies {
                    ensure!(
                        reply.seq > last,
                        "its creator's node sent replies out of order"
                    );
                    last = reply.seq;
                    match checked_reply(reply, root) {
                        Ok(info) => {
                            if info.actor == me {
                                self.db.sent_reply_seen_published(&info.id)?;
                            }
                            infos.push(info);
                        }
                        Err(e) => debug!("left out a reply that doesn't check out: {e:#}"),
                    }
                }
                Ok(ClientResponse::Thread {
                    policy,
                    replies: infos,
                    more,
                })
            }
            RepliesResponse::Deleted { delete } => {
                self.found_deleted(&delete, root, note).await?;
                bail!("its creator deleted it")
            }
            RepliesResponse::Error { message } => {
                bail!("its creator's node replied: {message}")
            }
        }
    }
}

/// Asks the media endpoint `author` for the replies to `post` after `after`.
async fn ask_replies(
    viewing: &Endpoint,
    author: EndpointId,
    post: ObjectId,
    after: u64,
) -> Result<RepliesResponse> {
    let conn = viewing
        .connect(EndpointAddr::new(author), REPLIES_ALPN)
        .await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, &RepliesRequest { post, after }).await?;
    send.finish()?;
    let resp = read_frame_max(&mut recv, MAX_PAGE_FRAME).await?;
    conn.close(0u32.into(), b"done");
    Ok(resp)
}

/// One reply in a page, if it's a reply to `root` by whoever its profile is.
fn checked_reply(reply: &ThreadReply, root: &ObjectId) -> Result<ReplyInfo> {
    let Payload::Note(note) = reply.reply.verify()? else {
        bail!("not a note");
    };
    ensure!(note.in_reply_to == Some(*root), "it answers another post");
    note.check_reply()?;
    ensure!(
        reply.profile.signer() == note.actor,
        "the profile isn't the replier's"
    );
    let Payload::Person(Person { name, .. }) = reply.profile.verify()? else {
        bail!("that isn't a profile");
    };
    Ok(ReplyInfo {
        id: reply.reply.id(),
        post: *root,
        actor: note.actor,
        name: plain(&name),
        content: note.content,
        published: note.published,
        state: ReplyStatus::Published,
        why: None,
        seq: reply.seq,
    })
}
