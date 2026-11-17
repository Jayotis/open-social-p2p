//! One small HTTPS GET through Tor's SOCKS port, for the signed lists a
//! node follows (ADR 0042 §6). The site sees a Tor exit, never the node.
//!
//! HTTP/1.0, so the answer is the body as it is, up to the connection's
//! close. No redirects, no cookies: anything but a 200 is an error.

use std::{io, net::SocketAddr, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{
    TlsConnector,
    rustls::{ClientConfig, RootCertStore, pki_types::ServerName},
};

use crate::socks;

/// Fetches `url`, which must be `https://host/path`, through the SOCKS proxy
/// at `proxy`, with `isolation` as the circuit's name (see
/// [`socks::connect`]). Fails past `max` bytes of body.
pub async fn https_get(
    proxy: SocketAddr,
    isolation: &str,
    url: &str,
    max: usize,
) -> Result<Vec<u8>> {
    let (host, path) = split_url(url)?;
    let stream = socks::connect(proxy, isolation, host, 443)
        .await
        .with_context(|| format!("reaching {host} through Tor"))?;
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = ServerName::try_from(host.to_string()).context("bad host name")?;
    let mut tls = TlsConnector::from(Arc::new(config))
        .connect(name, stream)
        .await
        .with_context(|| format!("TLS with {host}"))?;
    let request = format!(
        "GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: osp\r\nAccept: application/json\r\n\r\n"
    );
    tls.write_all(request.as_bytes()).await?;
    tls.flush().await?;

    let mut answer = Vec::new();
    let mut buf = [0; 16 * 1024];
    loop {
        match tls.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => answer.extend_from_slice(&buf[..n]),
            // Many servers close without TLS's goodbye; what came is whole
            // if its length says so, which `body` checks.
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e).context("reading the answer"),
        }
        ensure!(answer.len() <= max + HEADER_ROOM, "the answer is too large");
    }
    let body = body(&answer)?;
    ensure!(body.len() <= max, "the answer is too large");
    Ok(body.to_vec())
}

/// Room for the status line and headers, beyond the body's limit.
const HEADER_ROOM: usize = 32 * 1024;

fn split_url(url: &str) -> Result<(&str, &str)> {
    let rest = url
        .strip_prefix("https://")
        .context("only https:// addresses")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    ensure!(
        !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'),
        "bad host in {url}"
    );
    ensure!(
        path.bytes().all(|b| b.is_ascii_graphic()),
        "bad path in {url}"
    );
    Ok((host, path))
}

/// The body of a 200 answer, checked against its `Content-Length` if it
/// gives one.
fn body(answer: &[u8]) -> Result<&[u8]> {
    let end = answer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("no end to the headers")?;
    let head = std::str::from_utf8(&answer[..end]).context("headers aren't text")?;
    let body = &answer[end + 4..];
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default();
    let code = status.split(' ').nth(1).unwrap_or_default();
    if code != "200" {
        bail!("the site answered {status:?}");
    }
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "transfer-encoding" && !value.eq_ignore_ascii_case("identity") {
            bail!("the site sent {value} encoding to an HTTP/1.0 request");
        }
        if name == "content-length" {
            let len: usize = value.parse().context("bad Content-Length")?;
            ensure!(body.len() == len, "the answer was cut short");
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Over a real Tor: `OSP_TEST_SOCKS=127.0.0.1:<port> cargo test -p
    /// osp-tor -- --ignored real_tor`, with `OSP_TEST_URL` to fetch.
    #[tokio::test]
    #[ignore = "needs a running Tor"]
    async fn fetches_over_real_tor() {
        let socks = std::env::var("OSP_TEST_SOCKS").unwrap().parse().unwrap();
        let url =
            std::env::var("OSP_TEST_URL").unwrap_or_else(|_| "https://osp2p.com/terms/".into());
        let body = https_get(socks, "osp-test", &url, MAX_TEST_BODY)
            .await
            .unwrap();
        println!("{} bytes from {url}", body.len());
        assert!(!body.is_empty());
    }

    const MAX_TEST_BODY: usize = 1024 * 1024;

    #[test]
    fn urls_split_into_host_and_path() {
        assert_eq!(
            split_url("https://osp2p.com/lists/hide.json").unwrap(),
            ("osp2p.com", "/lists/hide.json")
        );
        assert_eq!(split_url("https://osp2p.com").unwrap(), ("osp2p.com", "/"));
        assert!(split_url("http://osp2p.com/x").is_err());
        assert!(split_url("https://user@osp2p.com/x").is_err());
        assert!(split_url("https://osp2p.com/a b").is_err());
    }

    #[test]
    fn only_a_whole_200_is_taken() {
        let ok =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(body(ok).unwrap(), b"{}");
        let no_length = b"HTTP/1.0 200 OK\r\n\r\n{\"a\":1}";
        assert_eq!(body(no_length).unwrap(), b"{\"a\":1}");
        let short = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{}";
        assert!(body(short).is_err());
        let moved = b"HTTP/1.1 301 Moved Permanently\r\nLocation: /x/\r\n\r\n";
        assert!(body(moved).is_err());
        let challenge = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n";
        assert!(body(challenge).is_err());
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n";
        assert!(body(chunked).is_err());
    }
}
