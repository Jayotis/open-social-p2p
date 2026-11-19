//! Tickets: everything needed to follow someone, or to pair a phone with its
//! home node, as one string or QR code.

use std::{fmt, str::FromStr};

use anyhow::{Context, Result};
use data_encoding::BASE32_NOPAD;
use iroh_base::{EndpointAddr, PublicKey};
use serde::{Deserialize, Serialize};

/// Uppercase base32 keeps QR codes in their compact alphanumeric mode.
const FOLLOW_PREFIX: &str = "OSP1";
const PAIRING_PREFIX: &str = "OSPPAIR1";
/// A pairing ticket that also carries the node's relays and their token
/// (ADR 0044). An older app doesn't read it, so a node gives one out only
/// when it has a token to pass on.
const PAIRING_RELAYS_PREFIX: &str = "OSPPAIR2";
/// Starts the pairing pass (`PairingTicket::pass`).
const PAIRING_PASS_PREFIX: &[u8] = b"osp-pair/";

/// An identity plus an address for one of its home nodes.
///
/// By default the address holds only the node ID and its relay URL, so a
/// ticket shared in public doesn't reveal a home IP address. Direct addresses
/// are included only when relays are disabled (tests, LAN-only setups).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowTicket {
    pub actor: PublicKey,
    pub node: EndpointAddr,
}

/// What a phone needs to pair with its home node (ADR 0008): the node's
/// address and a one-time secret.
///
/// Unlike a follow ticket, the address includes the node's addresses on
/// private networks, so a phone at home can connect without a relay. The
/// ticket is shown only to the node's owner, works once, and expires after a
/// few minutes. It is encoded in binary (postcard) rather than JSON, to keep
/// the QR code small enough for a terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingTicket {
    pub node: EndpointAddr,
    pub secret: [u8; 16],
    /// The node's own relays, for the phone to use as its own: they let in
    /// only those with the token (ADR 0044). `None` for a node without a
    /// token, whose ticket an older app can still read.
    pub relays: Option<PairingRelays>,
}

/// The relays a node passes on to its phone (ADR 0044).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingRelays {
    /// Relay URLs, as in the node's config.
    pub urls: Vec<String>,
    /// The token they ask for.
    pub token: String,
    /// Use n0's relays while none of `urls` lets the phone in, as the node
    /// does with `relay_fallback = "default"`.
    pub fallback: bool,
}

/// The tickets as encoded: an `OSPPAIR1` ticket has no relays.
#[derive(Serialize, Deserialize)]
struct PairingV1 {
    node: EndpointAddr,
    secret: [u8; 16],
}

#[derive(Serialize, Deserialize)]
struct PairingV2 {
    node: EndpointAddr,
    secret: [u8; 16],
    relays: PairingRelays,
}

impl PairingTicket {
    /// What a phone that is pairing offers among its ALPNs. A node's direct
    /// endpoint refuses nodes it doesn't know before the handshake (ADR 0014),
    /// when the first packet is all it has seen. This shows it the phone holds
    /// the ticket: the hash of the secret, as the node keeps it. A relay can
    /// read it, but a relay sees the node's address anyway. A node that doesn't
    /// expect it ignores an ALPN it doesn't know.
    pub fn pass(&self) -> Vec<u8> {
        let hash = blake3::hash(&self.secret);
        [PAIRING_PASS_PREFIX, hash.as_bytes()].concat()
    }
}

/// The hash of a pairing secret in a pass (`PairingTicket::pass`), if `alpn`
/// is one.
pub fn pairing_pass_hash(alpn: &[u8]) -> Option<blake3::Hash> {
    let hash: [u8; 32] = alpn.strip_prefix(PAIRING_PASS_PREFIX)?.try_into().ok()?;
    Some(blake3::Hash::from_bytes(hash))
}

impl fmt::Display for FollowTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let json = serde_json::to_vec(self).map_err(|_| fmt::Error)?;
        write!(f, "{FOLLOW_PREFIX}{}", BASE32_NOPAD.encode(&json))
    }
}

impl FromStr for FollowTicket {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let json = decode(FOLLOW_PREFIX, s).context("not an osp follow ticket")?;
        serde_json::from_slice(&json).context("ticket contents are malformed")
    }
}

impl fmt::Display for PairingTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (node, secret) = (self.node.clone(), self.secret);
        let (prefix, bytes) = match &self.relays {
            None => (
                PAIRING_PREFIX,
                postcard::to_stdvec(&PairingV1 { node, secret }),
            ),
            Some(relays) => {
                let relays = relays.clone();
                let v2 = PairingV2 {
                    node,
                    secret,
                    relays,
                };
                (PAIRING_RELAYS_PREFIX, postcard::to_stdvec(&v2))
            }
        };
        let bytes = bytes.map_err(|_| fmt::Error)?;
        write!(f, "{prefix}{}", BASE32_NOPAD.encode(&bytes))
    }
}

