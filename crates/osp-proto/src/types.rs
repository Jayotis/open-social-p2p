//! ActivityStreams 2.0 payloads (ADR 0003).
//!
//! These are plain JSON using AS2 vocabulary; they are not processed as
//! JSON-LD. Unknown fields are ignored when parsing, so newer nodes can add
//! fields without breaking older ones.

use anyhow::{Result, ensure};
use iroh_base::{EndpointId, PublicKey};
use serde::{Deserialize, Serialize};

use crate::object::ObjectId;

/// Defines a 32-byte identifier that is written as lowercase hex.
macro_rules! hex_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name([u8; 32]);

        impl $name {
            pub fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }

            pub fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }

            /// The first 10 hex digits, for logs and terminal output.
            pub fn fmt_short(&self) -> String {
                data_encoding::HEXLOWER.encode(&self.0[..5])
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&data_encoding::HEXLOWER.encode(&self.0))
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({})", stringify!($name), self.fmt_short())
            }
        }

        impl std::str::FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(s: &str) -> anyhow::Result<Self> {
                let bytes = data_encoding::HEXLOWER_PERMISSIVE.decode(s.as_bytes())?;
                let bytes: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("expected 64 hex digits"))?;
                Ok(Self(bytes))
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}
pub(crate) use hex_id;

hex_id! {
    /// BLAKE3 hash of a media blob. The same hash iroh-blobs uses to address and verify content.
    BlobHash
}

/// Every object a node publishes or relays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Payload {
    Person(Person),
    Note(Note),
    Announce(Announce),
}

impl Payload {
    /// The identity that authored this object. Must match the signer.
    pub fn actor(&self) -> PublicKey {
        match self {
            Payload::Person(p) => p.actor,
            Payload::Note(n) => n.actor,
            Payload::Announce(a) => a.actor,
        }
    }

    /// The AS2 type name.
    pub fn kind(&self) -> &'static str {
        match self {
            Payload::Person(_) => "Person",
            Payload::Note(_) => "Note",
            Payload::Announce(_) => "Announce",
        }
    }

    pub fn published(&self) -> &str {
        match self {
            Payload::Person(p) => &p.published,
            Payload::Note(n) => &n.published,
            Payload::Announce(a) => &a.published,
        }
    }

    /// Checks the limits every node applies to objects, its own and others'.
    /// They keep one author from making readers store or fetch without bound.
    pub fn check_limits(&self) -> Result<()> {
        match self {
            Payload::Person(p) => {
                ensure!(
                    p.name.len() <= MAX_NAME_BYTES,
                    "profile name is longer than {MAX_NAME_BYTES} bytes"
                );
                ensure!(
                    p.summary
                        .as_ref()
                        .is_none_or(|s| s.len() <= MAX_SUMMARY_BYTES),
                    "profile summary is longer than {MAX_SUMMARY_BYTES} bytes"
                );
                ensure!(
                    p.nodes.len() <= MAX_PROFILE_NODES,
                    "profile lists more than {MAX_PROFILE_NODES} nodes"
                );
            }
            Payload::Note(n) => {
                ensure!(
                    n.content.len() <= MAX_CONTENT_BYTES,
                    "post text is longer than {MAX_CONTENT_BYTES} bytes"
                );
                ensure!(
                    n.attachment.len() <= MAX_ATTACHMENTS,
                    "a post can have at most {MAX_ATTACHMENTS} attachments"
                );
                for a in &n.attachment {
                    a.check_limits()?;
                }
            }
            Payload::Announce(_) => {}
        }
        Ok(())
    }
}

