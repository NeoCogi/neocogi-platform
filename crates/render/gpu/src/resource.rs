//! Exclusive public resource owners, weak provenance and immutable data versions.
//!
//! Registry mutation needs interior mutability because shareable factory methods
//! can be called through independently compiled COM clients. Only short local
//! registry borrows are taken; no such borrow spans a foreign reader or destructor.
use crate::{
    abi::*,
    budget::{Account, ByteCharge, ResourceCharge},
    shader::Shader,
};
use neocogi_com::{
    com_object, ComBox, ComInterface, ComObject, ComPtr, ComRef, IUnknown, LocalBox, NcCopyFromBufferOffset, NcOutputBuffer, NcStatus, ThreadLocal, WeakComBox,
    WeakComPtr, WeakLocalBox, NC_E_DROPPED, NC_E_INVALID_ARG, NC_E_OUT_OF_MEMORY, NC_E_OUT_OF_RANGE,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

/// Maximum storage for a single logical vertex/index byte buffer.
pub const MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;
/// Bounded one-way output reads keep codec scratch separate from owned payload admission.
pub const SOURCE_CHUNK: usize = 64 * 1024;

/// Shared logical renderer authority; contains no UI tree or native API callback.
#[derive(Clone)]
pub struct GpuDevice {
    /// Private registries and operation gate shared by factories and recording frames.
    pub(crate) inner: Rc<Device>,
}

/// Registries hold only construction-proven weak observers, never resource owners.
pub(crate) struct Device {
    /// Exclusive buffer objects constructed by this device.
    pub(crate) buffers: RefCell<Registry<BufferObject>>,
    /// Exclusive texture objects constructed by this device.
    pub(crate) textures: RefCell<Registry<TextureObject>>,
    /// Exclusive sampler objects constructed by this device.
    pub(crate) samplers: RefCell<Registry<SamplerObject>>,
    /// Exclusive shader objects constructed by this device.
    pub(crate) shaders: RefCell<Registry<ShaderObject>>,
    /// Exclusive pipeline objects constructed by this device.
    pub(crate) pipelines: RefCell<Registry<PipelineObject>>,
    /// Restricted factories are authenticated without querying a client object.
    pub(crate) factories: RefCell<HashMap<usize, FactoryEntry>>,
    /// Sealed GPU blocks use construction-proven weak provenance too.
    pub(crate) blocks: RefCell<Registry<crate::record::BlockObject>>,
    /// Checked monotonic backing identity; never exposed as provenance proof.
    next: Cell<u64>,
    /// A non-borrowing gate spanning foreign reads or native preparation.
    pub(crate) busy: Cell<bool>,
}

/// Weak factory metadata prevents allocation-address reuse while its registry entry exists.
pub(crate) struct FactoryEntry {
    /// Keeps metadata, not the shareable factory value, alive.
    observer: WeakComPtr<IRenderResourcesAbi>,
    /// Controllers/resources may outlive a factory; this link is deliberately weak.
    domain: Weak<Domain>,
}

/// One host-controlled domain; immutable versions retain its account, not this device owner.
pub(crate) struct Domain {
    /// Logical device used for provenance and guarded creation.
    pub(crate) device: GpuDevice,
    /// Closing gates new admission without freeing retained snapshots.
    pub(crate) closed: Cell<bool>,
    /// Device-free allocation counters.
    pub(crate) account: Rc<Account>,
    /// Weak pending sessions allow immediate close without retaining payloads.
    pub(crate) sessions: RefCell<Vec<Weak<crate::record::SessionCore>>>,
    /// Optional native host lease; private caches never retain this domain.
    _native: Option<ComPtr<IRenderDeviceLeaseAbi>>,
}

impl Domain {
    /// Rechecked after every foreign reader before publication or frame admission.
    pub(crate) fn open(&self) -> Result<(), NcStatus> {
        if self.closed.get() { Err(NC_E_DROPPED) } else { Ok(()) }
    }

    /// Close first, then drain pending payloads outside the registry borrow.
    pub(crate) fn close(&self) {
        self.closed.set(true);
        let sessions = std::mem::take(&mut *self.sessions.borrow_mut());
        for session in sessions {
            if let Some(session) = session.upgrade() {
                session.expire();
            }
        }
    }
}

/// Exact checked weak registration, without an erased implementation payload.
pub(crate) struct Entry<T: ComObject<Threading = ThreadLocal>> {
    /// Construction proves this implementation; with/with_mut take the provider gate.
    observer: WeakLocalBox<T>,
    /// Cached lifetime signal avoids temporary queries during registry sweeps.
    alive: WeakComBox<IUnknown>,
}

/// Typed map for one GPU resource class; arbitrary foreign objects are never downcast.
pub(crate) type Registry<T> = HashMap<usize, Entry<T>>;

/// Operation ownership does not hold a registry/account borrow across callbacks.
pub struct Operation {
    /// Logical device stays alive until the Busy flag is restored.
    inner: Rc<Device>,
}
impl Drop for Operation {
    /// Restore ordinary admission on every success/error/unwind path.
    fn drop(&mut self) {
        self.inner.busy.set(false);
    }
}

impl Default for GpuDevice {
    /// Starts an empty device with independent provenance and monotonic identities.
    fn default() -> Self {
        Self::new()
    }
}
impl GpuDevice {
    /// Creates no native object and retains no UI or client capability.
    pub fn new() -> Self {
        Self {
            inner: Rc::new(Device {
                buffers: RefCell::new(HashMap::new()),
                textures: RefCell::new(HashMap::new()),
                samplers: RefCell::new(HashMap::new()),
                shaders: RefCell::new(HashMap::new()),
                pipelines: RefCell::new(HashMap::new()),
                factories: RefCell::new(HashMap::new()),
                blocks: RefCell::new(HashMap::new()),
                next: Cell::new(0),
                busy: Cell::new(false),
            }),
        }
    }

    /// Creates a host controller for a purely logical domain, useful for independent CPU clients.
    pub fn create_client(&self, limits: RenderClientLimits) -> Result<ComBox<IRenderClientDomainAbi>, NcStatus> {
        self.create_client_inner(limits, None)
    }

    /// Retains a narrow native lifetime token without exposing it through resource QI/getters.
    pub fn create_client_with_lease(
        &self,
        limits: RenderClientLimits,
        native: ComPtr<IRenderDeviceLeaseAbi>,
    ) -> Result<ComBox<IRenderClientDomainAbi>, NcStatus> {
        self.create_client_inner(limits, Some(native))
    }

    /// Publishes separate controller and factory identities only after limits validate.
    fn create_client_inner(
        &self,
        limits: RenderClientLimits,
        native: Option<ComPtr<IRenderDeviceLeaseAbi>>,
    ) -> Result<ComBox<IRenderClientDomainAbi>, NcStatus> {
        let _operation = self.operation()?;
        let domain = Rc::new(Domain {
            device: self.clone(),
            closed: Cell::new(false),
            account: Account::new(limits)?,
            sessions: RefCell::new(Vec::new()),
            _native: native,
        });
        let factory: ComPtr<IRenderResourcesAbi> = Factory::from_value(Factory { domain: domain.clone() }).query()?;
        let entry = FactoryEntry {
            observer: factory.downgrade()?,
            domain: Rc::downgrade(&domain),
        };
        let key = factory.as_raw().object as usize;
        {
            let mut factories = self.inner.factories.borrow_mut();
            // A live factory necessarily owns its domain. Dead domain entries
            // can release weak metadata without consulting foreign code.
            factories.retain(|_, entry| entry.domain.strong_count() > 0);
            factories.try_reserve(1).map_err(|_| NC_E_OUT_OF_MEMORY)?;
            factories.insert(key, entry);
        }
        DomainOwner::into_box(DomainOwner { domain, factory }).query().map_err(|(status, _)| status)
    }

    /// Blocks reentrant factory writes throughout CPU source transactions/native preparation.
    pub fn operation(&self) -> Result<Operation, NcStatus> {
        if self.inner.busy.replace(true) {
            return Err(RENDER_E_BUSY);
        }
        Ok(Operation { inner: self.inner.clone() })
    }

    /// Allocates an internal immutable-version ID, rejecting exhaustion instead of reusing it.
    pub(crate) fn next(&self) -> Result<u64, NcStatus> {
        let next = self.inner.next.get().checked_add(1).ok_or(NC_E_OUT_OF_RANGE)?;
        self.inner.next.set(next);
        Ok(next)
    }

    /// Authenticates the factory's registered identity and checks its current domain gate.
    pub(crate) fn domain(&self, factory: ComRef<'_, IRenderResourcesAbi>) -> Result<Rc<Domain>, NcStatus> {
        let (observer, domain) = {
            let factories = self.inner.factories.try_borrow().map_err(|_| RENDER_E_BUSY)?;
            let entry = factories.get(&(factory.as_raw().object as usize)).ok_or(RENDER_E_OWNER)?;
            (entry.observer.clone(), entry.domain.clone())
        };
        let _live = observer.upgrade()?.ok_or(NC_E_DROPPED)?;
        let domain = domain.upgrade().ok_or(NC_E_DROPPED)?;
        domain.open()?;
        Ok(domain)
    }

    /// Host diagnostics report actual retained logical bytes, including old versions.
    pub fn live_bytes(&self, factory: ComRef<'_, IRenderResourcesAbi>) -> Result<u64, NcStatus> {
        Ok(self.domain(factory)?.account.live_bytes())
    }
}

/// Concrete typed capture methods share one provenance/lifetime rule with all callers.
macro_rules! snapshot_method {
    ($method:ident, $interface:ident, $field:ident, $version:ident) => {
        impl GpuDevice {
            #[doc = concat!("Captures one live same-domain ", stringify!($version), " under its provider reader gate; no public owner is promoted.")]
            pub fn $method(&self, factory: ComRef<'_, IRenderResourcesAbi>, resource: ComRef<'_, $interface>) -> Result<Rc<$version>, NcStatus> {
                let domain = self.domain(factory)?;
                let observer = observe(&self.inner.$field, resource.as_raw().object as usize)?;
                observer.with(|resource| {
                    same_domain(&domain, &resource.domain)?;
                    Ok(resource.current.clone())
                })?
            }
        }
    };
}
snapshot_method!(buffer_snapshot, IGpuBufferAbi, buffers, BufferVersion);
snapshot_method!(texture_snapshot, IGpuTextureAbi, textures, TextureVersion);
snapshot_method!(sampler_snapshot, IGpuSamplerAbi, samplers, SamplerVersion);
snapshot_method!(shader_snapshot, IGpuShaderAbi, shaders, ShaderVersion);
snapshot_method!(pipeline_snapshot, IGpuPipelineAbi, pipelines, PipelineVersion);

/// Immutable logical vertex/index version; native caches may retain this device-free value.
pub struct BufferVersion {
    /// Renderer-private cache identity.
    pub id: u64,
    /// Immutable role/extent.
    pub desc: GpuBufferDesc,
    /// Explicit GPU bytes; no Rust semantic vertex layout is involved.
    pub bytes: Vec<u8>,
    /// Persists through actual last backing ownership.
    _charge: ByteCharge,
}
/// Immutable sampled texture version with tightly packed top-left RGBA pixels.
pub struct TextureVersion {
    /// Renderer-private cache identity.
    pub id: u64,
    /// Immutable sampled texture shape.
    pub desc: GpuTextureDesc,
    /// Complete initialized pixel snapshot.
    pub bytes: Vec<u8>,
    /// Persists while caches/commands retain the version.
    _charge: ByteCharge,
}
/// Immutable sampler state, independent of its exclusive public owner.
pub struct SamplerVersion {
    /// Renderer-private cache identity.
    pub id: u64,
    /// Complete portable filtering/address state.
    pub desc: GpuSamplerDesc,
    /// Device-free logical-state charge.
    _charge: ByteCharge,
}
/// Validated shader CPU artifacts can outlive their public owner in a pipeline.
pub struct ShaderVersion {
    /// Renderer-private cache identity.
    pub id: u64,
    /// Canonical source, native artifacts and reflection.
    pub artifact: Shader,
    /// Owned artifact bytes remain charged through pipeline/cache lifetime.
    _charge: ByteCharge,
}
/// Immutable complete graphics pipeline and captured shader dependencies.
pub struct PipelineVersion {
    /// Renderer-private cache identity.
    pub id: u64,
    /// Fixed triangle pipeline state.
    pub info: GpuPipelineInfo,
    /// Explicit byte stepping for each declared stream.
    pub vertex_buffers: Vec<GpuVertexBufferLayout>,
    /// Explicit attribute footprints and locations.
    pub attributes: Vec<GpuVertexAttribute>,
    /// Exact active stage/slot resource declarations.
    pub bindings: Vec<GpuShaderBinding>,
    /// Captured vertex shader, independent of public shader owner lifetime.
    pub vertex: Rc<ShaderVersion>,
    /// Captured fragment shader, independent of public shader owner lifetime.
    pub fragment: Rc<ShaderVersion>,
    /// Copied descriptor charge, without any device/domain reference.
    pub(crate) _charge: ByteCharge,
}

/// Produces concrete exclusive resource classes without type-erased storage.
macro_rules! resource_object {
    ($name:ident, $interface:ident, $version:ident) => {
        #[com_object(interfaces($interface), ownership = exclusive)]
        /// Exclusive public owner; registry observers retain only lifetime metadata.
        pub(crate) struct $name {
            /// Domain/device authority exists only while this public value exists.
            pub(crate) domain: Rc<Domain>,
            /// Ordinary mutable current pointer, accessed through checked local gates.
            pub(crate) current: Rc<$version>,
            /// Released at public value destruction, independently of version bytes.
            _owner: ResourceCharge,
        }
        impl $interface for $name {}
    };
}
resource_object!(BufferObject, IGpuBuffer, BufferVersion);
resource_object!(TextureObject, IGpuTexture, TextureVersion);
resource_object!(SamplerObject, IGpuSampler, SamplerVersion);
resource_object!(ShaderObject, IGpuShader, ShaderVersion);
resource_object!(PipelineObject, IGpuPipeline, PipelineVersion);

/// Registers only locally constructed metadata before transferring the unique owner.
pub(crate) fn publish<T: ComObject<Threading = ThreadLocal>, I: ComInterface>(
    registry: &RefCell<Registry<T>>,
    local: LocalBox<T>,
) -> Result<ComBox<I>, NcStatus> {
    let projection = local.query::<I>()?;
    let key = projection.as_raw().object as usize;
    let entry = Entry {
        observer: local.downgrade(),
        alive: local.query::<IUnknown>()?,
    };
    {
        let mut registry = registry.try_borrow_mut().map_err(|_| RENDER_E_BUSY)?;
        registry.retain(|_, entry| entry.alive.is_alive());
        registry.try_reserve(1).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        registry.insert(key, entry);
    }
    local.into_interface().map_err(|(status, _)| status)
}

/// Clones construction proof under a short map borrow; the resource gate is acquired afterwards.
pub(crate) fn observe<T: ComObject<Threading = ThreadLocal>>(registry: &RefCell<Registry<T>>, key: usize) -> Result<WeakLocalBox<T>, NcStatus> {
    registry
        .try_borrow()
        .map_err(|_| RENDER_E_BUSY)?
        .get(&key)
        .map(|entry| entry.observer.clone())
        .ok_or(RENDER_E_OWNER)
}

/// Enforces exact device/domain admission without calling a client object.
pub(crate) fn same_domain(expected: &Rc<Domain>, actual: &Rc<Domain>) -> Result<(), NcStatus> {
    expected.open()?;
    if Rc::ptr_eq(expected, actual) { Ok(()) } else { Err(RENDER_E_OWNER) }
}

#[com_object(interfaces(IRenderClientDomain), ownership = exclusive)]
/// Host controller and resource factory deliberately have different QI identities.
struct DomainOwner {
    /// One authoritative close/account record.
    domain: Rc<Domain>,
    /// Shareable restricted creation service delegated explicitly to clients.
    factory: ComPtr<IRenderResourcesAbi>,
}
impl IRenderClientDomain for DomainOwner {
    /// Existing factory clones remain in the same domain and never reset accounting.
    fn resources(&self) -> Result<ComPtr<IRenderResourcesAbi>, NcStatus> {
        self.domain.open()?;
        Ok(self.factory.clone())
    }
    /// Pending session retirement is handled through the shared domain gate.
    fn close(&mut self) -> Result<(), NcStatus> {
        self.domain.close();
        Ok(())
    }
}
impl Drop for DomainOwner {
    /// Controller removal revokes admission but does not free retained backing versions.
    fn drop(&mut self) {
        self.domain.close();
    }
}

#[com_object(interfaces(IRenderResources))]
/// Shareable factory; exclusive resources carry separate identities and ownership.
struct Factory {
    /// Holds device authority while clients deliberately retain this service.
    domain: Rc<Domain>,
}

/// Reserves initialized bytes without panicking on allocation failure.
fn zeroed(len: usize) -> Result<Vec<u8>, NcStatus> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| NC_E_OUT_OF_MEMORY)?;
    bytes.resize(len, 0);
    Ok(bytes)
}

