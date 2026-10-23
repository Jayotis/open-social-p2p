//! Identity backup and recovery (ADR 0009).
//!
//! **Recovery words.** The identity key is written as 24 words from the
//! BIP39 English list. The words encode the key's 32 bytes directly (BIP39's
//! entropy encoding, not its seed derivation), so every existing key has
//! words, and the last word carries a checksum that catches a mistyped word.
//! Since ADR 0010 the words are the only copy of the identity key: a node
//! keeps a signing key the identity certified instead.
//!
//! **Backup file.** The profile, own posts in outbox order, what own
//! reshares point at, follows, followers and the media of own posts,
//! encrypted so that only the recovery words open it. The node keeps a
//! backup key, derived from the identity key and its signing key's public
//! half; the file's header names that signing key, so a restore derives the
//! same backup key from the words. The file is a header (magic, format
//! version, the identity's public key, the signing key's, a random salt),
//! then the contents sealed with ChaCha20-Poly1305 in chunks of 1 MiB. Each
//! chunk's nonce holds its number and whether it is the last, so a chunk
//! can't be changed, moved, dropped or cut off unnoticed. The contents are a
//! manifest (its length in 8 bytes, then JSON), then each media file's bytes
//! in the manifest's order.
//!
//! Version 1 files, made before ADR 0010 (and still by a home that keeps its
//! identity key), have no signing key in the header, and their key comes
//! from the identity key directly.

use std::{
    fs::{self, File},
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use bip39::{Language, Mnemonic};
use iroh::{EndpointAddr, PublicKey, SecretKey};
use osp_proto::{BlobHash, Payload, SignedObject, rpc::BackupInfo};
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufWriter};
use tracing::warn;

use crate::{
    db::{BlobClass, Db},
    home::Home,
    keys::{self, Signer},
    node::Core,
};

/// 256 bits of key and an 8-bit checksum, at 11 bits per word.
pub const WORD_COUNT: usize = 24;

/// The recovery words of a home made before ADR 0010, which still keeps
/// its identity key. Newer homes don't have them.
pub fn recovery_words(home: &Path) -> Result<Vec<&'static str>> {
    let home = Home::new(home);
    ensure!(
        home.identity_key().exists(),
        "this node doesn't keep your identity key, so it can't show your recovery words: \
         they are only where you wrote them down (`osp keys check` checks them)"
    );
    Ok(words(&keys::load(&home.identity_key())?))
}

/// A new identity, as its recovery words. Its key exists nowhere else until
/// a node is made from them (`Node::init_with_words`).
pub fn new_identity_words() -> Vec<&'static str> {
    words(&SecretKey::generate())
}

fn words(key: &SecretKey) -> Vec<&'static str> {
    Mnemonic::from_entropy_in(Language::English, &key.to_bytes())
        .expect("32 bytes is a valid BIP39 entropy length")
        .words()
        .collect()
}

/// Whether recovery words belong to the identity of the node in `home`.
pub fn check_words(home: &Path, words: &str) -> Result<bool> {
    let identity = identity_from_words(words)?;
    Ok(Signer::load(&Home::new(home))?.identity() == identity.public())
}