/// Longest post text, in bytes.
pub const MAX_CONTENT_BYTES: usize = 10_000;
/// Most photos or videos in one post.
pub const MAX_ATTACHMENTS: usize = 4;
/// Largest media file an attachment may declare. Ten minutes of video in
/// media profile v1 is about 120 MB; this leaves room for nodes that
/// allow longer videos.
pub const MAX_ATTACHMENT_BYTES: u64 = 1 << 30;
/// Largest thumbnail. A 480-pixel WebP is tens of kilobytes.
pub const MAX_THUMBNAIL_BYTES: u64 = 2 << 20;
/// Longest alt text, in bytes.
pub const MAX_ALT_TEXT_BYTES: usize = 1_500;
/// The file types media profile v1 produces (ADR 0005).
pub const MEDIA_TYPES: [&str; 2] = ["video/mp4", "image/webp"];
/// Longest display name, in bytes.
pub const MAX_NAME_BYTES: usize = 200;
/// Longest profile summary, in bytes.
pub const MAX_SUMMARY_BYTES: usize = 2_000;
/// Most home nodes a profile may list.
pub const MAX_PROFILE_NODES: usize = 8;

/// A profile. Also tells others which home nodes speak for this identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Person {
    pub actor: PublicKey,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Endpoint IDs of this identity's home nodes. An extension to AS2.
    pub nodes: Vec<EndpointId>,
    pub published: String,
}

/// A post.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub actor: PublicKey,
    pub content: String,
    pub published: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment: Vec<Attachment>,
    #[serde(rename = "inReplyTo", default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<ObjectId>,
}

impl Note {
    /// Every blob this note refers to: media and thumbnails.
    pub fn blobs(&self) -> impl Iterator<Item = BlobHash> + '_ {
        self.attachment.iter().flat_map(|a| a.blobs())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Video,
    Image,
}

/// A media attachment. Because it sits inside a signed Note, it is the signed
/// manifest for the media: every chunk is verified against `blob`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    #[serde(rename = "type")]
    pub kind: MediaKind,
    #[serde(rename = "mediaType")]
    pub media_type: String,
    pub blob: BlobHash,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    #[serde(
        rename = "durationMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub duration_ms: Option<u64>,
    /// Alt text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<BlobRef>,
    /// The transcoding profile that produced the blob, e.g. `v1-720p` (ADR 0005).
    pub profile: String,
}

impl Attachment {
    pub fn blobs(&self) -> impl Iterator<Item = BlobHash> + '_ {
        std::iter::once(self.blob).chain(self.thumbnail.as_ref().map(|t| t.blob))
    }

    fn check_limits(&self) -> Result<()> {
        ensure!(
            MEDIA_TYPES.contains(&self.media_type.as_str()),
            "attachments must be one of {MEDIA_TYPES:?}, not {:?}",
            self.media_type
        );
        ensure!(
            self.size <= MAX_ATTACHMENT_BYTES,
            "attachment declares {} bytes; the limit is {MAX_ATTACHMENT_BYTES}",
            self.size
        );
        ensure!(
            self.name
                .as_ref()
                .is_none_or(|n| n.len() <= MAX_ALT_TEXT_BYTES),
            "alt text is longer than {MAX_ALT_TEXT_BYTES} bytes"
        );
        if let Some(thumb) = &self.thumbnail {
            ensure!(
                thumb.media_type == "image/webp",
                "thumbnails must be image/webp, not {:?}",
                thumb.media_type
            );
            ensure!(
                thumb.size <= MAX_THUMBNAIL_BYTES,
                "thumbnail declares {} bytes; the limit is {MAX_THUMBNAIL_BYTES}",
                thumb.size
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlobRef {
    pub blob: BlobHash,
    pub size: u64,
    #[serde(rename = "mediaType")]
    pub media_type: String,
}

/// A reshare.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Announce {
    pub actor: PublicKey,
    pub object: ObjectId,
    /// Author of the reshared object, so readers know whose signature to expect.
    #[serde(rename = "objectActor")]
    pub object_actor: PublicKey,
    pub published: String,
    /// True when the resharer's node keeps a copy of the media and serves it.
    #[serde(default)]
    pub hosting: bool,
}