/// Copies exact bounded output prefixes; callbacks see no resource/registry/account guard.
fn read_bytes(source: ComRef<'_, IGpuDataSourceAbi>, len: usize, mut write: impl FnMut(usize, &[u8])) -> Result<(), NcStatus> {
    let mut output = NcOutputBuffer::new();
    for offset in (0..len).step_by(SOURCE_CHUNK) {
        let wanted = (len - offset).min(SOURCE_CHUNK);
        source.read(output.output(offset as u64, wanted))?;
        if output.len() != wanted {
            return Err(NC_E_INVALID_ARG);
        }
        write(offset, output.as_slice());
    }
    if source.byte_count()? != len as u64 {
        return Err(NC_E_INVALID_ARG);
    }
    Ok(())
}

/// Role separation is mandatory even on native APIs that permit mixed bindings.
fn buffer_desc(desc: GpuBufferDesc) -> Result<usize, NcStatus> {
    if !matches!(desc.role, GpuBufferRole::Vertex | GpuBufferRole::Index) {
        return Err(NC_E_INVALID_ARG);
    }
    usize::try_from(desc.byte_len)
        .ok()
        .filter(|len| *len > 0 && *len <= MAX_BUFFER_BYTES)
        .ok_or(NC_E_OUT_OF_RANGE)
}

