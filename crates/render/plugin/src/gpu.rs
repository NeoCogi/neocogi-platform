//! Native GPU execution remains local to this plugin; only construction differs between selected backends.
use super::*;
use neocogi_render_core::{
    Color, FrameInfo, ImageSource, RenderItemArgs, RendererBackend, RendererFrame, TextureId,
    prelude::{Dimensioni, Recti},
};
use neocogi_render_mesh::{MeshCommand, MeshRenderer};
use neocogi_render_support::com::Batch;

#[cfg(feature = "glow")]
use neocogi_render_glow::GLRenderer as Renderer;
#[cfg(feature = "vulkan")]
use neocogi_render_vulkan::VulkanRenderer as Renderer;
#[cfg(feature = "wgpu")]
use neocogi_render_wgpu::WgpuRenderer as Renderer;
#[cfg(feature = "webgl")]
use neocogi_render_webgl::WebGlCanvasRenderer as Renderer;
/// Concrete drawable owner selected at compilation, never a native-handle escape through COM.
#[cfg(not(target_arch = "wasm32"))]
pub(super) type Window = sdl2::video::Window;
/// Browser objects stay in the statically assembled platform implementation.
#[cfg(target_arch = "wasm32")]
pub(super) type Window = web_sys::HtmlCanvasElement;

/// Concrete native owner. Its field order destroys all UI/application GPU resources before the surface.
pub(super) struct Gpu {
    /// Both UI drawing and the reusable depth-tested mesh renderer live in this concrete backend.
    renderer: Renderer<MeshRenderer>,
    /// Shared loader is used only to check GL execution errors before presentation.
    #[cfg(any(feature = "glow", feature = "webgl"))]
    gl: std::rc::Rc<glow::Context>,
    /// GL context outlives renderer destruction, including its application pipelines and buffers.
    #[cfg(all(not(target_arch = "wasm32"), feature = "glow"))]
    context: sdl2::video::GLContext,
    /// The browser context reports loss before native operations; restoration requires a new device.
    #[cfg(target_arch = "wasm32")]
    context: web_sys::WebGl2RenderingContext,
    /// Test-only readback captures completed GPU output before the window swap.
    #[cfg(all(test, feature = "glow"))]
    last_pixels: Vec<u8>,
    /// Surface outlives both backend resources and the optional GL context.
    window: Window,
}

