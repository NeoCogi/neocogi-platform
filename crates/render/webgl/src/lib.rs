//! WebGL 2 renderer for an HTML canvas.
//!
//! This backend owns the browser-specific shader contract and exposes a distinct backend/frame
//! type. Its command batching and GL resource implementation are shared with the native Glow
//! renderer.

use std::rc::Rc;

use neocogi_render_core::{AtlasUploadError, FrameError, FrameInfo, ImageSource, RendererBackend, TextureError, TextureId};
use neocogi_render_glow::{GLRenderer, GlFrame};
pub use neocogi_render_glow::PreparedBlock;

const VERTEX_SHADER: &str = "#version 300 es
uniform highp mat4 uTransform;
in highp vec2 vertexPosition;
in highp vec2 vertexTexCoord;
in lowp vec4 vertexColor;
out highp vec2 vTexCoord;
out lowp vec4 vVertexColor;
void main()
{
    vVertexColor = vertexColor;
    vTexCoord = vertexTexCoord;
    highp vec4 pos = vec4(vertexPosition.x, vertexPosition.y, 0.0, 1.0);
    gl_Position = uTransform * pos;
}";

const FRAGMENT_SHADER: &str = "#version 300 es
precision mediump float;
in highp vec2 vTexCoord;
in lowp vec4 vVertexColor;
uniform sampler2D uTexture;
out lowp vec4 fragmentColor;
void main()
{
    lowp vec4 col = texture(uTexture, vTexCoord);
    fragmentColor = col * vVertexColor;
}";

/// Renderer specialized for a browser WebGL 2 context attached to an HTML canvas.
pub struct WebGlCanvasRenderer {
    /// Shared common executor; browser code selects only the canvas shader dialect.
    inner: GLRenderer,
}

impl WebGlCanvasRenderer {
    /// Creates the renderer from the canvas's current WebGL 2 context.
    pub fn new(gl: Rc<glow::Context>, atlas: ImageSource<'_>, width: u32, height: u32) -> Result<Self, String> {
        GLRenderer::new_with_shaders(gl, atlas, width, height, VERTEX_SHADER, FRAGMENT_SHADER).map(|inner| Self { inner })
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

    fn replace_atlas(&mut self, atlas: ImageSource<'_>) -> Result<(), AtlasUploadError> {
        self.inner.replace_atlas(atlas)
    }

    fn frame(&mut self, info: FrameInfo) -> Result<Self::Frame<'_>, FrameError> {
        self.inner.frame(info)
    }

    /// Uses the shared GL resource owner for unpublished canvas-independent output.
    fn create_render_texture(&mut self, id: TextureId) -> Result<(), TextureError> {
        self.inner.create_render_texture(id)
    }

    /// Selects an offscreen target without clearing or presenting the browser canvas.
    fn texture_frame(&mut self, info: FrameInfo, output: TextureId, atlas: TextureId) -> Result<Self::Frame<'_>, FrameError> {
        self.inner.texture_frame(info, output, atlas)
    }

    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), TextureError> {
        self.inner.create_texture(id, pixels)
    }

    fn destroy_texture(&mut self, id: TextureId) {
        self.inner.destroy_texture(id);
    }
}