/// Baseline sampled textures have one tightly packed RGBA8 mip, no target usage.
fn texture_desc(desc: GpuTextureDesc) -> Result<usize, NcStatus> {
    if desc.mip_count != 1 || !matches!(desc.format, GpuTextureFormat::Rgba8Unorm | GpuTextureFormat::Rgba8Srgb) {
        return Err(NC_E_INVALID_ARG);
    }
    rgba_bytes(desc.size)
}

/// Checks all open enum values before they can reach a backend state mapping.
fn sampler_desc(desc: GpuSamplerDesc) -> Result<(), NcStatus> {
    if [desc.min_filter, desc.mag_filter]
        .iter()
        .any(|filter| !matches!(*filter, GpuFilter::Nearest | GpuFilter::Linear))
        || [desc.address_u, desc.address_v]
            .iter()
            .any(|address| !matches!(*address, GpuAddressMode::Clamp | GpuAddressMode::Repeat | GpuAddressMode::Mirror))
    {
        return Err(NC_E_INVALID_ARG);
    }
    Ok(())
}

impl IRenderResources for Factory {
    /// Whole-version quota admission precedes bulk reads; public owner is transferred last.
    fn create_buffer(&self, desc: GpuBufferDesc, source: ComRef<'_, IGpuDataSourceAbi>) -> Result<ComBox<IGpuBufferAbi>, NcStatus> {
        let _operation = self.domain.device.operation()?;
        self.domain.open()?;
        let len = buffer_desc(desc)?;
        if source.byte_count()? != len as u64 {
            return Err(NC_E_INVALID_ARG);
        }
        let owner = self.domain.account.resource()?;
        let charge = self.domain.account.bytes(len)?;
        let mut bytes = zeroed(len)?;
        read_bytes(source, len, |offset, chunk| bytes[offset..offset + chunk.len()].copy_from_slice(chunk))?;
        self.domain.open()?;
        let current = Rc::new(BufferVersion {
            id: self.domain.device.next()?,
            desc,
            bytes,
            _charge: charge,
        });
        publish(
            &self.domain.device.inner.buffers,
            BufferObject::into_local_box(BufferObject {
                domain: self.domain.clone(),
                current,
                _owner: owner,
            }),
        )
    }

