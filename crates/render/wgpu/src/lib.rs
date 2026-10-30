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
//! WGPU renderer backend used by examples.
//!
//! One device executes bounded generic graphics blocks in painter order and owns native images.

use std::{collections::HashMap, mem, slice};

use neocogi_render_core::prelude::*;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use sdl2::video::Window;
use wgpu::util::DeviceExt;

mod generic;
pub use generic::PreparedBlock;

/// Private depth attachment format; clients declare portable depth state, never native formats.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

// WGPU backend overview:
// - UI/custom draws are described as `RenderCommand`s while building the frame.
// - Painter order shares one color/depth target; each generic view records its own regional depth initialization.
// - The swapchain surface is reconfigured on resize; theme selection replaces the immutable atlas.

#[repr(C)]
#[derive(Clone, Copy, Default)]
/// Fully initialized packed vertex consumed by the WGPU input layout.
struct GpuVertex {
    /// Two screen-space coordinates at byte offset zero.
    pos: [f32; 2],
    /// Two texture coordinates immediately following position.
    uv: [f32; 2],
    /// Four packed color channels complete the record without tail padding.
    color: [u8; 4],
}

impl GpuVertex {
    /// Converts the generic microui vertex layout into the packed wgpu vertex format.
    fn from_vertex(v: &Vertex) -> Self {
        let pos = v.position();
        let uv = v.tex_coord();
        let c = v.color();
        Self {
            pos: [pos.x, pos.y],
            uv: [uv.x, uv.y],
            color: [c.x, c.y, c.z, c.w],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
/// Four initialized floats matching the shader’s sixteen-byte uniform block.
struct Uniforms {
    /// Physical surface width and height used to map pixels to clip coordinates.
    screen_size: [f32; 2],
    /// Explicit initialized padding required by the shader uniform alignment.
    _pad: [f32; 2],
}

impl Uniforms {
    /// Encodes the exact shader block without reading a Rust object representation.
    fn bytes(&self) -> [u8; 16] {
        let mut bytes = [0; 16];
        for (slot, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(self.screen_size.iter().chain(&self._pad)) {
            slot.copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }
}

#[allow(clippy::enum_variant_names)] // The prefix makes queued draw operations explicit at use sites.
/// Painter-ordered backend and application operations retained until the WGPU pass is recorded.
enum RenderCommand {
    /// Draws queued UI vertices through this exclusive index with the atlas texture.
    DrawUiTo(usize),
    /// Draws a pre-clipped quad using a backend-owned texture.
    DrawTexture {
        /// Opaque texture capability resolved through the backend texture map.
        id: TextureId,
        /// Six GPU vertices forming the already-clipped quad.
        vertices: Vec<GpuVertex>,
    },
    /// Draws arbitrary colored triangles through the UI pipeline.
    DrawColored {
        /// Authoritative logical clip converted to a render-pass scissor during preparation.
        clip: Recti,
        /// Pre-transformed triangle-list vertices uploaded with the UI batch.
        vertices: Vec<GpuVertex>,
    },
    /// Executes an authenticated, pre-realized block without lending native capabilities.
    Block {
        /// Native allocation snapshots and device-free capacity receipts.
        block: PreparedBlock,
        /// Trusted frame dimensions defining logical-to-physical mapping.
        logical: neocogi_render_gpu::abi::Size,
    },
}

/// Complete sampled-texture owner published only after upload and binding validation succeed.
struct GpuTexture {
    /// Keeps the texture alive as long as the bind group references its view.
    _texture: wgpu::Texture,
    /// Stored view can become a color attachment only for a render-target allocation.
    view: wgpu::TextureView,
    /// Upload-only textures are never accepted as offscreen render destinations.
    render_target: bool,
    /// Uniform, sampler, and validated texture-view bindings used by UI drawing.
    bind_group: wgpu::BindGroup,
}

/// An acquired presentation image or a UI-only destination owned by the same device.
enum FrameTarget {
    /// This frame alone may present to the native surface.
    Window(wgpu::SurfaceTexture),
    /// Offscreen frames bind their private atlas and never acquire a swapchain image.
    Texture {
        /// Unpublished immutable-output allocation.
        output: TextureId,
        /// Independently retained source atlas.
        atlas: TextureId,
    },
}

/// Depth texture retained beside the surface configuration that determines its dimensions.
struct DepthTarget {
    /// Owning texture; the view alone does not keep the native allocation alive.
    _texture: wgpu::Texture,
    /// View reused across item passes; every pass clears it and discards its completed depth.
    view: wgpu::TextureView,
}

/// WGPU renderer backend for neocogi.ui.
///
/// The renderer batches all per-frame geometry into a single staging vertex buffer and
/// replays a lightweight command list (`RenderCommand`) to switch texture/scissor state.
pub struct WgpuRenderer {
    textures: HashMap<TextureId, GpuTexture>,

    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    /// Window pipeline includes the shared application depth attachment.
    pipeline: wgpu::RenderPipeline,
    /// UI-only output uses linear RGBA8 without a depth allocation.
    texture_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
    atlas_texture: GpuTexture,
    /// Depth target recreated transactionally whenever the surface dimensions change.
    depth_target: DepthTarget,

    ui_vertices: Vec<GpuVertex>,
    commands: Vec<RenderCommand>,
    current_batch_end: usize,

    staging_vertex_buffer: wgpu::Buffer,
    staging_vertex_capacity: usize,

    width: u32,
    height: u32,
    clear_color: Color,
    /// Common native realization and completion retention for every generic block.
    generic: generic::Executor,
    /// Shared SDL owner of the native window and video subsystem used by `surface`.
    ///
    /// Keep this last: Rust drops fields in declaration order, so every GPU surface/resource is
    /// released before the final native-window owner can disappear. SDL's clone shares its
    /// existing window context; it does not allocate a second native window.
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

    /// Exposes only the concrete packed vertex layout, never arbitrary Rust values.
    fn vertex_bytes(values: &[GpuVertex]) -> &[u8] {
        // These compile-time checks cover every byte in the C layout. Adding a field or
        // changing its alignment cannot silently introduce padding into a safe upload.
        const {
            assert!(mem::size_of::<GpuVertex>() == 20);
            assert!(mem::offset_of!(GpuVertex, pos) == 0);
            assert!(mem::offset_of!(GpuVertex, uv) == 8);
            assert!(mem::offset_of!(GpuVertex, color) == 16);
        }
        // SAFETY: GpuVertex consists exclusively of initialized float/channel arrays;
        // the assertions exclude interior/tail padding. The borrowed slice supplies the
        // allocation extent and lifetime; u8 requires no stronger alignment.
        unsafe { slice::from_raw_parts(values.as_ptr().cast::<u8>(), mem::size_of_val(values)) }
    }

    /// Creates a sampled RGBA texture plus the bind group used to draw with it.
    #[allow(clippy::too_many_arguments)] // Texture creation keeps the related WGPU resources explicit.
    fn create_gpu_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bind_group_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        uniform_buffer: &wgpu::Buffer,
        width: u32,
        height: u32,
        pixels: Option<&[u8]>,
    ) -> Result<GpuTexture, String> {
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
        // Bind group also carries uniforms/sampler so draw calls only switch one handle.
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("neocogi.ui.texture.bind_group"),
            layout: bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });

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
            bind_group,
        })
    }