/// The identity key that recovery words stand for. Case, extra spaces and
/// line breaks don't matter.
pub fn identity_from_words(text: &str) -> Result<SecretKey> {
    let typed: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    if typed.len() != WORD_COUNT {
        bail!("expected {WORD_COUNT} recovery words, got {}", typed.len());
    }
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, &typed.join(" ")).map_err(
        |e| match e {
            bip39::Error::UnknownWord(i) => {
                anyhow!("word {} ({:?}) isn't a recovery word", i + 1, typed[i])
            }
            bip39::Error::InvalidChecksum => {
                anyhow!("the words don't add up; check for a mistyped or swapped word")
            }
            other => anyhow!("not valid recovery words: {other}"),
        },
    )?;
    let (entropy, len) = mnemonic.to_entropy_array();
    let bytes: [u8; 32] = entropy[..len]
        .try_into()
        .map_err(|_| anyhow!("expected {WORD_COUNT} recovery words"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

// ---- the backup file ----

/// Every backup file starts with this, then the format version.
const MAGIC: &[u8; 9] = b"OSPBACKUP";
/// Keyed from the identity key itself. Made before ADR 0010, and still by
/// a home that keeps its identity key.
const VERSION_1: u8 = 1;
/// Keyed from the backup key, which the identity key and the signing key
/// named in the header give.
const VERSION_2: u8 = 2;
/// Plaintext bytes in each sealed chunk. Only the last chunk holds fewer.
const CHUNK_LEN: usize = 1 << 20;
const TAG_LEN: usize = 16;
/// These keep each key derived from the identity key apart from every
/// other use of it.
const FILE_KEY_V1_CONTEXT: &str = "open-social-p2p 2026-10-22 backup file v1";
const BACKUP_KEY_CONTEXT: &str = "open-social-p2p 2026-10-23 backup key";
const FILE_KEY_V2_CONTEXT: &str = "open-social-p2p 2026-10-23 backup file v2";
/// Largest manifest a restore reads. A person's posts take far less.
const MAX_MANIFEST_LEN: u64 = 1 << 30;

/// Everything in a backup file except the bytes of the media.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    /// When the backup was made.
    created: String,
    profile: Option<SignedObject>,
    /// Own posts and reshares, oldest first.
    outbox: Vec<SignedObject>,
    /// What own reshares point at: the posts, and their authors' profiles.
    referenced: Vec<SignedObject>,
    follows: Vec<FollowRecord>,
    followers: Vec<EndpointAddr>,
    /// Media of own posts, in the order their bytes follow the manifest.
    media: Vec<MediaRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FollowRecord {
    actor: PublicKey,
    addr: EndpointAddr,
}

#[derive(Debug, Serialize, Deserialize)]
struct MediaRecord {
    blob: BlobHash,
    size: u64,
}

/// The backup key for a signing key: what a node keeps in `backup.key`,
/// and what a restore derives again from the words and a file's header.
pub(crate) fn backup_key(identity: &SecretKey, signing: &[u8; 32]) -> [u8; 32] {
    derive(BACKUP_KEY_CONTEXT, &identity.to_bytes(), signing)
}

/// BLAKE3's key derivation, from two 32-byte inputs.
fn derive(context: &str, a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut material = [0; 64];
    material[..32].copy_from_slice(a);
    material[32..].copy_from_slice(b);
    blake3::derive_key(context, &material)
}

fn aead(key: &[u8; 32]) -> LessSafeKey {
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).expect("a 32-byte key"))
}

/// What the files a node writes are keyed with.
enum Keying {
    /// Version 1: the identity key itself, on a home that still keeps it.
    Identity(SecretKey),
    /// Version 2: the backup key the node keeps for its signing key.
    BackupKey {
        identity: PublicKey,
        signing: PublicKey,
        key: [u8; 32],
    },
}

impl Keying {
    fn load(home: &Home, signer: &Signer) -> Result<Self> {
        Ok(match signer {
            Signer::Identity(identity) => Self::Identity(identity.clone()),
            Signer::Certified(key) => Self::BackupKey {
                identity: key.identity(),
                signing: key.public(),
                key: keys::load_bytes(&home.backup_key())?,
            },
        })
    }

    /// A new file's header, and its key. The random salt gives each file
    /// its own key, so no two chunks ever share a key and nonce.
    fn new_file(&self, salt: &[u8; 32]) -> (Vec<u8>, LessSafeKey) {
        let mut header = MAGIC.to_vec();
        let key = match self {
            Self::Identity(identity) => {
                header.push(VERSION_1);
                header.extend_from_slice(identity.public().as_bytes());
                derive(FILE_KEY_V1_CONTEXT, &identity.to_bytes(), salt)
            }
            Self::BackupKey {
                identity,
                signing,
                key,
            } => {
                header.push(VERSION_2);
                header.extend_from_slice(identity.as_bytes());
                header.extend_from_slice(signing.as_bytes());
                derive(FILE_KEY_V2_CONTEXT, key, salt)
            }
        };
        header.extend_from_slice(salt);
        (header, aead(&key))
    }
}

