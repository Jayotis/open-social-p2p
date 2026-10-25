//! What a paired phone runs: one iroh endpoint that talks only to its own
//! home node, over `osp/client/0` (ADR 0001, ADR 0008).
//!
//! The Android app calls this through flutter_rust_bridge. Tests and desktop
//! tools can use it directly. Everything the client keeps lives in one
//! directory: the pairing, the last feed it saw, a post still on its way to
//! the node, and its device key, unless the app keeps that in the phone's
//! secure storage (`open_with_key`).

mod media;

use std::{
    fs::OpenOptions,
    io::{SeekFrom, Write},
    net::SocketAddr,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{
    Endpoint, EndpointAddr, PublicKey, RelayMode,
    address_lookup::{DnsAddressLookup, MemoryLookup, PkarrResolver},
    endpoint::{Connection, RecvStream, SendStream, presets},
};
use osp_proto::{
    BlobHash, CLIENT_ALPN, ObjectId, PairingTicket,
    framing::{read_frame, write_frame},
    rpc::{ClientRequest, ClientResponse, FeedItem, Upload, UploadId},
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing::debug;

pub use iroh::SecretKey;
pub use osp_proto;

use crate::media::MediaServer;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long to wait for an ordinary reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);
/// How long to wait for a post with media: the node transcodes before replying.
const POST_REPLY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How long to wait for a blob the node may first have to fetch, and for
/// `Fetch` and `Sync`, which can also wait on other nodes.
const BLOB_REPLY_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Waits between tries to reach the home node again during a post. The
/// last one repeats.
const POST_RETRY_WAITS: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];
/// A post whose bytes stop moving for this long is carried on over a new
/// connection.
const POST_STALL: Duration = Duration::from_secs(30);
/// After sending fails, how long to wait for the node's reply, which says
/// why if the node refused the post part way.
const REFUSAL_WAIT: Duration = Duration::from_secs(5);
/// Where the post still on its way to the home node is kept.
const PENDING_POST: &str = "pending-post.json";

/// How the client reaches the network.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// Use n0's public relays, so the node can be reached from anywhere.
    pub relays: bool,
    /// Look the node up by its ID (n0's DNS service) if its relay changes
    /// after pairing. The client never publishes its own address.
    pub address_lookup: bool,
    /// Bind to this address only (for example `127.0.0.1:0` in tests).
    pub bind: Option<SocketAddr>,
    /// A post gives up after this long without contact with the home node.
    /// Ten minutes covers the usual short breaks (a lift, Wi-Fi to mobile
    /// data, the home computer restarting); past that, something needs the
    /// owner's attention.
    pub post_give_up: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            relays: true,
            address_lookup: true,
            bind: None,
            post_give_up: Duration::from_secs(10 * 60),
        }
    }
}

impl ClientOptions {
    /// Loopback only, no relays or lookups: for tests against local nodes.
    pub fn local_test() -> Self {
        Self {
            relays: false,
            address_lookup: false,
            bind: Some("127.0.0.1:0".parse().expect("valid address")),
            ..Self::default()
        }
    }
}

/// The home node this device is paired with. Saved as `pairing.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pairing {
    pub node: EndpointAddr,
    /// The identity the node speaks for.
    pub identity: PublicKey,
    /// The node owner's display name.
    pub name: String,
    /// When pairing happened (RFC 3339).
    pub paired: String,
}

/// The feed as last fetched, for when the home node can't be reached.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedFeed {
    pub items: Vec<FeedItem>,
    /// When it was fetched (RFC 3339).
    pub saved: String,
}

/// A post whose files are on their way to the home node. Saved as
/// `pending-post.json` until the node publishes it or the owner discards
/// it, so it can be carried on after a broken connection or a restart of
/// the app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPost {
    /// Names the upload to the node (`ResumeUpload`).
    pub upload: UploadId,
    pub content: String,
    pub files: Vec<PendingFile>,
    /// When the post was started (RFC 3339).
    pub started: String,
}

/// A file of a pending post, with what it looked like when the post began.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingFile {
    pub path: PathBuf,
    pub size: u64,
    /// When the file last changed, in milliseconds since 1970.
    pub modified_ms: u64,
}

