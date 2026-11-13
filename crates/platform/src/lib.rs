//! NeoCogi platform with builtin audio, windowing and shader compilation.
//!
//! Ordinary Rust applications link this library. Dynamic applications use the same
//! contracts and load final COM providers built by `neocogi-platform-dylib`.
//! Glow is the default renderer; select another backend with default features disabled.
#![deny(missing_docs)]

extern crate self as neocogi_platform;
extern crate alloc;

/// Builtin bounded PCM playback and microphone capture; opening audio needs no window.
pub mod audio;

/// Typed resources and command recording for the selected renderer.
pub mod gpu;
/// Builtin SDL/DOM window hosting with one selected graphics backend.
pub mod platform;
/// Backend-neutral graphics/window contracts, upload sources and Rust conveniences.
pub mod render;