    /// Records the boundary between the current UI batch and later render commands.
    fn flush_ui_batch(&mut self) {
        // UI quads are appended continuously; this records a boundary in command order so
        // later custom draws can interleave correctly.
        let end = self.ui_vertices.len();
        if end > self.current_batch_end {
            self.commands.push(RenderCommand::DrawUiTo(end));
            self.current_batch_end = end;
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

    /// Grows the shared staging vertex buffer to fit a full frame upload.
    fn ensure_staging_capacity(&mut self, byte_count: usize) {
        if byte_count <= self.staging_vertex_capacity {
            return;
        }

        // Grow geometrically to avoid reallocating every frame with fluctuating counts.
        let target = byte_count.next_power_of_two().max(64 * 1024);
        self.staging_vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("neocogi.ui.staging.vertices"),
            size: target as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.staging_vertex_capacity = target;
    }

    /// Converts a signed UI clip rectangle into a clamped wgpu scissor rectangle.
    fn clip_to_scissor(clip: Recti, surface_width: u32, surface_height: u32) -> Option<(u32, u32, u32, u32)> {
        // Convert signed UI clip rect into a clamped, positive scissor rectangle.
        if clip.width <= 0 || clip.height <= 0 {
            return None;
        }

        let x0 = clip.x.max(0).min(surface_width as i32);
        let y0 = clip.y.max(0).min(surface_height as i32);
        let x1 = (clip.x + clip.width).max(0).min(surface_width as i32);
        let y1 = (clip.y + clip.height).max(0).min(surface_height as i32);

        if x1 <= x0 || y1 <= y0 {
            return None;
        }

        Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
    }

    /// Expands one quad into two triangles in the backend vertex format.
    fn append_quad(dst: &mut Vec<GpuVertex>, v0: &Vertex, v1: &Vertex, v2: &Vertex, v3: &Vertex) {
        // Expand quad into two triangles in clockwise draw order.
        dst.extend_from_slice(&[
            GpuVertex::from_vertex(v0),
            GpuVertex::from_vertex(v1),
            GpuVertex::from_vertex(v2),
            GpuVertex::from_vertex(v0),
            GpuVertex::from_vertex(v2),
            GpuVertex::from_vertex(v3),
        ]);
    }

    /// Builds the single render pipeline shared by atlas, textured, and colored UI draws.
    fn create_pipeline(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (wgpu::RenderPipeline, wgpu::RenderPipeline, wgpu::BindGroupLayout, wgpu::Sampler) {
        // One pipeline is enough for all UI/custom draws: position + uv + color, alpha blend.
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("neocogi.ui.wgpu.shader"),
            source: wgpu::ShaderSource::Wgsl(UI_SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("neocogi.ui.bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("neocogi.ui.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("neocogi.ui.pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // Both pipelines share this exact layout, so every retained sampled image can
        // be used in either target without rebinding or uploading its source pixels.
        let build_pipeline = |format, depth| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("neocogi.ui.pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: mem::size_of::<GpuVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 8,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Unorm8x4,
                                offset: 16,
                                shader_location: 2,
                            },
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                // The shared pass always carries the application depth target. WGPU requires every
                // pipeline used in that pass to declare a compatible attachment format even when UI
                // rendering deliberately performs no depth reads or writes.
                depth_stencil: if depth {
                    Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    })
                } else {
                    None
                },
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview_mask: None,
                cache: None,
            })
        };

        (
            build_pipeline(format, true),
            build_pipeline(wgpu::TextureFormat::Rgba8Unorm, false),
            bind_group_layout,
            sampler,
        )
    }

    /// Creates one native device, presentation target and common graphics executor.
    ///
    /// Retains an SDL window clone, so dropping the caller's window before the renderer is safe.
    pub fn new(window: &Window, atlas: ImageSource<'_>, width: u32, height: u32) -> Result<Self, String> {
        atlas.validate().map_err(|error| error.to_string())?;
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

        let (pipeline, texture_pipeline, bind_group_layout, sampler) = Self::create_pipeline(&device, config.format);
        let uniforms = Uniforms {
            screen_size: [width.max(1) as f32, height.max(1) as f32],
            _pad: [0.0, 0.0],
        };
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("neocogi.ui.uniforms"),
            contents: &uniforms.bytes(),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let atlas_pixels = atlas.pixels;
        let atlas_texture = Self::create_gpu_texture(
            &device,
            &queue,
            &bind_group_layout,
            &sampler,
            &uniform_buffer,
            atlas.width as u32,
            atlas.height as u32,
            Some(atlas_pixels),
        )?;
        let depth_target = Self::create_depth_target(&device, config.width, config.height);

        let initial_capacity = 64 * 1024;
        let staging_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("neocogi.ui.staging.vertices"),
            size: initial_capacity as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let generic = generic::Executor::new(&device, &queue);
        let renderer = Self {
            textures: HashMap::new(),

            _instance: instance,
            surface,
            _adapter: adapter,
            device,
            queue,
            config,

            pipeline,
            texture_pipeline,
            bind_group_layout,
            sampler,
            uniform_buffer,
            atlas_texture,
            depth_target,

            ui_vertices: Vec::new(),
            commands: Vec::new(),
            current_batch_end: 0,

            staging_vertex_buffer,
            staging_vertex_capacity: initial_capacity,

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

    /// Queues arbitrary colored triangles that should still respect the UI clip space.
    pub fn enqueue_colored_vertices(&mut self, args: RenderItemArgs, vertices: Vec<Vertex>) {
        if vertices.is_empty() {
            return;
        }
        self.flush_ui_batch();
        // Colored draws still use the atlas bind group; their UVs point at the white icon center.
        let vertices = vertices.iter().map(GpuVertex::from_vertex).collect();
        self.commands.push(RenderCommand::DrawColored { clip: args.view, vertices });
    }
}

impl WgpuRenderer {
    /// Resets shared command storage and uniforms without selecting or resizing native presentation.
    fn begin(&mut self, width: i32, height: i32, clr: Color) {
        // Reset per-frame CPU-side command/vertex queues.
        self.width = width.max(0) as u32;
        self.height = height.max(0) as u32;
        self.clear_color = clr;
        self.ui_vertices.clear();
        self.commands.clear();
        self.current_batch_end = 0;

        // Keep screen-size uniform in sync for pixel->NDC conversion in vertex shader.
        let uniforms = Uniforms {
            screen_size: [self.width.max(1) as f32, self.height.max(1) as f32],
            _pad: [0.0, 0.0],
        };
        self.queue.write_buffer(&self.uniform_buffer, 0, &uniforms.bytes());
    }

    /// Appends one UI quad to the CPU-side UI vertex batch.
    fn push_quad_vertices(&mut self, v0: &Vertex, v1: &Vertex, v2: &Vertex, v3: &Vertex) {
        Self::append_quad(&mut self.ui_vertices, v0, v1, v2, v3);
    }

    /// Appends complete triangles to the shared CPU-side UI vertex batch.
    fn push_triangle_vertices(&mut self, vertices: &[Vertex]) {
        // Conversion remains required by WGPU's concrete upload layout, but extending from one
        // iterator lets the vector reserve and advance as a complete prepared range.
        debug_assert!(vertices.len().is_multiple_of(3));
        self.ui_vertices.extend(vertices.iter().map(GpuVertex::from_vertex));
    }

    /// Closes the current UI batch so later commands preserve ordering.
    fn flush(&mut self) {
        self.flush_ui_batch();
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
        self.flush_ui_batch();
        let (view, atlas) = match &target {
            FrameTarget::Window(frame) => (frame.texture.create_view(&wgpu::TextureViewDescriptor::default()), None),
            FrameTarget::Texture { output, atlas } => (self.textures[output].view.clone(), Some(*atlas)),
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("neocogi.ui.encoder") });

        self.record_commands(&mut encoder, &view, atlas)?;
        self.queue.submit(Some(encoder.finish()));
        let blocks = self
            .commands
            .iter()
            .filter_map(|command| match command {
                RenderCommand::Block { block, .. } => Some(block.clone()),
                _ => None,
            })
            .collect();
        self.generic.submitted(blocks);
        Ok(match target {
            FrameTarget::Window(frame) => Some(frame),
            FrameTarget::Texture { .. } => None,
        })
    }

    /// Prepares one ordered command stream and records it into an already selected attachment.
    /// Presentation stays in `finish_target`; regional view commands initialize depth inside their own roots.
    fn record_commands(&mut self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, atlas: Option<TextureId>) -> Result<(), FrameError> {
        /// One validated contiguous slice of this frame's shared vertex upload.
        struct PreparedDraw {
            /// Starting byte in the shared upload buffer.
            vertex_offset: u64,
            /// Complete byte length of this draw's triangle list.
            vertex_size: u64,
            /// Number of vertices submitted from the bound range.
            vertex_count: u32,
            /// Explicit source binding, independent of the native presentation image.
            source: DrawSource,
            /// Optional physical clip for colored application geometry.
            scissor: Option<(u32, u32, u32, u32)>,
        }
        #[derive(Clone, Copy)]
        /// The current session atlas or one independently owned image.
        enum DrawSource {
            /// Resolves to the window atlas or selected offscreen session atlas.
            Atlas,
            /// Resolves through the device's authenticated texture ownership map.
            Texture(TextureId),
        }

        let mut ui_start = 0usize;
        // Keep one optional prepared draw beside every original command. Application commands have
        // no UI upload, but their vacant slot preserves exact interleaving during replay.
        let mut prepared_draws: Vec<Option<PreparedDraw>> = Vec::with_capacity(self.commands.len());
        let mut packed_upload: Vec<u8> = Vec::new();

        for cmd in &self.commands {
            let (vertices, source, scissor) = match cmd {
                RenderCommand::DrawUiTo(end) => {
                    let end = (*end).min(self.ui_vertices.len());
                    if end <= ui_start {
                        prepared_draws.push(None);
                        continue;
                    }
                    let verts = &self.ui_vertices[ui_start..end];
                    ui_start = end;
                    (verts, DrawSource::Atlas, None)
                }
                RenderCommand::DrawTexture { id, vertices } => {
                    if !self.textures.contains_key(id) {
                        prepared_draws.push(None);
                        continue;
                    }
                    (vertices.as_slice(), DrawSource::Texture(*id), None)
                }
                RenderCommand::DrawColored { clip, vertices } => {
                    let scissor = Self::clip_to_scissor(*clip, self.width, self.height);
                    if scissor.is_none() {
                        prepared_draws.push(None);
                        continue;
                    }
                    (vertices.as_slice(), DrawSource::Atlas, scissor)
                }
                RenderCommand::Block { .. } => {
                    prepared_draws.push(None);
                    continue;
                }
            };

            if vertices.is_empty() {
                prepared_draws.push(None);
                continue;
            }

            // Pack every draw's vertex payload into one contiguous upload buffer so we only
            // issue one queue.write_buffer call for the entire frame.
            let bytes = Self::vertex_bytes(vertices);
            let vertex_offset = packed_upload.len() as u64;
            let vertex_size = bytes.len() as u64;
            packed_upload.extend_from_slice(bytes);
            prepared_draws.push(Some(PreparedDraw {
                vertex_offset,
                vertex_size,
                vertex_count: vertices.len() as u32,
                source,
                scissor,
            }));
        }

        if !packed_upload.is_empty() {
            self.ensure_staging_capacity(packed_upload.len());
            self.queue.write_buffer(&self.staging_vertex_buffer, 0, packed_upload.as_slice());
        }

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
                depth_stencil_attachment: if atlas.is_none() {
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

            for (command, prepared) in self.commands.iter().zip(prepared_draws.iter()) {
                match command {
                    RenderCommand::Block { block, logical } => {
                        self.generic
                            .record(
                                &mut pass,
                                block,
                                *logical,
                                neocogi_render_gpu::abi::Size {
                                    width: self.width as i32,
                                    height: self.height as i32,
                                },
                            )
                            .map_err(FrameError::new)?;
                    }
                    RenderCommand::DrawUiTo(_) | RenderCommand::DrawTexture { .. } | RenderCommand::DrawColored { .. } => {
                        let Some(draw) = prepared else {
                            continue;
                        };
                        if let Some((x, y, w, h)) = draw.scissor {
                            pass.set_scissor_rect(x, y, w, h);
                        } else {
                            pass.set_scissor_rect(0, 0, self.width, self.height);
                        }

                        let bind_group = match draw.source {
                            DrawSource::Atlas => match atlas {
                                Some(id) => &self.textures[&id].bind_group,
                                None => &self.atlas_texture.bind_group,
                            },
                            DrawSource::Texture(id) => {
                                let Some(texture) = self.textures.get(&id) else {
                                    continue;
                                };
                                &texture.bind_group
                            }
                        };

                        // Application rendering may have replaced every pass binding, so each UI
                        // draw explicitly restores the complete pipeline and resource state.
                        pass.set_pipeline(match atlas {
                            Some(_) => &self.texture_pipeline,
                            None => &self.pipeline,
                        });
                        pass.set_bind_group(0, bind_group, &[]);
                        pass.set_viewport(0.0, 0.0, self.width as f32, self.height as f32, 0.0, 1.0);
                        pass.set_vertex_buffer(0, self.staging_vertex_buffer.slice(draw.vertex_offset..(draw.vertex_offset + draw.vertex_size)));
                        pass.draw(0..draw.vertex_count, 0..1);
                    }
                }
            }
        }

        Ok(())
    }

    /// Creates a backend-owned sampled texture and its bind group.
    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), TextureError> {
        // Reserve ownership storage before allocating native resources or replacing a live entry.
        self.textures
            .try_reserve(1)
            .map_err(|error| TextureError::backend(format!("failed to reserve texture ownership entry: {error}")))?;
        // The opaque capability is the sole dimension source validated by the Context executor.
        let dimensions = id.size();
        // Keep the WGPU helper's useful string diagnostics local, then translate them at the
        // backend boundary so callers receive the crate-wide typed texture error.
        let texture = Self::create_gpu_texture(
            &self.device,
            &self.queue,
            &self.bind_group_layout,
            &self.sampler,
            &self.uniform_buffer,
            dimensions.width as u32,
            dimensions.height as u32,
            Some(pixels),
        )
        .map_err(TextureError::backend)?;
        self.textures.insert(id, texture);
        Ok(())
    }

    /// Drops a backend-owned texture by removing it from the bind-group map.
    fn destroy_texture(&mut self, id: TextureId) {
        self.textures.remove(&id);
    }

    /// Queues one pre-clipped textured quad that samples from a backend-owned texture.
    fn draw_texture(&mut self, id: TextureId, vertices: [Vertex; 4]) {
        if !self.textures.contains_key(&id) {
            return;
        }

        // The Context executor owns the ordering boundary and has already clipped/adjusted UVs.
        let mut quad = Vec::with_capacity(6);
        Self::append_quad(&mut quad, &vertices[0], &vertices[1], &vertices[2], &vertices[3]);
        self.commands.push(RenderCommand::DrawTexture { id, vertices: quad });
    }
}

#[must_use = "call finish to submit the WGPU frame and observe errors"]
pub struct WgpuFrame<'a> {
    /// Exclusive backend borrow prevents concurrent frame acquisition.
    backend: &'a mut WgpuRenderer,
    /// Explicit target is consumed once; dropping a frame never submits or presents.
    target: Option<FrameTarget>,
}

