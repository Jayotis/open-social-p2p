//! Signed objects and their identifiers (ADR 0004).
//!
//! A signed object carries the exact JSON bytes its author signed. Those bytes
//! are never re-serialised, so verification never depends on how a particular
//! JSON library orders keys or formats numbers.
//!
//! An object is signed either by its identity key itself or by a signing key
//! that identity certified (ADR 0010). The certificate travels inside the
//! object, so anyone can check it alone. Whether the key has since been
//! revoked is for the reader to check against what it knows of the identity.

use anyhow::{Context, Result, bail, ensure};
use iroh_base::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::types::{EarlierKeys, Key, Payload};

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

/// An object as it is stored and sent: payload bytes, the identity they
/// belong to, the certificate of the key that signed them (unless the
/// identity key signed them itself), and the signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedObject {
    #[serde(with = "base64url")]
    payload: Vec<u8>,
    signer: PublicKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<Box<SignedObject>>,
    #[serde(with = "base64url")]
    sig: Vec<u8>,
}

impl SignedObject {
    /// Serialises `payload` once and signs those exact bytes with the
    /// identity key itself. Nodes do this only for key certificates
    /// (ADR 0010); objects made before them were all signed this way.
    pub fn sign(identity: &SecretKey, payload: &Payload) -> Result<Self> {
        ensure!(
            payload.actor() == identity.public(),
            "payload actor must be the signing identity"
        );
        Self::seal(identity, identity.public(), None, payload)
    }

    /// Serialises `payload` once and signs those exact bytes with a key its
    /// identity certified.
    pub fn sign_certified(key: &CertifiedKey, payload: &Payload) -> Result<Self> {
        ensure!(
            payload.actor() == key.identity,
            "payload actor must be the key's identity"
        );
        ensure!(
            !matches!(payload, Payload::Key(_)),
            "only the identity key itself signs key certificates"
        );
        Self::seal(&key.secret, key.identity, Some(key.cert.clone()), payload)
    }

    fn seal(
        secret: &SecretKey,
        signer: PublicKey,
        key: Option<Box<SignedObject>>,
        payload: &Payload,
    ) -> Result<Self> {
        payload.check_limits()?;
        let bytes = serde_json::to_vec(payload)?;
        ensure!(
            bytes.len() <= MAX_PAYLOAD_BYTES,
            "object is larger than {MAX_PAYLOAD_BYTES} bytes"
        );
        let sig = secret.sign(&signing_input(&bytes));
        Ok(Self {
            payload: bytes,
            signer,
            key,
            sig: sig.to_bytes().to_vec(),
        })
    }

    /// This object, signed by `key` instead. The payload bytes don't change,
    /// so neither does the ID (ADR 0010).
    pub fn signed_again(&self, key: &CertifiedKey) -> Result<Self> {
        let payload = self.verify()?;
        ensure!(
            payload.actor() == key.identity,
            "the object belongs to another identity"
        );
        ensure!(
            !matches!(payload, Payload::Key(_)),
            "only the identity key itself signs key certificates"
        );
        let sig = key.secret.sign(&signing_input(&self.payload));
        Ok(Self {
            payload: self.payload.clone(),
            signer: key.identity,
            key: Some(key.cert.clone()),
            sig: sig.to_bytes().to_vec(),
        })
    }

    /// The object's ID: BLAKE3 of the payload bytes.
    pub fn id(&self) -> ObjectId {
        ObjectId::of(&self.payload)
    }

    /// The identity this object belongs to.
    pub fn signer(&self) -> PublicKey {
        self.signer
    }

    /// The certificate of the key that signed this, if the identity key
    /// didn't sign it itself.
    pub fn certificate(&self) -> Option<&SignedObject> {
        self.key.as_deref()
    }

    /// The exact bytes that were signed.
    pub fn payload_bytes(&self) -> &[u8] {
        &self.payload
    }

    /// Checks the size, the signature, that the payload's `actor` is the
    /// signer, and the limits on its contents, then returns the parsed
    /// payload. Never use a payload that hasn't been through this.
    ///
    /// An object signed by a certified key is checked only against its own
    /// certificate here. Readers must also check that key hasn't been
    /// revoked: see `verify_with_key`.
    pub fn verify(&self) -> Result<Payload> {
        Ok(self.verify_with_key()?.0)
    }

