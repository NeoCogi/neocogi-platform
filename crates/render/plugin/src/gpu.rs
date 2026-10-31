//! Native GPU execution remains local to this plugin; only construction differs between selected backends.
use super::*;
use neocogi_render_core::{Color, FrameInfo, RendererBackend, RendererFrame, prelude::Dimensioni};

#[cfg(feature = "glow")]
pub(super) use neocogi_render_glow::{Backend, Core, GLRenderer as Renderer};
#[cfg(feature = "vulkan")]
pub(super) use neocogi_render_vulkan::{Backend, VulkanRenderer as Renderer};
#[cfg(feature = "wgpu")]
pub(super) use neocogi_render_wgpu::{Backend, WgpuRenderer as Renderer};
#[cfg(feature = "webgl")]
pub(super) use neocogi_render_webgl::{Backend, Core, WebGlCanvasRenderer as Renderer};
/// Native immutable image selected with its statically typed resource family.
pub(super) type Image = <Renderer as RendererBackend>::Image;
/// Consumed private output target; cannot be reused after publication.
type Output = <Renderer as RendererBackend>::Output;
/// Concrete drawable owner selected at compilation, never a native-handle escape through COM.
#[cfg(not(target_arch = "wasm32"))]
pub(super) type Window = sdl2::video::Window;
/// Browser objects stay in the statically assembled platform implementation.
#[cfg(target_arch = "wasm32")]
pub(super) type Window = web_sys::HtmlCanvasElement;

/// Concrete native owner. Its field order destroys all UI/application GPU resources before the surface.
pub(super) struct Gpu {
    /// UI and generic client graphics execute on this single concrete backend.
    renderer: Renderer,
    /// Shared loader is used only to check GL execution errors before presentation.
    #[cfg(any(feature = "glow", feature = "webgl"))]
    gl: std::rc::Rc<glow::Context>,
    /// Test-only readback captures completed GPU output before the window swap.
    #[cfg(all(test, feature = "glow"))]
    last_pixels: Vec<u8>,
    /// Surface outlives both backend resources and the optional GL context.
    window: Window,
}

