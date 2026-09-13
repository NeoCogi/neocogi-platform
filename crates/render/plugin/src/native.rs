//! SDL subsystem initialization and native event routing remain local to this assembly.
use super::*;
use std::collections::{BTreeMap, VecDeque};
use sdl2::event::Event;

/// Native host lifetime and per-window event queues, owned entirely within this binary.
pub(super) struct NativeHost {
    /// Pending events are routed by SDL window identity rather than consumed by another window.
    pub(super) queues: RefCell<BTreeMap<u32, VecDeque<Event>>>,
    /// SDL permits one event pump; shared interior access rejects reentrant polling.
    pump: RefCell<sdl2::EventPump>,
    /// Video subsystem remains alive until all windows have been destroyed.
    pub(super) video: sdl2::VideoSubsystem,
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
    pub(super) fn poll(&self, id: u32) -> Result<Option<Event>, NcStatus> {
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

impl PlatformObject {
    /// Constructs the concrete native owner used by COM activation and native lifetime acceptance tests.
    pub(super) fn window(&self, title: &str, size: Size, source: ComRef<'_, IAssetBundleAbi>) -> Result<WindowObject, NcStatus> {
        // Validate pixel storage before creating a native window or borrowing SDL ownership.
        let atlas = source.add_ref().query::<IRgbaImageAbi>()?;
        let pixels = AtlasPixels::read(atlas.as_com_ref())?;
        let mut owner = self.host.try_borrow_mut().map_err(|_| UI_E_BUSY)?;
        if owner.is_none() {
            *owner = Some(NativeHost::new().map_err(|error| {
                *self.diagnostic.borrow_mut() = error;
                UI_E_RENDER
            })?);
        }
        let host = owner.as_ref().expect("host initialized").clone();
        drop(owner);
        let gpu = gpu::Gpu::new(&host.video, title, size, pixels.image()).map_err(|error| {
            *self.diagnostic.borrow_mut() = error;
            UI_E_RENDER
        })?;
        let window = gpu.window();
        host.queues.borrow_mut().insert(window.id(), VecDeque::new());
        let device = Rc::new(Device {
            render: RefCell::new(Some(RenderState {
                gpu,
                atlas,
                prepared: Prepared::default(),
            })),
            textures: RefCell::default(),
            scenes: RefCell::default(),
            retired: RefCell::default(),
            diagnostic: RefCell::default(),
            window,
            host,
        });
        Ok(WindowObject {
            device,
            input: RefCell::default(),
            polling: Cell::new(false),
        })
    }
}
impl Drop for Device {
    /// Removes event routing only when all resources have relinquished the native owner.
    fn drop(&mut self) {
        self.host.queues.borrow_mut().remove(&self.window.id());
    }
}
