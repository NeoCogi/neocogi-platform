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
use neocogi_render_core::TextureId;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
    sync::atomic::{AtomicU64, Ordering},
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
    /// Required compiler capability, acquired once by the renderer assembly.
    compiler: ComPtr<neocogi_shader_core::abi::ShaderCompilerAbi>,
    /// One bounded diagnostic shared with the native renderer. Interior mutability
    /// supports shareable COM methods, with no borrow across foreign calls.
    diagnostic: RefCell<String>,
    /// Retained identity minted for this concrete driver instance.
    identity: ComPtr<IRenderDriverIdentityAbi>,
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
    /// Established by trusted host construction, never copied from caller-reported metadata.
    pub(crate) kind: DomainKind,
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

/// Trusted compositor provenance is separate from every publicly created client domain.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum DomainKind {
    /// Scoped programmable client work initializes independent depth at its ordered position.
    Client,
    /// Private GFX work shares the vocabulary but may not change attachment depth.
    Ui,
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

impl GpuDevice {
    /// Creates no native object and retains no UI or client capability.
    pub fn new(compiler: ComPtr<neocogi_shader_core::abi::ShaderCompilerAbi>) -> Self {
        Self {
            inner: Rc::new(Device {
                compiler,
                diagnostic: RefCell::new(String::new()),
                identity: neocogi_render_core::source::new_driver_identity(),
                buffers: RefCell::new(HashMap::new()),
                textures: RefCell::new(HashMap::new()),
                samplers: RefCell::new(HashMap::new()),
                shaders: RefCell::new(HashMap::new()),
                pipelines: RefCell::new(HashMap::new()),
                factories: RefCell::new(HashMap::new()),
                blocks: RefCell::new(HashMap::new()),
                busy: Cell::new(false),
            }),
        }
    }

    /// Stable compatibility metadata; this getter grants no renderer or factory authority.
    pub fn driver_identity(&self) -> ComPtr<IRenderDriverIdentityAbi> {
        self.inner.identity.clone()
    }

    /// Copies current bounded detail for the native diagnostic facade.
    pub fn diagnostic(&self) -> String {
        self.inner.diagnostic.borrow().clone()
    }

    /// Replaces detail after formatting outside the store borrow; no foreign code
    /// runs while it is borrowed. The store owns at most 4 KiB of UTF-8 text.
    pub fn set_diagnostic(&self, mut detail: String) {
        let mut end = detail.len().min(neocogi_shader_core::MAX_DIAGNOSTIC_BYTES);
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        detail.truncate(end);
        *self.inner.diagnostic.borrow_mut() = detail;
    }

    /// Creates a host controller for a purely logical domain, useful for independent CPU clients.
    pub fn create_client(&self, limits: RenderClientLimits) -> Result<ComBox<IRenderClientDomainAbi>, NcStatus> {
        self.create_client_inner(limits, None, DomainKind::Client)
    }

    /// Retains a narrow native lifetime token without exposing it through resource QI/getters.
    pub fn create_client_with_lease(
        &self,
        limits: RenderClientLimits,
        native: ComPtr<IRenderDeviceLeaseAbi>,
    ) -> Result<ComBox<IRenderClientDomainAbi>, NcStatus> {
        self.create_client_inner(limits, Some(native), DomainKind::Client)
    }

    /// Trusted host assembly creates a private compositor domain. Its factory must stay inside GFX.
    /// Public client creation never selects this provenance, and resource QI cannot obtain it.
    pub fn create_ui_client(
        &self,
        limits: RenderClientLimits,
        native: Option<ComPtr<IRenderDeviceLeaseAbi>>,
    ) -> Result<ComBox<IRenderClientDomainAbi>, NcStatus> {
        self.create_client_inner(limits, native, DomainKind::Ui)
    }

