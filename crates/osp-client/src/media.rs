//! A small HTTP server on 127.0.0.1, so the phone's video player and image
//! widgets can load media from the home node by URL.
//!
//! Each HTTP request becomes one `ReadBlob` over the connection to the home
//! node. A `Range` header becomes an offset and length, so seeking works.
//! Media the node is still fetching arrives as the node gets it.
//! Other apps on the phone can reach 127.0.0.1 too, so every URL carries a
//! random token and requests without it are refused.
//!
//! Only what players need is supported: GET and HEAD, single byte ranges,
//! one request per connection.

use std::{
    sync::{Arc, Weak},
    time::Duration,
};

use anyhow::{Context, Result};
use osp_proto::BlobHash;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tracing::debug;

use crate::Client;

/// Longest request head accepted.
const MAX_HEAD: usize = 8 * 1024;
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct MediaServer {
    port: u16,
    token: String,
    task: JoinHandle<()>,
}

impl MediaServer {
    /// Holds the client weakly, so the server never keeps it alive.
    pub async fn start(client: Weak<Client>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("starting the media server")?;
        let port = listener.local_addr()?.port();
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no random numbers: {e}"))?;
        let token = data_encoding::HEXLOWER.encode(&bytes);
        let task = tokio::spawn(serve(listener, client, token.clone()));
        Ok(Self { port, token, task })
    }

    pub fn url(&self, blob: &BlobHash, media_type: &str) -> String {
        let ext = match media_type {
            "video/mp4" => "mp4",
            "image/webp" => "webp",
            _ => "bin",
        };
        format!("http://127.0.0.1:{}/{}/{blob}.{ext}", self.port, self.token)
    }
}

impl Drop for MediaServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(listener: TcpListener, client: Weak<Client>, token: String) {
    let token = Arc::new(token);
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let (client, token) = (client.clone(), token.clone());
        tokio::spawn(async move {
            if let Err(e) = handle(stream, &client, &token).await {
                debug!("media request failed: {e:#}");
            }
        });
    }
}

/// What a request asked for, once its head is parsed.
#[derive(Debug, PartialEq, Eq)]
struct MediaRequest {
    head_only: bool,
    blob: BlobHash,
    content_type: &'static str,
    /// First byte and, optionally, last byte (inclusive), from `Range`.
    range: Option<(u64, Option<u64>)>,
}

async fn handle(stream: TcpStream, client: &Weak<Client>, token: &str) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader).take(MAX_HEAD as u64);
    let head = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut reader))
        .await
        .context("request head timed out")??;
    let req = match parse(&head, token) {
        Ok(req) => req,
        Err(status) => return respond_error(&mut writer, status).await,
    };
    let Some(client) = client.upgrade() else {
        return respond_error(&mut writer, "503 Service Unavailable").await;
    };
    // Without the head's size limit, so it's only the player hanging up
    // that ends a read.
    let mut reader = reader.into_inner();

    let (offset, len) = match req.range {
        _ if req.head_only => (0, Some(0)),
        None => (0, None),
        Some((first, None)) => (first, None),
        Some((first, Some(last))) => (first, Some(last.saturating_sub(first) + 1)),
    };
    // The node may be waiting for bytes still on their way to it. A player
    // that seeks elsewhere meanwhile hangs up, and the node can stop.
    let found = tokio::select! {
        found = client.read_blob(&req.blob, offset, len) => found,
        () = hung_up(&mut reader) => return Ok(()),
    };
    let (size, body) = match found {
        Ok(found) => found,
        Err(e) => {
            debug!("home node couldn't serve {}: {e:#}", req.blob.fmt_short());
            return respond_error(&mut writer, "502 Bad Gateway").await;
        }
    };

    let mut headers = format!(
        "Content-Type: {}\r\nAccept-Ranges: bytes\r\n\
         Cache-Control: private, max-age=31536000, immutable\r\nConnection: close\r\n",
        req.content_type
    );
    let (status, body_len) = match req.range {
        Some((first, last)) if !req.head_only => {
            if first >= size {
                headers.push_str(&format!("Content-Range: bytes */{size}\r\n"));
                let resp = format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\n{headers}Content-Length: 0\r\n\r\n"
                );
                return Ok(writer.write_all(resp.as_bytes()).await?);
            }
            let last = last.unwrap_or(u64::MAX).min(size - 1);
            headers.push_str(&format!("Content-Range: bytes {first}-{last}/{size}\r\n"));
            ("206 Partial Content", last - first + 1)
        }
        _ => ("200 OK", size),
    };
    let resp = format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {body_len}\r\n\r\n");
    writer.write_all(resp.as_bytes()).await?;
    if !req.head_only {
        // Stops early if the player hangs up (a seek, say); dropping the
        // stream then tells the node to stop sending.
        let mut body = body.take(body_len);
        tokio::select! {
            copied = tokio::io::copy(&mut body, &mut writer) => {
                copied?;
            }
            () = hung_up(&mut reader) => return Ok(()),
        }
    }
    writer.shutdown().await?;
    Ok(())
}

