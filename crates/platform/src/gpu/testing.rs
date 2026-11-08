//! Explicit opt-in CPU probe family for common ownership/code-generation tests, never a renderer fallback.
use crate::gpu::{
    GpuDevice,
    native::NativeResources,
    resource::{BackingCharge, BufferVersion, TextureVersion, SamplerVersion, PipelineVersion},
    abi::*,
};
use neocogi_platform::render::source::{DriverIdentity, new_driver_identity};
use std::{cell::Cell, rc::Rc};

/// Counter/fault state only: no resource owners, submissions, channel endpoints or UI authority.
pub struct Core {
    /// Explicit immutable format for compiler transfer regressions.
    target: crate::render::shader::ShaderTarget,
    /// One retained identity for the virtual compatible device.
    identity: DriverIdentity,
    /// Number of successfully constructed virtual native allocations.
    created: Cell<usize>,
    /// Number of last-owner native destruction observations.
    destroyed: Cell<usize>,
    /// One recoverable constructor failure leaves the immutable source retryable.
    fail_next: Cell<bool>,
    /// Authoritative terminal device-loss observation for Ready reuse tests.
    available: Cell<bool>,
}

/// Copies a fixture range through the production synchronous buffer ABI.
pub fn write_buffer(mut buffer: neocogi_com::ComRef<'_, IGpuBufferAbi>, offset: u64, bytes: &[u8]) -> Result<(), neocogi_com::NcStatus> {
    buffer.upload(neocogi_com::NcCopyFromBufferOffset::new(offset, bytes))
}

/// Fixture-only rectangle composition above the flat row-major byte transfer API.
/// A single upload publishes the completed image, including preserving dynamic pixels.
pub fn write_pixels(mut buffer: neocogi_com::ComRef<'_, IGpuBufferAbi>, rect: Rect, bytes: &[u8]) -> Result<(), neocogi_com::NcStatus> {
    let info = buffer.info()?;
    let GpuBufferDesc::Texture { size, .. } = info.desc else {
        return Err(neocogi_com::NC_E_INVALID_ARG);
    };
    if rect.x < 0
        || rect.y < 0
        || rect.width <= 0
        || rect.height <= 0
        || i64::from(rect.x) + i64::from(rect.width) > i64::from(size.width)
        || i64::from(rect.y) + i64::from(rect.height) > i64::from(size.height)
    {
        return Err(neocogi_com::NC_E_OUT_OF_RANGE);
    }
    let row = rect.width as usize * 4;
    if row * rect.height as usize != bytes.len() {
        return Err(neocogi_com::NC_E_INVALID_ARG);
    }
    let mut pixels = neocogi_com::NcOutputBuffer::new();
    let mut image = vec![0; info.desc.byte_len()?];
    if info.usage == GpuBufferUsage::Dynamic {
        buffer.read(pixels.output(0, image.len()))?;
        image.copy_from_slice(pixels.as_slice());
    }
    let pitch = size.width as usize * 4;
    for y in 0..rect.height as usize {
        let first = (rect.y as usize + y) * pitch + rect.x as usize * 4;
        image[first..first + row].copy_from_slice(&bytes[y * row..(y + 1) * row]);
    }
    buffer.upload(neocogi_com::NcCopyFromBufferOffset::new(0, &image))
}

