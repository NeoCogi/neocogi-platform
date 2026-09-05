//! Native SDL platform assembly with one build-selected Glow, Vulkan or WGPU backend.
//! Native windows, GPU resources and application mesh rendering remain in this binary.
#![deny(missing_docs)]
use microui_contracts::*;
use microui_demo_contracts::*;
use microui_redux_demo_renderer::{MeshBuffers, MeshSubmission};
use microui_redux_renderer_support::com::{PreparedFrame, Resources};
use microui_ui::TextureId;
use nc_com::{ComInterface, ComPtr, ComRef, IUnknown, INcStringAbi, NcString, NcCopyFromBufferOffset, NcStatus, NC_E_INVALID_ARG, com_class};
use sdl2::event::Event;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap, VecDeque},
    rc::{Rc, Weak},
};
mod gpu;
mod input;
mod resources;
use resources::{SceneRecord, TextureRecord};

/// Prepared resources are concrete native records, never erased objects or callbacks.
type Prepared = PreparedFrame<Rc<TextureRecord>, MeshSubmission>;

#[derive(Default)]
#[com_class(class_id(PLATFORM_CLASS), name = "microui.Platform", version = "0.9.0-alpha.0",
    constructor(args = none, default), interfaces(IPlatform, IRenderDiagnostics))]
/// Native bootstrap; initialization is fallible and no native subsystem crosses the COM boundary.
pub struct Platform {
    /// One SDL event pump shared by this platform's windows.
    host: RefCell<Option<Rc<NativeHost>>>,
    /// Owned detail from the most recent failed native construction.
    diagnostic: RefCell<String>,
}

/// Native host lifetime and per-window event queues, owned entirely within this binary.
struct NativeHost {
    /// Pending events are routed by SDL window identity rather than consumed by another window.
    queues: RefCell<BTreeMap<u32, VecDeque<Event>>>,
    /// SDL permits one event pump; shared interior access rejects reentrant polling.
    pump: RefCell<sdl2::EventPump>,
    /// Video subsystem remains alive until all windows have been destroyed.
    video: sdl2::VideoSubsystem,
    /// Root SDL guard is dropped after the pump and video subsystem.
    _sdl: sdl2::Sdl,
}

impl NativeHost {
    /// Initializes native services once for this platform instance.
    fn new() -> Result<Rc<Self>, String> {
        let sdl = sdl2::init()?;
        let video = sdl.video()?;
        let pump = sdl.event_pump()?;
        Ok(Rc::new(Self {
            queues: RefCell::default(),
            pump: RefCell::new(pump),
            video,
            _sdl: sdl,
        }))
    }
    /// Routes native events to their own windows; global quit is broadcast to every live window.
    fn poll(&self, id: u32) -> Result<Option<Event>, NcStatus> {
        let mut queues = self.queues.try_borrow_mut().map_err(|_| UI_E_BUSY)?;
        for event in self.pump.try_borrow_mut().map_err(|_| UI_E_BUSY)?.poll_iter() {
            if matches!(event, Event::Quit { .. }) {
                for queue in queues.values_mut() {
                    queue.push_back(event.clone());
                }
            } else if let Some(queue) = event.get_window_id().and_then(|window| queues.get_mut(&window)) {
                if matches!(event, Event::MouseMotion { .. }) && matches!(queue.back(), Some(Event::MouseMotion { .. })) {
                    queue.pop_back();
                }
                queue.push_back(event);
            }
        }
        Ok(queues.get_mut(&id).and_then(VecDeque::pop_front))
    }
}

