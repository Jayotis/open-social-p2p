//! Signed objects and their identifiers (ADR 0004).
//!
//! A signed object carries the exact JSON bytes its author signed. Those bytes
//! are never re-serialised, so verification never depends on how a particular
//! JSON library orders keys or formats numbers.

use anyhow::{Context, Result, ensure};
use iroh_base::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::types::Payload;

/// Prefix mixed into every signature so it can't be confused with any other
/// use of the same key.
const SIG_DOMAIN: &[u8] = b"osp-v0:";

/// Largest payload, in bytes. The longest post allowed fits even if every
/// character of its text and alt text needs escaping in JSON.
pub const MAX_PAYLOAD_BYTES: usize = 128 * 1024;

crate::types::hex_id! {
    /// Identifier of a signed object: the BLAKE3 hash of its payload bytes.
    ObjectId
}

impl ObjectId {
    /// Computes the ID of the given payload bytes.
    pub fn of(payload: &[u8]) -> Self {
        Self(*blake3::hash(payload).as_bytes())
    }
}

/// An object as it is stored and sent: payload bytes, who signed them, and the signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedObject {
    #[serde(with = "base64url")]
    payload: Vec<u8>,
    signer: PublicKey,
    #[serde(with = "base64url")]
    sig: Vec<u8>,
}

impl SignedObject {
    /// Serialises `payload` once and signs those exact bytes.
    pub fn sign(key: &SecretKey, payload: &Payload) -> Result<Self> {
        ensure!(
            payload.actor() == key.public(),
            "payload actor must be the signing identity"
        );
        payload.check_limits()?;
        let bytes = serde_json::to_vec(payload)?;
        ensure!(
            bytes.len() <= MAX_PAYLOAD_BYTES,
            "object is larger than {MAX_PAYLOAD_BYTES} bytes"
        );
        let sig = key.sign(&signing_input(&bytes));
        Ok(Self {
            payload: bytes,
            signer: key.public(),
            sig: sig.to_bytes().to_vec(),
        })
    }

    /// The object's ID: BLAKE3 of the payload bytes.
    pub fn id(&self) -> ObjectId {
        ObjectId::of(&self.payload)
    }

    /// The identity that signed this object.
    pub fn signer(&self) -> PublicKey {
        self.signer
    }

    /// The exact bytes that were signed.
    pub fn payload_bytes(&self) -> &[u8] {
        &self.payload
    }

    /// Checks the size, the signature, that the payload's `actor` is the
    /// signer, and the limits on its contents, then returns the parsed
    /// payload. Never use a payload that hasn't been through this.
    pub fn verify(&self) -> Result<Payload> {
        ensure!(
            self.payload.len() <= MAX_PAYLOAD_BYTES,
            "object is larger than {MAX_PAYLOAD_BYTES} bytes"
        );
        let sig: [u8; Signature::LENGTH] = self
            .sig
            .as_slice()
            .try_into()
            .context("signature must be 64 bytes")?;
        self.signer
            .verify(&signing_input(&self.payload), &Signature::from_bytes(&sig))
            .map_err(|_| anyhow::anyhow!("invalid signature"))?;
        let payload: Payload =
            serde_json::from_slice(&self.payload).context("payload is not a valid object")?;
        ensure!(
            payload.actor() == self.signer,
            "payload actor does not match signer"
        );
        payload.check_limits()?;
        Ok(payload)
    }
}

fn signing_input(payload: &[u8]) -> Vec<u8> {
    [SIG_DOMAIN, payload].concat()
}

