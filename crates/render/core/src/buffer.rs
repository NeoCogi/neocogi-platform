//! Typed authoring above an untyped driver resource capability.
//!
//! The factory owns only `IRenderResources`. Drivers allocate ordinary buffers;
//! application element types and shader packing never become backend parameters.
use crate::{abi::*, source::Bytes};
use neocogi_com::{ComBox, ComPtr, ComRef, WeakComBox, NcCopyFromBufferOffset, NcOutput, NcStatus, NC_E_INVALID_ARG, NC_E_OUT_OF_MEMORY, NC_E_OUT_OF_RANGE};
use std::{marker::PhantomData, mem, slice};

/// Plain initialized native values whose exact bytes are their declared GPU layout.
///
/// # Safety
/// Every bit pattern must be valid, values must own no references/resources, and
/// all bytes including padding must be initialized. Native little-endian layout,
/// alignment and stride must exactly match the application's shader declaration.
/// COM codec compatibility alone is not this GPU-packing proof. Reads return
/// CPU-authored contents rather than reading native GPU memory.
pub unsafe trait GpuElement: Copy + 'static {
    /// Index interpretation is opt-in; vertex/uniform/texel layout alone grants none.
    const INDEX_FORMAT: Option<GpuIndexFormat> = None;
}

macro_rules! numeric_elements {
    ($($ty:ty),* $(,)?) => {$(
        // SAFETY: Numeric scalar values have unrestricted bits, no padding or
        // ownership, and the factory rejects non-little-endian native authoring.
        unsafe impl GpuElement for $ty {}
    )*};
}
numeric_elements!(u8, i8, i16, i32, u64, i64, f32, f64);
// SAFETY: Unsigned integers have the exact corresponding portable index representation.
unsafe impl GpuElement for u16 {
    const INDEX_FORMAT: Option<GpuIndexFormat> = Some(GpuIndexFormat::U16);
}
// SAFETY: Unsigned integers have the exact corresponding portable index representation.
unsafe impl GpuElement for u32 {
    const INDEX_FORMAT: Option<GpuIndexFormat> = Some(GpuIndexFormat::U32);
}
// SAFETY: Arrays preserve their element layout and have no inter-element padding.
// Zero-sized arrays are rejected before allocation or transfer.
unsafe impl<T: GpuElement, const N: usize> GpuElement for [T; N] {}

#[derive(Clone, Copy, Debug)]
/// Typed authoring shape; linear capacities count elements rather than raw bytes.
pub enum TypedGpuBufferDesc {
    /// Vertex/instance elements interpreted by an explicit pipeline layout.
    Vertex {
        /// Fixed vertex or instance element capacity.
        element_count: u64,
    },
    /// Index elements; binding selects their supported unsigned interpretation.
    Index {
        /// Fixed unsigned index element capacity.
        element_count: u64,
    },
    /// Uniform elements with explicitly padded portable shader packing.
    Uniform {
        /// Fixed count of explicitly packed uniform elements.
        element_count: u64,
    },
    /// RGBA texels; one typed element must occupy exactly one four-byte pixel.
    Texture {
        /// Positive texel dimensions, independent of upload row padding.
        size: Size,
        /// Interpretation of each four-byte typed texel.
        format: GpuTextureFormat,
        /// Baseline resources support exactly one mip level.
        mip_count: u32,
    },
}
impl TypedGpuBufferDesc {
    /// Resolves one authoritative driver shape with checked element arithmetic.
    pub fn resolve<T: GpuElement>(self) -> Result<GpuBufferDesc, NcStatus> {
        element_layout::<T>()?;
        let bytes = |count: u64| count.checked_mul(mem::size_of::<T>() as u64).ok_or(NC_E_OUT_OF_RANGE);
        let desc = match self {
            Self::Vertex { element_count } => GpuBufferDesc::Vertex { byte_len: bytes(element_count)? },
            Self::Index { element_count } => {
                if !matches!(
                    (T::INDEX_FORMAT, mem::size_of::<T>()),
                    (Some(GpuIndexFormat::U16), 2) | (Some(GpuIndexFormat::U32), 4)
                ) {
                    return Err(NC_E_INVALID_ARG);
                }
                GpuBufferDesc::Index { byte_len: bytes(element_count)? }
            }
            Self::Uniform { element_count } => GpuBufferDesc::Uniform { byte_len: bytes(element_count)? },
            Self::Texture { size, format, mip_count } => {
                if mem::size_of::<T>() != 4 {
                    return Err(NC_E_INVALID_ARG);
                }
                GpuBufferDesc::Texture { size, format, mip_count }
            }
        };
        desc.byte_len()?;
        Ok(desc)
    }
}