impl PendingPost {
    fn new(content: String, files: &[PathBuf]) -> Result<Self> {
        let mut id = [0u8; 32];
        getrandom::fill(&mut id)
            .map_err(|e| anyhow::anyhow!("no secure random numbers available: {e}"))?;
        Ok(Self {
            upload: UploadId::from_bytes(id),
            content,
            files: files
                .iter()
                .map(|path| PendingFile::read(path))
                .collect::<Result<_>>()?,
            started: osp_proto::now_rfc3339(),
        })
    }

    /// The files as the node knows them.
    fn uploads(&self) -> Vec<Upload> {
        self.files
            .iter()
            .map(|f| Upload {
                name: f.name(),
                size: f.size,
            })
            .collect()
    }

    fn total(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Stops a post from carrying on with files that changed since it
    /// began, as their bytes would no longer line up with what was sent.
    fn check_files(&self) -> Result<()> {
        for file in &self.files {
            let now = PendingFile::read(&file.path)?;
            ensure!(
                (now.size, now.modified_ms) == (file.size, file.modified_ms),
                "{} changed since the post began; discard the post and post it again",
                file.name()
            );
        }
        Ok(())
    }
}

impl PendingFile {
    fn read(path: &Path) -> Result<Self> {
        let meta =
            std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        Ok(Self {
            path: path.to_path_buf(),
            size: meta.len(),
            modified_ms,
        })
    }

    fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// How a post is getting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostProgress {
    /// `sent` of `total` bytes have gone to the home node. Once all have,
    /// the node is preparing the post.
    Sending { sent: u64, total: u64 },
    /// Lost contact with the home node; trying again.
    Waiting,
}

/// How one try at sending a post ended, short of a broken connection.
enum Attempt {
    Posted(ObjectId),
    /// The node said no, and why.
    Refused(String),
}

/// How the device is connected to its home node right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// `direct`, `relay`, or `none` if the connection has no open path.
    pub path: String,
    pub rtt_ms: Option<u64>,
}

pub struct Client {
    dir: PathBuf,
    endpoint: Endpoint,
    lookup: MemoryLookup,
    pairing: Mutex<Option<Pairing>>,
    /// The open connection to the home node, if any. Held across a connect
    /// so two callers don't dial at once.
    conn: tokio::sync::Mutex<Option<Connection>>,
    media: tokio::sync::OnceCell<MediaServer>,
    post_give_up: Duration,
}

