//! ActivityStreams 2.0 payloads (ADR 0003).
//!
//! These are plain JSON using AS2 vocabulary; they are not processed as
//! JSON-LD. Unknown fields are ignored when parsing, so newer nodes can add
//! fields without breaking older ones.

use anyhow::{Result, ensure};
use iroh_base::{EndpointId, PublicKey};
use serde::{Deserialize, Serialize};

use crate::{
    binding::{EndpointBinding, EndpointKind},
    object::ObjectId,
};

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
    Key(Key),
    Delete(Delete),
    ForwardDelete(ForwardDelete),
    FoundEarly(FoundEarly),
}

impl Payload {
    /// The identity that authored this object. Must match the signer.
    pub fn actor(&self) -> PublicKey {
        match self {
            Payload::Person(p) => p.actor,
            Payload::Note(n) => n.actor,
            Payload::Announce(a) => a.actor,
            Payload::Key(k) => k.actor,
            Payload::Delete(d) => d.actor,
            Payload::ForwardDelete(f) => f.actor,
            Payload::FoundEarly(f) => f.actor,
        }
    }

    /// The AS2 type name, or `Key` for a key certificate (not AS2).
    pub fn kind(&self) -> &'static str {
        match self {
            Payload::Person(_) => "Person",
            Payload::Note(_) => "Note",
            Payload::Announce(_) => "Announce",
            Payload::Key(_) => "Key",
            Payload::Delete(_) => "Delete",
            Payload::ForwardDelete(_) => "ForwardDelete",
            Payload::FoundEarly(_) => "FoundEarly",
        }
    }

    pub fn published(&self) -> &str {
        match self {
            Payload::Person(p) => &p.published,
            Payload::Note(n) => &n.published,
            Payload::Announce(a) => &a.published,
            Payload::Key(k) => &k.published,
            Payload::Delete(d) => &d.published,
            Payload::ForwardDelete(f) => &f.published,
            Payload::FoundEarly(f) => &f.published,
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
                ensure!(
                    p.media_endpoints.len() <= MAX_PROFILE_NODES,
                    "profile lists more than {MAX_PROFILE_NODES} media endpoints"
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
            Payload::Announce(_)
            | Payload::Key(_)
            | Payload::Delete(_)
            | Payload::ForwardDelete(_)
            | Payload::FoundEarly(_) => {}
        }
        Ok(())
    }
}

/// Longest post text, in bytes.
pub const MAX_CONTENT_BYTES: usize = 10_000;
/// Longest reply text, in bytes (ADR 0028).
pub const MAX_REPLY_BYTES: usize = 4_000;
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
    /// Whether a directory may list this profile and its latest posts, and
    /// others may pass them on (ADR 0015). A later profile can take it back.
    /// An extension to AS2, named as Mastodon names it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub discoverable: bool,
    /// Endpoint IDs of this identity's home nodes. An extension to AS2.
    pub nodes: Vec<EndpointId>,
    /// The bindings of those nodes' media endpoints (ADR 0024), so a node
    /// that follows nobody here can fetch a public post's media from one.
    /// A media endpoint is relay-only and found by its ID, so this shows only
    /// which relay it uses. Never a direct endpoint's. An extension to AS2.
    #[serde(
        rename = "mediaEndpoints",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub media_endpoints: Vec<EndpointBinding>,
    pub published: String,
}

impl Person {
    /// The media endpoint of `node`, one of this profile's nodes, if the
    /// profile gives a binding for it that checks out: both keys signed it,
    /// it's for a media endpoint, and the node is one this profile lists. Any
    /// other binding is ignored, not the profile. Of two for one node, the
    /// later holds.
    pub fn media_endpoint(&self, node: &EndpointId) -> Option<&EndpointBinding> {
        self.media_endpoints()
            .filter(|b| b.public == *node)
            .max_by_key(|b| b.since_ms)
    }

    /// Every binding in `media_endpoints` that checks out, as above.
    pub fn media_endpoints(&self) -> impl Iterator<Item = &EndpointBinding> + '_ {
        self.media_endpoints.iter().filter(|b| {
            b.kind == EndpointKind::Media && self.nodes.contains(&b.public) && b.verify().is_ok()
        })
    }
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
    /// Whether this note answers another post (ADR 0028). A reply goes only to
    /// the post's creator, never into an outbox, a feed, search or a
    /// directory's lists.
    pub fn is_reply(&self) -> bool {
        self.in_reply_to.is_some()
    }

    /// Checks what a reply must be beyond any note (ADR 0028): it answers a
    /// post, its text isn't empty once trimmed and is within
    /// [`MAX_REPLY_BYTES`], and it has no attachments.
    pub fn check_reply(&self) -> Result<()> {
        ensure!(self.is_reply(), "that isn't a reply");
        ensure!(self.attachment.is_empty(), "a reply can't have attachments");
        ensure!(!self.content.trim().is_empty(), "the reply is empty");
        ensure!(
            self.content.len() <= MAX_REPLY_BYTES,
            "the reply is longer than {MAX_REPLY_BYTES} bytes"
        );
        Ok(())
    }

    /// Every blob this note refers to: media and thumbnails.
    pub fn blobs(&self) -> impl Iterator<Item = BlobHash> + '_ {
        self.attachment.iter().flat_map(|a| a.blobs())
    }

    /// The same blobs, each with the size this note declares for it.
    pub fn sized_blobs(&self) -> impl Iterator<Item = (BlobHash, u64)> + '_ {
        self.attachment.iter().flat_map(|a| {
            std::iter::once((a.blob, a.size)).chain(a.thumbnail.as_ref().map(|t| (t.blob, t.size)))
        })
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

