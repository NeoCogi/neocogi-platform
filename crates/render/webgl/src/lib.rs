//! WebGL 2 renderer for an HTML canvas.
//!
//! This backend owns the browser-specific shader contract and exposes a distinct backend/frame
//! type. Its command batching and GL resource implementation are shared with the native Glow
//! renderer.

use std::rc::Rc;

use neocogi_ui_runtime::{
    prelude::{ImageSource, AtlasUploadError, FrameError, FrameInfo, RendererBackend, TextureId},
    render::TextureError,
};
use neocogi_render_glow::{GLRenderer, GlFrame};
pub use neocogi_render_glow::GlApplicationRenderer;

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
pub struct WebGlCanvasRenderer<Application = ()>
where
    Application: GlApplicationRenderer,
{
    /// Shared GL implementation parameterized by the application's concrete renderer type.
    inner: GLRenderer<Application>,
}

impl WebGlCanvasRenderer<()> {
    /// Creates the renderer from the canvas's current WebGL 2 context.
    pub fn new(gl: Rc<glow::Context>, atlas: ImageSource<'_>, width: u32, height: u32) -> Result<Self, String> {
        GLRenderer::new_with_shaders(gl, atlas, width, height, VERTEX_SHADER, FRAGMENT_SHADER).map(|inner| Self { inner })
    }
}

impl<Application> WebGlCanvasRenderer<Application>
where
    Application: GlApplicationRenderer,
{
    /// Creates a WebGL renderer with one strongly typed application GPU extension.
    pub fn new_with_application(gl: Rc<glow::Context>, atlas: ImageSource<'_>, width: u32, height: u32, application: Application) -> Result<Self, String> {
        // The wrapper selects only the UI shader dialect. Application shaders remain entirely
        // owned by `Application` and are compiled through its native Glow context.
        GLRenderer::new_with_application_and_shaders(gl, atlas, width, height, application, VERTEX_SHADER, FRAGMENT_SHADER).map(|inner| Self { inner })
    }
}

impl<Application> RendererBackend for WebGlCanvasRenderer<Application>
where
    Application: GlApplicationRenderer,
{
    type Frame<'a>
        = GlFrame<'a, Application>
    where
        Application: 'a;

    fn replace_atlas(&mut self, atlas: ImageSource<'_>) -> Result<(), AtlasUploadError> {
        self.inner.replace_atlas(atlas)
    }

    fn frame(&mut self, info: FrameInfo) -> Result<Self::Frame<'_>, FrameError> {
        self.inner.frame(info)
    }

    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), TextureError> {
        self.inner.create_texture(id, pixels)
    }

    fn destroy_texture(&mut self, id: TextureId) {
        self.inner.destroy_texture(id);
    }
}
