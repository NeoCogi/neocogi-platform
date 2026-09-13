//! Renderer-independent native-window, normalized-input, and IME contracts.
//!
//! The generated ABI exposes no SDL, DOM, raw-window-handle, graphics-device,
//! widget, skin, or asset-catalog representation. Platform implementations
//! translate native events into copied logical input and accept only a narrow
//! RGBA image when constructing a window.

#![no_std]
#![deny(missing_docs)]

include!(concat!(env!("OUT_DIR"), "/neocogi_platform.rs"));

mod helpers;
pub use helpers::*;
