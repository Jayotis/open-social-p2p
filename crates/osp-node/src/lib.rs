//! Open Social P2P home node.
//!
//! A home node is a person's agent on the network (ADR 0001): it holds their
//! identity, publishes their posts, follows other people, gathers and
//! prefetches content for their phone, and optionally hosts copies of posts
//! they reshare. [`Node`] is the entry point.

mod activity;
mod admission;
pub mod backup;
mod carrier;
mod client;
mod cluster;
pub mod config;
mod connected;
mod control;
mod db;
mod directory;
mod endpoints;
mod events;
mod health;
mod hide_list;
mod home;
mod hops;
mod host_circuit;
mod hosting;
mod keys;
mod likes;
mod machine;
pub mod media;
mod meet;
mod network;
mod node;
mod opened;
mod peers;
mod picks;
mod places;
mod profiles;
mod relays;
mod replies;
mod replying;
#[cfg(feature = "sim")]
pub mod simnet;
mod sky;
mod social;
mod store;
mod strangers;
mod sync;
mod threads;
mod tor;
mod trust;
mod universe;
mod uploads;
mod viewing;
mod vouches;
mod work_offers;
mod world;

/// This build's version: the release's tag when the release job sets
/// `OSP_RELEASE` (such as v0.1.0-alpha.3), or else the crate's version.
pub const VERSION: &str = match option_env!("OSP_RELEASE") {
    Some(tag) => tag,
    None => env!("CARGO_PKG_VERSION"),
};

#[cfg(feature = "sim")]
pub use admission::Meeting;
pub use config::{
    CarrierConfig, Config, DirectRule, IntroductionsConfig, KnownDirectory, RelayChoice,
    SeedCarrier,
};
pub use db::BlobClass;
pub use endpoints::PeerEndpoints;
pub use home::Home;
pub use meet::{Acquaintance, IntroductionCounts, MeetingTimes, Stage};
#[cfg(feature = "sim")]
pub use node::SimSettings;
pub use node::{Node, Restored};
pub use trust::plain;
pub use vouches::Gathered;