    /// Like `verify`, and also returns the certificate of the key that
    /// signed the object, or `None` if the identity key signed it itself.
    pub fn verify_with_key(&self) -> Result<(Payload, Option<Key>)> {
        ensure!(
            self.payload.len() <= MAX_PAYLOAD_BYTES,
            "object is larger than {MAX_PAYLOAD_BYTES} bytes"
        );
        let (signing_key, cert) = match &self.key {
            None => (self.signer, None),
            Some(cert) => {
                // A certificate's own signer is checked against `actor` by
                // this same function, one level down. No deeper: no chains.
                ensure!(
                    cert.key.is_none(),
                    "a key certificate must be signed by its identity itself"
                );
                let Payload::Key(k) = cert.verify().context("invalid key certificate")? else {
                    bail!("`key` holds something other than a key certificate");
                };
                ensure!(
                    k.actor == self.signer,
                    "the key certificate is for another identity"
                );
                (k.key, Some(k))
            }
        };
        let sig: [u8; Signature::LENGTH] = self
            .sig
            .as_slice()
            .try_into()
            .context("signature must be 64 bytes")?;
        signing_key
            .verify(&signing_input(&self.payload), &Signature::from_bytes(&sig))
            .map_err(|_| anyhow::anyhow!("invalid signature"))?;
        let payload: Payload =
            serde_json::from_slice(&self.payload).context("payload is not a valid object")?;
        ensure!(
            payload.actor() == self.signer,
            "payload actor does not match signer"
        );
        ensure!(
            !(matches!(payload, Payload::Key(_)) && cert.is_some()),
            "only the identity key itself signs key certificates"
        );
        payload.check_limits()?;
        Ok((payload, cert))
    }
}

/// A signing key, with the certificate its identity signed for it (ADR 0010).
#[derive(Clone)]
pub struct CertifiedKey {
    secret: SecretKey,
    identity: PublicKey,
    cert: Box<SignedObject>,
}

impl CertifiedKey {
    /// Has the identity key certify `secret`, dated `published`.
    pub fn certify(
        identity: &SecretKey,
        secret: SecretKey,
        published: String,
        earlier_keys: EarlierKeys,
    ) -> Result<Self> {
        let cert = Payload::Key(Key {
            actor: identity.public(),
            key: secret.public(),
            published,
            earlier_keys,
        });
        let cert = SignedObject::sign(identity, &cert)?;
        Self::new(secret, cert)
    }

    /// Pairs a signing key with its certificate, checking they belong together.
    pub fn new(secret: SecretKey, cert: SignedObject) -> Result<Self> {
        let Payload::Key(k) = cert.verify()? else {
            bail!("not a key certificate");
        };
        ensure!(
            cert.key.is_none(),
            "a key certificate must be signed by its identity itself"
        );
        ensure!(
            k.key == secret.public(),
            "the certificate is for another key"
        );
        Ok(Self {
            secret,
            identity: k.actor,
            cert: Box::new(cert),
        })
    }

    /// The identity this key signs for.
    pub fn identity(&self) -> PublicKey {
        self.identity
    }

    /// The signing key's public half.
    pub fn public(&self) -> PublicKey {
        self.secret.public()
    }

    pub fn certificate(&self) -> &SignedObject {
        &self.cert
    }
}

fn signing_input(payload: &[u8]) -> Vec<u8> {
    [SIG_DOMAIN, payload].concat()
}

/// Serde helper: bytes as unpadded base64url strings.
pub(crate) mod base64url {
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
            key: None,
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

    fn certified(identity: &SecretKey, earlier_keys: EarlierKeys) -> CertifiedKey {
        let published = "2026-10-23T09:00:00.000Z".to_string();
        CertifiedKey::certify(identity, SecretKey::generate(), published, earlier_keys).unwrap()
    }

    #[test]
    fn a_certified_key_signs_for_its_identity() {
        let identity = SecretKey::generate();
        let key = certified(&identity, EarlierKeys::Retired);
        let payload = note(&identity, "hello");
        let obj = SignedObject::sign_certified(&key, &payload).unwrap();
        assert_eq!(obj.signer(), identity.public());

        let json = serde_json::to_string(&obj).unwrap();
        let back: SignedObject = serde_json::from_str(&json).unwrap();
        let (verified, cert) = back.verify_with_key().unwrap();
        assert_eq!(verified, payload);
        let cert = cert.expect("signed by a certified key");
        assert_eq!(cert.key, key.secret.public());
        assert_eq!(cert.earlier_keys, EarlierKeys::Retired);

        // The JSON names match ADR 0010.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value["key"]["payload"].is_string());
        let cert_json: serde_json::Value =
            serde_json::from_slice(key.certificate().payload_bytes()).unwrap();
        assert_eq!(cert_json["type"], "Key");
        assert_eq!(cert_json["earlierKeys"], "retired");

        // An object the identity key signed itself has no certificate.
        let old = SignedObject::sign(&identity, &payload).unwrap();
        assert!(old.verify_with_key().unwrap().1.is_none());
        assert!(!serde_json::to_string(&old).unwrap().contains("\"key\""));
    }

