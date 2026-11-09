// The validated platform implementation expands at crate root from lib.rs.
// Its public surface and native owners are identical for each legal backend selection.
use neocogi_platform::render::platform::*;
use neocogi_platform_contracts::audio::{AudioConfig, AudioDirection, IAudioCapabilitiesAbi, IAudioOpenRequestAbi};
use neocogi_platform::render::abi::*;
use neocogi_platform::platform::prepared::{PreparedFrame, Resources};
use neocogi_com::{ComInterface, ComPtr, ComRef, IUnknown, NcCopyFromBufferOffset, NcStatus, NC_E_INVALID_ARG, com_object};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};
mod clipboard;
mod prepared;
mod wheel;
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

#[com_object(neocogi_platform::render::platform::Platform)]
/// Native bootstrap; initialization is fallible and no native subsystem crosses the COM boundary.
pub struct PlatformObject {
    /// One SDL event pump shared by this platform's windows.
    #[cfg(not(target_arch = "wasm32"))]
    host: RefCell<Option<Rc<NativeHost>>>,
}

impl PlatformObject {
    /// Creates a native-free platform; each device later owns its backend-bound compiler.
    pub fn new() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            host: RefCell::new(None),
        }
    }
}

impl Default for PlatformObject {
    /// Same native-free construction as `new`.
    fn default() -> Self { Self::new() }
}

impl Platform for PlatformObject {
    /// Query native/backend observations without constructing a window or requesting capture permission.
    fn audio_capabilities(&self) -> Result<ComPtr<IAudioCapabilitiesAbi>, NcStatus> { crate::audio::capabilities() }
    /// Audio initializes independently of the window host and never constructs a graphics device.
    fn open_audio(&self, direction: AudioDirection, config: AudioConfig) -> Result<neocogi_com::ComBox<IAudioOpenRequestAbi>, NcStatus> {
        crate::audio::open(direction, config)
    }
    /// Validates input and publishes one complete native object only after the selected backend exists.
    fn create_window_result(&self, title: NcCopyFromBufferOffset<'_, u8>, size: Size) -> Result<neocogi_com::ComBox<IWindowCreationResultAbi>, NcStatus> {
        let result = (|| -> Result<_, RenderError> {
            if title.offset() != 0 || size.width <= 0 || size.height <= 0 {
                return Err(NC_E_INVALID_ARG.into());
            }
            let title = std::str::from_utf8(title.as_slice()).map_err(|_| NC_E_INVALID_ARG)?;
            Ok(WindowObject::from_value(self.window(title, size)?)
                .query()
                .expect("native window declares IPlatformWindow"))
        })();
        Ok(window_creation_result(result.map_err(|error| error.context("create window"))))
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
    /// Immutable device-free success report reused by successful frame and maintenance calls.
    success: ComPtr<IRenderOutcomeAbi>,
    /// Generic resource provenance and CPU recording shared with this native device.
    logical: neocogi_platform::gpu::GpuDevice<gpu::Backend>,
    /// Canonical COM identity maps contain weak native records and never own their COM objects.
    textures: Rc<RefCell<HashMap<usize, Weak<TextureRecord>>>>,
    /// Weak construction records authenticate surface parameters without querying caller metadata.
    /// COM uses shared receivers, so registration/retirement use short interior borrows; no registry
    /// borrow spans foreign QI/AddRef/Release, and a public surface removes its entry before address reuse.
    surfaces: RefCell<HashMap<usize, Weak<surface::SurfaceRecord>>>,
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
    /// Observes the actual drawable without synchronizing browser CSS or acquiring a frame.
    /// Metadata queries and final submission admission share this owner even while render state is detached.
    fn drawable_size(&self) -> Result<Size, NcStatus> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Ok(gpu::pixel_size(&self.window))
        }
        #[cfg(target_arch = "wasm32")]
        {
            Ok(Size {
                width: self.window.width() as i32,
                height: self.window.height() as i32,
            })
        }
    }

    /// Takes the complete native state; nested render/upload attempts cannot borrow its internals.
    fn access(&self) -> Result<RenderAccess<'_>, NcStatus> {
        let state = self.render.try_borrow_mut().map_err(|_| RENDER_E_BUSY)?.take().ok_or(RENDER_E_BUSY)?;
        Ok(RenderAccess { device: self, state: Some(state) })
    }
    /// Preserves backend text directly in the operation's returned error.
    fn failure(&self, error: impl std::fmt::Display) -> RenderError {
        RenderError::new(RENDER_E_BACKEND, error.to_string())
    }
    /// Reuses only immutable success; each failure has an independent owned report.
    fn complete(&self, result: Result<(), RenderError>, operation: &str) -> ComPtr<IRenderOutcomeAbi> {
        match result {
            Ok(()) => self.success.clone(),
            Err(error) => operation_result(Err(error.context(operation))),
        }
    }
    /// Retires submitted snapshots before completing an operation. The primary
    /// failure wins over a cleanup failure; successful work reports failed cleanup.
    /// No native/registry borrow spans a foreign stream, source, or destructor.
    fn execute<T>(&self, operation: impl FnOnce(&mut RenderState) -> Result<T, RenderError>) -> Result<T, RenderError> {
        let mut access = self.access()?;
        let result = operation(access.state());
        let cleanup = access.retire();
        result.and_then(|value| cleanup.map(|()| value))
    }
}
impl RenderAccess<'_> {
    /// Gives local Rust execution exclusive access without lending it through a COM object.
    fn state(&mut self) -> &mut RenderState {
        self.state.as_mut().expect("live render transaction")
    }
    /// Restores native ownership on both cleanup outcomes; a completed retirement is idempotent.
    fn retire(&mut self) -> Result<(), RenderError> {
        let Some(mut state) = self.state.take() else {
            return Ok(());
        };
        state.prepared.clear();
        let result = state.gpu.maintain().map_err(|error| self.device.failure(error));
        *self.device.render.borrow_mut() = Some(state);
        result
    }
}
impl Drop for RenderAccess<'_> {
    /// Panic and status-only metadata paths still restore ownership and attempt
    /// cleanup. Result-bearing native operations call retire explicitly so their
    /// cleanup failure is returned. Destructors cannot deliver a new error.
    fn drop(&mut self) {
        let _ = self.retire();
    }
}
#[com_object(interfaces(IPlatformWindow, IRenderer, IRenderClientFactory, ITextureFactory, IRenderSurfaceFactory))]
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

