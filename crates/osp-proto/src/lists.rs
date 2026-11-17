//! Signed lists a publisher puts out for nodes to follow by choice (ADR 0019,
//! ADR 0042 §6). The first is the project's hide list: posts and files that
//! official software, set up as released, doesn't fetch, serve or show.
//!
//! A list never names what it lists. Each entry is a hash of the post's ID
//! or the file's hash, so only someone who already has the post or file can
//! tell it's listed. Entries are dated, and a retracted one stays, so a node
//! that missed the retraction learns it.
//!
//! The file a publisher serves is a [`SignedList`]: the list's exact JSON
//! bytes and a signature over them. Nobody edits it by hand.

use anyhow::{Context, Result, bail, ensure};
use iroh_base::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::{object::base64url, types::BlobHash};

/// The format's name, in every list and mixed into its signature.
pub const LIST_FORMAT: &str = "osp-list/1";

/// Largest signed list a node takes, in bytes: a few thousand entries.
pub const MAX_LIST_BYTES: usize = 1024 * 1024;

/// The signature's domain, so a list's signature can't pass for any other.
const SIG_DOMAIN: &[u8] = b"osp-list/1:";

/// The hash-of-hash's domain (BLAKE3 key derivation).
const ENTRY_CONTEXT: &str = "osp-list/1 hide entry";

/// What a hide list entry stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HideKind {
    /// A post or reshare, by its object ID.
    Post,
    /// A file, by its hash: any post naming it.
    File,
}

impl HideKind {
    fn tag(self) -> u8 {
        match self {
            Self::Post => b'p',
            Self::File => b'f',
        }
    }
}

/// The hash an entry holds for a post's ID or a file's hash.
pub fn entry_hash(kind: HideKind, raw: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key(ENTRY_CONTEXT);
    h.update(&[kind.tag()]);
    h.update(raw);
    *h.finalize().as_bytes()
}

/// The entry hash for a post's ID.
pub fn post_entry(id: &crate::ObjectId) -> [u8; 32] {
    entry_hash(HideKind::Post, id.as_bytes())
}

/// The entry hash for a file's hash.
pub fn file_entry(hash: &BlobHash) -> [u8; 32] {
    entry_hash(HideKind::File, hash.as_bytes())
}

/// One entry: a hash of a hash, when it was added, and when it was taken
/// back, if it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HideEntry {
    pub kind: HideKind,
    #[serde(with = "hex32")]
    pub h: [u8; 32],
    pub added: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retracted: Option<String>,
}

/// A hide list, as signed. `seq` goes up with each list a publisher signs,
/// so a node never goes back to an older one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HideList {
    pub format: String,
    /// Which list this is: `hide`.
    pub list: String,
    /// The publisher's key, which signed it.
    pub key: PublicKey,
    pub seq: u64,
    pub published: String,
    pub entries: Vec<HideEntry>,
}

impl HideList {
    /// An empty list, before its first entry.
    pub fn new(key: PublicKey) -> Self {
        Self {
            format: LIST_FORMAT.into(),
            list: "hide".into(),
            key,
            seq: 0,
            published: String::new(),
            entries: Vec::new(),
        }
    }

    /// Adds an entry, or brings back a retracted one. False if it was
    /// already on.
    pub fn add(&mut self, kind: HideKind, h: [u8; 32], now: &str) -> bool {
        match self.entries.iter_mut().find(|e| e.kind == kind && e.h == h) {
            Some(e) if e.retracted.is_none() => false,
            Some(e) => {
                e.retracted = None;
                e.added = now.into();
                true
            }
            None => {
                self.entries.push(HideEntry {
                    kind,
                    h,
                    added: now.into(),
                    retracted: None,
                });
                true
            }
        }
    }

    /// Takes an entry back; it stays in the list, retracted. False if there
    /// was none on.
    pub fn retract(&mut self, kind: HideKind, h: [u8; 32], now: &str) -> bool {
        match self
            .entries
            .iter_mut()
            .find(|e| e.kind == kind && e.h == h && e.retracted.is_none())
        {
            Some(e) => {
                e.retracted = Some(now.into());
                true
            }
            None => false,
        }
    }

    /// The entries in force.
    pub fn active(&self) -> impl Iterator<Item = &HideEntry> {
        self.entries.iter().filter(|e| e.retracted.is_none())
    }
}

/// A list as served: its exact JSON bytes and the publisher's signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedList {
    #[serde(with = "base64url")]
    list: Vec<u8>,
    #[serde(with = "base64url")]
    sig: Vec<u8>,
}

