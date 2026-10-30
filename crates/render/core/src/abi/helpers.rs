//! Rust conveniences layered over the generated `neocogi.renderer` ABI.
//!
//! These helpers validate local values and do not contribute to the module's
//! generated ABI description or declaration identities.

use crate::abi::*;
use neocogi_com::{NC_E_INVALID_ARG, NcStatus};
use rs_math3d::{Color4b, Vec2f, color4b};

/// Maximum byte count accepted for one uncompressed RGBA image.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

/// Maximum number of final vertices accepted in one renderer submission.
pub const MAX_RENDER_VERTICES: u32 = 16 * 1024 * 1024;

/// Maximum number of ordered batches accepted in one renderer submission.
pub const MAX_RENDER_BATCHES: u32 = 1024 * 1024;

/// Renderer state is already borrowed by another upload, submission, or resource operation.
pub const RENDER_E_BUSY: NcStatus = NcStatus::from_code(-1001);

/// A texture or graphics capability belongs to a different renderer device.
pub const RENDER_E_OWNER: NcStatus = NcStatus::from_code(-1002);

/// An invocation-bound graphics view or recording frame is no longer active.
pub const RENDER_E_EXPIRED: NcStatus = NcStatus::from_code(-1003);

/// An operation is illegal in the current recording/session state.
pub const RENDER_E_STATE: NcStatus = NcStatus::from_code(-1004);

/// A renderer backend failed; its diagnostics capability retains concrete detail.
pub const RENDER_E_BACKEND: NcStatus = NcStatus::from_code(-1005);

impl Default for RenderClientLimits {
    /// Finite domain defaults; related widgets sharing a factory share these quotas.
    fn default() -> Self {
        Self {
            resources: 1024,
            live_bytes: 256 * 1024 * 1024,
            pending_bytes: 8 * 1024 * 1024,
            frame_draws: 16_384,
            frame_indices: 64 * 1024 * 1024,
            live_views: 256,
            stack_depth: 64,
        }
    }
}

impl Default for GpuBlendState {
    /// Opaque replacement, with fully specified dormant alpha/color factors.
    fn default() -> Self {
        Self {
            enabled: false,
            src_color: GpuBlendFactor::One,
            dst_color: GpuBlendFactor::Zero,
            color_op: GpuBlendOp::Add,
            src_alpha: GpuBlendFactor::One,
            dst_alpha: GpuBlendFactor::Zero,
            alpha_op: GpuBlendOp::Add,
        }
    }
}

impl Default for GpuPipelineInfo {
    /// A procedural triangle pipeline with no declared inputs and no depth effects.
    fn default() -> Self {
        Self {
            blend: GpuBlendState::default(),
            color_write: 15,
            depth_write: false,
            depth_compare: GpuCompare::Always,
            cull: GpuCullMode::None,
            front_face: GpuFrontFace::Ccw,
            vertex_buffer_count: 0,
            attribute_count: 0,
            binding_count: 0,
        }
    }
}

impl Default for GpuSamplerDesc {
    /// Pixel-preserving nearest sampling, clamped to the texture's edges.
    fn default() -> Self {
        Self {
            min_filter: GpuFilter::Nearest,
            mag_filter: GpuFilter::Nearest,
            address_u: GpuAddressMode::Clamp,
            address_v: GpuAddressMode::Clamp,
        }
    }
}

impl Default for Size {
    /// Returns zero width and height for caller-owned output initialization.
    fn default() -> Self {
        Self { width: 0, height: 0 }
    }
}

impl Default for Rect {
    /// Returns an empty rectangle at the coordinate-system origin.
    fn default() -> Self {
        Self { x: 0, y: 0, width: 0, height: 0 }
    }
}

impl Default for Color {
    /// Returns transparent black.
    fn default() -> Self {
        Self { r: 0, g: 0, b: 0, a: 0 }
    }
}

impl Default for RenderVertex {
    /// Returns a transparent vertex at the origin.
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, u: 0.0, v: 0.0, rgba: 0 }
    }
}

/// Constructs one renderer-neutral RGBA color.
pub const fn color(r: u8, g: u8, b: u8, a: u8) -> Color {
    // Keeping channel construction here prevents callers from depending on a
    // renderer backend's private packed-color representation.
    Color { r, g, b, a }
}

