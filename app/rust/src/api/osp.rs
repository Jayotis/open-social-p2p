//! Everything the Flutter app calls. Each function wraps `osp-client` and
//! turns its wire types into plain structs that flutter_rust_bridge can
//! hand to Dart. Byte counts are `i64` because Dart has no unsigned 64-bit
//! integers.

use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{bail, Result};
use flutter_rust_bridge::frb;
use osp_client::{
    device_key_from_hex,
    osp_proto::{
        rpc::{
            ClientRequest, ClientResponse, FeedItem, Reached, ReplierChoice, ReplyAnswer,
            ReplyInfo, ReplyStatus, SentReplyInfo, SentStatus, TrustInfo,
        },
        social::ReplyPolicy,
        BlobHash, MediaKind, ObjectId,
    },
    Client, ClientOptions, NoContact, Pairing,
};
use tokio::sync::oneshot;

use crate::{frb_generated::StreamSink, logging};

#[frb(init)]
pub fn init_app() {
    // Not flutter_rust_bridge's default setup: that sends every Rust log
    // line to logcat at trace level and sets RUST_BACKTRACE, so every error
    // would capture a backtrace. Warnings are enough for logcat; the app's
    // own log goes to a file (see `logging.rs`).
    flutter_rust_bridge::setup_log_to_console(log::LevelFilter::Warn);
}

/// The app's connection to its home node.
#[frb(opaque)]
pub struct OspApp {
    client: Arc<Client>,
    dir: PathBuf,
    /// Stops the post being sent, if any (`discard_post`).
    stop_post: Mutex<Option<oneshot::Sender<()>>>,
}

/// The home node this phone is paired with.
pub struct NodeInfo {
    /// The node owner's display name.
    pub name: String,
    pub identity: String,
    /// When pairing happened (RFC 3339).
    pub paired: String,
}

/// The node's health, and how this phone reaches it.
pub struct NodeStatus {
    pub name: String,
    pub version: String,
    pub uptime_secs: i64,
    pub follows: u32,
    pub followers: u32,
    pub devices: u32,
    pub disk_free_bytes: Option<i64>,
    pub relay_connected: bool,
    /// Problems the node reports, in plain words. Empty when all is well.
    pub problems: Vec<String>,
    /// `direct`, `relay` or `none`.
    pub path: String,
    pub rtt_ms: Option<u32>,
    /// Requests to be a trusted follower waiting for an answer.
    pub trust_requests: u32,
    /// Replies to the owner's posts waiting for an answer (ADR 0028).
    pub reply_requests: u32,
    /// Messages between nodes go through Tor, so they take seconds, and a
    /// first contact up to a few minutes.
    pub over_tor: bool,
}

/// A request to be a trusted follower, or someone trusted (ADR 0013).
pub struct TrustEntry {
    /// The node asking, or trusted: what `answer_trust` takes.
    pub node: String,
    pub identity: String,
    /// What the person says: their name, and the note with their request
    /// (empty if trusted without one). Neither is checked: show them as
    /// their words, as plain text.
    pub name: String,
    pub note: String,
    /// Seconds since they asked, while waiting; since they were trusted.
    pub secs_ago: i64,
    /// Whether the owner follows this identity, which is the check that
    /// it's who they say: the owner added their follow code.
    pub followed: bool,
    /// Whether this person is trusted on another of their nodes.
    pub trusted_elsewhere: bool,
    /// How long the node has followed the owner's, in seconds; `None` if
    /// it no longer does.
    pub follower_secs: Option<i64>,
    /// `direct` or `relay` if the node reached the owner's in the last few
    /// minutes, `none` otherwise.
    pub reached: String,
    /// How many of the owner's files it holds a copy of.
    pub copies: u32,
}

/// Requests waiting, newest first, and the nodes trusted.
pub struct TrustList {
    pub requests: Vec<TrustEntry>,
    pub trusted: Vec<TrustEntry>,
}

/// Someone the node follows.
pub struct Followed {
    pub identity: String,
    pub name: String,
    /// The node hasn't reached theirs yet.
    pub connecting: bool,
}

/// What a search found on the node, the best match first in each list.
pub struct Found {
    pub people: Vec<FoundPerson>,
    pub posts: Vec<Post>,
}

