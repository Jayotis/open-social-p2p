//! Secret keys on disk: 32 bytes as hex, readable only by the owner. And
//! what a node signs with: a signing key its identity certified (ADR 0010),
//! or on a home made before that, the identity key itself.

use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use iroh::{PublicKey, SecretKey};
use osp_proto::{CertifiedKey, EarlierKeys, Key, Payload, SignedObject};

use crate::{backup, home::Home};

/// Generates a key and writes it to `path`. Refuses to overwrite an existing key.
pub fn create(path: &Path) -> Result<SecretKey> {
    let key = SecretKey::generate();
    save(path, &key)?;
    Ok(key)
}

/// Writes `key` to `path`. Refuses to overwrite an existing key.
pub fn save(path: &Path, key: &SecretKey) -> Result<()> {
    save_bytes(path, &key.to_bytes())
}

pub fn load(path: &Path) -> Result<SecretKey> {
    Ok(SecretKey::from_bytes(&load_bytes(path)?))
}

/// Writes 32 secret bytes to `path`, readable only by the owner. Refuses to
/// overwrite an existing file.
pub(crate) fn save_bytes(path: &Path, bytes: &[u8; 32]) -> Result<()> {
    write_new(path, data_encoding::HEXLOWER.encode(bytes).as_bytes())
}

pub(crate) fn load_bytes(path: &Path) -> Result<[u8; 32]> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let bytes = data_encoding::HEXLOWER
        .decode(text.trim().as_bytes())
        .with_context(|| format!("{} is not a hex key", path.display()))?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("{} must hold 32 bytes", path.display()))
}

/// Writes `contents` to `path`, readable only by the owner: beside it first,
/// then renamed over it, so a reader sees the old file or the new one.
fn write_over(path: &Path, contents: &[u8]) -> Result<()> {
    let mut name = path.file_name().context("no file name")?.to_owned();
    name.push(".new");
    let new = PathBuf::from(path).with_file_name(name);
    let _ = std::fs::remove_file(&new);
    write_new(&new, contents)?;
    std::fs::rename(&new, path).with_context(|| format!("replacing {}", path.display()))
}

/// Creates `path`, readable only by the owner, and writes `contents`.
fn write_new(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

/// What a node signs its objects with.
pub(crate) enum Signer {
    /// A signing key its identity certified (ADR 0010).
    Certified(CertifiedKey),
    /// The identity key itself, on a home made before ADR 0010, until
    /// `osp keys protect` takes it off the node.
    Identity(SecretKey),
}

impl Signer {
    /// The signing key and its certificate if the home has them, or else
    /// the identity key.
    pub fn load(home: &Home) -> Result<Self> {
        if !home.signing_key().exists() {
            return Ok(Self::Identity(load(&home.identity_key())?));
        }
        let secret = load(&home.signing_key())?;
        let cert = load_certificate(home)?;
        let key = CertifiedKey::new(secret, cert).with_context(|| {
            format!(
                "{} doesn't match {}; if a key change stopped part way, run `osp keys replace` again",
                home.signing_cert().display(),
                home.signing_key().display()
            )
        })?;
        Ok(Self::Certified(key))
    }

    /// The identity this node signs for.
    pub fn identity(&self) -> PublicKey {
        match self {
            Self::Certified(key) => key.identity(),
            Self::Identity(key) => key.public(),
        }
    }

    pub fn sign(&self, payload: &Payload) -> Result<SignedObject> {
        match self {
            Self::Certified(key) => SignedObject::sign_certified(key, payload),
            Self::Identity(key) => SignedObject::sign(key, payload),
        }
    }

    /// The certificate this node's objects carry; none when the identity
    /// key signs them itself.
    pub fn certificate(&self) -> Option<&SignedObject> {
        match self {
            Self::Certified(key) => Some(key.certificate()),
            Self::Identity(_) => None,
        }
    }
}

fn load_certificate(home: &Home) -> Result<SignedObject> {
    let path = home.signing_cert();
    let cert =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&cert)
        .with_context(|| format!("{} is not a key certificate", path.display()))
}

/// What the home's key certificate says, checked against its signature.
pub(crate) fn read_certificate(home: &Home) -> Result<Key> {
    match load_certificate(home)?.verify()? {
        Payload::Key(key) => Ok(key),
        _ => bail!("{} is not a key certificate", home.signing_cert().display()),
    }
}

