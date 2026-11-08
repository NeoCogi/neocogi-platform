//! Rust conveniences layered over the generated `neocogi.renderer` ABI.
//!
//! These helpers validate local values and do not contribute to the module's
//! generated ABI description or declaration identities.

use crate::render::abi::*;
use neocogi_com::{NC_E_INVALID_ARG, NcStatus};

/// Maximum byte count accepted for one uncompressed RGBA image.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

/// Maximum number of sealed generic contributions accepted in one renderer submission.
pub const MAX_RENDER_BLOCKS: u32 = 1024 * 1024;

/// Renderer state is already borrowed by another upload, submission, or resource operation.
pub const RENDER_E_BUSY: NcStatus = NcStatus::from_code(-1001);

/// A texture or graphics capability belongs to a different renderer device.
pub const RENDER_E_OWNER: NcStatus = NcStatus::from_code(-1002);

/// An invocation-bound graphics view or recording frame is no longer active.
pub const RENDER_E_EXPIRED: NcStatus = NcStatus::from_code(-1003);

/// An operation is illegal in the current recording/session state.
pub const RENDER_E_STATE: NcStatus = NcStatus::from_code(-1004);

/// A renderer backend failed; the operation error owns its concrete detail.
pub const RENDER_E_BACKEND: NcStatus = NcStatus::from_code(-1005);

/// The resource group's immutable creation permissions forbid this operation.
pub const RENDER_E_PERMISSION: NcStatus = NcStatus::from_code(-1006);

impl GpuBufferDesc {
    /// Builds a linear shape from a selected binding role; textures require dimensions.
    pub fn linear(role: GpuBufferRole, byte_len: u64) -> Result<Self, NcStatus> {
        match role {
            GpuBufferRole::Vertex => Ok(Self::Vertex { byte_len }),
            GpuBufferRole::Index => Ok(Self::Index { byte_len }),
            GpuBufferRole::Uniform => Ok(Self::Uniform { byte_len }),
            _ => Err(NC_E_INVALID_ARG),
        }
    }
    /// Derives binding authority from the shape, avoiding a contradictory role field.
    pub fn role(self) -> GpuBufferRole {
        match self {
            Self::Vertex { .. } => GpuBufferRole::Vertex,
            Self::Index { .. } => GpuBufferRole::Index,
            Self::Uniform { .. } => GpuBufferRole::Uniform,
            Self::Texture { .. } => GpuBufferRole::Texture,
        }
    }

    /// Returns the checked unpadded upload length; native storage may require alignment.
    pub fn byte_len(self) -> Result<usize, NcStatus> {
        let len = match self {
            Self::Vertex { byte_len } | Self::Index { byte_len } | Self::Uniform { byte_len } => {
                usize::try_from(byte_len).map_err(|_| neocogi_com::NC_E_OUT_OF_RANGE)?
            }
            Self::Texture { size, format, mip_count } => {
                if mip_count != 1 || !matches!(format, GpuTextureFormat::Rgba8Unorm | GpuTextureFormat::Rgba8Srgb) {
                    return Err(NC_E_INVALID_ARG);
                }
                rgba_bytes(size)?
            }
        };
        if len == 0 || len > MAX_IMAGE_BYTES {
            return Err(neocogi_com::NC_E_OUT_OF_RANGE);
        }
        Ok(len)
    }

    /// Extracts sampled-image metadata without interpreting linear buffers as pixels.
    pub fn texture_info(self) -> Option<GpuTextureInfo> {
        match self {
            Self::Texture { size, format, mip_count } => Some(GpuTextureInfo { size, format, mip_count }),
            Self::Vertex { .. } | Self::Index { .. } | Self::Uniform { .. } => None,
        }
    }
}

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

impl RenderClientPermissions {
    /// Deny every optional operation; callers explicitly enable the operations they delegate.
    pub const fn none() -> Self {
        Self {
            clear_color: false,
            clear_depth: false,
            write_color: false,
            write_depth: false,
            read_depth: false,
            write_resources: false,
            import_images: false,
            initialize_depth: false,
        }
    }
}

impl Default for RenderClientPermissions {
    /// Ordinary programmable framebuffer drawing, including regional depth initialization.
    /// Image import is an independent permission and is denied unless explicitly requested.
    fn default() -> Self {
        Self {
            clear_color: true,
            clear_depth: true,
            write_color: true,
            write_depth: true,
            read_depth: true,
            write_resources: true,
            import_images: false,
            initialize_depth: true,
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

/// Constructs one renderer-neutral RGBA color.
pub const fn color(r: u8, g: u8, b: u8, a: u8) -> Color {
    // Keeping channel construction here prevents callers from depending on a
    // renderer backend's private packed-color representation.
    Color { r, g, b, a }
}

/// Returns the exact RGBA storage required by positive image dimensions.
pub fn rgba_bytes(size: Size) -> Result<usize, NcStatus> {
    crate::render::checked_rgba_len(size.width, size.height).map_err(|_| NC_E_INVALID_ARG)
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
