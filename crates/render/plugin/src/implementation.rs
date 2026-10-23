// The validated renderer implementation expands at crate root from lib.rs.
// Its public surface and native owners are identical for each legal backend selection.
use neocogi_platform_contracts::*;
use neocogi_renderer_contracts::*;
use neocogi_render_mesh_contracts::*;
use neocogi_render_mesh::{MeshBuffers, MeshSubmission};
use neocogi_render_support::com::{PreparedFrame, Resources};
use neocogi_render_core::TextureId;
use neocogi_com::{ComInterface, ComPtr, ComRef, IUnknown, INcStringAbi, NcCopyFromBufferOffset, NcStatus, NcString, NC_E_INVALID_ARG, com_object};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};
mod clipboard;
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
mod surface;
use resources::{SceneRecord, TextureRecord};

/// Prepared resources are concrete native records, never erased objects or callbacks.
type Prepared = PreparedFrame<Rc<TextureRecord>, MeshSubmission>;

#[derive(Default)]
#[com_object(neocogi_platform_contracts::Platform)]
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
    /// Retains the candidate first, then publishes its identity after successful upload.
    fn prepare_atlas(&mut self, source: ComRef<'_, IRgbaImageAbi>) -> Result<(), String> {
        if self.atlas.as_raw().object == source.as_raw().object {
            return Ok(());
        }
        // The image projection deliberately carries no fonts, names, theme records, or catalog.
        // Those higher-level meanings have already been resolved before renderer submission.
        let retained = source.try_add_ref().map_err(|status| status.to_string())?;
        let pixels = AtlasPixels::read(source).map_err(|status| status.to_string())?;
        self.gpu.replace_atlas(pixels.image())?;
        self.atlas = retained;
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
#[com_object(interfaces(
    IPlatformWindow,
    ITextClipboard,
    IRenderer,
    ITextureFactory,
    IRenderSurfaceFactory,
    IMeshScenes,
    IRenderDiagnostics
))]
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
    Ok(identity.as_raw().object as usize)
}

#[cfg(feature = "glow")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.render.glow")]
fn acquire_root(_runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_platform_contracts::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::default()).query()
}
#[cfg(feature = "vulkan")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.render.vulkan")]
fn acquire_root(_runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_platform_contracts::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::default()).query()
}
#[cfg(feature = "wgpu")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.render.wgpu")]
fn acquire_root(_runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_platform_contracts::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::default()).query()
}
#[cfg(feature = "webgl")]
/// Acquires one owned root; object construction remains on its typed methods.
#[neocogi_com::plugin(name = "neocogi.render.webgl")]
fn acquire_root(_runtime: ::neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_platform_contracts::PlatformAbi>, neocogi_com::NcStatus> {
    PlatformObject::from_value(PlatformObject::default()).query()
}
// Only the native assembly owns the dynamic entrypoint; browser assembly uses static descriptors.

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
