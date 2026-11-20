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
    /// Windows clipboard projections share one lazy private owner while any projection is live.
    #[cfg(windows)]
    clipboard_window: RefCell<Weak<gpu::Window>>,
    /// The host shares one selection service while retaining no clipboard projection itself.
    #[cfg(target_os = "linux")]
    x11_clipboard: RefCell<Weak<clipboard::X11Clipboard>>,
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
            #[cfg(windows)]
            clipboard_window: RefCell::default(),
            #[cfg(target_os = "linux")]
            x11_clipboard: RefCell::default(),
            video,
            _sdl: sdl,
        }))
    }

    /// Shares clipboard authority for the captured X11 display without retaining the SDL host.
    #[cfg(target_os = "linux")]
    pub(super) fn x11_clipboard(&self, window: &gpu::Window) -> Result<Rc<clipboard::X11Clipboard>, NcStatus> {
        if let Some(service) = self.x11_clipboard.borrow().upgrade() {
            return Ok(service);
        }
        let service = Rc::new(clipboard::X11Clipboard::new(x11_display_name(window)?));
        self.x11_clipboard.replace(Rc::downgrade(&service));
        Ok(service)
    }

    /// SDL2 uses a live HWND when publishing clipboard data. This hidden owner
    /// keeps that service usable without keeping a graphics window or input route.
    #[cfg(windows)]
    pub(super) fn clipboard_window(&self) -> Result<Rc<gpu::Window>, NcStatus> {
        if let Some(owner) = self.clipboard_window.borrow().upgrade() {
            return Ok(owner);
        }
        let window = self.video.window("NeoCogi clipboard", 1, 1).hidden().build().map_err(|_| PLATFORM_E_BACKEND)?;
        let owner = Rc::new(window);
        self.clipboard_window.replace(Rc::downgrade(&owner));
        Ok(owner)
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

/// Copy the display name of SDL's actual X11 connection. SDL video may have already
/// been opened by the embedding application before its current DISPLAY was set.
#[cfg(target_os = "linux")]
fn x11_display_name(window: &gpu::Window) -> Result<String, NcStatus> {
    use sdl2::sys;
    use std::{
        ffi::{CStr, c_char},
        ptr::NonNull,
    };

    // SAFETY: Zero is SDL_SYSWM_UNKNOWN and valid for every other field of this C structure.
    let mut info: sys::SDL_SysWMinfo = unsafe { std::mem::zeroed() };
    // SAFETY: The version and structure are writable. This live SDL window owns the returned display.
    let display = unsafe {
        sys::SDL_GetVersion(&mut info.version);
        if sys::SDL_GetWindowWMInfo(window.raw(), &mut info) != sys::SDL_bool::SDL_TRUE || info.subsystem != sys::SDL_SYSWM_TYPE::SDL_SYSWM_X11 {
            return Err(PLATFORM_E_BACKEND);
        }
        NonNull::new(info.info.x11.display).ok_or(PLATFORM_E_BACKEND)?
    };
    // SAFETY: SDL has already loaded the X11 client ABI for this connection. The extra
    // module reference pins XDisplayString through the call, without a link-time X11 dependency.
    unsafe {
        let library = NonNull::new(sys::SDL_LoadObject(c"libX11.so.6".as_ptr())).ok_or(PLATFORM_E_BACKEND)?;
        let symbol = sys::SDL_LoadFunction(library.as_ptr(), c"XDisplayString".as_ptr());
        let name = if symbol.is_null() {
            Err(PLATFORM_E_BACKEND)
        } else {
            // Xlib's XDisplayString takes this borrowed live Display and returns its C string.
            let display_string: unsafe extern "C" fn(*mut sys::Display) -> *const c_char = std::mem::transmute(symbol);
            let name = display_string(display.as_ptr());
            if name.is_null() {
                Err(PLATFORM_E_BACKEND)
            } else {
                CStr::from_ptr(name).to_str().map(str::to_owned).map_err(|_| PLATFORM_E_BACKEND)
            }
        };
        sys::SDL_UnloadObject(library.as_ptr());
        name
    }
}

impl PlatformObject {
    /// Constructs the concrete native owner used by COM activation and native lifetime acceptance tests.
    pub(super) fn window(&self, title: &str, size: Size) -> Result<WindowObject, RenderError> {
        let mut owner = self.host.try_borrow_mut().map_err(|_| PLATFORM_E_BUSY)?;
        if owner.is_none() {
            *owner = Some(NativeHost::new().map_err(|error| RenderError::new(PLATFORM_E_BACKEND, error))?);
        }
        let host = owner.as_ref().expect("host initialized").clone();
        drop(owner);
        let gpu = gpu::Gpu::new(&host.video, title, size).map_err(|error| RenderError::new(PLATFORM_E_BACKEND, error))?;
        let window = gpu.window();
        // Compiler composition must succeed before this window gains an input route.
        let logical = neocogi_platform::gpu::GpuDevice::new(gpu.core())?;
        host.queues.borrow_mut().insert(window.id(), VecDeque::new());
        let device = Rc::new(Device {
            success: operation_result(Ok(())),
            logical,
            #[cfg(feature = "vulkan")]
            drawable: gpu.drawable(),
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
        let local = PlatformObject::into_local(PlatformObject::new());
        let platform = local.query::<PlatformAbi>().unwrap();
        for size in [Size { width: 0, height: 8 }, Size { width: 8, height: -1 }] {
            assert_eq!(
                platform
                    .create_window(NcCopyFromBufferOffset::new(0, b"invalid"), size)
                    .map_err(|error| error.status)
                    .unwrap_err(),
                NC_E_INVALID_ARG
            );
        }
        assert_eq!(
            platform
                .create_window(NcCopyFromBufferOffset::new(0, &[0xff]), Size { width: 8, height: 8 })
                .map_err(|error| error.status)
                .unwrap_err(),
            NC_E_INVALID_ARG
        );
        assert!(local.try_borrow().unwrap().host.borrow().is_none());
    }
}