/// The date for a new key certificate: now, or just after `after`, the
/// newest certificate known for the identity, if this computer's clock is
/// behind it. Followers take the newer certificate (ADR 0010).
pub(crate) fn certificate_date(after: Option<i64>) -> String {
    let mut at = SystemTime::now();
    if let Some(ms) = after {
        at = at.max(UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64 + 1));
    }
    humantime::format_rfc3339_millis(at).to_string()
}

/// A new signing key for `identity`, certified with `published` and
/// `earlier_keys`.
pub(crate) struct NewKey {
    pub published: String,
    pub earlier_keys: EarlierKeys,
}

/// Makes a signing key for `identity` in `home`, and the backup key that
/// goes with it. Refuses to overwrite any of them. The identity key itself
/// is not written anywhere.
pub(crate) fn create_signing_key(home: &Home, identity: &SecretKey, new: NewKey) -> Result<()> {
    write_signing_key(home, identity, new, write_new)
}

fn write_signing_key(
    home: &Home,
    identity: &SecretKey,
    new: NewKey,
    write: fn(&Path, &[u8]) -> Result<()>,
) -> Result<()> {
    let secret = SecretKey::generate();
    let key = CertifiedKey::certify(identity, secret.clone(), new.published, new.earlier_keys)?;
    let backup_key = backup::backup_key(identity, secret.public().as_bytes());
    let hex = |bytes: &[u8]| data_encoding::HEXLOWER.encode(bytes).into_bytes();
    write(&home.backup_key(), &hex(&backup_key))?;
    // The key and its certificate one straight after the other: a node
    // that finds them apart refuses to start, and says to run this again.
    write(&home.signing_key(), &hex(&secret.to_bytes()))?;
    write(
        &home.signing_cert(),
        &serde_json::to_vec(key.certificate())?,
    )
}

/// Replaces the home's signing key with a new one, whose certificate
/// revokes the keys before it and is dated after the current one. The node
/// must not be running; when it next starts, it signs its outbox again with
/// the new key and tells its followers.
pub(crate) fn replace(home: &Home, identity: &SecretKey) -> Result<()> {
    let current = read_certificate(home)?;
    ensure!(
        current.actor == identity.public(),
        "these recovery words belong to another identity, not this node's"
    );
    let new = NewKey {
        published: certificate_date(osp_proto::parse_rfc3339_ms(&current.published)),
        earlier_keys: EarlierKeys::Revoked,
    };
    write_signing_key(home, identity, new, write_over)
}

/// Takes the identity key off a home made before ADR 0010: makes a signing
/// key, its certificate and the backup key, then deletes `identity.key`.
/// The node must not be running. Nothing it signed needs signing again.
pub(crate) fn protect(home: &Home) -> Result<()> {
    let identity = load(&home.identity_key())?;
    // A run that stopped part way left the signing key in place; check it's
    // for this identity before deleting anything.
    if home.signing_key().exists() {
        ensure!(
            Signer::load(home)?.identity() == identity.public(),
            "{} is for another identity than {}",
            home.signing_key().display(),
            home.identity_key().display()
        );
    } else {
        let new = NewKey {
            published: osp_proto::now_rfc3339(),
            earlier_keys: EarlierKeys::Retired,
        };
        create_signing_key(home, &identity, new)?;
    }
    std::fs::remove_file(home.identity_key())
        .with_context(|| format!("deleting {}", home.identity_key().display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_retired() -> NewKey {
        NewKey {
            published: osp_proto::now_rfc3339(),
            earlier_keys: EarlierKeys::Retired,
        }
    }

    #[test]
    fn protect_finishes_a_run_that_stopped_part_way_but_not_for_another_identity() {
        // A run that made the signing key, then stopped before deleting.
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path());
        let identity = SecretKey::generate();
        save(&home.identity_key(), &identity).unwrap();
        create_signing_key(&home, &identity, now_retired()).unwrap();
        protect(&home).unwrap();
        assert!(!home.identity_key().exists());
        assert_eq!(Signer::load(&home).unwrap().identity(), identity.public());

        // A signing key for some other identity: nothing is deleted.
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path());
        save(&home.identity_key(), &SecretKey::generate()).unwrap();
        create_signing_key(&home, &SecretKey::generate(), now_retired()).unwrap();
        let err = protect(&home).unwrap_err().to_string();
        assert!(err.contains("another identity"), "{err}");
        assert!(home.identity_key().exists());
    }
}