    /// Publishes immutable pixels independently from ordinary shared ITexture capabilities.
    fn create_texture(&self, desc: GpuTextureDesc, source: ComRef<'_, IGpuDataSourceAbi>) -> Result<ComBox<IGpuTextureAbi>, NcStatus> {
        let _operation = self.domain.device.operation()?;
        self.domain.open()?;
        let len = texture_desc(desc)?;
        if source.byte_count()? != len as u64 {
            return Err(NC_E_INVALID_ARG);
        }
        let owner = self.domain.account.resource()?;
        let charge = self.domain.account.bytes(len)?;
        let mut bytes = zeroed(len)?;
        read_bytes(source, len, |offset, chunk| bytes[offset..offset + chunk.len()].copy_from_slice(chunk))?;
        self.domain.open()?;
        let current = Rc::new(TextureVersion {
            id: self.domain.device.next()?,
            desc,
            bytes,
            _charge: charge,
        });
        publish(
            &self.domain.device.inner.textures,
            TextureObject::into_local_box(TextureObject {
                domain: self.domain.clone(),
                current,
                _owner: owner,
            }),
        )
    }

    /// Filtering/addressing state contains no resource lookup or native callback.
    fn create_sampler(&self, desc: GpuSamplerDesc) -> Result<ComBox<IGpuSamplerAbi>, NcStatus> {
        let _operation = self.domain.device.operation()?;
        self.domain.open()?;
        sampler_desc(desc)?;
        let owner = self.domain.account.resource()?;
        let current = Rc::new(SamplerVersion {
            id: self.domain.device.next()?,
            desc,
            _charge: self.domain.account.bytes(std::mem::size_of::<GpuSamplerDesc>())?,
        });
        publish(
            &self.domain.device.inner.samplers,
            SamplerObject::into_local_box(SamplerObject {
                domain: self.domain.clone(),
                current,
                _owner: owner,
            }),
        )
    }

