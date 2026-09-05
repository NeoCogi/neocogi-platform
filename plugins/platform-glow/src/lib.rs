//! Native SDL/Glow assembly. Native windows, GL contexts and GPU resources remain together.
#![deny(missing_docs)]
use microui_contracts::*;
use microui_ui::{
    prelude::{Dimensioni, Vec2f, color4b},
    render::{Color, FrameInfo, RendererBackend, RendererFrame, Vertex},
};
use microui_redux_renderer_glow::GLRenderer;
use nc_com::{ComPtr, ComRef, NcCopyFromBufferOffset, NcCopyToBufferOffset, NcStatus, NC_E_INVALID_ARG, com_class};
use sdl2::{
    event::{Event, WindowEvent},
    keyboard::Keycode,
    mouse::MouseButton,
    video::GLProfile,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
    sync::Arc,
};

/// Maximum accepted complete frame allocation, checked before resizing scratch.
const MAX_VERTICES: usize = 16 * 1024 * 1024;

/// Converts a platform diagnostic to the common backend status while preserving local detail.
fn failure(error: impl std::fmt::Display) -> NcStatus {
    eprintln!("platform: {error}");
    UI_E_RENDER
}

#[derive(Default)]
#[com_class(class_id(PLATFORM_CLASS), name = "microui.Platform", version = "0.9.0-alpha.0",
    constructor(args = none, default), interfaces(IPlatform))]
/// Native platform bootstrap; native initialization remains fallible and lazy.
pub struct Platform {
    /// One event pump is shared by the windows created through this platform instance.
    host: RefCell<Option<Rc<NativeHost>>>,
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
    fn new() -> Result<Rc<Self>, NcStatus> {
        let sdl = sdl2::init().map_err(failure)?;
        let video = sdl.video().map_err(failure)?;
        let pump = sdl.event_pump().map_err(failure)?;
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
                queue.push_back(event);
            }
        }
        Ok(queues.get_mut(&id).and_then(VecDeque::pop_front))
    }
}

impl IPlatform for Platform {
    /// Validates input before allocating native/GPU resources and publishes only a complete object.
    fn create_window(&self, title: NcCopyFromBufferOffset<'_, u8>, size: Size) -> Result<ComPtr<IPlatformWindowAbi>, NcStatus> {
        if title.offset() != 0 || size.width <= 0 || size.height <= 0 {
            return Err(NC_E_INVALID_ARG);
        }
        let title = std::str::from_utf8(title.as_slice()).map_err(|_| NC_E_INVALID_ARG)?;
        let mut owner = self.host.try_borrow_mut().map_err(|_| UI_E_BUSY)?;
        if owner.is_none() {
            *owner = Some(NativeHost::new()?);
        }
        let host = owner.as_ref().expect("host initialized").clone();
        drop(owner);
        let attributes = host.video.gl_attr();
        attributes.set_context_profile(GLProfile::GLES);
        attributes.set_context_version(3, 0);
        attributes.set_depth_size(24);
        let window = host
            .video
            .window(title, size.width as u32, size.height as u32)
            .resizable()
            .allow_highdpi()
            .opengl()
            .build()
            .map_err(failure)?;
        let context = window.gl_create_context().map_err(failure)?;
        window.gl_make_current(&context).map_err(failure)?;
        // SAFETY: SDL made this live GL context current; the loader resolves its native functions.
        let gl = Arc::new(unsafe { glow::Context::from_loader_function(|name| host.video.gl_get_proc_address(name) as *const _) });
        let (width, height) = window.drawable_size();
        let renderer = GLRenderer::new(gl, microui_redux_demo_assets::load_atlas(), width, height).map_err(failure)?;
        host.queues.borrow_mut().insert(window.id(), VecDeque::new());
        Ok(NativeWindow::from_value(NativeWindow {
            render: RefCell::new(RenderState { renderer, vertices: Vec::new() }),
            context,
            window,
            host,
        })
        .query()
        .expect("native window declares its interface"))
    }
}

/// Backend and reusable copied vertices share one checked mutable entry gate.
struct RenderState {
    /// Native renderer is destroyed before the GL context and window.
    renderer: GLRenderer,
    /// Backend-owned copy buffer amortizes allocations across frames.
    vertices: Vec<RenderVertex>,
}

#[com_class(interfaces(IPlatformWindow, IRenderer))]
/// One COM identity retains native window, context, renderer and their destruction order.
struct NativeWindow {
    /// Declared first so GPU resources are destroyed while context/window still exist.
    render: RefCell<RenderState>,
    /// Native GL context is dropped only after renderer destruction.
    context: sdl2::video::GLContext,
    /// Native presentation surface remains alive during GPU teardown.
    window: sdl2::video::Window,
    /// Keeps SDL subsystems alive until after the window is destroyed.
    host: Rc<NativeHost>,
}

impl NativeWindow {
    /// Scales logical SDL pointer coordinates into the drawable-pixel UI coordinate space.
    fn pointer(&self, x: i32, y: i32) -> (i32, i32) {
        let (lw, lh) = self.window.size();
        let (dw, dh) = self.window.drawable_size();
        (
            (i64::from(x) * i64::from(dw) / i64::from(lw.max(1))).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            (i64::from(y) * i64::from(dh) / i64::from(lh.max(1))).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        )
    }
}

