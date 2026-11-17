//! `osp-list`: makes and signs the project's hide list (ADR 0042 §6).
//!
//! It runs wherever the list key is, never on a node: it reads and writes
//! files, and talks to nothing. Its output, `hide.json`, goes on the site as
//! it is, where nodes fetch it.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use osp_proto::{
    BlobHash, ObjectId, SecretKey,
    lists::{HideKind, HideList, SignedList, entry_hash},
    now_rfc3339,
};

#[derive(Parser)]
#[command(
    name = "osp-list",
    version,
    about = "Make and sign the project's hide list. Run it where the list key is, never on a node"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Make a new list key: the secret goes to OUT, the public key is
    /// printed, to be built into the node and the apps
    Keygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// Put a post or a file on the list, and sign it again
    Add {
        kind: Kind,
        /// The post's ID, or the file's hash (64 hex digits)
        id: String,
        #[command(flatten)]
        files: Files,
    },
    /// Take an entry back, and sign the list again. It stays in the list,
    /// retracted, so nodes that held it learn of it
    Retract {
        kind: Kind,
        id: String,
        #[command(flatten)]
        files: Files,
    },
    /// Whether a post or a file is on the list
    Check {
        kind: Kind,
        id: String,
        #[arg(long)]
        list: PathBuf,
    },
    /// What the list holds: its key, number, date and how many entries
    Show {
        #[arg(long)]
        list: PathBuf,
    },
}

#[derive(clap::Args)]
struct Files {
    /// The list key's secret, from `keygen`
    #[arg(long)]
    key: PathBuf,
    /// The signed list; made if it isn't there yet
    #[arg(long)]
    list: PathBuf,
}

#[derive(Clone, Copy, ValueEnum)]
enum Kind {
    Post,
    File,
}

impl Kind {
    fn entry(self, id: &str) -> Result<(HideKind, [u8; 32])> {
        Ok(match self {
            Self::Post => {
                let id: ObjectId = id.trim().parse().context("a post ID is 64 hex digits")?;
                (HideKind::Post, entry_hash(HideKind::Post, id.as_bytes()))
            }
            Self::File => {
                let hash: BlobHash = id.trim().parse().context("a file hash is 64 hex digits")?;
                (HideKind::File, entry_hash(HideKind::File, hash.as_bytes()))
            }
        })
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Keygen { out } => keygen(&out),
        Command::Add { kind, id, files } => {
            let (kind, h) = kind.entry(&id)?;
            change(
                &files,
                |list, now| list.add(kind, h, now),
                "added",
                "already on",
            )
        }
        Command::Retract { kind, id, files } => {
            let (kind, h) = kind.entry(&id)?;
            change(
                &files,
                |list, now| list.retract(kind, h, now),
                "retracted",
                "not on",
            )
        }
        Command::Check { kind, id, list } => {
            let (kind, h) = kind.entry(&id)?;
            let list = read(&list)?;
            match list.entries.iter().find(|e| e.kind == kind && e.h == h) {
                Some(e) if e.retracted.is_none() => println!("On the list since {}.", e.added),
                Some(e) => println!(
                    "Not on: retracted {}.",
                    e.retracted.as_deref().unwrap_or("")
                ),
                None => println!("Not on the list."),
            }
            Ok(())
        }
        Command::Show { list } => {
            let list = read(&list)?;
            let on = list.active().count();
            println!("Key:       {}", list.key);
            println!("Number:    {}", list.seq);
            println!("Signed:    {}", list.published);
            println!("Entries:   {on} on, {} retracted", list.entries.len() - on);
            Ok(())
        }
    }
}

fn keygen(out: &Path) -> Result<()> {
    ensure!(!out.exists(), "{} is there already", out.display());
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    let key = SecretKey::from_bytes(&bytes);
    let mut file = create_private(out)?;
    writeln!(file, "{}", data_hex(&bytes))?;
    println!(
        "Secret key written to {}. Keep it off every node.",
        out.display()
    );
    println!("Public key: {}", key.public());
    Ok(())
}

#[cfg(unix)]
fn create_private(path: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))
}

fn data_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_key(path: &Path) -> Result<SecretKey> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let text = text.trim();
    ensure!(
        text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()),
        "{} isn't a list key",
        path.display()
    );
    let mut bytes = [0; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[2 * i..2 * i + 2], 16)?;
    }
    Ok(SecretKey::from_bytes(&bytes))
}

/// A list as it says it was signed, checked against the key it names.
fn read(path: &Path) -> Result<HideList> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let signed = SignedList::from_json(&bytes)?;
    signed.verify(&signed.named_key()?)
}

fn change(
    files: &Files,
    apply: impl FnOnce(&mut HideList, &str) -> bool,
    done: &str,
    unchanged: &str,
) -> Result<()> {
    let key = read_key(&files.key)?;
    let mut list = if files.list.exists() {
        let bytes = fs::read(&files.list)?;
        SignedList::from_json(&bytes)?
            .verify(&key.public())
            .context("the list there isn't signed by this key")?
    } else {
        HideList::new(key.public())
    };
    let now = now_rfc3339();
    if !apply(&mut list, &now) {
        bail!("Nothing to do: {unchanged} the list.");
    }
    list.seq += 1;
    list.published = now;
    let signed = SignedList::sign(&list, &key)?;
    let tmp = files.list.with_extension("json.new");
    fs::write(&tmp, signed.to_json())?;
    fs::rename(&tmp, &files.list)?;
    println!(
        "Entry {done}. List number {} written to {}: put it on the site as it is.",
        list.seq,
        files.list.display()
    );
    Ok(())
}
