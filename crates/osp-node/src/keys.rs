//! Secret keys on disk: 32 bytes as hex, readable only by the owner. And
//! what a node signs with: a signing key its identity certified (ADR 0010),
//! or on a home made before that, the identity key itself.

use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

use anyhow::{Context, Result, ensure};
use iroh::{PublicKey, SecretKey};
use osp_proto::{CertifiedKey, EarlierKeys, Payload, SignedObject};

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
        let path = home.signing_cert();
        let cert = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cert: SignedObject = serde_json::from_str(&cert)
            .with_context(|| format!("{} is not a key certificate", path.display()))?;
        Ok(Self::Certified(CertifiedKey::new(secret, cert)?))
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

/// Makes a signing key for `identity` in `home`, with a certificate dated
/// now, and the backup key that goes with them. Refuses to overwrite any of
/// them. The identity key itself is not written anywhere.
pub(crate) fn create_signing_key(home: &Home, identity: &SecretKey) -> Result<()> {
    let secret = SecretKey::generate();
    let key = CertifiedKey::certify(
        identity,
        secret.clone(),
        osp_proto::now_rfc3339(),
        EarlierKeys::Retired,
    )?;
    save(&home.signing_key(), &secret)?;
    write_new(
        &home.signing_cert(),
        &serde_json::to_vec(key.certificate())?,
    )?;
    save_bytes(
        &home.backup_key(),
        &backup::backup_key(identity, secret.public().as_bytes()),
    )?;
    Ok(())
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
        create_signing_key(home, &identity)?;
    }
    std::fs::remove_file(home.identity_key())
        .with_context(|| format!("deleting {}", home.identity_key().display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protect_finishes_a_run_that_stopped_part_way_but_not_for_another_identity() {
        // A run that made the signing key, then stopped before deleting.
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path());
        let identity = SecretKey::generate();
        save(&home.identity_key(), &identity).unwrap();
        create_signing_key(&home, &identity).unwrap();
        protect(&home).unwrap();
        assert!(!home.identity_key().exists());
        assert_eq!(Signer::load(&home).unwrap().identity(), identity.public());

        // A signing key for some other identity: nothing is deleted.
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path());
        save(&home.identity_key(), &SecretKey::generate()).unwrap();
        create_signing_key(&home, &SecretKey::generate()).unwrap();
        let err = protect(&home).unwrap_err().to_string();
        assert!(err.contains("another identity"), "{err}");
        assert!(home.identity_key().exists());
    }
}