    /// Publishes separate controller and factory identities only after limits validate.
    fn create_client_inner(
        &self,
        limits: RenderClientLimits,
        native: Option<ComPtr<IRenderDeviceLeaseAbi>>,
        kind: DomainKind,
    ) -> Result<ComBox<IRenderClientDomainAbi>, NcStatus> {
        let _operation = self.operation()?;
        let domain = Rc::new(Domain {
            kind,
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

    /// Trusted native assembly authenticates the immutable image and supplies its actual native key.
    /// This bridge is restricted to private UI domains and has no public client-resource method.
    pub fn import_ui_texture(
        &self,
        factory: ComRef<'_, IRenderResourcesAbi>,
        id: TextureId,
        image: ComPtr<ITextureAbi>,
    ) -> Result<ComBox<IGpuTextureAbi>, NcStatus> {
        let _operation = self.operation()?;
        let domain = self.domain(factory)?;
        if domain.kind != DomainKind::Ui {
            return Err(RENDER_E_OWNER);
        }
        let desc = GpuTextureDesc {
            size: Size { width: id.width(), height: id.height() },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        };
        let len = texture_desc(desc)?;
        let owner = domain.account.resource()?;
        let current = Rc::new(TextureVersion {
            id: next_version_id()?,
            desc,
            contents: TextureContents::Imported { id, _image: image },
            charge: Rc::new(domain.account.bytes(len)?),
        });
        publish(
            &self.inner.textures,
            TextureObject::into_local_box(TextureObject { domain, current, _owner: owner }),
        )
    }

    /// Native preflight for a private UI upload cannot accept a client factory or foreign resource.
    pub fn ui_texture_snapshot(&self, factory: ComRef<'_, IRenderResourcesAbi>, texture: ComRef<'_, IGpuTextureAbi>) -> Result<Rc<TextureVersion>, NcStatus> {
        let _operation = self.operation()?;
        if self.domain(factory)?.kind != DomainKind::Ui {
            return Err(RENDER_E_OWNER);
        }
        self.texture_snapshot(factory, texture)
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

/// Immutable initialized contents with allocation-only zeros represented without a CPU payload.
/// Logical bounds and quotas apply equally to both variants; native readers must initialize zeros.
pub enum VersionBytes {
    /// Descriptor-sized transparent/zero contents, without a full CPU zero vector.
    Zeroed {
        /// Admitted logical byte count.
        len: usize,
    },
    /// Complete initialized bytes supplied or atomically materialized by an update.
    Owned(Vec<u8>),
}
impl VersionBytes {
    /// Admitted logical size independent of whether bytes were materialized.
    pub fn len(&self) -> usize {
        match self {
            Self::Zeroed { len } => *len,
            Self::Owned(bytes) => bytes.len(),
        }
    }
    /// Whether the logical extent is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Materialized bytes, or None when the complete logical extent is zero.
    pub fn as_slice(&self) -> Option<&[u8]> {
        match self {
            Self::Zeroed { .. } => None,
            Self::Owned(bytes) => Some(bytes),
        }
    }
    /// Copies the complete defined contents into an already admitted output allocation.
    pub fn copy_into(&self, output: &mut [u8]) -> Result<(), NcStatus> {
        if output.len() != self.len() {
            return Err(NC_E_INVALID_ARG);
        }
        match self {
            Self::Zeroed { .. } => output.fill(0),
            Self::Owned(bytes) => output.copy_from_slice(bytes),
        }
        Ok(())
    }
    /// Reads a small checked byte interval without materializing zero storage.
    pub fn read(&self, offset: usize, output: &mut [u8]) -> Result<(), NcStatus> {
        let end = offset.checked_add(output.len()).filter(|end| *end <= self.len()).ok_or(NC_E_OUT_OF_RANGE)?;
        match self {
            Self::Zeroed { .. } => output.fill(0),
            Self::Owned(bytes) => output.copy_from_slice(&bytes[offset..end]),
        }
        Ok(())
    }
}

/// Immutable logical vertex/index version; native caches may retain this device-free value.
pub struct BufferVersion {
    /// Renderer-private cache identity.
    pub(crate) id: u64,
    /// Immutable role/extent.
    pub(crate) desc: GpuBufferDesc,
    /// Explicit GPU bytes; no Rust semantic vertex layout is involved.
    pub(crate) bytes: VersionBytes,
    /// Persists through actual last backing ownership.
    _charge: ByteCharge,
}
/// Immutable sampled texture version with tightly packed top-left RGBA pixels.
pub struct TextureVersion {
    /// Renderer-private cache identity.
    pub(crate) id: u64,
    /// Immutable sampled texture shape.
    pub(crate) desc: GpuTextureDesc,
    /// Pixel storage or a trusted immutable-image lease; never an editable native handle.
    contents: TextureContents,
    /// Native lowering retains this device-free receipt instead of an imported COM/device lease.
    charge: Rc<ByteCharge>,
}

/// An ordinary immutable image can be sampled privately without exposing a client conversion API.
enum TextureContents {
    /// Complete initialized row-major bytes; updates publish a fresh complete version.
    Pixels(VersionBytes),
    /// Authenticated host key and the immutable public lifetime needed during CPU preparation.
    Imported {
        /// Resolved by the same native device's private image map.
        id: TextureId,
        /// Never retained in native caches or GPU completion collections.
        _image: ComPtr<ITextureAbi>,
    },
}

impl TextureVersion {
    /// Imported native images have no CPU pixel buffer and cannot be updated through the factory.
    pub fn pixels(&self) -> Option<&VersionBytes> {
        match &self.contents {
            TextureContents::Pixels(bytes) => Some(bytes),
            TextureContents::Imported { .. } => None,
        }
    }

    /// A native executor resolves only keys installed by trusted UI import on this exact device.
    pub fn imported_id(&self) -> Option<TextureId> {
        match self.contents {
            TextureContents::Imported { id, .. } => Some(id),
            TextureContents::Pixels(_) => None,
        }
    }

    /// Strip COM/native-facade ownership before native caching or submission retention.
    pub fn receipt(self: &Rc<Self>) -> TextureReceipt {
        TextureReceipt {
            alive: Rc::downgrade(self),
            _charge: self.charge.clone(),
        }
    }
}

/// Device-free native image accounting plus passive private-version retirement metadata.
pub struct TextureReceipt {
    /// Retaining metadata cannot retain the imported image or its native device facade.
    alive: Weak<TextureVersion>,
    /// Refunded only when all CPU/native/completion owners release the actual backing charge.
    _charge: Rc<ByteCharge>,
}
impl TextureReceipt {
    /// Cache-only entries retire after private CPU ownership ends; in-flight owners stay separate.
    pub fn is_alive(&self) -> bool {
        self.alive.strong_count() > 0
    }
}
/// Immutable sampler state, independent of its exclusive public owner.
pub struct SamplerVersion {
    /// Renderer-private cache identity.
    pub(crate) id: u64,
    /// Complete portable filtering/address state.
    pub(crate) desc: GpuSamplerDesc,
    /// Device-free logical-state charge.
    _charge: ByteCharge,
}
/// Validated shader CPU artifacts can outlive their public owner in a pipeline.
pub struct ShaderVersion {
    /// Renderer-private cache identity.
    pub(crate) id: u64,
    /// Canonical source, native artifacts and reflection.
    pub(crate) artifact: Shader,
    /// Owned artifact bytes remain charged through pipeline/cache lifetime.
    _charge: ByteCharge,
}
/// Immutable complete graphics pipeline and captured shader dependencies.
pub struct PipelineVersion {
    /// Renderer-private cache identity.
    pub(crate) id: u64,
    /// Fixed triangle pipeline state.
    pub(crate) info: GpuPipelineInfo,
    /// Explicit byte stepping for each declared stream.
    pub(crate) vertex_buffers: Vec<GpuVertexBufferLayout>,
    /// Explicit attribute footprints and locations.
    pub(crate) attributes: Vec<GpuVertexAttribute>,
    /// Exact active stage/slot resource declarations.
    pub(crate) bindings: Vec<GpuShaderBinding>,
    /// Captured vertex shader, independent of public shader owner lifetime.
    pub(crate) vertex: Rc<ShaderVersion>,
    /// Captured fragment shader, independent of public shader owner lifetime.
    pub(crate) fragment: Rc<ShaderVersion>,
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
        impl IRenderDriverBound for $name {
            fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
                Ok(self.domain.device.driver_identity())
            }
        }
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

impl IRenderDriverBound for Factory {
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        Ok(self.domain.device.driver_identity())
    }
}
impl IRenderResources for Factory {
    /// Whole-version quota admission precedes bulk reads; public owner is transferred last.
    fn create_buffer(&self, desc: GpuBufferDesc, source: neocogi_com::NcValueRef<'_, InitialGpuData>) -> Result<ComBox<IGpuBufferAbi>, NcStatus> {
        let _operation = self.domain.device.operation()?;
        self.domain.open()?;
        let len = buffer_desc(desc)?;
        let owner = self.domain.account.resource()?;
        let charge = self.domain.account.bytes(len)?;
        let bytes = match neocogi_render_core::source::read_initial(source, len)? {
            Some(bytes) => VersionBytes::Owned(bytes),
            None => VersionBytes::Zeroed { len },
        };
        self.domain.open()?;
        let current = Rc::new(BufferVersion {
            id: next_version_id()?,
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
    fn create_texture(&self, desc: GpuTextureDesc, source: neocogi_com::NcValueRef<'_, InitialGpuData>) -> Result<ComBox<IGpuTextureAbi>, NcStatus> {
        let _operation = self.domain.device.operation()?;
        self.domain.open()?;
        let len = texture_desc(desc)?;
        let owner = self.domain.account.resource()?;
        let charge = self.domain.account.bytes(len)?;
        let bytes = match neocogi_render_core::source::read_initial(source, len)? {
            Some(bytes) => VersionBytes::Owned(bytes),
            None => VersionBytes::Zeroed { len },
        };
        self.domain.open()?;
        let current = Rc::new(TextureVersion {
            id: next_version_id()?,
            desc,
            contents: TextureContents::Pixels(bytes),
            charge: Rc::new(charge),
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
            id: next_version_id()?,
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
        self.domain.device.set_diagnostic(String::new());
        // The busy gate and provider receipt enclose every callback and final Release.
        // Input allocations die before their receipt; native publication follows the
        // final foreign destructor and a fresh domain-open check.
        let imported = (|| {
            if entry.offset() != 0 {
                return Err(NC_E_INVALID_ARG);
            }
            let entry = entry.as_slice();
            if entry.is_empty()
                || entry.len() > 128
                || std::str::from_utf8(entry).is_err()
                || !matches!(stage, GpuShaderStage::Vertex | GpuShaderStage::Fragment)
            {
                return Err(NC_E_INVALID_ARG);
            }
            let mut expected = [0u8; 128];
            expected[..entry.len()].copy_from_slice(entry);
            let len = usize::try_from(source.byte_count()?).map_err(|_| NC_E_OUT_OF_RANGE)?;
            self.domain.open()?;
            if len == 0 || len > crate::shader::MAX_SHADER_BYTES {
                return Err(NC_E_OUT_OF_RANGE);
            }
            let owner = self.domain.account.resource()?;
            let input_charge = self.domain.account.bytes(len)?;
            let mut bytes = zeroed(len)?;
            read_bytes(source, len, |offset, chunk| bytes[offset..offset + chunk.len()].copy_from_slice(chunk))?;
            self.domain.open()?;
            let cap = (self.domain.account.remaining_bytes().min(crate::shader::MAX_SHADER_ARTIFACT_BYTES as u64)) as usize;
            let mut provider_charge = self.domain.account.bytes(cap)?;
            let result = self.domain.device.inner.compiler.compile(
                neocogi_shader_core::abi::ShaderCompileRequest { stage, max_output_bytes: cap as u64 },
                NcCopyFromBufferOffset::new(0, &bytes),
                NcCopyFromBufferOffset::new(0, &expected[..entry.len()]),
            );
            drop(bytes);
            drop(input_charge);
            let report = result?;
            let status = NcStatus::from_code(report.status()?);
            if status != neocogi_com::NC_OK {
                use neocogi_com::INcStringComPtrExt;
                let detail = report.diagnostic()?.to_string_bounded(neocogi_shader_core::MAX_DIAGNOSTIC_BYTES)?;
                self.domain.device.set_diagnostic(detail);
                return Err(status.normalize_error());
            }
            let prepared = neocogi_shader_core::PreparedShader::new(report, stage, &expected[..entry.len()], cap)?;
            provider_charge.shrink(prepared.byte_len())?;
            let charge = self.domain.account.bytes(prepared.byte_len())?;
            let artifact = prepared.copy()?;
            drop(prepared);
            drop(provider_charge);
            self.domain.open()?;
            Ok((owner, charge, artifact))
        })();
        let (owner, charge, artifact) = imported.inspect_err(|status| {
            if self.domain.device.inner.diagnostic.borrow().is_empty() {
                self.domain.device.set_diagnostic(format!("shader creation failed: {status}"));
            }
        })?;
        let current = Rc::new(ShaderVersion {
            id: next_version_id()?,
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
        previous.bytes.copy_into(&mut bytes)?;
        read_bytes(source, count, |offset, chunk| {
            bytes[start + offset..start + offset + chunk.len()].copy_from_slice(chunk)
        })?;
        debug_assert!(end <= bytes.len());
        let current = Rc::new(BufferVersion {
            id: next_version_id()?,
            desc: previous.desc,
            bytes: VersionBytes::Owned(bytes),
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
        let previous_bytes = previous.pixels().ok_or(NC_E_INVALID_ARG)?;
        let count = rgba_bytes(Size { width: rect.width, height: rect.height })?;
        if source.byte_count()? != count as u64 {
            return Err(NC_E_INVALID_ARG);
        }
        let charge = self.domain.account.bytes(previous_bytes.len())?;
        let mut bytes = zeroed(previous_bytes.len())?;
        previous_bytes.copy_into(&mut bytes)?;
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
            id: next_version_id()?,
            desc: previous.desc,
            contents: TextureContents::Pixels(VersionBytes::Owned(bytes)),
            charge: Rc::new(charge),
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

#[cfg(test)]
mod compiler_tests;

impl BufferVersion {
    /// Reads the admitted id without granting mutation authority.
    pub fn id(&self) -> u64 {
        self.id
    }
    /// Reads the admitted desc without granting mutation authority.
    pub fn desc(&self) -> GpuBufferDesc {
        self.desc
    }
    /// Reads the admitted bytes without granting mutation authority.
    pub fn bytes(&self) -> &VersionBytes {
        &self.bytes
    }
}

impl TextureVersion {
    /// Reads the admitted id without granting mutation authority.
    pub fn id(&self) -> u64 {
        self.id
    }
    /// Reads the admitted desc without granting mutation authority.
    pub fn desc(&self) -> GpuTextureDesc {
        self.desc
    }
}

impl SamplerVersion {
    /// Reads the admitted id without granting mutation authority.
    pub fn id(&self) -> u64 {
        self.id
    }
    /// Reads the admitted desc without granting mutation authority.
    pub fn desc(&self) -> GpuSamplerDesc {
        self.desc
    }
}

impl ShaderVersion {
    /// Reads the admitted id without granting mutation authority.
    pub fn id(&self) -> u64 {
        self.id
    }
    /// Reads the admitted artifact without granting mutation authority.
    pub fn artifact(&self) -> &Shader {
        &self.artifact
    }
}

impl PipelineVersion {
    /// Reads the admitted id without granting mutation authority.
    pub fn id(&self) -> u64 {
        self.id
    }
    /// Reads the admitted info without granting mutation authority.
    pub fn info(&self) -> GpuPipelineInfo {
        self.info
    }
    /// Reads the admitted vertex buffers without granting mutation authority.
    pub fn vertex_buffers(&self) -> &[GpuVertexBufferLayout] {
        &self.vertex_buffers
    }
    /// Reads the admitted attributes without granting mutation authority.
    pub fn attributes(&self) -> &[GpuVertexAttribute] {
        &self.attributes
    }
    /// Reads the admitted bindings without granting mutation authority.
    pub fn bindings(&self) -> &[GpuShaderBinding] {
        &self.bindings
    }
    /// Reads the admitted vertex without granting mutation authority.
    pub fn vertex(&self) -> &Rc<ShaderVersion> {
        &self.vertex
    }
    /// Reads the admitted fragment without granting mutation authority.
    pub fn fragment(&self) -> &Rc<ShaderVersion> {
        &self.fragment
    }
}

/// Cache identity spans logical devices that can feed the same native adapter.
/// It never authenticates COM provenance and owns no device or resource lifetime.
static NEXT_VERSION_ID: AtomicU64 = AtomicU64::new(1);

/// Allocates identity only; relaxed ordering does not publish resource contents.
fn allocate_version_id(sequence: &AtomicU64) -> Result<u64, NcStatus> {
    sequence
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
        .map_err(|_| NC_E_OUT_OF_RANGE)
}

/// All creation/import/update paths use the same checked, non-reusing sequence.
pub(crate) fn next_version_id() -> Result<u64, NcStatus> {
    allocate_version_id(&NEXT_VERSION_ID)
}
