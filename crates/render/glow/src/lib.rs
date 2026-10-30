//
// Copyright 2022-Present (c) Raja Lehtihet & Wael El Oraiby
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice,
// this list of conditions and the following disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice,
// this list of conditions and the following disclaimer in the documentation
// and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors
// may be used to endorse or promote products derived from this software without
// specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
// AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
// LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
// CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
// SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
// INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
// CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
// ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
// POSSIBILITY OF SUCH DAMAGE.
//
//! OpenGL/Glow renderer backend used by examples.
//!
//! This module implements the `RendererBackend` trait, texture uploads, UI batching, and optional custom
//! mesh rendering for the demo application.

mod generic;
pub use generic::PreparedBlock;
use core::slice;
use std::{collections::HashMap, convert::Infallible, io, rc::Rc};

use neocogi_render_core::prelude::*;
use glow::*;
use neocogi_render_support::resource_guard::ResourceGuard;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

// GL backend overview:
// - Regular UI quads are accumulated into CPU-side vertex/index buffers and emitted in one batch
//   from `flush`.
// - Backend texture draws and solid custom vertices flush the UI batch first; application GPU
//   commands use the same ordering boundary through the concrete application renderer.
// - Each immutable atlas texture is uploaded transactionally at construction or theme selection.

/// Native frame capabilities lent to one application-renderer command.
///
/// This value deliberately exposes the graphics API rather than inventing a lowest-common-
/// denominator drawing vocabulary. The application renderer may bind its own programs, buffers,
/// textures, and fixed-function state. It cannot retain the context wrapper because the wrapper's
/// lifetime is tied to the active microui backend frame.
pub struct GlApplicationRenderContext<'a> {
    /// Shared Glow context owned by the concrete backend and valid for the active frame.
    ///
    /// The shared owner lets lazily created application resources retain the context needed for
    /// deterministic native-handle destruction without transferring backend ownership.
    pub gl: &'a Rc<glow::Context>,
    /// Complete drawable size used to translate microui's top-left coordinate space when needed.
    pub framebuffer_size: (u32, u32),
    /// Authoritative widget allocation and final clip supplied by microui's display-list executor.
    pub args: RenderItemArgs,
}

/// Strongly typed application-rendering extension hosted by the Glow backend.
///
/// The associated command keeps every application payload concrete. A demo may use an enum when it
/// has several operations, while an application with one operation may use a single struct. The
/// backend never inspects that value and therefore never depends on application mesh, material, or
/// scene types.
pub trait GlApplicationRenderer: 'static {
    /// Concrete command inserted into the active frame by application custom-render callbacks.
    type Command: 'static;

    /// Executes one command after the preceding UI batch has been flushed.
    ///
    /// Later UI draws restore their framebuffer, viewport, scissor, color writes, blending, depth,
    /// stencil, rasterization enables, program, texture/sampler, and private vertex layout. Pixel
    /// uploads likewise reset unpack state. Implementations must use their own GPU objects, finish
    /// scoped operations such as queries/transform feedback, and restore extension-specific state
    /// outside this GL ES 3 / WebGL 2 contract before returning. They must not delete or modify
    /// backend-owned resources. The backend renders UI to the default window/canvas framebuffer.
    /// An error is retained for `RendererFrame::finish`; later commands in that frame are skipped.
    fn render(&mut self, context: GlApplicationRenderContext<'_>, command: Self::Command) -> Result<(), String>;
}

impl GlApplicationRenderer for () {
    type Command = Infallible;

    /// Proves exhaustively that a UI-only renderer can never receive an application command.
    fn render(&mut self, _context: GlApplicationRenderContext<'_>, command: Self::Command) -> Result<(), String> {
        match command {}
    }
}

const VERTEX_SHADER: &str = "#version 100
uniform highp mat4 uTransform;
attribute highp vec2 vertexPosition;
attribute highp vec2 vertexTexCoord;
attribute lowp vec4 vertexColor;
varying highp vec2 vTexCoord;
varying lowp vec4 vVertexColor;
void main()
{
    vVertexColor = vertexColor;
    vTexCoord = vertexTexCoord;
    highp vec4 pos = vec4(vertexPosition.x, vertexPosition.y, 0.0, 1.0);
    gl_Position = uTransform * pos;
}";

const FRAGMENT_SHADER: &str = "#version 100
varying highp vec2 vTexCoord;
varying lowp vec4 vVertexColor;
uniform sampler2D uTexture;
void main()
{
    lowp vec4 col = texture2D(uTexture, vTexCoord);
    gl_FragColor = col * vVertexColor;
}";

