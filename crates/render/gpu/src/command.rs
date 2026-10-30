//! Generic immutable graphics commands consumed by every concrete backend.
//!
//! No semantic geometry, native callback, COM resource pointer or mutable scene
//! is replayed here. Recording resolves live public resources into these exact
//! backing versions; native preparation then owns the resulting allocations.
use crate::{abi::*, budget::PendingCharge, resource::*};
use std::rc::Rc;

/// One captured vertex/instance stream, with its immutable version and byte base.
pub struct VertexBinding {
    /// Pipeline stream slot.
    pub slot: u32,
    /// Exact admitted immutable bytes.
    pub buffer: Rc<BufferVersion>,
    /// Four-byte-aligned byte base inside that version.
    pub offset: u64,
}

/// One captured index stream; base vertex and first instance are always zero.
pub struct IndexBinding {
    /// Exact admitted immutable index bytes.
    pub buffer: Rc<BufferVersion>,
    /// Explicit unsigned element width.
    pub format: GpuIndexFormat,
    /// Element-aligned byte base inside that version.
    pub offset: u64,
}

/// Read-only sampled image and sampler versions for one stage/slot.
pub struct SampledBinding {
    /// Graphics stage consuming the pair.
    pub stage: GpuShaderStage,
    /// Stage-local pair slot.
    pub slot: u32,
    /// Exact texture version, independent of public owner liveness after admission.
    pub texture: Rc<TextureVersion>,
    /// Immutable filtering/addressing snapshot.
    pub sampler: Rc<SamplerVersion>,
}

/// Copied uniform bytes can be shared by successive draws until another push changes them.
pub struct UniformData {
    /// Exact portable reflected byte layout; native final padding is separate.
    pub bytes: Vec<u8>,
    /// Follows actual copied payload ownership, including native in-flight storage.
    pub(crate) _charge: PendingCharge,
}

/// One stage/slot's exact copied uniform bytes.
pub struct UniformBinding {
    /// Vertex or fragment stage.
    pub stage: GpuShaderStage,
    /// Stage-local uniform slot.
    pub slot: u32,
    /// Immutable bytes, not a borrowed application pointer.
    pub data: Rc<UniformData>,
}

/// Validated direct-draw range; restart, indirect draws and nonzero bases are excluded.
#[derive(Clone, Copy, Debug)]
pub enum DrawRange {
    /// Non-indexed triangle-list vertices.
    Vertices {
        /// First vertex relative to each stream's captured byte base.
        first: u32,
        /// Number of vertices.
        count: u32,
        /// Number of instances; first instance is zero.
        instances: u32,
    },
    /// Indexed triangle-list vertices.
    Indices {
        /// First element relative to the captured index-buffer byte base.
        first: u32,
        /// Number of index elements.
        count: u32,
        /// Number of instances; base vertex and first instance are zero.
        instances: u32,
    },
}

/// Complete immutable state for one draw; backends never re-read public resources.
pub struct Draw {
    /// Captured logical pipeline and shader dependencies.
    pub pipeline: Rc<PipelineVersion>,
    /// Only streams consumed by this pipeline, not stale unrelated binding state.
    pub vertex_buffers: Vec<VertexBinding>,
    /// Present exactly for indexed draws.
    pub index: Option<IndexBinding>,
    /// Only sampled slots required by the pipeline.
    pub textures: Vec<SampledBinding>,
    /// Only uniform slots required by the pipeline, copied at draw admission.
    pub uniforms: Vec<UniformBinding>,
    /// Checked direct-draw counts and offsets.
    pub range: DrawRange,
    /// Host-bounded logical target mapping.
    pub viewport: Rect,
    /// Host-bounded effective logical raster clip.
    pub clip: Rect,
    /// Command metadata/capacity follows this immutable snapshot until actual release.
    pub(crate) _charge: PendingCharge,
}

/// One ordered generic operation; all coordinates are already flattened and bounded.
pub enum Command {
    /// Bind complete captured state and issue a direct triangle draw.
    Draw(Draw),
    /// Exact unblended regional color replacement, independent of client pipeline masks.
    ClearColor {
        /// Packed RGBA with red in the least significant byte.
        rgba: u32,
        /// Authorized target clip; never an attachment-wide client clear.
        clip: Rect,
        /// Retained record-capacity charge.
        charge: CommandCharge,
    },
    /// Exact finite normalized regional depth replacement.
    ClearDepth {
        /// Requested attachment value in the inclusive unit interval.
        depth: f32,
        /// Authorized target clip.
        clip: Rect,
        /// Retained record-capacity charge.
        charge: CommandCharge,
    },
}

/// Opaque capacity receipt; backends can move it but cannot manufacture admission.
#[derive(Clone)]
pub struct CommandCharge {
    /// Opaque receipt is released with the containing command, never inspected by an adapter.
    pub(crate) _charge: PendingCharge,
}

impl Command {
    /// Keeps accounting alive through native lowering/completion without retaining a client domain.
    pub fn retain_capacity(&self) -> CommandCharge {
        match self {
            Self::Draw(draw) => CommandCharge { _charge: draw._charge.clone() },
            Self::ClearColor { charge, .. } | Self::ClearDepth { charge, .. } => charge.clone(),
        }
    }
}