pub struct FoundPerson {
    pub identity: String,
    pub name: String,
    pub summary: Option<String>,
    /// Whether the node follows them.
    pub followed: bool,
    /// The directory they came from, in words, if they're on the node only
    /// because of one.
    pub found_through: Option<String>,
}

pub struct Post {
    pub id: String,
    pub author: String,
    /// The author's identity: the owner's own for their posts.
    pub author_identity: String,
    /// RFC 3339.
    pub published: String,
    pub text: String,
    pub media: Vec<Media>,
    /// Who reshared it, if this is a reshare.
    pub reshared_by: Option<String>,
    /// Whether the resharer keeps and serves a copy of its media.
    pub resharer_hosts: bool,
    /// The directory it came from, in words, if it's on the node only because
    /// of one.
    pub found_through: Option<String>,
    /// What the owner may delete here (ADR 0019): their reshare, if this is
    /// one, or else their own post. `None` for anyone else's.
    pub deletable: Option<String>,
}

/// A reply to a post (ADR 0028).
pub struct Reply {
    pub id: String,
    /// The post it answers.
    pub post: String,
    /// The replier's identity.
    pub identity: String,
    /// The name in the profile that came with it. Not checked.
    pub name: String,
    pub text: String,
    /// The date the replier gave it (RFC 3339).
    pub published: String,
    /// `published`, `hidden` or `request`.
    pub state: String,
    /// For a request, why it's there: "you don't follow them", say.
    pub why: Option<String>,
    /// Its place in the thread, to read on after it.
    pub seq: i64,
}

/// A post's replies, a page at a time.
pub struct Thread {
    /// `off`, `circle` or `network`. Only the owner's own posts say.
    pub policy: String,
    pub replies: Vec<Reply>,
    /// Another page follows: ask after the last reply's `seq`.
    pub more: bool,
}

/// One of the owner's replies to someone else's post, and where it stands.
pub struct SentReply {
    pub id: String,
    /// The post it answers.
    pub post: String,
    /// Whose post that is.
    pub creator: String,
    /// Empty once it's being deleted.
    pub text: String,
    pub published: String,
    /// `sending`, `published`, `awaiting_approval`, `not_published`,
    /// `closed`, `deleted`, `refused`, `gave_up` or `deleting`.
    pub status: String,
    /// Why the creator's node refused it, or why it's still being tried.
    pub error: Option<String>,
    /// While it's being sent: seconds until the next try.
    pub next_try_secs: Option<i64>,
}

pub struct Media {
    pub is_video: bool,
    pub blob: String,
    pub media_type: String,
    pub size: i64,
    pub width: u32,
    pub height: u32,
    pub duration_ms: Option<u32>,
    pub thumbnail_blob: Option<String>,
    /// Whether the home node already has it, so playback can start at once.
    pub on_node: bool,
}

/// The feed as last fetched, for when the node can't be reached.
pub struct SavedFeed {
    pub posts: Vec<Post>,
    /// RFC 3339.
    pub saved: String,
}

#[derive(Clone, Copy)]
pub enum PostStage {
    /// Sending files to the node.
    Sending,
    /// Lost contact with the node; trying again.
    Waiting,
    /// The node is transcoding and publishing.
    Processing,
    Done,
    Failed,
}

#[derive(Clone)]
pub struct PostProgress {
    pub stage: PostStage,
    pub sent: i64,
    pub total: i64,
    /// The new post's ID when done, or what went wrong.
    pub message: Option<String>,
    /// When failed: the post is kept on the phone, to try again
    /// (`continue_post`) or discard.
    pub kept: bool,
    /// When failed: it gave up after a long time without contact with the
    /// node.
    pub no_contact: bool,
}

/// A post that didn't finish sending, kept on the phone until the node
/// publishes it or the owner discards it.
pub struct UnsentPost {
    pub text: String,
    pub files: Vec<UnsentFile>,
    /// When it was started (RFC 3339).
    pub started: String,
}

pub struct UnsentFile {
    pub path: String,
    pub size: i64,
}

