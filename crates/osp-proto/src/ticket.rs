//! Follow tickets: everything needed to follow someone, as one string or QR code.

use std::{fmt, str::FromStr};

use anyhow::{Context, Result};
use data_encoding::BASE32_NOPAD;
use iroh_base::{EndpointAddr, PublicKey};
use serde::{Deserialize, Serialize};

/// Uppercase base32 keeps QR codes in their compact alphanumeric mode.
const PREFIX: &str = "OSP1";

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

impl fmt::Display for FollowTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let json = serde_json::to_vec(self).map_err(|_| fmt::Error)?;
        write!(f, "{PREFIX}{}", BASE32_NOPAD.encode(&json))
    }
}

impl FromStr for FollowTicket {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim().to_ascii_uppercase();
        let body = s.strip_prefix(PREFIX).context("not an osp follow ticket")?;
        let json = BASE32_NOPAD
            .decode(body.as_bytes())
            .context("ticket is not valid base32")?;
        serde_json::from_slice(&json).context("ticket contents are malformed")
    }
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
        assert!(s.starts_with(PREFIX));
        assert_eq!(s.parse::<FollowTicket>().unwrap(), ticket);
        assert_eq!(s.to_lowercase().parse::<FollowTicket>().unwrap(), ticket);
    }
}
