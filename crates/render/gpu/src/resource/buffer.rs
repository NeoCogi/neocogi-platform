//! Persistent public buffers and fixed reusable upload slots.
//!
//! A slot can be rewritten only when neither a recorded snapshot nor a concrete
//! prepared/submitted native owner retains it. Transfers copy synchronously; no
//! storage pointer, mapping session or lease escapes the public buffer.
use super::*;
use neocogi_com::NcOutputWriter;

/// Streaming permits the host's conventional two queued frames plus one CPU writer.
/// Slots are reserved and charged at creation; exhaustion never grows capacity.
pub(crate) const STREAM_SLOTS: usize = 3;
/// Fixed initialized CPU contents retained for native realization and refresh.
/// Capacity never changes after construction; synchronous transfers need no alignment shim.
pub struct UploadBytes {
    /// Logical bytes, initially zero and never resized.
    bytes: Vec<u8>,
}
impl UploadBytes {
    /// Reserves and initializes the complete logical capacity before publication.
    pub(crate) fn new(len: usize) -> Result<Self, NcStatus> {
        Ok(Self { bytes: zeroed(len)? })
    }
    /// Borrows the published CPU representation.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Exclusivity follows from having no logical or concrete native owners.
    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

/// Typed native families remain distinct internally even though the public owner is shared.
pub(crate) enum BufferSlot<B: NativeResources> {
    /// Vertex, index or uniform bytes with stable native buffer memoization.
    Linear(Rc<BufferVersion<B>>),
    /// Sampled pixels or an authenticated immutable imported image.
    Texture(Rc<TextureVersion<B>>),
}
impl<B: NativeResources> BufferSlot<B> {
    /// Both logical captures and private queue/prepared owners must be absent.
    /// Weak Rust snapshot observers also exclude reuse: they could upgrade into
    /// readers while an upload writes. Public COM weak bindings do not hold slots.
    fn idle(&self) -> bool {
        match self {
            Self::Linear(value) => Rc::strong_count(value) == 1 && Rc::weak_count(value) == 0 && value.native.idle(),
            Self::Texture(value) => Rc::strong_count(value) == 1 && Rc::weak_count(value) == 0 && value.native.idle(),
        }
    }
    /// Obtains exclusive initialized authoring bytes without replacing its allocation.
    fn bytes_mut(&mut self) -> Result<&mut [u8], NcStatus> {
        let bytes = match self {
            Self::Linear(value) => &mut Rc::get_mut(value).ok_or(RENDER_E_BUSY)?.bytes,
            Self::Texture(value) => match &mut Rc::get_mut(value).ok_or(RENDER_E_BUSY)?.contents {
                TextureContents::Pixels(bytes) => bytes,
                TextureContents::Imported { .. } => return Err(RENDER_E_PERMISSION),
            },
        };
        match bytes {
            VersionBytes::Upload(bytes) => Ok(bytes.bytes_mut()),
            VersionBytes::Owned(_) => Err(RENDER_E_PERMISSION),
        }
    }
    /// The next preparation refreshes the same concrete native storage, if already realized.
    fn changed(&self) {
        match self {
            Self::Linear(value) => value.dirty.set(true),
            Self::Texture(value) => value.dirty.set(true),
        }
    }
}

#[com_object(interfaces(IGpuBuffer), ownership = exclusive)]
/// One persistent driver-created resource with bounded reusable physical backing.
pub(crate) struct BufferObject<B: NativeResources> {
    /// Public authority ends with this object, not with retained snapshots.
    pub(crate) domain: Rc<Domain<B>>,
    /// Immutable shape and creation/update policy.
    info: GpuBufferInfo,
    /// Static/dynamic have one slot; streamed buffers reserve their fixed slots once.
    slots: Vec<BufferSlot<B>>,
    /// Published slot; changed only after a complete admitted upload.
    current: usize,
    /// Public resource admission is independent of each slot's retained byte receipt.
    _owner: ResourceCharge,
}

impl<B: NativeResources> BufferObject<B> {
    /// Creates all upload capacity and logical reservations before publishing an owner.
    pub(super) fn create(
        domain: &Rc<Domain<B>>,
        desc: GpuBufferDesc,
        usage: GpuBufferUsage,
        source: Option<ComRef<'_, IGpuDataSourceAbi>>,
    ) -> Result<ComBox<IGpuBufferAbi>, NcStatus> {
        domain.open()?;
        let len = desc.byte_len()?;
        let owner = domain.account.resource()?;
        let count = if usage == GpuBufferUsage::Streamed { STREAM_SLOTS } else { 1 };
        let mut slots = Vec::new();
        slots.try_reserve_exact(count).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        for _ in 0..count {
            let charge = Rc::new(domain.account.bytes(len)?);
            let bytes = if let Some(source) = source {
                if source.byte_count()? != len as u64 {
                    return Err(NC_E_INVALID_ARG);
                }
                let mut bytes = zeroed(len)?;
                read_bytes(source, len, |offset, chunk| bytes[offset..offset + chunk.len()].copy_from_slice(chunk))?;
                VersionBytes::Owned(bytes)
            } else {
                VersionBytes::Upload(UploadBytes::new(len)?)
            };
            let core = domain.device.inner.core.clone();
            let native = NativeSlot::default();
            let dirty = Cell::new(false);
            slots.push(match desc.texture_info() {
                Some(info) => BufferSlot::Texture(Rc::new(TextureVersion {
                    core,
                    desc: info,
                    contents: TextureContents::Pixels(bytes),
                    native,
                    charge,
                    dirty,
                    usage,
                })),
                None => BufferSlot::Linear(Rc::new(BufferVersion {
                    core,
                    desc,
                    bytes,
                    native: NativeSlot::default(),
                    _charge: charge,
                    dirty,
                    usage,
                })),
            });
        }
        domain.open()?;
        publish(
            &domain.device.inner.buffers,
            Self::into_local_box(Self {
                domain: domain.clone(),
                info: GpuBufferInfo { desc, usage },
                slots,
                current: 0,
                _owner: owner,
            }),
        )
    }

