//! Reusable fixed-material mesh rendering for the platform renderer's scene capability.
//!
//! This crate owns mesh shaders, pipelines, immutable geometry, and backend recording.
//! Native GPU types remain private to the selected backend; applications submit typed
//! scene data through `neocogi-render-mesh-contracts` and ordinary render items.
#![deny(missing_docs)]

use std::sync::Arc;

use rs_math3d::Mat4f;

#[cfg(any(test, feature = "glow", feature = "webgl", feature = "vulkan", feature = "wgpu"))]
mod vertex;

#[cfg(all(test, any(feature = "vulkan", feature = "wgpu", all(feature = "glow", not(target_arch = "wasm32")))))]
mod pixel_tests;

#[cfg(any(feature = "glow", feature = "webgl"))]
mod glow;
#[cfg(all(not(feature = "webgl"), not(feature = "glow"), feature = "vulkan"))]
mod vulkan;
#[cfg(all(not(feature = "webgl"), not(feature = "glow"), not(feature = "vulkan"), feature = "wgpu"))]
mod wgpu;

#[cfg(any(feature = "glow", feature = "webgl"))]
pub use self::glow::MeshRenderer;
#[cfg(all(not(feature = "webgl"), not(feature = "glow"), feature = "vulkan"))]
pub use self::vulkan::MeshRenderer;
#[cfg(all(not(feature = "webgl"), not(feature = "glow"), not(feature = "vulkan"), feature = "wgpu"))]
pub use self::wgpu::MeshRenderer;

/// Canonical application vertex shared with the typed COM mesh upload contract.
pub use neocogi_render_mesh_contracts::MeshVertex;

/// Immutable CPU mesh storage shared between retained scene state and frame commands.
#[derive(Clone)]
pub struct MeshBuffers {
    /// Semantic vertex storage encoded into the renderer's private GPU layout at upload.
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

    /// Compares immutable allocation identity without hashing the vertex and index contents.
    pub(crate) fn same_storage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.vertices, &other.vertices) && Arc::ptr_eq(&self.indices, &other.indices)
    }

    /// Returns whether a scene or frame command still owns either source array.
    /// A cache owns exactly one clone, so it may retire an upload when both
    /// allocation counts fall back to one.
    pub(crate) fn externally_retained(&self) -> bool {
        Arc::strong_count(&self.vertices) > 1 || Arc::strong_count(&self.indices) > 1
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
pub enum MeshCommand {
    /// Draws a three-dimensional mesh in the custom widget allocation supplied by neocogi.ui.
    DrawMesh(MeshSubmission),
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    /// Upload caches reuse clones and retire their source after the last external owner.
    #[test]
    fn immutable_mesh_identity_tracks_shared_allocations() {
        let source = MeshBuffers::from_vecs(vec![MeshVertex::default()], vec![0]);
        let same_values = MeshBuffers::from_vecs(vec![MeshVertex::default()], vec![0]);
        assert!(!source.same_storage(&same_values));
        let retained = source.clone();
        assert!(source.same_storage(&retained));
        assert!(source.externally_retained());
        drop(retained);
        assert!(!source.externally_retained());
    }
}
