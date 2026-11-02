//! Messages of the machine protocol `osp/machine/1` (ADR 0015): what nodes say
//! to each other under their machine names, which are tied to no person.
//!
//! As in `osp/social/1`, one QUIC bidirectional stream per request: the asking
//! machine sends one [`MachineRequest`] frame, and the other answers with one
//! [`MachineResponse`].

use anyhow::{Context, Result, ensure};
use iroh_base::{EndpointId, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::object::base64url;

const RECORD_DOMAIN: &[u8] = b"osp-machine-record-v0:";

/// Longest signed part of a machine record, in bytes.
pub const MAX_RECORD_BYTES: usize = 1024;
/// Most things a record can say its machine offers.
pub const MAX_OFFERS: usize = 8;
/// Largest request frame a machine reads. A request carries one record at most.
pub const MAX_REQUEST_LEN: u32 = 4 * 1024;
/// Largest reply frame a machine reads.
pub const MAX_REPLY_LEN: u32 = 16 * 1024;

/// What a machine says of itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineInfo {
    pub machine: EndpointId,
    /// What it offers other machines, by name. A reader ignores names it
    /// doesn't know, so new ones can be added.
    #[serde(default)]
    pub offers: Vec<String>,
    /// RFC 3339.
    pub published: String,
}

/// A machine's signed statement about itself. Only its own key signs it, so
/// nobody can make one up for a machine, and it ties the machine to nothing
/// else. The signature covers these exact bytes, so a later version can add to
/// them and older readers can still check them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineRecord {
    #[serde(with = "base64url")]
    payload: Vec<u8>,
    machine: EndpointId,
    #[serde(with = "base64url")]
    sig: Vec<u8>,
}

impl MachineRecord {
    pub fn new(key: &SecretKey, offers: Vec<String>, published: String) -> Result<Self> {
        let info = MachineInfo {
            machine: key.public(),
            offers,
            published,
        };
        let payload = serde_json::to_vec(&info)?;
        ensure!(
            payload.len() <= MAX_RECORD_BYTES,
            "a machine record is larger than {MAX_RECORD_BYTES} bytes"
        );
        let sig = key.sign(&signing_input(&payload)).to_bytes().to_vec();
        Ok(Self {
            payload,
            machine: key.public(),
            sig,
        })
    }

    /// Checks the size, the signature, and that it speaks of the key that
    /// signed it. Whether it's the machine you expected, and whether its date
    /// makes sense, is for the caller to check.
    pub fn verify(&self) -> Result<MachineInfo> {
        ensure!(
            self.payload.len() <= MAX_RECORD_BYTES,
            "a machine record is larger than {MAX_RECORD_BYTES} bytes"
        );
        let sig: [u8; Signature::LENGTH] = self
            .sig
            .as_slice()
            .try_into()
            .context("signature must be 64 bytes")?;
        self.machine
            .verify(&signing_input(&self.payload), &Signature::from_bytes(&sig))
            .map_err(|_| anyhow::anyhow!("invalid signature on a machine record"))?;
        let info: MachineInfo =
            serde_json::from_slice(&self.payload).context("not a machine record")?;
        ensure!(
            info.machine == self.machine,
            "this record speaks of another machine"
        );
        ensure!(
            info.offers.len() <= MAX_OFFERS,
            "a machine record can name at most {MAX_OFFERS} offers"
        );
        Ok(info)
    }
}

