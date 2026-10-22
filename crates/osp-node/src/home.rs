//! Layout of a node's home directory.

use std::path::{Path, PathBuf};

/// A node keeps everything in one directory, so several nodes can run on one
/// machine side by side (useful for testing).
#[derive(Debug, Clone)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// The author key that signs posts (ADR 0004).
    pub fn identity_key(&self) -> PathBuf {
        self.root.join("identity.key")
    }

    /// The transport key that identifies this machine on the network.
    pub fn node_key(&self) -> PathBuf {
        self.root.join("node.key")
    }

    pub fn db(&self) -> PathBuf {
        self.root.join("osp.db")
    }

    pub fn blobs(&self) -> PathBuf {
        self.root.join("blobs")
    }

    /// Scratch space for transcoding.
    pub fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    /// Media a restore left for the node to import when it starts (ADR 0009).
    pub fn restored_media(&self) -> PathBuf {
        self.root.join("restored-media")
    }

    /// Unix socket the local command-line tool talks to.
    pub fn socket(&self) -> PathBuf {
        self.root.join("osp.sock")
    }

    /// Held locked by the running node, so a second one can't start on the same home.
    pub fn lock(&self) -> PathBuf {
        self.root.join("lock")
    }

    pub fn is_initialized(&self) -> bool {
        self.config().exists() && self.identity_key().exists()
    }
}
