//! Messages of the machine protocol `osp/machine/1` (ADR 0015): what nodes say
//! to each other under their machine names, which are tied to no person.
//!
//! As in `osp/social/2`, one QUIC bidirectional stream per request: the asking
//! machine sends one [`MachineRequest`] frame, and the other answers with one
//! [`MachineResponse`].

use anyhow::{Context, Result, ensure};
use iroh_base::{EndpointId, PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::object::{SignedObject, base64url};

const RECORD_DOMAIN: &[u8] = b"osp-machine-record-v0:";
const INTRODUCE_DOMAIN: &[u8] = b"osp-machine-introduce-v0:";

/// Longest signed part of a machine record, in bytes.
pub const MAX_RECORD_BYTES: usize = 1024;
/// Longest signed part of a request to be introduced, in bytes.
pub const MAX_INTRODUCE_BYTES: usize = 512;
/// Longest a request to be introduced may run before it expires, so a machine
/// remembers the requests it has seen for no longer than this.
pub const MAX_INTRODUCE_LIFETIME_SECS: u64 = 60 * 60;
/// Most things a record can say its machine offers.
pub const MAX_OFFERS: usize = 8;
/// Largest request frame a machine reads. A request carries one record at most.
pub const MAX_REQUEST_LEN: u32 = 4 * 1024;
/// Largest reply frame a machine reads: a page of a listing, at most.
pub const MAX_REPLY_LEN: u32 = 128 * 1024;
/// What a machine's record says it offers when it's a directory.
pub const OFFERS_DIRECTORY: &str = "directory";
/// Most entries in one page of a listing.
pub const MAX_LISTING_PAGE: u32 = 100;
/// A page of a listing stops before passing this many bytes of entries, as bulk
/// speed over Tor varies so much (ADR 0014). It always has at least one.
pub const MAX_LISTING_PAGE_BYTES: usize = 64 * 1024;
/// Most posts a directory lists of one person.
pub const MAX_LISTED_POSTS: usize = 5;
/// Most records in a reply to `Known`.
pub const MAX_KNOWN: usize = 20;
/// Largest listing of one person, in bytes.
pub const MAX_LISTED_PERSON_BYTES: usize = 32 * 1024;

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

/// What a machine asking to be introduced says (ADR 0016): introduce me,
/// `asker`, through you, `via`, to `target`, a machine whose record you passed
/// on to me. It names machines only, nothing of either owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntroduceInfo {
    /// Random, so the machine introduced to can drop one it has seen.
    pub id: String,
    pub asker: EndpointId,
    pub via: EndpointId,
    pub target: EndpointId,
    /// RFC 3339.
    pub expires: String,
}

/// A request to be introduced, signed by the machine asking, with its own
/// record. The machine asked passes it on as it is, so the machine introduced
/// to checks both itself, and can't be told of a request the asker didn't make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntroduceRequest {
    #[serde(with = "base64url")]
    payload: Vec<u8>,
    asker: EndpointId,
    #[serde(with = "base64url")]
    sig: Vec<u8>,
    /// The asker's own record.
    pub record: MachineRecord,
}

impl IntroduceRequest {
    pub fn new(
        key: &SecretKey,
        id: [u8; 16],
        via: EndpointId,
        target: EndpointId,
        expires: String,
        record: MachineRecord,
    ) -> Result<Self> {
        let info = IntroduceInfo {
            id: data_encoding::HEXLOWER.encode(&id),
            asker: key.public(),
            via,
            target,
            expires,
        };
        let payload = serde_json::to_vec(&info)?;
        ensure!(
            payload.len() <= MAX_INTRODUCE_BYTES,
            "a request to be introduced is larger than {MAX_INTRODUCE_BYTES} bytes"
        );
        let sig = key.sign(&introduce_input(&payload)).to_bytes().to_vec();
        Ok(Self {
            payload,
            asker: key.public(),
            sig,
            record,
        })
    }