/// Returns the exact RGBA storage required by positive image dimensions.
pub fn rgba_bytes(size: Size) -> Result<usize, NcStatus> {
    // Validate signs before conversion, then check both multiplication steps so
    // malformed foreign dimensions cannot wrap into a small allocation.
    if size.width <= 0 || size.height <= 0 {
        return Err(NC_E_INVALID_ARG);
    }
    let bytes = (size.width as usize)
        .checked_mul(size.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(NC_E_INVALID_ARG)?;
    if bytes > MAX_IMAGE_BYTES {
        return Err(NC_E_INVALID_ARG);
    }
    Ok(bytes)
}

impl RenderVertex {
    /// Creates one canonical renderer and COM transport vertex.
    pub fn new(position: Vec2f, tex_coord: Vec2f, color: Color4b) -> Self {
        // Packing once at the producer boundary gives all backends the same
        // channel order without a second renderer-private vertex structure.
        Self {
            x: position.x,
            y: position.y,
            u: tex_coord.x,
            v: tex_coord.y,
            rgba: u32::from_le_bytes([color.x, color.y, color.z, color.w]),
        }
    }

    /// Returns the screen-space position used by clipping and native backends.
    pub fn position(&self) -> Vec2f {
        Vec2f::new(self.x, self.y)
    }

    /// Returns the normalized image coordinates used by clipping and sampling.
    pub fn tex_coord(&self) -> Vec2f {
        Vec2f::new(self.u, self.v)
    }

    /// Decodes the canonical packed color into renderer channel storage.
    pub fn color(&self) -> Color4b {
        let [r, g, b, a] = self.rgba.to_le_bytes();
        color4b(r, g, b, a)
    }

    /// Linearly interpolates every attribute for geometry clipping.
    pub fn lerp(left: Self, right: Self, amount: f64) -> Self {
        // Keep the clipping fraction and differences wide: subtracting two finite f32
        // endpoints can overflow in f32 before multiplication restores the range.
        // A clamped convex combination stays within the finite endpoint interval.
        let amount = amount.clamp(0.0, 1.0);
        let left_color = left.rgba.to_le_bytes();
        let right_color = right.rgba.to_le_bytes();
        Self {
            x: (f64::from(left.x) + (f64::from(right.x) - f64::from(left.x)) * amount) as f32,
            y: (f64::from(left.y) + (f64::from(right.y) - f64::from(left.y)) * amount) as f32,
            u: (f64::from(left.u) + (f64::from(right.u) - f64::from(left.u)) * amount) as f32,
            v: (f64::from(left.v) + (f64::from(right.v) - f64::from(left.v)) * amount) as f32,
            rgba: u32::from_le_bytes([
                Self::lerp_channel(left_color[0], right_color[0], amount),
                Self::lerp_channel(left_color[1], right_color[1], amount),
                Self::lerp_channel(left_color[2], right_color[2], amount),
                Self::lerp_channel(left_color[3], right_color[3], amount),
            ]),
        }
    }

    /// Interpolates one packed channel and rounds it back into byte storage.
    fn lerp_channel(left: u8, right: u8, amount: f64) -> u8 {
        // Inputs and the clamped amount make adding one half sufficient for
        // nearest rounding without requiring a `std` floating-point helper.
        let interpolated = f64::from(left) + (f64::from(right) - f64::from(left)) * amount;
        (interpolated + 0.5).clamp(0.0, 255.0) as u8
    }
}

impl TextureRegion {
    /// Complete image region used by ordinary texture drawing.
    pub const FULL: Self = Self { u0: 0.0, v0: 0.0, u1: 1.0, v1: 1.0 };

    /// Rejects nonfinite, reversed, and out-of-image coordinates.
    pub fn validate(self) -> Result<(), NcStatus> {
        // Validate all components before ordering so NaN never reaches a
        // comparison whose false result could be mistaken for a valid range.
        if [self.u0, self.v0, self.u1, self.v1]
            .iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
            && self.u0 <= self.u1
            && self.v0 <= self.v1
        {
            Ok(())
        } else {
            Err(NC_E_INVALID_ARG)
        }
    }
}

/// Numerical regressions exercise the contract independently of renderer selection.
#[cfg(test)]
mod interpolation_tests {
    use super::*;

    /// Finite opposite extremes must produce bounded attributes, including at either endpoint.
    #[test]
    fn finite_extremes_interpolate_without_overflow() {
        let left = RenderVertex {
            x: -f32::MAX,
            y: f32::MAX,
            u: -f32::MAX,
            v: f32::MAX,
            rgba: 0,
        };
        let right = RenderVertex {
            x: f32::MAX,
            y: -f32::MAX,
            u: f32::MAX,
            v: -f32::MAX,
            rgba: u32::MAX,
        };
        for amount in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let value = RenderVertex::lerp(left, right, amount);
            assert!([value.x, value.y, value.u, value.v].into_iter().all(f32::is_finite));
        }
        let midpoint = RenderVertex::lerp(left, right, 0.5);
        assert_eq!([midpoint.x, midpoint.y, midpoint.u, midpoint.v], [0.0; 4]);
        assert_eq!(midpoint.rgba, 0x80808080);
    }
}
