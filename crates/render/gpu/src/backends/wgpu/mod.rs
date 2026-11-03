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
use std::{
    rc::Rc,
    ops::Deref,
    sync::{
        Arc,
        mpsc::{Sender, SendError},
        atomic::{AtomicBool, Ordering},
    },
};
use neocogi_render_core::{
    abi::Size,
    source::{DriverIdentity, new_driver_identity},
};
use crate::resource::BackingCharge;
use neocogi_render_core::prelude::*;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use sdl2::video::Window;
mod generic;
pub use generic::{Backend, PreparedBlock};
/// One fixed native depth format is an executor detail, never client authority.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

/// Physical device lifetime and authoritative terminal availability signal; no driver, queue endpoints,
/// logical registrations, native realization maps or in-flight owners are retained here.
pub struct Core {
    /// WGPU device owns the underlying native dispatch and allocation lifetime.
    device: wgpu::Device,
    /// Native submission queue belongs to the same compatible device instance.
    queue: wgpu::Queue,
    /// Inspection token cannot retain this native owner.
    identity: DriverIdentity,
    /// Native loss or uncertain initialization submission permanently invalidates this compatible core.
    unavailable: Arc<AtomicBool>,
}
impl Core {
    /// Takes one compatible device/queue pair. The device-loss callback belongs exclusively to Core.
    pub(in crate::backends::wgpu) fn new(device: wgpu::Device, queue: wgpu::Queue) -> Rc<Self> {
        let unavailable = Arc::new(AtomicBool::new(false));
        let signal = unavailable.clone();
        device.set_device_lost_callback(move |_, _| signal.store(true, Ordering::Release));
        Rc::new(Self {
            device,
            queue,
            identity: new_driver_identity(),
            unavailable,
        })
    }
    /// Native loss is terminal: old Ready slots cannot be reused on a replacement device.
    fn check(&self) -> Result<(), String> {
        if self.unavailable.load(Ordering::Acquire) {
            Err("WGPU core is unavailable after device loss or uncertain submission".into())
        } else {
            Ok(())
        }
    }
    /// A failed submit may have consumed pending writes. No compatible facade can reuse old slots.
    fn fail_submission(&self) {
        self.unavailable.store(true, Ordering::Release);
    }
    /// Owns all native construction diagnostics before publishing a candidate in its version slot.
    fn scoped<T>(&self, operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        self.check()?;
        let scopes =
            [wgpu::ErrorFilter::OutOfMemory, wgpu::ErrorFilter::Internal, wgpu::ErrorFilter::Validation].map(|filter| self.device.push_error_scope(filter));
        let result = operation();
        let mut failure = None;
        for scope in scopes.into_iter().rev() {
            if let Some(error) = pollster::block_on(scope.pop()) {
                failure.get_or_insert_with(|| error.to_string());
            }
        }
        match (failure, result) {
            (Some(error), _) | (_, Err(error)) => Err(error),
            (None, Ok(value)) => {
                self.check()?;
                Ok(value)
            }
        }
    }
    /// Rejects unrepresentable extents before configure or attachment allocation can panic.
    fn extent(&self, width: u32, height: u32) -> Result<(), String> {
        let max = self.device.limits().max_texture_dimension_2d;
        if width == 0 || height == 0 || width > max || height > max {
            Err(format!("WGPU extent {width}x{height} exceeds supported nonzero 2D limit {max}"))
        } else {
            self.check()
        }
    }
}
/// Sender-free owned retirement packet; no logical source, public COM image or driver back-edge.
pub struct ImageAllocation {
    /// Actual compatible native device lifetime.
    core: Rc<Core>,
    /// Native allocation is retained alongside its read-only view.
    _texture: wgpu::Texture,
    /// Sampling/output view of exactly this allocation.
    view: wgpu::TextureView,
    /// Admitted logical pixel extent.
    size: Size,
    /// Only generic allocation storage owns its original reservation; imports own separate receipts.
    _charge: Option<BackingCharge>,
}
/// Immutable initialized native image. Its last owner notifies the existing driver by owned packet.
pub struct Image {
    /// Taken exactly once on final release; packet itself contains no producer.
    allocation: Option<ImageAllocation>,
    /// Disconnected receiver returns the same owned packet for native-safe late cleanup.
    retired: Sender<generic::Retirement>,
}
impl Deref for Image {
    type Target = ImageAllocation;
    fn deref(&self) -> &ImageAllocation {
        self.allocation.as_ref().expect("live image")
    }
}
impl Drop for Image {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take()
            && let Err(SendError(packet)) = self.retired.send(generic::Retirement::Image(allocation))
        {
            packet.discard();
        }
    }
}
/// Single-use unpublished destination; only successful finish returns its initialized immutable image.
pub struct Output {
    /// Exact native pixels, retained independently of any public image or driver facade.
    image: Rc<Image>,
}
/// Exclusively acquired window or consumed unpublished image.
enum FrameTarget {
    /// Presented only after successful recording, submission and diagnostic resolution.
    Window(wgpu::SurfaceTexture),
    /// The exact attachment also follows submission completion, including zero-block clears.
    Texture(Rc<Image>),
}

