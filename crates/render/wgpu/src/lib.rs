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
//! Native WGPU presentation, immutable images and sealed generic graphics execution.
use std::collections::HashMap;
use neocogi_render_core::prelude::*;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use sdl2::video::Window;
mod generic;
pub use generic::PreparedBlock;
/// One fixed native depth format is an executor detail, never client authority.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

/// Immutable image owner shared by ordinary image capabilities and private UI imports.
struct GpuTexture {
    /// Native allocation outlives each cloned view or generic realization.
    _texture: wgpu::Texture,
    /// Prepared output or sampling view; no UI uniform/sampler binding is stored here.
    view: wgpu::TextureView,
    /// Only freshly allocated output storage can be selected for rendering.
    render_target: bool,
}

/// Exclusively acquired window or fresh unpublished image.
enum FrameTarget {
    /// Native surface texture is presented only after successful explicit finish.
    Window(wgpu::SurfaceTexture),
    /// UI-only output has no depth attachment or presentation operation.
    Texture {
        /// Native key already installed in this device's image map.
        output: TextureId,
    },
}

/// Depth texture retained beside the surface configuration that determines its dimensions.
struct DepthTarget {
    /// Owning texture; the view alone does not keep the native allocation alive.
    _texture: wgpu::Texture,
    /// View reused across item passes; every pass clears it and discards its completed depth.
    view: wgpu::TextureView,
}

