//! Native GPU execution remains local to this plugin; only construction differs between selected backends.
use super::*;
use microui_ui::{
    AtlasHandle, TextureId,
    prelude::{Dimensioni, Recti, Vec2f, color4b},
    render::{Color, CustomRenderArgs, FrameInfo, RendererBackend, RendererFrame, Vertex},
};
use microui_redux_demo_renderer::{DemoCommand, DemoRenderer};
use microui_redux_renderer_support::com::Batch;

#[cfg(feature = "glow")]
use microui_redux_renderer_glow::GLRenderer as Renderer;
#[cfg(all(not(feature = "glow"), feature = "vulkan"))]
use microui_redux_renderer_vulkan::VulkanRenderer as Renderer;
#[cfg(all(not(feature = "glow"), not(feature = "vulkan"), feature = "wgpu"))]
use microui_redux_renderer_wgpu::WgpuRenderer as Renderer;
#[cfg(not(any(feature = "glow", feature = "vulkan", feature = "wgpu")))]
compile_error!("select glow, vulkan, or wgpu for the native platform plugin");

/// Concrete native owner. Its field order destroys all UI/application GPU resources before the surface.
pub(super) struct Gpu {
    /// Both UI drawing and the demo's native depth-tested mesh renderer live in this concrete backend.
    renderer: Renderer<DemoRenderer>,
    /// Shared loader is used only to check GL execution errors before presentation.
    #[cfg(feature = "glow")]
    gl: std::sync::Arc<glow::Context>,
    /// GL context outlives renderer destruction, including its application pipelines and buffers.
    #[cfg(feature = "glow")]
    context: sdl2::video::GLContext,
    /// Test-only readback captures completed GPU output before the window swap.
    #[cfg(all(test, feature = "glow"))]
    last_pixels: Vec<u8>,
    /// Surface outlives both backend resources and the optional GL context.
    window: sdl2::video::Window,
}