impl OspApp {
    /// Starts the client. `dir` is the app's private data directory, and
    /// `device_key` the key the app keeps in the phone's secure storage.
    pub async fn open(dir: String, device_key: String) -> Result<OspApp> {
        let dir = PathBuf::from(dir);
        logging::init(&dir);
        tracing::info!("app starting");
        let key = device_key_from_hex(&device_key)?;
        let client = Client::open_with_key(&dir, key, ClientOptions::default()).await?;
        Ok(OspApp {
            client,
            dir,
            stop_post: Mutex::new(None),
        })
    }

    #[frb(sync)]
    pub fn device_id(&self) -> String {
        self.client.device_id().to_string()
    }

    #[frb(sync)]
    pub fn paired_node(&self) -> Option<NodeInfo> {
        self.client.pairing().map(node_info)
    }

    pub async fn pair(&self, ticket: String, device_name: String) -> Result<NodeInfo> {
        Ok(node_info(self.client.pair(&ticket, &device_name).await?))
    }

    /// Forgets the node on this phone. Unpair it on the node too
    /// (`osp unpair`) to revoke its access.
    pub async fn forget(&self) -> Result<()> {
        self.client.forget().await
    }

    pub async fn status(&self) -> Result<NodeStatus> {
        let ClientResponse::Status(s) = self.client.request(&ClientRequest::Status).await? else {
            bail!("unexpected reply to Status");
        };
        let conn = self.client.connection_info().await;
        Ok(NodeStatus {
            name: s.name,
            version: s.version,
            uptime_secs: s.uptime_secs as i64,
            follows: s.follows as u32,
            followers: s.followers as u32,
            devices: s.devices as u32,
            disk_free_bytes: s.disk_free_bytes.map(|b| b as i64),
            relay_connected: s.relay.is_some(),
            problems: s.problems,
            path: conn
                .as_ref()
                .map(|c| c.path.clone())
                .unwrap_or_else(|| "none".to_string()),
            rtt_ms: conn.and_then(|c| c.rtt_ms).map(|r| r as u32),
            trust_requests: s.trust_requests as u32,
            reply_requests: s.reply_requests as u32,
            over_tor: s.tor.is_some(),
        })
    }

    pub async fn feed(&self, limit: u32) -> Result<Vec<Post>> {
        Ok(self
            .client
            .feed(limit)
            .await?
            .into_iter()
            .map(post)
            .collect())
    }

    /// Looks through the posts and profiles the node has received. Nothing
    /// leaves the node.
    pub async fn search(&self, words: String, limit: u32) -> Result<Found> {
        let (people, posts) = self.client.search(&words, limit).await?;
        Ok(Found {
            people: people
                .into_iter()
                .map(|p| FoundPerson {
                    identity: p.actor.to_string(),
                    name: p.name,
                    summary: p.summary,
                    followed: p.followed,
                    found_through: p.found_through,
                })
                .collect(),
            posts: posts.into_iter().map(post).collect(),
        })
    }

    #[frb(sync)]
    pub fn saved_feed(&self) -> Option<SavedFeed> {
        self.client.cached_feed().map(|f| SavedFeed {
            posts: f.items.into_iter().map(post).collect(),
            saved: f.saved,
        })
    }

    /// A URL on this phone that serves a blob from the home node.
    pub async fn media_url(&self, blob: String, media_type: String) -> Result<String> {
        let blob: BlobHash = blob.parse()?;
        self.client.media_url(&blob, &media_type).await
    }

    /// Waits until the node has the start of a blob, asking it to fetch
    /// the blob if it hasn't, so a player can start on it. The rest
    /// reaches the player as it arrives.
    pub async fn wait_for_media(&self, blob: String) -> Result<()> {
        let blob: BlobHash = blob.parse()?;
        self.client.wait_for_media(&blob).await
    }

    /// Posts text with photos or videos from this phone, reporting progress.
    /// It carries on after short breaks in the connection; if it fails, the
    /// post is usually kept (`pending_post`) to try again.
    pub async fn post(
        &self,
        text: String,
        files: Vec<String>,
        sink: StreamSink<PostProgress>,
    ) -> Result<()> {
        let files: Vec<PathBuf> = files.into_iter().map(PathBuf::from).collect();
        let sending = self.client.post_with(text, &files, report_to(sink.clone()));
        self.run_post(sending, sink).await;
        Ok(())
    }