impl Gpu {
    /// Creates the chosen native window and backend in one fallible transaction.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn new(video: &sdl2::VideoSubsystem, title: &str, size: Size, atlas: ImageSource<'_>) -> Result<Self, String> {
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
            let gl = std::rc::Rc::new(unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name) as *const _) });
            let renderer = Renderer::new_with_application(gl.clone(), atlas, actual.width as u32, actual.height as u32, MeshRenderer::new())?;
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
                Renderer::new_with_application(&window, atlas, actual.width as u32, actual.height as u32, MeshRenderer::new()).map_err(|e| e.to_string())?;
            Ok(Self { renderer, window })
        }
    }
    /// Creates the browser renderer using the same concrete UI and mesh execution path.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn from_canvas(window: Window, atlas: ImageSource<'_>) -> Result<Self, String> {
        use wasm_bindgen::JsCast;
        let webgl = window
            .get_context("webgl2")
            .map_err(|error| format!("{error:?}"))?
            .ok_or("WebGL 2 is unavailable")?
            .dyn_into::<web_sys::WebGl2RenderingContext>()
            .map_err(|_| "canvas context is not WebGL 2")?;
        let gl = std::rc::Rc::new(glow::Context::from_webgl2_context(webgl.clone()));
        let renderer = Renderer::new_with_application(gl.clone(), atlas, window.width(), window.height(), MeshRenderer::new())?;
        Ok(Self { renderer, gl, context: webgl, window })
    }
    /// SDL's cheap owned clone keeps polling available while render state is temporarily detached.
    pub(super) fn window(&self) -> Window {
        self.window.clone()
    }
    /// Selects this GL context before any native resource operation; explicit APIs need no global selection.
    fn current(&self) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        if self.context.is_context_lost() {
            return Err("WebGL context was lost; recreate the platform window and its GPU resources".to_owned());
        }
        #[cfg(all(not(target_arch = "wasm32"), feature = "glow"))]
        self.window.gl_make_current(&self.context)?;
        Ok(())
    }
    /// Uploads into this device's current context using the backend's existing atomic replacement.
    pub(super) fn replace_atlas(&mut self, atlas: ImageSource<'_>) -> Result<(), String> {
        self.current()?;
        self.renderer.replace_atlas(atlas).map_err(|error| error.to_string())
    }
    /// Executes only validated owned commands; no foreign method is invoked while a frame is acquired.
    pub(super) fn render(&mut self, info: RenderInfo, prepared: &Prepared) -> Result<(), String> {
        self.render_target(info, prepared, None)
    }

    /// Reports surface availability before accepting an independent rendering session.
    pub(super) fn supports_surfaces(&self) -> bool {
        cfg!(any(feature = "glow", feature = "webgl", feature = "wgpu", feature = "vulkan"))
    }

    /// Allocates an unpublished offscreen output through the native image owner.
    pub(super) fn create_render_texture(&mut self, id: TextureId) -> Result<(), String> {
        self.current()?;
        self.renderer.create_render_texture(id).map_err(|error| error.to_string())
    }

    /// Executes UI into an unpublished texture without any native presentation.
    pub(super) fn render_surface(&mut self, info: RenderInfo, prepared: &Prepared, output: TextureId, atlas: TextureId) -> Result<(), String> {
        self.render_target(info, prepared, Some((output, atlas)))
    }

    /// Shares validated command execution while keeping target acquisition and presentation explicit.
    fn render_target(&mut self, info: RenderInfo, prepared: &Prepared, target: Option<(TextureId, TextureId)>) -> Result<(), String> {
        self.current()?;
        let [r, g, b, a] = info.clear.to_le_bytes();
        let dimensions = Dimensioni::new(info.size.width, info.size.height);
        let frame_info = FrameInfo::try_new(dimensions, Color { r, g, b, a }).map_err(|e| e.to_string())?;
        // Resource compilation/upload finishes before any target acquire or attachment clear.
        #[cfg(any(feature = "glow", feature = "webgl"))]
        let blocks = prepared
            .batches()
            .iter()
            .map(|batch| match batch {
                Batch::View(block) => self.renderer.prepare_block(block, target.is_some()).map(Some),
                _ => Ok(None),
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut frame = match target {
            None => self.renderer.frame(frame_info),
            Some((output, atlas)) => self.renderer.texture_frame(frame_info, output, atlas),
        }
        .map_err(|error| error.to_string())?;
        for (index, batch) in prepared.batches().iter().enumerate() {
            match batch {
                Batch::View(_block) => {
                    #[cfg(any(feature = "glow", feature = "webgl"))]
                    frame.execute_block(blocks[index].as_ref().expect("prepared GPU block"), info.size);
                    #[cfg(not(any(feature = "glow", feature = "webgl")))]
                    return Err("generic command execution is not integrated for this backend".to_owned());
                }

                Batch::Atlas(range) => {
                    // Preparation already validated this as a complete contiguous triangle range;
                    // preserve that coarse unit through the native backend instead of replaying it
                    // through one method invocation and one vector extension per triangle.
                    frame.push_triangles(&prepared.vertices()[range.clone()]);
                }
                Batch::Texture { vertices, texture } => {
                    // Close preceding atlas geometry before recording the image. Explicit backends
                    // otherwise submit that geometry later, painting backgrounds over the texture.
                    frame.flush();
                    frame.draw_texture(texture.id, *vertices);
                }
                Batch::Item { rect, clip, item } => frame.enqueue_application(
                    RenderItemArgs {
                        dimensions,
                        content_area: local_rect(*rect),
                        view: local_rect(*clip),
                    },
                    MeshCommand::DrawMesh(item.clone()),
                ),
            }
        }
        // Drop abandons; consuming finish alone submits. Vulkan/WGPU also present inside their finish.
        frame.finish().map_err(|e| e.to_string())?;
        #[cfg(any(feature = "glow", feature = "webgl"))]
        {
            use glow::HasContext;
            // SAFETY: The same live GL context remains current throughout the foreign-call-free execution phase.
            let error = unsafe { self.gl.get_error() };
            if error != glow::NO_ERROR {
                return Err(format!("OpenGL frame failed with error 0x{error:04x}"));
            }
            #[cfg(all(test, feature = "glow"))]
            if target.is_none() {
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
            if target.is_none() {
                self.window.gl_swap_window();
            }
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
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn pixel_size(window: &Window) -> Size {
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