impl IPlatformWindow for NativeWindow {
    /// Reports actual drawable size, independent of logical window scaling.
    fn size(&self) -> Result<Size, NcStatus> {
        let (width, height) = self.window.drawable_size();
        Ok(Size {
            width: i32::try_from(width).map_err(|_| NC_E_INVALID_ARG)?,
            height: i32::try_from(height).map_err(|_| NC_E_INVALID_ARG)?,
        })
    }
    /// Converts relevant events without lending SDL or Rust enum storage across COM.
    fn poll(&self) -> Result<PlatformEvent, NcStatus> {
        while let Some(event) = self.host.poll(self.window.id())? {
            let (kind, x, y) = match event {
                Event::Quit { .. } | Event::Window { win_event: WindowEvent::Close, .. } => (PlatformEventKind::QUIT, 0, 0),
                Event::Window {
                    win_event: WindowEvent::SizeChanged(..), ..
                } => {
                    let size = self.size()?;
                    (PlatformEventKind::RESIZE, size.width, size.height)
                }
                Event::MouseMotion { x, y, .. } => {
                    let (x, y) = self.pointer(x, y);
                    (PlatformEventKind::POINTER_MOVE, x, y)
                }
                Event::MouseButtonDown { mouse_btn: MouseButton::Left, x, y, .. } => {
                    let (x, y) = self.pointer(x, y);
                    (PlatformEventKind::POINTER_DOWN, x, y)
                }
                Event::MouseButtonUp { mouse_btn: MouseButton::Left, x, y, .. } => {
                    let (x, y) = self.pointer(x, y);
                    (PlatformEventKind::POINTER_UP, x, y)
                }
                Event::KeyDown {
                    keycode: Some(Keycode::Return | Keycode::Space),
                    repeat: false,
                    ..
                } => (PlatformEventKind::ACTIVATE, 0, 0),
                Event::KeyDown {
                    keycode: Some(Keycode::Tab),
                    repeat: false,
                    ..
                } => (PlatformEventKind::TAB, 0, 0),
                _ => continue,
            };
            return Ok(PlatformEvent { kind, x, y });
        }
        Ok(PlatformEvent {
            kind: PlatformEventKind::NONE,
            x: 0,
            y: 0,
        })
    }
}

impl IRenderer for NativeWindow {
    /// Validates/copies the complete frame before issuing GPU commands, then explicitly finishes.
    fn render(&self, info: RenderInfo, stream: ComRef<'_, IRenderStreamAbi>) -> Result<(), NcStatus> {
        if info.size != self.size()?
            || info.size.width <= 0
            || info.size.height <= 0
            || !info.vertex_count.is_multiple_of(3)
            || info.vertex_count as usize > MAX_VERTICES
        {
            return Err(NC_E_INVALID_ARG);
        }
        let mut state = self.render.try_borrow_mut().map_err(|_| UI_E_BUSY)?;
        let RenderState { renderer, vertices } = &mut *state;
        vertices.resize(info.vertex_count as usize, RenderVertex::default());
        let copied = stream.vertices(info.invocation, NcCopyToBufferOffset::new(0, vertices))?;
        if copied != info.vertex_count
            || vertices
                .iter()
                .any(|v| !v.x.is_finite() || !v.y.is_finite() || !v.u.is_finite() || !v.v.is_finite())
        {
            return Err(NC_E_INVALID_ARG);
        }
        self.window.gl_make_current(&self.context).map_err(failure)?;
        let [r, g, b, a] = info.clear.to_le_bytes();
        let frame_info = FrameInfo::try_new(Dimensioni::new(info.size.width, info.size.height), Color { r, g, b, a }).map_err(failure)?;
        let mut frame = renderer.frame(frame_info).map_err(failure)?;
        for triangle in vertices.as_chunks::<3>().0 {
            frame.push_triangle(triangle.map(vertex));
        }
        frame.finish().map_err(failure)?;
        self.window.gl_swap_window();
        Ok(())
    }
}

/// Converts one already-validated ABI vertex to the private native backend value.
fn vertex(value: RenderVertex) -> Vertex {
    let [r, g, b, a] = value.rgba.to_le_bytes();
    Vertex::new(Vec2f::new(value.x, value.y), Vec2f::new(value.u, value.v), color4b(r, g, b, a))
}

impl Drop for NativeWindow {
    /// Selects the correct context before fields destroy GPU resources; never presents on release.
    fn drop(&mut self) {
        let _ = self.window.gl_make_current(&self.context);
        self.host.queues.borrow_mut().remove(&self.window.id());
    }
}

nc_com::plugin! {
    plugin_id(auto_guid = "microui.plugin.platform.glow"),
    name = "microui.plugin.platform.glow",
    version = "0.9.0-alpha.0",
    classes = [Platform],
}

// Only the selected native assembly owns a dynamic entrypoint.
#[cfg(feature = "dynamic-plugin")]
nc_com::export_plugin!();