    /// Canonical UTF-8 source goes through shared profile validation before logical publication.
    fn create_shader(
        &self,
        stage: GpuShaderStage,
        source: ComRef<'_, IGpuDataSourceAbi>,
        entry: NcCopyFromBufferOffset<'_, u8>,
    ) -> Result<ComBox<IGpuShaderAbi>, NcStatus> {
        let _operation = self.domain.device.operation()?;
        self.domain.open()?;
        if entry.offset() != 0 {
            return Err(NC_E_INVALID_ARG);
        }
        let entry = std::str::from_utf8(entry.as_slice()).map_err(|_| NC_E_INVALID_ARG)?;
        if entry.is_empty() || entry.len() > 128 || !matches!(stage, GpuShaderStage::Vertex | GpuShaderStage::Fragment) {
            return Err(NC_E_INVALID_ARG);
        }
        let len = usize::try_from(source.byte_count()?).map_err(|_| NC_E_OUT_OF_RANGE)?;
        if len == 0 || len > crate::shader::MAX_SHADER_BYTES {
            return Err(NC_E_OUT_OF_RANGE);
        }
        let owner = self.domain.account.resource()?;
        let input_charge = self.domain.account.bytes(len)?;
        let mut bytes = zeroed(len)?;
        read_bytes(source, len, |offset, chunk| bytes[offset..offset + chunk.len()].copy_from_slice(chunk))?;
        let text = std::str::from_utf8(&bytes).map_err(|_| NC_E_INVALID_ARG)?;
        let remaining = usize::try_from(self.domain.account.remaining_bytes()).map_err(|_| NC_E_OUT_OF_RANGE)?;
        let artifact = Shader::compile_bounded(stage, text, entry, remaining).map_err(|error| match error {
            crate::shader::CompileError::Invalid(_) => NC_E_INVALID_ARG,
            crate::shader::CompileError::Limit => NC_E_OUT_OF_RANGE,
            crate::shader::CompileError::Allocation => NC_E_OUT_OF_MEMORY,
        })?;
        let charge = self.domain.account.bytes(artifact.byte_len())?;
        drop(input_charge);
        self.domain.open()?;
        let current = Rc::new(ShaderVersion {
            id: self.domain.device.next()?,
            artifact,
            _charge: charge,
        });
        publish(
            &self.domain.device.inner.shaders,
            ShaderObject::into_local_box(ShaderObject {
                domain: self.domain.clone(),
                current,
                _owner: owner,
            }),
        )
    }