/// Selected draw target for one exclusively borrowed frame.
#[derive(Clone, Copy)]
enum DrawTarget {
    /// Native window or browser canvas with its persistent default atlas.
    Window,
    /// Unpublished offscreen image and its session's independently resident atlas.
    Texture {
        /// Framebuffer owned by the output image's backend allocation.
        framebuffer: Framebuffer,
        /// Private atlas image selected only for this frame.
        atlas: Texture,
    },
}

/// Concrete GL device resources and one exclusively selected frame target.
pub struct GLRenderer<Application = ()>
where
    Application: GlApplicationRenderer,
{
    // `GLRenderer` is deliberately simple: it keeps one shader program plus shared VBO/IBO state
    // for ordinary UI draws, and falls back to explicit immediate draw calls for custom work that
    // cannot be folded into the batch easily.
    // GL contexts and every resource user remain on their owning UI thread. Rc expresses that
    // lifetime directly and also works for browser WebGL contexts, which are never Send/Sync.
    /// Current-thread GL context shared with guarded resource destruction.
    gl: Rc<glow::Context>,
    /// Pending atlas vertices for the current exclusive frame.
    verts: Vec<Vertex>,
    /// Triangle indices into the pending vertex batch.
    indices: Vec<u16>,

    /// Private vertex array isolates UI attributes from application vertex layouts.
    vao: VertexArray,
    /// Reusable UI vertex upload buffer.
    vbo: Buffer,
    /// Reusable UI index upload buffer.
    ibo: Buffer,
    /// Persistent atlas belonging to ordinary window frames.
    tex_o: Texture,

    /// Shared textured UI shader program.
    program: Program,

    /// Physical width of the selected frame.
    width: u32,
    /// Physical height of the selected frame.
    height: u32,

    /// Device-local immutable images, including unpublished render outputs.
    textures: HashMap<TextureId, Texture>,
    /// Framebuffers exist only for images allocated as render outputs.
    framebuffers: HashMap<TextureId, Framebuffer>,
    /// Frame-local target selection; reset when the exclusive frame is released.
    target: DrawTarget,
    /// Application-owned native renderer invoked only for its concrete command type.
    application: Application,
    /// Common graphics executor, isolated from the legacy UI vertex layout.
    generic: generic::Executor,
}