    /// Checks the size, the asker's signature, and that the record is the
    /// asker's own. Whether it has expired, and who it names, is for the
    /// caller to check.
    pub fn verify(&self) -> Result<IntroduceInfo> {
        ensure!(
            self.payload.len() <= MAX_INTRODUCE_BYTES,
            "a request to be introduced is larger than {MAX_INTRODUCE_BYTES} bytes"
        );
        let sig: [u8; Signature::LENGTH] = self
            .sig
            .as_slice()
            .try_into()
            .context("signature must be 64 bytes")?;
        self.asker
            .verify(
                &introduce_input(&self.payload),
                &Signature::from_bytes(&sig),
            )
            .map_err(|_| anyhow::anyhow!("invalid signature on a request to be introduced"))?;
        let info: IntroduceInfo =
            serde_json::from_slice(&self.payload).context("not a request to be introduced")?;
        ensure!(
            info.asker == self.asker,
            "this request speaks of another machine"
        );
        ensure!(info.id.len() == 32, "a request's ID is 16 bytes");
        let record = self.record.verify()?;
        ensure!(
            record.machine == info.asker,
            "the record with a request isn't the asker's"
        );
        Ok(info)
    }
}

fn introduce_input(payload: &[u8]) -> Vec<u8> {
    [INTRODUCE_DOMAIN, payload].concat()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MachineRequest {
    /// Here's my record; what's yours? It must be the record of the machine
    /// asking.
    Hello { record: MachineRecord },
    /// A directory's lists, in pages, as an outbox is read: the entries with a
    /// sequence number greater than `since`. Asking for none still says where
    /// the end is. Other machines say they aren't directories.
    Listing { since: u64, limit: u32 },
    /// Some machines you've met lately: up to `MAX_KNOWN` of their records, as
    /// they signed them, picked at random. It carries nothing of what the
    /// asker wants, and the reply says nothing of how or when they met.
    Known,
    /// Introduce me to a machine whose record you passed on to me (ADR 0016).
    /// It must be the asker's own request, naming the machine asked as `via`.
    /// The reply says only whether it was passed on.
    Introduce { request: IntroduceRequest },
    /// A machine you passed on to me asks to be introduced to you, as it
    /// signed it. The reply is the same whatever the machine introduced to
    /// decides, so the one passing it on never learns.
    Introduction { request: IntroduceRequest },
    /// A request from a later version. It gets an error rather than a closed
    /// connection, so requests can be added.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum MachineResponse {
    Hello {
        record: MachineRecord,
    },
    Listing {
        entries: Vec<ListingEntry>,
        /// The newest sequence number in the lists.
        latest: u64,
    },
    Known {
        records: Vec<MachineRecord>,
    },
    Introduce {
        passed_on: bool,
    },
    Introduction,
    Error {
        message: String,
    },
}

/// One entry in a directory's lists, standing for one machine or one person. It
/// gets a new sequence number when what it says changes, so a reader asks only
/// for what changed since it last read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListingEntry {
    pub seq: u64,
    pub item: ListingItem,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ListingItem {
    /// A machine that greeted the directory, and answered when greeted back.
    Machine { record: MachineRecord },
    /// Someone who asked to be listed: their signed profile, and their latest
    /// posts, newest first, each as they signed it.
    Person {
        profile: SignedObject,
        posts: Vec<SignedObject>,
    },
    /// Someone no longer listed. It isn't signed, as it can only take away what
    /// came from this directory, and a directory can always leave things out.
    Gone { actor: PublicKey },
    /// An entry from a later version, which a reader passes over. Once it
    /// takes that kind, it reads each directory again from the start.
    #[serde(other)]
    Unknown,
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
    fn a_full_known_reply_of_the_largest_records_fits() {
        let largest = signed_payload(&key(1), vec![0xff; MAX_RECORD_BYTES]);
        let reply = MachineResponse::Known {
            records: vec![largest; MAX_KNOWN],
        };
        let len = serde_json::to_vec(&reply).unwrap().len();
        assert!(len <= MAX_REPLY_LEN as usize, "{len} bytes");
        let request = serde_json::to_vec(&MachineRequest::Known).unwrap();
        assert_eq!(request, br#"{"op":"known"}"#);
    }

    #[test]
    fn the_largest_listing_of_a_person_and_a_full_page_fit_a_reply() {
        let key = key(1);
        // Control characters take six bytes each in JSON.
        let person = crate::types::Person {
            actor: key.public(),
            name: "\u{1}".repeat(crate::types::MAX_NAME_BYTES),
            summary: Some("\u{1}".repeat(crate::types::MAX_SUMMARY_BYTES)),
            discoverable: true,
            nodes: vec![key.public(); crate::types::MAX_PROFILE_NODES],
            published: DATE.into(),
        };
        let profile = SignedObject::sign(&key, &crate::Payload::Person(person)).unwrap();
        let len = serde_json::to_vec(&profile).unwrap().len();
        assert!(len <= MAX_LISTED_PERSON_BYTES, "{len} bytes");

        // A page stops short of its limit, and at worst holds that much more.
        let item = ListingItem::Person {
            profile,
            posts: vec![],
        };
        let filler = ListingEntry {
            seq: u64::MAX,
            item: item.clone(),
        };
        let per_entry = serde_json::to_vec(&filler).unwrap().len();
        let count = MAX_LISTING_PAGE_BYTES / per_entry + 1;
        let reply = MachineResponse::Listing {
            entries: vec![filler; count],
            latest: u64::MAX,
        };
        let len = serde_json::to_vec(&reply).unwrap().len();
        assert!(len <= MAX_REPLY_LEN as usize, "{len} bytes");
    }

    #[test]
    fn an_entry_from_a_later_version_is_read_as_unknown() {
        let later = r#"{"seq":3,"item":{"kind":"something_later","with":1}}"#;
        let entry: ListingEntry = serde_json::from_str(later).unwrap();
        assert_eq!(entry.item, ListingItem::Unknown);
    }

    fn introduce(asker: &SecretKey, record: MachineRecord) -> IntroduceRequest {
        IntroduceRequest::new(
            asker,
            [9; 16],
            key(2).public(),
            key(3).public(),
            DATE.into(),
            record,
        )
        .unwrap()
    }

    #[test]
    fn a_request_to_be_introduced_checks_out_after_travelling() {
        let request = introduce(&key(1), record());
        let json = serde_json::to_string(&MachineRequest::Introduce {
            request: request.clone(),
        })
        .unwrap();
        let MachineRequest::Introduce { request: back } = serde_json::from_str(&json).unwrap()
        else {
            panic!("another request");
        };
        assert_eq!(back, request);
        let info = back.verify().unwrap();
        assert_eq!(info.asker, key(1).public());
        assert_eq!(info.via, key(2).public());
        assert_eq!(info.target, key(3).public());
        assert_eq!(info.id, "09".repeat(16));
    }

    #[test]
    fn a_request_to_be_introduced_is_the_askers_alone() {
        let request = introduce(&key(1), record());
        let other = introduce(&key(4), record_of(&key(4)));
        let changed = [
            // Another asker's name on it.
            IntroduceRequest {
                asker: key(4).public(),
                ..request.clone()
            },
            // Another payload under this signature.
            IntroduceRequest {
                payload: other.payload.clone(),
                ..request.clone()
            },
            IntroduceRequest {
                sig: vec![0; 64],
                ..request.clone()
            },
            // Another machine's record with it.
            IntroduceRequest {
                record: record_of(&key(4)),
                ..request.clone()
            },
        ];
        for request in changed {
            request.verify().unwrap_err();
        }
        // A record signed for a record isn't a request to be introduced.
        let as_record = IntroduceRequest {
            sig: key(1)
                .sign(&signing_input(&request.payload))
                .to_bytes()
                .to_vec(),
            ..request
        };
        as_record.verify().unwrap_err();
    }

    fn record_of(key: &SecretKey) -> MachineRecord {
        MachineRecord::new(key, vec![], DATE.into()).unwrap()
    }

    #[test]
    fn the_largest_request_to_be_introduced_fits_a_request() {
        let request = IntroduceRequest {
            payload: vec![0xff; MAX_INTRODUCE_BYTES],
            asker: key(1).public(),
            sig: vec![0xff; Signature::LENGTH],
            record: signed_payload(&key(1), vec![0xff; MAX_RECORD_BYTES]),
        };
        let len = serde_json::to_vec(&MachineRequest::Introduction { request })
            .unwrap()
            .len();
        assert!(len <= MAX_REQUEST_LEN as usize, "{len} bytes");
    }

    #[test]
    fn a_request_from_a_later_version_is_read_as_unknown() {
        let later = r#"{"op":"something_later","with":{"a":1}}"#;
        let request: MachineRequest = serde_json::from_str(later).unwrap();
        assert!(matches!(request, MachineRequest::Unknown));
    }
}
