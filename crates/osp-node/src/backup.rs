//! Identity backup and recovery (ADR 0009).
//!
//! **Recovery words.** The identity key is written as 24 words from the
//! BIP39 English list. The words encode the key's 32 bytes directly (BIP39's
//! entropy encoding, not its seed derivation), so every existing key has
//! words, and the last word carries a checksum that catches a mistyped word.
//!
//! **Backup file.** The profile, own posts in outbox order, what own
//! reshares point at, follows, followers and the media of own posts,
//! encrypted with a key derived from the identity key, so only the
//! recovery words open it. The file is a header (magic, format version, the
//! identity's public key, a random salt), then the contents sealed with
//! ChaCha20-Poly1305 in chunks of 1 MiB. Each chunk's nonce holds its number
//! and whether it is the last, so a chunk can't be changed, moved, dropped
//! or cut off unnoticed. The contents are a manifest (its length in 8 bytes,
//! then JSON), then each media file's bytes in the manifest's order.

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
    keys,
    node::Core,
};

/// 256 bits of key and an 8-bit checksum, at 11 bits per word.
pub const WORD_COUNT: usize = 24;

/// The recovery words of the identity of the node in `home`.
pub fn recovery_words(home: &Path) -> Result<Vec<&'static str>> {
    Ok(words(&keys::load(&Home::new(home).identity_key())?))
}

fn words(key: &SecretKey) -> Vec<&'static str> {
    Mnemonic::from_entropy_in(Language::English, &key.to_bytes())
        .expect("32 bytes is a valid BIP39 entropy length")
        .words()
        .collect()
}

/// The identity key that recovery words stand for. Case, extra spaces and
/// line breaks don't matter.
pub(crate) fn identity_from_words(text: &str) -> Result<SecretKey> {
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
const VERSION: u8 = 1;
/// Magic, version, the identity's public key and the file's salt.
const HEADER_LEN: usize = MAGIC.len() + 1 + 32 + 32;
/// Plaintext bytes in each sealed chunk. Only the last chunk holds fewer.
const CHUNK_LEN: usize = 1 << 20;
const TAG_LEN: usize = 16;
/// Keeps the backup key apart from every other use of the identity key.
const KEY_CONTEXT: &str = "open-social-p2p 2026-10-22 backup file v1";
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

fn header(identity: &PublicKey, salt: &[u8; 32]) -> [u8; HEADER_LEN] {
    let mut header = [0; HEADER_LEN];
    header[..MAGIC.len()].copy_from_slice(MAGIC);
    header[MAGIC.len()] = VERSION;
    header[MAGIC.len() + 1..MAGIC.len() + 33].copy_from_slice(identity.as_bytes());
    header[MAGIC.len() + 33..].copy_from_slice(salt);
    header
}

/// One file's key, from the identity key and the file's random salt, so no
/// two files share a key (and so no two chunks ever share a key and nonce).
fn file_key(identity: &SecretKey, salt: &[u8; 32]) -> LessSafeKey {
    let mut material = [0; 64];
    material[..32].copy_from_slice(&identity.to_bytes());
    material[32..].copy_from_slice(salt);
    let key = blake3::derive_key(KEY_CONTEXT, &material);
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, &key).expect("a 32-byte key"))
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
    header: [u8; HEADER_LEN],
    chunk: Vec<u8>,
    chunk_len: usize,
    sealed: u64,
    /// Bytes written to `out` so far.
    bytes: u64,
}