impl SignedList {
    /// Signs `list` with `key`, which must be the key it names. Its `seq`
    /// and `published` are the caller's to set first.
    pub fn sign(list: &HideList, key: &SecretKey) -> Result<Self> {
        ensure!(list.key == key.public(), "the list names another key");
        let bytes = serde_json::to_vec(list)?;
        let sig = key.sign(&[SIG_DOMAIN, &bytes].concat());
        let signed = Self {
            list: bytes,
            sig: sig.to_bytes().to_vec(),
        };
        ensure!(
            signed.to_json().len() <= MAX_LIST_BYTES,
            "the list is too large"
        );
        Ok(signed)
    }

    /// Parses a served file.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() <= MAX_LIST_BYTES, "the list is too large");
        serde_json::from_slice(bytes).context("not a signed list")
    }

    /// The key the list says signed it, unchecked: for showing a list, never
    /// for trusting one.
    pub fn named_key(&self) -> Result<PublicKey> {
        #[derive(Deserialize)]
        struct Named {
            key: PublicKey,
        }
        let named: Named = serde_json::from_slice(&self.list).context("not a hide list")?;
        Ok(named.key)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a signed list serialises")
    }

    /// Checks the signature against `key`, the one publisher this node
    /// takes the list from, and returns the list. A list signed by any other
    /// key is refused.
    pub fn verify(&self, key: &PublicKey) -> Result<HideList> {
        let sig: [u8; Signature::LENGTH] = self
            .sig
            .as_slice()
            .try_into()
            .context("signature must be 64 bytes")?;
        key.verify(
            &[SIG_DOMAIN, &self.list].concat(),
            &Signature::from_bytes(&sig),
        )
        .map_err(|_| anyhow::anyhow!("the list isn't signed by the expected key"))?;
        let list: HideList = serde_json::from_slice(&self.list).context("not a hide list")?;
        if list.format != LIST_FORMAT || list.list != "hide" {
            bail!("not a hide list in {LIST_FORMAT}");
        }
        ensure!(list.key == *key, "the list names another key");
        Ok(list)
    }
}

/// Serde helper: 32 bytes as lowercase hex.
mod hex32 {
    use data_encoding::HEXLOWER;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&HEXLOWER.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(deserializer)?;
        HEXLOWER
            .decode(s.as_bytes())
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| serde::de::Error::custom("expected 64 hex digits"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ObjectId;

    fn key(n: u8) -> SecretKey {
        SecretKey::from_bytes(&[n; 32])
    }

    #[test]
    fn a_list_signed_by_its_key_verifies_and_by_another_is_refused() {
        let publisher = key(1);
        let mut list = HideList::new(publisher.public());
        let post = ObjectId::of(b"a post");
        assert!(list.add(HideKind::Post, post_entry(&post), "2026-11-17T00:00:00Z"));
        list.seq = 1;
        let signed = SignedList::sign(&list, &publisher).unwrap();
        let back = SignedList::from_json(signed.to_json().as_bytes()).unwrap();
        assert_eq!(back.verify(&publisher.public()).unwrap(), list);

        // Another key, signing a list that names it, isn't the publisher.
        let other = key(2);
        let mut forged = list.clone();
        forged.key = other.public();
        let forged = SignedList::sign(&forged, &other).unwrap();
        assert!(forged.verify(&publisher.public()).is_err());

        // A list changed after signing, say an entry taken out, is refused.
        let mut changed = list.clone();
        changed.entries.clear();
        let changed = SignedList {
            list: serde_json::to_vec(&changed).unwrap(),
            sig: signed.sig.clone(),
        };
        assert!(changed.verify(&publisher.public()).is_err());
    }

    #[test]
    fn the_list_holds_no_plain_hash() {
        let publisher = key(1);
        let mut list = HideList::new(publisher.public());
        let post = ObjectId::of(b"a post");
        let file = BlobHash::from_bytes(*blake3::hash(b"a file").as_bytes());
        list.add(HideKind::Post, post_entry(&post), "t");
        list.add(HideKind::File, file_entry(&file), "t");
        let json = SignedList::sign(&list, &publisher).unwrap().to_json();
        let inner = serde_json::to_string(&list).unwrap();
        for plain in [post.to_string(), file.to_string()] {
            assert!(!json.contains(&plain) && !inner.contains(&plain));
        }
        // The same 32 bytes as a post and as a file are different entries.
        assert_ne!(
            entry_hash(HideKind::Post, post.as_bytes()),
            entry_hash(HideKind::File, post.as_bytes())
        );
    }

    #[test]
    fn a_retraction_stays_in_the_list_and_an_entry_can_come_back() {
        let mut list = HideList::new(key(1).public());
        let h = post_entry(&ObjectId::of(b"x"));
        assert!(list.add(HideKind::Post, h, "1"));
        assert!(!list.add(HideKind::Post, h, "2"));
        assert!(list.retract(HideKind::Post, h, "3"));
        assert!(!list.retract(HideKind::Post, h, "4"));
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.active().count(), 0);
        assert!(list.add(HideKind::Post, h, "5"));
        assert_eq!(list.active().count(), 1);
    }
}
