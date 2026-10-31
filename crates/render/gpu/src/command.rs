//! Generic immutable graphics commands consumed by every concrete backend.
//!
//! Recording resolves public resources into immutable backing versions, without semantic geometry
//! or native callbacks. Trusted UI imports capture actual native images directly. Native
//! lowering retains their allocations and per-use receipts, without public-image COM owners.
use crate::{abi::*, budget::PendingCharge, resource::*};
use std::rc::Rc;

/// One captured vertex/instance stream, with its immutable version and byte base.
pub struct VertexBinding<B: crate::native::NativeResources> {
    /// Pipeline stream slot.
    pub slot: u32,
    /// Exact admitted immutable bytes.
    pub buffer: Rc<BufferVersion<B>>,
    /// Four-byte-aligned byte base inside that version.
    pub offset: u64,
}

/// One captured index stream; base vertex and first instance are always zero.
pub struct IndexBinding<B: crate::native::NativeResources> {
    /// Exact admitted immutable index bytes.
    pub buffer: Rc<BufferVersion<B>>,
    /// Explicit unsigned element width.
    pub format: GpuIndexFormat,
    /// Element-aligned byte base inside that version.
    pub offset: u64,
}

/// Read-only sampled image and sampler versions for one stage/slot.
pub struct SampledBinding<B: crate::native::NativeResources> {
    /// Graphics stage consuming the pair.
    pub stage: GpuShaderStage,
    /// Stage-local pair slot.
    pub slot: u32,
    /// Exact texture version, independent of public owner liveness after admission.
    pub texture: Rc<TextureVersion<B>>,
    /// Immutable filtering/addressing snapshot.
    pub sampler: Rc<SamplerVersion<B>>,
}

/// Copied uniform bytes can be shared by successive draws until another push changes them.
pub struct UniformData<B: crate::native::NativeResources> {
    /// Fixes compatible native instance before the payload's first use.
    pub(crate) core: Rc<B::Core>,
    /// Resource-local native upload, private to backend-owned realization.
    pub(crate) native: crate::native::NativeSlot<B::Uniform>,
    /// Exact portable reflected byte layout; native final padding is separate.
    pub(crate) bytes: Vec<u8>,
    /// Follows actual copied payload ownership, including native in-flight storage.
    pub(crate) _charge: PendingCharge,
}

/// One stage/slot's exact copied uniform bytes.
pub struct UniformBinding<B: crate::native::NativeResources> {
    /// Vertex or fragment stage.
    pub stage: GpuShaderStage,
    /// Stage-local uniform slot.
    pub slot: u32,
    /// Immutable bytes, not a borrowed application pointer.
    pub data: Rc<UniformData<B>>,
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
pub struct Draw<B: crate::native::NativeResources> {
    /// Captured logical pipeline and shader dependencies.
    pub pipeline: Rc<PipelineVersion<B>>,
    /// Only streams consumed by this pipeline, not stale unrelated binding state.
    pub vertex_buffers: Vec<VertexBinding<B>>,
    /// Present exactly for indexed draws.
    pub index: Option<IndexBinding<B>>,
    /// Only sampled slots required by the pipeline.
    pub textures: Vec<SampledBinding<B>>,
    /// Only uniform slots required by the pipeline, copied at draw admission.
    pub uniforms: Vec<UniformBinding<B>>,
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
pub enum Command<B: crate::native::NativeResources> {
    /// Bind complete captured state and issue a direct triangle draw.
    Draw(Draw<B>),
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

impl<B: crate::native::NativeResources> Command<B> {
    /// Keeps accounting alive through native lowering/completion without retaining a client domain.
    pub fn retain_capacity(&self) -> CommandCharge {
        match self {
            Self::Draw(draw) => CommandCharge { _charge: draw._charge.clone() },
            Self::ClearColor { charge, .. } | Self::ClearDepth { charge, .. } => charge.clone(),
        }
    }
}

impl<B: crate::native::NativeResources> UniformData<B> {
    /// Borrows the charged, immutable uniform payload captured during admission.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl<B: crate::native::NativeResources> UniformData<B> {
    /// Exact copied-payload charge, independent of a domain or source-version owner.
    pub fn receipt(&self) -> BackingCharge {
        BackingCharge::pending(self._charge.clone())
    }
    /// Reuses only this payload's successfully initialized backing on its actual native core.
    pub fn native(&self, preparation: &mut B::Preparation<'_>) -> Result<Rc<B::Uniform>, String> {
        crate::native::check_core::<B>(&self.core, preparation)?;
        self.native.realize(|| B::uniform(self, preparation))
    }
}