impl Device {
    /// Resolve the selected surface before constructing its independently controlled resource group.
    /// Device access spans foreign AddRef/QI/Release so callbacks cannot acquire a nested native frame.
    /// Only locally registered surfaces are accepted; the logical device then validates permissions/limits.
    fn create_client(
        &self,
        limits: RenderClientLimits,
        surface: ComRef<'_, IRenderSurfaceAbi>,
        permissions: RenderClientPermissions,
    ) -> Result<neocogi_com::ComBox<IRenderClientDomainAbi>, NcStatus> {
        let _access = self.access()?;
        let record = self.surface(surface.try_add_ref()?)?;
        self.logical.create_client(limits, record.target.clone(), permissions)
    }
}
impl IRenderClientFactory for WindowObject {
    /// Application setup selects the actual target and delegates only the resulting restricted factory.
    fn create_client(
        &self,
        limits: RenderClientLimits,
        surface: ComRef<'_, IRenderSurfaceAbi>,
        permissions: RenderClientPermissions,
    ) -> Result<neocogi_com::ComBox<IRenderClientDomainAbi>, NcStatus> {
        self.device.create_client(limits, surface, permissions)
    }
}
impl IRenderClientFactory for GraphicsHost {
    /// Grants the same group assembly as the window without exposing window presentation or OS interfaces.
    fn create_client(
        &self,
        limits: RenderClientLimits,
        surface: ComRef<'_, IRenderSurfaceAbi>,
        permissions: RenderClientPermissions,
    ) -> Result<neocogi_com::ComBox<IRenderClientDomainAbi>, NcStatus> {
        self.device.create_client(limits, surface, permissions)
    }
}
#[com_object(interfaces(IRenderHost, IRenderClientFactory, IRenderSurfaceFactory))]
/// Trusted graphics assembly retains no window COM object, input registration or UI runtime.
/// Its surface/group factory projections select targets and permissions; they cannot recover the window
/// renderer. An independently created image surface exposes only that image's renderer.
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
    fn maintain_result(&self) -> Result<ComPtr<IRenderOutcomeAbi>, NcStatus> {
        Ok(self.device.complete(self.device.execute(|_| Ok(())), "maintain renderer"))
    }
    /// Resolves image provenance while detached; the result exclusively owns the imported texture.
    fn import_texture_result(
        &self,
        resources: ComRef<'_, IRenderResourcesAbi>,
        image: ComRef<'_, ITextureAbi>,
    ) -> Result<neocogi_com::ComBox<ITextureImportResultAbi>, NcStatus> {
        let result = self.device.execute(|_| {
            // Canonical QI and receiver-owned foreign Release finish before final admission.
            let record = {
                let image = image.try_add_ref()?;
                self.device.texture(image)?
            };
            Ok(self.device.logical.import_texture(resources, record.image.clone())?)
        });
        Ok(texture_import_result(result.map_err(|error| error.context("import texture"))))
    }
    /// Prepares the owned version without acquiring a target and returns its exact native failure.
    fn prepare_texture_result(
        &self,
        resources: ComRef<'_, IRenderResourcesAbi>,
        texture: ComRef<'_, IGpuBufferAbi>,
    ) -> Result<ComPtr<IRenderOutcomeAbi>, NcStatus> {
        let result = self.device.execute(|state| {
            let version = self.device.logical.imported_texture_snapshot(resources, texture)?;
            state.gpu.prepare_texture(version).map_err(|error| self.device.failure(error))
        });
        Ok(self.device.complete(result, "prepare texture"))
    }

    /// Separate broker identity prevents indirect QueryInterface access to the window.
    fn recording_broker(&self) -> Result<ComPtr<IRenderViewBrokerAbi>, NcStatus> {
        Ok(self.device.logical.broker())
    }
}