    /// Imports an immutable actual image into the same public buffer registry.
    pub(super) fn imported(domain: Rc<Domain<B>>, value: Rc<TextureVersion<B>>, owner: ResourceCharge) -> Result<ComBox<IGpuBufferAbi>, NcStatus> {
        let desc = GpuBufferDesc::Texture {
            size: value.desc.size,
            format: value.desc.format,
            mip_count: value.desc.mip_count,
        };
        let registry = domain.device.inner.clone();
        publish(
            &registry.buffers,
            Self::into_local_box(Self {
                domain,
                info: GpuBufferInfo { desc, usage: GpuBufferUsage::Static },
                slots: vec![BufferSlot::Texture(value)],
                current: 0,
                _owner: owner,
            }),
        )
    }

    /// Physical availability is checked independently of public domain authority.
    fn open(&self) -> Result<(), NcStatus> {
        self.domain.open()?;
        B::storage_available(self.domain.device.inner.core.as_ref()).map_err(|_| RENDER_E_BACKEND)
    }
    /// Captures only published linear storage; image roles reject.
    pub(crate) fn linear(&self) -> Result<&Rc<BufferVersion<B>>, NcStatus> {
        self.open()?;
        match &self.slots[self.current] {
            BufferSlot::Linear(value) => Ok(value),
            BufferSlot::Texture(_) => Err(NC_E_INVALID_ARG),
        }
    }
    /// Captures only published sampled pixels; linear roles reject.
    pub(crate) fn texture(&self) -> Result<&Rc<TextureVersion<B>>, NcStatus> {
        self.open()?;
        match &self.slots[self.current] {
            BufferSlot::Texture(value) => Ok(value),
            BufferSlot::Linear(_) => Err(NC_E_INVALID_ARG),
        }
    }
    /// Finds reusable capacity without allocating or guessing from public-owner counts.
    fn available(&self) -> Option<usize> {
        match self.info.usage {
            GpuBufferUsage::Static => None,
            GpuBufferUsage::Dynamic => self.slots[self.current].idle().then_some(self.current),
            GpuBufferUsage::Streamed => (0..self.slots.len())
                .map(|step| (self.current + step) % self.slots.len())
                .find(|slot| self.slots[*slot].idle()),
            _ => None,
        }
    }
    /// Checks the complete byte interval before mutation or output publication.
    fn range(&self, offset: u64, len: usize) -> Result<std::ops::Range<usize>, NcStatus> {
        let first = usize::try_from(offset).map_err(|_| NC_E_OUT_OF_RANGE)?;
        let capacity = self.info.desc.byte_len()?;
        let end = first.checked_add(len).filter(|end| *end <= capacity).ok_or(NC_E_OUT_OF_RANGE)?;
        Ok(first..end)
    }
}

impl<B: NativeResources> IRenderDriverBound for BufferObject<B> {
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        Ok(B::identity(self.domain.device.inner.core.as_ref()).clone())
    }
}
impl<B: NativeResources> IGpuBuffer for BufferObject<B> {
    fn info(&self) -> Result<GpuBufferInfo, NcStatus> {
        self.open()?;
        Ok(self.info)
    }
    fn upload(&mut self, data: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        let _operation = self.domain.device.operation()?;
        self.open()?;
        if self.info.usage == GpuBufferUsage::Static || !self.domain.permissions.write_resources {
            return Err(RENDER_E_PERMISSION);
        }
        let range = self.range(data.offset(), data.len())?;
        if data.is_empty() {
            return Ok(());
        }
        let slot = self.available().ok_or(RENDER_E_BUSY)?;
        let bytes = self.slots[slot].bytes_mut()?;
        // All fallible checks precede mutation. A failed upload leaves current and
        // every captured version unchanged. Streamed updates replace full contents.
        if self.info.usage == GpuBufferUsage::Streamed {
            bytes.fill(0);
        }
        bytes[range].copy_from_slice(data.as_slice());
        self.slots[slot].changed();
        self.current = slot;
        Ok(())
    }
    fn read(&self, mut output: NcOutputWriter<'_, u8>) -> Result<(), NcStatus> {
        let _operation = self.domain.device.operation()?;
        self.open()?;
        let range = self.range(output.offset(), output.capacity())?;
        let bytes = match &self.slots[self.current] {
            BufferSlot::Linear(value) => value.bytes.as_slice(),
            BufferSlot::Texture(value) => match &value.contents {
                TextureContents::Pixels(bytes) => bytes.as_slice(),
                // An imported GPU-only image supplies no CPU representation. This
                // transfer API deliberately does not initiate native GPU readback.
                TextureContents::Imported { .. } => return Err(RENDER_E_PERMISSION),
            },
        };
        output.extend_from_slice(&bytes[range])
    }
}

#[cfg(test)]
mod tests;
