//! Exclusive public resource owners, weak provenance and immutable data versions.
//!
//! Registry mutation needs interior mutability because shareable factory methods
//! can be called through independently compiled COM clients. Only short local
//! registry borrows are taken; no such borrow spans a foreign reader or destructor.
mod buffer;
pub(crate) use buffer::BufferObject;

use crate::gpu::{
    abi::*,
    budget::{Account, ByteCharge, ResourceCharge},
    shader::Shader,
};
use neocogi_com::{
    com_object, ComBox, ComInterface, ComObject, ComPtr, ComRef, IUnknown, LocalBox, NcCopyFromBufferOffset, NcOutputBuffer, NcStatus, ThreadLocal, WeakComBox,
    WeakComPtr, WeakLocalBox, NC_E_DROPPED, NC_E_INVALID_ARG, NC_E_OUT_OF_MEMORY, NC_E_OUT_OF_RANGE,
};
use crate::gpu::native::{NativeResources, NativeSlot, check_core};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

/// Shared maximum logical slot capacity for linear buffers and texture pixels.
pub const MAX_BUFFER_BYTES: usize = MAX_IMAGE_BYTES;
/// Bounded one-way output reads keep codec scratch separate from owned payload admission.
pub const SOURCE_CHUNK: usize = 64 * 1024;

/// Shared logical renderer authority; contains no UI tree or native API callback.
pub struct GpuDevice<B: crate::gpu::native::NativeResources> {
    /// Private registries and operation gate shared by factories and recording frames.
    pub(crate) inner: Rc<Device<B>>,
}

impl<B: NativeResources> Clone for GpuDevice<B> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

/// Registries hold only construction-proven weak observers, never resource owners.
pub(crate) struct Device<B: crate::gpu::native::NativeResources> {
    /// Required compiler capability, acquired once by the renderer assembly.
    compiler: ComPtr<neocogi_platform::render::abi::ShaderCompilerAbi>,
    /// Actual compatible native instance retained without any renderer or submission owner.
    pub(crate) core: Rc<B::Core>,
    /// Exclusive buffer objects constructed by this device.
    pub(crate) buffers: RefCell<Registry<BufferObject<B>>>,
    /// Exclusive sampler objects constructed by this device.
    pub(crate) samplers: RefCell<Registry<SamplerObject<B>>>,
    /// Exclusive shader objects constructed by this device.
    pub(crate) shaders: RefCell<Registry<ShaderObject<B>>>,
    /// Exclusive pipeline objects constructed by this device.
    pub(crate) pipelines: RefCell<Registry<PipelineObject<B>>>,
    /// Restricted factories are authenticated without querying a client object.
    pub(crate) factories: RefCell<HashMap<usize, FactoryEntry<B>>>,
    /// Sealed GPU blocks use construction-proven weak provenance too.
    pub(crate) blocks: RefCell<Registry<crate::gpu::record::BlockObject<B>>>,
    /// A non-borrowing gate spanning foreign reads or native preparation.
    pub(crate) busy: Cell<bool>,
}

/// Weak factory metadata prevents allocation-address reuse while its registry entry exists.
pub(crate) struct FactoryEntry<B: crate::gpu::native::NativeResources> {
    /// Keeps metadata, not the shareable factory value, alive.
    observer: WeakComPtr<IRenderResourcesAbi>,
    /// Controllers/resources may outlive a factory; this link is deliberately weak.
    domain: Weak<Domain<B>>,
}

/// One independently owned resource group, with a fixed drawing target and operation permissions.
/// The exclusive controller closes admission; restricted factories cannot widen its permissions.
/// Immutable resource versions retain its account, not this device owner, through native completion.
pub(crate) struct Domain<B: crate::gpu::native::NativeResources> {
    /// Authenticated target selected at construction, never copied from caller-reported metadata.
    /// Shared target ownership permits several groups without sharing their resources or quotas.
    pub(crate) target: Rc<crate::gpu::target::SurfaceTarget<B>>,
    /// Immutable operation permissions selected by application setup.
    pub(crate) permissions: RenderClientPermissions,
    /// Logical device used for provenance and guarded creation.
    pub(crate) device: GpuDevice<B>,
    /// Closing gates new admission without freeing retained snapshots.
    pub(crate) closed: Cell<bool>,
    /// Device-free allocation counters.
    pub(crate) account: Rc<Account>,
    /// Weak pending sessions allow immediate close without retaining payloads.
    pub(crate) sessions: RefCell<Vec<Weak<crate::gpu::record::SessionCore<B>>>>,
}