impl<Application> GLRenderer<Application>
where
    Application: GlApplicationRenderer,
{
    /// Realizes a sealed common graphics block before acquiring or clearing its target.
    pub fn prepare_block(&mut self, block: &neocogi_render_gpu::record::Block, offscreen: bool) -> Result<PreparedBlock, String> {
        self.generic.prepare(block, offscreen)
    }

    /// Returns the full default framebuffer in the UI's top-left coordinates.
    fn full_clip(&self) -> Recti {
        Recti::new(0, 0, self.width as i32, self.height as i32)
    }

    /// Establishes the window/canvas target and complete color-write region for draws and clears.
    fn bind_target(&self, clip: Recti) {
        // Restore the exact selected target before every draw, regardless of
        // framebuffer state left by preceding operations.
        unsafe {
            match self.target {
                DrawTarget::Window => {
                    self.gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
                    self.gl.draw_buffers(&[glow::BACK]);
                }
                DrawTarget::Texture { framebuffer, .. } => {
                    self.gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(framebuffer));
                    self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
                }
            }
            self.gl.viewport(0, 0, self.width as i32, self.height as i32);
            self.gl.color_mask(true, true, true, true);
            self.gl.disable(glow::RASTERIZER_DISCARD);
            self.gl.disable(glow::DITHER);
            self.gl.enable(glow::SCISSOR_TEST);
            let (x, y, width, height) = self.scissor_from_ui(clip).unwrap_or((0, 0, 0, 0));
            self.gl.scissor(x, y, width, height);
        }
    }

    /// Resets pixel-unpack inputs before any tightly packed CPU RGBA upload.
    fn prepare_upload(gl: &glow::Context) {
        // Selecting the unit precedes binding a texture. A stale unpack buffer would reinterpret
        // a CPU slice as a buffer offset; stale strides/skips would change its addressed byte range.
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            for parameter in [
                glow::UNPACK_ROW_LENGTH,
                glow::UNPACK_IMAGE_HEIGHT,
                glow::UNPACK_SKIP_PIXELS,
                glow::UNPACK_SKIP_ROWS,
                glow::UNPACK_SKIP_IMAGES,
            ] {
                gl.pixel_store_i32(parameter, 0);
            }
            #[cfg(target_arch = "wasm32")]
            {
                // Browser unpack conversion flags must not alter already-decoded RGBA bytes.
                // Glow exposes pixelStorei but omits these WebGL-only enum constants.
                const UNPACK_FLIP_Y_WEBGL: u32 = 0x9240;
                const UNPACK_PREMULTIPLY_ALPHA_WEBGL: u32 = 0x9241;
                const UNPACK_COLORSPACE_CONVERSION_WEBGL: u32 = 0x9243;
                gl.pixel_store_i32(UNPACK_FLIP_Y_WEBGL, 0);
                gl.pixel_store_i32(UNPACK_PREMULTIPLY_ALPHA_WEBGL, 0);
                gl.pixel_store_i32(UNPACK_COLORSPACE_CONVERSION_WEBGL, glow::NONE as i32);
            }
        }
    }

    /// Draws all UI geometry through one complete GL ES 3 / WebGL 2 state definition.
    ///
    /// `indices` selects indexed quads/triangles or a non-indexed colored triangle stream. The
    /// private VAO prevents application layouts from leaking into these identical Vertex records.
    fn draw_vertices(&self, texture: Texture, vertices: &[Vertex], indices: Option<&[u16]>, clip: Recti) {
        if vertices.is_empty() || indices.is_some_and(<[u16]>::is_empty) || clip.width <= 0 || clip.height <= 0 {
            return;
        }
        self.bind_target(clip);
        let gl = &self.gl;
        unsafe {
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::FUNC_ADD);
            // Preserve opaque target alpha under translucent UI primitives.
            gl.blend_func_separate(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            for capability in [
                glow::CULL_FACE,
                glow::DEPTH_TEST,
                glow::STENCIL_TEST,
                glow::RASTERIZER_DISCARD,
                glow::SAMPLE_ALPHA_TO_COVERAGE,
                glow::SAMPLE_COVERAGE,
                glow::POLYGON_OFFSET_FILL,
            ] {
                gl.disable(capability);
            }
            gl.depth_mask(false);
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_sampler(0, None);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            let sampler = gl.get_uniform_location(self.program, "uTexture").unwrap();
            gl.uniform_1_i32(Some(&sampler), 0);
            let transform = gl.get_uniform_location(self.program, "uTransform").unwrap();
            // Offscreen row zero must sample at v=0 like uploaded CPU images.
            let (bottom, top) = match self.target {
                DrawTarget::Window => (self.height as f32, 0.0),
                DrawTarget::Texture { .. } => (0.0, self.height as f32),
            };
            let matrix = ortho4(0.0, self.width as f32, bottom, top, -1.0, 1.0);
            let matrix = slice::from_raw_parts(matrix.col.as_ptr().cast::<f32>(), 16);
            gl.uniform_matrix_4_f32_slice(Some(&transform), false, matrix);

            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.ibo));
            // Vertex is repr(C), has a validated 20-byte layout, and is read only for this call.
            let bytes = slice::from_raw_parts(vertices.as_ptr().cast::<u8>(), std::mem::size_of_val(vertices));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::DYNAMIC_DRAW);
            for (name, count, kind, normalized, offset) in [
                ("vertexPosition", 2, glow::FLOAT, false, 0),
                ("vertexTexCoord", 2, glow::FLOAT, false, 8),
                ("vertexColor", 4, glow::UNSIGNED_BYTE, true, 16),
            ] {
                let attribute = gl.get_attrib_location(self.program, name).unwrap();
                gl.enable_vertex_attrib_array(attribute);
                gl.vertex_attrib_divisor(attribute, 0);
                gl.vertex_attrib_pointer_f32(attribute, count, kind, normalized, 20, offset);
            }
            if let Some(indices) = indices {
                let bytes = slice::from_raw_parts(indices.as_ptr().cast::<u8>(), std::mem::size_of_val(indices));
                gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytes, glow::DYNAMIC_DRAW);
                gl.draw_elements(glow::TRIANGLES, indices.len() as i32, glow::UNSIGNED_SHORT, 0);
            } else {
                gl.draw_arrays(glow::TRIANGLES, 0, vertices.len() as i32);
            }
            // Leave no private VAO/program selected when application code next receives the context.
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
            debug_assert_eq!(gl.get_error(), glow::NO_ERROR);
        }
    }

    /// Creates and fully uploads one atlas texture without changing the renderer's active atlas.
    ///
    /// Keeping allocation behind this helper gives atlas replacement a transaction boundary: the
    /// existing texture remains bound to future frames unless all GL setup and upload work for the
    /// candidate succeeds.
    fn create_atlas_texture(&self, atlas: ImageSource<'_>) -> Result<Texture, AtlasUploadError> {
        atlas.validate().map_err(|error| AtlasUploadError::new(error.to_string()))?;
        unsafe {
            let texture_gl = self.gl.clone();
            let texture = ResourceGuard::new(
                self.gl
                    .create_texture()
                    .map_err(|error| AtlasUploadError::new(format!("failed to create replacement atlas texture: {error}")))?,
                move |texture| texture_gl.delete_texture(texture),
            );
            Self::prepare_upload(&self.gl);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(*texture.get()));
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            let bytes = atlas.pixels;
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                atlas.width,
                atlas.height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                PixelUnpackData::Slice(Some(bytes)),
            );
            let error = self.gl.get_error();
            self.gl.bind_texture(glow::TEXTURE_2D, None);
            if error != glow::NO_ERROR {
                // The guard deletes the failed candidate while the renderer retains its old atlas.
                return Err(AtlasUploadError::new(format!(
                    "failed to upload replacement atlas texture: GL error 0x{error:04X}"
                )));
            }
            Ok(texture.into_inner())
        }
    }

    /// Converts a UI clip rectangle into GL scissor coordinates with bottom-left origin.
    fn scissor_from_ui(&self, clip: Recti) -> Option<(i32, i32, i32, i32)> {
        if clip.width <= 0 || clip.height <= 0 {
            return None;
        }
        let x = clip.x;
        let y = match self.target {
            DrawTarget::Window => (self.height as i32).saturating_sub(clip.y.saturating_add(clip.height)),
            DrawTarget::Texture { .. } => clip.y,
        };
        Some((x, y, clip.width, clip.height))
    }

    /// Creates a renderer with one concrete application renderer and caller-selected UI shaders.
    ///
    /// Shader selection remains a construction detail of the UI backend; the application renderer
    /// owns and compiles its own programs independently through the native context it receives.
    pub fn new_with_application_and_shaders(
        gl: Rc<glow::Context>,
        atlas: ImageSource<'_>,
        width: u32,
        height: u32,
        application: Application,
        vertex_shader: &str,
        fragment_shader: &str,
    ) -> Result<Self, String> {
        atlas.validate().map_err(|error| error.to_string())?;
        assert_eq!(core::mem::size_of::<Vertex>(), 20);
        unsafe {
            // Each driver object stays guarded until all persistent resources exist. Any
            // later GL/shader error therefore releases everything acquired earlier instead of
            // leaking a partially constructed renderer.
            let texture_gl = gl.clone();
            let atlas_texture = ResourceGuard::new(
                gl.create_texture().map_err(|err| format!("failed to create atlas texture: {err}"))?,
                move |texture| texture_gl.delete_texture(texture),
            );
            debug_assert!(gl.get_error() == 0);
            Self::prepare_upload(&gl);
            gl.bind_texture(glow::TEXTURE_2D, Some(*atlas_texture.get()));
            debug_assert!(gl.get_error() == 0);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            debug_assert!(gl.get_error() == 0);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            debug_assert!(gl.get_error() == 0);

            let pixel_slice = atlas.pixels;
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                atlas.width,
                atlas.height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                PixelUnpackData::Slice(Some(pixel_slice)),
            );
            let atlas_error = gl.get_error();
            if atlas_error != glow::NO_ERROR {
                return Err(format!("failed to upload immutable atlas texture: GL error 0x{atlas_error:04X}"));
            }
            gl.bind_texture(glow::TEXTURE_2D, None);

            let array_gl = gl.clone();
            let vertex_array = ResourceGuard::new(
                gl.create_vertex_array().map_err(|error| format!("failed to create UI vertex array: {error}"))?,
                move |array| array_gl.delete_vertex_array(array),
            );
            let vertex_gl = gl.clone();
            let vertex_buffer = ResourceGuard::new(
                gl.create_buffer().map_err(|err| format!("failed to create vertex buffer: {err}"))?,
                move |buffer| vertex_gl.delete_buffer(buffer),
            );
            let index_gl = gl.clone();
            let index_buffer = ResourceGuard::new(
                gl.create_buffer().map_err(|err| format!("failed to create index buffer: {err}"))?,
                move |buffer| index_gl.delete_buffer(buffer),
            );

            let program_gl = gl.clone();
            let program = ResourceGuard::new(
                create_program(&gl, vertex_shader, fragment_shader).map_err(|err| format!("failed to create UI program: {err}"))?,
                move |program| program_gl.delete_program(program),
            );

            // Construction is now infallible, so transfer all guarded handles together to the
            // renderer's deterministic `Drop` implementation.
            let vao = vertex_array.into_inner();
            let tex_o = atlas_texture.into_inner();
            let vbo = vertex_buffer.into_inner();
            let ibo = index_buffer.into_inner();
            let program = program.into_inner();

            let generic = generic::Executor::new(gl.clone())?;
            Ok(Self {
                gl,
                generic,
                verts: Vec::new(),
                indices: Vec::new(),

                vao,
                vbo,
                ibo,
                tex_o,
                program,

                width,
                height,
                textures: HashMap::new(),
                framebuffers: HashMap::new(),
                target: DrawTarget::Window,
                application,
            })
        }
    }

    /// Creates a native Glow renderer with one concrete application-rendering extension.
    pub fn new_with_application(gl: Rc<glow::Context>, atlas: ImageSource<'_>, width: u32, height: u32, application: Application) -> Result<Self, String> {
        // Native examples use the backend's standard GLSL dialect; only the application object is
        // supplied by the caller.
        Self::new_with_application_and_shaders(gl, atlas, width, height, application, VERTEX_SHADER, FRAGMENT_SHADER)
    }
}