/// Depth texture retained beside the surface configuration that determines its dimensions.
struct DepthTarget {
    /// Owning texture; the view alone does not keep the native allocation alive.
    _texture: wgpu::Texture,
    /// View reused across item passes; every pass clears it and discards its completed depth.
    view: wgpu::TextureView,
}

/// Surface configuration is destructively mutable inside WGPU. A failed attempt cannot keep Ready.
enum Presentation {
    /// Complete admitted native surface/depth assembly.
    Ready { config: wgpu::SurfaceConfiguration, depth: DepthTarget },
    /// Previous successful settings are a retry template, never proof that native presentation exists.
    NeedsConfiguration { config: wgpu::SurfaceConfiguration },
}
impl Presentation {
    /// Format/settings remain inspectable without claiming that an acquisition is legal.
    fn config(&self) -> &wgpu::SurfaceConfiguration {
        match self {
            Self::Ready { config, .. } | Self::NeedsConfiguration { config } => config,
        }
    }
    /// Remove readiness before any native call that may destructively replace presentation storage.
    fn invalidate(&mut self) {
        *self = Self::NeedsConfiguration { config: self.config().clone() };
    }
    /// Only an acquired window frame with a complete successful configuration can use depth.
    fn depth(&self) -> Result<&DepthTarget, FrameError> {
        match self {
            Self::Ready { depth, .. } => Ok(depth),
            Self::NeedsConfiguration { .. } => Err(FrameError::new("WGPU presentation needs configuration")),
        }
    }
}

