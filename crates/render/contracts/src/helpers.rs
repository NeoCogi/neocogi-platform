//! Rust conveniences layered over the generated `neocogi.renderer` ABI.
//!
//! These helpers validate local values and do not contribute to the module's
//! generated ABI description or declaration identities.

use crate::*;
use neocogi_com::{NC_E_INVALID_ARG, NcStatus};
use rs_math3d::{Color4b, Vec2f, color4b};

/// Maximum byte count accepted for one uncompressed RGBA image.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

/// Maximum number of final vertices accepted in one renderer submission.
pub const MAX_RENDER_VERTICES: u32 = 16 * 1024 * 1024;

/// Maximum number of ordered batches accepted in one renderer submission.
pub const MAX_RENDER_BATCHES: u32 = 1024 * 1024;

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
    pub fn lerp(left: Self, right: Self, amount: f32) -> Self {
        // Clamp numerical noise at an intersection so clipping cannot
        // extrapolate a vertex beyond the validated input segment.
        let amount = amount.clamp(0.0, 1.0);
        let left_color = left.rgba.to_le_bytes();
        let right_color = right.rgba.to_le_bytes();
        Self {
            x: left.x + (right.x - left.x) * amount,
            y: left.y + (right.y - left.y) * amount,
            u: left.u + (right.u - left.u) * amount,
            v: left.v + (right.v - left.v) * amount,
            rgba: u32::from_le_bytes([
                Self::lerp_channel(left_color[0], right_color[0], amount),
                Self::lerp_channel(left_color[1], right_color[1], amount),
                Self::lerp_channel(left_color[2], right_color[2], amount),
                Self::lerp_channel(left_color[3], right_color[3], amount),
            ]),
        }
    }

    /// Interpolates one packed channel and rounds it back into byte storage.
    fn lerp_channel(left: u8, right: u8, amount: f32) -> u8 {
        // Inputs and the clamped amount make adding one half sufficient for
        // nearest rounding without requiring a `std` floating-point helper.
        let interpolated = (left as f32) + (right as f32 - left as f32) * amount;
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