impl Gpu {
    /// Test-only cost sample separates native preparation from completed execution;
    /// it bypasses acceptance readback and presentation and adds no deployed telemetry.
    #[cfg(all(test, feature = "glow", not(target_arch = "wasm32")))]
    pub(super) fn measure(&mut self, blocks: &[std::rc::Rc<neocogi_render_gpu::record::Block<Backend>>], sample: super::tests::costs::Sample) {
        use glow::HasContext;
        use std::time::Instant;
        let super::tests::costs::Sample {
            setup_ns,
            record_ns,
            mixed,
            commands,
            draws,
            buffers,
            textures,
            pipelines,
        } = sample;
        self.current().unwrap();
        // SAFETY: The live owning GL context is selected throughout this measurement.
        let adapter = unsafe { self.gl.get_parameter_string(glow::RENDERER) };
        let setup_calls = super::tests::costs::calls::take();
        let mut cold = (0, 0);
        let mut cold_calls = super::tests::costs::calls::Calls::default();
        let mut warm = (0, 0);
        for frame in 0..=100 {
            let start = Instant::now();
            let prepared = blocks
                .iter()
                .map(|block| self.renderer.prepare_block(block, false).unwrap())
                .collect::<Vec<_>>();
            let preparation = start.elapsed().as_nanos();
            let start = Instant::now();
            let mut target = self
                .renderer
                .frame(FrameInfo::try_new(Dimensioni::new(64, 64), Color { r: 0, g: 0, b: 0, a: 255 }).unwrap())
                .unwrap();
            for block in &prepared {
                target.execute_block(block);
            }
            target.finish().unwrap();
            // SAFETY: Finish completes commands from this test's current context.
            unsafe {
                self.gl.finish();
            }
            let execution = start.elapsed().as_nanos();
            if frame == 0 {
                cold = (preparation, execution);
                cold_calls = super::tests::costs::calls::take();
            } else {
                warm.0 += preparation;
                warm.1 += execution;
            }
        }
        let warm_calls = super::tests::costs::calls::take();
        println!("native_calls setup={setup_calls:?} cold={cold_calls:?} warm_total={warm_calls:?}");
        println!(
            "native_cost workload={} adapter={adapter:?} size=64x64 setup_ns={setup_ns} ui_record_ns={record_ns} commands={commands} draws={draws} buffer_versions={buffers} texture_versions={textures} pipeline_versions={pipelines} cold_prepare_ns={} cold_completed_execute_ns={} warm_frames=100 warm_prepare_ns={} warm_completed_execute_ns={} reused_immutable_versions=true",
            if mixed { "ui+client" } else { "ui" },
            cold.0,
            cold.1,
            warm.0 / 100,
            warm.1 / 100
        );
    }
    /// Creates the chosen native window and backend in one fallible transaction.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn new(video: &sdl2::VideoSubsystem, title: &str, size: Size) -> Result<Self, String> {
        #[cfg(feature = "glow")]
        {
            let attributes = video.gl_attr();
            attributes.set_context_profile(sdl2::video::GLProfile::GLES);
            attributes.set_context_version(3, 0);
            attributes.set_depth_size(24);
            attributes.set_share_with_current_context(false);
        }
        let mut builder = video.window(title, size.width as u32, size.height as u32);
        builder.resizable().allow_highdpi();
        #[cfg(feature = "glow")]
        builder.opengl();
        #[cfg(all(feature = "wgpu", target_os = "macos"))]
        builder.metal_view();
        #[cfg(all(not(feature = "glow"), feature = "vulkan"))]
        builder.vulkan();
        let window = builder.build().map_err(|e| e.to_string())?;
        let actual = pixel_size(&window);
        #[cfg(feature = "glow")]
        {
            let context = window.gl_create_context()?;
            window.gl_make_current(&context)?;
            // SAFETY: SDL selected this live context, and the loader resolves functions for it.
            let gl = std::rc::Rc::new(unsafe {
                glow::Context::from_loader_function(|name| {
                    let pointer = video.gl_get_proc_address(name) as *const _;
                    #[cfg(test)]
                    let pointer = super::tests::costs::calls::load(name, pointer);
                    pointer
                })
            });
            // SAFETY: Dispatch was loaded for this exact context, whose unique owner moves into Core.
            let core = unsafe { Core::from_sdl(gl.clone(), window.clone(), context) }?;
            let renderer = Renderer::new(core, actual.width as u32, actual.height as u32)?;
            Ok(Self {
                renderer,
                gl,
                window,
                #[cfg(test)]
                last_pixels: Vec::new(),
            })
        }
        #[cfg(not(feature = "glow"))]
        {
            let renderer = Renderer::new(&window, actual.width as u32, actual.height as u32).map_err(|e| e.to_string())?;
            Ok(Self { renderer, window })
        }
    }
    /// Creates the browser renderer using the same common graphics execution path.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn from_canvas(window: Window) -> Result<Self, String> {
        use wasm_bindgen::JsCast;
        let webgl = window
            .get_context("webgl2")
            .map_err(|error| format!("{error:?}"))?
            .ok_or("WebGL 2 is unavailable")?
            .dyn_into::<web_sys::WebGl2RenderingContext>()
            .map_err(|_| "canvas context is not WebGL 2")?;
        let core = Core::from_canvas(window.clone(), webgl)?;
        let gl = core.dispatch().clone();
        let renderer = Renderer::new(core, window.width(), window.height())?;
        Ok(Self { renderer, gl, window })
    }
    /// SDL's cheap owned clone keeps polling available while render state is temporarily detached.
    pub(super) fn window(&self) -> Window {
        self.window.clone()
    }
    /// Selects this GL context before any native resource operation; explicit APIs need no global selection.
    fn current(&self) -> Result<(), String> {
        #[cfg(any(feature = "glow", feature = "webgl"))]
        self.renderer.make_current()?;
        Ok(())
    }

    /// Runs completion/retirement maintenance on the selected device before CPU quota admission.
    pub(super) fn maintain(&mut self) -> Result<(), String> {
        self.current()?;
        self.renderer.maintain().map_err(|error| error.to_string())
    }
    /// Shares physical device lifetime without retaining this driver or its queues.
    pub(super) fn core(&self) -> std::rc::Rc<<Backend as neocogi_render_gpu::native::NativeResources>::Core> {
        self.renderer.core()
    }
    /// Atlas replacement uses the common immutable image realization before publishing UI metadata.
    pub(super) fn prepare_texture(&mut self, texture: std::rc::Rc<neocogi_render_gpu::resource::TextureVersion<Backend>>) -> Result<(), String> {
        self.current()?;
        self.renderer.prepare_texture(texture)
    }
    /// Executes only validated owned commands; no foreign method is invoked while a frame is acquired.
    pub(super) fn render(&mut self, info: RenderInfo, prepared: &Prepared) -> Result<(), String> {
        self.render_target(info, prepared, None).map(|_| ())
    }

    /// Reports surface availability before accepting an independent rendering session.
    pub(super) fn supports_surfaces(&self) -> bool {
        cfg!(any(feature = "glow", feature = "webgl", feature = "wgpu", feature = "vulkan"))
    }

    /// A fresh destination is consumed before a completed immutable image can be published.
    pub(super) fn render_surface(&mut self, info: RenderInfo, prepared: &Prepared) -> Result<Image, String> {
        self.current()?;
        let output = self.renderer.create_render_texture(info.size).map_err(|error| error.to_string())?;
        self.render_target(info, prepared, Some(output))?
            .ok_or_else(|| "offscreen frame returned no image".into())
    }

    /// Shares validated command execution while keeping target acquisition and presentation explicit.
    fn render_target(&mut self, info: RenderInfo, prepared: &Prepared, target: Option<Output>) -> Result<Option<Image>, String> {
        self.current()?;
        let [r, g, b, a] = info.clear.to_le_bytes();
        let dimensions = Dimensioni::new(info.size.width, info.size.height);
        let frame_info = FrameInfo::try_new(dimensions, Color { r, g, b, a }).map_err(|e| e.to_string())?;
        // Resource compilation/upload finishes before any target acquire or attachment clear.
        let blocks = prepared
            .blocks()
            .iter()
            .map(|block| self.renderer.prepare_block(block, target.is_some()))
            .collect::<Result<Vec<_>, String>>()?;
        #[cfg(all(not(target_arch = "wasm32"), feature = "glow"))]
        let offscreen = target.is_some();
        let mut frame = match target {
            None => self.renderer.frame(frame_info),
            Some(output) => self.renderer.texture_frame(frame_info, output),
        }
        .map_err(|error| error.to_string())?;
        for block in &blocks {
            frame.execute_block(block);
        }
        // Drop abandons; consuming finish alone submits. Vulkan/WGPU also present inside their finish.
        let completed = frame.finish().map_err(|e| e.to_string())?;
        #[cfg(any(feature = "glow", feature = "webgl"))]
        {
            use glow::HasContext;
            // SAFETY: The same live GL context remains current throughout the foreign-call-free execution phase.
            let error = unsafe { self.gl.get_error() };
            if error != glow::NO_ERROR {
                return Err(format!("OpenGL frame failed with error 0x{error:04x}"));
            }
            #[cfg(all(test, feature = "glow"))]
            if !offscreen {
                self.last_pixels.resize(info.size.width as usize * info.size.height as usize * 4, 0);
                // SAFETY: The complete RGBA output is allocated, and the live native context remains current.
                unsafe {
                    self.gl.read_pixels(
                        0,
                        0,
                        info.size.width,
                        info.size.height,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelPackData::Slice(Some(&mut self.last_pixels)),
                    );
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            if !offscreen {
                self.window.gl_swap_window();
            }
        }
        Ok(completed)
    }
    /// Completed GPU output is available only to native acceptance tests, never through a production ABI.
    #[cfg(all(test, feature = "glow"))]
    pub(super) fn last_pixels(&self) -> &[u8] {
        &self.last_pixels
    }
    /// Creates an immutable initialized image whose native backing owns its retirement sender.
    pub(super) fn create_texture(&mut self, size: Size, pixels: Option<&[u8]>) -> Result<Image, String> {
        self.current()?;
        self.renderer.create_texture(size, pixels).map_err(|error| error.to_string())
    }
}

impl Drop for Gpu {
    /// Selects the owning context before field destruction; destruction never submits or presents.
    fn drop(&mut self) {
        let _ = self.current();
    }
}

/// SDL 2.26+ provides physical pixel dimensions independently of the selected native graphics API.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn pixel_size(window: &Window) -> Size {
    let (mut width, mut height) = (0, 0);
    // SAFETY: This owned live window and both writable outputs remain valid for the synchronous SDL call.
    unsafe {
        sdl2::sys::SDL_GetWindowSizeInPixels(window.raw(), &mut width, &mut height);
    }
    Size { width, height }
}
