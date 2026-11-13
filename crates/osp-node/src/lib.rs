//! Open Social P2P home node.
//!
//! A home node is a person's agent on the network (ADR 0001): it holds their
//! identity, publishes their posts, follows other people, gathers and
//! prefetches content for their phone, and optionally hosts copies of posts
//! they reshare. [`Node`] is the entry point.

pub mod backup;
mod client;
pub mod config;
mod control;
mod db;
mod directory;
mod endpoints;
mod health;
mod home;
mod hosting;
mod keys;
mod machine;
pub mod media;
mod meet;
mod network;
mod node;
mod peers;
mod relays;
mod replies;
mod replying;
#[cfg(feature = "sim")]
pub mod simnet;
mod social;
mod store;
mod sync;
mod threads;
mod tor;
mod trust;
mod uploads;
mod viewing;
mod vouches;

/// This build's version: the release's tag when the release job sets
/// `OSP_RELEASE` (such as v0.1.0-alpha.3), or else the crate's version.
pub const VERSION: &str = match option_env!("OSP_RELEASE") {
    Some(tag) => tag,
    None => env!("CARGO_PKG_VERSION"),
};

pub use config::{Config, DirectRule, IntroductionsConfig, KnownDirectory, RelayChoice};
pub use db::BlobClass;
pub use endpoints::PeerEndpoints;
pub use home::Home;
pub use meet::{Acquaintance, IntroductionCounts, MeetingTimes, Stage};
#[cfg(feature = "sim")]
pub use node::SimSettings;
pub use node::{Node, Restored};
pub use trust::plain;