impl<B: crate::gpu::native::NativeResources> Domain<B> {
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
pub struct Operation<B: crate::gpu::native::NativeResources> {
    /// Logical device stays alive until the Busy flag is restored.
    inner: Rc<Device<B>>,
}
impl<B: crate::gpu::native::NativeResources> Drop for Operation<B> {
    /// Restore ordinary admission on every success/error/unwind path.
    fn drop(&mut self) {
        self.inner.busy.set(false);
    }
}

impl<B: crate::gpu::native::NativeResources> GpuDevice<B> {
    /// Owns a compiler fixed to this backend; callers cannot choose output per request.
    #[cfg(any(test, feature = "shader-compiler"))]
    pub fn new(core: Rc<B::Core>) -> Result<Self, NcStatus> {
        use crate::render::compiler::CompilerObject;
        let compiler = CompilerObject::from_value(CompilerObject::new(B::shader_target(&core))?).query()?;
        Ok(Self::from_compiler(core, compiler))
    }

    /// Acceptance-only injection exercises malformed foreign reports through real ABI calls.
    /// Production construction always uses the renderer's own fixed-format compiler.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_compiler(core: Rc<B::Core>, compiler: ComPtr<neocogi_platform::render::abi::ShaderCompilerAbi>) -> Self {
        Self::from_compiler(core, compiler)
    }

