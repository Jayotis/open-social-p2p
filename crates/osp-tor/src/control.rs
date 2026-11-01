//! A small client for Tor's control port (Tor's control-spec): only what
//! the node needs. It replaces `torut`, which brought two crates with
//! security advisories (ADR 0014).

use std::{net::SocketAddr, path::Path, time::Duration};

use anyhow::{Context, Result, anyhow, bail, ensure};
use data_encoding::HEXUPPER;
use iroh::SecretKey;
use n0_future::task::AbortOnDropHandle;
use ring::hmac;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpStream, tcp::OwnedWriteHalf},
    sync::mpsc,
};

use crate::onion;

/// Longest Tor may take to answer a command. It answers at once, unless
/// it's stuck.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest line read from the control port. Tor's are far shorter.
const MAX_LINE: usize = 64 * 1024;
/// SAFECOOKIE's keys (control-spec, "AUTHCHALLENGE").
const SERVER_TO_CONTROLLER: &[u8] = b"Tor safe cookie authentication server-to-controller hash";
const CONTROLLER_TO_SERVER: &[u8] = b"Tor safe cookie authentication controller-to-server hash";

/// One reply from Tor: its status code, and its lines without the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reply {
    pub code: u16,
    pub lines: Vec<String>,
}

/// A connection to Tor's control port. Replies to commands come back in
/// order; events (status 650) go to the receiver `connect` returns, which
/// ends when Tor closes the connection.
pub(crate) struct Control {
    writer: OwnedWriteHalf,
    replies: mpsc::Receiver<Reply>,
    _reader: AbortOnDropHandle<()>,
}

impl Control {
    pub async fn connect(addr: SocketAddr) -> Result<(Self, mpsc::Receiver<Reply>)> {
        let stream = TcpStream::connect(addr)
            .await
            .with_context(|| format!("connecting to Tor's control port at {addr}"))?;
        let (read, writer) = stream.into_split();
        let (replies_tx, replies) = mpsc::channel(16);
        let (events_tx, events) = mpsc::channel(64);
        let reader = tokio::spawn(async move {
            let mut read = BufReader::new(read);
            while let Ok(Some(reply)) = read_reply(&mut read).await {
                let to = if reply.code / 100 == 6 {
                    &events_tx
                } else {
                    &replies_tx
                };
                if to.send(reply).await.is_err() {
                    break;
                }
            }
        });
        let control = Self {
            writer,
            replies,
            _reader: AbortOnDropHandle::new(reader),
        };
        Ok((control, events))
    }

    /// Sends one command and returns Tor's reply, whatever its code. The
    /// command may hold a secret, so errors name only its first word.
    pub async fn send(&mut self, command: &str) -> Result<Reply> {
        ensure!(
            !command.contains(['\r', '\n']),
            "a control command is one line"
        );
        let verb = command.split(' ').next().unwrap_or_default();
        self.writer
            .write_all(format!("{command}\r\n").as_bytes())
            .await
            .context("Tor closed its control port")?;
        tokio::time::timeout(COMMAND_TIMEOUT, self.replies.recv())
            .await
            .map_err(|_| anyhow!("Tor didn't answer {verb}"))?
            .ok_or_else(|| anyhow!("Tor closed its control port"))
    }

    /// Sends one command, failing unless Tor accepts it.
    pub async fn command(&mut self, command: &str) -> Result<Vec<String>> {
        let reply = self.send(command).await?;
        if reply.code / 100 != 2 {
            let verb = command.split(' ').next().unwrap_or_default();
            bail!(
                "Tor refused {verb}: {} {}",
                reply.code,
                reply.lines.join(" ")
            );
        }
        Ok(reply.lines)
    }