impl Client {
    /// Opens (or creates) the client state in `dir` and starts its endpoint.
    /// The device key is kept in `dir` too, as a file.
    pub async fn open(dir: &Path, options: ClientOptions) -> Result<Arc<Self>> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let key = load_or_create_key(&dir.join("device.key"))?;
        Self::open_with_key(dir, key, options).await
    }

    /// Like `open`, with a device key kept somewhere safer than a file,
    /// such as the phone's secure storage. Nothing about the key is
    /// written to `dir`.
    pub async fn open_with_key(
        dir: &Path,
        key: SecretKey,
        options: ClientOptions,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let pairing: Option<Pairing> = read_json(&dir.join("pairing.json"))?;

        let lookup = MemoryLookup::new();
        if let Some(p) = &pairing {
            lookup.add_endpoint_info(p.node.clone());
        }
        let mut builder = Endpoint::builder(presets::Minimal)
            .secret_key(key)
            .address_lookup(lookup.clone())
            .relay_mode(if options.relays {
                RelayMode::Default
            } else {
                RelayMode::Disabled
            });
        if options.address_lookup {
            builder = builder
                .address_lookup(PkarrResolver::n0_dns())
                .address_lookup(DnsAddressLookup::n0_dns());
        }
        if let Some(addr) = options.bind {
            builder = builder.clear_ip_transports().bind_addr(addr)?;
        }
        let endpoint = builder.bind().await.context("starting network endpoint")?;

        Ok(Arc::new(Self {
            dir: dir.to_path_buf(),
            endpoint,
            lookup,
            pairing: Mutex::new(pairing),
            conn: tokio::sync::Mutex::new(None),
            media: tokio::sync::OnceCell::new(),
            post_give_up: options.post_give_up,
        }))
    }

    /// This device's ID, as the home node lists it.
    pub fn device_id(&self) -> PublicKey {
        self.endpoint.id()
    }

    pub fn pairing(&self) -> Option<Pairing> {
        self.lock_pairing().clone()
    }

    /// Pairs with the node in `ticket` and remembers it.
    pub async fn pair(&self, ticket: &str, device_name: &str) -> Result<Pairing> {
        let ticket: PairingTicket = ticket.parse()?;
        self.disconnect().await;
        self.lookup.add_endpoint_info(ticket.node.clone());
        let conn = self.dial(&ticket.node).await?;
        let req = ClientRequest::Pair {
            secret: data_encoding::HEXLOWER.encode(&ticket.secret),
            name: device_name.to_string(),
        };
        let resp = request_on(&conn, &req, REPLY_TIMEOUT).await?;
        let ClientResponse::Paired { identity, name } = check(resp)? else {
            bail!("unexpected reply to Pair");
        };
        let pairing = Pairing {
            node: ticket.node,
            identity,
            name,
            paired: osp_proto::now_rfc3339(),
        };
        write_json(&self.dir.join("pairing.json"), &pairing)?;
        *self.lock_pairing() = Some(pairing.clone());
        *self.conn.lock().await = Some(conn);
        Ok(pairing)
    }

    /// Forgets the home node and everything saved from it. The device key
    /// stays, so pairing again shows up as the same device.
    pub async fn forget(&self) -> Result<()> {
        self.disconnect().await;
        *self.lock_pairing() = None;
        for file in ["pairing.json", "feed.json", PENDING_POST] {
            match std::fs::remove_file(self.dir.join(file)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(())
    }

    /// Sends one request to the home node and waits for its reply. A reply
    /// of `Error` becomes an `Err` with the node's message.
    pub async fn request(&self, req: &ClientRequest) -> Result<ClientResponse> {
        let timeout = match req {
            ClientRequest::Fetch { .. } | ClientRequest::Sync => BLOB_REPLY_TIMEOUT,
            _ => REPLY_TIMEOUT,
        };
        let (mut send, mut recv) = self.send_request(req).await?;
        send.finish()?;
        check(read_reply(&mut recv, timeout).await?)
    }

    /// Fetches the feed and saves a copy for offline use.
    pub async fn feed(&self, limit: u32) -> Result<Vec<FeedItem>> {
        let ClientResponse::Feed { items } = self.request(&ClientRequest::Feed { limit }).await?
        else {
            bail!("unexpected reply to Feed");
        };
        let cached = CachedFeed {
            items: items.clone(),
            saved: osp_proto::now_rfc3339(),
        };
        if let Err(e) = write_json(&self.dir.join("feed.json"), &cached) {
            debug!("couldn't save the feed: {e:#}");
        }
        Ok(items)
    }

    /// The feed as last fetched, if any.
    pub fn cached_feed(&self) -> Option<CachedFeed> {
        read_json(&self.dir.join("feed.json")).ok().flatten()
    }

    /// Publishes a post with files from this device. The node transcodes
    /// them before replying. `progress` is called with bytes sent and total.
    pub async fn post(
        &self,
        content: String,
        files: &[PathBuf],
        progress: impl Fn(u64, u64),
    ) -> Result<ObjectId> {
        self.post_with(content, files, |p| {
            if let PostProgress::Sending { sent, total } = p {
                progress(sent, total);
            }
        })
        .await
    }

    /// Publishes a post with files from this device, carrying on where it
    /// stopped after a broken connection, until `ClientOptions::post_give_up`
    /// passes without contact with the home node. The post is saved first and
    /// kept until the node publishes it, so if this fails, or the app
    /// stops, `continue_post` picks it up. Replaces any post still pending.
    pub async fn post_with(
        &self,
        content: String,
        files: &[PathBuf],
        progress: impl Fn(PostProgress),
    ) -> Result<ObjectId> {
        let pending = PendingPost::new(content, files)?;
        write_json(&self.dir.join(PENDING_POST), &pending)?;
        self.send_post(&pending, true, progress).await
    }

    /// The post still on its way to the home node, if any: one that failed
    /// or was interrupted by the app stopping.
    pub fn pending_post(&self) -> Option<PendingPost> {
        read_json(&self.dir.join(PENDING_POST)).ok().flatten()
    }

    /// Carries on with the pending post where it stopped.
    pub async fn continue_post(&self, progress: impl Fn(PostProgress)) -> Result<ObjectId> {
        let pending = self
            .pending_post()
            .context("no post is waiting to be sent")?;
        self.send_post(&pending, false, progress).await
    }

    /// Forgets the pending post on this device, and asks the home node to
    /// drop what it has of it if the node can be reached (otherwise the
    /// node drops it within a day). Fails with the node's reason if the
    /// post is already being prepared or was published.
    pub async fn discard_post(&self) -> Result<()> {
        let Some(pending) = self.pending_post() else {
            return Ok(());
        };
        let req = ClientRequest::CancelUpload {
            upload: pending.upload,
        };
        let reply = async {
            let (mut send, mut recv) = self.send_request(&req).await?;
            send.finish()?;
            read_reply(&mut recv, REPLY_TIMEOUT).await
        }
        .await;
        std::fs::remove_file(self.dir.join(PENDING_POST))?;
        match reply {
            Ok(ClientResponse::Error { message }) => bail!(message),
            Ok(_) => Ok(()),
            Err(e) => {
                debug!("couldn't tell the home node to drop the post: {e:#}");
                Ok(())
            }
        }
    }

    /// Sends a pending post until the node publishes or refuses it, or
    /// there's no contact for `post_give_up`. The first try of a new post
    /// is a `Post`; later ones are `ResumeUpload`s.
    async fn send_post(
        &self,
        pending: &PendingPost,
        mut first: bool,
        progress: impl Fn(PostProgress),
    ) -> Result<ObjectId> {
        let mut lost_since: Option<Instant> = None;
        let mut tries = 0;
        loop {
            if !first {
                pending.check_files()?;
            }
            let mut contact = false;
            let tried = Instant::now();
            let attempt = self.try_post(pending, first, &progress, &mut contact).await;
            if contact {
                lost_since = None;
                tries = 0;
            }
            let e = match attempt {
                Ok(Attempt::Posted(id)) => {
                    std::fs::remove_file(self.dir.join(PENDING_POST)).ok();
                    return Ok(id);
                }
                Ok(Attempt::Refused(message)) => bail!(message),
                Err(e) => e,
            };
            debug!("post interrupted, trying again: {e:#}");
            // The next try dials again, in case this connection is dead.
            self.disconnect().await;
            // Without contact since this try began, or since it broke.
            let since = *lost_since.get_or_insert(if contact { Instant::now() } else { tried });
            if since.elapsed() >= self.post_give_up {
                return Err(e.context(
                    "no contact with your home node for a while; the post is kept, to carry on later",
                ));
            }
            progress(PostProgress::Waiting);
            tokio::time::sleep(POST_RETRY_WAITS[tries.min(POST_RETRY_WAITS.len() - 1)]).await;
            tries += 1;
            first = false;
        }
    }

    /// One try at sending a post. An `Err` is a broken connection, worth
    /// another try. `contact` is set once the node is heard from or bytes
    /// go out.
    async fn try_post(
        &self,
        pending: &PendingPost,
        first: bool,
        progress: &impl Fn(PostProgress),
        contact: &mut bool,
    ) -> Result<Attempt> {
        let req = if first {
            ClientRequest::Post {
                content: pending.content.clone(),
                attachments: Vec::new(),
                uploads: pending.uploads(),
                upload: Some(pending.upload),
            }
        } else {
            ClientRequest::ResumeUpload {
                upload: pending.upload,
                content: pending.content.clone(),
                uploads: pending.uploads(),
            }
        };
        let (mut send, mut recv) = self.send_request(&req).await?;
        let from = if first {
            0
        } else {
            let reply = read_reply(&mut recv, REPLY_TIMEOUT).await?;
            *contact = true;
            match reply {
                ClientResponse::Resume { received } => received,
                ClientResponse::Posted { id } => return Ok(Attempt::Posted(id)),
                ClientResponse::Error { message } => return Ok(Attempt::Refused(message)),
                other => {
                    let message = format!("unexpected reply to ResumeUpload: {other:?}");
                    return Ok(Attempt::Refused(message));
                }
            }
        };
        progress(PostProgress::Sending {
            sent: from,
            total: pending.total(),
        });
        let sent = send_files(pending, from, &mut send, progress, contact).await;
        // If the node refuses the post part way, it stops reading and its
        // reply says why, so read the reply even when sending failed.
        let wait = if sent.is_ok() {
            POST_REPLY_TIMEOUT
        } else {
            REFUSAL_WAIT
        };
        match (sent, read_reply(&mut recv, wait).await) {
            (_, Ok(ClientResponse::Posted { id })) => Ok(Attempt::Posted(id)),
            (_, Ok(ClientResponse::Error { message })) => Ok(Attempt::Refused(message)),
            (Err(e), _) => Err(e.context("sending files to the home node")),
            (Ok(()), Err(e)) => Err(e),
            (Ok(()), Ok(other)) => Ok(Attempt::Refused(format!(
                "unexpected reply to Post: {other:?}"
            ))),
        }
    }

    /// A URL on 127.0.0.1 that serves a blob from the home node, for the
    /// video player and image widgets. Supports range requests.
    pub async fn media_url(self: &Arc<Self>, blob: &BlobHash, media_type: &str) -> Result<String> {
        let server = self
            .media
            .get_or_try_init(|| MediaServer::start(Arc::downgrade(self)))
            .await?;
        Ok(server.url(blob, media_type))
    }

    /// Waits until the home node has the start of a blob, fetching it if
    /// it doesn't, so a player opened next has bytes to read at once. Fails
    /// with the node's reason if it can't get it. A node from before
    /// streaming answers only once it has the whole blob.
    pub async fn wait_for_media(&self, blob: &BlobHash) -> Result<()> {
        self.read_blob(blob, 0, Some(1)).await?;
        Ok(())
    }

    /// Opens a `ReadBlob` stream: the blob's total size, and a stream of
    /// `len` bytes (or up to the end) from `offset`.
    pub(crate) async fn read_blob(
        &self,
        blob: &BlobHash,
        offset: u64,
        len: Option<u64>,
    ) -> Result<(u64, RecvStream)> {
        let req = ClientRequest::ReadBlob {
            blob: *blob,
            offset,
            len,
        };
        let (mut send, mut recv) = self.send_request(&req).await?;
        send.finish()?;
        match check(read_reply(&mut recv, BLOB_REPLY_TIMEOUT).await?)? {
            ClientResponse::Blob { size } => Ok((size, recv)),
            other => bail!("unexpected reply to ReadBlob: {other:?}"),
        }
    }

    /// How the device reaches its home node right now, without connecting.
    pub async fn connection_info(&self) -> Option<ConnectionInfo> {
        let guard = self.conn.lock().await;
        let conn = guard.as_ref().filter(|c| c.close_reason().is_none())?;
        let paths = conn.paths();
        let selected = paths.iter().find(|p| p.is_selected());
        let path = match &selected {
            Some(p) if p.is_relay() => "relay",
            Some(p) if p.is_ip() => "direct",
            _ => "none",
        };
        Some(ConnectionInfo {
            path: path.to_string(),
            rtt_ms: selected.map(|p| p.rtt().as_millis() as u64),
        })
    }

    /// Tells the network stack the phone's network may have changed (Wi-Fi
    /// to mobile data, for example). Android doesn't let native code watch
    /// for this itself.
    pub async fn network_change(&self) {
        self.endpoint.network_change().await;
    }

    /// Closes the connection to the home node, for example while the app is
    /// in the background. The next request reconnects.
    pub async fn disconnect(&self) {
        if let Some(conn) = self.conn.lock().await.take() {
            conn.close(0u32.into(), b"bye");
        }
    }

    /// Closes the endpoint. Call before dropping the client.
    pub async fn shutdown(&self) {
        self.disconnect().await;
        self.endpoint.close().await;
    }

    /// Opens a stream and writes `req` to it. If the cached connection
    /// turns out to be dead before the request is sent, reconnects once; a
    /// request that may have reached the node is never sent twice.
    async fn send_request(&self, req: &ClientRequest) -> Result<(SendStream, RecvStream)> {
        let mut retried = false;
        loop {
            let conn = self.connection().await?;
            let sent = async {
                let (mut send, recv) = conn.open_bi().await?;
                write_frame(&mut send, req).await?;
                anyhow::Ok((send, recv))
            }
            .await;
            match sent {
                Ok(streams) => return Ok(streams),
                Err(e) if !retried => {
                    debug!("connection to home node failed, reconnecting: {e:#}");
                    self.conn.lock().await.take();
                    retried = true;
                }
                Err(e) => return Err(e.context("sending to the home node")),
            }
        }
    }

    async fn connection(&self) -> Result<Connection> {
        let mut guard = self.conn.lock().await;
        if let Some(conn) = guard.as_ref()
            && conn.close_reason().is_none()
        {
            return Ok(conn.clone());
        }
        let node = self.pairing().context("not paired with a home node")?.node;
        let conn = self.dial(&node).await?;
        *guard = Some(conn.clone());
        Ok(conn)
    }

    async fn dial(&self, node: &EndpointAddr) -> Result<Connection> {
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(node.clone(), CLIENT_ALPN),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "the home node didn't answer within {} seconds",
                CONNECT_TIMEOUT.as_secs()
            )
        })?
        .context("can't reach the home node")
    }

    fn lock_pairing(&self) -> std::sync::MutexGuard<'_, Option<Pairing>> {
        self.pairing.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Sends a pending post's files from byte `from` on, counting them back to