    /// Retains one compiler for the device lifetime; no registry lookup occurs per shader.
    #[cfg(any(test, feature = "shader-compiler"))]
    fn from_compiler(core: Rc<B::Core>, compiler: ComPtr<neocogi_platform::render::abi::ShaderCompilerAbi>) -> Self {
        Self {
            inner: Rc::new(Device {
                compiler,
                core,
                buffers: RefCell::new(HashMap::new()),
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
        B::identity(self.inner.core.as_ref()).clone()
    }

    /// Creates target metadata on this actual native device; no framebuffer is allocated here.
    /// Trusted platform assembly publishes/authenticates the corresponding COM surface separately.
    /// Unknown kinds reject with `NC_E_INVALID_ARG`; native allocation belongs to frame execution.
    pub fn surface_target(&self, kind: RenderSurfaceKind) -> Result<Rc<crate::gpu::target::SurfaceTarget<B>>, NcStatus> {
        crate::gpu::target::SurfaceTarget::new(self.inner.core.clone(), kind)
    }

    /// Creates one independently limited resource group with an explicit target and permissions.
    /// Group ownership, close/reentry rules and version retirement are independent of drawing role.
    /// Foreign-device targets return `RENDER_E_OWNER`; inconsistent attachment permissions return
    /// `NC_E_INVALID_ARG`. Limits and target permissions validate before controller/factory publication.
    pub fn create_client(
        &self,
        limits: RenderClientLimits,
        target: Rc<crate::gpu::target::SurfaceTarget<B>>,
        permissions: RenderClientPermissions,
    ) -> Result<ComBox<IRenderClientDomainAbi>, NcStatus> {
        let _operation = self.operation()?;
        if !Rc::ptr_eq(&target.core, &self.inner.core) {
            return Err(RENDER_E_OWNER);
        }
        target.validate(permissions)?;
        let domain = Rc::new(Domain {
            target,
            permissions,
            device: self.clone(),
            closed: Cell::new(false),
            account: Account::new(limits)?,
            sessions: RefCell::new(Vec::new()),
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
    pub fn operation(&self) -> Result<Operation<B>, NcStatus> {
        if self.inner.busy.replace(true) {
            return Err(RENDER_E_BUSY);
        }
        Ok(Operation { inner: self.inner.clone() })
    }

    /// Authenticates the factory's registered identity and checks its current domain gate.
    pub(crate) fn domain(&self, factory: ComRef<'_, IRenderResourcesAbi>) -> Result<Rc<Domain<B>>, NcStatus> {
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

    /// Imports an authenticated native image only when this resource group permits image import.
    /// The actual allocation, not caller-reported metadata, determines compatible core and extent.
    /// Missing permission returns `RENDER_E_PERMISSION`; incompatible image cores return `RENDER_E_OWNER`.
    /// The imported version retains the native image and its own quota receipt, never its public COM owner.
    pub fn import_texture(&self, factory: ComRef<'_, IRenderResourcesAbi>, image: Rc<B::Image>) -> Result<ComBox<IGpuBufferAbi>, NcStatus> {
        let _operation = self.operation()?;
        let domain = self.domain(factory)?;
        if !domain.permissions.import_images {
            return Err(RENDER_E_PERMISSION);
        }
        if !Rc::ptr_eq(&self.inner.core, B::image_core(image.as_ref())) {
            return Err(RENDER_E_OWNER);
        }
        let desc = GpuTextureInfo {
            size: B::image_size(image.as_ref()),
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        };
        let len = texture_desc(desc)?;
        let owner = domain.account.resource()?;
        let current = Rc::new(TextureVersion::<B> {
            core: self.inner.core.clone(),
            native: NativeSlot::default(),
            desc,
            contents: TextureContents::Imported { image },
            charge: Rc::new(domain.account.bytes(len)?),
            dirty: Cell::new(false),
            usage: GpuBufferUsage::Static,
        });
        BufferObject::imported(domain, current, owner)
    }

    /// Resolves a texture for the trusted host's eager preparation path.
    /// Checks import permission and exact resource/group ownership; it also accepts a locally uploaded
    /// texture in that permitted group so a compositor can prepare its atlas before publication.
    pub fn imported_texture_snapshot(
        &self,
        factory: ComRef<'_, IRenderResourcesAbi>,
        texture: ComRef<'_, IGpuBufferAbi>,
    ) -> Result<Rc<TextureVersion<B>>, NcStatus> {
        let _operation = self.operation()?;
        if !self.domain(factory)?.permissions.import_images {
            return Err(RENDER_E_PERMISSION);
        }
        self.texture_snapshot(factory, texture)
    }
}

/// Concrete typed capture methods share one provenance/lifetime rule with all callers.
macro_rules! snapshot_method {
    ($method:ident, $interface:ident, $field:ident, $version:ident) => {
        impl<B: NativeResources> GpuDevice<B> {
            #[doc = concat!("Captures one live same-domain ", stringify!($version), " under its provider reader gate; no public owner is promoted.")]
            pub fn $method(&self, factory: ComRef<'_, IRenderResourcesAbi>, resource: ComRef<'_, $interface>) -> Result<Rc<$version<B>>, NcStatus> {
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
impl<B: NativeResources> GpuDevice<B> {
    /// Captures the selected completed write of an authenticated linear resource.
    pub fn buffer_snapshot(&self, factory: ComRef<'_, IRenderResourcesAbi>, resource: ComRef<'_, IGpuBufferAbi>) -> Result<Rc<BufferVersion<B>>, NcStatus> {
        let domain = self.domain(factory)?;
        observe(&self.inner.buffers, resource.as_raw().object as usize)?.with(|buffer| {
            same_domain(&domain, &buffer.domain)?;
            buffer.linear().cloned()
        })?
    }
    /// Texture-role buffers share the same registration and provenance boundary.
    pub fn texture_snapshot(&self, factory: ComRef<'_, IRenderResourcesAbi>, resource: ComRef<'_, IGpuBufferAbi>) -> Result<Rc<TextureVersion<B>>, NcStatus> {
        let domain = self.domain(factory)?;
        observe(&self.inner.buffers, resource.as_raw().object as usize)?.with(|buffer| {
            same_domain(&domain, &buffer.domain)?;
            buffer.texture().cloned()
        })?
    }
}
snapshot_method!(sampler_snapshot, IGpuSamplerAbi, samplers, SamplerVersion);
snapshot_method!(shader_snapshot, IGpuShaderAbi, shaders, ShaderVersion);
snapshot_method!(pipeline_snapshot, IGpuPipelineAbi, pipelines, PipelineVersion);

/// Initialized CPU contents, immutable for the lifetime of every captured slot.
pub enum VersionBytes {
    /// Static initial data; this allocation is never writable after publication.
    Owned(Vec<u8>),
    /// Fixed initialized authoring/upload capacity reused after all captures end.
    Upload(buffer::UploadBytes),
}
impl VersionBytes {
    /// Admitted logical size, excluding allocation padding.
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }
    /// Whether the logical extent is empty.
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
    /// All buffer slots have initialized bytes; native imports have separate contents.
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::Upload(bytes) => bytes.bytes(),
        }
    }
    /// Copies the complete defined contents into an already admitted output allocation.
    pub fn copy_into(&self, output: &mut [u8]) -> Result<(), NcStatus> {
        if output.len() != self.len() {
            return Err(NC_E_INVALID_ARG);
        }
        output.copy_from_slice(self.as_slice());
        Ok(())
    }
    /// Reads a bounded byte interval without allocating another payload.
    pub fn read(&self, offset: usize, output: &mut [u8]) -> Result<(), NcStatus> {
        let end = offset.checked_add(output.len()).filter(|end| *end <= self.len()).ok_or(NC_E_OUT_OF_RANGE)?;
        output.copy_from_slice(&self.as_slice()[offset..end]);
        Ok(())
    }
}

/// Shared device-free reservation retained by actual native use and retirement.
#[derive(Clone)]
pub struct BackingCharge {
    /// Cloning this token shares one original byte admission.
    _receipt: Rc<ByteCharge>,
}
impl BackingCharge {
    /// Only admitted resource storage can create a native receipt.
    fn bytes(receipt: Rc<ByteCharge>) -> Self {
        Self { _receipt: receipt }
    }
}

/// Immutable buffer version owns its native realization, with no native-to-version back-reference.
pub struct BufferVersion<B: NativeResources> {
    /// Fixes compatible native device/context before first realization.
    pub(crate) core: Rc<B::Core>,
    /// Immutable role and logical extent.
    pub(crate) desc: GpuBufferDesc,
    /// Native allocation policy is fixed independently of the role.
    pub(crate) usage: GpuBufferUsage,
    /// A completed write refreshes existing native storage before its next use.
    dirty: Cell<bool>,
    /// Initialized fixed contents, immutable while any draw or native owner retains this slot.
    pub(crate) bytes: VersionBytes,
    /// Native memoization is private and may only publish backend-validated candidates.
    native: NativeSlot<B::Buffer>,
    /// Shared opaque bytes remain charged through native use and queued disposal.
    _charge: Rc<ByteCharge>,
}
/// Immutable sampled version owns either initialized pixels or a directly shared native image.
pub struct TextureVersion<B: NativeResources> {
    /// Driver compatibility/lifetime independent of logical domain admission.
    pub(crate) core: Rc<B::Core>,
    /// Complete admitted logical sampled-image shape.
    pub(crate) desc: GpuTextureInfo,
    /// Native image update policy matches the public texture-role buffer.
    pub(crate) usage: GpuBufferUsage,
    /// Complete authored pixels changed after the previous concrete upload.
    dirty: Cell<bool>,
    /// Import keeps no public image COM capability, numeric key, or renderer facade.
    contents: TextureContents<B>,
    /// First imported use also crosses backend initialization/accounting admission.
    native: NativeSlot<B::Image>,
    /// Per-import reservation stays with that import/use, never globally on a shared image.
    charge: Rc<ByteCharge>,
}
/// Authoritative slot contents; captures freeze pixel storage and imports are always read-only.
enum TextureContents<B: NativeResources> {
    /// Complete initialized pixels; writes require exclusive idle-slot ownership.
    Pixels(VersionBytes),
    /// Construction-proven same-core image; no public image lease or facade back-edge.
    Imported {
        /// Actual independently owned native pixels.
        image: Rc<B::Image>,
    },
}
impl<B: NativeResources> TextureVersion<B> {
    /// Imported images have no CPU contents and reject uploads and CPU reads.
    pub fn pixels(&self) -> Option<&VersionBytes> {
        match &self.contents {
            TextureContents::Pixels(bytes) => Some(bytes),
            TextureContents::Imported { .. } => None,
        }
    }
    /// Backend-only realization input after trusted ordinary-image authentication.
    pub fn imported_image(&self) -> Option<&Rc<B::Image>> {
        match &self.contents {
            TextureContents::Imported { image } => Some(image),
            TextureContents::Pixels(_) => None,
        }
    }
    /// No passive source expiry or COM owner is retained in native work.
    pub fn receipt(&self) -> BackingCharge {
        BackingCharge::bytes(self.charge.clone())
    }
}
/// Immutable sampler version owns its one device-local realization.
pub struct SamplerVersion<B: NativeResources> {
    /// Native instance identity/lifetime, not a backend-name test.
    pub(crate) core: Rc<B::Core>,
    /// Complete immutable filter/address state.
    pub(crate) desc: GpuSamplerDesc,
    /// Shared native state is populated only after successful backend construction.
    native: NativeSlot<B::Sampler>,
    /// Descriptor reservation follows actual logical/native ownership.
    _charge: Rc<ByteCharge>,
}
/// Driver-bound validated shader artifacts; pipelines retain these exact immutable dependencies.
pub struct ShaderVersion<B: NativeResources> {
    /// Fixes the resource's creating native instance even before pipeline realization.
    pub(crate) core: Rc<B::Core>,
    /// Validated code and reflection copied from the renderer-owned compiler.
    pub(crate) artifact: Shader,
    /// Native pipelines retain only the opaque receipt, not this source version.
    _charge: Rc<ByteCharge>,
}
/// Successful native variants are owned by one immutable pipeline definition.
type PipelineVariants<B> = Vec<(<B as NativeResources>::PipelineKey, Rc<<B as NativeResources>::Pipeline>)>;

/// Complete immutable pipeline definition with resource-local target variants.
pub struct PipelineVersion<B: NativeResources> {
    /// All shader/resources and target variants belong to this compatible native core.
    pub(crate) core: Rc<B::Core>,
    /// Admitted triangle, blend, depth and raster state.
    pub(crate) info: GpuPipelineInfo,
    /// Explicit portable stream stepping.
    pub(crate) vertex_buffers: Vec<GpuVertexBufferLayout>,
    /// Checked locations, formats and offsets.
    pub(crate) attributes: Vec<GpuVertexAttribute>,
    /// Exact reflected stage-local resource declarations.
    pub(crate) bindings: Vec<GpuShaderBinding>,
    /// Immutable validated vertex dependency.
    pub(crate) vertex: Rc<ShaderVersion<B>>,
    /// Immutable validated fragment dependency.
    pub(crate) fragment: Rc<ShaderVersion<B>>,
    /// Required shared memoization; no borrow spans native pipeline construction.
    pub(crate) variants: RefCell<PipelineVariants<B>>,
    /// Native variant receipts share this and both shader reservations exactly once.
    pub(crate) _charge: Rc<ByteCharge>,
}

/// One realization rule for each non-variant native resource family.
macro_rules! native_resource {
    ($version:ident,$native:ident,$create:ident) => {
        impl<B: NativeResources> $version<B> {
            /// Reuses or creates this exact version on the checked live preparation core.
            /// Only backend-owned constructors can populate the private native slot.
            pub fn native(&self, preparation: &mut B::Preparation<'_>) -> Result<Rc<B::$native>, String> {
                check_core::<B>(&self.core, preparation)?;
                self.native.realize(|| B::$create(self, preparation))
            }
        }
    };
}
impl<B: NativeResources> BufferVersion<B> {
    /// Refreshes idle fixed storage before cloning a prepared native owner.
    pub fn native(&self, preparation: &mut B::Preparation<'_>) -> Result<Rc<B::Buffer>, String> {
        check_core::<B>(&self.core, preparation)?;
        if self.dirty.get() {
            self.native.refresh(|native| B::update_buffer(self, native, preparation))?;
        }
        let native = self.native.realize(|| B::buffer(self, preparation))?;
        self.dirty.set(false);
        Ok(native)
    }
}
impl<B: NativeResources> TextureVersion<B> {
    /// Image handles are stable; only an exclusively reused slot can refresh pixels.
    pub fn native(&self, preparation: &mut B::Preparation<'_>) -> Result<Rc<B::Image>, String> {
        check_core::<B>(&self.core, preparation)?;
        if self.dirty.get() {
            self.native.refresh(|native| B::update_image(self, native, preparation))?;
        }
        let native = self.native.realize(|| B::image(self, preparation))?;
        self.dirty.set(false);
        Ok(native)
    }
}
native_resource!(SamplerVersion, Sampler, sampler);
impl<B: NativeResources> BufferVersion<B> {
    /// Device-free shared reservation for actual native buffer use and disposal.
    pub fn receipt(&self) -> BackingCharge {
        BackingCharge::bytes(self._charge.clone())
    }
}
impl<B: NativeResources> SamplerVersion<B> {
    /// Native sampling state keeps no resource-version back-reference.
    pub fn receipt(&self) -> BackingCharge {
        BackingCharge::bytes(self._charge.clone())
    }
}
impl<B: NativeResources> ShaderVersion<B> {
    /// Receipt only; native modules/programs must not retain a source version.
    pub fn receipt(&self) -> BackingCharge {
        BackingCharge::bytes(self._charge.clone())
    }
}
impl<B: NativeResources> PipelineVersion<B> {
    /// Keeps the admitted definition and both exact shader dependencies charged during native use.
    pub fn receipts(&self) -> [BackingCharge; 3] {
        [BackingCharge::bytes(self._charge.clone()), self.vertex.receipt(), self.fragment.receipt()]
    }
    /// Target-dependent variants belong to this source, never a global resource-id map.
    pub fn native(&self, key: B::PipelineKey, preparation: &mut B::Preparation<'_>) -> Result<Rc<B::Pipeline>, String> {
        check_core::<B>(&self.core, preparation)?;
        if let Some((_, value)) = self
            .variants
            .try_borrow()
            .map_err(|_| "pipeline variants are busy")?
            .iter()
            .find(|(existing, _)| *existing == key)
        {
            return Ok(value.clone());
        }
        self.variants
            .try_borrow_mut()
            .map_err(|_| "pipeline variants are busy")?
            .try_reserve(1)
            .map_err(|error| error.to_string())?;
        let value = B::pipeline(self, &key, preparation)?;
        self.variants
            .try_borrow_mut()
            .map_err(|_| "pipeline variants are busy")?
            .push((key, value.clone()));
        Ok(value)
    }
}

/// Produces concrete exclusive resource classes without type-erased storage.
macro_rules! resource_object {
    ($name:ident, $interface:ident, $version:ident) => {
        #[com_object(interfaces($interface), ownership = exclusive)]
        /// Exclusive public owner; registry observers retain only lifetime metadata.
        pub(crate) struct $name<B: NativeResources> {
            /// Domain/device authority exists only while this public value exists.
            pub(crate) domain: Rc<Domain<B>>,
            /// Ordinary mutable current pointer, accessed through checked local gates.
            pub(crate) current: Rc<$version<B>>,
            /// Released at public value destruction, independently of version bytes.
            _owner: ResourceCharge,
        }
        impl<B: NativeResources> $interface for $name<B> {}
        impl<B: NativeResources> IRenderDriverBound for $name<B> {
            fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
                Ok(B::identity(self.current.core.as_ref()).clone())
            }
        }
    };
}
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
pub(crate) fn same_domain<B: crate::gpu::native::NativeResources>(expected: &Rc<Domain<B>>, actual: &Rc<Domain<B>>) -> Result<(), NcStatus> {
    expected.open()?;
    if Rc::ptr_eq(expected, actual) { Ok(()) } else { Err(RENDER_E_OWNER) }
}

#[com_object(interfaces(IRenderClientDomain), ownership = exclusive)]
/// Host controller and resource factory deliberately have different QI identities.
struct DomainOwner<B: crate::gpu::native::NativeResources> {
    /// One authoritative close/account record.
    domain: Rc<Domain<B>>,
    /// Shareable restricted creation service delegated explicitly to clients.
    factory: ComPtr<IRenderResourcesAbi>,
}
impl<B: crate::gpu::native::NativeResources> IRenderClientDomain for DomainOwner<B> {
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
impl<B: crate::gpu::native::NativeResources> Drop for DomainOwner<B> {
    /// Controller removal revokes admission but does not free retained backing versions.
    fn drop(&mut self) {
        self.domain.close();
    }
}

#[com_object(interfaces(IRenderResources))]
/// Shareable factory; exclusive resources carry separate identities and ownership.
struct Factory<B: crate::gpu::native::NativeResources> {
    /// Holds device authority while clients deliberately retain this service.
    domain: Rc<Domain<B>>,
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

/// Baseline sampled textures have one tightly packed RGBA8 mip, no target usage.
fn texture_desc(desc: GpuTextureInfo) -> Result<usize, NcStatus> {
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

impl<B: crate::gpu::native::NativeResources> IRenderDriverBound for Factory<B> {
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        Ok(self.domain.device.driver_identity())
    }
}
impl<B: crate::gpu::native::NativeResources> IRenderResources for Factory<B> {
    /// Complete immutable input is validated and copied before owner publication.
    fn create_static_buffer(&self, desc: GpuBufferDesc, source: ComRef<'_, IGpuDataSourceAbi>) -> Result<ComBox<IGpuBufferAbi>, NcStatus> {
        let _operation = self.domain.device.operation()?;
        BufferObject::create(&self.domain, desc, GpuBufferUsage::Static, Some(source))
    }
    /// Dynamic allocation reserves fixed initialized upload capacity once.
    fn create_dynamic_buffer(&self, desc: GpuBufferDesc) -> Result<ComBox<IGpuBufferAbi>, NcStatus> {
        let _operation = self.domain.device.operation()?;
        BufferObject::create(&self.domain, desc, GpuBufferUsage::Dynamic, None)
    }
    /// Streaming reserves every slot before publication; map exhaustion remains Pending.
    fn create_streamed_buffer(&self, desc: GpuBufferDesc) -> Result<ComBox<IGpuBufferAbi>, NcStatus> {
        let _operation = self.domain.device.operation()?;
        BufferObject::create(&self.domain, desc, GpuBufferUsage::Streamed, None)
    }

    /// Filtering/addressing state contains no resource lookup or native callback.
    fn create_sampler(&self, desc: GpuSamplerDesc) -> Result<ComBox<IGpuSamplerAbi>, NcStatus> {
        let _operation = self.domain.device.operation()?;
        self.domain.open()?;
        sampler_desc(desc)?;
        let owner = self.domain.account.resource()?;
        let current = Rc::new(SamplerVersion {
            core: self.domain.device.inner.core.clone(),
            native: NativeSlot::default(),
            desc,
            _charge: Rc::new(self.domain.account.bytes(std::mem::size_of::<GpuSamplerDesc>())?),
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
    fn create_shader_result(
        &self,
        stage: GpuShaderStage,
        source: ComRef<'_, IGpuDataSourceAbi>,
        entry: NcCopyFromBufferOffset<'_, u8>,
    ) -> Result<ComBox<IShaderCreationResultAbi>, NcStatus> {
        let result = (|| -> Result<_, RenderError> {
            let _operation = self.domain.device.operation()?;
            self.domain.open()?;
            // The busy gate and provider receipt enclose every callback and final Release.
            // Input allocations die before their receipt; native publication follows the
            // final foreign destructor and a fresh domain-open check.
            let imported = (|| -> Result<_, RenderError> {
                if entry.offset() != 0 {
                    return Err(NC_E_INVALID_ARG.into());
                }
                let entry = entry.as_slice();
                if entry.is_empty()
                    || entry.len() > 128
                    || std::str::from_utf8(entry).is_err()
                    || !matches!(stage, GpuShaderStage::Vertex | GpuShaderStage::Fragment)
                {
                    return Err(NC_E_INVALID_ARG.into());
                }
                let mut expected = [0u8; 128];
                expected[..entry.len()].copy_from_slice(entry);
                let len = usize::try_from(source.byte_count()?).map_err(|_| NC_E_OUT_OF_RANGE)?;
                self.domain.open()?;
                if len == 0 || len > crate::gpu::shader::MAX_SHADER_BYTES {
                    return Err(NC_E_OUT_OF_RANGE.into());
                }
                let owner = self.domain.account.resource()?;
                let input_charge = self.domain.account.bytes(len)?;
                let mut bytes = zeroed(len)?;
                read_bytes(source, len, |offset, chunk| bytes[offset..offset + chunk.len()].copy_from_slice(chunk))?;
                self.domain.open()?;
                let cap = (self.domain.account.remaining_bytes().min(crate::gpu::shader::MAX_SHADER_ARTIFACT_BYTES as u64)) as usize;
                let mut provider_charge = self.domain.account.bytes(cap)?;
                let target = B::shader_target(&self.domain.device.inner.core);
                let result = self.domain.device.inner.compiler.compile_result(
                    neocogi_platform::render::abi::ShaderCompileRequest { stage, max_output_bytes: cap as u64 },
                    NcCopyFromBufferOffset::new(0, &bytes),
                    NcCopyFromBufferOffset::new(0, &expected[..entry.len()]),
                );
                drop(bytes);
                drop(input_charge);
                let report = result?;
                neocogi_platform::render::shader::check_compilation(&report)?;
                let prepared = neocogi_platform::render::shader::PreparedShader::new(report, stage, target, &expected[..entry.len()], cap)?;
                provider_charge.shrink(prepared.byte_len())?;
                let charge = self.domain.account.bytes(prepared.byte_len())?;
                let artifact = prepared.copy()?;
                drop(prepared);
                drop(provider_charge);
                self.domain.open()?;
                Ok((owner, charge, artifact))
            })();
            let (owner, charge, artifact) = imported?;
            let current = Rc::new(ShaderVersion {
                core: self.domain.device.inner.core.clone(),
                artifact,
                _charge: Rc::new(charge),
            });
            publish(
                &self.domain.device.inner.shaders,
                ShaderObject::into_local_box(ShaderObject {
                    domain: self.domain.clone(),
                    current,
                    _owner: owner,
                }),
            )
            .map_err(RenderError::from)
        })();
        Ok(shader_creation_result(result.map_err(|error| error.context("create shader"))))
    }

    /// Copies finite descriptor arrays and captures live same-domain shaders under local gates.
    fn create_pipeline(
        &self,
        source: ComRef<'_, IGpuPipelineSourceAbi>,
        vertex: ComRef<'_, IGpuShaderAbi>,
        fragment: ComRef<'_, IGpuShaderAbi>,
    ) -> Result<ComBox<IGpuPipelineAbi>, NcStatus> {
        crate::gpu::pipeline::create(&self.domain, source, vertex, fragment)
    }
}

/// Constructs one fully validated immutable pipeline after all foreign descriptor reads.
pub(crate) fn publish_pipeline<B: crate::gpu::native::NativeResources>(
    domain: &Rc<Domain<B>>,
    version: PipelineVersion<B>,
    owner: ResourceCharge,
) -> Result<ComBox<IGpuPipelineAbi>, NcStatus> {
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

#[cfg(all(test, feature = "shader-spirv"))]
mod compiler_tests;

#[cfg(test)]
mod permissions_tests;

impl<B: crate::gpu::native::NativeResources> BufferVersion<B> {
    /// Fixed creation/update policy used by native allocation hints.
    pub fn usage(&self) -> GpuBufferUsage {
        self.usage
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

impl<B: crate::gpu::native::NativeResources> TextureVersion<B> {
    /// Fixed image upload policy determines retained reusable staging capacity.
    pub fn usage(&self) -> GpuBufferUsage {
        self.usage
    }
    /// Reads the admitted desc without granting mutation authority.
    pub fn desc(&self) -> GpuTextureInfo {
        self.desc
    }
}

impl<B: crate::gpu::native::NativeResources> SamplerVersion<B> {
    /// Reads the admitted desc without granting mutation authority.
    pub fn desc(&self) -> GpuSamplerDesc {
        self.desc
    }
}

impl<B: crate::gpu::native::NativeResources> ShaderVersion<B> {
    /// Reads the admitted artifact without granting mutation authority.
    pub fn artifact(&self) -> &Shader {
        &self.artifact
    }
}

impl<B: crate::gpu::native::NativeResources> PipelineVersion<B> {
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
    pub fn vertex(&self) -> &Rc<ShaderVersion<B>> {
        &self.vertex
    }
    /// Reads the admitted fragment without granting mutation authority.
    pub fn fragment(&self) -> &Rc<ShaderVersion<B>> {
        &self.fragment
    }
}
