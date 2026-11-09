//! Canonical NeoCogi contracts, shared types, limits and backend-neutral validation.
//! Owns the authored NCIDL schemas, generated Rust bindings and optional build helpers.
//! No renderer, Naga, SDL or graphics implementation is part of this package.
#![deny(missing_docs)]
extern crate self as neocogi_platform_contracts;
extern crate alloc;
/// Independent PCM audio contracts, shared types, limits and validation.
pub mod audio;
/// Consumer-owned schema generation and deterministic native resource staging.
#[cfg(feature = "build-support")]
#[path = "../build_support/mod.rs"]
pub mod build;
mod diagnostic;
/// Shared renderer, platform and shader contracts, producers and bounded readers.
pub mod render;
#[cfg(test)]
extern crate neocogi_com_host as _;