impl GLRenderer<()> {
    /// Creates a UI-only native Glow renderer.
    pub fn new(gl: Rc<glow::Context>, atlas: ImageSource<'_>, width: u32, height: u32) -> Result<Self, String> {
        Self::new_with_application(gl, atlas, width, height, ())
    }

    /// Creates a UI-only renderer using a caller-selected GLSL dialect.
    ///
    /// The WebGL canvas wrapper uses this constructor for its ES 3.00 UI shaders.
    pub fn new_with_shaders(
        gl: Rc<glow::Context>,
        atlas: ImageSource<'_>,
        width: u32,
        height: u32,
        vertex_shader: &str,
        fragment_shader: &str,
    ) -> Result<Self, String> {
        Self::new_with_application_and_shaders(gl, atlas, width, height, (), vertex_shader, fragment_shader)
    }
}

impl<Application> GLRenderer<Application>
where
    Application: GlApplicationRenderer,
{
    /// Resolves the atlas belonging to this frame without uploading or replacing another session.
    fn active_atlas(&self) -> Texture {
        match self.target {
            DrawTarget::Window => self.tex_o,
            DrawTarget::Texture { atlas, .. } => atlas,
        }
    }

    /// Flushes the accumulated UI quad batch through the shared atlas pipeline.
    fn flush(&mut self) {
        // Indexed atlas, standalone texture, and colored draws share all consumed GL state.
        self.draw_vertices(self.active_atlas(), &self.verts, Some(&self.indices), self.full_clip());
        self.verts.clear();
        self.indices.clear();
    }

    /// Appends one quad to the UI batch, flushing first if the `u16` index budget would overflow.
    fn push_quad_vertices(&mut self, v0: &Vertex, v1: &Vertex, v2: &Vertex, v3: &Vertex) {
        if self.verts.len() + 4 >= 65536 || self.indices.len() + 6 >= 65536 {
            self.flush();
        }

        let is = self.verts.len() as u16;
        self.indices.push(is);
        self.indices.push(is + 1);
        self.indices.push(is + 2);
        self.indices.push(is + 2);
        self.indices.push(is + 3);
        self.indices.push(is);

        self.verts.push(*v0);
        self.verts.push(*v1);
        self.verts.push(*v2);
        self.verts.push(*v3);
    }

    /// Appends one triangle to the normal indexed UI batch, flushing first if the `u16` budget
    /// would overflow.
    fn push_triangle_vertices(&mut self, v0: &Vertex, v1: &Vertex, v2: &Vertex) {
        if self.verts.len() + 3 >= 65536 || self.indices.len() + 3 >= 65536 {
            self.flush();
        }

        let is = self.verts.len() as u16;
        self.indices.push(is);
        self.indices.push(is + 1);
        self.indices.push(is + 2);

        self.verts.push(*v0);
        self.verts.push(*v1);
        self.verts.push(*v2);
    }

    /// Starts a new GL frame by clearing the backbuffer and updating cached size.
    fn begin(&mut self, width: i32, height: i32, clr: Color) {
        self.width = width as u32;
        self.height = height as u32;
        let gl = &self.gl;
        unsafe {
            self.bind_target(self.full_clip());
            gl.clear_color(clr.r as f32 / 255.0, clr.g as f32 / 255.0, clr.b as f32 / 255.0, clr.a as f32 / 255.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            debug_assert!(gl.get_error() == 0);
        }
    }

    /// Finishes the frame by flushing any remaining batched UI geometry.
    fn end(&mut self) {
        self.flush();
    }

    /// Creates a GL texture for a backend-owned external image.
    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), TextureError> {
        self.create_image(id, Some(pixels))
    }

    /// Allocates either uploaded immutable pixels or an unpublished render image.
    fn create_image(&mut self, id: TextureId, pixels: Option<&[u8]>) -> Result<(), TextureError> {
        // The opaque capability is the sole dimension source validated by the Context executor.
        let dimensions = id.size();

        // Reserving the map entry can fail before any GL resource exists. Preserve that allocation
        // failure as backend context so callers receive the renderer's structured texture error.
        self.textures
            .try_reserve(1)
            .map_err(|err| TextureError::backend(format!("failed to reserve texture ownership entry: {err}")))?;
        let gl = &self.gl;
        unsafe {
            // User textures share the same nearest-neighbor setup as the atlas. Wrap allocation
            // failures immediately, before installing the resource guard, because no native
            // handle exists for the guard to own in that case.
            let texture_gl = gl.clone();
            let texture = ResourceGuard::new(
                gl.create_texture()
                    .map_err(|err| TextureError::backend(format!("failed to create texture: {err}")))?,
                move |texture| texture_gl.delete_texture(texture),
            );
            Self::prepare_upload(gl);
            gl.bind_texture(glow::TEXTURE_2D, Some(*texture.get()));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                dimensions.width,
                dimensions.height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                PixelUnpackData::Slice(pixels),
            );
            let err = gl.get_error();
            if err != 0 {
                // Restore the binding before returning. `texture` remains guarded here, so the
                // failed native object is deleted while the driver code is surfaced as backend
                // error detail rather than being collapsed into an untyped string.
                gl.bind_texture(glow::TEXTURE_2D, None);
                return Err(TextureError::backend(format!("OpenGL texture upload failed with error 0x{err:04x}")));
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            // IDs are normally unique, but replacing defensively keeps the ownership map sound if
            // a caller retries an existing capability.
            let previous = self.textures.insert(id, *texture.get());
            let _texture = texture.into_inner();
            if let Some(previous) = previous {
                gl.delete_texture(previous);
            }
        }
        Ok(())
    }

    /// Releases framebuffer attachment ownership before its texture allocation.
    fn destroy_texture(&mut self, id: TextureId) {
        if let Some(framebuffer) = self.framebuffers.remove(&id) {
            // SAFETY: GPU retirement invokes this only on the owning current context.
            unsafe {
                self.gl.delete_framebuffer(framebuffer);
            }
        }
        if let Some(tex) = self.textures.remove(&id) {
            unsafe {
                self.gl.delete_texture(tex);
            }
        }
    }

    /// Draws one pre-clipped textured quad using a backend-owned texture outside the atlas batch.
    fn draw_texture(&mut self, id: TextureId, vertices: [Vertex; 4]) {
        // Changing texture is a painter-order boundary even for direct frame callers.
        self.flush();
        if let Some(&texture) = self.textures.get(&id) {
            self.draw_vertices(texture, &vertices, Some(&[0, 1, 2, 2, 3, 0]), self.full_clip());
        }
    }
}

