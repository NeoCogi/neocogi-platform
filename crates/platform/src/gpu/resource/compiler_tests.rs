//! Hostile-but-ABI-valid provider probes exercise the actual COM transfer rather
//! than the unrelated counted GPU input-source protocol.
use super::*;
/// Binary transfer probes use a SPIR-V backend family without adding a native GPU.
fn new_device(compiler: ComPtr<ShaderCompilerAbi>) -> GpuDevice {
    crate::gpu::testing::device_with_target(compiler, ShaderTarget::SpirV)
}
type GpuDevice = crate::gpu::GpuDevice<crate::gpu::testing::Backend>;
use neocogi_platform::render::abi::*;
use neocogi_com::{INcStringAbi, INcBytesAbi, NcBytes};
/// Valid source with a small deterministic artifact.
const SOURCE: &str = "@vertex fn vs()->@builtin(position) vec4<f32>{return vec4<f32>(0.,0.,0.,1.);}";

use neocogi_platform::render::shader::testing as admission;

/// A rejected module publishes no GPU resource and leaves quota and operation admission usable.
#[test]
fn inactive_profile_violations_reject_through_the_resource_factory() {
    let compiler = neocogi_platform::render::compiler::CompilerObject::from_value(
        neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::SpirV).unwrap(),
    )
    .query()
    .unwrap();
    let gpu = new_device(compiler);
    let controller = gpu
        .create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions::default(),
        )
        .unwrap();
    let factory = controller.resources().unwrap();
    for case in admission::cases() {
        let source = crate::gpu::source::Bytes::source(case.source.into_bytes());
        for (stage, entry) in [(GpuShaderStage::Vertex, "selected"), (case.stage, "outside")] {
            assert_eq!(
                factory
                    .create_shader(stage, source.as_com_ref(), NcCopyFromBufferOffset::new(0, entry.as_bytes()))
                    .map_err(|error| error.status)
                    .unwrap_err(),
                NC_E_INVALID_ARG,
                "{} / {entry}",
                case.name
            );
            assert_eq!(gpu.live_bytes(factory.as_com_ref()).unwrap(), 0);
        }
    }
    let source = crate::gpu::source::Bytes::source(admission::valid_mixed_module().into_bytes());
    for (stage, entry) in [(GpuShaderStage::Vertex, "selected"), (GpuShaderStage::Fragment, "outside")] {
        let shader = factory
            .create_shader(stage, source.as_com_ref(), NcCopyFromBufferOffset::new(0, entry.as_bytes()))
            .unwrap();
        assert!(gpu.live_bytes(factory.as_com_ref()).unwrap() > 0);
        drop(shader);
        assert_eq!(gpu.live_bytes(factory.as_com_ref()).unwrap(), 0);
    }
}
/// One observable intervention at a legitimate foreign-call boundary.
#[derive(Clone, Copy)]
enum Fault {
    /// Fully valid transfer.
    None,
    /// Error text getter fails independently of the original compilation status.
    Diagnostic,
    /// A valid string export exceeds the receiver diagnostic limit.
    LongDiagnostic,
    /// Missing selected code must not fall back to request source.
    MissingCode,
    /// Structural binary rejection, with initialized backing.
    Binary,
    /// Wrong selected stage in optional metadata.
    Stage,
    /// Independent target getter disagrees with the device's immutable format.
    Target,
    /// Optional code count disagrees with the selected export.
    CodeCount,
    /// Closure during a getter.
    CloseGetter,
    /// Closure during final report Release.
    CloseRelease,
    /// Closure during the last code-export Release, after report destruction.
    CloseExport,
    /// Oversized observed code length, with valid backing.
    HugeLength,
    /// Length changes after an initialized successful copy.
    ChangingLength,
    /// A returned copy error preserves rollback.
    CopyError,
    /// Optional summary disagrees with independently queried collection.
    Count,
    /// Foreign collection exceeds the fixed reflection bound.
    LocationCount,
    /// Unknown scalar family crosses valid raw storage.
    LocationEnum,
    /// Repeated location does not gain authority through aggregate counts.
    DuplicateLocation,
    /// Item belongs to the wrong entry direction.
    LocationDirection,
}
/// Weak target avoids retaining device/domain through its own compiler dependency.
struct Probe {
    /// Domain becomes known after the injected device constructs it.
    domain: RefCell<Weak<Domain<crate::gpu::testing::Backend>>>,
    /// Chosen fault, immutable for a test.
    fault: Fault,
    /// Real COM factory reached through a weak capability, avoiding a strong cycle.
    factory: RefCell<Option<neocogi_com::WeakComPtr<IRenderResourcesAbi>>>,
    /// Every provider/report boundary sees Busy.
    calls: Cell<u32>,
    /// Live charge at report destruction.
    release_bytes: Cell<u64>,
}
impl Probe {
    /// Exercises nested creation through the same gate without taking registry borrows.
    fn call(&self) {
        if let Some(domain) = self.domain.borrow().upgrade() {
            assert_eq!(domain.device.operation().err().unwrap(), RENDER_E_BUSY);
            if let Some(factory) = self.factory.borrow().as_ref().and_then(|weak| weak.upgrade().unwrap()) {
                let bytes = crate::gpu::source::Bytes::source(vec![0; 4]);
                assert_eq!(
                    factory.create_static_buffer(GpuBufferDesc::Vertex { byte_len: 4 }, bytes.as_com_ref()).err(),
                    Some(RENDER_E_BUSY)
                );
            }
            self.calls.set(self.calls.get() + 1);
        }
    }
}
/// Proxy compiler still invokes the real validator/emitter through COM.
#[com_object(interfaces(neocogi_platform::render::abi::ShaderCompiler))]
struct CompilerProxy {
    /// Real compiler fixed to the backend format.
    inner: ComPtr<ShaderCompilerAbi>,
    /// Shared observations only.
    probe: Rc<Probe>,
}
impl ShaderCompiler for CompilerProxy {
    fn compile_result(
        &self,
        request: ShaderCompileRequest,
        source: NcCopyFromBufferOffset<'_, u8>,
        entry: NcCopyFromBufferOffset<'_, u8>,
    ) -> Result<ComPtr<IShaderCompilationAbi>, NcStatus> {
        self.probe.call();
        let inner = self.inner.compile_result(request, source, entry)?;
        ReportProxy::from_value(ReportProxy { inner, probe: self.probe.clone() }).query()
    }
}
/// Complete report forwarding keeps each fault isolated to an observable valid ABI call.
#[com_object(interfaces(neocogi_platform::render::abi::IShaderCompilation))]
struct ReportProxy {
    /// Real immutable report.
    inner: ComPtr<IShaderCompilationAbi>,
    /// Weak reentry target and chosen intervention.
    probe: Rc<Probe>,
}
impl Drop for ReportProxy {
    fn drop(&mut self) {
        self.probe.call();
        if let Some(domain) = self.probe.domain.borrow().upgrade() {
            self.probe.release_bytes.set(domain.account.live_bytes());
            if matches!(self.probe.fault, Fault::CloseRelease) {
                domain.close();
            }
        }
    }
}
impl IShaderCompilation for ReportProxy {
    fn status(&self) -> Result<i32, NcStatus> {
        self.probe.call();
        self.inner.status()
    }
    fn diagnostic(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        match self.probe.fault {
            Fault::Diagnostic => Err(RENDER_E_BUSY),
            Fault::LongDiagnostic => Ok(neocogi_com::NcString::from_utf8_str(&"x".repeat(4097))),
            _ => self.inner.diagnostic(),
        }
    }
    fn info(&self) -> Result<ShaderInfo, NcStatus> {
        self.probe.call();
        let mut v = self.inner.info()?;
        if matches!(self.probe.fault, Fault::CodeCount) {
            v.code_byte_count += 4;
        }
        if matches!(self.probe.fault, Fault::Stage) {
            v.stage = GpuShaderStage::Fragment;
        }
        if matches!(self.probe.fault, Fault::Count) {
            v.input_count += 1;
        }
        Ok(v)
    }
    fn entry_point(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        self.inner.entry_point()
    }
    fn target(&self) -> Result<ShaderTarget, NcStatus> {
        if matches!(self.probe.fault, Fault::Target) {
            Ok(ShaderTarget::Wgsl)
        } else {
            self.inner.target()
        }
    }
    fn wgsl(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Err(neocogi_com::NC_E_LOAD_FAILED) // Never queried by a SPIR-V receiver.
    }
    fn spirv(&self) -> Result<ComPtr<INcBytesAbi>, NcStatus> {
        self.probe.call();
        if matches!(self.probe.fault, Fault::MissingCode) {
            return Err(NC_E_INVALID_ARG);
        }
        if matches!(self.probe.fault, Fault::CloseGetter)
            && let Some(domain) = self.probe.domain.borrow().upgrade()
        {
            domain.close();
        }
        if matches!(self.probe.fault, Fault::Binary) {
            use neocogi_com::INcBytesComPtrExt;
            let size = self.inner.spirv()?.len()?;
            return Ok(NcBytes::from_vec(vec![0; size as usize]));
        }
        let bytes = self.inner.spirv()?;
        if matches!(
            self.probe.fault,
            Fault::CloseExport | Fault::HugeLength | Fault::ChangingLength | Fault::CopyError
        ) {
            Ok(ExportProbe::create(bytes, self.probe.clone()))
        } else {
            Ok(bytes)
        }
    }
    fn glsl(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Err(neocogi_com::NC_E_LOAD_FAILED) // Never queried by a SPIR-V receiver.
    }
    fn inputs(&self) -> Result<ComPtr<IShaderLocationsAbi>, NcStatus> {
        if matches!(
            self.probe.fault,
            Fault::LocationCount | Fault::LocationEnum | Fault::DuplicateLocation | Fault::LocationDirection
        ) {
            return LocationsProbe::from_value(LocationsProbe { fault: self.probe.fault }).query();
        }
        self.inner.inputs()
    }
    fn outputs(&self) -> Result<ComPtr<IShaderLocationsAbi>, NcStatus> {
        self.inner.outputs()
    }
    fn uniforms(&self) -> Result<ComPtr<IShaderUniformsAbi>, NcStatus> {
        self.inner.uniforms()
    }
    fn sampled_textures(&self) -> Result<ComPtr<IShaderSampledTexturesAbi>, NcStatus> {
        self.inner.sampled_textures()
    }
}
/// Publishes the fixture's weak domain only after construction has finished.
fn fixture(fault: Fault, limit: u64) -> (GpuDevice, ComBox<IRenderClientDomainAbi>, ComPtr<IRenderResourcesAbi>, Rc<Probe>) {
    let probe = Rc::new(Probe {
        domain: RefCell::new(Weak::new()),
        fault,
        factory: RefCell::new(None),
        calls: Cell::new(0),
        release_bytes: Cell::new(0),
    });
    let inner = neocogi_platform::render::compiler::CompilerObject::from_value(
        neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::SpirV).unwrap(),
    )
    .query()
    .unwrap();
    let root = CompilerProxy::from_value(CompilerProxy { inner, probe: probe.clone() }).query().unwrap();
    let device = new_device(root);
    let domain = device
        .create_client(
            RenderClientLimits { live_bytes: limit, ..Default::default() },
            device.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions::default(),
        )
        .unwrap();
    let factory = domain.resources().unwrap();
    *probe.factory.borrow_mut() = Some(factory.downgrade().unwrap());
    *probe.domain.borrow_mut() = device.inner.factories.borrow().get(&(factory.as_raw().object as usize)).unwrap().domain.clone();
    (device, domain, factory, probe)
}
/// Every failure rolls back bytes/owners, and late closure is checked after Release.
#[test]
fn transfer_failures_reentry_and_release_close_are_atomic() {
    for (fault, expected) in [
        (Fault::MissingCode, NC_E_INVALID_ARG),
        (Fault::Binary, NC_E_INVALID_ARG),
        (Fault::Stage, NC_E_INVALID_ARG),
        (Fault::Target, NC_E_INVALID_ARG),
        (Fault::CodeCount, NC_E_INVALID_ARG),
        (Fault::CloseGetter, NC_E_DROPPED),
        (Fault::CloseRelease, NC_E_DROPPED),
        (Fault::CloseExport, NC_E_DROPPED),
        (Fault::HugeLength, NC_E_OUT_OF_RANGE),
        (Fault::ChangingLength, NC_E_INVALID_ARG),
        (Fault::CopyError, NC_E_INVALID_ARG),
        (Fault::Count, NC_E_INVALID_ARG),
        (Fault::LocationCount, NC_E_OUT_OF_RANGE),
        (Fault::LocationEnum, neocogi_com::NC_E_INVALID_TAG),
        (Fault::DuplicateLocation, NC_E_INVALID_ARG),
        (Fault::LocationDirection, NC_E_INVALID_ARG),
    ] {
        let (device, _domain, factory, probe) = fixture(fault, 8 * 1024 * 1024);
        let source = crate::gpu::source::Bytes::source(SOURCE.as_bytes().to_vec());
        assert_eq!(
            factory
                .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
                .map_err(|error| error.status)
                .err(),
            Some(expected)
        );
        assert_eq!(probe.domain.borrow().upgrade().unwrap().account.live_bytes(), 0);
        assert!(probe.calls.get() >= 3);
        assert!(probe.release_bytes.get() > 0);
        assert!(device.inner.shaders.borrow().is_empty());
    }
}
/// A successful provider can exceed receiving-copy headroom; its foreign payload
/// remains charged until Release and no shader is partially registered.
#[test]
fn receiving_copy_headroom_and_final_retention_are_exact() {
    let (device, domain, factory, probe) = fixture(Fault::None, 8 * 1024 * 1024);
    let source = crate::gpu::source::Bytes::source(SOURCE.as_bytes().to_vec());
    let shader = factory
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let p = device.live_bytes(factory.as_com_ref()).unwrap();
    assert_eq!(probe.release_bytes.get(), p * 2);
    drop(shader);
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 0);
    drop(domain);
    let (device, _domain, factory, probe) = fixture(Fault::None, p * 2 - 1);
    assert_eq!(
        factory
            .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
            .map_err(|error| error.status)
            .err(),
        Some(NC_E_OUT_OF_RANGE)
    );
    assert_eq!(probe.release_bytes.get(), p);
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 0);
    assert!(device.inner.shaders.borrow().is_empty());
}
/// Failure text remains owned after success, another failure, and final device destruction.
#[test]
fn shader_errors_outlive_success_and_device_destruction() {
    let (device, domain, factory, _probe) = fixture(Fault::None, 8 * 1024 * 1024);
    let bad = crate::gpu::source::Bytes::source(b"invalid wgsl".to_vec());
    let error = factory
        .create_shader(GpuShaderStage::Vertex, bad.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap_err();
    assert_eq!(error.status, NC_E_INVALID_ARG);
    assert!(error.message.contains("invalid"));
    let good = crate::gpu::source::Bytes::source(SOURCE.as_bytes().to_vec());
    drop(
        factory
            .create_shader(GpuShaderStage::Vertex, good.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
            .unwrap(),
    );
    let second = factory
        .create_shader(GpuShaderStage::Vertex, good.as_com_ref(), NcCopyFromBufferOffset::new(0, b"missing"))
        .unwrap_err();
    drop((factory, domain, device));
    assert!(error.message.contains("invalid"));
    assert!(!second.message.is_empty());
}

/// Losing error text must never replace the compiler's original failure code.
#[test]
fn failed_diagnostic_exports_preserve_compilation_status() {
    for fault in [Fault::Diagnostic, Fault::LongDiagnostic] {
        let (_device, _domain, factory, _probe) = fixture(fault, 8 * 1024 * 1024);
        let bad = crate::gpu::source::Bytes::source(b"invalid wgsl".to_vec());
        let error = factory
            .create_shader(GpuShaderStage::Vertex, bad.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
            .unwrap_err();
        assert_eq!(error.status, NC_E_INVALID_ARG);
        assert!(error.message.contains("error text unavailable"));
        assert!(error.message.len() <= MAX_RENDER_ERROR_BYTES);
    }
}

/// Raw SDK byte projection with valid initialized storage and observable final Release.
/// It models a foreign implementation without violating exact-write success semantics.
struct ExportProbe {
    /// Single-thread COM strong count; no weak projection is offered.
    refs: Cell<u32>,
    /// Real independently owned immutable bytes.
    inner: ComPtr<INcBytesAbi>,
    /// Destructor/reentry observations; contains only weak domain/factory links.
    probe: Rc<Probe>,
    /// Length becomes inconsistent only after an otherwise complete copy.
    copied: Cell<bool>,
}
impl ExportProbe {
    /// Pairs a fresh one-reference allocation with its static exact ABI table.
    fn create(inner: ComPtr<INcBytesAbi>, probe: Rc<Probe>) -> ComPtr<INcBytesAbi> {
        let object = Box::into_raw(Box::new(Self {
            refs: Cell::new(1),
            inner,
            probe,
            copied: Cell::new(false),
        }));
        // SAFETY: allocation and table agree, are initialized and transfer one owned reference.
        unsafe {
            ComPtr::from_raw_owned(neocogi_com::ComRaw {
                object: object.cast(),
                vtable: &EXPORT_TABLE,
            })
        }
        .unwrap()
    }
}
impl Drop for ExportProbe {
    fn drop(&mut self) {
        self.probe.call();
        if let Some(domain) = self.probe.domain.borrow().upgrade() {
            self.probe.release_bytes.set(domain.account.live_bytes());
            if matches!(self.probe.fault, Fault::CloseExport) {
                domain.close();
            }
        }
    }
}
/// Exact INcBytes table. All pointers supplied by these tests have matching live storage.
static EXPORT_TABLE: neocogi_com::INcBytesVTable = neocogi_com::INcBytesVTable {
    unknown: neocogi_com::IUnknownVTable {
        interface_id: &INcBytesAbi::INTERFACE_ID,
        query_interface: export_query,
        add_ref: export_add,
        release: export_release,
        downgrade: None,
    },
    len: export_len,
    byte_at: export_at,
    copy_to: export_copy,
};
/// Proper IUnknown projection preserves canonical identity and refcount.
static EXPORT_UNKNOWN: neocogi_com::IUnknownVTable = neocogi_com::IUnknownVTable {
    interface_id: &IUnknown::INTERFACE_ID,
    query_interface: export_query,
    add_ref: export_add,
    release: export_release,
    downgrade: None,
};
/// Query only the two projections this fixture implements.
/// SAFETY: matching object, readable identity and writable distinct raw output are supplied by COM.
unsafe extern "C" fn export_query(this: *mut core::ffi::c_void, id: *const neocogi_com::NcInterfaceId, out: *mut neocogi_com::ComRaw<IUnknown>) -> NcStatus {
    unsafe {
        out.write(neocogi_com::ComRaw::null());
        let vtable = if *id == INcBytesAbi::INTERFACE_ID {
            (&EXPORT_TABLE as *const neocogi_com::INcBytesVTable).cast()
        } else if *id == IUnknown::INTERFACE_ID {
            &EXPORT_UNKNOWN
        } else {
            return neocogi_com::NC_E_NOINTERFACE;
        };
        export_add(this);
        out.write(neocogi_com::ComRaw { object: this, vtable });
    }
    neocogi_com::NC_OK
}
/// SAFETY: paired live allocation remains borrowed for this retain operation.
unsafe extern "C" fn export_add(this: *mut core::ffi::c_void) -> u32 {
    let value = unsafe { &*this.cast::<ExportProbe>() };
    let next = value.refs.get().checked_add(1).unwrap();
    value.refs.set(next);
    next
}
/// SAFETY: consumes exactly one owned reference; final release reconstructs the original Box.
unsafe extern "C" fn export_release(this: *mut core::ffi::c_void) -> u32 {
    let value = unsafe { &*this.cast::<ExportProbe>() };
    let next = value.refs.get() - 1;
    value.refs.set(next);
    if next == 0 {
        unsafe {
            drop(Box::from_raw(this.cast::<ExportProbe>()));
        }
    }
    next
}
/// SAFETY: caller supplies a matching live object and one writable u64 output.
unsafe extern "C" fn export_len(this: *mut core::ffi::c_void, out: *mut u64) -> NcStatus {
    use neocogi_com::INcBytesComPtrExt;
    let value = unsafe { &*this.cast::<ExportProbe>() };
    value.probe.call();
    match value.inner.len() {
        Ok(len) => {
            unsafe {
                out.write(if matches!(value.probe.fault, Fault::HugeLength) {
                    u64::MAX
                } else if matches!(value.probe.fault, Fault::ChangingLength) && value.copied.get() {
                    len + 4
                } else {
                    len
                });
            }
            neocogi_com::NC_OK
        }
        Err(e) => e,
    }
}
/// SAFETY: valid COM output and matched live object are provided; the inner exact ABI validates index.
unsafe extern "C" fn export_at(this: *mut core::ffi::c_void, index: u64, out: *mut u8) -> NcStatus {
    let value = unsafe { &*this.cast::<ExportProbe>() };
    let raw = value.inner.as_raw();
    unsafe { ((*raw.vtable).byte_at)(raw.object, index, out) }
}
/// SAFETY: caller provides a valid nonoverlapping output of len bytes. Successful
/// forwarding initializes all bytes; an injected error returns before touching output.
unsafe extern "C" fn export_copy(this: *mut core::ffi::c_void, offset: u64, len: u64, out: *mut u8) -> NcStatus {
    let value = unsafe { &*this.cast::<ExportProbe>() };
    value.probe.call();
    if matches!(value.probe.fault, Fault::CopyError) {
        return NC_E_INVALID_ARG;
    }
    let raw = value.inner.as_raw();
    let status = unsafe { ((*raw.vtable).copy_to)(raw.object, offset, len, out) };
    value.copied.set(true);
    status
}

/// Malformed metadata remains initialized and ABI-valid, exercising value validation.
#[com_object(interfaces(neocogi_platform::render::abi::IShaderLocations))]
struct LocationsProbe {
    /// Chosen bounded or oversized value case.
    fault: Fault,
}
impl IShaderLocations for LocationsProbe {
    fn len(&self) -> Result<u32, NcStatus> {
        Ok(match self.fault {
            Fault::LocationCount => 17,
            Fault::DuplicateLocation => 2,
            _ => 1,
        })
    }
    fn get(&self, index: u32) -> Result<ComPtr<IShaderLocationAbi>, NcStatus> {
        if index >= self.len()? {
            return Err(NC_E_OUT_OF_RANGE);
        }
        LocationProbe::from_value(LocationProbe { fault: self.fault }).query()
    }
}
/// Independent item owns only numeric wire values, including open-enum unknown tags.
#[com_object(interfaces(neocogi_platform::render::abi::IShaderLocation))]
struct LocationProbe {
    /// One finite metadata intervention.
    fault: Fault,
}
impl IShaderLocation for LocationProbe {
    fn info(&self) -> Result<ShaderLocationInfo, NcStatus> {
        Ok(ShaderLocationInfo {
            stage: GpuShaderStage::Vertex,
            direction: if matches!(self.fault, Fault::LocationDirection) {
                ShaderDirection::Output
            } else {
                ShaderDirection::Input
            },
            location: 0,
            scalar: if matches!(self.fault, Fault::LocationEnum) {
                ShaderScalar::from_raw(255)
            } else {
                ShaderScalar::Float32
            },
            components: 2,
            interpolation: ShaderInterpolation::None,
            sampling: ShaderSampling::None,
        })
    }
}
