//! Application-owned GPU renderer used by the full demonstration.
//!
//! The crate deliberately lives beside the demos rather than inside a reusable renderer backend.
//! It owns the demo mesh vocabulary, shaders, pipelines, buffers, and backend-specific recording
//! code. Renderer crates supply only typed native extension points and frame ownership.

use std::sync::Arc;

use rs_math3d::Mat4f;

#[cfg(any(feature = "glow", feature = "webgl"))]
mod glow;
#[cfg(all(not(feature = "webgl"), not(feature = "glow"), feature = "vulkan"))]
mod vulkan;
#[cfg(all(not(feature = "webgl"), not(feature = "glow"), not(feature = "vulkan"), feature = "wgpu"))]
mod wgpu;

#[cfg(any(feature = "glow", feature = "webgl"))]
pub use self::glow::DemoRenderer;
#[cfg(all(not(feature = "webgl"), not(feature = "glow"), feature = "vulkan"))]
pub use self::vulkan::DemoRenderer;
#[cfg(all(not(feature = "webgl"), not(feature = "glow"), not(feature = "vulkan"), feature = "wgpu"))]
pub use self::wgpu::DemoRenderer;

/// Canonical application vertex shared with the typed COM mesh upload contract.
pub use microui_demo_contracts::MeshVertex;

/// Immutable CPU mesh storage shared between retained demo state and frame commands.
#[derive(Clone)]
pub struct MeshBuffers {
    /// Contiguous vertex storage uploaded by the selected application renderer.
    vertices: Arc<[MeshVertex]>,
    /// Triangle-list indices interpreted as unsigned 32-bit GPU indices.
    indices: Arc<[u32]>,
}

impl MeshBuffers {
    /// Converts uniquely owned construction vectors into cheap, immutable shared storage.
    pub fn from_vecs(vertices: Vec<MeshVertex>, indices: Vec<u32>) -> Self {
        Self {
            vertices: vertices.into(),
            indices: indices.into(),
        }
    }

    /// Borrows the complete vertex payload without exposing the storage owner.
    pub fn vertices(&self) -> &[MeshVertex] {
        &self.vertices
    }

    /// Borrows the complete triangle-list index payload.
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// Returns whether the mesh lacks either vertices or drawable indices.
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty() || self.indices.is_empty()
    }
}

/// Complete state required for one application-owned mesh draw.
#[derive(Clone)]
pub struct MeshSubmission {
    /// Shared immutable geometry uploaded by the active native renderer.
    pub mesh: MeshBuffers,
    /// Projection-view-model matrix used to position vertices in clip space.
    pub pvm: Mat4f,
    /// View-model matrix used to transform normals for lighting.
    pub view_model: Mat4f,
}

/// Concrete application command retained in the selected backend's typed frame stream.
pub enum DemoCommand {
    /// Draws a three-dimensional mesh in the custom widget allocation supplied by microui.
    DrawMesh(MeshSubmission),
}
