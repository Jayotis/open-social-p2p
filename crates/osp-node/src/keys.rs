//! Secret keys on disk: 32 bytes as hex, readable only by the owner.

use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

use anyhow::{Context, Result};
use iroh::SecretKey;

/// Generates a key and writes it to `path`. Refuses to overwrite an existing key.
pub fn create(path: &Path) -> Result<SecretKey> {
    let key = SecretKey::generate();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(data_encoding::HEXLOWER.encode(&key.to_bytes()).as_bytes())?;
    Ok(key)
}

pub fn load(path: &Path) -> Result<SecretKey> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let bytes = data_encoding::HEXLOWER
        .decode(text.trim().as_bytes())
        .with_context(|| format!("{} is not a hex key", path.display()))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("{} must hold 32 bytes", path.display()))?;
    Ok(SecretKey::from_bytes(&bytes))
}
