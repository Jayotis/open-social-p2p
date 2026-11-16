//! Connecting through Tor's SOCKS port (SOCKS 5, RFC 1928) to an onion
//! address, by name: Tor finds the onion service, so nothing is looked up
//! outside Tor.

use std::{io, net::SocketAddr};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

/// Opens a stream to `host`:`port` through the SOCKS proxy at `proxy`.
///
/// `isolation` goes as the SOCKS user name. Tor keeps streams with
/// different names on different circuits (`IsolateSOCKSAuth`, on by
/// default), so an onion service, which sees which circuit each stream
/// came in on, can't tell that two keys' streams came from one Tor.
pub(crate) async fn connect(
    proxy: SocketAddr,
    isolation: &str,
    host: &str,
    port: u16,
) -> io::Result<TcpStream> {
    let user = isolation.as_bytes();
    let user_len = u8::try_from(user.len())
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad isolation name"))?;
    let mut stream = TcpStream::connect(proxy).await?;
    // Version 5, one method: user name and password (RFC 1929).
    stream.write_all(&[5, 1, 2]).await?;
    let mut chosen = [0; 2];
    stream.read_exact(&mut chosen).await?;
    if chosen != [5, 2] {
        return Err(io::Error::other("Tor's SOCKS port refused to talk"));
    }
    // Tor takes any name and password; the password says nothing.
    let mut auth = vec![1, user_len];
    auth.extend_from_slice(user);
    auth.extend_from_slice(&[1, b'-']);
    stream.write_all(&auth).await?;
    let mut status = [0; 2];
    stream.read_exact(&mut status).await?;
    if status[1] != 0 {
        return Err(io::Error::other(
            "Tor's SOCKS port refused the isolation name",
        ));
    }
    let name = host.as_bytes();
    let len = u8::try_from(name.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "host name too long"))?;
    // Connect, to a host by name (address type 3).
    let mut request = vec![5, 1, 0, 3, len];
    request.extend_from_slice(name);
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;
    let mut reply = [0; 4];
    stream.read_exact(&mut reply).await?;
    if reply[0] != 5 {
        return Err(io::Error::other("Tor's SOCKS port answered oddly"));
    }
    if reply[1] != 0 {
        return Err(io::Error::other(format!("Tor: {}", failure(reply[1]))));
    }
    // Then the address Tor connected from, which says nothing here.
    let addr_len = match reply[3] {
        1 => 4,
        4 => 16,
        3 => stream.read_u8().await? as usize,
        _ => return Err(io::Error::other("Tor's SOCKS port answered oddly")),
    };
    let mut skip = vec![0; addr_len + 2];
    stream.read_exact(&mut skip).await?;
    Ok(stream)
}

/// What a SOCKS reply's code means, with Tor's own codes for onion
/// services (`ExtendedErrors` on the SOCKS port).
fn failure(code: u8) -> &'static str {
    match code {
        1 => "couldn't connect",
        2 => "not allowed by Tor's rules",
        3 => "network unreachable",
        4 => "host unreachable",
        5 => "connection refused",
        6 => "timed out",
        7 | 8 => "a request Tor doesn't take",
        0xf0 => "the onion service isn't up, or hasn't published its address yet",
        0xf1 => "the onion service published a broken address",
        0xf2 => "the onion service's introduction failed",
        0xf3 => "the onion service's rendezvous failed",
        0xf4 | 0xf5 => "the onion service asks for client authorization",
        0xf6 => "not an onion address",
        0xf7 => "the onion service took too long to answer",
        _ => "SOCKS failure",
    }
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;

    /// A SOCKS proxy that answers one request with `code`, and then echoes.
    /// Its task returns the user name, then the host and port asked for.
    async fn proxy(code: u8) -> (SocketAddr, tokio::task::JoinHandle<(Vec<u8>, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut hello = [0; 3];
            s.read_exact(&mut hello).await.unwrap();
            assert_eq!(hello, [5, 1, 2], "offers only user name and password");
            s.write_all(&[5, 2]).await.unwrap();
            let mut head = [0; 2];
            s.read_exact(&mut head).await.unwrap();
            let mut user = vec![0; head[1] as usize];
            s.read_exact(&mut user).await.unwrap();
            let mut pass_len = [0; 1];
            s.read_exact(&mut pass_len).await.unwrap();
            let mut pass = vec![0; pass_len[0] as usize];
            s.read_exact(&mut pass).await.unwrap();
            s.write_all(&[1, 0]).await.unwrap();
            let mut head = [0; 5];
            s.read_exact(&mut head).await.unwrap();
            let mut rest = vec![0; head[4] as usize + 2];
            s.read_exact(&mut rest).await.unwrap();
            // A domain-name bound address, the longest kind to skip.
            s.write_all(&[5, code, 0, 3, 4, b'a', b'b', b'c', b'd', 0, 80])
                .await
                .unwrap();
            if code == 0 {
                let mut buf = [0; 5];
                s.read_exact(&mut buf).await.unwrap();
                s.write_all(&buf).await.unwrap();
            }
            (user, rest)
        });
        (addr, task)
    }

    #[tokio::test]
    async fn connects_by_name_under_its_isolation_name() {
        let (addr, task) = proxy(0).await;
        let mut s = connect(addr, "key-1", "example.onion", 677).await.unwrap();
        s.write_all(b"hello").await.unwrap();
        let mut back = [0; 5];
        s.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"hello");
        let (user, rest) = task.await.unwrap();
        assert_eq!(user, b"key-1");
        assert_eq!(&rest[..13], b"example.onion");
        assert_eq!(&rest[13..], &677u16.to_be_bytes());
    }

    #[tokio::test]
    async fn says_why_an_onion_service_wasnt_reached() {
        let (addr, _task) = proxy(0xf0).await;
        let err = connect(addr, "key-1", "example.onion", 677)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("isn't up"), "{err}");
    }
}