    /// The post that didn't finish sending, if any: one that failed, or was
    /// stopped when the app was.
    #[frb(sync)]
    pub fn pending_post(&self) -> Option<UnsentPost> {
        self.client.pending_post().map(|p| UnsentPost {
            text: p.content,
            files: p
                .files
                .into_iter()
                .map(|f| UnsentFile {
                    path: f.path.to_string_lossy().into_owned(),
                    size: f.size as i64,
                })
                .collect(),
            started: p.started,
        })
    }

    /// Carries on sending the pending post where it stopped.
    pub async fn continue_post(&self, sink: StreamSink<PostProgress>) -> Result<()> {
        let sending = self.client.continue_post(report_to(sink.clone()));
        self.run_post(sending, sink).await;
        Ok(())
    }

    /// Stops the post being sent, if any, and discards the pending post on
    /// the phone and the node. Fails with the node's reason if it's too
    /// late: the node is preparing the post, or published it.
    pub async fn discard_post(&self) -> Result<()> {
        let stop = self
            .stop_post
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(stop) = stop {
            let _ = stop.send(());
        }
        self.client.discard_post().await
    }

    /// Deletes one of the owner's posts or reshares, by `Post::deletable`
    /// (ADR 0019). It can't be undone: the app waits before calling this.
    pub async fn delete_post(&self, id: String) -> Result<()> {
        let id: ObjectId = id.parse()?;
        self.client.request(&ClientRequest::Delete { id }).await?;
        Ok(())
    }

    /// Reshares a post; with `host`, the node also keeps and serves its media.
    pub async fn reshare(&self, post_id: String, host: bool) -> Result<()> {
        let id: ObjectId = post_id.parse()?;
        self.client
            .request(&ClientRequest::Reshare { id, host })
            .await?;
        Ok(())
    }

    /// Follows someone by their follow ticket (`OSP1...`).
    pub async fn follow(&self, ticket: String) -> Result<()> {
        self.client
            .request(&ClientRequest::Follow { ticket })
            .await?;
        Ok(())
    }

    /// Follows someone a search found, by their identity. The node asks their
    /// own node before anything of theirs reaches the feed.
    pub async fn follow_person(&self, identity: String) -> Result<()> {
        let actor = identity.parse()?;
        self.client
            .request(&ClientRequest::FollowPerson { actor })
            .await?;
        Ok(())
    }

    /// Stops following someone, whether or not the node has reached theirs.
    pub async fn unfollow(&self, identity: String) -> Result<()> {
        let actor = identity.parse()?;
        self.client
            .request(&ClientRequest::Unfollow { actor })
            .await?;
        Ok(())
    }

    /// Who the node follows, by name.
    pub async fn following(&self) -> Result<Vec<Followed>> {
        let ClientResponse::Following { follows } =
            self.client.request(&ClientRequest::Following).await?
        else {
            bail!("unexpected reply to Following");
        };
        let mut followed: Vec<Followed> = follows
            .into_iter()
            .map(|f| Followed {
                identity: f.actor.to_string(),
                name: f.name.unwrap_or_else(|| f.actor.fmt_short().to_string()),
                connecting: f.last_sync.is_none(),
            })
            .collect();
        followed.sort_by_key(|f| f.name.to_lowercase());
        Ok(followed)
    }

    /// Replies to `post_id` as the owner (ADR 0028). The node keeps it and
    /// sends it to the post's creator, trying again for 30 days. A reshare's
    /// ID answers the post it shares.
    pub async fn reply(&self, post_id: String, text: String) -> Result<SentReply> {
        let post: ObjectId = post_id.parse()?;
        let ClientResponse::ReplySent { reply } = self
            .client
            .request(&ClientRequest::Reply {
                post,
                content: text,
            })
            .await?
        else {
            bail!("unexpected reply to Reply");
        };
        Ok(sent_reply(reply))
    }