    /// Copies finite descriptor arrays and captures live same-domain shaders under local gates.
    fn create_pipeline(
        &self,
        source: ComRef<'_, IGpuPipelineSourceAbi>,
        vertex: ComRef<'_, IGpuShaderAbi>,
        fragment: ComRef<'_, IGpuShaderAbi>,
    ) -> Result<ComBox<IGpuPipelineAbi>, NcStatus> {
        crate::pipeline::create(&self.domain, source, vertex, fragment)
    }

    /// Copy-on-write publication happens under the resource writer gate after the last callback.
    fn update_buffer(&self, buffer: ComRef<'_, IGpuBufferAbi>, offset: u64, source: ComRef<'_, IGpuDataSourceAbi>) -> Result<(), NcStatus> {
        let _operation = self.domain.device.operation()?;
        self.domain.open()?;
        let mut observer = observe(&self.domain.device.inner.buffers, buffer.as_raw().object as usize)?;
        let previous = observer.with(|buffer| {
            same_domain(&self.domain, &buffer.domain)?;
            Ok(buffer.current.clone())
        })??;
        let count = usize::try_from(source.byte_count()?).map_err(|_| NC_E_OUT_OF_RANGE)?;
        let start = usize::try_from(offset).map_err(|_| NC_E_OUT_OF_RANGE)?;
        let end = start.checked_add(count).filter(|end| *end <= previous.bytes.len()).ok_or(NC_E_OUT_OF_RANGE)?;
        let charge = self.domain.account.bytes(previous.bytes.len())?;
        let mut bytes = zeroed(previous.bytes.len())?;
        bytes.copy_from_slice(&previous.bytes);
        read_bytes(source, count, |offset, chunk| {
            bytes[start + offset..start + offset + chunk.len()].copy_from_slice(chunk)
        })?;
        debug_assert!(end <= bytes.len());
        let current = Rc::new(BufferVersion {
            id: self.domain.device.next()?,
            desc: previous.desc,
            bytes,
            _charge: charge,
        });
        let retired = observer.with_mut(|buffer| {
            same_domain(&self.domain, &buffer.domain)?;
            if buffer.current.id != previous.id {
                return Err(RENDER_E_BUSY);
            }
            Ok(std::mem::replace(&mut buffer.current, current))
        })??;
        drop(retired);
        Ok(())
    }

