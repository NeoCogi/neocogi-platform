//! Deployable rendering plugin with build-selected Glow, Vulkan, WGPU, or WebGL execution.
//!
//! Each backend build has its own plugin identity. The native window, input pump, GPU resources,
//! and renderer deliberately remain in that same artifact: splitting those owners would expose
//! native handles or add another dynamic call boundary to every frame.
#![deny(missing_docs)]

#[cfg(not(any(feature = "webgl", feature = "glow", feature = "vulkan", feature = "wgpu")))]
compile_error!("select exactly one renderer feature: glow, vulkan, wgpu, or webgl");
#[cfg(any(
    all(feature = "webgl", any(feature = "glow", feature = "vulkan", feature = "wgpu")),
    all(feature = "glow", any(feature = "vulkan", feature = "wgpu")),
    all(feature = "vulkan", feature = "wgpu"),
))]
compile_error!("renderer features are mutually exclusive");
#[cfg(all(feature = "webgl", not(target_arch = "wasm32")))]
compile_error!("the webgl renderer plugin requires wasm32");
#[cfg(all(target_arch = "wasm32", not(feature = "webgl")))]
compile_error!("wasm32 rendering requires the webgl feature");
use neocogi_platform_contracts::*;
use neocogi_renderer_contracts::*;
use neocogi_demo_render_contracts::*;
use neocogi_demo_renderer::{MeshBuffers, MeshSubmission};
use neocogi_render_support::com::{PreparedFrame, Resources};
use neocogi_render_core::TextureId;
use neocogi_com::{
    ComFactoryBootstrap, ComInterface, ComPtr, ComRef, IUnknown, INcStringAbi, NcCopyFromBufferOffset, NcStatus, NcString, NC_E_INVALID_ARG, com_factory,
    com_object,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};
mod gpu;
mod pixels;
use pixels::AtlasPixels;
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
use resources::{SceneRecord, TextureRecord};

/// Prepared resources are concrete native records, never erased objects or callbacks.
type Prepared = PreparedFrame<Rc<TextureRecord>, MeshSubmission>;

#[derive(Default)]
#[com_factory(neocogi_platform_contracts::Platform)]
/// Native bootstrap; initialization is fallible and no native subsystem crosses the COM boundary.
pub struct PlatformObject {
    /// One SDL event pump shared by this platform's windows.
    #[cfg(not(target_arch = "wasm32"))]
    host: RefCell<Option<Rc<NativeHost>>>,
    /// Owned detail from the most recent failed native construction.
    diagnostic: RefCell<String>,
}

impl Platform for PlatformObject {
    /// Validates input and publishes one complete native object only after the selected backend exists.
    fn create_window(
        &self,
        title: NcCopyFromBufferOffset<'_, u8>,
        size: Size,
        atlas: ComRef<'_, IRgbaImageAbi>,
    ) -> Result<ComPtr<IPlatformWindowAbi>, NcStatus> {
        if title.offset() != 0 || size.width <= 0 || size.height <= 0 {
            return Err(NC_E_INVALID_ARG);
        }
        let title = std::str::from_utf8(title.as_slice()).map_err(|_| NC_E_INVALID_ARG)?;
        Ok(WindowObject::from_value(self.window(title, size, atlas)?)
            .query()
            .expect("native window declares IPlatformWindow"))
    }
}

