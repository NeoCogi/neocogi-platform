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
//! Current-thread execution of typed resource-owned GL/WebGL graphics objects.
mod context;
pub use context::Core;
mod generic;
pub use generic::{Backend, PreparedBlock};
use std::{
    io,
    ops::Deref,
    rc::Rc,
    sync::mpsc::{Sender, SendError},
};
use glow::*;
use neocogi_render_core::{prelude::*, abi::Size};
use crate::resource::BackingCharge;
use crate::resource_guard::ResourceGuard;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

/// One bounded read-only chunk shared by allocation-only storage initialization.
static ZERO_BYTES: [u8; 64 * 1024] = [0; 64 * 1024];

/// Self-contained owned pixels. Queued retirement never carries a sender or source version.
pub struct ImageAllocation {
    /// Actual compatible physical context lifetime.
    core: Rc<Core>,
    /// Exactly one native image name.
    raw: Texture,
    /// Immutable admitted extent of this same allocation.
    size: Size,
    /// Generic storage charge; ordinary images/import-use receipts have distinct owners.
    _charge: Option<BackingCharge>,
}
impl Drop for ImageAllocation {
    fn drop(&mut self) {
        if self.core.for_drop() {
            // SAFETY: The actual owning physical context was selected before native destruction.
            unsafe { self.core.dispatch().delete_texture(self.raw) };
        }
    }
}
/// Shared initialized native image backing; last wrapper release signals the existing driver.
/// An unpublished output keeps this private until successful frame completion.
pub struct Image {
    /// Output attachments may retain this sender-free allocation independently of sampling wrappers.
    allocation: Rc<ImageAllocation>,
    /// Only the notifying wrapper retains a producer; the core and payload never do.
    retired: Sender<generic::Retirement>,
}
impl Deref for Image {
    type Target = ImageAllocation;
    fn deref(&self) -> &ImageAllocation {
        &self.allocation
    }
}
impl Drop for Image {
    fn drop(&mut self) {
        if let Err(SendError(retired)) = self.retired.send(generic::Retirement::Image(self.allocation.clone())) {
            drop(retired);
        }
    }
}
/// Owned attachment packet; it retains only raw allocation lifetime, never a notifying image wrapper.
pub(in crate::backends::glow) struct FramebufferAllocation {
    /// Parent context selected for this framebuffer's destruction.
    core: Rc<Core>,
    /// One framebuffer name, no public target-selector API.
    raw: Framebuffer,
    /// Pixels outlive attachment destruction even when sampling/retirement owners overlap.
    _image: Rc<ImageAllocation>,
}
impl Drop for FramebufferAllocation {
    fn drop(&mut self) {
        if self.core.for_drop() {
            // SAFETY: The matching physical context and image allocation remain live.
            unsafe { self.core.dispatch().delete_framebuffer(self.raw) };
        }
    }
}
/// Internal shared target lifetime retained by immediate/submitted work, including clear-only frames.
struct OutputState {
    /// Immutable image can be published only by successful frame finish.
    image: Rc<Image>,
    /// Taken exactly once by final target destruction; packet owns real allocation dependencies.
    allocation: Option<FramebufferAllocation>,
    /// Notifier remains outside the actual owned attachment packet.
    retired: Sender<generic::Retirement>,
}
impl OutputState {
    /// An active shared target always owns its attachment; only final Drop can take it.
    fn framebuffer(&self) -> Framebuffer {
        self.allocation.as_ref().expect("live output attachment").raw
    }
}
impl Drop for OutputState {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take()
            && let Err(SendError(retired)) = self.retired.send(generic::Retirement::Framebuffer(allocation))
        {
            drop(retired);
        }
    }
}
/// Single-use unpublished destination. Consuming it prevents rewriting an already returned image.
pub struct Output {
    /// Only native work can clone the attachment owner; callers receive no mutable pixels.
    state: Rc<OutputState>,
}
/// Target authority belongs to the exclusively borrowed frame, not client commands.
enum DrawTarget {
    /// Actual physical default framebuffer.
    Window,
    /// Shared native owner exists before any clear/draw can reference its attachments.
    Texture(Rc<OutputState>),
}
/// One driver execution state; actual hardware lifetime is separate and resource-owned.
pub struct GLRenderer {
    /// Dispatch is accompanied by the actual context lifetime guard.
    gl: Rc<glow::Context>,
    /// Owns no driver/submission/channel state and may outlive this facade.
    core: Rc<Core>,
    /// Current output dimensions, separate from allocation/compatibility identity.
    width: u32,
    /// Current output dimensions, separate from allocation/compatibility identity.
    height: u32,
    /// Real incomplete-FBO injection exercises actual construction rollback.
    #[cfg(test)]
    incomplete_output: bool,
    /// Exclusive current target; only this native implementation constructs it.
    target: DrawTarget,
    /// Prepared/completed work plus the one retirement receiver belong to this existing driver.
    generic: generic::Executor,
}
impl GLRenderer {
    /// Starts a concrete executor using already owned compatible physical context lifetime.
    pub fn new(core: Rc<Core>, width: u32, height: u32) -> Result<Self, String> {
        core.current()?;
        let gl = core.dispatch().clone();
        let generic = generic::Executor::new(core.clone())?;
        Ok(Self {
            gl,
            core,
            width,
            height,
            #[cfg(test)]
            incomplete_output: false,
            target: DrawTarget::Window,
            generic,
        })
    }
    /// Shares hardware lifetime without rendering/submission or input authority.
    pub fn core(&self) -> Rc<Core> {
        self.core.clone()
    }
    /// Native supported entries reselect the physical context after foreign activity or late drops.
    pub fn make_current(&self) -> Result<(), String> {
        self.core.current()
    }
    /// Realizes exact immutable objects, checking driver ownership even for empty/clear-only blocks.
    pub fn prepare_block(&mut self, block: &crate::record::Block<Backend>, offscreen: bool) -> Result<PreparedBlock, String> {
        self.generic.prepare(block, offscreen)
    }
    /// Prepares an owned imported image without acquiring or clearing any target.
    pub fn prepare_texture(&mut self, source: Rc<crate::resource::TextureVersion<Backend>>) -> Result<(), String> {
        self.generic.prepare_texture(source)
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
            match &self.target {
                DrawTarget::Window => {
                    self.gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
                    self.gl.draw_buffers(&[glow::BACK]);
                }
                DrawTarget::Texture(output) => {
                    self.gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(output.framebuffer()));
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
        let y = match &self.target {
            DrawTarget::Window => (self.height as i32).saturating_sub(clip.y.saturating_add(clip.height)),
            DrawTarget::Texture(_) => clip.y,
        };
        Some((x, y, clip.width, clip.height))
    }

    /// Complete host-owned output clear. The caller installs target work ownership before issuing it.
    fn begin(&mut self, info: FrameInfo) -> Result<(), FrameError> {
        let size = info.dimensions();
        self.width = size.width as u32;
        self.height = size.height as u32;
        let color = info.clear();
        // SAFETY: The selected live target and full clip belong to this exclusively borrowed driver.
        unsafe {
            self.bind_target(self.full_clip());
            self.gl
                .clear_color(color.r as f32 / 255., color.g as f32 / 255., color.b as f32 / 255., color.a as f32 / 255.);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        self.generic.check().map_err(FrameError::new)
    }
}
/// Exclusively borrowed immediate frame. Drop abandons publication and services already-issued work.
#[must_use = "call finish to observe rendering errors and obtain a completed image"]
pub struct GlFrame<'a> {
    /// Context, selected target and completion tracker remain owned by this execution transaction.
    backend: &'a mut GLRenderer,
    /// Preserve the first native failure even if safe final maintenance also reports a failure.
    error: Option<FrameError>,
}
impl GlFrame<'_> {
    /// Executes only prevalidated immutable native objects at their original painter position.
    pub fn execute_block(&mut self, block: &PreparedBlock) {
        if self.error.is_some() {
            return;
        }
        if let Err(error) = self.backend.core.current() {
            self.error = Some(FrameError::new(error));
            return;
        }
        self.backend.bind_target(self.backend.full_clip());
        let size = Size {
            width: self.backend.width as i32,
            height: self.backend.height as i32,
        };
        let offscreen = matches!(&self.backend.target, DrawTarget::Texture(_));
        if let Err(error) = self.backend.generic.execute(block, size, size, offscreen) {
            self.error = Some(FrameError::new(error));
        }
    }
}
impl RendererFrame for GlFrame<'_> {
    type Image = Rc<Image>;
    fn finish(mut self) -> Result<Option<Rc<Image>>, FrameError> {
        let completion = self.backend.generic.complete().map_err(FrameError::new);
        if let Some(error) = self.error.take() {
            let _ = completion;
            return Err(error);
        }
        completion?;
        Ok(match &self.backend.target {
            DrawTarget::Window => None,
            DrawTarget::Texture(output) => Some(output.image.clone()),
        })
    }
}
impl Drop for GlFrame<'_> {
    fn drop(&mut self) {
        self.backend.target = DrawTarget::Window;
        let _ = self.backend.generic.maintain();
    }
}
impl RendererBackend for GLRenderer {
    type Image = Rc<Image>;
    type Output = Output;
    type Frame<'a> = GlFrame<'a>;
    fn frame(&mut self, info: FrameInfo) -> Result<GlFrame<'_>, FrameError> {
        self.generic.collect().map_err(FrameError::new)?;
        self.target = DrawTarget::Window;
        if let Err(error) = self.begin(info) {
            self.target = DrawTarget::Window;
            let _ = self.generic.maintain();
            return Err(error);
        }
        Ok(GlFrame { backend: self, error: None })
    }
    fn create_render_texture(&mut self, size: Size) -> Result<Output, TextureError> {
        neocogi_render_core::checked_rgba_len(size.width, size.height)?;
        self.generic.maintain().map_err(TextureError::backend)?;
        // The image stays inaccessible in the single-use Output until the complete host clear.
        let image = self.generic.create_image(size, glow::RGBA8, None, None, false).map_err(TextureError::backend)?;
        let gl = self.gl.clone();
        let core = self.core.clone();
        // SAFETY: The matching current context owns both the guarded framebuffer and image.
        let framebuffer = unsafe {
            let cleanup = core.clone();
            let framebuffer = ResourceGuard::new(gl.create_framebuffer().map_err(TextureError::backend)?, move |raw| {
                if cleanup.for_drop() {
                    cleanup.dispatch().delete_framebuffer(raw);
                }
            });
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(*framebuffer.get()));
            gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(image.raw), 0);
            #[cfg(test)]
            if std::mem::take(&mut self.incomplete_output) {
                gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, None, 0);
            }
            gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
            let status = gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                return Err(TextureError::backend(format!("incomplete output framebuffer: 0x{status:04x}")));
            }
            framebuffer.into_inner()
        };
        Ok(Output {
            state: Rc::new(OutputState {
                allocation: Some(FramebufferAllocation {
                    core,
                    raw: framebuffer,
                    _image: image.allocation.clone(),
                }),
                image,
                retired: self.generic.sender(),
            }),
        })
    }
    fn texture_frame(&mut self, info: FrameInfo, output: Output) -> Result<GlFrame<'_>, FrameError> {
        let dimensions = info.dimensions();
        if !Rc::ptr_eq(&self.core, &output.state.image.core)
            || output.state.image.size
                != (Size {
                    width: dimensions.width,
                    height: dimensions.height,
                })
            || info.clear().a != 255
        {
            return Err(FrameError::new("incompatible output driver, extent or clear"));
        }
        self.generic.collect().map_err(FrameError::new)?;
        self.generic.retain_target(output.state.clone()).map_err(FrameError::new)?;
        self.target = DrawTarget::Texture(output.state);
        if let Err(error) = self.begin(info) {
            self.target = DrawTarget::Window;
            let _ = self.generic.maintain();
            return Err(error);
        }
        Ok(GlFrame { backend: self, error: None })
    }
    fn maintain(&mut self) -> Result<(), FrameError> {
        self.generic.maintain().map_err(FrameError::new)
    }
    fn create_texture(&mut self, size: Size, pixels: Option<&[u8]>) -> Result<Rc<Image>, TextureError> {
        match pixels {
            Some(pixels) => ImageSource {
                width: size.width,
                height: size.height,
                pixels,
            }
            .validate()?,
            None => {
                neocogi_render_core::checked_rgba_len(size.width, size.height)?;
            }
        }
        // Independent image admission can reclaim idle packets before allocating new native storage.
        self.generic.maintain().map_err(TextureError::backend)?;
        self.generic.create_image(size, glow::RGBA8, pixels, None, true).map_err(TextureError::backend)
    }
}

/// Compiles and links a GL program from vertex/fragment shader sources.
fn create_program(gl: &glow::Context, vertex_shader_source: &str, fragment_shader_source: &str) -> Result<Program, io::Error> {
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