    /// Row-aware copy of a bounded patch; only one 64KiB foreign output region exists at a time.
    fn update_texture(&self, texture: ComRef<'_, IGpuTextureAbi>, rect: Rect, source: ComRef<'_, IGpuDataSourceAbi>) -> Result<(), NcStatus> {
        let _operation = self.domain.device.operation()?;
        self.domain.open()?;
        let mut observer = observe(&self.domain.device.inner.textures, texture.as_raw().object as usize)?;
        let previous = observer.with(|texture| {
            same_domain(&self.domain, &texture.domain)?;
            Ok(texture.current.clone())
        })??;
        if rect.x < 0
            || rect.y < 0
            || rect.width <= 0
            || rect.height <= 0
            || i64::from(rect.x) + i64::from(rect.width) > i64::from(previous.desc.size.width)
            || i64::from(rect.y) + i64::from(rect.height) > i64::from(previous.desc.size.height)
        {
            return Err(NC_E_OUT_OF_RANGE);
        }
        let count = rgba_bytes(Size { width: rect.width, height: rect.height })?;
        if source.byte_count()? != count as u64 {
            return Err(NC_E_INVALID_ARG);
        }
        let charge = self.domain.account.bytes(previous.bytes.len())?;
        let mut bytes = zeroed(previous.bytes.len())?;
        bytes.copy_from_slice(&previous.bytes);
        let row = rect.width as usize * 4;
        let stride = previous.desc.size.width as usize * 4;
        read_bytes(source, count, |offset, chunk| {
            let mut consumed = 0;
            while consumed < chunk.len() {
                let source_offset = offset + consumed;
                let column = source_offset % row;
                let wanted = (row - column).min(chunk.len() - consumed);
                let destination = (rect.y as usize + source_offset / row) * stride + rect.x as usize * 4 + column;
                bytes[destination..destination + wanted].copy_from_slice(&chunk[consumed..consumed + wanted]);
                consumed += wanted;
            }
        })?;
        let current = Rc::new(TextureVersion {
            id: self.domain.device.next()?,
            desc: previous.desc,
            bytes,
            _charge: charge,
        });
        let retired = observer.with_mut(|texture| {
            same_domain(&self.domain, &texture.domain)?;
            if texture.current.id != previous.id {
                return Err(RENDER_E_BUSY);
            }
            Ok(std::mem::replace(&mut texture.current, current))
        })??;
        drop(retired);
        Ok(())
    }
}

/// Constructs one fully validated immutable pipeline after all foreign descriptor reads.
pub(crate) fn publish_pipeline(domain: &Rc<Domain>, version: PipelineVersion, owner: ResourceCharge) -> Result<ComBox<IGpuPipelineAbi>, NcStatus> {
    domain.open()?;
    let current = Rc::new(version);
    publish(
        &domain.device.inner.pipelines,
        PipelineObject::into_local_box(PipelineObject {
            domain: domain.clone(),
            current,
            _owner: owner,
        }),
    )
}

#[cfg(test)]
mod tests;
