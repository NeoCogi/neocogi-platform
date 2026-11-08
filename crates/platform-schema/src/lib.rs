//! Canonical NeoCogi schemas, generated contracts and backend-neutral Rust helpers.
//! No renderer, Naga, SDL or graphics implementation is part of this package.
#![deny(missing_docs)]
extern crate self as neocogi_platform_schema;
extern crate alloc;
/// Consumer-owned schema generation and deterministic native resource staging.
#[cfg(feature = "build-support")]
#[path = "../build_support/mod.rs"]
pub mod build;
/// Shared renderer, platform and shader contracts, producers and bounded readers.
pub mod render;
#[cfg(test)]
extern crate neocogi_com_host as _;