impl Gpu {
    /// Creates the chosen native window and backend in one fallible transaction.
    pub(super) fn new(video: &sdl2::VideoSubsystem, title: &str, size: Size, atlas: AtlasHandle) -> Result<Self, String> {
        #[cfg(feature = "glow")]
        {
            let attributes = video.gl_attr();
            attributes.set_context_profile(sdl2::video::GLProfile::GLES);
            attributes.set_context_version(3, 0);
            attributes.set_depth_size(24);
        }
        let mut builder = video.window(title, size.width as u32, size.height as u32);
        builder.resizable().allow_highdpi();
        #[cfg(feature = "glow")]
        builder.opengl();
        #[cfg(all(not(feature = "glow"), feature = "vulkan"))]
        builder.vulkan();
        let window = builder.build().map_err(|e| e.to_string())?;
        let actual = pixel_size(&window);
        #[cfg(feature = "glow")]
        {
            let context = window.gl_create_context()?;
            window.gl_make_current(&context)?;
            // SAFETY: SDL selected this live context, and the loader resolves functions for it.
            let gl = std::sync::Arc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name) as *const _) });
            let renderer = Renderer::new_with_application(gl.clone(), atlas, actual.width as u32, actual.height as u32, DemoRenderer::new())?;
            Ok(Self {
                renderer,
                gl,
                context,
                window,
                #[cfg(test)]
                last_pixels: Vec::new(),
            })
        }
        #[cfg(not(feature = "glow"))]
        {
            let renderer =
                Renderer::new_with_application(&window, atlas, actual.width as u32, actual.height as u32, DemoRenderer::new()).map_err(|e| e.to_string())?;
            Ok(Self { renderer, window })
        }
    }
    /// SDL's cheap owned clone keeps polling available while render state is temporarily detached.
    pub(super) fn window(&self) -> sdl2::video::Window {
        self.window.clone()
    }
    /// Selects this GL context before any native resource operation; explicit APIs need no global selection.
    fn current(&self) -> Result<(), String> {
        #[cfg(feature = "glow")]
        self.window.gl_make_current(&self.context)?;
        Ok(())
    }
    /// Uploads into this device's current context using the backend's existing atomic replacement.
    pub(super) fn replace_atlas(&mut self, atlas: AtlasHandle) -> Result<(), String> {
        self.current()?;
        self.renderer.replace_atlas(atlas).map_err(|error| error.to_string())
    }
    /// Executes only validated owned commands; no foreign method is invoked while a frame is acquired.
    pub(super) fn render(&mut self, info: RenderInfo, prepared: &Prepared) -> Result<(), String> {
        self.current()?;
        let [r, g, b, a] = info.clear.to_le_bytes();
        let dimensions = Dimensioni::new(info.size.width, info.size.height);
        let frame_info = FrameInfo::try_new(dimensions, Color { r, g, b, a }).map_err(|e| e.to_string())?;
        let mut frame = self.renderer.frame(frame_info).map_err(|e| e.to_string())?;
        for batch in prepared.batches() {
            match batch {
                Batch::Atlas(range) => {
                    for triangle in prepared.vertices()[range.clone()].as_chunks::<3>().0 {
                        frame.push_triangle(triangle.map(vertex));
                    }
                }
                Batch::Texture { vertices, texture } => frame.draw_texture(texture.id, vertices.map(vertex)),
                Batch::Item { rect, clip, item } => frame.enqueue_application(
                    CustomRenderArgs {
                        dimensions,
                        content_area: local_rect(*rect),
                        view: local_rect(*clip),
                    },
                    DemoCommand::DrawMesh(item.clone()),
                ),
            }
        }
        // Drop abandons; consuming finish alone submits. Vulkan/WGPU also present inside their finish.
        frame.finish().map_err(|e| e.to_string())?;
        #[cfg(feature = "glow")]
        {
            use glow::HasContext;
            // SAFETY: The same live GL context remains current throughout the foreign-call-free execution phase.
            let error = unsafe { self.gl.get_error() };
            if error != glow::NO_ERROR {
                return Err(format!("OpenGL frame failed with error 0x{error:04x}"));
            }
            #[cfg(test)]
            {
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
            self.window.gl_swap_window();
        }
        Ok(())
    }
    /// Completed GPU output is available only to native acceptance tests, never through a production ABI.
    #[cfg(all(test, feature = "glow"))]
    pub(super) fn last_pixels(&self) -> &[u8] {
        &self.last_pixels
    }
    /// Uploads into the native ownership map; the COM object is published only after success.
    pub(super) fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), String> {
        self.current()?;
        self.renderer.create_texture(id, pixels).map_err(|e| e.to_string())
    }
    /// Deletes retired allocations only after native frames have finished and the right context is selected.
    pub(super) fn retire(&mut self, ids: &mut Vec<TextureId>) -> Result<(), String> {
        if ids.is_empty() {
            return Ok(());
        }
        self.current()?;
        for id in ids.drain(..) {
            self.renderer.destroy_texture(id);
        }
        Ok(())
    }
}

impl Drop for Gpu {
    /// Selects the owning context before field destruction; destruction never submits or presents.
    fn drop(&mut self) {
        let _ = self.current();
    }
}

/// SDL 2.26+ provides physical pixel dimensions independently of the selected native graphics API.
pub(super) fn pixel_size(window: &sdl2::video::Window) -> Size {
    let (mut width, mut height) = (0, 0);
    // SAFETY: This owned live window and both writable outputs remain valid for the synchronous SDL call.
    unsafe {
        sdl2::sys::SDL_GetWindowSizeInPixels(window.raw(), &mut width, &mut height);
    }
    Size { width, height }
}

/// Converts canonical copied geometry once at the private native integration boundary.
fn local_rect(value: Rect) -> Recti {
    Recti::new(value.x, value.y, value.width, value.height)
}
/// Converts already validated vertex components to the native backend's temporary representation.
fn vertex(value: RenderVertex) -> Vertex {
    let [r, g, b, a] = value.rgba.to_le_bytes();
    Vertex::new(Vec2f::new(value.x, value.y), Vec2f::new(value.u, value.v), color4b(r, g, b, a))
}
