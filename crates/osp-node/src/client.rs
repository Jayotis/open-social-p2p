//! The device protocol `osp/client/0` (ADR 0008): paired phones talking to
//! their home node, and new phones pairing with it.
//!
//! Requests are the same as on the local socket (see `control.rs`), one per
//! QUIC stream. Two carry raw bytes besides their frames: a `Post` with
//! uploads, and `ReadBlob`.

use std::{
    fmt,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use iroh::{
    EndpointAddr, EndpointId, TransportAddr,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use osp_proto::{
    BlobHash, ObjectId, PairingTicket,
    framing::{read_frame_max, write_frame},
    rpc::{
        ClientRequest, ClientResponse, DeviceInfo, MAX_PAIR_REQUEST_LEN, MAX_REQUEST_LEN, Upload,
    },
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::Semaphore,
};
use tracing::{debug, info};

use crate::{
    config::RelayChoice,
    control::{self, Origin},
    db::BlobClass,
    node::Core,
};

/// How long a pairing ticket works.
const PAIRING_TTL: Duration = Duration::from_secs(600);
/// Longest device name kept, in characters.
const MAX_DEVICE_NAME: usize = 64;
/// Largest file a device may upload. Phone videos up to the length limit fit.
const MAX_UPLOAD_BYTES: u64 = 4 << 30;
/// Give up on a request frame, or on an upload that stops moving, after this long.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Most requests from unknown devices handled at once. They can only try
/// to pair, which is rare, so more than this is someone probing the node.
const MAX_UNKNOWN_REQUESTS: usize = 8;
/// Give up on an unknown device's request after this long.
const UNKNOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// A pairing secret not used yet. Only its hash is kept.
pub(crate) struct PendingPairing {
    secret_hash: blake3::Hash,
    expires: Instant,
}

/// Accepts incoming `osp/client/0` connections.
#[derive(Clone)]
pub(crate) struct ClientProtocol {
    core: Arc<Core>,
    /// Shared by every connection, for `MAX_UNKNOWN_REQUESTS`.
    unknown_requests: Arc<Semaphore>,
}

impl ClientProtocol {
    pub fn new(core: Arc<Core>) -> Self {
        Self {
            core,
            unknown_requests: Arc::new(Semaphore::new(MAX_UNKNOWN_REQUESTS)),
        }
    }
}

impl fmt::Debug for ClientProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClientProtocol")
    }
}

impl ProtocolHandler for ClientProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        // The QUIC handshake proved the device holds the key for this ID.
        let device = conn.remote_id();
        while let Ok((send, recv)) = conn.accept_bi().await {
            let core = self.core.clone();
            // Checked on every request, so a device can pair on this
            // connection and carry on, and an unpaired one is refused at once.
            let paired = match core.db.is_device(&device) {
                Ok(paired) => paired,
                Err(e) => {
                    debug!("checking device {}: {e:#}", device.fmt_short());
                    continue;
                }
            };
            if paired {
                tokio::spawn(async move {
                    if let Err(e) = serve_stream(&core, device, true, send, recv).await {
                        debug!(device = %device.fmt_short(), "client request failed: {e:#}");
                    }
                });
                continue;
            }
            let Ok(permit) = self.unknown_requests.clone().try_acquire_owned() else {
                debug!(device = %device.fmt_short(), "too many requests from unknown devices");
                continue;
            };
            tokio::spawn(async move {
                let served = tokio::time::timeout(
                    UNKNOWN_TIMEOUT,
                    serve_stream(&core, device, false, send, recv),
                )
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("took too long")));
                if let Err(e) = served {
                    debug!(device = %device.fmt_short(), "unknown device's request failed: {e:#}");
                }
                drop(permit);
            });
        }
        Ok(())
    }
}