fn signing_input(payload: &[u8]) -> Vec<u8> {
    [RECORD_DOMAIN, payload].concat()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MachineRequest {
    /// Here's my record; what's yours? It must be the record of the machine
    /// asking.
    Hello { record: MachineRecord },
    /// A request from a later version. It gets an error rather than a closed
    /// connection, so requests can be added.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum MachineResponse {
    Hello { record: MachineRecord },
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATE: &str = "2026-11-01T00:00:00Z";

    fn key(byte: u8) -> SecretKey {
        SecretKey::from_bytes(&[byte; 32])
    }

    fn record() -> MachineRecord {
        MachineRecord::new(&key(1), vec!["directory".into()], DATE.into()).unwrap()
    }

    #[test]
    fn a_record_checks_out_after_travelling() {
        let json = serde_json::to_string(&record()).unwrap();
        let back: MachineRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, record());
        let info = back.verify().unwrap();
        assert_eq!(info.machine, key(1).public());
        assert_eq!(info.offers, ["directory"]);
        assert_eq!(info.published, DATE);
    }

    #[test]
    fn changing_a_record_breaks_it() {
        let other = MachineRecord::new(&key(1), vec![], DATE.into()).unwrap();
        let changed = [
            // Another machine's name on it.
            MachineRecord {
                machine: key(2).public(),
                ..record()
            },
            // Another payload under this signature.
            MachineRecord {
                payload: other.payload.clone(),
                ..record()
            },
            MachineRecord {
                sig: vec![0; 64],
                ..record()
            },
            MachineRecord {
                sig: vec![0; 3],
                ..record()
            },
        ];
        for record in changed {
            record.verify().unwrap_err();
        }
    }

    /// One a machine signed itself, saying it's another.
    fn signed_payload(signer: &SecretKey, payload: Vec<u8>) -> MachineRecord {
        MachineRecord {
            sig: signer.sign(&signing_input(&payload)).to_bytes().to_vec(),
            payload,
            machine: signer.public(),
        }
    }

    #[test]
    fn a_machine_cant_sign_for_another() {
        let info = MachineInfo {
            machine: key(2).public(),
            offers: vec![],
            published: DATE.into(),
        };
        let payload = serde_json::to_vec(&info).unwrap();
        let why = signed_payload(&key(1), payload).verify().unwrap_err();
        assert!(why.to_string().contains("another machine"), "{why:#}");
    }

    #[test]
    fn what_a_later_version_adds_is_ignored_and_still_checks_out() {
        let payload = serde_json::to_vec(&serde_json::json!({
            "machine": key(1).public(),
            "offers": ["directory", "something-new"],
            "published": DATE,
            "later": {"field": 1},
        }))
        .unwrap();
        let info = signed_payload(&key(1), payload).verify().unwrap();
        assert_eq!(info.offers, ["directory", "something-new"]);
    }

    #[test]
    fn records_have_limits() {
        let offers = vec!["x".to_string(); MAX_OFFERS + 1];
        MachineRecord::new(&key(1), offers, DATE.into())
            .unwrap()
            .verify()
            .unwrap_err();
        let long = vec!["x".repeat(MAX_RECORD_BYTES)];
        MachineRecord::new(&key(1), long.clone(), DATE.into()).unwrap_err();
        let payload = serde_json::to_vec(&serde_json::json!({
            "machine": key(1).public(),
            "offers": long,
            "published": DATE,
        }))
        .unwrap();
        signed_payload(&key(1), payload).verify().unwrap_err();
    }

    #[test]
    fn the_largest_record_fits_a_request_and_a_reply() {
        let largest = signed_payload(&key(1), vec![0xff; MAX_RECORD_BYTES]);
        let request = MachineRequest::Hello {
            record: largest.clone(),
        };
        let len = serde_json::to_vec(&request).unwrap().len();
        assert!(len <= MAX_REQUEST_LEN as usize, "{len} bytes");
        let reply = MachineResponse::Hello { record: largest };
        let len = serde_json::to_vec(&reply).unwrap().len();
        assert!(len <= MAX_REPLY_LEN as usize, "{len} bytes");
    }

    #[test]
    fn a_request_from_a_later_version_is_read_as_unknown() {
        let later = r#"{"op":"something_later","with":{"a":1}}"#;
        let request: MachineRequest = serde_json::from_str(later).unwrap();
        assert!(matches!(request, MachineRequest::Unknown));
    }
}
