//! The mesh shader's private upload representation, independent of COM application layout.

use std::{mem, slice};
use crate::MeshVertex;

/// Fully initialized shader attributes; no generated application record is uploaded as bytes.
#[repr(C)]
struct GpuVertex {
    /// Location zero: object-space position.
    position: [f32; 3],
    /// Location one: object-space normal.
    normal: [f32; 3],
    /// Location two: procedural material coordinates.
    uv: [f32; 2],
}

/// Size of one complete vertex in the renderer-owned shader format.
pub(crate) const STRIDE: usize = mem::size_of::<GpuVertex>();
/// Byte offset consumed by the position attribute at location zero.
pub(crate) const POSITION_OFFSET: usize = mem::offset_of!(GpuVertex, position);
/// Byte offset consumed by the normal attribute at location one.
pub(crate) const NORMAL_OFFSET: usize = mem::offset_of!(GpuVertex, normal);
/// Byte offset consumed by the material attribute at location two.
pub(crate) const UV_OFFSET: usize = mem::offset_of!(GpuVertex, uv);

/// Temporary encoded vertices, discarded after the immutable GPU upload completes.
/// The existing MeshBuffers allocation remains the upload cache's identity owner.
pub(crate) struct PackedVertices {
    /// Every float is initialized by named-field conversion before byte access is possible.
    vertices: Vec<GpuVertex>,
}

impl PackedVertices {
    /// Encodes semantic fields once per new upload, with allocation failure before GPU creation.
    pub(crate) fn new(source: &[MeshVertex]) -> Result<Self, String> {
        let mut vertices = Vec::new();
        vertices
            .try_reserve_exact(source.len())
            .map_err(|_| "reserve mesh vertex upload failed".to_owned())?;
        vertices.extend(source.iter().map(|vertex| GpuVertex {
            position: [vertex.position.x, vertex.position.y, vertex.position.z],
            normal: [vertex.normal.x, vertex.normal.y, vertex.normal.z],
            uv: [vertex.uv.x, vertex.uv.y],
        }));
        Ok(Self { vertices })
    }

    /// Borrows the exact dense position/normal/UV payload accepted by every mesh shader.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        const {
            assert!(STRIDE == 32);
            assert!(POSITION_OFFSET == 0);
            assert!(NORMAL_OFFSET == 12);
            assert!(UV_OFFSET == 24);
        }
        // SAFETY: repr(C) fixes field order, the assertions exclude all padding,
        // and construction initializes every float. The view borrows this allocation.
        unsafe { slice::from_raw_parts(self.vertices.as_ptr().cast::<u8>(), mem::size_of_val(self.vertices.as_slice())) }
    }
}

/// Borrows initialized triangle indices; arbitrary application records are never accepted here.
pub(crate) fn index_bytes(indices: &[u32]) -> &[u8] {
    // SAFETY: u32 has no padding, every element is initialized, and the slice supplies its extent.
    unsafe { slice::from_raw_parts(indices.as_ptr().cast::<u8>(), mem::size_of_val(indices)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neocogi_render_mesh_contracts::{Vector2, Vector3};

    /// Distinct values in every field and vertex pin what the shaders read at each byte offset.
    #[test]
    fn shader_attributes_preserve_named_fields_across_vertex_strides() {
        let vertices = [
            MeshVertex {
                position: Vector3 { x: 1., y: 2., z: 3. },
                normal: Vector3 { x: 4., y: 5., z: 6. },
                uv: Vector2 { x: 7., y: 8. },
            },
            MeshVertex {
                position: Vector3 { x: -1., y: -2., z: -3. },
                normal: Vector3 { x: -4., y: -5., z: -6. },
                uv: Vector2 { x: -7., y: -8. },
            },
        ];
        let packed = PackedVertices::new(&vertices).unwrap();
        let floats: Vec<_> = packed.as_bytes().as_chunks::<4>().0.iter().map(|bytes| f32::from_ne_bytes(*bytes)).collect();
        assert_eq!(floats, [1., 2., 3., 4., 5., 6., 7., 8., -1., -2., -3., -4., -5., -6., -7., -8.]);
        assert_eq!(packed.as_bytes().len(), 2 * STRIDE);
        assert!(PackedVertices::new(&[]).unwrap().as_bytes().is_empty());
        let indices = [0_u32, 1, u32::MAX];
        let decoded: Vec<_> = index_bytes(&indices)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| u32::from_ne_bytes(*bytes))
            .collect();
        assert_eq!(decoded, indices);
        assert!(index_bytes(&[]).is_empty());
    }
}