async fn serve_stream(
    core: &Arc<Core>,
    device: EndpointId,
    paired: bool,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<()> {
    // An unknown device may only pair, so its request is read with a small
    // limit and it can't make the node hold a large frame.
    let max = if paired {
        MAX_REQUEST_LEN
    } else {
        MAX_PAIR_REQUEST_LEN
    };
    let req: ClientRequest = tokio::time::timeout(IDLE_TIMEOUT, read_frame_max(&mut recv, max))
        .await
        .context("request timed out")??;
    if let ClientRequest::Pair { secret, name } = req {
        let resp = reply(core.pair_device(device, &secret, &name));
        write_frame(&mut send, &resp).await?;
    } else if !paired {
        let resp = ClientResponse::Error {
            message: "this device isn't paired with this node".to_string(),
        };
        write_frame(&mut send, &resp).await?;
    } else {
        match req {
            ClientRequest::ReadBlob { blob, offset, len } => {
                core.send_blob(&blob, offset, len, &mut send).await?;
            }
            ClientRequest::Post {
                content,
                attachments,
                uploads,
            } if attachments.is_empty() && !uploads.is_empty() => {
                let resp = reply(
                    core.post_uploads(content, &uploads, &mut recv)
                        .await
                        .map(|id| ClientResponse::Posted { id }),
                );
                write_frame(&mut send, &resp).await?;
            }
            req => {
                let resp = core.handle_client(req, Origin::Device).await;
                write_frame(&mut send, &resp).await?;
            }
        }
    }
    send.finish()?;
    Ok(())
}

fn reply(result: Result<ClientResponse>) -> ClientResponse {
    result.unwrap_or_else(|e| ClientResponse::Error {
        message: format!("{e:#}"),
    })
}

impl Core {
    /// Hands out a one-time pairing ticket.
    pub(crate) async fn new_pairing(&self) -> Result<ClientResponse> {
        let mut secret = [0u8; 16];
        getrandom::fill(&mut secret)
            .map_err(|e| anyhow::anyhow!("no secure random numbers available: {e}"))?;
        {
            let mut pending = self.lock_pairings();
            let now = Instant::now();
            pending.retain(|p| p.expires > now);
            pending.push(PendingPairing {
                secret_hash: blake3::hash(&secret),
                expires: now + PAIRING_TTL,
            });
        }
        let relays = self.config.network.relay != RelayChoice::Disabled;
        let ticket = PairingTicket {
            node: pairing_addr(self.full_addr().await, relays),
            secret,
        };
        Ok(ClientResponse::Pairing {
            ticket: ticket.to_string(),
            expires_in_secs: PAIRING_TTL.as_secs(),
        })
    }

    fn pair_device(&self, device: EndpointId, secret: &str, name: &str) -> Result<ClientResponse> {
        // Bad hex hashes as empty input, which never matches.
        let secret = data_encoding::HEXLOWER_PERMISSIVE
            .decode(secret.as_bytes())
            .unwrap_or_default();
        let hash = blake3::hash(&secret);
        {
            let mut pending = self.lock_pairings();
            let now = Instant::now();
            pending.retain(|p| p.expires > now);
            // blake3::Hash compares in constant time, so timing reveals nothing.
            let i = pending
                .iter()
                .position(|p| p.secret_hash == hash)
                .context("this pairing code is wrong, used or expired; run `osp pair` again")?;
            pending.remove(i);
        }
        let name: String = name.trim().chars().take(MAX_DEVICE_NAME).collect();
        self.db.add_device(&device, &name)?;
        info!(device = %device.fmt_short(), %name, "paired a device");
        Ok(ClientResponse::Paired {
            identity: self.identity.public(),
            name: self.config.name.clone(),
        })
    }

    pub(crate) fn unpair(&self, device: &EndpointId) -> Result<()> {
        ensure!(self.db.remove_device(device)?, "no paired device {device}");
        self.tracker.close_all(*device, b"unpaired");
        info!(device = %device.fmt_short(), "unpaired a device");
        Ok(())
    }

    pub(crate) fn devices(&self) -> Result<Vec<DeviceInfo>> {
        let live: Vec<EndpointId> = self.tracker.peers().iter().map(|p| p.node).collect();
        Ok(self
            .db
            .devices()?
            .into_iter()
            .map(|(id, name, paired)| DeviceInfo {
                id,
                name,
                paired,
                connected: live.contains(&id),
            })
            .collect())
    }

    /// Answers `ReadBlob`: a `Blob` frame, then the bytes. Fetches the blob
    /// first if this node doesn't have it.
    async fn send_blob(
        &self,
        blob: &BlobHash,
        offset: u64,
        len: Option<u64>,
        send: &mut SendStream,
    ) -> Result<()> {
        let size = async {
            let size = match self.store.stored_size(blob).await? {
                Some(size) => size,
                None => {
                    // The device only names the blob; our posts say how big it may be.
                    let declared = self.db.declared_size(blob)?.with_context(|| {
                        format!("no post on this node includes {}", blob.fmt_short())
                    })?;
                    self.ensure_blob(blob, BlobClass::Cache, declared).await?
                }
            };
            self.db.touch_blob(blob).ok();
            ensure!(
                offset <= size,
                "offset {offset} is past the end of {} ({size} bytes)",
                blob.fmt_short()
            );
            Ok(size)
        }
        .await;
        let size = match size {
            Ok(size) => size,
            Err(e) => {
                let resp = ClientResponse::Error {
                    message: format!("{e:#}"),
                };
                return write_frame(send, &resp).await;
            }
        };
        let len = len.unwrap_or(u64::MAX).min(size - offset);
        write_frame(send, &ClientResponse::Blob { size }).await?;
        self.store.write_range(blob, offset, len, send).await
    }

    /// Receives a device's files into a scratch directory, then posts them
    /// like local attachments. The scratch copies are deleted either way.
    /// A post the node can't take on is refused before any file bytes are
    /// read: too big, the node busy with other posts, or the disk too full.
    async fn post_uploads(
        self: &Arc<Self>,
        content: String,
        uploads: &[Upload],
        recv: &mut RecvStream,
    ) -> Result<ObjectId> {
        control::check_post(&content, uploads.len())?;
        for u in uploads {
            ensure!(u.size > 0, "{} is empty", u.name);
            ensure!(
                u.size <= MAX_UPLOAD_BYTES,
                "{} is larger than the {} GB limit",
                u.name,
                MAX_UPLOAD_BYTES >> 30
            );
        }
        let _slot = self.media_post_slot()?;
        // Room for every file before any arrives, so two uploads at once
        // can't both count on the same free space.
        let mut rooms = Vec::new();
        for u in uploads {
            rooms.push(self.store.reserve_upload(u.size).await?);
        }

        let dir = upload_dir(&self.home.work())?;
        let result = async {
            let mut paths = Vec::new();
            for ((i, upload), room) in uploads.iter().enumerate().zip(rooms) {
                let path = dir.join(format!("{i}.{}", extension(&upload.name)));
                receive_file(recv, &path, upload.size)
                    .await
                    .with_context(|| format!("receiving {}", upload.name))?;
                // The file is on the disk now, so the free space counts it.
                drop(room);
                paths.push(path);
            }
            self.publish_post(content, &paths).await
        }
        .await;
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    fn lock_pairings(&self) -> std::sync::MutexGuard<'_, Vec<PendingPairing>> {
        self.pairings.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Copies exactly `size` bytes from the stream into a new file.
async fn receive_file<R: AsyncRead + Unpin>(recv: &mut R, path: &Path, size: u64) -> Result<()> {
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("creating {}", path.display()))?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut left = size;
    while left > 0 {
        let n = left.min(buf.len() as u64) as usize;
        tokio::time::timeout(IDLE_TIMEOUT, recv.read_exact(&mut buf[..n]))
            .await
            .context("upload stalled")?
            .context("upload ended early")?;
        file.write_all(&buf[..n]).await?;
        left -= n as u64;
    }
    file.flush().await?;
    Ok(())
}

/// The addresses that go in a pairing ticket: the relay, plus addresses on
/// private networks, so a phone at home can connect directly even without
/// internet. Public addresses are left out to keep the QR code small; the
/// phone learns them through the relay. Without relays, everything is kept.
fn pairing_addr(addr: EndpointAddr, relays: bool) -> EndpointAddr {
    if !relays {
        return addr;
    }
    let keep = |a: &TransportAddr| match a {
        TransportAddr::Relay(_) => true,
        TransportAddr::Ip(ip) => match ip.ip() {
            IpAddr::V4(v4) => v4.is_private(),
            IpAddr::V6(v6) => v6.is_unique_local(),
        },
        _ => false,
    };
    EndpointAddr::from_parts(addr.id, addr.addrs.into_iter().filter(keep))
}

fn upload_dir(work: &Path) -> Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = work.join(format!("upload-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// The upload's extension if it looks like one. ffmpeg goes by the content,
/// so this only helps when reading logs.
fn extension(name: &str) -> String {
    match Path::new(name).extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) => {
            ext.to_ascii_lowercase()
        }
        _ => "bin".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_tickets_keep_relay_and_private_addresses() {
        let id = iroh::SecretKey::generate().public();
        let addr = EndpointAddr::new(id)
            .with_relay_url("https://relay.example.org./".parse().unwrap())
            .with_ip_addr("172.16.5.20:4433".parse().unwrap())
            .with_ip_addr("203.0.113.9:4433".parse().unwrap())
            .with_ip_addr("[fd00::20]:4433".parse().unwrap())
            .with_ip_addr("[2001:db8::20]:4433".parse().unwrap());
        let kept = pairing_addr(addr.clone(), true);
        let ips: Vec<String> = kept.ip_addrs().map(|a| a.to_string()).collect();
        assert_eq!(ips, ["172.16.5.20:4433", "[fd00::20]:4433"]);
        assert_eq!(kept.relay_urls().count(), 1);
        assert_eq!(pairing_addr(addr.clone(), false), addr);
    }

    #[test]
    fn upload_extensions_are_sanitised() {
        assert_eq!(extension("IMG_0001.JPG"), "jpg");
        assert_eq!(extension("clip.mp4"), "mp4");
        assert_eq!(extension("../../etc/passwd"), "bin");
        assert_eq!(extension("x.a/b"), "bin");
        assert_eq!(extension("noext"), "bin");
        assert_eq!(extension("weird.sh;rm"), "bin");
    }

    #[test]
    fn largest_requests_fit_their_limits() {
        // Control characters are escaped as \u00XX: six bytes each.
        let post = ClientRequest::Post {
            content: "\u{1}".repeat(osp_proto::types::MAX_CONTENT_BYTES),
            attachments: Vec::new(),
            uploads: (0..osp_proto::types::MAX_ATTACHMENTS)
                .map(|_| Upload {
                    name: "\u{1}".repeat(255),
                    size: u64::MAX,
                })
                .collect(),
        };
        let len = serde_json::to_vec(&post).unwrap().len();
        assert!(len <= MAX_REQUEST_LEN as usize, "post is {len} bytes");

        let pair = ClientRequest::Pair {
            secret: "f".repeat(32),
            name: "\u{1}".repeat(MAX_DEVICE_NAME * 4),
        };
        let len = serde_json::to_vec(&pair).unwrap().len();
        assert!(len <= MAX_PAIR_REQUEST_LEN as usize, "pair is {len} bytes");
    }

    #[tokio::test]
    async fn short_upload_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let mut data: &[u8] = b"only ten b";
        receive_file(&mut data, &path, 10).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"only ten b");
        let mut data: &[u8] = b"short";
        assert!(receive_file(&mut data, &path, 10).await.is_err());
    }
}