/// Resource creation capability plus checked typed byte/element conversion.
/// There is no native backend type parameter or application-specific COM factory.
pub struct TypedGpuBufferFactory {
    /// Shared capability preserves the original device, domain permissions and quotas.
    resources: ComPtr<IRenderResourcesAbi>,
}
impl TypedGpuBufferFactory {
    /// Retains the selected resource group without acquiring a backend or renderer.
    pub fn new(resources: ComPtr<IRenderResourcesAbi>) -> Self {
        Self { resources }
    }

    /// Initializes immutable storage once from the exact supplied typed elements.
    pub fn create_static<T: GpuElement>(&self, desc: TypedGpuBufferDesc, values: &[T]) -> Result<TypedGpuBuffer<T>, NcStatus> {
        let desc = desc.resolve::<T>()?;
        let len = values.len().checked_mul(mem::size_of::<T>()).ok_or(NC_E_OUT_OF_RANGE)?;
        if desc.byte_len()? != len {
            return Err(NC_E_INVALID_ARG);
        }
        // SAFETY: GpuElement proves initialized exact GPU bytes, and multiplication
        // above proves the native input range length without integer overflow.
        let bytes = unsafe { slice::from_raw_parts(values.as_ptr().cast::<u8>(), len) };
        let mut encoded = Vec::new();
        encoded.try_reserve_exact(bytes.len()).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        encoded.extend_from_slice(bytes);
        let source = Bytes::source(encoded);
        TypedGpuBuffer::wrap(self.resources.create_static_buffer(desc, source.as_com_ref())?)
    }

    /// Reserves preserving fixed-capacity storage; no initial data source is created.
    pub fn create_dynamic<T: GpuElement>(&self, desc: TypedGpuBufferDesc) -> Result<TypedGpuBuffer<T>, NcStatus> {
        TypedGpuBuffer::wrap(self.resources.create_dynamic_buffer(desc.resolve::<T>()?)?)
    }

    /// Reserves bounded replacement slots once; exhausted writes never grow storage.
    pub fn create_streamed<T: GpuElement>(&self, desc: TypedGpuBufferDesc) -> Result<TypedGpuBuffer<T>, NcStatus> {
        TypedGpuBuffer::wrap(self.resources.create_streamed_buffer(desc.resolve::<T>()?)?)
    }
}