/// Serde helper: bytes as unpadded base64url strings.
mod base64url {
    use data_encoding::BASE64URL_NOPAD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&BASE64URL_NOPAD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(deserializer)?;
        BASE64URL_NOPAD
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        Attachment, BlobHash, BlobRef, MAX_ALT_TEXT_BYTES, MAX_ATTACHMENT_BYTES, MAX_ATTACHMENTS,
        MAX_CONTENT_BYTES, MAX_NAME_BYTES, MediaKind, Note, Person,
    };

    fn note(key: &SecretKey, text: &str) -> Payload {
        Payload::Note(Note {
            actor: key.public(),
            content: text.to_string(),
            published: "2026-10-21T12:00:00.000Z".to_string(),
            attachment: vec![],
            in_reply_to: None,
        })
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let key = SecretKey::generate();
        let payload = note(&key, "hello");
        let obj = SignedObject::sign(&key, &payload).unwrap();
        assert_eq!(obj.verify().unwrap(), payload);

        // Survives a trip through JSON, which is how it travels.
        let json = serde_json::to_string(&obj).unwrap();
        let back: SignedObject = serde_json::from_str(&json).unwrap();
        assert_eq!(back.verify().unwrap(), payload);
        assert_eq!(back.id(), obj.id());
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let key = SecretKey::generate();
        let mut obj = SignedObject::sign(&key, &note(&key, "hello")).unwrap();
        let pos = obj.payload.iter().position(|b| *b == b'h').unwrap();
        obj.payload[pos] = b'j';
        assert!(obj.verify().is_err());
    }

    #[test]
    fn wrong_signer_is_rejected() {
        let alice = SecretKey::generate();
        let mallory = SecretKey::generate();
        let mut obj = SignedObject::sign(&alice, &note(&alice, "hello")).unwrap();
        obj.signer = mallory.public();
        assert!(obj.verify().is_err());
    }

    #[test]
    fn cannot_sign_for_someone_else() {
        let alice = SecretKey::generate();
        let mallory = SecretKey::generate();
        assert!(SignedObject::sign(&mallory, &note(&alice, "hello")).is_err());
    }

    /// Signs without the limit checks, as a hostile author's software might.
    fn sign_unchecked(key: &SecretKey, payload: &Payload) -> SignedObject {
        let bytes = serde_json::to_vec(payload).unwrap();
        SignedObject {
            sig: key.sign(&signing_input(&bytes)).to_bytes().to_vec(),
            payload: bytes,
            signer: key.public(),
        }
    }

    fn attachment(media_type: &str, size: u64, alt: Option<String>) -> Attachment {
        Attachment {
            kind: MediaKind::Image,
            media_type: media_type.to_string(),
            blob: BlobHash::from_bytes([1; 32]),
            size,
            width: 10,
            height: 10,
            duration_ms: None,
            name: alt,
            thumbnail: Some(BlobRef {
                blob: BlobHash::from_bytes([2; 32]),
                size: 100,
                media_type: "image/webp".to_string(),
            }),
            profile: "v1-720p".to_string(),
        }
    }

    fn note_with(key: &SecretKey, content: String, attachment: Vec<Attachment>) -> Payload {
        Payload::Note(Note {
            actor: key.public(),
            content,
            published: "2026-10-22T12:00:00.000Z".to_string(),
            attachment,
            in_reply_to: None,
        })
    }

    #[test]
    fn objects_over_the_limits_are_refused_when_signing_and_verifying() {
        let key = SecretKey::generate();
        let image = || attachment("image/webp", 1000, None);
        let bad = [
            note_with(&key, "x".repeat(MAX_CONTENT_BYTES + 1), vec![]),
            note_with(&key, "hi".into(), vec![image(); MAX_ATTACHMENTS + 1]),
            note_with(&key, "hi".into(), vec![attachment("text/html", 10, None)]),
            note_with(
                &key,
                "hi".into(),
                vec![attachment("video/mp4", MAX_ATTACHMENT_BYTES + 1, None)],
            ),
            Payload::Person(Person {
                actor: key.public(),
                name: "n".repeat(MAX_NAME_BYTES + 1),
                summary: None,
                nodes: vec![],
                published: "2026-10-22T12:00:00.000Z".to_string(),
            }),
        ];
        for payload in &bad {
            assert!(SignedObject::sign(&key, payload).is_err(), "{payload:?}");
            assert!(
                sign_unchecked(&key, payload).verify().is_err(),
                "{payload:?}"
            );
        }
        // Junk padding in fields nobody checks still hits the size limit.
        let mut huge = sign_unchecked(&key, &note(&key, "hi"));
        huge.payload
            .extend(std::iter::repeat_n(b' ', MAX_PAYLOAD_BYTES));
        assert!(huge.verify().unwrap_err().to_string().contains("larger"));
    }

    #[test]
    fn the_largest_allowed_post_fits_even_when_fully_escaped() {
        let key = SecretKey::generate();
        // Control characters are escaped as \u00XX: six bytes each.
        let alt = || Some("\u{1}".repeat(MAX_ALT_TEXT_BYTES));
        let payload = note_with(
            &key,
            "\u{1}".repeat(MAX_CONTENT_BYTES),
            vec![attachment("video/mp4", MAX_ATTACHMENT_BYTES, alt()); MAX_ATTACHMENTS],
        );
        let obj = SignedObject::sign(&key, &payload).unwrap();
        assert_eq!(obj.verify().unwrap(), payload);
    }

    #[test]
    fn id_is_blake3_of_payload() {
        let key = SecretKey::generate();
        let obj = SignedObject::sign(&key, &note(&key, "hello")).unwrap();
        assert_eq!(
            obj.id().as_bytes(),
            blake3::hash(obj.payload_bytes()).as_bytes()
        );
        let parsed: ObjectId = obj.id().to_string().parse().unwrap();
        assert_eq!(parsed, obj.id());
    }
}