impl<Application> Drop for GLRenderer<Application>
where
    Application: GlApplicationRenderer,
{
    /// Releases every persistent GL object, including user textures that remain live at shutdown.
    fn drop(&mut self) {
        unsafe {
            for (_, framebuffer) in self.framebuffers.drain() {
                self.gl.delete_framebuffer(framebuffer);
            }
            for (_, texture) in self.textures.drain() {
                self.gl.delete_texture(texture);
            }
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_program(self.program);
            self.gl.delete_buffer(self.ibo);
            self.gl.delete_buffer(self.vbo);
            self.gl.delete_texture(self.tex_o);
        }
    }
}

/// Immediate GL frame whose pending batch must be completed explicitly.
#[must_use = "call finish to complete the OpenGL frame and observe errors"]
pub struct GlFrame<'a, Application = ()>
where
    Application: GlApplicationRenderer,
{
    /// Exclusive renderer borrow that prevents a second frame from being acquired concurrently.
    backend: &'a mut GLRenderer<Application>,
    /// First application failure, returned by finish; later commands are ignored.
    error: Option<FrameError>,
}

impl<Application> GlFrame<'_, Application>
where
    Application: GlApplicationRenderer,
{
    /// Executes only already realized common commands at this exact painter-order boundary.
    pub fn execute_block(&mut self, block: &PreparedBlock, logical: neocogi_render_gpu::abi::Size) {
        if self.error.is_some() {
            return;
        }
        self.backend.flush();
        self.backend.bind_target(self.backend.full_clip());
        let physical = neocogi_render_gpu::abi::Size {
            width: self.backend.width as i32,
            height: self.backend.height as i32,
        };
        let offscreen = matches!(self.backend.target, DrawTarget::Texture { .. });
        if let Err(error) = self.backend.generic.execute(block, logical, physical, offscreen) {
            self.error = Some(FrameError::new(error));
        }
    }

    /// Flushes preceding UI work and executes one typed application GPU command immediately.
    pub fn enqueue_application(&mut self, args: RenderItemArgs, command: Application::Command) {
        if self.error.is_some() {
            return;
        }
        if matches!(self.backend.target, DrawTarget::Texture { .. }) {
            self.error = Some(FrameError::new("application render items are unavailable on UI surfaces"));
            return;
        }
        // GL records directly into the current context. Closing the UI batch here preserves the
        // display-list order established by Context before application state changes GL bindings.
        self.backend.flush();
        let context = GlApplicationRenderContext {
            gl: &self.backend.gl,
            framebuffer_size: (self.backend.width, self.backend.height),
            args,
        };
        if let Err(error) = self.backend.application.render(context, command) {
            self.error = Some(FrameError::new(error));
        }
    }

    /// Draws pre-transformed colored vertices using the custom operation's authoritative clip.
    pub fn enqueue_colored_vertices(&mut self, args: RenderItemArgs, vertices: Vec<Vertex>) {
        if self.error.is_none() {
            self.backend.enqueue_colored_vertices(args, vertices);
        }
    }
}

