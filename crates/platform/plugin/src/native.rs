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
        let mut queues = self.queues.try_borrow_mut().map_err(|_| PLATFORM_E_BUSY)?;
        for event in self.pump.try_borrow_mut().map_err(|_| PLATFORM_E_BUSY)?.poll_iter() {
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
    pub(super) fn window(&self, title: &str, size: Size) -> Result<WindowObject, NcStatus> {
        let mut owner = self.host.try_borrow_mut().map_err(|_| PLATFORM_E_BUSY)?;
        if owner.is_none() {
            *owner = Some(NativeHost::new().map_err(|error| {
                *self.diagnostic.borrow_mut() = error;
                PLATFORM_E_BACKEND
            })?);
        }
        let host = owner.as_ref().expect("host initialized").clone();
        drop(owner);
        let gpu = gpu::Gpu::new(&host.video, title, size).map_err(|error| {
            *self.diagnostic.borrow_mut() = error;
            PLATFORM_E_BACKEND
        })?;
        let window = gpu.window();
        host.queues.borrow_mut().insert(window.id(), VecDeque::new());
        let device = Rc::new(Device {
            logical: neocogi_render_gpu::GpuDevice::new(gpu.core(), self.compiler.clone()),
            render: RefCell::new(Some(RenderState { gpu, prepared: Prepared::default() })),
            textures: Rc::default(),
            surfaces: RefCell::default(),
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
impl Drop for WindowObject {
    /// Input routing ends with the public window; graphics-only owners retain just native lifetime.
    fn drop(&mut self) {
        self.device.host.queues.borrow_mut().remove(&self.device.window.id());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Invalid envelope values reject before SDL/window/device construction.
    #[test]
    fn invalid_window_requests_do_not_initialize_native_services() {
        let local = PlatformObject::into_local(PlatformObject::new(
            nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
                .query()
                .unwrap(),
        ));
        let platform = local.query::<PlatformAbi>().unwrap();
        for size in [Size { width: 0, height: 8 }, Size { width: 8, height: -1 }] {
            assert_eq!(
                platform.create_window(NcCopyFromBufferOffset::new(0, b"invalid"), size).unwrap_err(),
                NC_E_INVALID_ARG
            );
        }
        assert_eq!(
            platform
                .create_window(NcCopyFromBufferOffset::new(0, &[0xff]), Size { width: 8, height: 8 })
                .unwrap_err(),
            NC_E_INVALID_ARG
        );
        assert!(local.try_borrow().unwrap().host.borrow().is_none());
    }
}