    /// One page of a post's replies, after `after` (0 for the first). For the
    /// owner's own posts, all of them, hidden ones too, and the policy.
    /// Anyone else's are read from their node without saying who reads.
    pub async fn thread(&self, post_id: String, after: i64) -> Result<Thread> {
        let post: ObjectId = post_id.parse()?;
        let ClientResponse::Thread {
            policy,
            replies,
            more,
        } = self
            .client
            .request(&ClientRequest::Thread {
                post,
                after: after.max(0) as u64,
            })
            .await?
        else {
            bail!("unexpected reply to Thread");
        };
        Ok(Thread {
            policy: policy.as_str().to_string(),
            replies: replies.into_iter().map(reply_info).collect(),
            more,
        })
    }

    /// The owner's replies to others' posts, newest first.
    pub async fn sent_replies(&self) -> Result<Vec<SentReply>> {
        let ClientResponse::SentReplies { replies } =
            self.client.request(&ClientRequest::SentReplies).await?
        else {
            bail!("unexpected reply to SentReplies");
        };
        Ok(replies.into_iter().map(sent_reply).collect())
    }

    /// Replies to the owner's posts waiting for an answer, newest first.
    pub async fn reply_requests(&self) -> Result<Vec<Reply>> {
        let ClientResponse::ReplyRequests { requests } =
            self.client.request(&ClientRequest::ReplyRequests).await?
        else {
            bail!("unexpected reply to ReplyRequests");
        };
        Ok(requests.into_iter().map(reply_info).collect())
    }

    /// Answers a waiting reply: `publish`, `publish_and_allow`, `decline` or
    /// `block`. The replier isn't told which.
    pub async fn answer_reply(&self, reply: String, answer: String) -> Result<()> {
        let reply: ObjectId = reply.parse()?;
        let answer = match answer.as_str() {
            "publish" => ReplyAnswer::Publish,
            "publish_and_allow" => ReplyAnswer::PublishAndAllow,
            "decline" => ReplyAnswer::Decline,
            "block" => ReplyAnswer::Block,
            _ => bail!("no such answer: {answer:?}"),
        };
        self.client
            .request(&ClientRequest::AnswerReply { reply, answer })
            .await?;
        Ok(())
    }

    /// Hides a published reply to one of the owner's posts, or shows it again.
    /// Nothing is deleted.
    pub async fn hide_reply(&self, reply: String, hidden: bool) -> Result<()> {
        let reply: ObjectId = reply.parse()?;
        self.client
            .request(&ClientRequest::HideReply { reply, hidden })
            .await?;
        Ok(())
    }

    /// Sets who may reply to one of the owner's posts: `off`, `circle` or
    /// `network`.
    pub async fn set_reply_policy(&self, post_id: String, policy: String) -> Result<()> {
        let post: ObjectId = post_id.parse()?;
        let policy: ReplyPolicy = policy.parse()?;
        self.client
            .request(&ClientRequest::SetReplyPolicy { post, policy })
            .await?;
        Ok(())
    }

    /// Lets someone reply to the owner's posts from now on (`allow`), blocks
    /// them (`block`), or with `None` forgets either.
    pub async fn set_replier(&self, identity: String, choice: Option<String>) -> Result<()> {
        let actor = identity.parse()?;
        let choice = match choice.as_deref() {
            None => None,
            Some("allow") => Some(ReplierChoice::Allow),
            Some("block") => Some(ReplierChoice::Block),
            Some(other) => bail!("no such choice: {other:?}"),
        };
        self.client
            .request(&ClientRequest::SetReplier { actor, choice })
            .await?;
        Ok(())
    }

    /// Requests to be a trusted follower waiting, newest first, and who is
    /// trusted.
    pub async fn trust_list(&self) -> Result<TrustList> {
        let ClientResponse::Trust { requests, trusted } =
            self.client.request(&ClientRequest::TrustRequests).await?
        else {
            bail!("unexpected reply to TrustRequests");
        };
        Ok(TrustList {
            requests: requests.into_iter().map(trust_entry).collect(),
            trusted: trusted.into_iter().map(trust_entry).collect(),
        })
    }

    /// Allows or declines the request from `node`. The node isn't told.
    pub async fn answer_trust(&self, node: String, allow: bool) -> Result<()> {
        let node = node.parse()?;
        self.client
            .request(&ClientRequest::AnswerTrust { node, allow })
            .await?;
        Ok(())
    }