impl<Application> RendererFrame for GlFrame<'_, Application>
where
    Application: GlApplicationRenderer,
{
    /// Flushes the final batch only if all earlier application commands succeeded.
    fn finish(mut self) -> Result<(), FrameError> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        self.backend.end();
        Ok(())
    }

    fn push_quad(&mut self, vertices: [Vertex; 4]) {
        if self.error.is_none() {
            self.backend.push_quad_vertices(&vertices[0], &vertices[1], &vertices[2], &vertices[3]);
        }
    }

    fn push_triangle(&mut self, vertices: [Vertex; 3]) {
        if self.error.is_none() {
            self.backend.push_triangle_vertices(&vertices[0], &vertices[1], &vertices[2]);
        }
    }

    fn flush(&mut self) {
        if self.error.is_none() {
            self.backend.flush();
        }
    }

    fn draw_texture(&mut self, id: TextureId, vertices: [Vertex; 4]) {
        if self.error.is_none() {
            self.backend.draw_texture(id, vertices);
        }
    }
}

impl<Application> Drop for GlFrame<'_, Application>
where
    Application: GlApplicationRenderer,
{
    fn drop(&mut self) {
        // Immediate draws cannot be undone; discard only the remaining CPU batch. In particular,
        // unwinding through a custom renderer must not call GL or submit more application work.
        self.backend.verts.clear();
        self.backend.indices.clear();
        self.backend.target = DrawTarget::Window;
    }
}

