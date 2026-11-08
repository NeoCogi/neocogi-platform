//! Backend-neutral Rust integration shared by the UI runtime and renderer plugins.
//!
//! This crate has no plugin entry point. It owns the shared contracts, upload producers,
//! frame values and traits. Optional compiler support belongs to this renderer module.
#![deny(missing_docs)]

extern crate alloc;
mod backend;
mod color;
mod image;
mod texture;

pub use backend::{FrameError, FrameInfo, FrameInfoError, RendererBackend, RendererFrame};
pub use color::{Color, color};
pub use image::{ImageSource, ImageValidationError, MAX_IMAGE_BYTES, checked_rgba_len};
pub use texture::{TextureError};

/// Convenient imports for concrete backend implementations.
pub mod prelude {
    pub use crate::render::{Color, FrameError, FrameInfo, FrameInfoError, ImageSource, ImageValidationError, RendererBackend, RendererFrame, TextureError, color};
    pub use rs_math3d::{
        Box3f, Color4b, CrossProduct, Dimension, Dimensioni, RealVector, Mat4f, Quat, Quatf, Rect, Recti, Vec2f, Vec2i, Vec3f, Vec4f, Vector, Vector3, color4b,
        ortho4,
    };
}

/// Shared IDL bindings and their Rust conveniences.
#[allow(clippy::len_without_is_empty, reason = "IDL collections use len/get; checking zero needs no extra ABI call")]
pub mod abi;
/// Typed factories and mapped element access above the untyped resource driver.
pub mod buffer;
/// Shared IDL bindings and their Rust conveniences.
pub mod platform;
/// Owned upload producers shared by clients and GFX without linking renderer implementation.
pub mod source;

/// Renderer-owned Naga compiler with one immutable backend output format.
#[cfg(any(test, feature = "shader-compiler"))]
pub mod compiler;

/// Portable shader snapshots, reflection and bounded compiler artifact readers.
pub mod shader;
