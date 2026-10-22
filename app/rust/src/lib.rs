//! The Rust side of the Android app: a thin layer over `osp-client` that
//! flutter_rust_bridge exposes to Dart. See `api/osp.rs`.

pub mod api;
mod frb_generated;
mod logging;