/// Keeps the public uniform owner explicit until the consuming fixture draw captures it.
pub fn bind_uniform(
    resources: neocogi_com::ComRef<'_, IRenderResourcesAbi>,
    view: &mut IRenderViewClient,
    stage: GpuShaderStage,
    slot: u32,
    bytes: &[u8],
) -> Result<neocogi_com::ComBox<IGpuBufferAbi>, neocogi_com::NcStatus> {
    let source = neocogi_platform::render::source::Bytes::source(bytes.to_vec());
    let buffer = resources.create_static_buffer(GpuBufferDesc::Uniform { byte_len: bytes.len() as u64 }, source.as_com_ref())?;
    view.bind_uniform_buffer(stage, slot, buffer.as_com_ref(), 0, bytes.len() as u64)?;
    Ok(buffer)
}
impl Core {
    /// Creates a distinct explicitly selected virtual driver with no native resources.
    pub fn new() -> Rc<Self> {
        Self::with_target(crate::render::shader::ShaderTarget::Wgsl)
    }
    /// Creates a virtual core for exactly one requested shader artifact family.
    pub fn with_target(target: crate::render::shader::ShaderTarget) -> Rc<Self> {
        Rc::new(Self {
            target,
            identity: new_driver_identity(),
            created: Cell::new(0),
            destroyed: Cell::new(0),
            fail_next: Cell::new(false),
            available: Cell::new(true),
        })
    }
    /// Successful native constructor count across this virtual device.
    pub fn created(&self) -> usize {
        self.created.get()
    }
    /// Actual last-owner destruction count, independent of public COM release.
    pub fn destroyed(&self) -> usize {
        self.destroyed.get()
    }
    /// Rejects the next new candidate without publishing a native slot.
    pub fn fail_next(&self) {
        self.fail_next.set(true);
    }
    /// Permanently rejects later preparation, including a previously Ready slot.
    pub fn lose(&self) {
        self.available.set(false);
    }
    /// Constructor admission is separate from source validation and slot publication.
    fn allocate(&self) -> Result<(), String> {
        if self.fail_next.replace(false) {
            return Err("injected native construction failure".into());
        }
        self.created.set(self.created.get() + 1);
        Ok(())
    }
}
/// Virtual backend type remains binary-local and is never transported through COM.
pub struct Backend;
/// Explicit preparation capability for this virtual driver; no hidden fallback selection.
pub struct Preparation<'a> {
    /// Narrow native identity/lifetime selected for this transaction.
    core: &'a Rc<Core>,
}
impl<'a> Preparation<'a> {
    /// Establishes the concrete virtual preparation instance for an ownership test.
    pub fn new(core: &'a Rc<Core>) -> Self {
        Self { core }
    }
}
/// Virtual native storage used for buffer/sampler/uniform ownership, without source back-references.
pub struct Storage {
    /// Native device lifetime stays independent of its logical source.
    core: Rc<Core>,
    /// Opaque reservation follows the actual virtual allocation.
    _receipt: BackingCharge,
}
impl Drop for Storage {
    fn drop(&mut self) {
        self.core.destroyed.set(self.core.destroyed.get() + 1);
    }
}
/// Immutable native image stand-in; dimensions and core come from this actual allocation.
pub struct Image {
    /// Native lifetime only, never a logical factory or a public image COM owner.
    core: Rc<Core>,
    /// Admitted immutable logical image dimensions.
    size: Size,
    /// Optional test observation of the original ordinary-image capability identity.
    origin: Option<usize>,
    /// Private imports have their own separate use receipts rather than changing this field.
    _receipt: Option<BackingCharge>,
}
impl Image {
    /// Explicit trusted fixture ordinary image; this does not authenticate an application capability.
    pub fn ordinary(core: Rc<Core>, size: Size, origin: usize) -> Rc<Self> {
        core.allocate().expect("fixture ordinary image creation is not fault-injected");
        Rc::new(Self {
            core,
            size,
            origin: Some(origin),
            _receipt: None,
        })
    }
    /// Headless observations never re-query a captured public COM capability.
    pub fn origin(&self) -> Option<usize> {
        self.origin
    }
}
impl Drop for Image {
    fn drop(&mut self) {
        self.core.destroyed.set(self.core.destroyed.get() + 1);
    }
}
/// Virtual target variant retains only actual native lifetime and opaque dependency receipts.
pub struct Pipeline {
    /// No path leads back to a logical pipeline version or driver facade.
    core: Rc<Core>,
    /// Definition and both shader reservations share one final refund per source.
    _receipts: [BackingCharge; 3],
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        self.core.destroyed.set(self.core.destroyed.get() + 1);
    }
}
impl NativeResources for Backend {
    fn shader_target(core: &Core) -> crate::render::shader::ShaderTarget {
        core.target
    }
    type Core = Core;
    type Buffer = Storage;
    type Image = Image;
    type Sampler = Storage;
    type Pipeline = Pipeline;
    type PipelineKey = u8;
    type Preparation<'a> = Preparation<'a>;
    fn identity(core: &Core) -> &DriverIdentity {
        &core.identity
    }
    fn storage_available(core: &Core) -> Result<(), String> {
        if core.available.get() {
            Ok(())
        } else {
            Err("virtual graphics core is unavailable".into())
        }
    }
    fn core<'a>(preparation: &'a Preparation<'_>) -> &'a Rc<Core> {
        preparation.core
    }
    fn check(preparation: &mut Preparation<'_>) -> Result<(), String> {
        if preparation.core.available.get() {
            Ok(())
        } else {
            Err("virtual native device is lost".into())
        }
    }
    fn image_core(image: &Image) -> &Rc<Core> {
        &image.core
    }
    fn image_size(image: &Image) -> Size {
        image.size
    }
    fn buffer(source: &BufferVersion<Self>, preparation: &mut Preparation<'_>) -> Result<Rc<Storage>, String> {
        preparation.core.allocate()?;
        Ok(Rc::new(Storage {
            core: preparation.core.clone(),
            _receipt: source.receipt(),
        }))
    }
    fn update_buffer(_: &BufferVersion<Self>, _: &Rc<Storage>, preparation: &mut Preparation<'_>) -> Result<(), String> {
        Self::check(preparation)
    }
    fn update_image(_: &TextureVersion<Self>, _: &Rc<Image>, preparation: &mut Preparation<'_>) -> Result<(), String> {
        Self::check(preparation)
    }
    fn sampler(source: &SamplerVersion<Self>, preparation: &mut Preparation<'_>) -> Result<Rc<Storage>, String> {
        preparation.core.allocate()?;
        Ok(Rc::new(Storage {
            core: preparation.core.clone(),
            _receipt: source.receipt(),
        }))
    }
    fn image(source: &TextureVersion<Self>, preparation: &mut Preparation<'_>) -> Result<Rc<Image>, String> {
        if let Some(image) = source.imported_image() {
            return Ok(image.clone());
        }
        preparation.core.allocate()?;
        Ok(Rc::new(Image {
            core: preparation.core.clone(),
            size: source.desc().size,
            origin: None,
            _receipt: Some(source.receipt()),
        }))
    }
    fn pipeline(source: &PipelineVersion<Self>, _: &u8, preparation: &mut Preparation<'_>) -> Result<Rc<Pipeline>, String> {
        preparation.core.allocate()?;
        Ok(Rc::new(Pipeline {
            core: preparation.core.clone(),
            _receipts: source.receipts(),
        }))
    }
}
/// Explicit fixture constructor used by common CPU tests; production always supplies a real core.
pub fn device(compiler: neocogi_com::ComPtr<neocogi_platform::render::abi::ShaderCompilerAbi>) -> GpuDevice<Backend> {
    GpuDevice::with_compiler(Core::new(), compiler)
}

/// Canonical derivative inputs and recorders shared by native and browser acceptance.
pub mod derivatives;
/// Real COM shader/image fixtures shared by native and browser acceptance.
pub mod sampled;
/// Exact logical uniform spans exercised through native GL and browser WebGL.
pub mod uniforms;

/// Explicit target fixture; it never selects a compiler fallback or native graphics backend.
pub fn device_with_target(
    compiler: neocogi_com::ComPtr<crate::render::abi::ShaderCompilerAbi>,
    target: crate::render::shader::ShaderTarget,
) -> GpuDevice<Backend> {
    GpuDevice::with_compiler(Core::with_target(target), compiler)
}