/// back, then finishes the stream. Fails if the bytes stop moving for
/// `POST_STALL`.
async fn send_files(
    pending: &PendingPost,
    from: u64,
    send: &mut SendStream,
    progress: &impl Fn(PostProgress),
    contact: &mut bool,
) -> Result<()> {
    let total = pending.total();
    let mut done = from;
    let mut start = 0;
    let mut buf = vec![0u8; 256 * 1024];
    for file in &pending.files {
        let end = start + file.size;
        if from < end {
            let skip = from.saturating_sub(start);
            let mut f = tokio::fs::File::open(&file.path)
                .await
                .with_context(|| format!("reading {}", file.path.display()))?;
            f.seek(SeekFrom::Start(skip)).await?;
            let mut f = f.take(file.size - skip);
            loop {
                let n = f.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                tokio::time::timeout(POST_STALL, send.write_all(&buf[..n]))
                    .await
                    .context("the upload stopped moving")??;
                *contact = true;
                done += n as u64;
                progress(PostProgress::Sending { sent: done, total });
            }
        }
        start = end;
    }
    ensure!(done == total, "a file got shorter while it was being sent");
    send.finish()?;
    Ok(())
}

async fn request_on(
    conn: &Connection,
    req: &ClientRequest,
    timeout: Duration,
) -> Result<ClientResponse> {
    let (mut send, mut recv) = conn.open_bi().await?;
    write_frame(&mut send, req).await?;
    send.finish()?;
    read_reply(&mut recv, timeout).await
}

