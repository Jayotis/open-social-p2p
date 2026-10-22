//! Node configuration, stored as `config.toml` in the home directory.

use std::{net::SocketAddr, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Display name in this identity's profile.
    pub name: String,
    pub network: NetworkConfig,
    pub sync: SyncConfig,
    pub media: MediaConfig,
    pub storage: StorageConfig,
    pub serving: ServingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    /// Which relays to use when a direct connection isn't possible.
    pub relay: RelayChoice,
    /// Publish this node's relay URL to n0's DNS service and look up other
    /// nodes there, so peers can be reached by ID alone.
    pub address_lookup: bool,
    /// Bind to this address only (for example `127.0.0.1:0` in tests).
    /// By default the node binds every interface on a random port.
    pub bind: Option<SocketAddr>,
    /// Connect only through this node's own relays, never directly, so
    /// other nodes and phones never learn its IP address. The relay
    /// operator still sees it. Slower, and every byte passes through the
    /// relay (ADR 0007).
    pub relay_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayChoice {
    /// n0's public relays. Fine for development; see ADR 0002 on privacy.
    Default,
    /// No relays: only direct connections. Tickets then include IP addresses.
    Disabled,
    /// Your own relay servers.
    Custom(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncConfig {
    /// How often to poll followed nodes, in addition to their push notifications.
    pub interval_secs: u64,
    /// Download media of new posts from followed people ahead of time, so the
    /// phone can load it from home quickly.
    pub prefetch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaConfig {
    pub ffmpeg: String,
    pub ffprobe: String,
    /// Longest video accepted for publishing.
    pub max_video_secs: u64,
    /// Stop transcoding a video that takes longer than this. On a slow
    /// machine, raise it if long videos fail to post.
    pub max_transcode_secs: u64,
    /// x264 speed/size trade-off. Slower presets give smaller files.
    pub x264_preset: String,
}

/// Disk space for media other people posted. Media of your own posts has
/// no quota; it only has to fit on the disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Media you host for others by resharing with hosting. When it's
    /// full, new hosting is refused; nothing is deleted to make room.
    pub replica_quota_mb: u64,
    /// Media fetched for your own devices. When it's full, what was used
    /// longest ago is deleted to make room.
    pub cache_quota_mb: u64,
}

/// Sending media to other nodes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServingConfig {
    /// Most megabits per second for sending media to other nodes, all
    /// together. 0 means no cap. About half your upload speed leaves room
    /// for everything else, your phone included.
    pub max_upload_mbps: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: "Anonymous".to_string(),
            network: NetworkConfig::default(),
            sync: SyncConfig::default(),
            media: MediaConfig::default(),
            storage: StorageConfig::default(),
            serving: ServingConfig::default(),
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            relay: RelayChoice::Default,
            address_lookup: true,
            bind: None,
            relay_only: false,
        }
    }
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            interval_secs: 60,
            prefetch: true,
        }
    }
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            ffmpeg: "ffmpeg".to_string(),
            ffprobe: "ffprobe".to_string(),
            max_video_secs: 600,
            // As long as a phone waits for its post to be published.
            max_transcode_secs: 1800,
            x264_preset: "medium".to_string(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            replica_quota_mb: 10 * 1024,
            cache_quota_mb: 5 * 1024,
        }
    }
}

impl Config {
    /// A node that only talks to other nodes on this machine: no relays, no
    /// DNS lookup, loopback only, fast transcoding, no background polling.
    pub fn local_test(name: &str) -> Self {
        Self {
            name: name.to_string(),
            network: NetworkConfig {
                relay: RelayChoice::Disabled,
                address_lookup: false,
                bind: Some("127.0.0.1:0".parse().expect("valid address")),
                relay_only: false,
            },
            sync: SyncConfig {
                interval_secs: 3600,
                prefetch: true,
            },
            media: MediaConfig {
                x264_preset: "veryfast".to_string(),
                ..MediaConfig::default()
            },
            storage: StorageConfig::default(),
            serving: ServingConfig::default(),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, toml::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }
}
