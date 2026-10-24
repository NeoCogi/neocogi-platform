//! Backend-neutral Rust integration shared by the UI runtime and renderer plugins.
//!
//! This crate is not a deployable plugin and exports no COM classes. It contains only
//! concrete frame values and traits needed to compile a renderer implementation without
//! linking the UI runtime implementation into the renderer's shared library.
#![deny(missing_docs)]

extern crate self as neocogi_render_core;
extern crate alloc;
mod backend;
mod color;
mod image;
mod texture;

pub use backend::{AtlasUploadError, FrameError, FrameInfo, FrameInfoError, RenderItemArgs, RendererBackend, RendererFrame, Vertex};
pub use color::{Color, color};
pub use image::{ImageSource, ImageValidationError, MAX_IMAGE_BYTES};
pub use texture::{RendererId, TextureError, TextureId};

/// Convenient imports for concrete backend implementations.
pub mod prelude {
    pub use crate::{
        AtlasUploadError, Color, FrameError, FrameInfo, FrameInfoError, ImageSource, ImageValidationError, RenderItemArgs, RendererBackend, RendererFrame,
        TextureError, TextureId, Vertex, color,
    };
    pub use rs_math3d::{
        Box3f, Color4b, CrossProduct, Dimension, Dimensioni, FloatVector, Mat4f, Quat, Quatf, Rect, Recti, Vec2f, Vec2i, Vec3f, Vec4f, Vector, Vector3,
        color4b, ortho4,
    };
}

/// Shared IDL bindings and their Rust conveniences.
pub mod abi;
/// Shared IDL bindings and their Rust conveniences.
pub mod platform;

/// Authored NCIDL sources owned by this Cargo contract package.
pub const SCHEMA_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/schemas");
