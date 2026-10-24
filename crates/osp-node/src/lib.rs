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
mod health;
mod home;
mod keys;
pub mod media;
mod network;
mod node;
mod peers;
mod relays;
mod social;
mod store;
mod sync;

pub use config::{Config, RelayChoice};
pub use db::BlobClass;
pub use home::Home;
#[cfg(feature = "sim")]
pub use node::SimSettings;
pub use node::{Node, Restored};