impl IPlatform for Platform {
    /// Validates input and publishes one complete native object only after the selected backend exists.
    fn create_window(
        &self,
        title: NcCopyFromBufferOffset<'_, u8>,
        size: Size,
        assets: ComRef<'_, IAssetBundleAbi>,
    ) -> Result<ComPtr<IPlatformWindowAbi>, NcStatus> {
        if title.offset() != 0 || size.width <= 0 || size.height <= 0 {
            return Err(NC_E_INVALID_ARG);
        }
        let title = std::str::from_utf8(title.as_slice()).map_err(|_| NC_E_INVALID_ARG)?;
        Ok(NativeWindow::from_value(self.window(title, size, assets)?)
            .query()
            .expect("native window declares IPlatformWindow"))
    }
}
impl Platform {
    /// Constructs the concrete native owner used by COM activation and native lifetime acceptance tests.
    fn window(&self, title: &str, size: Size, source: ComRef<'_, IAssetBundleAbi>) -> Result<NativeWindow, NcStatus> {
        // Validate all foreign metadata before creating a native window or borrowing SDL ownership.
        let assets = microui_ui::assets::ImportedAssets::read(source, None)?;
        let mut owner = self.host.try_borrow_mut().map_err(|_| UI_E_BUSY)?;
        if owner.is_none() {
            *owner = Some(NativeHost::new().map_err(|error| {
                *self.diagnostic.borrow_mut() = error;
                UI_E_RENDER
            })?);
        }
        let host = owner.as_ref().expect("host initialized").clone();
        drop(owner);
        let gpu = gpu::Gpu::new(&host.video, title, size, assets.bundle().atlas().clone()).map_err(|error| {
            *self.diagnostic.borrow_mut() = error;
            UI_E_RENDER
        })?;
        let window = gpu.window();
        host.queues.borrow_mut().insert(window.id(), VecDeque::new());
        let device = Rc::new(Device {
            render: RefCell::new(Some(RenderState {
                gpu,
                assets,
                prepared: Prepared::default(),
            })),
            textures: RefCell::default(),
            scenes: RefCell::default(),
            retired: RefCell::default(),
            diagnostic: RefCell::default(),
            window,
            host,
        });
        Ok(NativeWindow { device, input: RefCell::default(), polling: Cell::new(false) })
    }
}
impl IRenderDiagnostics for Platform {
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
    /// Current immutable resource revision; a same-pointer preparation performs no copies or upload.
    assets: microui_ui::assets::ImportedAssets,
}
impl RenderState {
    /// Builds and uploads a complete candidate before replacing the current resource reference.
    fn prepare_assets(&mut self, source: ComRef<'_, IAssetBundleAbi>) -> Result<(), String> {
        if self.assets.source().as_ptr() == source.as_ptr() {
            return Ok(());
        }
        // Renderers may serve independent UIs, so a new catalog is valid here. Only UI replacement
        // preserves application references and therefore requires the original catalog identity.
        let next = microui_ui::assets::ImportedAssets::read(source, None).map_err(|status| status.to_string())?;
        self.gpu.replace_atlas(next.bundle().atlas().clone())?;
        self.assets = next;
        Ok(())
    }
}

/// Shared native lifetime retained by the window, every image and every editable scene.
struct Device {
    /// None throughout preparation and submission; reentrant GPU operations receive UI_E_BUSY.
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
    window: sdl2::video::Window,
    /// SDL subsystems outlive the backend and every native window clone.
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
        let state = self.render.try_borrow_mut().map_err(|_| UI_E_BUSY)?.take().ok_or(UI_E_BUSY)?;
        Ok(RenderAccess { device: self, state: Some(state) })
    }
    /// Preserves native backend failure text rather than erasing it into a status alone.
    fn failure(&self, error: impl std::fmt::Display) -> NcStatus {
        *self.diagnostic.borrow_mut() = error.to_string();
        UI_E_RENDER
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
impl Drop for Device {
    /// Removes event routing only when all resources have relinquished the native owner.
    fn drop(&mut self) {
        self.host.queues.borrow_mut().remove(&self.window.id());
    }
}

#[com_class(interfaces(IPlatformWindow, IRenderer, ITextureFactory, IDemoScenes, IRenderDiagnostics))]
/// One COM identity exposes OS polling, rendering and concrete resource factories.
struct NativeWindow {
    /// Resources retain this same native owner after the public window interface is released.
    device: Rc<Device>,
    /// Fractional wheel input belongs to this window and is reset on focus cancellation.
    input: RefCell<input::InputState>,
    /// Rejects recursive delivery even after native and queue borrows have been released.
    polling: Cell<bool>,
}

impl IRenderer for NativeWindow {
    /// Takes owned device state so nested bundle methods cannot reenter native upload or submission.
    fn prepare_assets(&self, assets: ComRef<'_, IAssetBundleAbi>) -> Result<(), NcStatus> {
        let mut access = self.device.access()?;
        access.state().prepare_assets(assets).map_err(|error| self.device.failure(error))
    }
    /// Prepares the complete frame without borrowing native state, then executes and explicitly finishes it.
    fn render(&self, info: RenderInfo, assets: ComRef<'_, IAssetBundleAbi>, stream: ComRef<'_, IRenderStreamAbi>) -> Result<(), NcStatus> {
        if info.size != self.size()? {
            return Err(NC_E_INVALID_ARG);
        }
        let mut access = self.device.access()?;
        let state = access.state();
        state.prepared.read(info, stream, self).inspect_err(|error| {
            *self.device.diagnostic.borrow_mut() = format!("frame preparation failed: {error}");
        })?;
        // Reject malformed commands and foreign resource owners before changing GPU atlas state.
        state.prepare_assets(assets).map_err(|error| self.device.failure(error))?;
        state.gpu.render(info, &state.prepared).map_err(|error| self.device.failure(error))
    }
}
impl IRenderDiagnostics for NativeWindow {
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

nc_com::plugin! {
    plugin_id(auto_guid = "microui.plugin.platform.native"), name = "microui.plugin.platform.native", version = "0.9.0-alpha.0", classes = [Platform],
}
// Only the native assembly owns the dynamic entrypoint; browser assembly uses static descriptors.
#[cfg(feature = "dynamic-plugin")]
nc_com::export_plugin!();

#[cfg(test)]
mod tests;