/// Returns once the player closes the connection. Players send nothing
/// after their request (one per connection), so anything else is ignored.
async fn hung_up<R: AsyncRead + Unpin>(reader: &mut R) {
    let mut buf = [0u8; 256];
    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            return;
        }
    }
}

/// Reads up to and including the blank line that ends the request head.
async fn read_head<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> Result<String> {
    let mut head = String::new();
    loop {
        let n = reader.read_line(&mut head).await?;
        anyhow::ensure!(n > 0, "connection closed before the request was complete");
        if head.ends_with("\r\n\r\n") || head.ends_with("\n\n") {
            return Ok(head);
        }
    }
}

/// Parses a request head. On failure, returns the HTTP status to send.
fn parse(head: &str, token: &str) -> Result<MediaRequest, &'static str> {
    let mut lines = head.lines();
    let mut request_line = lines.next().unwrap_or("").split(' ');
    let method = request_line.next().unwrap_or("");
    let target = request_line.next().unwrap_or("");
    let head_only = match method {
        "GET" => false,
        "HEAD" => true,
        _ => return Err("405 Method Not Allowed"),
    };

    // /<token>/<blob hash>.<ext>
    let mut parts = target.trim_start_matches('/').splitn(2, '/');
    let given = parts.next().unwrap_or("");
    // Compare hashes, which takes the same time however much of the token matches.
    if blake3::hash(given.as_bytes()) != blake3::hash(token.as_bytes()) {
        return Err("403 Forbidden");
    }
    let file = parts.next().unwrap_or("");
    let (hash, ext) = file.split_once('.').unwrap_or((file, ""));
    let blob: BlobHash = hash.parse().map_err(|_| "404 Not Found")?;
    let content_type = match ext {
        "mp4" => "video/mp4",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };

    let range = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("range"))
        .and_then(|(_, value)| parse_range(value.trim()));
    Ok(MediaRequest {
        head_only,
        blob,
        content_type,
        range,
    })
}

/// `bytes=FIRST-` or `bytes=FIRST-LAST`. Anything else (suffix ranges,
/// several ranges) is ignored, and the whole blob is sent, as HTTP allows.
fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let spec = value.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (first, last) = spec.split_once('-')?;
    let first: u64 = first.trim().parse().ok()?;
    let last = match last.trim() {
        "" => None,
        s => Some(s.parse::<u64>().ok().filter(|l| *l >= first)?),
    };
    Some((first, last))
}

async fn respond_error<W: AsyncWriteExt + Unpin>(writer: &mut W, status: &str) -> Result<()> {
    let resp = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    writer.write_all(resp.as_bytes()).await?;
    writer.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn blob() -> BlobHash {
        BlobHash::from_bytes([9; 32])
    }

    #[test]
    fn parses_player_requests() {
        let head = format!(
            "GET /{TOKEN}/{}.mp4 HTTP/1.1\r\nHost: 127.0.0.1\r\nrange: bytes=100-\r\n\r\n",
            blob()
        );
        let req = parse(&head, TOKEN).unwrap();
        assert_eq!(
            req,
            MediaRequest {
                head_only: false,
                blob: blob(),
                content_type: "video/mp4",
                range: Some((100, None)),
            }
        );
        let head = format!("HEAD /{TOKEN}/{}.webp HTTP/1.1\r\n\r\n", blob());
        let req = parse(&head, TOKEN).unwrap();
        assert!(req.head_only);
        assert_eq!(req.content_type, "image/webp");
        assert_eq!(req.range, None);
    }

    #[test]
    fn refuses_wrong_token_method_or_blob() {
        let wrong = format!("GET /nottoken/{}.mp4 HTTP/1.1\r\n\r\n", blob());
        assert_eq!(parse(&wrong, TOKEN), Err("403 Forbidden"));
        let no_token = format!("GET /{}.mp4 HTTP/1.1\r\n\r\n", blob());
        assert_eq!(parse(&no_token, TOKEN), Err("403 Forbidden"));
        let post = format!("POST /{TOKEN}/{}.mp4 HTTP/1.1\r\n\r\n", blob());
        assert_eq!(parse(&post, TOKEN), Err("405 Method Not Allowed"));
        let bad_blob = format!("GET /{TOKEN}/../etc/passwd HTTP/1.1\r\n\r\n");
        assert_eq!(parse(&bad_blob, TOKEN), Err("404 Not Found"));
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-"), Some((0, None)));
        assert_eq!(parse_range("bytes=10-19"), Some((10, Some(19))));
        assert_eq!(parse_range("bytes=-500"), None);
        assert_eq!(parse_range("bytes=0-1,5-9"), None);
        assert_eq!(parse_range("bytes=20-10"), None);
        assert_eq!(parse_range("items=0-1"), None);
    }
}