impl IRenderer for WindowObject {
    /// Prepares the complete frame without borrowing native state, then executes and explicitly finishes it.
    fn render_result(&self, info: RenderInfo, stream: ComRef<'_, IRenderStreamAbi>) -> Result<ComPtr<IRenderOutcomeAbi>, NcStatus> {
        let result = (|| -> Result<(), RenderError> {
            if info.size != self.size()? {
                return Err(RenderError::new(NC_E_INVALID_ARG, "submitted frame size differs from the window drawable size"));
            }
            self.device.execute(|state| {
                let _operation = self.device.logical.operation()?;
                state
                    .prepared
                    .read(info, stream, self)
                    .map_err(|status| RenderError::from(status).context("frame preparation"))?;
                // Foreign callbacks may close groups, replace targets or synchronize a browser
                // canvas resize. Recheck all admission inputs before any native frame acquisition.
                for block in state.prepared.blocks() {
                    block.admit_framebuffer()?;
                }
                if info.size != self.device.drawable_size()? {
                    return Err(RenderError::new(NC_E_INVALID_ARG, "window drawable size changed during frame preparation"));
                }
                state.gpu.render(info, &state.prepared).map_err(|error| self.device.failure(error))
            })
        })();
        Ok(self.device.complete(result, "render window"))
    }
}

/// Queries canonical identity before borrowing any native registry; foreign QueryInterface may reenter.
fn identity<I: ComInterface>(interface: &ComPtr<I>) -> Result<usize, NcStatus> {
    let identity: ComPtr<IUnknown> = interface.query()?;
    Ok(identity.as_raw().object as usize)
}

#[cfg(feature = "glow")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.platform.glow")]
fn acquire_root(_runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_platform::render::platform::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::new()).query()
}
#[cfg(feature = "vulkan")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.platform.vulkan")]
fn acquire_root(_runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_platform::render::platform::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::new()).query()
}
#[cfg(feature = "wgpu")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.platform.wgpu")]
fn acquire_root(_runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_platform::render::platform::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::new()).query()
}
#[cfg(feature = "webgl")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.platform.webgl")]
fn acquire_root(_runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_platform::render::platform::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::new()).query()
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

/// Owned acceptance observations, enabled explicitly and absent from normal deployment builds.
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

#[cfg(all(any(feature = "glow", feature = "webgl"), any(test, all(target_arch = "wasm32", feature = "test-support"))))]
mod extent_tests;
