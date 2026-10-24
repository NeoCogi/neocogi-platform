//! Renderer-neutral COM contracts plus Rust-only validation conveniences.
//!
//! The generated ABI deliberately knows nothing about widgets, skins, fonts,
//! or theme catalogs. UI and GFX plugins submit typed bulk frame data while a
//! renderer plugin owns native textures, devices, and presentation.

#![deny(missing_docs)]

include!(env!("IDL_NEOCOGI_RENDERER"));

mod helpers;
pub use helpers::*;

mod budget;
pub use budget::*;
