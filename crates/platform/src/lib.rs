//! NeoCogi contracts and optional implementations for linked and loaded applications.
//!
//! Ordinary Rust applications link this library. Dynamic applications use the same
//! contracts and load final COM providers built by `neocogi-platform-dylib`.
//! Empty defaults keep native libraries and the shader compiler out of SDK consumers.
#![deny(missing_docs)]

extern crate self as neocogi_platform;
extern crate alloc;

/// Build-script schema generation and deterministic native resource staging.
#[cfg(feature = "build-support")]
#[path = "../build_support/mod.rs"]
pub mod build;
/// Optional Naga-backed CPU compiler and its independently registered COM descriptor.
#[cfg(any(test, feature = "shader-compiler"))]
pub mod compiler;
/// Typed resources, command recording and additive concrete backend implementations.
pub mod gpu;
/// Optional SDL/DOM hosting with one explicitly selected graphics backend.
#[cfg(feature = "platform")]
pub mod platform;
/// Backend-neutral graphics/window contracts, upload sources and Rust conveniences.
pub mod render;
/// Portable shader contracts, reflection and bounded artifact readers.
pub mod shader;