/// A found-early credit (ADR 0029): the author of `post` credits `finder`
/// with being one of the first to reshare it, in the order the author's node
/// heard them. Not AS2. Never in an outbox: the author's node sends it to the
/// finder's, which keeps it as a vouch for its owner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FoundEarly {
    pub actor: PublicKey,
    pub post: ObjectId,
    pub finder: PublicKey,
    pub published: String,
}

/// A key certificate (ADR 0010): the identity names the key that signs its
/// objects from now on. Only the identity key itself may sign one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub actor: PublicKey,
    /// The signing key this certificate is for.
    pub key: PublicKey,
    pub published: String,
    /// What happens to every key certified before this one.
    #[serde(rename = "earlierKeys")]
    pub earlier_keys: EarlierKeys,
}

/// Its author takes back a post or reshare (ADR 0019). Only the identity that
/// signed `object` may delete it, and a node that hears of it keeps the
/// `Delete` for good, so an old copy can't come back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delete {
    pub actor: PublicKey,
    /// The post or reshare, by its ID.
    pub object: ObjectId,
    pub published: String,
}

/// A resharer passing on the author's `Delete` of a post it reshared (ADR
/// 0019), as a reshare passes on its post: the `Delete` itself comes with the
/// outbox page. Not AS2. The resharer's signature shows where it came from;
/// only the author's, on the `Delete`, gives the right to delete.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForwardDelete {
    pub actor: PublicKey,
    /// The author's `Delete`, by its ID.
    pub delete: ObjectId,
    pub published: String,
}

/// What a newer key certificate does to the keys certified before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EarlierKeys {
    /// Their owner stopped using them; what they signed stays valid.
    Retired,
    /// Someone else may have them: what they signed is no longer valid.
    Revoked,
}

#[cfg(test)]
mod tests {
    use iroh_base::SecretKey;

    use super::*;

    fn person(discoverable: bool) -> Person {
        Person {
            actor: SecretKey::from_bytes(&[1; 32]).public(),
            name: "Alice".into(),
            summary: None,
            discoverable,
            nodes: vec![],
            media_endpoints: vec![],
            published: "2026-11-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn a_profile_says_nothing_of_being_listed_unless_it_may_be() {
        let json = serde_json::to_string(&person(false)).unwrap();
        assert!(!json.contains("discoverable"), "{json}");
        // So one from before it means no, and signed bytes stay as they were.
        let back: Person = serde_json::from_str(&json).unwrap();
        assert!(!back.discoverable);

        let json = serde_json::to_string(&person(true)).unwrap();
        assert!(json.contains(r#""discoverable":true"#), "{json}");
        assert!(serde_json::from_str::<Person>(&json).unwrap().discoverable);
    }

    fn key(byte: u8) -> SecretKey {
        SecretKey::from_bytes(&[byte; 32])
    }

    /// Alice's profile, listing node 2, with these bindings.
    fn with_bindings(bindings: Vec<EndpointBinding>) -> Person {
        Person {
            nodes: vec![key(2).public()],
            media_endpoints: bindings,
            ..person(true)
        }
    }

    fn media(node: u8, endpoint: u8, since_ms: u64) -> EndpointBinding {
        EndpointBinding::new(&key(node), &key(endpoint), EndpointKind::Media, since_ms)
    }

    #[test]
    fn a_profiles_media_endpoint_checks_out_after_travelling() {
        let alice = with_bindings(vec![media(2, 3, 1_000)]);
        let json = serde_json::to_string(&alice).unwrap();
        assert!(json.contains(r#""mediaEndpoints":["#), "{json}");
        let back: Person = serde_json::from_str(&json).unwrap();
        assert_eq!(back, alice);
        let found = back.media_endpoint(&key(2).public()).unwrap();
        assert_eq!(found.endpoint, key(3).public());
        // A profile without one says nothing of it, as before.
        let json = serde_json::to_string(&person(true)).unwrap();
        assert!(!json.contains("mediaEndpoints"), "{json}");
    }

    #[test]
    fn a_forged_binding_in_a_profile_is_ignored() {
        let json = serde_json::to_string(&media(2, 3, 1_000)).unwrap();
        let forged: EndpointBinding = serde_json::from_str(&json.replace("1000", "1001")).unwrap();
        let person = with_bindings(vec![forged]);
        assert!(person.media_endpoint(&key(2).public()).is_none());
        // The binding is ignored, not the profile.
        Payload::Person(person).check_limits().unwrap();
    }

    #[test]
    fn a_binding_for_a_node_the_profile_doesnt_list_is_ignored() {
        // Node 4 binds its own media endpoint, validly, but Alice's profile
        // lists only node 2.
        let person = with_bindings(vec![media(4, 3, 1_000)]);
        assert!(person.media_endpoint(&key(4).public()).is_none());
        assert!(person.media_endpoint(&key(2).public()).is_none());
        assert_eq!(person.media_endpoints().count(), 0);
    }

    #[test]
    fn a_direct_binding_in_a_profile_is_ignored() {
        let direct = EndpointBinding::new(&key(2), &key(3), EndpointKind::Direct, 1_000);
        let person = with_bindings(vec![direct]);
        assert!(person.media_endpoint(&key(2).public()).is_none());
    }

    #[test]
    fn of_two_bindings_for_one_node_the_later_holds() {
        let person = with_bindings(vec![media(2, 5, 2_000), media(2, 3, 1_000)]);
        let found = person.media_endpoint(&key(2).public()).unwrap();
        assert_eq!(found.endpoint, key(5).public());
    }

    #[test]
    fn a_profile_with_too_many_bindings_is_refused() {
        let many = (0..=MAX_PROFILE_NODES as u8)
            .map(|i| media(2, 10 + i, 1_000))
            .collect();
        assert!(Payload::Person(with_bindings(many)).check_limits().is_err());
    }
}