/// One native device executes UI and client blocks through the same executor.
pub struct WgpuRenderer {
    /// Canonical immutable image ownership; generic imports clone native backings.
    textures: HashMap<TextureId, GpuTexture>,
    /// Native dispatch and surface assembly outlive the executor and pending submissions.
    _instance: wgpu::Instance,
    /// Native presentation target is independent of offscreen UI outputs.
    surface: wgpu::Surface<'static>,
    /// Selected physical-device owner.
    _adapter: wgpu::Adapter,
    /// Current-thread logical native device.
    device: wgpu::Device,
    /// Submission/initialization/completion queue shared by every resource kind.
    queue: wgpu::Queue,
    /// Current physical presentation dimensions and attachment format.
    config: wgpu::SurfaceConfiguration,
    /// Window depth is shared within a frame and initialized regionally for each client view.
    depth_target: DepthTarget,
    /// Complete realized blocks in painter order; abandonment clears without submission.
    commands: Vec<PreparedBlock>,
    /// Physical dimensions of the currently selected output.
    width: u32,
    /// Physical dimensions of the currently selected output.
    height: u32,
    /// Host attachment-clear color, independent of regional client commands.
    clear_color: Color,
    /// Sole native graphics execution/cache/completion owner.
    generic: generic::Executor,
    /// Keep last: SDL native handles outlive surfaces and every native resource on success and failure.
    _window: Window,
}
impl WgpuRenderer {
    /// Acquires the native surface texture before any display-list execution begins.
    fn acquire_surface_frame(&mut self) -> Result<wgpu::SurfaceTexture, FrameError> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
                    status => Err(FrameError::new(format!("failed to acquire WGPU frame after reconfigure: {status:?}"))),
                }
            }
            status => Err(FrameError::new(format!("failed to acquire WGPU frame: {status:?}"))),
        }
    }

    /// Reconfigures the surface when the requested frame size changes.
    fn configure_surface(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }

        if self.config.width != width || self.config.height != height {
            // Surface reconfigure is required on resize before acquiring next frame.
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
            // The depth allocation must exactly match the newly configured surface. Replacing the
            // complete owner keeps the old view alive until the new texture and view both exist.
            self.depth_target = Self::create_depth_target(&self.device, width, height);
        }
    }

    /// Allocates the depth attachment shared by UI and application rendering for one surface size.
    fn create_depth_target(device: &wgpu::Device, width: u32, height: u32) -> DepthTarget {
        // FrameInfo rejects non-positive logical dimensions, but construction and surface recovery
        // still defensively clamp native allocations to WGPU's non-zero texture requirement.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("neocogi.ui.application.depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        DepthTarget { _texture: texture, view }
    }

    fn create_gpu_texture(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32, pixels: Option<&[u8]>) -> Result<GpuTexture, String> {
        if width == 0 || height == 0 {
            return Err("texture dimensions must be non-zero".into());
        }
        let limit = device.limits().max_texture_dimension_2d;
        if width > limit || height > limit {
            return Err(format!("texture {width}x{height} exceeds the device's 2D dimension limit {limit}"));
        }

        // Capture all native creation/upload failures before publishing a texture owner. The
        // native backend completes these scopes synchronously; no browser event loop is blocked.
        let scopes = [
            device.push_error_scope(wgpu::ErrorFilter::OutOfMemory),
            device.push_error_scope(wgpu::ErrorFilter::Internal),
            device.push_error_scope(wgpu::ErrorFilter::Validation),
        ];

        // All UI/user textures share the same simple RGBA8 + nearest sampling setup.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("neocogi.ui.texture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | match pixels {
                    Some(_) => wgpu::TextureUsages::COPY_DST,
                    None => wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                },
            view_formats: &[],
        });

        if let Some(bytes) = pixels {
            // Upload the initial texel payload during texture construction.
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width.saturating_mul(4)),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            );
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Pop every scope in reverse order even when one failed, observing each result before
        // later operations begin rather than silently discarding diagnostics through guard Drop.
        let mut failure = None;
        for scope in scopes.into_iter().rev() {
            if let Some(error) = pollster::block_on(scope.pop()) {
                failure.get_or_insert(error);
            }
        }
        if let Some(error) = failure {
            return Err(format!("WGPU texture creation or upload failed: {error}"));
        }
        Ok(GpuTexture {
            _texture: texture,
            view,
            render_target: pixels.is_none(),
        })
    }

    /// Creates one native device, presentation target and common graphics executor.
    ///
    /// Retains an SDL window clone, so dropping the caller's window before the renderer is safe.
    pub fn new(window: &Window, width: u32, height: u32) -> Result<Self, String> {
        // SDL's raw-window-handle adapter panics for these software-only video
        // drivers. Reject them before crossing a non-unwinding plugin boundary.
        match window.subsystem().current_video_driver() {
            "offscreen" | "dummy" => return Err("WGPU requires an SDL driver with native window handles".to_owned()),
            _ => {}
        }
        // Declare the native owner before GPU locals so failure unwinding also drops surfaces first.
        let window = window.clone();
        let instance = wgpu::Instance::default();

        let raw_window_handle = window
            .window_handle()
            .map_err(|err| format!("failed to get raw window handle: {err}"))?
            .as_raw();
        let raw_display_handle = window
            .display_handle()
            .map_err(|err| format!("failed to get raw display handle: {err}"))?
            .as_raw();

        // SAFETY: the local SDL owner keeps both raw handles alive on all construction paths. On
        // success it moves into the renderer's last field, outliving the stored 'static surface.
        let surface = unsafe {
            instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(raw_display_handle),
                    raw_window_handle,
                })
                .map_err(|err| format!("failed to create wgpu surface: {err}"))?
        };

        // Pick one adapter/device pair up front; the examples do not hot-switch adapters.
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|err| format!("failed to request wgpu adapter: {err}"))?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("neocogi.ui.wgpu.device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|err| format!("failed to request wgpu device: {err}"))?;

        // Surface configuration defines the swapchain textures wgpu will present into.
        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or_else(|| "surface is not supported by adapter".to_string())?;
        if config.format.is_srgb() {
            // Shader already outputs linearized UI colors; keep swapchain format non-sRGB.
            config.format = config.format.remove_srgb_suffix();
        }
        surface.configure(&device, &config);

        let depth_target = Self::create_depth_target(&device, config.width, config.height);
        let generic = generic::Executor::new(&device, &queue);
        let renderer = Self {
            textures: HashMap::new(),

            _instance: instance,
            surface,
            _adapter: adapter,
            device,
            queue,
            config,

            depth_target,
            commands: Vec::new(),
            width,
            height,
            clear_color: color(0, 0, 0, 255),
            generic,
            _window: window,
        };
        Ok(renderer)
    }

    /// Realizes resources before target acquisition using its concrete attachment signature.
    pub fn prepare_block(&mut self, block: &neocogi_render_gpu::record::Block, offscreen: bool) -> Result<PreparedBlock, String> {
        let format = if offscreen { wgpu::TextureFormat::Rgba8Unorm } else { self.config.format };
        self.generic.prepare(block, format, !offscreen, &self.textures)
    }

    /// Trusted private UI uploads complete their native error transaction before asset publication.
    pub fn prepare_texture(&mut self, texture: std::rc::Rc<neocogi_render_gpu::resource::TextureVersion>) -> Result<(), String> {
        self.generic.prepare_texture(texture, &self.textures)
    }

    /// Begin owns only frame metadata and generic contributions, never UI vertices or uniforms.
    fn begin(&mut self, width: i32, height: i32, clear: Color) {
        self.width = width as u32;
        self.height = height as u32;
        self.clear_color = clear;
        self.commands.clear();
    }
    /// Owns WGPU diagnostics before publishing an output or presenting a native frame.
    fn finish(&mut self, target: FrameTarget) -> Result<(), FrameError> {
        let scopes = [
            self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory),
            self.device.push_error_scope(wgpu::ErrorFilter::Internal),
            self.device.push_error_scope(wgpu::ErrorFilter::Validation),
        ];
        let result = self.finish_target(target);
        let mut failure = None;
        for scope in scopes.into_iter().rev() {
            if let Some(error) = pollster::block_on(scope.pop()) {
                failure.get_or_insert(error);
            }
        }
        match failure {
            Some(error) => Err(FrameError::new(format!("WGPU frame failed: {error}"))),
            None => {
                // Presentation follows successful recording, submission, and error-scope
                // completion. Texture targets have no native image to present.
                if let Some(frame) = result? {
                    self.queue.present(frame);
                }
                Ok(())
            }
        }
    }

    /// Records the same painter-order stream into an explicit window or texture attachment.
    fn finish_target(&mut self, target: FrameTarget) -> Result<Option<wgpu::SurfaceTexture>, FrameError> {
        let view = match &target {
            FrameTarget::Window(frame) => frame.texture.create_view(&wgpu::TextureViewDescriptor::default()),
            FrameTarget::Texture { output } => self.textures[output].view.clone(),
        };
        let has_depth = matches!(&target, FrameTarget::Window(_));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("neocogi.ui.encoder") });

        self.record_commands(&mut encoder, &view, has_depth)?;
        self.queue.submit(Some(encoder.finish()));
        let blocks = self.commands.clone();
        self.generic.submitted(blocks);
        Ok(match target {
            FrameTarget::Window(frame) => Some(frame),
            FrameTarget::Texture { .. } => None,
        })
    }

    /// One pass preserves depth and painter order; each block establishes complete native state.
    fn record_commands(&mut self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, has_depth: bool) -> Result<(), FrameError> {
        // One pass preserves depth between draws and ordered views. Client regional clears
        // initialize only their own roots; they never become attachment-wide load operations.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("neocogi.ui.render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: self.clear_color.r as f64 / 255.0,
                            g: self.clear_color.g as f64 / 255.0,
                            b: self.clear_color.b as f64 / 255.0,
                            a: self.clear_color.a as f64 / 255.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: if has_depth {
                    Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_target.view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            // Generic views share depth within the pass; only regional commands reset it.
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    })
                } else {
                    None
                },
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_scissor_rect(0, 0, self.width, self.height);

            for command in &self.commands {
                self.generic
                    .record(
                        &mut pass,
                        command,
                        neocogi_render_gpu::abi::Size {
                            width: self.width as i32,
                            height: self.height as i32,
                        },
                        neocogi_render_gpu::abi::Size {
                            width: self.width as i32,
                            height: self.height as i32,
                        },
                    )
                    .map_err(FrameError::new)?;
            }
        }
        Ok(())
    }
    /// Creates a backend-owned sampled texture and its bind group.
    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), TextureError> {
        // Validate the actual slice before ownership reservation or native queue work.
        ImageSource {
            width: id.width(),
            height: id.height(),
            pixels,
        }
        .validate()?;
        if self.textures.contains_key(&id) {
            return Err(TextureError::backend("native image key is already resident"));
        }
        // Reserve ownership storage before allocating native resources or replacing a live entry.
        self.textures
            .try_reserve(1)
            .map_err(|error| TextureError::backend(format!("failed to reserve texture ownership entry: {error}")))?;
        // Dimensions and actual storage have now been admitted together.
        let dimensions = id.size();
        // Keep the WGPU helper's useful string diagnostics local, then translate them at the
        // backend boundary so callers receive the crate-wide typed texture error.
        let texture = Self::create_gpu_texture(&self.device, &self.queue, dimensions.width as u32, dimensions.height as u32, Some(pixels))
            .map_err(TextureError::backend)?;
        self.textures.insert(id, texture);
        Ok(())
    }

    /// Drops a backend-owned texture by removing it from the bind-group map.
    fn destroy_texture(&mut self, id: TextureId) {
        self.textures.remove(&id);
    }
}