    /// Trusts someone the node follows, without a request.
    pub async fn trust(&self, identity: String) -> Result<()> {
        let actor = identity.parse()?;
        self.client
            .request(&ClientRequest::TrustFollowed { actor })
            .await?;
        Ok(())
    }

    /// Stops trusting someone, on every node they were trusted on.
    pub async fn untrust(&self, identity: String) -> Result<()> {
        let actor = identity.parse()?;
        self.client
            .request(&ClientRequest::Untrust { actor })
            .await?;
        Ok(())
    }

    /// Asks someone the node follows to trust the owner, with a note saying
    /// who they are. Their node never says whether it did.
    pub async fn ask_trust(&self, identity: String, note: String) -> Result<()> {
        let actor = identity.parse()?;
        self.client
            .request(&ClientRequest::AskTrust { actor, note })
            .await?;
        Ok(())
    }

    /// The node owner's follow ticket, to show as a QR code.
    pub async fn invite(&self) -> Result<String> {
        match self.client.request(&ClientRequest::Invite).await? {
            ClientResponse::Ticket { ticket } => Ok(ticket),
            _ => bail!("unexpected reply to Invite"),
        }
    }

    /// Asks the node to sync everyone it follows now. Returns how many new
    /// objects arrived.
    pub async fn sync(&self) -> Result<u32> {
        match self.client.request(&ClientRequest::Sync).await? {
            ClientResponse::Synced { new_objects } => Ok(new_objects as u32),
            _ => bail!("unexpected reply to Sync"),
        }
    }

    /// Call when the phone's network changes (Wi-Fi to mobile data, say).
    pub async fn network_changed(&self) {
        self.client.network_change().await;
    }

    /// Call when the app comes back to the foreground. The network may have
    /// changed meanwhile, and iOS may have taken back the app's sockets.
    pub async fn resumed(&self) {
        self.client.resumed();
        self.client.network_change().await;
    }

    /// Closes the connection while the app is in the background.
    pub async fn disconnect(&self) {
        self.client.disconnect().await;
    }

    /// The end of the app's log, for the status screen.
    #[frb(sync)]
    pub fn recent_log(&self, lines: u32) -> String {
        logging::tail(&self.dir, lines as usize)
    }

    /// The app's log file, to share.
    #[frb(sync)]
    pub fn log_file(&self) -> String {
        logging::path(&self.dir).to_string_lossy().into_owned()
    }

    /// Runs a post until it ends, and reports how, unless `discard_post`
    /// stops it first.
    async fn run_post(
        &self,
        sending: impl Future<Output = Result<ObjectId>>,
        sink: StreamSink<PostProgress>,
    ) {
        let (stop, stopped) = oneshot::channel();
        *self.stop_post.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop);
        let result = tokio::select! {
            result = sending => result,
            // Discarded; `discard_post` says how that went.
            Ok(()) = stopped => return,
        };
        let last = match result {
            Ok(id) => PostProgress {
                message: Some(id.to_string()),
                ..progress(PostStage::Done, 0, 0)
            },
            Err(e) => {
                tracing::warn!("post failed: {e:#}");
                PostProgress {
                    message: Some(format!("{e:#}")),
                    kept: self.client.pending_post().is_some(),
                    no_contact: e.downcast_ref::<NoContact>().is_some(),
                    ..progress(PostStage::Failed, 0, 0)
                }
            }
        };
        let _ = sink.add(last);
    }
}

/// Passes a post's progress on to Dart.
fn report_to(sink: StreamSink<PostProgress>) -> impl Fn(osp_client::PostProgress) {
    move |p| {
        let report = match p {
            osp_client::PostProgress::Sending { sent, total } if sent < total => {
                progress(PostStage::Sending, sent, total)
            }
            osp_client::PostProgress::Sending { sent, total } => {
                progress(PostStage::Processing, sent, total)
            }
            osp_client::PostProgress::Waiting => progress(PostStage::Waiting, 0, 0),
        };
        let _ = sink.add(report);
    }
}

