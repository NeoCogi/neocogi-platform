//! Rust math conversions layered over the generated demo-render transport values.

use crate::{Matrix4, MeshVertex, Vector2, Vector3};
use rs_math3d::{Mat4f, Vec2f, Vec3f};

impl Default for MeshVertex {
    /// Returns an all-zero vertex suitable for caller-owned output storage.
    fn default() -> Self {
        Self {
            position: Vector3 { x: 0.0, y: 0.0, z: 0.0 },
            normal: Vector3 { x: 0.0, y: 0.0, z: 0.0 },
            uv: Vector2 { x: 0.0, y: 0.0 },
        }
    }
}

impl From<Vec2f> for Vector2 {
    /// Copies an application vector into the IDL-owned two-component layout.
    fn from(value: Vec2f) -> Self {
        Self { x: value.x, y: value.y }
    }
}

impl From<Vector2> for Vec2f {
    /// Copies an IDL vector into the application math representation.
    fn from(value: Vector2) -> Self {
        Self::new(value.x, value.y)
    }
}

impl From<Vec3f> for Vector3 {
    /// Copies an application vector into the IDL-owned three-component layout.
    fn from(value: Vec3f) -> Self {
        Self { x: value.x, y: value.y, z: value.z }
    }
}

impl From<Vector3> for Vec3f {
    /// Copies an IDL vector into the application math representation.
    fn from(value: Vector3) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

impl From<Mat4f> for Matrix4 {
    /// Copies a column-major application matrix into explicit IDL fields.
    fn from(value: Mat4f) -> Self {
        Self {
            m00: value.col[0].x,
            m01: value.col[0].y,
            m02: value.col[0].z,
            m03: value.col[0].w,
            m10: value.col[1].x,
            m11: value.col[1].y,
            m12: value.col[1].z,
            m13: value.col[1].w,
            m20: value.col[2].x,
            m21: value.col[2].y,
            m22: value.col[2].z,
            m23: value.col[2].w,
            m30: value.col[3].x,
            m31: value.col[3].y,
            m32: value.col[3].z,
            m33: value.col[3].w,
        }
    }
}

impl From<Matrix4> for Mat4f {
    /// Copies explicit IDL matrix fields into the application math type.
    fn from(value: Matrix4) -> Self {
        Self::new(
            value.m00, value.m01, value.m02, value.m03, value.m10, value.m11, value.m12, value.m13, value.m20, value.m21, value.m22, value.m23, value.m30,
            value.m31, value.m32, value.m33,
        )
    }
}
