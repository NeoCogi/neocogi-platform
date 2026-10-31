// The validated renderer implementation expands at crate root from lib.rs.
// Its public surface and native owners are identical for each legal backend selection.
use neocogi_render_core::platform::*;
use neocogi_render_core::abi::*;
use neocogi_render_support::com::{PreparedFrame, Resources};
use neocogi_com::{ComInterface, ComPtr, ComRef, IUnknown, INcStringAbi, NcCopyFromBufferOffset, NcStatus, NcString, NC_E_INVALID_ARG, com_object};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};
mod clipboard;
mod gpu;
#[cfg(not(target_arch = "wasm32"))]
mod input;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native::NativeHost;
#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
#[path = "browser_input.rs"]
mod input;
mod resources;
mod surface;
use resources::TextureRecord;

/// Prepared resources are concrete native records, never erased objects or callbacks.
type Prepared = PreparedFrame<gpu::Backend>;

#[com_object(neocogi_render_core::platform::Platform)]
/// Native bootstrap; initialization is fallible and no native subsystem crosses the COM boundary.
pub struct PlatformObject {
    /// Required CPU compiler acquired before any native subsystem is constructed.
    compiler: ComPtr<neocogi_shader_core::abi::ShaderCompilerAbi>,
    /// One SDL event pump shared by this platform's windows.
    #[cfg(not(target_arch = "wasm32"))]
    host: RefCell<Option<Rc<NativeHost>>>,
    /// Owned detail from the most recent failed native construction.
    diagnostic: RefCell<String>,
}

impl PlatformObject {
    /// Explicit dependency injection; there is no implicit linked compiler fallback.
    pub fn new(compiler: ComPtr<neocogi_shader_core::abi::ShaderCompilerAbi>) -> Self {
        Self {
            compiler,
            #[cfg(not(target_arch = "wasm32"))]
            host: RefCell::new(None),
            diagnostic: RefCell::new(String::new()),
        }
    }
}

impl Platform for PlatformObject {
    /// Validates input and publishes one complete native object only after the selected backend exists.
    fn create_window(&self, title: NcCopyFromBufferOffset<'_, u8>, size: Size) -> Result<ComPtr<IPlatformWindowAbi>, NcStatus> {
        if title.offset() != 0 || size.width <= 0 || size.height <= 0 {
            return Err(NC_E_INVALID_ARG);
        }
        let title = std::str::from_utf8(title.as_slice()).map_err(|_| NC_E_INVALID_ARG)?;
        Ok(WindowObject::from_value(self.window(title, size)?)
            .query()
            .expect("native window declares IPlatformWindow"))
    }
}
impl IRenderDiagnostics for PlatformObject {
    /// Copies construction detail; callers may retain the resulting immutable text independently.
    fn diagnostic(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(NcString::from_utf8_str(&self.diagnostic.borrow()))
    }
}

/// Native renderer and reusable preparation storage are detached together across all foreign stream calls.
struct RenderState {
    /// Owns the generic native backend, context and presentation surface.
    gpu: gpu::Gpu,
    /// Resource references are cleared before restoring this state, preventing a Device ownership cycle.
    prepared: Prepared,
}

/// Existing driver facade retained by active window/surface/host capabilities; images retain only Core.
struct Device {
    /// None throughout preparation and submission; reentrant GPU operations receive RENDER_E_BUSY.
    render: RefCell<Option<RenderState>>,
    /// Generic resource provenance and CPU recording shared with this native device.
    logical: neocogi_render_gpu::GpuDevice<gpu::Backend>,
    /// Canonical COM identity maps contain weak native records and never own their COM objects.
    textures: Rc<RefCell<HashMap<usize, Weak<TextureRecord>>>>,
    /// Polling can access a live SDL owner even while renderer state is detached.
    window: gpu::Window,
    /// SDL subsystems outlive the backend and every native window clone.
    #[cfg(not(target_arch = "wasm32"))]
    host: Rc<NativeHost>,
}

