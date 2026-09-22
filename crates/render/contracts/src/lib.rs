//! Renderer-neutral COM contracts plus Rust-only validation conveniences.
//!
//! The generated ABI deliberately knows nothing about widgets, skins, fonts,
//! or theme catalogs. UI and GFX plugins submit typed bulk frame data while a
//! renderer plugin owns native textures, devices, and presentation.

#![no_std]
#![deny(missing_docs)]

include!(concat!(env!("OUT_DIR"), "/neocogi_renderer.include.rs"));

mod helpers;
pub use helpers::*;
