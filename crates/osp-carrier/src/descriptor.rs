//! A carrier's descriptor: what a machine publishes so others can build
//! circuits through it, signed by its machine key.

use anyhow::{Context, Result, ensure};
use iroh_base::{EndpointId, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::{PROTOCOL, VERSION};

/// What the machine key signs, before the payload, so the signature means
/// nothing anywhere else.
const DOMAIN: &[u8] = b"osp/carrier/1 descriptor:";
/// Largest signed part of a descriptor, in bytes.
pub const MAX_DESCRIPTOR_BYTES: usize = 512;
/// Longest relay URL a descriptor may name.
const MAX_RELAY_URL: usize = 256;
/// Most things a descriptor may say its carrier offers, and the longest name.
const MAX_OFFERS: usize = 8;
const MAX_OFFER_LEN: usize = 32;
/// How far a reader's clock may be from the carrier's: a descriptor is taken
/// this long before it says it's valid, and this long after.
const SKEW_SECS: u64 = 5 * 60;

/// What a carrier says of itself. No person's identity, and no IP address
/// (the design note's "A carrier's descriptor").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierInfo {
    pub version: u8,
    /// The machine, under its machine name.
    pub machine: EndpointId,
    /// The endpoint it carries on: published nowhere, reached by this ID
    /// through `relay`.
    pub endpoint: EndpointId,
    pub relay: String,
    /// The X25519 key circuits are set up with, used for nothing else.
    pub circuit_key: [u8; 32],
    /// Unix seconds: when the circuit key may be used, from and until.
    pub valid_from: u64,
    pub valid_until: u64,
    /// What it offers, by name. A reader ignores names it doesn't know.
    pub offers: Vec<String>,
}

/// A [`CarrierInfo`] signed by its machine key. The signature covers these
/// exact bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    payload: Vec<u8>,
    sig: Vec<u8>,
}

impl Descriptor {
    /// Signs `info` with `key`, which must be `info.machine`'s.
    pub fn new(key: &SecretKey, info: &CarrierInfo) -> Result<Self> {
        ensure!(
            key.public() == info.machine,
            "a descriptor is signed by its own machine"
        );
        check_shape(info)?;
        let payload = postcard::to_stdvec(info)?;
        ensure!(
            payload.len() <= MAX_DESCRIPTOR_BYTES,
            "a descriptor is larger than {MAX_DESCRIPTOR_BYTES} bytes"
        );
        let sig = key.sign(&signing_input(&payload)).to_bytes().to_vec();
        Ok(Self { payload, sig })
    }

    /// Checks the size, the signature and the shape, and that the circuit
    /// key is valid at `now` (unix seconds), give or take a few minutes'
    /// clock skew. Whether it's the machine you meant to reach is for the
    /// caller to check.
    pub fn verify(&self, now: u64) -> Result<CarrierInfo> {
        ensure!(
            self.payload.len() <= MAX_DESCRIPTOR_BYTES,
            "a descriptor is larger than {MAX_DESCRIPTOR_BYTES} bytes"
        );
        let info: CarrierInfo =
            postcard::from_bytes(&self.payload).context("not a carrier descriptor")?;
        let sig: [u8; Signature::LENGTH] = self
            .sig
            .as_slice()
            .try_into()
            .context("signature must be 64 bytes")?;
        info.machine
            .verify(&signing_input(&self.payload), &Signature::from_bytes(&sig))
            .map_err(|_| anyhow::anyhow!("invalid signature on a carrier descriptor"))?;
        check_shape(&info)?;
        ensure!(
            now + SKEW_SECS >= info.valid_from && now <= info.valid_until + SKEW_SECS,
            "this carrier's circuit key isn't valid now"
        );
        Ok(info)
    }

    /// The bytes on the wire, as `EXTEND` carries them.
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("a descriptor always encodes")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_DESCRIPTOR_BYTES + Signature::LENGTH + 8,
            "a descriptor is too large"
        );
        postcard::from_bytes(bytes).context("not a carrier descriptor")
    }
}

fn check_shape(info: &CarrierInfo) -> Result<()> {
    ensure!(
        info.version == VERSION,
        "carrier descriptor version {} isn't known",
        info.version
    );
    ensure!(
        info.valid_from < info.valid_until,
        "a circuit key's validity must end after it starts"
    );
    ensure!(
        !info.relay.is_empty() && info.relay.len() <= MAX_RELAY_URL,
        "a descriptor names one relay URL of at most {MAX_RELAY_URL} bytes"
    );
    ensure!(
        info.offers.len() <= MAX_OFFERS && info.offers.iter().all(|o| o.len() <= MAX_OFFER_LEN),
        "a descriptor names at most {MAX_OFFERS} offers of {MAX_OFFER_LEN} bytes"
    );
    Ok(())
}

fn signing_input(payload: &[u8]) -> Vec<u8> {
    debug_assert!(DOMAIN.starts_with(PROTOCOL));
    [DOMAIN, payload].concat()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn key(seed: u8) -> SecretKey {
        SecretKey::from_bytes(&[seed; 32])
    }

    pub(crate) fn info(machine: &SecretKey) -> CarrierInfo {
        CarrierInfo {
            version: VERSION,
            machine: machine.public(),
            endpoint: key(99).public(),
            relay: "https://relay1.example/".into(),
            circuit_key: [7; 32],
            valid_from: 1_000,
            valid_until: 1_000 + 86_400,
            offers: vec![],
        }
    }

    #[test]
    fn signed_descriptor_verifies_while_valid() {
        let machine = key(1);
        let d = Descriptor::new(&machine, &info(&machine)).unwrap();
        let back = Descriptor::from_bytes(&d.to_bytes()).unwrap();
        assert_eq!(back.verify(2_000).unwrap(), info(&machine));
        // A few minutes' skew either side is taken, more isn't.
        assert!(back.verify(1_000 - 60).is_ok());
        assert!(back.verify(1_000 - SKEW_SECS - 1).is_err());
        assert!(back.verify(1_000 + 86_400 + 60).is_ok());
        assert!(back.verify(1_000 + 86_400 + SKEW_SECS + 1).is_err());
    }

    #[test]
    fn only_its_own_machine_signs_it() {
        let machine = key(1);
        assert!(Descriptor::new(&key(2), &info(&machine)).is_err());
    }

    #[test]
    fn a_changed_byte_fails() {
        let machine = key(1);
        let d = Descriptor::new(&machine, &info(&machine)).unwrap();
        for i in 0..d.payload.len() {
            let mut bad = d.clone();
            bad.payload[i] ^= 1;
            assert!(bad.verify(2_000).is_err(), "byte {i}");
        }
        let mut bad = d;
        bad.sig[0] ^= 1;
        assert!(bad.verify(2_000).is_err());
    }

    #[test]
    fn shape_is_checked() {
        let machine = key(1);
        let mut i = info(&machine);
        i.version = 2;
        assert!(Descriptor::new(&machine, &i).is_err());
        let mut i = info(&machine);
        i.valid_until = i.valid_from;
        assert!(Descriptor::new(&machine, &i).is_err());
        let mut i = info(&machine);
        i.relay = "x".repeat(MAX_RELAY_URL + 1);
        assert!(Descriptor::new(&machine, &i).is_err());
        let mut i = info(&machine);
        i.offers = vec!["seed".into(); MAX_OFFERS + 1];
        assert!(Descriptor::new(&machine, &i).is_err());
    }
}