impl FromStr for PairingTicket {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let malformed = "ticket contents are malformed";
        if let Ok(bytes) = decode(PAIRING_RELAYS_PREFIX, s) {
            let PairingV2 {
                node,
                secret,
                relays,
            } = postcard::from_bytes(&bytes).context(malformed)?;
            return Ok(Self {
                node,
                secret,
                relays: Some(relays),
            });
        }
        let bytes = decode(PAIRING_PREFIX, s).context("not an osp pairing ticket")?;
        let PairingV1 { node, secret } = postcard::from_bytes(&bytes).context(malformed)?;
        Ok(Self {
            node,
            secret,
            relays: None,
        })
    }
}

/// Whether `s` looks like a pairing ticket, before it's decoded: for an app
/// telling a pairing code from a follow code.
pub fn is_pairing_ticket(s: &str) -> bool {
    let s = s.trim().to_ascii_uppercase();
    s.starts_with(PAIRING_PREFIX) || s.starts_with(PAIRING_RELAYS_PREFIX)
}

/// Strips the prefix and decodes the base32 body. Accepts lowercase too.
fn decode(prefix: &str, s: &str) -> Result<Vec<u8>> {
    let s = s.trim().to_ascii_uppercase();
    let body = s.strip_prefix(prefix).context("wrong prefix")?;
    BASE32_NOPAD
        .decode(body.as_bytes())
        .context("ticket is not valid base32")
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    #[test]
    fn round_trip() {
        let ticket = FollowTicket {
            actor: SecretKey::generate().public(),
            node: EndpointAddr::new(SecretKey::generate().public())
                .with_ip_addr("127.0.0.1:4433".parse().unwrap()),
        };
        let s = ticket.to_string();
        assert!(s.starts_with(FOLLOW_PREFIX));
        assert_eq!(s.parse::<FollowTicket>().unwrap(), ticket);
        assert_eq!(s.to_lowercase().parse::<FollowTicket>().unwrap(), ticket);
    }

    #[test]
    fn pairing_and_follow_tickets_are_not_confused() {
        let pairing = PairingTicket {
            node: EndpointAddr::new(SecretKey::generate().public())
                .with_relay_url("https://relay.example.org./".parse().unwrap())
                .with_ip_addr("172.16.5.20:4433".parse().unwrap())
                .with_ip_addr("[fd00::20]:4433".parse().unwrap()),
            secret: [7; 16],
            relays: None,
        };
        let s = pairing.to_string();
        assert!(s.starts_with(PAIRING_PREFIX));
        // Small enough for a QR code that fits in a terminal.
        assert!(s.len() < 250, "{} characters", s.len());
        assert_eq!(s.parse::<PairingTicket>().unwrap(), pairing);
        assert_eq!(s.to_lowercase().parse::<PairingTicket>().unwrap(), pairing);
        assert!(s.parse::<FollowTicket>().is_err());

        let follow = FollowTicket {
            actor: SecretKey::generate().public(),
            node: EndpointAddr::new(SecretKey::generate().public()),
        };
        assert!(follow.to_string().parse::<PairingTicket>().is_err());
        assert!(is_pairing_ticket(&s.to_lowercase()));
        assert!(!is_pairing_ticket(&follow.to_string()));
    }

    /// A node with a relay token gives out `OSPPAIR2`, which carries it
    /// (ADR 0044). The project's token is 32 characters.
    #[test]
    fn pairing_ticket_carries_relays_and_token() {
        let url = "https://relay1.example.com";
        let pairing = PairingTicket {
            node: EndpointAddr::new(SecretKey::generate().public())
                .with_relay_url(url.parse().unwrap())
                .with_ip_addr("172.16.5.20:4433".parse().unwrap())
                .with_ip_addr("[fd00::20]:4433".parse().unwrap()),
            secret: [7; 16],
            relays: Some(PairingRelays {
                urls: vec![url.into()],
                token: "t".repeat(32),
                fallback: true,
            }),
        };
        let s = pairing.to_string();
        assert!(s.starts_with(PAIRING_RELAYS_PREFIX));
        assert!(is_pairing_ticket(&s));
        // Still a QR code that fits in a terminal (version 10, 57 modules).
        assert!(s.len() < 390, "{} characters", s.len());
        assert_eq!(s.parse::<PairingTicket>().unwrap(), pairing);
        // The token's case survives the base32 body being uppercased.
        assert_eq!(s.to_lowercase().parse::<PairingTicket>().unwrap(), pairing);
    }
}
