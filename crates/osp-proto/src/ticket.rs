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
        let bytes = postcard::to_stdvec(self).map_err(|_| fmt::Error)?;
        write!(f, "{PAIRING_PREFIX}{}", BASE32_NOPAD.encode(&bytes))
    }
}

impl FromStr for PairingTicket {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let bytes = decode(PAIRING_PREFIX, s).context("not an osp pairing ticket")?;
        postcard::from_bytes(&bytes).context("ticket contents are malformed")
    }
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
    }
}