impl<Application> RendererBackend for GLRenderer<Application>
where
    Application: GlApplicationRenderer,
{
    type Frame<'a>
        = GlFrame<'a, Application>
    where
        Application: 'a;

    fn replace_atlas(&mut self, atlas: ImageSource<'_>) -> Result<(), AtlasUploadError> {
        // Upload into a separate texture first. Only a complete candidate may displace the live
        // GPU image; the borrowed CPU bytes are released by the caller after this method returns.
        let texture = self.create_atlas_texture(atlas)?;
        let previous = std::mem::replace(&mut self.tex_o, texture);

        unsafe {
            self.gl.delete_texture(previous);
        }
        Ok(())
    }

    fn frame(&mut self, info: FrameInfo) -> Result<Self::Frame<'_>, FrameError> {
        self.generic.collect();
        let dimensions = info.dimensions();
        self.target = DrawTarget::Window;
        self.begin(dimensions.width, dimensions.height, info.clear());
        Ok(GlFrame { backend: self, error: None })
    }

    /// Allocates a framebuffer-backed image without CPU zero filling or window presentation.
    fn create_render_texture(&mut self, id: TextureId) -> Result<(), TextureError> {
        self.framebuffers.try_reserve(1).map_err(TextureError::backend)?;
        self.create_image(id, None)?;
        let result = (|| {
            // SAFETY: This device owns the current context and both attachment handles.
            unsafe {
                let gl = &self.gl;
                let framebuffer = ResourceGuard::new(gl.create_framebuffer().map_err(TextureError::backend)?, |value| gl.delete_framebuffer(value));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(*framebuffer.get()));
                gl.framebuffer_texture_2d(
                    glow::DRAW_FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    self.textures.get(&id).copied(),
                    0,
                );
                gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
                let status = gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER);
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
                if status != glow::FRAMEBUFFER_COMPLETE {
                    return Err(TextureError::backend(format!("incomplete preview framebuffer: 0x{status:04x}")));
                }
                Ok(framebuffer.into_inner())
            }
        })();
        match result {
            Ok(framebuffer) => {
                self.framebuffers.insert(id, framebuffer);
                Ok(())
            }
            Err(error) => {
                self.destroy_texture(id);
                Err(error)
            }
        }
    }

    /// Selects already resident output and atlas images for one opaque UI frame.
    fn texture_frame(&mut self, info: FrameInfo, output: TextureId, atlas: TextureId) -> Result<Self::Frame<'_>, FrameError> {
        if output == atlas || (output.width(), output.height()) != (info.dimensions().width, info.dimensions().height) || info.clear().a != 255 {
            return Err(FrameError::new("invalid offscreen image, size, or nonopaque clear color"));
        }
        let framebuffer = *self.framebuffers.get(&output).ok_or_else(|| FrameError::new("missing offscreen framebuffer"))?;
        let atlas = *self.textures.get(&atlas).ok_or_else(|| FrameError::new("missing surface atlas"))?;
        self.target = DrawTarget::Texture { framebuffer, atlas };
        let dimensions = info.dimensions();
        self.begin(dimensions.width, dimensions.height, info.clear());
        Ok(GlFrame { backend: self, error: None })
    }

    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), TextureError> {
        // The inherent upload helper translates driver failures into the public texture error.
        self.create_texture(id, pixels)
    }

    fn destroy_texture(&mut self, id: TextureId) {
        self.destroy_texture(id);
    }
}