/// Chunk `n`'s nonce: its number, and whether it is the last.
fn nonce(n: u64, last: bool) -> Nonce {
    let mut nonce = [0; 12];
    nonce[..8].copy_from_slice(&n.to_be_bytes());
    nonce[11] = last as u8;
    Nonce::assume_unique_for_key(nonce)
}

/// Writes a backup file: the header, then whatever is written to it,
/// sealed a chunk at a time.
struct Sealer<W> {
    out: W,
    key: LessSafeKey,
    header: Vec<u8>,
    chunk: Vec<u8>,
    chunk_len: usize,
    sealed: u64,
    /// Bytes written to `out` so far.
    bytes: u64,
}

impl<W: AsyncWrite + Unpin> Sealer<W> {
    async fn new(mut out: W, keying: &Keying, chunk_len: usize) -> Result<Self> {
        let mut salt = [0; 32];
        getrandom::fill(&mut salt)
            .map_err(|e| anyhow!("no secure random numbers available: {e}"))?;
        let (header, key) = keying.new_file(&salt);
        out.write_all(&header).await?;
        Ok(Self {
            out,
            key,
            bytes: header.len() as u64,
            header,
            chunk: Vec::with_capacity(chunk_len + TAG_LEN),
            chunk_len,
            sealed: 0,
        })
    }

    async fn write(&mut self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            let take = (self.chunk_len - self.chunk.len()).min(data.len());
            self.chunk.extend_from_slice(&data[..take]);
            data = &data[take..];
            // A full chunk is never the last one: that holds what's left,
            // perhaps nothing.
            if self.chunk.len() == self.chunk_len {
                self.seal(false).await?;
            }
        }
        Ok(())
    }

    /// Seals the last chunk. Returns the output and the file's size.
    async fn finish(mut self) -> Result<(W, u64)> {
        self.seal(true).await?;
        self.out.flush().await?;
        Ok((self.out, self.bytes))
    }

    async fn seal(&mut self, last: bool) -> Result<()> {
        let fresh = Vec::with_capacity(self.chunk_len + TAG_LEN);
        let mut chunk = std::mem::replace(&mut self.chunk, fresh);
        self.key
            .seal_in_place_append_tag(
                nonce(self.sealed, last),
                Aad::from(self.header.as_slice()),
                &mut chunk,
            )
            .map_err(|_| anyhow!("encrypting the backup failed"))?;
        self.out.write_all(&chunk).await?;
        self.sealed += 1;
        self.bytes += chunk.len() as u64;
        Ok(())
    }
}

/// Reads a backup file, opening each chunk as it comes. A read fails if the
/// file was changed in any way, including being cut short.
struct Opener<R> {
    input: R,
    key: LessSafeKey,
    header: Vec<u8>,
    chunk_len: usize,
    plain: Vec<u8>,
    pos: usize,
    opened: u64,
    done: bool,
}

