//! A node's onion address and onion service key follow from its iroh key,
//! as both are ed25519 keys (Tor's rend-spec-v3, "Encoding onion
//! addresses"). So a node needs nothing but another node's ID to reach it.

use data_encoding::{BASE32_NOPAD, BASE64};
use iroh::{EndpointId, SecretKey};
use sha3::{Digest, Sha3_256};

const VERSION: u8 = 3;

/// The 56 characters before `.onion` in a node's onion address: its key, a
/// checksum and the version, in lower-case base32.
pub fn service_id(node: &EndpointId) -> String {
    let key = node.as_bytes();
    let mut checksum = Sha3_256::new();
    checksum.update(b".onion checksum");
    checksum.update(key);
    checksum.update([VERSION]);
    let checksum = checksum.finalize();
    let mut bytes = Vec::with_capacity(35);
    bytes.extend_from_slice(key);
    bytes.extend_from_slice(&checksum[..2]);
    bytes.push(VERSION);
    BASE32_NOPAD.encode(&bytes).to_ascii_lowercase()
}

/// A node's onion address, as Tor's SOCKS port takes it.
pub fn host(node: &EndpointId) -> String {
    format!("{}.onion", service_id(node))
}

/// The node's key as Tor's `ADD_ONION` takes it: the expanded ed25519
/// secret key, the SHA-512 of the seed with its scalar clamped, in base64.
pub(crate) fn add_onion_key(key: &SecretKey) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA512, &key.to_bytes());
    let mut expanded: [u8; 64] = hash.as_ref().try_into().expect("SHA-512 is 64 bytes");
    expanded[0] &= 248;
    expanded[31] &= 63;
    expanded[31] |= 64;
    let blob = format!("ED25519-V3:{}", BASE64.encode(&expanded));
    expanded.fill(0);
    blob
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From Tor 0.4.9.15: `ADD_ONION` with the key below named its service
    /// with this ID.
    #[test]
    fn addresses_match_tors() {
        let key = SecretKey::from_bytes(&[7; 32]);
        assert_eq!(
            service_id(&key.public()),
            "5jfgyy7ctrjavpxvkb5rglwf7gkuo5vox27hxescd3vgsfcg2iwfmxqd"
        );
    }

    #[test]
    fn addresses_look_like_onion_addresses() {
        let node = SecretKey::generate().public();
        let host = host(&node);
        assert_eq!(host.len(), 62);
        assert!(host.ends_with(".onion"));
        let id = &host[..56];
        assert!(
            id.chars()
                .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c))
        );
        assert!(id.ends_with('d'), "version 3 ends the address in d");
    }

    #[test]
    fn the_onion_key_is_the_clamped_expanded_key() {
        let blob = add_onion_key(&SecretKey::from_bytes(&[7; 32]));
        let expanded = BASE64
            .decode(blob.strip_prefix("ED25519-V3:").unwrap().as_bytes())
            .unwrap();
        assert_eq!(expanded.len(), 64);
        assert_eq!(expanded[0] & 7, 0);
        assert_eq!(expanded[31] & 0xc0, 0x40);
    }
}
