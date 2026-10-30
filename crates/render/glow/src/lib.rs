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
//! Current-thread native GL and WebGL execution of sealed generic graphics blocks.
mod generic;
pub use generic::PreparedBlock;
use std::{collections::HashMap, io, rc::Rc};
use neocogi_render_core::prelude::*;
use glow::*;
use neocogi_render_support::resource_guard::ResourceGuard;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

/// Target selection is private to the exclusively borrowed frame.
#[derive(Clone, Copy)]
enum DrawTarget {
    /// Native window or browser canvas.
    Window,
    /// Fresh unpublished color image; it has no depth attachment.
    Texture {
        /// Framebuffer owned by the output image allocation.
        framebuffer: Framebuffer,
    },
}

/// Immutable native image may be shared by public UI capabilities and prepared generic draws.
struct Image {
    /// Dispatch owner has no renderer/cache/domain back-reference.
    gl: Rc<glow::Context>,
    /// Deleted exactly once, after the last image/command/completion owner releases it.
    raw: Texture,
}
impl Drop for Image {
    /// The plugin selects the owning context before all image and renderer retirement.
    fn drop(&mut self) {
        // SAFETY: The plugin, or a trusted Rust caller, keeps this owning native context current.
        unsafe { self.gl.delete_texture(self.raw) };
    }
}

/// One installed native image and its optional render attachment. Sampling retains
/// only the image; framebuffer residency ends when this entry is retired.
struct ResidentImage {
    /// Shared immutable pixels retain their existing independent completion lifetime.
    image: Rc<Image>,
    /// A render output owns exactly one framebuffer; ordinary uploads have none.
    framebuffer: Option<Framebuffer>,
}
impl Drop for ResidentImage {
    /// Delete attachment ownership while the image and its dispatch owner are alive.
    fn drop(&mut self) {
        if let Some(framebuffer) = self.framebuffer.take() {
            // SAFETY: all backend retirement occurs on the owning current context.
            unsafe { self.image.gl.delete_framebuffer(framebuffer) };
        }
    }
}

/// One current-thread native context, generic executor and immutable-image registry.
/// Trusted Rust callers own the actual native context lifetime and must make it current
/// for every call and resource destruction, including retained `PreparedBlock` drops.
/// An `Rc<glow::Context>` retains dispatch data, not the OS context.
pub struct GLRenderer {
    /// Dispatch owner outlives every native allocation.
    gl: Rc<glow::Context>,
    /// Physical dimensions of the selected output.
    width: u32,
    /// Physical dimensions of the selected output.
    height: u32,
    /// Immutable images shared by public image capabilities and generic prepared draws.
    textures: HashMap<TextureId, ResidentImage>,
    /// Test-only actual incomplete-framebuffer failure, consumed after allocation.
    #[cfg(test)]
    incomplete_output: bool,
    /// Selected native target, reset when its exclusive frame is dropped.
    target: DrawTarget,
    /// Sole draw executor; UI layout and shader policy live in GFX.
    generic: generic::Executor,
}
impl GLRenderer {
    /// Creates a generic native executor without shaders, atlas or UI vertex state.
    pub fn new(gl: Rc<glow::Context>, width: u32, height: u32) -> Result<Self, String> {
        let generic = generic::Executor::new(gl.clone())?;
        Ok(Self {
            gl,
            width,
            height,
            textures: HashMap::new(),
            #[cfg(test)]
            incomplete_output: false,
            target: DrawTarget::Window,
            generic,
        })
    }
    /// Realizes a sealed common graphics block before acquiring or clearing its target.
    pub fn prepare_block(&mut self, block: &neocogi_render_gpu::record::Block, offscreen: bool) -> Result<PreparedBlock, String> {
        self.generic.prepare(block, offscreen, &self.textures)
    }