impl<R: Read> Opener<R> {
    /// Reads the header, and checks the file belongs to `identity`.
    fn new(mut input: R, identity: &SecretKey, chunk_len: usize) -> Result<Self> {
        let mut header = vec![0; MAGIC.len() + 1];
        input
            .read_exact(&mut header)
            .map_err(|_| anyhow!("this isn't a backup file"))?;
        ensure!(header.starts_with(MAGIC), "this isn't a backup file");
        let version = header[MAGIC.len()];
        ensure!(
            version == VERSION_1 || version == VERSION_2,
            "this backup file has format version {version}; this osp reads versions \
             {VERSION_1} and {VERSION_2}"
        );
        let owner = read_32(&mut input, &mut header)?;
        if owner != *identity.public().as_bytes() {
            let owner = PublicKey::from_bytes(&owner)
                .map(|k| k.fmt_short().to_string())
                .unwrap_or_else(|_| "unknown".into());
            bail!("this backup file belongs to another identity ({owner})");
        }
        let key = if version == VERSION_1 {
            let salt = read_32(&mut input, &mut header)?;
            derive(FILE_KEY_V1_CONTEXT, &identity.to_bytes(), &salt)
        } else {
            let signing = read_32(&mut input, &mut header)?;
            let salt = read_32(&mut input, &mut header)?;
            derive(FILE_KEY_V2_CONTEXT, &backup_key(identity, &signing), &salt)
        };
        Ok(Self {
            input,
            key: aead(&key),
            header,
            chunk_len,
            plain: Vec::new(),
            pos: 0,
            opened: 0,
            done: false,
        })
    }

    fn open_next(&mut self) -> Result<()> {
        let mut chunk = vec![0; self.chunk_len + TAG_LEN];
        let n = read_up_to(&mut self.input, &mut chunk)?;
        // Only the last chunk is shorter than a full one.
        let last = n < chunk.len();
        ensure!(n >= TAG_LEN, "the backup file is cut short");
        chunk.truncate(n);
        let len = self
            .key
            .open_in_place(
                nonce(self.opened, last),
                Aad::from(self.header.as_slice()),
                &mut chunk,
            )
            .map_err(|_| anyhow!("the backup file is damaged, or was changed"))?
            .len();
        chunk.truncate(len);
        self.plain = chunk;
        self.pos = 0;
        self.opened += 1;
        self.done = last;
        Ok(())
    }
}