    /// Proves to Tor that this program can read its cookie file, and checks
    /// that Tor can too (SAFECOOKIE), so a program pretending to be Tor on
    /// its port learns nothing. `cookie` is where the file must be; without
    /// it, wherever Tor says.
    pub async fn authenticate(&mut self, cookie: Option<&Path>) -> Result<()> {
        let info = self.command("PROTOCOLINFO 1").await?;
        let auth = info
            .iter()
            .find_map(|l| l.strip_prefix("AUTH "))
            .context("Tor didn't say how to authenticate")?;
        let methods = field(auth, "METHODS").unwrap_or_default();
        ensure!(
            methods.split(',').any(|m| m == "SAFECOOKIE"),
            "Tor's control port doesn't take cookie authentication (it offers {methods})"
        );
        let path = match cookie {
            Some(path) => path.to_path_buf(),
            None => field(auth, "COOKIEFILE")
                .context("Tor didn't name its cookie file")?
                .into(),
        };
        let cookie = std::fs::read(&path)
            .with_context(|| format!("reading Tor's cookie, {}", path.display()))?;
        ensure!(cookie.len() == 32, "Tor's cookie isn't 32 bytes");

        let mut client_nonce = [0; 32];
        getrandom::fill(&mut client_nonce).map_err(|e| anyhow!("no random numbers: {e}"))?;
        let lines = self
            .command(&format!(
                "AUTHCHALLENGE SAFECOOKIE {}",
                HEXUPPER.encode(&client_nonce)
            ))
            .await?;
        let challenge = lines.first().context("Tor sent no challenge")?;
        let server_hash = hex_field(challenge, "SERVERHASH")?;
        let server_nonce = hex_field(challenge, "SERVERNONCE")?;
        let mut msg = cookie;
        msg.extend_from_slice(&client_nonce);
        msg.extend_from_slice(&server_nonce);
        let server_key = hmac::Key::new(hmac::HMAC_SHA256, SERVER_TO_CONTROLLER);
        hmac::verify(&server_key, &msg, &server_hash)
            .map_err(|_| anyhow!("the program on Tor's control port doesn't know Tor's cookie"))?;
        let client_key = hmac::Key::new(hmac::HMAC_SHA256, CONTROLLER_TO_SERVER);
        let client_hash = hmac::sign(&client_key, &msg);
        msg.fill(0);
        self.command(&format!(
            "AUTHENTICATE {}",
            HEXUPPER.encode(client_hash.as_ref())
        ))
        .await?;
        Ok(())
    }

    /// Sets up an onion service for `key`, on this connection only: it ends
    /// when the connection does. Streams to its `port` go to `target`.
    pub async fn add_onion(
        &mut self,
        key: &SecretKey,
        port: u16,
        target: SocketAddr,
    ) -> Result<()> {
        let expected = onion::service_id(&key.public());
        let reply = self
            .send(&format!(
                "ADD_ONION {} Flags=DiscardPK Port={port},{target}",
                onion::add_onion_key(key)
            ))
            .await?;
        match reply.code {
            250 => {}
            552 => bail!(
                "Tor already has the onion service {expected}: is another node running with this key?"
            ),
            code => bail!(
                "Tor refused the onion service: {code} {}",
                reply.lines.join(" ")
            ),
        }
        let named = reply
            .lines
            .iter()
            .find_map(|l| l.strip_prefix("ServiceID="))
            .context("Tor didn't name the onion service")?;
        ensure!(
            named == expected,
            "Tor named the onion service {named}, not {expected} as the node's key gives"
        );
        Ok(())
    }

    pub async fn del_onion(&mut self, service_id: &str) -> Result<()> {
        self.command(&format!("DEL_ONION {service_id}")).await?;
        Ok(())
    }

    /// How far Tor has got connecting to the Tor network, from 0 to 100.
    pub async fn bootstrap_progress(&mut self) -> Result<u8> {
        let lines = self.command("GETINFO status/bootstrap-phase").await?;
        lines
            .iter()
            .find_map(|l| l.strip_prefix("status/bootstrap-phase="))
            .and_then(|l| field(l, "PROGRESS"))
            .and_then(|p| p.parse().ok())
            .context("Tor didn't say how far it has got")
    }

    /// Where Tor's SOCKS port listens.
    pub async fn socks_addr(&mut self) -> Result<SocketAddr> {
        let lines = self.command("GETINFO net/listeners/socks").await?;
        lines
            .iter()
            .find_map(|l| l.strip_prefix("net/listeners/socks="))
            .and_then(|l| {
                l.split(' ')
                    .find_map(|a| a.trim_matches('"').parse::<SocketAddr>().ok())
            })
            .context("Tor has no SOCKS port")
    }
}

/// Reads one whole reply: `250-` lines go on, `250+` starts a block of
/// lines ending in `.`, and `250 ` ends the reply. `None` at the end of the
/// connection.
async fn read_reply<R: AsyncBufReadExt + Unpin>(read: &mut R) -> Result<Option<Reply>> {
    let mut lines = Vec::new();
    loop {
        let Some(line) = read_line(read).await? else {
            return Ok(None);
        };
        ensure!(line.len() >= 4, "Tor sent a short line");
        let code: u16 = line[..3]
            .parse()
            .context("Tor sent a line without a code")?;
        let (sep, rest) = (line.as_bytes()[3], &line[4..]);
        match sep {
            b'-' => lines.push(rest.to_string()),
            b'+' => {
                let mut block = rest.to_string();
                loop {
                    let line = read_line(read)
                        .await?
                        .context("Tor closed the connection")?;
                    if line == "." {
                        break;
                    }
                    block.push('\n');
                    // A leading "." is doubled within a block.
                    block.push_str(line.strip_prefix('.').unwrap_or(&line));
                }
                lines.push(block);
            }
            b' ' => {
                lines.push(rest.to_string());
                return Ok(Some(Reply { code, lines }));
            }
            _ => bail!("Tor sent a line Tor wouldn't send"),
        }
    }
}

