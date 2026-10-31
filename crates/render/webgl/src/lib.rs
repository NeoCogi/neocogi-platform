//! WebGL 2 renderer for an HTML canvas.
//!
//! Browser canvas assembly delegates the generic command vocabulary to the shared native GL
//! executor. UI shader and vertex policy remain in GFX.

use std::rc::Rc;

use neocogi_render_core::{FrameError, FrameInfo, RendererBackend, TextureError, TextureId};
use neocogi_render_glow::{GLRenderer, GlFrame};
pub use neocogi_render_glow::PreparedBlock;

/// Renderer specialized for a browser WebGL 2 context attached to an HTML canvas.
pub struct WebGlCanvasRenderer {
    /// Shared generic executor dispatches through the browser WebGL context.
    inner: GLRenderer,
}

impl WebGlCanvasRenderer {
    /// Creates the renderer from the canvas's current WebGL 2 context.
    pub fn new(gl: Rc<glow::Context>, width: u32, height: u32) -> Result<Self, String> {
        GLRenderer::new(gl, width, height).map(|inner| Self { inner })
    }
}

impl WebGlCanvasRenderer {
    /// Realizes the common graphics vocabulary through the shared WebGL 2 executor.
    pub fn prepare_block(&mut self, block: &neocogi_render_gpu::record::Block, offscreen: bool) -> Result<PreparedBlock, String> {
        self.inner.prepare_block(block, offscreen)
    }

    /// Private UI image preparation uses the same context and immutable image accounting.
    pub fn prepare_texture(&mut self, texture: Rc<neocogi_render_gpu::resource::TextureVersion>) -> Result<(), String> {
        self.inner.prepare_texture(texture)
    }
}

impl RendererBackend for WebGlCanvasRenderer {
    type Frame<'a> = GlFrame<'a>;

    fn frame(&mut self, info: FrameInfo) -> Result<Self::Frame<'_>, FrameError> {
        self.inner.frame(info)
    }

    /// Uses the shared GL resource owner for unpublished canvas-independent output.
    fn create_render_texture(&mut self, id: TextureId) -> Result<(), TextureError> {
        self.inner.create_render_texture(id)
    }

    /// Selects an offscreen target without clearing or presenting the browser canvas.
    fn texture_frame(&mut self, info: FrameInfo, output: TextureId) -> Result<Self::Frame<'_>, FrameError> {
        self.inner.texture_frame(info, output)
    }

    /// Observes prior completion and releases safe native owners without touching a target.
    fn maintain(&mut self) -> Result<(), FrameError> {
        self.inner.maintain()
    }
    /// Creates an initialized immutable image independently of render-target selection.
    fn create_texture(&mut self, id: TextureId, pixels: Option<&[u8]>) -> Result<(), TextureError> {
        self.inner.create_texture(id, pixels)
    }

    fn destroy_texture(&mut self, id: TextureId) {
        self.inner.destroy_texture(id);
    }
}