impl WgpuFrame<'_> {
    /// Inserts realized common commands at this exact painter-order boundary.
    pub fn execute_block(&mut self, block: &PreparedBlock, logical: neocogi_render_gpu::abi::Size) {
        self.backend.flush_ui_batch();
        self.backend.commands.push(RenderCommand::Block { block: block.clone(), logical });
    }

    /// Queues pre-transformed colored vertices using microui's authoritative custom-render clip.
    pub fn enqueue_colored_vertices(&mut self, args: RenderItemArgs, vertices: Vec<Vertex>) {
        self.backend.enqueue_colored_vertices(args, vertices);
    }
}

impl RendererFrame for WgpuFrame<'_> {
    /// Consumes the surface image only through an explicit, observable submit/present operation.
    fn finish(mut self) -> Result<(), FrameError> {
        let target = self.target.take().expect("an active frame owns its explicit target");
        self.backend.finish(target)
    }

    fn push_quad(&mut self, vertices: [Vertex; 4]) {
        self.backend.push_quad_vertices(&vertices[0], &vertices[1], &vertices[2], &vertices[3]);
    }

    fn push_triangle(&mut self, vertices: [Vertex; 3]) {
        self.backend.push_triangle_vertices(&vertices);
    }

    fn push_triangles(&mut self, vertices: &[Vertex]) {
        self.backend.push_triangle_vertices(vertices);
    }

    fn flush(&mut self) {
        self.backend.flush();
    }

    fn draw_texture(&mut self, id: TextureId, vertices: [Vertex; 4]) {
        self.backend.draw_texture(id, vertices);
    }
}