/// One line without its CRLF; `None` at the end of the connection.
async fn read_line<R: AsyncBufReadExt + Unpin>(read: &mut R) -> Result<Option<String>> {
    let mut buf = Vec::new();
    let n = (&mut *read)
        .take(MAX_LINE as u64)
        .read_until(b'\n', &mut buf)
        .await?;
    if n == 0 {
        return Ok(None);
    }
    ensure!(buf.ends_with(b"\n"), "Tor sent too long a line");
    buf.pop();
    if buf.ends_with(b"\r") {
        buf.pop();
    }
    Ok(Some(
        String::from_utf8(buf).context("Tor sent a line that isn't UTF-8")?,
    ))
}

/// The value of `KEY=value` or `KEY="quoted value"` among a line's fields.
fn field(line: &str, key: &str) -> Option<String> {
    let start = line
        .match_indices(&format!("{key}="))
        .find(|(i, _)| *i == 0 || line.as_bytes()[i - 1] == b' ')?
        .0
        + key.len()
        + 1;
    let rest = &line[start..];
    let Some(quoted) = rest.strip_prefix('"') else {
        return Some(rest.split(' ').next().unwrap_or_default().to_string());
    };
    let mut value = String::new();
    let mut chars = quoted.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(value),
            '\\' => match chars.next()? {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'r' => value.push('\r'),
                other => value.push(other),
            },
            c => value.push(c),
        }
    }
    None
}

fn hex_field(line: &str, key: &str) -> Result<[u8; 32]> {
    let hex = field(line, key).with_context(|| format!("Tor's challenge has no {key}"))?;
    HEXUPPER
        .decode(hex.to_ascii_uppercase().as_bytes())
        .ok()
        .and_then(|b| b.try_into().ok())
        .with_context(|| format!("Tor's {key} isn't 32 bytes of hex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn parse(text: &str) -> Vec<Reply> {
        let mut read = BufReader::new(text.as_bytes());
        let mut replies = Vec::new();
        while let Some(reply) = read_reply(&mut read).await.unwrap() {
            replies.push(reply);
        }
        replies
    }

    #[tokio::test]
    async fn reads_replies_of_every_shape() {
        let replies = parse(concat!(
            "250-PROTOCOLINFO 1\r\n",
            "250-AUTH METHODS=COOKIE,SAFECOOKIE COOKIEFILE=\"/home/a b/\\\"c\\\"\"\r\n",
            "250 OK\r\n",
            "250+onions/current=\r\n",
            "abc\r\n",
            "..dot\r\n",
            ".\r\n",
            "250 OK\r\n",
            "650 HS_DESC UPLOADED abc UNKNOWN x\r\n",
        ))
        .await;
        assert_eq!(replies.len(), 3);
        assert_eq!(
            replies[0].lines[1],
            "AUTH METHODS=COOKIE,SAFECOOKIE COOKIEFILE=\"/home/a b/\\\"c\\\"\""
        );
        assert_eq!(replies[1].lines[0], "onions/current=\nabc\n.dot");
        assert_eq!(replies[2].code, 650);
    }

    #[test]
    fn reads_fields_plain_and_quoted() {
        let auth = "METHODS=COOKIE,SAFECOOKIE COOKIEFILE=\"/home/a b/\\\"c\\\"\"";
        assert_eq!(field(auth, "METHODS").unwrap(), "COOKIE,SAFECOOKIE");
        assert_eq!(field(auth, "COOKIEFILE").unwrap(), "/home/a b/\"c\"");
        assert_eq!(field(auth, "FILE"), None, "only whole keys");
        let phase = "NOTICE BOOTSTRAP PROGRESS=85 TAG=ap_conn SUMMARY=\"Connecting\"";
        assert_eq!(field(phase, "PROGRESS").unwrap(), "85");
    }

    #[tokio::test]
    async fn refuses_long_lines() {
        let long = format!("250 {}\r\n", "x".repeat(MAX_LINE));
        let mut read = BufReader::new(long.as_bytes());
        assert!(read_reply(&mut read).await.is_err());
    }
}
