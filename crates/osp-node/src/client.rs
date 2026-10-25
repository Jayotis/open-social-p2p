//! The device protocol `osp/client/0` (ADR 0008): paired phones talking to
//! their home node, and new phones pairing with it.
//!
//! Requests are the same as on the local socket (see `control.rs`), one per
//! QUIC stream. Three carry raw bytes besides their frames: a `Post` with
//! uploads and `ResumeUpload` (both in `uploads.rs`), and `ReadBlob`.

use std::{
    collections::HashMap,
    fmt,
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{
    EndpointAddr, EndpointId, TransportAddr,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use osp_proto::{
    BlobHash, PairingTicket,
    framing::{read_frame_max, write_frame},
    rpc::{ClientRequest, ClientResponse, DeviceInfo, MAX_PAIR_REQUEST_LEN, MAX_REQUEST_LEN},
};
use tokio::sync::{Semaphore, watch};
use tracing::{debug, info};

use crate::{config::RelayChoice, control::Origin, db::BlobClass, node::Core, store::Download};

/// How long a pairing ticket works.
const PAIRING_TTL: Duration = Duration::from_secs(600);
/// Longest device name kept, in characters.
const MAX_DEVICE_NAME: usize = 64;
/// Give up on a request frame, or on an upload that stops moving, after this long.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
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
                upload,
            } if attachments.is_empty() && !uploads.is_empty() => {
                let resp = reply(
                    core.receive_post(device, upload, content, &uploads, &mut recv)
                        .await
                        .map(|id| ClientResponse::Posted { id }),
                );
                write_frame(&mut send, &resp).await?;
            }
            ClientRequest::ResumeUpload {
                upload,
                content,
                uploads,
            } => {
                let resp = reply(
                    core.resume_upload(device, upload, content, &uploads, &mut send, &mut recv)
                        .await,
                );
                write_frame(&mut send, &resp).await?;
            }
            ClientRequest::CancelUpload { upload } => {
                let resp = reply(
                    core.cancel_upload(device, &upload)
                        .await
                        .map(|()| ClientResponse::Ok),
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
            identity: self.signer.identity(),
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

    /// Answers `ReadBlob`: a `Blob` frame, then the bytes. A blob this node
    /// doesn't have yet is fetched, and its bytes are sent as they arrive.
    /// The frame waits for the first of them, so a fetch that fails from
    /// the start still sends the device its reason.
    async fn send_blob(
        self: &Arc<Self>,
        blob: &BlobHash,
        offset: u64,
        len: Option<u64>,
        send: &mut SendStream,
    ) -> Result<()> {
        // The size, and the download if the blob isn't all here.
        let found = async {
            if let Some(size) = self.store.stored_size(blob).await? {
                self.db.touch_blob(blob).ok();
                ensure!(
                    offset <= size,
                    "offset {offset} is past the end of {} ({size} bytes)",
                    blob.fmt_short()
                );
                return Ok((size, None));
            }
            // The device only names the blob; our posts say how big it is.
            let size = self
                .db
                .declared_size(blob)?
                .with_context(|| format!("no post on this node includes {}", blob.fmt_short()))?;
            ensure!(
                offset <= size,
                "offset {offset} is past the end of {} ({size} bytes)",
                blob.fmt_short()
            );
            let mut download = self.open_blob(blob, size);
            let mut arriving = self.store.arriving(blob).await?;
            if offset < size {
                tokio::select! {
                    stored = arriving.stored_from(offset, &mut download) => { stored?; }
                    _ = send.stopped() => bail!("the device stopped waiting"),
                }
            }
            Ok((size, Some((arriving, download))))
        }
        .await;
        let (size, arriving) = match found {
            Ok(found) => found,
            Err(e) => {
                let resp = ClientResponse::Error {
                    message: format!("{e:#}"),
                };
                return write_frame(send, &resp).await;
            }
        };
        let len = len.unwrap_or(u64::MAX).min(size - offset);
        write_frame(send, &ClientResponse::Blob { size }).await?;
        let Some((mut arriving, mut download)) = arriving else {
            return self.store.write_range(blob, offset, len, send).await;
        };
        // What is here, then more as it comes, until the device hangs up.
        let end = offset + len;
        let mut position = offset;
        while position < end {
            let stored = tokio::select! {
                stored = arriving.stored_from(position, &mut download) => stored?,
                _ = send.stopped() => return Ok(()),
            };
            let upto = stored.min(end);
            self.store
                .write_range(blob, position, upto - position, send)
                .await?;
            position = upto;
        }
        Ok(())
    }

    /// Starts fetching a blob a device opened, or joins the fetch under
    /// way, so devices reading it at the same time share one download. The
    /// receiver says when it's over.
    fn open_blob(self: &Arc<Self>, blob: &BlobHash, declared: u64) -> watch::Receiver<Download> {
        let mut opening = self.lock_opening();
        // A download whose task is gone without saying how it ended is
        // started again.
        if let Some(download) = opening.get(blob)
            && download.has_changed().is_ok()
        {
            return download.clone();
        }
        let (tx, rx) = watch::channel(Download::Running);
        opening.insert(*blob, rx.clone());
        let (core, blob) = (self.clone(), *blob);
        tokio::spawn(async move {
            let fetched = core.ensure_blob(&blob, BlobClass::Cache, declared).await;
            core.lock_opening().remove(&blob);
            tx.send_replace(match fetched {
                Ok(_) => {
                    core.db.touch_blob(&blob).ok();
                    Download::Done
                }
                Err(e) => {
                    debug!(blob = %blob.fmt_short(), "couldn't fetch media a device opened: {e:#}");
                    Download::Failed(format!("{e:#}"))
                }
            });
        });
        rx
    }

    fn lock_pairings(&self) -> std::sync::MutexGuard<'_, Vec<PendingPairing>> {
        self.pairings.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_opening(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<BlobHash, watch::Receiver<Download>>> {
        self.opening.lock().unwrap_or_else(|e| e.into_inner())
    }
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

#[cfg(test)]
mod tests {
    use osp_proto::rpc::{Upload, UploadId};

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
    fn largest_requests_fit_their_limits() {
        // Control characters are escaped as \u00XX: six bytes each.
        let content = "\u{1}".repeat(osp_proto::types::MAX_CONTENT_BYTES);
        let uploads: Vec<Upload> = (0..osp_proto::types::MAX_ATTACHMENTS)
            .map(|_| Upload {
                name: "\u{1}".repeat(255),
                size: u64::MAX,
            })
            .collect();
        let upload = UploadId::from_bytes([0xff; 32]);
        let post = ClientRequest::Post {
            content: content.clone(),
            attachments: Vec::new(),
            uploads: uploads.clone(),
            upload: Some(upload),
        };
        let resume = ClientRequest::ResumeUpload {
            upload,
            content,
            uploads,
        };
        for req in [post, resume] {
            let len = serde_json::to_vec(&req).unwrap().len();
            assert!(len <= MAX_REQUEST_LEN as usize, "{req:?} is {len} bytes");
        }

        let pair = ClientRequest::Pair {
            secret: "f".repeat(32),
            name: "\u{1}".repeat(MAX_DEVICE_NAME * 4),
        };
        let len = serde_json::to_vec(&pair).unwrap().len();
        assert!(len <= MAX_PAIR_REQUEST_LEN as usize, "pair is {len} bytes");
    }
}