/// Sole ownership of one original driver-created buffer with local typed metadata.
/// This Rust wrapper introduces no new resource identity and allocates no GPU storage.
pub struct TypedGpuBuffer<T: GpuElement> {
    /// The exact registered object is passed back to recording and import/preparation.
    inner: ComBox<IGpuBufferAbi>,
    /// Immutable metadata acquired once, before exposing the typed wrapper.
    info: GpuBufferInfo,
    /// Exact logical element capacity, excluding any native allocation padding.
    count: u64,
    /// Type ownership affects authoring only; no T value is stored in this wrapper.
    element: PhantomData<T>,
}
impl<T: GpuElement> TypedGpuBuffer<T> {
    /// Wraps an existing driver result after checking shape, stride and element capacity.
    pub fn wrap(inner: ComBox<IGpuBufferAbi>) -> Result<Self, NcStatus> {
        element_layout::<T>()?;
        let info = inner.info()?;
        let bytes = info.desc.byte_len()?;
        if bytes % mem::size_of::<T>() != 0 || (info.desc.role() == GpuBufferRole::Texture && mem::size_of::<T>() != 4) {
            return Err(NC_E_INVALID_ARG);
        }
        if info.desc.role() == GpuBufferRole::Index
            && !matches!(
                (T::INDEX_FORMAT, mem::size_of::<T>()),
                (Some(GpuIndexFormat::U16), 2) | (Some(GpuIndexFormat::U32), 4)
            )
        {
            return Err(NC_E_INVALID_ARG);
        }
        let count = (bytes / mem::size_of::<T>()) as u64;
        Ok(Self { inner, info, count, element: PhantomData })
    }
    /// Borrows the original registered resource; this is not a wrapper COM projection.
    pub fn as_gpu_buffer(&self) -> ComRef<'_, IGpuBufferAbi> {
        self.inner.as_com_ref()
    }
    /// Returns a non-owning observer of the original resource for COM wrappers.
    /// It neither duplicates the exclusive owner nor pins any internal upload slot.
    pub fn underlying_buffer(&self) -> WeakComBox<IGpuBufferAbi> {
        self.inner.downgrade()
    }
    /// Transfers the same sole driver owner, discarding only local typed metadata.
    pub fn into_gpu_buffer(self) -> ComBox<IGpuBufferAbi> {
        self.inner
    }
    /// Returns immutable shape and update policy without another foreign call.
    pub fn info(&self) -> GpuBufferInfo {
        self.info
    }
    /// Returns the fixed number of authored elements or texture texels.
    pub fn element_count(&self) -> u64 {
        self.count
    }
    /// Returns the exact application-declared packed GPU stride.
    pub fn element_stride(&self) -> usize {
        mem::size_of::<T>()
    }

    /// Copies initialized elements synchronously; first is an element offset for
    /// every role, including row-major texture texels. No data is retained by COM.
    pub fn upload(&mut self, first: u64, values: &[T]) -> Result<(), NcStatus> {
        let (offset, len) = element_range::<T>(first, values.len(), self.count)?;
        // SAFETY: GpuElement proves initialized bytes and the checked multiplication
        // covers the complete input slice. Only a shared call-scoped view is lent.
        let bytes = unsafe { slice::from_raw_parts(values.as_ptr().cast::<u8>(), len) };
        self.inner.upload(NcCopyFromBufferOffset::new(offset, bytes))
    }

    /// Copies published CPU contents into caller-owned elements. This is not GPU
    /// readback. The destination remains a valid T slice even if a provider fails;
    /// callers should use its contents only after success.
    pub fn read(&self, first: u64, values: &mut [T]) -> Result<(), NcStatus> {
        let (offset, len) = element_range::<T>(first, values.len(), self.count)?;
        // SAFETY: GpuElement permits every resulting bit pattern, including a
        // partially written result on failure. MaybeUninit<u8> has byte alignment;
        // the exclusive caller borrow covers the whole checked output region.
        let slots = unsafe { slice::from_raw_parts_mut(values.as_mut_ptr().cast::<mem::MaybeUninit<u8>>(), len) };
        let mut written = 0;
        self.inner.read(NcOutput::from_slots(offset, slots, &mut written)?)?;
        if written != len {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(())
    }
}

/// Checks element offsets and capacities before any foreign call or byte view.
fn element_range<T: GpuElement>(first: u64, count: usize, capacity: u64) -> Result<(u64, usize), NcStatus> {
    if first.checked_add(count as u64).filter(|end| *end <= capacity).is_none() {
        return Err(NC_E_OUT_OF_RANGE);
    }
    let stride = mem::size_of::<T>();
    let offset = first.checked_mul(stride as u64).ok_or(NC_E_OUT_OF_RANGE)?;
    let len = count.checked_mul(stride).ok_or(NC_E_OUT_OF_RANGE)?;
    Ok((offset, len))
}

/// Native GPU packing is deliberately narrower than arbitrary Rust/COM values.
fn element_layout<T: GpuElement>() -> Result<(), NcStatus> {
    if !cfg!(target_endian = "little") || mem::size_of::<T>() == 0 {
        return Err(NC_E_INVALID_ARG);
    }
    Ok(())
}