async fn read_reply(recv: &mut RecvStream, timeout: Duration) -> Result<ClientResponse> {
    // The timeout's own error says only "deadline has elapsed", so replace it.
    tokio::time::timeout(timeout, read_frame(recv))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "the home node didn't reply within {} seconds",
                timeout.as_secs()
            )
        })?
}

/// Turns an `Error` reply into an `Err`.
fn check(resp: ClientResponse) -> Result<ClientResponse> {
    match resp {
        ClientResponse::Error { message } => bail!(message),
        resp => Ok(resp),
    }
}

/// A new device key, written as 64 hex digits for storing outside the client.
pub fn new_device_key() -> String {
    device_key_to_hex(&SecretKey::generate())
}

/// Reads a device key written as 64 hex digits.
pub fn device_key_from_hex(text: &str) -> Result<SecretKey> {
    let bytes = data_encoding::HEXLOWER
        .decode(text.trim().as_bytes())
        .context("the device key is not hex")?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("the device key must be 32 bytes"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

fn device_key_to_hex(key: &SecretKey) -> String {
    data_encoding::HEXLOWER.encode(&key.to_bytes())
}

/// The device key file: 32 bytes as hex, readable only by the app.
fn load_or_create_key(path: &Path) -> Result<SecretKey> {
    if path.exists() {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        return device_key_from_hex(&text).with_context(|| format!("in {}", path.display()));
    }
    let key = SecretKey::generate();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(device_key_to_hex(&key).as_bytes())?;
    Ok(key)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Writes through a temporary file, so a crash never leaves half a file.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(value)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