impl<W: AsyncWrite + Unpin> Sealer<W> {
    async fn new(mut out: W, identity: &SecretKey, chunk_len: usize) -> Result<Self> {
        let mut salt = [0; 32];
        getrandom::fill(&mut salt)
            .map_err(|e| anyhow!("no secure random numbers available: {e}"))?;
        let header = header(&identity.public(), &salt);
        out.write_all(&header).await?;
        Ok(Self {
            out,
            key: file_key(identity, &salt),
            header,
            chunk: Vec::with_capacity(chunk_len + TAG_LEN),
            chunk_len,
            sealed: 0,
            bytes: HEADER_LEN as u64,
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
                Aad::from(&self.header),
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
    header: [u8; HEADER_LEN],
    chunk_len: usize,
    plain: Vec<u8>,
    pos: usize,
    opened: u64,
    done: bool,
}

impl<R: Read> Opener<R> {
    /// Reads the header, and checks the file belongs to `identity`.
    fn new(mut input: R, identity: &SecretKey, chunk_len: usize) -> Result<Self> {
        let mut header = [0; HEADER_LEN];
        input
            .read_exact(&mut header)
            .map_err(|_| anyhow!("this isn't a backup file"))?;
        ensure!(header.starts_with(MAGIC), "this isn't a backup file");
        let version = header[MAGIC.len()];
        ensure!(
            version == VERSION,
            "this backup file has format version {version}; this osp reads version {VERSION}"
        );
        let owner = &header[MAGIC.len() + 1..MAGIC.len() + 33];
        if owner != identity.public().as_bytes() {
            let owner = PublicKey::from_bytes(owner.try_into().expect("32 bytes"))
                .map(|k| k.fmt_short().to_string())
                .unwrap_or_else(|_| "unknown".into());
            bail!("this backup file belongs to another identity ({owner})");
        }
        let salt = header[MAGIC.len() + 33..].try_into().expect("32 bytes");
        Ok(Self {
            input,
            key: file_key(identity, salt),
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
                Aad::from(&self.header),
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
        let me = self.identity.public();
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
        let mut sealer = Sealer::new(BufWriter::new(file), &self.identity, CHUNK_LEN).await?;
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

    /// Small chunks, so the tests cross many chunk boundaries cheaply.
    const SMALL: usize = 16;

    async fn seal(data: &[u8], key: &SecretKey) -> Vec<u8> {
        let mut sealer = Sealer::new(Vec::new(), key, SMALL).await.unwrap();
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
        for len in [0, 1, SMALL - 1, SMALL, SMALL + 1, 3 * SMALL, 3 * SMALL + 5] {
            let data: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let file = seal(&data, &key).await;
            // Only the last chunk is short, and there always is one.
            let chunks = len / SMALL + 1;
            assert_eq!(file.len(), HEADER_LEN + len + chunks * TAG_LEN);
            assert_eq!(open(&file, &key).unwrap(), data, "{len} bytes");
        }
    }

    #[tokio::test]
    async fn any_change_to_a_sealed_file_is_caught() {
        let key = SecretKey::generate();
        let data = vec![7; 3 * SMALL + 5];
        let file = seal(&data, &key).await;

        // Every byte after the header: content, tags, the salt in the header.
        for i in HEADER_LEN - 32..file.len() {
            let mut changed = file.clone();
            changed[i] ^= 1;
            fails_with(open(&changed, &key), "damaged");
        }
        // Chunks moved around.
        let chunk = SMALL + TAG_LEN;
        let mut swapped = file.clone();
        let (a, b) = (HEADER_LEN, HEADER_LEN + chunk);
        let first: Vec<u8> = swapped[a..a + chunk].to_vec();
        swapped.copy_within(b..b + chunk, a);
        swapped[b..b + chunk].copy_from_slice(&first);
        fails_with(open(&swapped, &key), "damaged");
        // Cut off after a whole chunk, or within one.
        let after_chunks = HEADER_LEN + 3 * chunk;
        fails_with(open(&file[..after_chunks], &key), "cut short");
        fails_with(open(&file[..file.len() - 3], &key), "damaged");
        // Bytes added at the end.
        let mut longer = file.clone();
        longer.extend_from_slice(&[0; 9]);
        fails_with(open(&longer, &key), "damaged");
    }

    #[tokio::test]
    async fn a_file_opens_only_for_its_own_identity() {
        let key = SecretKey::generate();
        let file = seal(b"hello", &key).await;
        fails_with(
            open(&file, &SecretKey::generate()),
            "belongs to another identity",
        );
        fails_with(
            open(b"not a backup at all, just text", &key),
            "isn't a backup",
        );
        fails_with(open(b"OSP", &key), "isn't a backup");
        let mut newer = file.clone();
        newer[MAGIC.len()] = VERSION + 1;
        fails_with(open(&newer, &key), "format version");
        // The same contents sealed twice give different files (fresh salt).
        assert_ne!(seal(b"hello", &key).await, file);
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

        let mut sealer = Sealer::new(Vec::new(), &key, CHUNK_LEN).await.unwrap();
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
        let err = crate::Node::restore(&home, &config, &words, Some(&cut)).unwrap_err();
        assert!(format!("{err:#}").contains("cut short"), "{err:#}");
        assert!(!home.exists(), "a failed restore left its home behind");
        crate::Node::restore(&home, &config, &words, Some(&whole)).unwrap();
    }
}