/// Exclusively acquired target; only finish submits and permits presentation.
#[must_use = "call finish to submit the WGPU frame and observe errors"]
pub struct WgpuFrame<'a> {
    /// Exclusive current-thread backend ownership.
    backend: &'a mut WgpuRenderer,
    /// Taken once by finish; dropping an acquired window abandons presentation.
    target: Option<FrameTarget>,
}
impl WgpuFrame<'_> {
    /// Retains realized blocks at their original painter positions without UI-specific batching.
    pub fn execute_block(&mut self, block: &PreparedBlock) {
        self.backend.commands.push(block.clone());
    }
}
impl RendererFrame for WgpuFrame<'_> {
    /// Submit the whole selected target and observe all native error scopes before presentation.
    fn finish(mut self) -> Result<(), FrameError> {
        let target = self.target.take().expect("active frame has an explicit target");
        self.backend.finish(target)
    }
}
impl Drop for WgpuFrame<'_> {
    /// No queue submit or presentation occurs during abandonment.
    fn drop(&mut self) {
        self.backend.commands.clear();
    }
}
impl RendererBackend for WgpuRenderer {
    type Frame<'a> = WgpuFrame<'a>;
    fn frame(&mut self, info: FrameInfo) -> Result<Self::Frame<'_>, FrameError> {
        self.generic.collect().map_err(FrameError::new)?;
        let dimensions = info.dimensions();
        self.configure_surface(dimensions.width as u32, dimensions.height as u32);
        self.begin(dimensions.width, dimensions.height, info.clear());
        let surface_frame = self.acquire_surface_frame()?;
        Ok(WgpuFrame {
            backend: self,
            target: Some(FrameTarget::Window(surface_frame)),
        })
    }

    /// Allocates output storage without a CPU zero image; WGPU owns in-flight retirement.
    fn create_render_texture(&mut self, id: TextureId) -> Result<(), TextureError> {
        if self.textures.contains_key(&id) {
            return Err(TextureError::backend("render target already exists"));
        }
        self.textures.try_reserve(1).map_err(|error| TextureError::backend(error.to_string()))?;
        let size = id.size();
        let texture = Self::create_gpu_texture(&self.device, &self.queue, size.width as u32, size.height as u32, None).map_err(TextureError::backend)?;
        self.textures.insert(id, texture);
        Ok(())
    }

    /// Begins a UI-only texture pass without touching surface configuration or presentation.
    fn texture_frame(&mut self, info: FrameInfo, output: TextureId) -> Result<Self::Frame<'_>, FrameError> {
        self.generic.collect().map_err(FrameError::new)?;
        let size = info.dimensions();
        if info.clear().a != 255
            || output.size().width != size.width
            || output.size().height != size.height
            || !self.textures.get(&output).is_some_and(|texture| texture.render_target)
        {
            return Err(FrameError::new("invalid WGPU output"));
        }
        self.begin(size.width, size.height, info.clear());
        Ok(WgpuFrame {
            backend: self,
            target: Some(FrameTarget::Texture { output }),
        })
    }
    /// Upload validated ordinary image pixels on this device.
    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), TextureError> {
        self.create_texture(id, pixels)
    }
    /// Native clones and completion receipts own any remaining sampled backing.
    fn destroy_texture(&mut self, id: TextureId) {
        self.destroy_texture(id);
    }
}

#[cfg(test)]
mod tests;