    #[test]
    fn signing_again_keeps_the_id() {
        let identity = SecretKey::generate();
        let old = SignedObject::sign(&identity, &note(&identity, "hello")).unwrap();
        let key = certified(&identity, EarlierKeys::Revoked);
        let again = old.signed_again(&key).unwrap();
        assert_eq!(again.id(), old.id());
        assert_ne!(again, old);
        assert!(again.verify_with_key().unwrap().1.is_some());

        // Not someone else's object.
        let other = certified(&SecretKey::generate(), EarlierKeys::Retired);
        assert!(old.signed_again(&other).is_err());
    }

    #[test]
    fn certificates_that_dont_fit_are_refused() {
        let alice = SecretKey::generate();
        let mallory = SecretKey::generate();
        let alices_key = certified(&alice, EarlierKeys::Retired);
        let good = SignedObject::sign_certified(&alices_key, &note(&alice, "hello")).unwrap();
        good.verify().unwrap();
        let refused = |obj: &SignedObject, expected: &str| {
            let err = format!("{:#}", obj.verify().unwrap_err());
            assert!(err.contains(expected), "{err:?}, not {expected:?}");
        };

        // Signed by some key other than the one the certificate names.
        let mut wrong_key = good.clone();
        wrong_key.sig = SecretKey::generate()
            .sign(&signing_input(&good.payload))
            .to_bytes()
            .to_vec();
        refused(&wrong_key, "invalid signature");

        // A certificate Mallory made for her own key, on Alice's post.
        let mallorys_key = certified(&mallory, EarlierKeys::Retired);
        let mut borrowed =
            SignedObject::sign_certified(&mallorys_key, &note(&mallory, "hi")).unwrap();
        borrowed.payload = good.payload.clone();
        borrowed.signer = alice.public();
        borrowed.sig = mallorys_key
            .secret
            .sign(&signing_input(&good.payload))
            .to_bytes()
            .to_vec();
        refused(&borrowed, "for another identity");

        // A certificate Mallory signed naming Alice and Mallory's key.
        let forged = Payload::Key(Key {
            actor: alice.public(),
            key: mallorys_key.secret.public(),
            published: "2026-10-23T09:00:00.000Z".into(),
            earlier_keys: EarlierKeys::Revoked,
        });
        let mut forged_cert = sign_unchecked(&mallory, &forged);
        forged_cert.signer = alice.public();
        let mut forged_obj = good.clone();
        forged_obj.key = Some(Box::new(forged_cert));
        forged_obj.sig = mallorys_key
            .secret
            .sign(&signing_input(&good.payload))
            .to_bytes()
            .to_vec();
        refused(&forged_obj, "invalid key certificate");

        // A changed certificate.
        let mut tampered = good.clone();
        let cert = tampered.key.as_mut().unwrap();
        let pos = cert.payload.iter().position(|b| *b == b'r').unwrap();
        cert.payload[pos] = b'R';
        refused(&tampered, "invalid key certificate");

        // A certificate signed by a certified key: no chains.
        let second = SecretKey::generate();
        let chained = Payload::Key(Key {
            actor: alice.public(),
            key: second.public(),
            published: "2026-10-23T09:00:00.000Z".into(),
            earlier_keys: EarlierKeys::Retired,
        });
        let chained = SignedObject::seal(
            &alices_key.secret,
            alice.public(),
            Some(alices_key.cert.clone()),
            &chained,
        )
        .unwrap();
        refused(&chained, "only the identity key itself");
        assert!(CertifiedKey::new(second.clone(), chained.clone()).is_err());
        let mut via_chain = SignedObject::seal(
            &second,
            alice.public(),
            Some(Box::new(chained)),
            &note(&alice, "hello"),
        )
        .unwrap();
        refused(&via_chain, "signed by its identity itself");
        // Nor a post in the certificate's place.
        let post = SignedObject::sign(&alice, &note(&alice, "hi")).unwrap();
        via_chain.key = Some(Box::new(post));
        refused(&via_chain, "other than a key certificate");

        // And a certified key can't certify a key through `sign_certified`.
        assert!(SignedObject::sign_certified(&alices_key, &forged).is_err());
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