/// Private ownership transaction, never a RefCell guard held while foreign code runs.
struct RenderAccess<'a> {
    /// Native owner remains alive until restoration and pending destruction finish.
    device: &'a Device,
    /// Owned state is taken exactly once and restored by Drop on every Result path.
    state: Option<RenderState>,
}
impl Device {
    /// Takes the complete native state; nested render/upload attempts cannot borrow its internals.
    fn access(&self) -> Result<RenderAccess<'_>, NcStatus> {
        let state = self.render.try_borrow_mut().map_err(|_| RENDER_E_BUSY)?.take().ok_or(RENDER_E_BUSY)?;
        Ok(RenderAccess { device: self, state: Some(state) })
    }
    /// Preserves native backend failure text rather than erasing it into a status alone.
    fn failure(&self, error: impl std::fmt::Display) -> NcStatus {
        self.logical.set_diagnostic(error.to_string());
        RENDER_E_BACKEND
    }
}
impl RenderAccess<'_> {
    /// Gives local Rust execution exclusive access without lending it through a COM object.
    fn state(&mut self) -> &mut RenderState {
        self.state.as_mut().expect("live render transaction")
    }
}
impl Drop for RenderAccess<'_> {
    /// Release snapshots before native texture destruction and restore even after failed preparation/finish.
    fn drop(&mut self) {
        let mut state = self.state.take().expect("render transaction restored once");
        state.prepared.clear();
        if let Err(error) = state.gpu.maintain() {
            self.device.failure(error);
        }
        *self.device.render.borrow_mut() = Some(state);
    }
}
#[com_object(interfaces(IPlatformWindow, IRenderer, IRenderClientFactory, ITextureFactory, IRenderSurfaceFactory, IRenderDiagnostics))]
/// One COM identity exposes OS polling, rendering and concrete resource factories.
struct WindowObject {
    /// Resources retain this same native owner after the public window interface is released.
    device: Rc<Device>,
    /// Fractional wheel input belongs to this window and is reset on focus cancellation.
    #[cfg(not(target_arch = "wasm32"))]
    input: RefCell<input::InputState>,
    /// DOM registrations and committed text queue belong to the canvas COM object.
    #[cfg(target_arch = "wasm32")]
    input: input::BrowserInputHost,
    /// Rejects recursive delivery even after native and queue borrows have been released.
    polling: Cell<bool>,
}

/// Resets delivery authority on all exits without keeping a RefCell borrow during a callback.
struct Polling<'a> {
    /// Delivery flag whose owner remains borrowed until the current callback completes.
    active: &'a Cell<bool>,
}
impl Drop for Polling<'_> {
    fn drop(&mut self) {
        self.active.set(false);
    }
}

impl IRenderClientFactory for WindowObject {
    /// Publishes only a narrow domain controller, then clients receive its separate factory.
    fn create_client(&self, limits: RenderClientLimits) -> Result<neocogi_com::ComBox<IRenderClientDomainAbi>, NcStatus> {
        self.device.logical.create_client(limits)
    }
}
#[com_object(interfaces(IRenderHost))]
/// Trusted graphics assembly retains no window COM object, input registration or UI runtime.
struct GraphicsHost {
    /// Native resources survive public window release without retaining UI/event authority.
    device: Rc<Device>,
}
impl GraphicsHost {
    /// Publish a separate allocation so QI cannot turn retained assembly access into a window.
    fn for_device(device: Rc<Device>) -> Result<ComPtr<IRenderHostAbi>, NcStatus> {
        Self::from_value(Self { device }).query()
    }
}
impl IRenderHost for GraphicsHost {
    /// Reclamation is reachable before GFX lowers and allocates the next frame's resources.
    fn maintain(&self) -> Result<(), NcStatus> {
        let mut access = self.device.access()?;
        access.state().gpu.maintain().map_err(|error| self.device.failure(error))
    }
    /// The private native registry authenticates ordinary image identity before logical import.
    fn import_ui_texture(
        &self,
        resources: ComRef<'_, IRenderResourcesAbi>,
        image: ComRef<'_, ITextureAbi>,
    ) -> Result<neocogi_com::ComBox<IGpuTextureAbi>, NcStatus> {
        let _access = self.device.access()?;
        // Finish canonical QI and the receiver-owned foreign Release before final admission.
        let record = {
            let image = image.try_add_ref()?;
            self.device.texture(image)?
        };
        self.device.logical.import_ui_texture(resources, record.image.clone())
    }