fn progress(stage: PostStage, sent: u64, total: u64) -> PostProgress {
    PostProgress {
        stage,
        sent: sent as i64,
        total: total as i64,
        message: None,
        kept: false,
        no_contact: false,
    }
}

fn trust_entry(t: TrustInfo) -> TrustEntry {
    TrustEntry {
        node: t.node.to_string(),
        identity: t.actor.to_string(),
        name: t.name,
        note: t.note,
        secs_ago: t.secs_ago as i64,
        followed: t.followed,
        trusted_elsewhere: t.trusted_elsewhere,
        follower_secs: t.follower_secs.map(|s| s as i64),
        reached: match t.reached {
            Some(Reached::Direct) => "direct",
            Some(Reached::Relay) => "relay",
            Some(Reached::Tor) => "tor",
            None => "none",
        }
        .to_string(),
        copies: t.copies as u32,
    }
}

fn reply_info(r: ReplyInfo) -> Reply {
    Reply {
        id: r.id.to_string(),
        post: r.post.to_string(),
        identity: r.actor.to_string(),
        name: r.name,
        text: r.content,
        published: r.published,
        state: match r.state {
            ReplyStatus::Published => "published",
            ReplyStatus::Hidden => "hidden",
            ReplyStatus::Request => "request",
        }
        .to_string(),
        why: r.why,
        seq: r.seq as i64,
    }
}

fn sent_reply(r: SentReplyInfo) -> SentReply {
    SentReply {
        id: r.id.to_string(),
        post: r.post.to_string(),
        creator: r.creator.to_string(),
        text: r.content,
        published: r.published,
        status: match r.status {
            SentStatus::Sending => "sending",
            SentStatus::Published => "published",
            SentStatus::AwaitingApproval => "awaiting_approval",
            SentStatus::NotPublished => "not_published",
            SentStatus::Closed => "closed",
            SentStatus::Deleted => "deleted",
            SentStatus::Refused => "refused",
            SentStatus::GaveUp => "gave_up",
            SentStatus::Deleting => "deleting",
        }
        .to_string(),
        error: r.error,
        next_try_secs: r.next_try_secs.map(|s| s as i64),
    }
}

fn node_info(p: Pairing) -> NodeInfo {
    NodeInfo {
        name: p.name,
        identity: p.identity.to_string(),
        paired: p.paired,
    }
}

fn post(item: FeedItem) -> Post {
    let author = item
        .actor_name
        .unwrap_or_else(|| item.actor.fmt_short().to_string());
    let (reshared_by, resharer_hosts) = match item.reshared_by {
        Some(by) => (
            Some(by.name.unwrap_or_else(|| by.actor.fmt_short().to_string())),
            by.hosting,
        ),
        None => (None, false),
    };
    Post {
        id: item.id.to_string(),
        author_identity: item.actor.to_string(),
        author,
        published: item.published,
        text: item.content,
        media: item
            .attachments
            .into_iter()
            .map(|a| Media {
                is_video: a.attachment.kind == MediaKind::Video,
                blob: a.attachment.blob.to_string(),
                media_type: a.attachment.media_type,
                size: a.attachment.size as i64,
                width: a.attachment.width,
                height: a.attachment.height,
                duration_ms: a.attachment.duration_ms.map(|d| d as u32),
                thumbnail_blob: a.attachment.thumbnail.map(|t| t.blob.to_string()),
                on_node: a.local,
            })
            .collect(),
        reshared_by,
        resharer_hosts,
        found_through: item.found_through,
        deletable: item.deletable.map(|id| id.to_string()),
    }
}

/// A new device key, as 64 hex digits, for the app to keep in the phone's
/// secure storage.
#[frb(sync)]
pub fn new_device_key() -> String {
    osp_client::new_device_key()
}

/// Makes sure a ticket looks like a pairing ticket before trying to pair.
#[frb(sync)]
pub fn is_pairing_ticket(text: String) -> bool {
    text.trim().to_ascii_uppercase().starts_with("OSPPAIR1")
}

/// Whether text looks like a follow ticket.
#[frb(sync)]
pub fn is_follow_ticket(text: String) -> bool {
    text.trim().to_ascii_uppercase().starts_with("OSP1")
}