impl ComFactoryBootstrap for PlatformObject {
    /// Creates the stateful renderer root without publishing constructor arguments through the ABI.
    fn create_factory() -> Result<ComPtr<PlatformAbi>, NcStatus> {
        Self::from_value(Self::default()).query()
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
    /// Owns the native backend, its application renderer, context and presentation surface.
    gpu: gpu::Gpu,
    /// Resource references are cleared before restoring this state, preventing a Device ownership cycle.
    prepared: Prepared,
    /// Current immutable atlas image; a same-pointer preparation performs no copy or upload.
    atlas: ComPtr<IRgbaImageAbi>,
}
impl RenderState {
    /// Copies and uploads candidate pixels before retaining the new image identity.
    fn prepare_atlas(&mut self, source: ComRef<'_, IRgbaImageAbi>) -> Result<(), String> {
        if self.atlas.as_ptr() == source.as_ptr() {
            return Ok(());
        }
        // The image projection deliberately carries no fonts, names, theme records, or catalog.
        // Those higher-level meanings have already been resolved before renderer submission.
        let pixels = AtlasPixels::read(source).map_err(|status| status.to_string())?;
        self.gpu.replace_atlas(pixels.image())?;
        self.atlas = source.add_ref();
        Ok(())
    }
}

/// Shared native lifetime retained by the window, every image and every editable scene.
struct Device {
    /// None throughout preparation and submission; reentrant GPU operations receive RENDER_E_BUSY.
    render: RefCell<Option<RenderState>>,
    /// Canonical COM identity maps contain weak native records and never own their COM objects.
    textures: RefCell<HashMap<usize, Weak<TextureRecord>>>,
    /// Application scenes use the same authentication rule as images.
    scenes: RefCell<HashMap<usize, Weak<SceneRecord>>>,
    /// Final texture releases during a detached frame wait until native execution has ended.
    retired: RefCell<Vec<TextureId>>,
    /// Owned detail survives failed preparation, upload, submission or presentation.
    diagnostic: RefCell<String>,
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
        *self.diagnostic.borrow_mut() = error.to_string();
        RENDER_E_BACKEND
    }
    /// Deletes immediately when idle; a live frame's ownership transaction drains the same queue on Drop.
    fn retire(&self, id: TextureId) {
        self.retired.borrow_mut().push(id);
        if let Ok(access) = self.access() {
            drop(access);
        }
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
        if let Err(error) = state.gpu.retire(&mut self.device.retired.borrow_mut()) {
            self.device.failure(error);
        }
        *self.device.render.borrow_mut() = Some(state);
    }
}
#[com_object(interfaces(IPlatformWindow, IRenderer, ITextureFactory, IDemoScenes, IRenderDiagnostics))]
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

impl IRenderer for WindowObject {
    /// Takes owned device state so nested image methods cannot reenter native upload or submission.
    fn prepare_atlas(&self, atlas: ComRef<'_, IRgbaImageAbi>) -> Result<(), NcStatus> {
        let mut access = self.device.access()?;
        access.state().prepare_atlas(atlas).map_err(|error| self.device.failure(error))
    }
    /// Prepares the complete frame without borrowing native state, then executes and explicitly finishes it.
    fn render(&self, info: RenderInfo, atlas: ComRef<'_, IRgbaImageAbi>, stream: ComRef<'_, IRenderStreamAbi>) -> Result<(), NcStatus> {
        if info.size != self.size()? {
            return Err(NC_E_INVALID_ARG);
        }
        let mut access = self.device.access()?;
        let state = access.state();
        state.prepared.read(info, stream, self).inspect_err(|error| {
            *self.device.diagnostic.borrow_mut() = format!("frame preparation failed: {error}");
        })?;
        // Reject malformed commands and foreign resource owners before changing GPU atlas state.
        state.prepare_atlas(atlas).map_err(|error| self.device.failure(error))?;
        state.gpu.render(info, &state.prepared).map_err(|error| self.device.failure(error))
    }
}
impl IRenderDiagnostics for WindowObject {
    /// Retains a copy so subsequent native failures cannot mutate an earlier diagnostic.
    fn diagnostic(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(NcString::from_utf8_str(&self.device.diagnostic.borrow()))
    }
}

/// Queries canonical identity before borrowing any native registry; foreign QueryInterface may reenter.
fn identity<I: ComInterface>(interface: &ComPtr<I>) -> Result<usize, NcStatus> {
    let identity: ComPtr<IUnknown> = interface.query()?;
    Ok(identity.as_ptr() as usize)
}

#[cfg(feature = "glow")]
neocogi_com::factory_plugin! {
    name = "neocogi.render.glow",
    version = "0.9.0-alpha.0",
    factory = PlatformObject,
}
#[cfg(feature = "vulkan")]
neocogi_com::factory_plugin! {
    name = "neocogi.render.vulkan",
    version = "0.9.0-alpha.0",
    factory = PlatformObject,
}
#[cfg(feature = "wgpu")]
neocogi_com::factory_plugin! {
    name = "neocogi.render.wgpu",
    version = "0.9.0-alpha.0",
    factory = PlatformObject,
}
#[cfg(feature = "webgl")]
neocogi_com::factory_plugin! {
    name = "neocogi.render.webgl",
    version = "0.9.0-alpha.0",
    factory = PlatformObject,
}
// Only the native assembly owns the dynamic entrypoint; browser assembly uses static descriptors.
#[cfg(feature = "dynamic-plugin")]
neocogi_com::export_factory_plugin!();

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