    /// Logical provenance is checked while native state is detached; driver calls consume owned data.
    fn prepare_ui_texture(&self, resources: ComRef<'_, IRenderResourcesAbi>, texture: ComRef<'_, IGpuTextureAbi>) -> Result<(), NcStatus> {
        let mut access = self.device.access()?;
        let version = self.device.logical.ui_texture_snapshot(resources, texture)?;
        access.state().gpu.prepare_texture(version).map_err(|error| self.device.failure(error))
    }

    /// Private compositor domains share the physical core without retaining this driver facade.
    fn create_ui_client(&self, limits: RenderClientLimits) -> Result<neocogi_com::ComBox<IRenderClientDomainAbi>, NcStatus> {
        self.device.logical.create_ui_client(limits)
    }
    /// Separate broker identity prevents indirect QueryInterface access to the window.
    fn recording_broker(&self) -> Result<ComPtr<IRenderViewBrokerAbi>, NcStatus> {
        Ok(self.device.logical.broker())
    }
}

impl IRenderer for WindowObject {
    /// Prepares the complete frame without borrowing native state, then executes and explicitly finishes it.
    fn render(&self, info: RenderInfo, stream: ComRef<'_, IRenderStreamAbi>) -> Result<(), NcStatus> {
        if info.size != self.size()? {
            return Err(NC_E_INVALID_ARG);
        }
        let mut access = self.device.access()?;
        let state = access.state();
        let _operation = self.device.logical.operation()?;
        state.prepared.read(info, stream, self).inspect_err(|error| {
            self.device.logical.set_diagnostic(format!("frame preparation failed: {error}"));
        })?;
        state.gpu.render(info, &state.prepared).map_err(|error| self.device.failure(error))
    }
}
impl IRenderDiagnostics for WindowObject {
    /// Retains a copy so subsequent native failures cannot mutate an earlier diagnostic.
    fn diagnostic(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(NcString::from_utf8_str(&self.device.logical.diagnostic()))
    }
}

/// Queries canonical identity before borrowing any native registry; foreign QueryInterface may reenter.
fn identity<I: ComInterface>(interface: &ComPtr<I>) -> Result<usize, NcStatus> {
    let identity: ComPtr<IUnknown> = interface.query()?;
    Ok(identity.as_raw().object as usize)
}

#[cfg(feature = "glow")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.render.glow", dependencies = [neocogi_shader_core::SHADER_COMPILER_PLUGIN_ID])]
fn acquire_root(runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_render_core::platform::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::new(
        runtime.acquire_plugin_root(neocogi_shader_core::SHADER_COMPILER_PLUGIN_ID)?,
    ))
    .query()
}
#[cfg(feature = "vulkan")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.render.vulkan", dependencies = [neocogi_shader_core::SHADER_COMPILER_PLUGIN_ID])]
fn acquire_root(runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_render_core::platform::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::new(
        runtime.acquire_plugin_root(neocogi_shader_core::SHADER_COMPILER_PLUGIN_ID)?,
    ))
    .query()
}
#[cfg(feature = "wgpu")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.render.wgpu", dependencies = [neocogi_shader_core::SHADER_COMPILER_PLUGIN_ID])]
fn acquire_root(runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_render_core::platform::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::new(
        runtime.acquire_plugin_root(neocogi_shader_core::SHADER_COMPILER_PLUGIN_ID)?,
    ))
    .query()
}
#[cfg(feature = "webgl")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.render.webgl", dependencies = [neocogi_shader_core::SHADER_COMPILER_PLUGIN_ID])]
fn acquire_root(runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_render_core::platform::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::new(
        runtime.acquire_plugin_root(neocogi_shader_core::SHADER_COMPILER_PLUGIN_ID)?,
    ))
    .query()
}
// Only the native assembly owns the dynamic entrypoint; browser assembly uses static descriptors.

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

impl IRenderDriverBound for WindowObject {
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        Ok(self.device.logical.driver_identity())
    }
}

impl IRenderDriverBound for GraphicsHost {
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        Ok(self.device.logical.driver_identity())
    }
}