impl Drop for WgpuFrame<'_> {
    fn drop(&mut self) {
        // WGPU releases an unpresented surface image on drop. No encoder is submitted here,
        // including when application code panics while recording or preparing the frame.
        self.backend.commands.clear();
        self.backend.ui_vertices.clear();
        self.backend.current_batch_end = 0;
    }
}

impl RendererBackend for WgpuRenderer {
    type Frame<'a> = WgpuFrame<'a>;

    fn replace_atlas(&mut self, atlas: ImageSource<'_>) -> Result<(), AtlasUploadError> {
        atlas.validate().map_err(|error| AtlasUploadError::new(error.to_string()))?;
        // Construct the complete sampled texture and bind group before publishing either field.
        // Dropping the old GpuTexture after assignment releases it only once the candidate exists.
        let pixels = atlas.pixels;
        let texture = Self::create_gpu_texture(
            &self.device,
            &self.queue,
            &self.bind_group_layout,
            &self.sampler,
            &self.uniform_buffer,
            atlas.width as u32,
            atlas.height as u32,
            Some(pixels),
        )
        .map_err(AtlasUploadError::new)?;
        self.atlas_texture = texture;

        Ok(())
    }

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
        let texture = Self::create_gpu_texture(
            &self.device,
            &self.queue,
            &self.bind_group_layout,
            &self.sampler,
            &self.uniform_buffer,
            size.width as u32,
            size.height as u32,
            None,
        )
        .map_err(TextureError::backend)?;
        self.textures.insert(id, texture);
        Ok(())
    }

    /// Begins a UI-only texture pass without touching surface configuration or presentation.
    fn texture_frame(&mut self, info: FrameInfo, output: TextureId, atlas: TextureId) -> Result<Self::Frame<'_>, FrameError> {
        self.generic.collect().map_err(FrameError::new)?;
        let size = info.dimensions();
        if output == atlas
            || info.clear().a != 255
            || output.size().width != size.width
            || output.size().height != size.height
            || !self.textures.get(&output).is_some_and(|texture| texture.render_target)
            || !self.textures.contains_key(&atlas)
        {
            return Err(FrameError::new("invalid WGPU output or private atlas"));
        }
        self.begin(size.width, size.height, info.clear());
        Ok(WgpuFrame {
            backend: self,
            target: Some(FrameTarget::Texture { output, atlas }),
        })
    }

    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> Result<(), TextureError> {
        // The inherent upload helper classifies native failures as backend texture errors.
        self.create_texture(id, pixels)
    }

    fn destroy_texture(&mut self, id: TextureId) {
        self.destroy_texture(id);
    }
}

// Minimal shader: convert pixel positions into NDC, sample bound texture, then modulate by
// per-vertex color (used both for regular UI and white-atlas colored primitives).
const UI_SHADER: &str = r#"
struct Uniforms {
    screen_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(0) @binding(1)
var atlas_sampler: sampler;

@group(0) @binding(2)
var atlas_texture: texture_2d<f32>;

struct VsIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(input: VsIn) -> VsOut {
    var out: VsOut;
    let ndc_x = (input.position.x / uniforms.screen_size.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (input.position.y / uniforms.screen_size.y) * 2.0;
    out.clip_pos = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.uv = input.uv;
    out.color = input.color;
    return out;
}

@fragment
fn fs_main(input: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(atlas_texture, atlas_sampler, input.uv);
    return texel * input.color;
}
"#;

#[cfg(test)]
mod tests;