    /// Trusted UI upload preparation has no target acquisition or presentation side effect.
    pub fn prepare_texture(&mut self, texture: Rc<neocogi_render_gpu::resource::TextureVersion>) -> Result<(), String> {
        self.generic.prepare_texture(texture, &self.textures)
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

    /// Allocates either uploaded immutable pixels or an unpublished render image.
    fn create_image(&mut self, id: TextureId, pixels: Option<&[u8]>) -> Result<Rc<Image>, TextureError> {
        // A key cannot certify this new slice. Validate before reserving ownership or entering GL.
        match pixels {
            Some(pixels) => ImageSource {
                width: id.width(),
                height: id.height(),
                pixels,
            }
            .validate()?,
            None => {
                neocogi_render_core::checked_rgba_len(id.width(), id.height())?;
            }
        }
        if self.textures.contains_key(&id) {
            return Err(TextureError::backend("native image key is already resident"));
        }
        let dimensions = id.size();

        // Reserving the map entry can fail before any GL resource exists. Preserve that allocation
        // failure as backend context so callers receive the renderer's structured texture error.
        self.textures
            .try_reserve(1)
            .map_err(|err| TextureError::backend(format!("failed to reserve texture ownership entry: {err}")))?;
        let gl = &self.gl;
        unsafe {
            // Immutable ordinary images default to nearest sampling. Wrap allocation
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
            Ok(Rc::new(Image {
                gl: gl.clone(),
                raw: texture.into_inner(),
            }))
        }
    }

    /// Retires attachment residency independently of image sampling/completion owners.
    fn destroy_texture(&mut self, id: TextureId) {
        self.textures.remove(&id);
    }
}

/// Immediate native frame. Explicit finish observes failures and retains execution through a fence.
#[must_use = "call finish to complete the OpenGL frame and observe errors"]
pub struct GlFrame<'a> {
    /// Exclusive target ownership prevents concurrent acquisition.
    backend: &'a mut GLRenderer,
    /// First native execution error; subsequent commands are ignored.
    error: Option<FrameError>,
}
impl GlFrame<'_> {
    /// Executes only already realized common commands at this exact painter-order boundary.
    pub fn execute_block(&mut self, block: &PreparedBlock) {
        if self.error.is_some() {
            return;
        }
        self.backend.bind_target(self.backend.full_clip());
        let physical = neocogi_render_gpu::abi::Size {
            width: self.backend.width as i32,
            height: self.backend.height as i32,
        };
        let logical = physical;
        let offscreen = matches!(self.backend.target, DrawTarget::Texture { .. });
        if let Err(error) = self.backend.generic.execute(block, logical, physical, offscreen) {
            self.error = Some(FrameError::new(error));
        }
    }
}
impl RendererFrame for GlFrame<'_> {
    /// Establish completion retention even after a partial immediate failure.
    fn finish(mut self) -> Result<(), FrameError> {
        if let Some(error) = self.error.take() {
            let _ = self.backend.generic.complete();
            return Err(error);
        }
        self.backend.generic.complete().map_err(FrameError::new)
    }
}
impl Drop for GlFrame<'_> {
    /// Abandonment does not issue more GPU work; the executor still owns pending resource backings.
    fn drop(&mut self) {
        self.backend.target = DrawTarget::Window;
    }
}
impl RendererBackend for GLRenderer {
    type Frame<'a> = GlFrame<'a>;
    fn frame(&mut self, info: FrameInfo) -> Result<Self::Frame<'_>, FrameError> {
        self.generic.collect().map_err(FrameError::new)?;
        let dimensions = info.dimensions();
        self.target = DrawTarget::Window;
        self.begin(dimensions.width, dimensions.height, info.clear());
        Ok(GlFrame { backend: self, error: None })
    }

    /// Allocates a framebuffer-backed image without CPU zero filling or window presentation.
    fn create_render_texture(&mut self, id: TextureId) -> Result<(), TextureError> {
        let image = self.create_image(id, None)?;
        // Keep both parts unpublished and guarded until the real driver admits the attachment.
        // SAFETY: the owning context is current and the local image/guard own every handle.
        let framebuffer = unsafe {
            let gl = &self.gl;
            let framebuffer = ResourceGuard::new(gl.create_framebuffer().map_err(TextureError::backend)?, |value| gl.delete_framebuffer(value));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(*framebuffer.get()));
            gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(image.raw), 0);
            #[cfg(test)]
            if std::mem::take(&mut self.incomplete_output) {
                // Exercise production completeness rejection and guard cleanup with real handles.
                gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, None, 0);
            }
            gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
            let status = gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                return Err(TextureError::backend(format!("incomplete preview framebuffer: 0x{status:04x}")));
            }
            framebuffer.into_inner()
        };
        self.textures.insert(id, ResidentImage { image, framebuffer: Some(framebuffer) });
        Ok(())
    }

    /// Selects a resident private output for one opaque generic frame.
    fn texture_frame(&mut self, info: FrameInfo, output: TextureId) -> Result<Self::Frame<'_>, FrameError> {
        self.generic.collect().map_err(FrameError::new)?;
        if (output.width(), output.height()) != (info.dimensions().width, info.dimensions().height) || info.clear().a != 255 {
            return Err(FrameError::new("invalid offscreen image, size, or nonopaque clear color"));
        }
        let framebuffer = self
            .textures
            .get(&output)
            .and_then(|entry| entry.framebuffer)
            .ok_or_else(|| FrameError::new("missing offscreen framebuffer"))?;
        self.target = DrawTarget::Texture { framebuffer };
        let dimensions = info.dimensions();
        self.begin(dimensions.width, dimensions.height, info.clear());
        Ok(GlFrame { backend: self, error: None })
    }
    /// Upload exact validated immutable image bytes using the same native owner as UI imports.
    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), TextureError> {
        let image = self.create_image(id, Some(pixels))?;
        self.textures.insert(id, ResidentImage { image, framebuffer: None });
        Ok(())
    }
    /// Retire framebuffer and image references together.
    fn destroy_texture(&mut self, id: TextureId) {
        self.destroy_texture(id);
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

mod shader_adapter;