impl<Application> GLRenderer<Application>
where
    Application: GlApplicationRenderer,
{
    /// Draws arbitrary colored triangles while preserving microui's authoritative final clip.
    pub fn enqueue_colored_vertices(&mut self, args: RenderItemArgs, vertices: Vec<Vertex>) {
        self.draw_colored_vertices(args.view, vertices.as_slice());
    }

    /// Draws arbitrary colored triangles by sampling a white atlas texel and modulating by vertex color.
    fn draw_colored_vertices(&mut self, clip: Recti, vertices: &[Vertex]) {
        self.flush();
        // These UVs sample the white atlas texel, using the identical shader and vertex layout.
        self.draw_vertices(self.active_atlas(), vertices, None, clip);
    }
}

/// Compiles and links a GL program from vertex/fragment shader sources.
pub fn create_program(gl: &glow::Context, vertex_shader_source: &str, fragment_shader_source: &str) -> Result<Program, io::Error> {
    unsafe {
        // Program and shader guards cover allocation errors, compile/link errors, and unwinding.
        // Handles leave these guards only after a successful link.
        let program = ResourceGuard::new(gl.create_program().map_err(io::Error::other)?, |program| gl.delete_program(program));

        let shader_sources = [(glow::VERTEX_SHADER, vertex_shader_source), (glow::FRAGMENT_SHADER, fragment_shader_source)];

        let mut shaders = Vec::with_capacity(shader_sources.len());

        // Compile both stages first so we can bail out with a useful shader log if needed.
        for (shader_type, shader_source) in shader_sources.iter() {
            let shader = ResourceGuard::new(gl.create_shader(*shader_type).map_err(io::Error::other)?, |shader| gl.delete_shader(shader));
            gl.shader_source(*shader.get(), shader_source);
            gl.compile_shader(*shader.get());
            if !gl.get_shader_compile_status(*shader.get()) {
                return Err(io::Error::other(gl.get_shader_info_log(*shader.get())));
            }
            gl.attach_shader(*program.get(), *shader.get());
            shaders.push(shader);
        }

        // Link once both stages compiled successfully.
        gl.link_program(*program.get());
        if !gl.get_program_link_status(*program.get()) {
            return Err(io::Error::other(gl.get_program_info_log(*program.get())));
        }

        for shader in shaders {
            let shader = shader.into_inner();
            gl.detach_shader(*program.get(), shader);
            gl.delete_shader(shader);
        }

        Ok(program.into_inner())
    }
}