/// One native device executes UI and client blocks through the same executor.
pub struct WgpuRenderer {
    /// Native dispatch and surface assembly outlive the executor and pending submissions.
    _instance: wgpu::Instance,
    /// Native presentation target is independent of offscreen UI outputs.
    surface: wgpu::Surface<'static>,
    /// Selected physical-device owner.
    _adapter: wgpu::Adapter,
    /// Narrow actual device identity/lifetime, independent from facade and surface ownership.
    core: Rc<Core>,
    /// One authoritative configuration/readiness owner, including recovery after native configure errors.
    presentation: Presentation,
    /// Fail after actual native configure to exercise destructive-error recovery, without driver OOM.
    #[cfg(test)]
    configuration_failure: bool,
    /// Inject actual native NotConfigured presentation failure after successful frame submission.
    #[cfg(test)]
    present_failure: bool,
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
    /// Every acquisition owns its diagnostics, including an unexpected native NotConfigured result.
    fn acquire_surface_frame(&mut self) -> Result<wgpu::SurfaceTexture, FrameError> {
        let status = self.acquire()?;
        match status {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                let config = self.presentation.config();
                let (width, height) = (config.width, config.height);
                self.reconfigure_surface(width, height)?;
                match self.acquire()? {
                    wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
                    status => Err(FrameError::new(format!("failed to acquire WGPU frame after reconfigure: {status:?}"))),
                }
            }
            status => Err(FrameError::new(format!("failed to acquire WGPU frame: {status:?}"))),
        }
    }
    /// Scope unexpected native acquisition failures and invalidate the known-unusable assembly.
    fn acquire(&mut self) -> Result<wgpu::CurrentSurfaceTexture, FrameError> {
        match self.core.scoped(|| Ok(self.surface.get_current_texture())) {
            Ok(status) => Ok(status),
            Err(error) => {
                self.presentation.invalidate();
                Err(FrameError::new(error))
            }
        }
    }
    /// Checked size errors preserve Ready because no destructive native work has begun.
    fn configure_surface(&mut self, width: u32, height: u32) -> Result<(), FrameError> {
        self.core.extent(width, height).map_err(FrameError::new)?;
        if matches!(&self.presentation,Presentation::Ready{config,..} if (config.width,config.height)==(width,height)) {
            return Ok(());
        }
        self.reconfigure_surface(width, height)
    }
    /// Resize and lost-surface recovery use the same explicit Ready -> NeedsConfiguration -> Ready rule.
    fn reconfigure_surface(&mut self, width: u32, height: u32) -> Result<(), FrameError> {
        self.core.extent(width, height).map_err(FrameError::new)?;
        let mut config = self.presentation.config().clone();
        config.width = width;
        config.height = height;
        self.presentation.invalidate();
        let depth = Self::configure_target(&self.core, &self.surface, &config).map_err(FrameError::new)?;
        #[cfg(test)]
        if std::mem::take(&mut self.configuration_failure) {
            return Err(FrameError::new("injected failure after native configure"));
        }
        self.presentation = Presentation::Ready { config, depth };
        Ok(())
    }

    /// One fallible target transaction is shared by construction, resize and lost-surface recovery.
    fn configure_target(core: &Core, surface: &wgpu::Surface<'_>, config: &wgpu::SurfaceConfiguration) -> Result<DepthTarget, String> {
        core.extent(config.width, config.height)?;
        core.scoped(|| {
            let depth = Self::create_depth_target(core, config.width, config.height)?;
            surface.configure(&core.device, config);
            Ok(depth)
        })
    }

    /// Checked depth allocation also serves headless native pass tests without a surface.
    fn create_depth_target(core: &Core, width: u32, height: u32) -> Result<DepthTarget, String> {
        core.extent(width, height)?;
        core.scoped(|| {
            let texture = core.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("neocogi.depth"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            Ok(DepthTarget { _texture: texture, view })
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

        let core = Core::new(device, queue);
        core.extent(width, height)?;
        // Surface configuration defines the swapchain textures wgpu will present into.
        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or_else(|| "surface is not supported by adapter".to_string())?;
        if config.format.is_srgb() {
            // Shader already outputs linearized UI colors; keep swapchain format non-sRGB.
            config.format = config.format.remove_srgb_suffix();
        }
        let depth_target = Self::configure_target(&core, &surface, &config)?;
        let generic = generic::Executor::new(core.clone());
        let renderer = Self {
            _instance: instance,
            surface,
            _adapter: adapter,
            core,
            presentation: Presentation::Ready { config, depth: depth_target },
            #[cfg(test)]
            configuration_failure: false,
            #[cfg(test)]
            present_failure: false,
            commands: Vec::new(),
            width,
            height,
            clear_color: color(0, 0, 0, 255),
            generic,
            _window: window,
        };
        Ok(renderer)
    }

    /// Shared physical lifetime has no input, renderer, submissions or channel receiver authority.
    pub fn core(&self) -> Rc<Core> {
        self.core.clone()
    }
    /// Realizes resources before target acquisition using its concrete attachment signature.
    pub fn prepare_block(&mut self, block: &crate::record::Block<Backend>, offscreen: bool) -> Result<PreparedBlock, String> {
        let format = if offscreen {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            self.presentation.config().format
        };
        self.generic.prepare(block, format, !offscreen)
    }

    /// Trusted private UI uploads complete their native error transaction before asset publication.
    pub fn prepare_texture(&mut self, texture: std::rc::Rc<crate::resource::TextureVersion<Backend>>) -> Result<(), String> {
        self.generic.prepare_texture(texture)
    }

    /// Begin owns only frame metadata and generic contributions, never UI vertices or uniforms.
    fn begin(&mut self, width: i32, height: i32, clear: Color) {
        self.width = width as u32;
        self.height = height as u32;
        self.clear_color = clear;
        self.commands.clear();
    }
    /// Owns WGPU diagnostics before publishing an output or presenting a native frame.
    fn finish(&mut self, target: FrameTarget) -> Result<Option<Rc<Image>>, FrameError> {
        self.core.check().map_err(FrameError::new)?;
        let core = self.core.clone();
        let mut present = None;
        let result = core
            .scoped(|| self.finish_target(target, &mut present).map_err(|error| error.to_string()))
            .map_err(FrameError::new)?;
        if let Some(frame) = present {
            self.present(frame)?;
        }
        Ok(result)
    }

    /// Present only after recording/submission scopes succeeded, then own presentation diagnostics too.
    fn present(&mut self, frame: wgpu::SurfaceTexture) -> Result<(), FrameError> {
        #[cfg(test)]
        if std::mem::take(&mut self.present_failure) {
            // WGPU removes presentation before rejecting an existing acquired output. Deliberately
            // reach that native prefix to exercise real present-error propagation, not a fake Result.
            let config = self.presentation.config().clone();
            let rejected = self.core.scoped(|| {
                self.surface.configure(&self.core.device, &config);
                Ok(())
            });
            assert!(rejected.is_err(), "fixture requires an acquired output");
        }
        let result = self.core.scoped(|| {
            self.core.queue.present(frame);
            Ok(())
        });
        if result.is_err() {
            self.presentation.invalidate();
        }
        result.map_err(FrameError::new)
    }

    /// Retains actual attachment and blocks before uncertain submit, even for a clear-only frame.
    fn finish_target(&mut self, target: FrameTarget, present: &mut Option<wgpu::SurfaceTexture>) -> Result<Option<Rc<Image>>, FrameError> {
        let view = match &target {
            FrameTarget::Window(frame) => frame.texture.create_view(&Default::default()),
            FrameTarget::Texture(image) => image.view.clone(),
        };
        let has_depth = matches!(target, FrameTarget::Window(_));
        let mut encoder = self
            .core
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("neocogi.encoder") });
        self.record_commands(&mut encoder, &view, has_depth)?;
        let blocks = std::mem::take(&mut self.commands);
        let attachments = match &target {
            FrameTarget::Texture(image) => vec![image.clone()],
            FrameTarget::Window(_) => Vec::new(),
        };
        self.generic
            .submit(Some(encoder.finish()), blocks, attachments, Vec::new())
            .map_err(FrameError::new)?;
        match target {
            FrameTarget::Window(frame) => {
                *present = Some(frame);
                Ok(None)
            }
            FrameTarget::Texture(image) => Ok(Some(image)),
        }
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
                        view: &self.presentation.depth()?.view,
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
                        crate::abi::Size {
                            width: self.width as i32,
                            height: self.height as i32,
                        },
                        crate::abi::Size {
                            width: self.width as i32,
                            height: self.height as i32,
                        },
                    )
                    .map_err(FrameError::new)?;
            }
        }
        Ok(())
    }
}