impl<R: Read> Read for Opener<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos == self.plain.len() {
            if self.done {
                return Ok(0);
            }
            self.open_next().map_err(io::Error::other)?;
        }
        let n = out.len().min(self.plain.len() - self.pos);
        out[..n].copy_from_slice(&self.plain[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Reads the next 32 bytes of a header, adding them to `header`.
fn read_32(input: &mut impl Read, header: &mut Vec<u8>) -> Result<[u8; 32]> {
    let mut bytes = [0; 32];
    input
        .read_exact(&mut bytes)
        .map_err(|_| anyhow!("the backup file is cut short"))?;
    header.extend_from_slice(&bytes);
    Ok(bytes)
}

/// Reads until `buf` is full or the input ends. Returns how much was read.
fn read_up_to(input: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match input.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

impl Core {
    /// Writes a backup file of this identity. With `media`, it includes the
    /// media of own posts. It is written beside `path` and then renamed, so
    /// a backup that fails part way never replaces a good one.
    pub(crate) async fn export_backup(&self, path: &Path, media: bool) -> Result<BackupInfo> {
        let me = self.signer.identity();
        let mut manifest = Manifest {
            created: osp_proto::now_rfc3339(),
            profile: self.db.profile(&me)?,
            ..Default::default()
        };
        for (_, obj) in self.db.outbox_since(0, u32::MAX)? {
            if let Ok(Payload::Announce(a)) = obj.verify() {
                let post = self.db.get_object(&a.object)?;
                let author = self.db.profile(&a.object_actor)?;
                for obj in post.into_iter().chain(author) {
                    if !manifest.referenced.contains(&obj) {
                        manifest.referenced.push(obj);
                    }
                }
            }
            manifest.outbox.push(obj);
        }
        for follow in self.db.follows()? {
            manifest.follows.push(FollowRecord {
                actor: follow.actor,
                addr: follow.addr,
            });
        }
        manifest.followers = self.db.followers()?;
        let mut media_missing = 0;
        if media {
            for (blob, class) in self.db.blobs()? {
                if class != BlobClass::Own {
                    continue;
                }
                match self.store.stored_size(&blob).await? {
                    Some(size) => manifest.media.push(MediaRecord { blob, size }),
                    None => {
                        warn!(blob = %blob.fmt_short(), "media of one of your posts is missing; it isn't in the backup");
                        media_missing += 1;
                    }
                }
            }
        }

        let part = PathBuf::from(format!("{}.part", path.display()));
        let written = self.write_backup(&part, &manifest).await;
        let bytes = match written {
            Ok(bytes) => bytes,
            Err(e) => {
                let _ = fs::remove_file(&part);
                return Err(e);
            }
        };
        fs::rename(&part, path).with_context(|| format!("renaming {}", part.display()))?;
        Ok(BackupInfo {
            posts: manifest.outbox.len(),
            follows: manifest.follows.len(),
            followers: manifest.followers.len(),
            media_files: manifest.media.len(),
            media_missing,
            bytes,
        })
    }

    /// Writes the backup file: the manifest, then the media. Returns its size.
    async fn write_backup(&self, path: &Path, manifest: &Manifest) -> Result<u64> {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .await
            .with_context(|| format!("creating {}", path.display()))?;
        let keying = Keying::load(&self.home, &self.signer)?;
        let mut sealer = Sealer::new(BufWriter::new(file), &keying, CHUNK_LEN).await?;
        let json = serde_json::to_vec(manifest)?;
        sealer.write(&(json.len() as u64).to_be_bytes()).await?;
        sealer.write(&json).await?;
        for m in &manifest.media {
            // A slice at a time, so a large video never sits in memory whole.
            let mut offset = 0;
            while offset < m.size {
                let len = (m.size - offset).min(CHUNK_LEN as u64);
                let mut bytes = Vec::with_capacity(len as usize);
                self.store
                    .write_range(&m.blob, offset, len, &mut bytes)
                    .await?;
                sealer.write(&bytes).await?;
                offset += len;
            }
        }
        let (out, bytes) = sealer.finish().await?;
        out.into_inner().sync_all().await?;
        Ok(bytes)
    }
}

/// An opened backup file and its manifest, checked against the identity
/// before anything is restored from it.
pub(crate) struct BackupFile {
    input: Opener<BufReader<File>>,
    manifest: Manifest,
    bytes: u64,
}

impl BackupFile {
    pub(crate) fn open(path: &Path, identity: &SecretKey) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let bytes = file.metadata()?.len();
        let mut input = Opener::new(BufReader::new(file), identity, CHUNK_LEN)?;
        let manifest = read_manifest(&mut input)?;
        Ok(Self {
            input,
            manifest,
            bytes,
        })
    }

    /// The date of the newest key certificate in the backup, so a restore
    /// can date its new one later (ADR 0010).
    pub(crate) fn newest_certificate_ms(&self) -> Option<i64> {
        let m = &self.manifest;
        m.profile
            .iter()
            .chain(&m.outbox)
            .filter_map(|obj| match obj.verify_with_key() {
                Ok((_, Some(key))) => osp_proto::parse_rfc3339_ms(&key.published),
                _ => None,
            })
            .max()
    }

    /// The name in the backed-up profile, if it has one.
    pub(crate) fn name(&self) -> Option<String> {
        match self.manifest.profile.as_ref()?.verify() {
            Ok(Payload::Person(person)) => Some(person.name),
            _ => None,
        }
    }

    /// Restores the backup into a new home: posts in their original order,
    /// follows (each one starts over, as a new follow does), followers, and
    /// media, which the node imports when it starts (`Home::restored_media`).
    pub(crate) fn restore(mut self, home: &Home, identity: &SecretKey) -> Result<BackupInfo> {
        let me = identity.public();
        let m = &self.manifest;
        let db = Db::open(&home.db())?;
        if let Some(profile) = &m.profile {
            let Payload::Person(person) = profile.verify()? else {
                bail!("the backed-up profile isn't a profile");
            };
            ensure!(
                person.actor == me,
                "the backed-up profile is someone else's"
            );
            db.upsert_profile(&me, &person.name, profile)?;
        }
        for obj in &m.outbox {
            let payload = obj.verify().context("a backed-up post is invalid")?;
            ensure!(payload.actor() == me, "a backed-up post is someone else's");
            db.insert_object(obj, &payload)?;
            db.append_outbox(&obj.id())?;
        }
        for obj in &m.referenced {
            let Ok(payload) = obj.verify() else {
                warn!("skipping an invalid reshared post in the backup");
                continue;
            };
            match &payload {
                Payload::Person(person) => db.upsert_profile(&person.actor, &person.name, obj)?,
                _ => {
                    db.insert_object(obj, &payload)?;
                }
            }
        }
        for follow in &m.follows {
            db.upsert_follow(&follow.actor, &follow.addr)?;
        }
        // Each counts as heard from now, so it has the usual 30 days to
        // hear that we moved before it's forgotten as silent.
        for addr in &m.followers {
            db.add_follower(addr)?;
        }
        let dir = home.restored_media();
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        for media in &m.media {
            copy_media(&mut self.input, media, &dir.join(media.blob.to_string()))?;
        }
        // Reading on to the end opens the last chunk, which shows nothing
        // was cut off.
        let mut rest = [0; 1];
        ensure!(
            self.input.read(&mut rest)? == 0,
            "the backup file holds more than its manifest lists"
        );
        Ok(BackupInfo {
            posts: m.outbox.len(),
            follows: m.follows.len(),
            followers: m.followers.len(),
            media_files: m.media.len(),
            media_missing: 0,
            bytes: self.bytes,
        })
    }
}

fn read_manifest(input: &mut impl Read) -> Result<Manifest> {
    let mut len = [0; 8];
    input.read_exact(&mut len)?;
    let len = u64::from_be_bytes(len);
    ensure!(
        len <= MAX_MANIFEST_LEN,
        "the backup's manifest is too large"
    );
    let mut json = vec![0; len as usize];
    input.read_exact(&mut json)?;
    serde_json::from_slice(&json).context("the backup's manifest is unreadable")
}

/// Copies one media file out of the backup, checking it against its hash.
fn copy_media(input: &mut impl Read, media: &MediaRecord, to: &Path) -> Result<()> {
    let mut file = File::create(to).with_context(|| format!("creating {}", to.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0; 64 << 10];
    let mut left = media.size;
    while left > 0 {
        let n = buf.len().min(left as usize);
        input.read_exact(&mut buf[..n])?;
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])?;
        left -= n as u64;
    }
    file.sync_all()?;
    ensure!(
        hasher.finalize().as_bytes() == media.blob.as_bytes(),
        "media {} in the backup doesn't match its hash",
        media.blob.fmt_short()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use osp_proto::EarlierKeys;

    #[test]
    fn words_and_key_round_trip() {
        let key = SecretKey::generate();
        let words = words(&key);
        assert_eq!(words.len(), WORD_COUNT);
        let back = identity_from_words(&words.join(" ")).unwrap();
        assert_eq!(back.to_bytes(), key.to_bytes());
    }

    #[test]
    fn words_are_the_bip39_encoding_of_the_key_bytes() {
        // Test vectors from BIP39: the words for 32 bytes of 0x00 and of 0xff.
        let zeros = words(&SecretKey::from_bytes(&[0; 32])).join(" ");
        assert_eq!(zeros, format!("{}art", "abandon ".repeat(23)));
        let ones = words(&SecretKey::from_bytes(&[0xff; 32])).join(" ");
        assert_eq!(ones, format!("{}vote", "zoo ".repeat(23)));
    }

    #[test]
    fn case_and_spacing_dont_matter() {
        let key = SecretKey::generate();
        let words = words(&key);
        let messy = format!(
            "  {}\n{}  ",
            words[..12].join("  ").to_uppercase(),
            words[12..].join("\t")
        );
        let back = identity_from_words(&messy).unwrap();
        assert_eq!(back.to_bytes(), key.to_bytes());
    }

    #[test]
    fn wrong_words_are_refused() {
        let mut words = words(&SecretKey::from_bytes(&[7; 32]));
        let err = identity_from_words(&words[..23].join(" ")).unwrap_err();
        assert!(err.to_string().contains("got 23"), "{err}");

        let mut typo = words.clone();
        typo[4] = "abandonn";
        let err = identity_from_words(&typo.join(" ")).unwrap_err();
        assert!(err.to_string().contains("word 5"), "{err}");

        // Every word is on the list, but two are swapped: the checksum catches it.
        assert_ne!(words[3], words[9]);
        words.swap(3, 9);
        let err = identity_from_words(&words.join(" ")).unwrap_err();
        assert!(err.to_string().contains("don't add up"), "{err}");
    }

    /// A key change gives a new backup key, so whoever copied the old one
    /// can't open files made after it (ADR 0010).
    #[test]
    fn each_signing_key_has_its_own_backup_key() {
        let identity = SecretKey::generate();
        let first = SecretKey::generate().public();
        let second = SecretKey::generate().public();
        let key = backup_key(&identity, first.as_bytes());
        assert_eq!(key, backup_key(&identity, first.as_bytes()));
        assert_ne!(key, backup_key(&identity, second.as_bytes()));
        assert_ne!(key, backup_key(&SecretKey::generate(), first.as_bytes()));
    }

    /// Small chunks, so the tests cross many chunk boundaries cheaply.
    const SMALL: usize = 16;

    /// Both ways a node keys its files: version 2, from the backup key for
    /// a signing key, and version 1, from the identity key itself.
    fn keyings(identity: &SecretKey) -> [Keying; 2] {
        let signing = SecretKey::generate().public();
        [
            Keying::BackupKey {
                identity: identity.public(),
                signing,
                key: backup_key(identity, signing.as_bytes()),
            },
            Keying::Identity(identity.clone()),
        ]
    }

    fn header_len(keying: &Keying) -> usize {
        match keying {
            Keying::BackupKey { .. } => MAGIC.len() + 1 + 3 * 32,
            Keying::Identity(_) => MAGIC.len() + 1 + 2 * 32,
        }
    }

    async fn seal(data: &[u8], keying: &Keying) -> Vec<u8> {
        let mut sealer = Sealer::new(Vec::new(), keying, SMALL).await.unwrap();
        // In odd-sized pieces, so writes straddle chunks.
        for piece in data.chunks(7) {
            sealer.write(piece).await.unwrap();
        }
        let (file, bytes) = sealer.finish().await.unwrap();
        assert_eq!(bytes, file.len() as u64);
        file
    }

    fn open(file: &[u8], key: &SecretKey) -> Result<Vec<u8>> {
        let mut opener = Opener::new(file, key, SMALL)?;
        let mut data = Vec::new();
        opener.read_to_end(&mut data)?;
        Ok(data)
    }

    fn fails_with(result: Result<Vec<u8>>, expected: &str) {
        let err = result.expect_err("should fail").to_string();
        assert!(err.contains(expected), "{err:?}, not {expected:?}");
    }

    #[tokio::test]
    async fn sealed_contents_open_unchanged_whatever_their_length() {
        let key = SecretKey::generate();
        for keying in keyings(&key) {
            for len in [0, 1, SMALL - 1, SMALL, SMALL + 1, 3 * SMALL, 3 * SMALL + 5] {
                let data: Vec<u8> = (0..len).map(|i| i as u8).collect();
                let file = seal(&data, &keying).await;
                // Only the last chunk is short, and there always is one.
                let chunks = len / SMALL + 1;
                assert_eq!(file.len(), header_len(&keying) + len + chunks * TAG_LEN);
                assert_eq!(open(&file, &key).unwrap(), data, "{len} bytes");
            }
        }
    }

    #[tokio::test]
    async fn any_change_to_a_sealed_file_is_caught() {
        let key = SecretKey::generate();
        for keying in keyings(&key) {
            let data = vec![7; 3 * SMALL + 5];
            let file = seal(&data, &keying).await;
            let header = header_len(&keying);

            // Every byte after the identity: in the header, the signing key
            // and the salt; then content and tags.
            for i in MAGIC.len() + 1 + 32..file.len() {
                let mut changed = file.clone();
                changed[i] ^= 1;
                fails_with(open(&changed, &key), "damaged");
            }
            // Chunks moved around.
            let chunk = SMALL + TAG_LEN;
            let mut swapped = file.clone();
            let (a, b) = (header, header + chunk);
            let first: Vec<u8> = swapped[a..a + chunk].to_vec();
            swapped.copy_within(b..b + chunk, a);
            swapped[b..b + chunk].copy_from_slice(&first);
            fails_with(open(&swapped, &key), "damaged");
            // Cut off after a whole chunk, or within one, or in the header.
            let after_chunks = header + 3 * chunk;
            fails_with(open(&file[..after_chunks], &key), "cut short");
            fails_with(open(&file[..file.len() - 3], &key), "damaged");
            fails_with(open(&file[..header - 1], &key), "cut short");
            // Bytes added at the end.
            let mut longer = file.clone();
            longer.extend_from_slice(&[0; 9]);
            fails_with(open(&longer, &key), "damaged");
        }
    }

    #[tokio::test]
    async fn a_file_opens_only_for_its_own_identity() {
        let key = SecretKey::generate();
        for keying in keyings(&key) {
            let file = seal(b"hello", &keying).await;
            fails_with(
                open(&file, &SecretKey::generate()),
                "belongs to another identity",
            );
            // The same contents sealed twice give different files (fresh salt).
            assert_ne!(seal(b"hello", &keying).await, file);
        }
        fails_with(
            open(b"not a backup at all, just text", &key),
            "isn't a backup",
        );
        fails_with(open(b"OSP", &key), "isn't a backup");
        let mut newer = seal(b"hello", &keyings(&key)[0]).await;
        newer[MAGIC.len()] = VERSION_2 + 1;
        fails_with(open(&newer, &key), "format version");
    }

    /// A backup whose contents end exactly at a chunk boundary, so its last
    /// chunk is empty: cutting that chunk off is caught only by reading on
    /// to the end, which a restore must do.
    #[tokio::test]
    async fn a_restore_refuses_a_backup_cut_after_a_whole_chunk() {
        let key = SecretKey::generate();
        let mut manifest = Manifest::default();
        let base = serde_json::to_vec(&manifest).unwrap().len();
        manifest.created = "x".repeat(CHUNK_LEN - 8 - base);
        let json = serde_json::to_vec(&manifest).unwrap();
        assert_eq!(8 + json.len(), CHUNK_LEN);

        let keying = &keyings(&key)[0];
        let mut sealer = Sealer::new(Vec::new(), keying, CHUNK_LEN).await.unwrap();
        sealer
            .write(&(json.len() as u64).to_be_bytes())
            .await
            .unwrap();
        sealer.write(&json).await.unwrap();
        let (file, _) = sealer.finish().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (whole, cut) = (dir.path().join("whole"), dir.path().join("cut"));
        fs::write(&whole, &file).unwrap();
        fs::write(&cut, &file[..file.len() - TAG_LEN]).unwrap();

        // The manifest reads fine; only the end shows the file was cut. The
        // restore then removes the home it had started, so it can run again.
        let (home, config) = (dir.path().join("home"), Config::local_test("Test"));
        let words = words(&key).join(" ");
        let err = crate::Node::restore(&home, &config, &words, Some(&cut), EarlierKeys::Retired)
            .unwrap_err();
        assert!(format!("{err:#}").contains("cut short"), "{err:#}");
        assert!(!home.exists(), "a failed restore left its home behind");
        crate::Node::restore(&home, &config, &words, Some(&whole), EarlierKeys::Retired).unwrap();
    }
}
