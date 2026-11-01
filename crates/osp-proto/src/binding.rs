//! Bindings (ADR 0014): a node's media and direct endpoints have keys of their
//! own, and a binding ties one of them to the node's public ID, which the
//! identity's profile lists. It can be passed on by a third node and still be
//! checked.
//!
//! Both keys sign it. The public key alone would stop a node claiming that
//! another's public ID uses its endpoint, but not a node claiming another's
//! endpoint as its own, which could send a peer to a node it never meant to
//! show its address to.

use anyhow::{Context, Result, ensure};
use iroh_base::{EndpointAddr, EndpointId, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::object::base64url;

const BINDING_DOMAIN: &[u8] = b"osp-binding-v0:";

/// What an endpoint other than the public one is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    /// Relays only, no IP sockets: serves media to anyone.
    Media,
    /// IP and relays: serves media and the phones' protocol, to the node's
    /// own phones and to peers its rule allows.
    Direct,
}

/// "This endpoint serves for this public node, from this date."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointBinding {
    pub public: EndpointId,
    pub endpoint: EndpointId,
    pub kind: EndpointKind,
    /// Unix milliseconds. Of two bindings for a node's endpoint of one kind,
    /// the later one holds.
    pub since_ms: u64,
    #[serde(with = "base64url")]
    public_sig: Vec<u8>,
    #[serde(with = "base64url")]
    endpoint_sig: Vec<u8>,
}

impl EndpointBinding {
    pub fn new(
        public: &SecretKey,
        endpoint: &SecretKey,
        kind: EndpointKind,
        since_ms: u64,
    ) -> Self {
        let input = signing_input(&public.public(), &endpoint.public(), kind, since_ms);
        Self {
            public: public.public(),
            endpoint: endpoint.public(),
            kind,
            since_ms,
            public_sig: public.sign(&input).to_bytes().to_vec(),
            endpoint_sig: endpoint.sign(&input).to_bytes().to_vec(),
        }
    }

    /// Checks both signatures. Whether `public` is the node you expected is
    /// for the caller to check, against the profile that lists it.
    pub fn verify(&self) -> Result<()> {
        ensure!(
            self.public != self.endpoint,
            "a node's public endpoint can't be bound to itself"
        );
        let input = signing_input(&self.public, &self.endpoint, self.kind, self.since_ms);
        for (key, sig) in [
            (&self.public, &self.public_sig),
            (&self.endpoint, &self.endpoint_sig),
        ] {
            let sig: [u8; Signature::LENGTH] = sig
                .as_slice()
                .try_into()
                .context("signature must be 64 bytes")?;
            key.verify(&input, &Signature::from_bytes(&sig))
                .map_err(|_| anyhow::anyhow!("invalid signature on an endpoint binding"))?;
        }
        Ok(())
    }
}

fn signing_input(
    public: &EndpointId,
    endpoint: &EndpointId,
    kind: EndpointKind,
    since_ms: u64,
) -> Vec<u8> {
    let kind: u8 = match kind {
        EndpointKind::Media => 0,
        EndpointKind::Direct => 1,
    };
    [
        BINDING_DOMAIN,
        public.as_bytes(),
        endpoint.as_bytes(),
        &[kind],
        &since_ms.to_be_bytes(),
    ]
    .concat()
}

/// An endpoint as one node hands it to another: its binding, and how to reach
/// it, which isn't signed as a relay can change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundEndpoint {
    pub binding: EndpointBinding,
    pub addr: EndpointAddr,
}

impl BoundEndpoint {
    /// Checks that this is a valid endpoint of `kind` for the node `public`.
    pub fn check(&self, public: &EndpointId, kind: EndpointKind) -> Result<()> {
        self.binding.verify()?;
        ensure!(
            self.binding.public == *public,
            "this endpoint is bound to another node"
        );
        ensure!(
            self.binding.kind == kind,
            "this endpoint is of another kind"
        );
        ensure!(
            self.addr.id == self.binding.endpoint,
            "this address is another endpoint's"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> SecretKey {
        SecretKey::from_bytes(&[byte; 32])
    }

    fn binding() -> EndpointBinding {
        EndpointBinding::new(&key(1), &key(2), EndpointKind::Direct, 1_000)
    }

    #[test]
    fn a_binding_checks_out_after_travelling() {
        let json = serde_json::to_string(&binding()).unwrap();
        let back: EndpointBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(back, binding());
        back.verify().unwrap();
    }

    #[test]
    fn changing_any_part_breaks_it() {
        let changed = [
            EndpointBinding {
                public: key(3).public(),
                ..binding()
            },
            EndpointBinding {
                endpoint: key(3).public(),
                ..binding()
            },
            EndpointBinding {
                kind: EndpointKind::Media,
                ..binding()
            },
            EndpointBinding {
                since_ms: 1_001,
                ..binding()
            },
        ];
        for binding in changed {
            assert!(binding.verify().is_err(), "{binding:?}");
        }
    }

    /// The reason for the second signature: node 3 signs a claim that node
    /// 2's endpoint is its own, and has only its own key to sign with.
    #[test]
    fn a_node_cant_claim_another_nodes_endpoint() {
        let input = signing_input(
            &key(3).public(),
            &key(2).public(),
            EndpointKind::Direct,
            1_000,
        );
        let claim = EndpointBinding {
            public: key(3).public(),
            endpoint: key(2).public(),
            kind: EndpointKind::Direct,
            since_ms: 1_000,
            public_sig: key(3).sign(&input).to_bytes().to_vec(),
            endpoint_sig: key(3).sign(&input).to_bytes().to_vec(),
        };
        assert!(claim.verify().is_err());
    }

    /// And for the first: node 3 claims that node 1's public ID uses node 3's
    /// endpoint, and has only its own key to sign with.
    #[test]
    fn a_node_cant_claim_another_nodes_public_id() {
        let input = signing_input(
            &key(1).public(),
            &key(3).public(),
            EndpointKind::Direct,
            1_000,
        );
        let claim = EndpointBinding {
            public: key(1).public(),
            endpoint: key(3).public(),
            kind: EndpointKind::Direct,
            since_ms: 1_000,
            public_sig: key(3).sign(&input).to_bytes().to_vec(),
            endpoint_sig: key(3).sign(&input).to_bytes().to_vec(),
        };
        assert!(claim.verify().is_err());
    }

    #[test]
    fn a_signature_of_the_wrong_length_is_refused() {
        let short = EndpointBinding {
            public_sig: vec![0; 10],
            ..binding()
        };
        assert!(short.verify().is_err());
    }

    #[test]
    fn a_node_cant_be_bound_to_itself() {
        let own = EndpointBinding::new(&key(1), &key(1), EndpointKind::Media, 1_000);
        assert!(own.verify().is_err());
    }

    #[test]
    fn an_endpoint_is_checked_against_the_node_and_kind_expected() {
        let bound = BoundEndpoint {
            binding: binding(),
            addr: EndpointAddr::new(key(2).public()),
        };
        let node = key(1).public();
        bound.check(&node, EndpointKind::Direct).unwrap();
        assert!(bound.check(&key(3).public(), EndpointKind::Direct).is_err());
        assert!(bound.check(&node, EndpointKind::Media).is_err());
        let other_addr = BoundEndpoint {
            addr: EndpointAddr::new(key(3).public()),
            ..bound
        };
        assert!(other_addr.check(&node, EndpointKind::Direct).is_err());
    }
}