/// Exclusively acquired target; only finish submits and permits presentation.
#[must_use = "call finish to submit the WGPU frame and observe errors"]
pub struct WgpuFrame<'a> {
    /// Exclusive current-thread backend ownership.
    backend: &'a mut WgpuRenderer,
    /// Taken once by finish; dropping an acquired window abandons presentation.
    target: Option<FrameTarget>,
    /// The first incompatible prepared block prevents all command submission.
    error: Option<FrameError>,
}
impl WgpuFrame<'_> {
    /// Retains realized blocks at their original painter positions without UI-specific batching.
    pub fn execute_block(&mut self, block: &PreparedBlock) {
        if self.error.is_some() {
            return;
        }
        let signature = match self.target.as_ref().expect("live target") {
            FrameTarget::Window(_) => (self.backend.presentation.config().format, true),
            FrameTarget::Texture(_) => (wgpu::TextureFormat::Rgba8Unorm, false),
        };
        match self.backend.generic.admit(block, signature) {
            Ok(()) => self.backend.commands.push(block.clone()),
            Err(error) => self.error = Some(FrameError::new(error)),
        }
    }
}
impl RendererFrame for WgpuFrame<'_> {
    type Image = Rc<Image>;
    /// Submit the whole selected target and observe all native error scopes before presentation.
    fn finish(mut self) -> Result<Option<Rc<Image>>, FrameError> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        let target = self.target.take().expect("active frame has an explicit target");
        self.backend.finish(target)
    }
}
impl Drop for WgpuFrame<'_> {
    /// Abandonment releases CPU commands, then services safe completion/retirement without submission.
    fn drop(&mut self) {
        self.target.take();
        self.backend.commands.clear();
        let _ = self.backend.generic.collect();
    }
}
impl RendererBackend for WgpuRenderer {
    type Image = Rc<Image>;
    type Output = Output;
    type Frame<'a> = WgpuFrame<'a>;
    fn frame(&mut self, info: FrameInfo) -> Result<Self::Frame<'_>, FrameError> {
        self.generic.collect().map_err(FrameError::new)?;
        let dimensions = info.dimensions();
        self.configure_surface(dimensions.width as u32, dimensions.height as u32)?;
        self.begin(dimensions.width, dimensions.height, info.clear());
        let surface_frame = self.acquire_surface_frame()?;
        Ok(WgpuFrame {
            backend: self,
            target: Some(FrameTarget::Window(surface_frame)),
            error: None,
        })
    }

    /// Allocates a private single-use destination without an unnecessary CPU zero image.
    fn create_render_texture(&mut self, size: Size) -> Result<Output, TextureError> {
        neocogi_render_core::checked_rgba_len(size.width, size.height)?;
        self.generic
            .image(size, wgpu::TextureFormat::Rgba8Unorm, None, None, true)
            .map(|image| Output { image })
            .map_err(TextureError::backend)
    }
    /// Consumes only a compatible destination; a completed image cannot be passed here again.
    fn texture_frame(&mut self, info: FrameInfo, output: Output) -> Result<WgpuFrame<'_>, FrameError> {
        let size = info.dimensions();
        if !Rc::ptr_eq(&self.core, &output.image.core)
            || info.clear().a != 255
            || (size.width, size.height) != (output.image.size.width, output.image.size.height)
        {
            return Err(FrameError::new("invalid WGPU output"));
        }
        self.generic.collect().map_err(FrameError::new)?;
        self.begin(size.width, size.height, info.clear());
        Ok(WgpuFrame {
            backend: self,
            target: Some(FrameTarget::Texture(output.image)),
            error: None,
        })
    }
    /// Completion and retirement never acquire, clear or present a target.
    fn maintain(&mut self) -> Result<(), FrameError> {
        self.generic.collect().map_err(FrameError::new)
    }
    /// Absent ordinary contents have WGPU's guaranteed defined zero initialization.
    fn create_texture(&mut self, size: Size, pixels: Option<&[u8]>) -> Result<Rc<Image>, TextureError> {
        if let Some(pixels) = pixels {
            ImageSource {
                width: size.width,
                height: size.height,
                pixels,
            }
            .validate()?;
        } else {
            neocogi_render_core::checked_rgba_len(size.width, size.height)?;
        }
        self.generic
            .image(size, wgpu::TextureFormat::Rgba8Unorm, pixels, None, false)
            .map_err(TextureError::backend)
    }
}

#[cfg(test)]
mod tests;
