//! Native WGPU realization of the shared immutable command vocabulary.
//! No client callbacks or COM owners enter a render pass or completion collection.
use neocogi_render_gpu::{abi::*, command::*, record::Block, resource::*, scope::physical_rect};
use std::{
    collections::HashMap,
    num::NonZeroU64,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Native caches own device-free CPU versions; completion owns the exact prepared snapshots.
pub(crate) struct Executor {
    /// WGPU's internal native owner has no reference to logical client registries/domains.
    device: wgpu::Device,
    /// Upload and completion operations use the same submission queue as final presentation.
    queue: wgpu::Queue,
    /// Immutable vertex and index uploads, keyed by canonical backing identity.
    buffers: HashMap<u64, Rc<Buffer>>,
    /// Immutable sampled image versions.
    textures: HashMap<u64, Rc<Texture>>,
    /// Immutable sampler versions.
    samplers: HashMap<u64, Rc<Sampler>>,
    /// Target signature participates in the key, including depth-attachment presence.
    pipelines: HashMap<(u64, wgpu::TextureFormat, bool), Rc<Pipeline>>,
    /// Source ownership pins each address until removal, preventing pointer-key reuse.
    uniforms: HashMap<usize, Rc<Uniform>>,
    /// Small host-owned pipelines implement regional color/depth replacement.
    clears: HashMap<(wgpu::TextureFormat, bool, bool), Rc<ClearPipeline>>,
    /// Only a completion callback's atomic signal crosses threads; snapshots remain UI-thread owned.
    submitted: Vec<Submitted>,
    /// Preparatory queue writes own their backings even when no frame ever submits.
    uploads: Vec<Upload>,
}

macro_rules! allocation {
    ($name:ident, $source:ty, $native:ty) => {
        /// Native allocation with exactly one immutable CPU version/accounting reference.
        struct $name {
            /// Holds byte receipts without retaining a client domain or COM capability.
            source: Rc<$source>,
            /// WGPU handles retain native storage for the declared usage.
            raw: $native,
        }
    };
}
allocation!(Buffer, BufferVersion, wgpu::Buffer);
allocation!(Sampler, SamplerVersion, wgpu::Sampler);
allocation!(Uniform, UniformData, wgpu::Buffer);

/// Immutable native texture plus view and device-free accounting.
struct Texture {
    /// Captured pixel version, with no device/domain back-reference.
    source: TextureReceipt,
    /// Native image owner kept beside its sampling view.
    _raw: wgpu::Texture,
    /// Read-only view used only by graphics bind groups.
    view: wgpu::TextureView,
}
/// Target-compatible native pipeline and its four stage-separated layouts.
struct Pipeline {
    /// Immutable layout/shader versions also retain their logical accounting.
    source: Rc<PipelineVersion>,
    /// Native executable with complete fixed state.
    raw: wgpu::RenderPipeline,
    /// Vertex textures/uniforms, then fragment textures/uniforms.
    groups: [wgpu::BindGroupLayout; 4],
}
/// A host-created exact replacement pipeline; never exposed as a client resource.
struct ClearPipeline {
    /// Color writes or depth writes are selected at creation, independently of client masks.
    raw: wgpu::RenderPipeline,
    /// One initialized vec4 color and one vec4 containing normalized depth.
    layout: wgpu::BindGroupLayout,
}

/// Opaque native block can be retained for submission without retaining logical domain authority.
#[derive(Clone)]
pub struct PreparedBlock(Rc<PreparedData>);
/// Concrete native lowering and original metadata receipts share one completion lifetime.
struct PreparedData {
    /// Only native draw/clear operations, in exact painter order.
    commands: Vec<PreparedCommand>,
    /// Original recording capacity remains charged through native completion.
    _capacity: Vec<CommandCharge>,
}
/// Fully prepared native operations require no allocation or foreign reads in a pass.
enum PreparedCommand {
    /// All consumed graphics bindings and their immutable native owners.
    Draw(PreparedDraw),
    /// Private full-screen triangle is restricted by the authoritative regional scissor.
    Clear {
        /// Host pipeline ignores previous client depth/blend/write state.
        pipeline: Rc<ClearPipeline>,
        /// Exact initialized replacement value, retained by this group and native buffer.
        group: wgpu::BindGroup,
        /// Native uniform storage survives submission with the prepared command.
        _buffer: wgpu::Buffer,
        /// Host-bounded logical write rectangle.
        clip: Rect,
    },
}
/// Complete draw state; bind-group ownership alone does not refund logical bytes early.
struct PreparedDraw {
    /// Executable/layout plus device-free pipeline/shader accounting.
    pipeline: Rc<Pipeline>,
    /// Bound stream slot, immutable native version and checked byte base.
    vertices: Vec<(u32, Rc<Buffer>, u64)>,
    /// Optional index version, element width and byte base.
    index: Option<(Rc<Buffer>, wgpu::IndexFormat, u64)>,
    /// Exactly four explicit groups, including empty groups where a stage has no resources.
    groups: Vec<wgpu::BindGroup>,
    /// Retains actual image/sampler charges through completion, not merely native bind-group handles.
    _textures: Vec<(u32, u32, Rc<Texture>, Rc<Sampler>)>,
    /// Retains copied uniform payload charges through completion.
    _uniforms: Vec<(u32, u32, Rc<Uniform>)>,
    /// Checked direct triangle ranges.
    range: DrawRange,
    /// Viewport mapping is independent of its raster clip.
    viewport: Rect,
    /// Effective host-bounded raster write rectangle.
    clip: Rect,
}
/// UI-thread resources are released only after the queue's ordered completion callback fires.
struct Submitted {
    /// Callback owns only this thread-safe signal, never an Rc or COM owner.
    done: Arc<AtomicBool>,
    /// Exact snapshots/accounting used by the submitted encoder.
    _blocks: Vec<PreparedBlock>,
    /// Native initialization is submitted separately from attachment rendering on the same queue.
    _uploads: Vec<Upload>,
    /// Partial preparation retains original capacity without retaining domain authority.
    _capacity: Vec<CommandCharge>,
}

/// Exact typed backing references for asynchronous initialization; none owns a client domain.
enum Upload {
    /// Vertex/index logical bytes remain charged while the native upload is in flight.
    Buffer { _resource: Rc<Buffer> },
    /// Image bytes remain charged independently from later frame acquisition.
    Texture { _resource: Rc<Texture> },
    /// Uniform pending bytes follow their native initialization as well as later draws.
    Uniform { _resource: Rc<Uniform> },
    /// Private clear values use the containing command's retained capacity receipt.
    Clear { _buffer: wgpu::Buffer },
}

impl Executor {
    /// Shares the concrete device/queue without exposing them through a client interface.
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            device: device.clone(),
            queue: queue.clone(),
            buffers: HashMap::new(),
            textures: HashMap::new(),
            samplers: HashMap::new(),
            pipelines: HashMap::new(),
            uniforms: HashMap::new(),
            clears: HashMap::new(),
            submitted: Vec::new(),
            uploads: Vec::new(),
        }
    }

    /// Observes completion before dropping cache-only versions; public weak expiry is insufficient.
    pub(crate) fn collect(&mut self) -> Result<(), String> {
        self.device.poll(wgpu::PollType::Poll).map_err(|error| error.to_string())?;
        self.submitted.retain(|entry| !entry.done.load(Ordering::Acquire));
        self.buffers.retain(|_, entry| Rc::strong_count(&entry.source) > 1);
        self.textures.retain(|_, entry| entry.source.is_alive());
        self.samplers.retain(|_, entry| Rc::strong_count(&entry.source) > 1);
        self.pipelines.retain(|_, entry| Rc::strong_count(&entry.source) > 1);
        self.uniforms.retain(|_, entry| Rc::strong_count(&entry.source) > 1);
        Ok(())
    }

    /// Realizes a complete block under native error scopes before target acquisition or clearing.
    pub(crate) fn prepare(
        &mut self,
        block: &Block,
        format: wgpu::TextureFormat,
        depth: bool,
        images: &HashMap<neocogi_render_core::TextureId, super::GpuTexture>,
    ) -> Result<PreparedBlock, String> {
        self.preflight(block.retain_capacity(), |executor| executor.prepare_inner(block, format, depth, images))
    }

    /// Private UI image preparation shares the same fallible upload/completion boundary.
    pub(crate) fn prepare_texture(
        &mut self,
        source: Rc<TextureVersion>,
        images: &HashMap<neocogi_render_core::TextureId, super::GpuTexture>,
    ) -> Result<(), String> {
        self.preflight(Vec::new(), |executor| executor.texture(source, images).map(|_| ()))
    }

    /// Every scope is popped on success and failure; initialized resources retire independently of frames.
    fn preflight<T>(&mut self, capacity: Vec<CommandCharge>, operation: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        self.collect()?;
        // Reserve the completion owner before initiating native writes. A failed preparation
        // still submits those writes and retains their storage without requiring a target frame.
        self.submitted.try_reserve(1).map_err(|error| error.to_string())?;
        let scopes =
            [wgpu::ErrorFilter::OutOfMemory, wgpu::ErrorFilter::Internal, wgpu::ErrorFilter::Validation].map(|filter| self.device.push_error_scope(filter));
        let result = operation(self);
        self.finish_uploads(capacity);
        let mut failure = None;
        for scope in scopes.into_iter().rev() {
            if let Some(error) = pollster::block_on(scope.pop()) {
                failure.get_or_insert_with(|| error.to_string());
            }
        }
        if let Some(error) = failure {
            // No invalid handle may become a successful retry through an already populated cache.
            self.buffers.clear();
            self.textures.clear();
            self.samplers.clear();
            self.pipelines.clear();
            self.uniforms.clear();
            self.clears.clear();
            Err(error)
        } else {
            result
        }
    }

    /// Makes queued uploads progress even after preparation/acquisition failure or abandonment.
    /// This uses the same device queue and emits no attachment commands or presentation.
    fn finish_uploads(&mut self, capacity: Vec<CommandCharge>) {
        if self.uploads.is_empty() {
            return;
        }
        self.queue.submit([]);
        let done = Arc::new(AtomicBool::new(false));
        let callback = done.clone();
        self.queue.on_submitted_work_done(move || callback.store(true, Ordering::Release));
        self.submitted.push(Submitted {
            done,
            _blocks: Vec::new(),
            _uploads: std::mem::take(&mut self.uploads),
            _capacity: capacity,
        });
    }

    /// Explicit creation and scoped queue writes propagate native failures without mapped-range
    /// convenience helpers that panic when creation returns an invalid/OOM buffer.
    fn upload_buffer(&self, bytes: &[u8], usage: wgpu::BufferUsages, label: &'static str) -> wgpu::Buffer {
        let padded = bytes.len().div_ceil(4) * 4;
        let raw = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: padded as u64,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if padded == bytes.len() {
            self.queue.write_buffer(&raw, 0, bytes);
        } else {
            let mut data = bytes.to_vec();
            data.resize(padded, 0);
            self.queue.write_buffer(&raw, 0, &data);
        }
        raw
    }

    /// Converts only authenticated immutable CPU snapshots, without entering a native pass.
    fn prepare_inner(
        &mut self,
        block: &Block,
        format: wgpu::TextureFormat,
        depth: bool,
        images: &HashMap<neocogi_render_core::TextureId, super::GpuTexture>,
    ) -> Result<PreparedBlock, String> {
        let mut commands = Vec::new();
        commands.try_reserve_exact(block.commands().len()).map_err(|error| error.to_string())?;
        for command in block.commands() {
            commands.push(match command {
                Command::Draw(draw) => PreparedCommand::Draw(self.draw(draw, format, depth, images)?),
                Command::ClearColor { rgba, clip, .. } => self.clear(format, depth, false, rgba.to_le_bytes().map(|byte| f32::from(byte) / 255.), 0., *clip)?,
                Command::ClearDepth { depth: value, clip, .. } => {
                    if !depth {
                        return Err("depth commands require a host depth attachment".into());
                    }
                    self.clear(format, depth, true, [0.; 4], *value, *clip)?
                }
            });
        }
        Ok(PreparedBlock(Rc::new(PreparedData {
            commands,
            _capacity: block.retain_capacity(),
        })))
    }

    /// Uploads one immutable buffer; native WGPU copy alignment does not change logical bounds.
    fn buffer(&mut self, source: Rc<BufferVersion>) -> Rc<Buffer> {
        if let Some(value) = self.buffers.get(&source.id()) {
            return value.clone();
        }
        let usage = if source.desc().role == GpuBufferRole::Index {
            wgpu::BufferUsages::INDEX
        } else {
            wgpu::BufferUsages::VERTEX
        };
        let raw = self.upload_buffer(source.bytes(), usage, "neocogi.gpu.bytes");
        let id = source.id();
        let value = Rc::new(Buffer { raw, source });
        self.uploads.push(Upload::Buffer { _resource: value.clone() });
        self.buffers.insert(id, value.clone());
        value
    }

    /// Resolve only trusted immutable imports, otherwise upload top-left pixels with scoped native errors.
    fn texture(&mut self, source: Rc<TextureVersion>, images: &HashMap<neocogi_render_core::TextureId, super::GpuTexture>) -> Result<Rc<Texture>, String> {
        if let Some(value) = self.textures.get(&source.id()) {
            return Ok(value.clone());
        }
        let id = source.id();
        if let Some(import) = source.imported_id() {
            let image = images.get(&import).ok_or("imported UI image is unavailable on this device")?;
            let value = Rc::new(Texture {
                source: source.receipt(),
                _raw: image._texture.clone(),
                view: image.view.clone(),
            });
            // Ordinary image construction may still have initialization queued on this same
            // queue. Flush it and retain this device-free receipt through completion, even if
            // import preparation is the only operation and no target frame is ever acquired.
            self.uploads.push(Upload::Texture { _resource: value.clone() });
            self.textures.insert(id, value.clone());
            return Ok(value);
        }
        let size = wgpu::Extent3d {
            width: source.desc().size.width as u32,
            height: source.desc().size.height as u32,
            depth_or_array_layers: 1,
        };
        let raw = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("neocogi.gpu.image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if source.desc().format == GpuTextureFormat::Rgba8Srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &raw,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            source.pixels().ok_or("missing texture pixel storage")?,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 4),
                rows_per_image: Some(size.height),
            },
            size,
        );
        let view = raw.create_view(&wgpu::TextureViewDescriptor::default());
        let value = Rc::new(Texture {
            source: source.receipt(),
            _raw: raw,
            view,
        });
        self.uploads.push(Upload::Texture { _resource: value.clone() });
        self.textures.insert(id, value.clone());
        Ok(value)
    }

    /// Creates immutable one-level sampling state independently of the image resource.
    fn sampler(&mut self, source: Rc<SamplerVersion>) -> Rc<Sampler> {
        self.samplers
            .entry(source.id())
            .or_insert_with(|| {
                Rc::new(Sampler {
                    raw: self.device.create_sampler(&wgpu::SamplerDescriptor {
                        label: Some("neocogi.gpu.sampler"),
                        address_mode_u: address(source.desc().address_u),
                        address_mode_v: address(source.desc().address_v),
                        mag_filter: filter(source.desc().mag_filter),
                        min_filter: filter(source.desc().min_filter),
                        ..Default::default()
                    }),
                    source,
                })
            })
            .clone()
    }

    /// Copies initialized uniform bytes and pads only the native allocation's final extent.
    fn uniform(&mut self, source: Rc<UniformData>) -> Rc<Uniform> {
        let key = Rc::as_ptr(&source) as usize;
        if let Some(value) = self.uniforms.get(&key) {
            return value.clone();
        }
        let mut bytes = source.bytes().to_vec();
        bytes.resize(bytes.len().div_ceil(16) * 16, 0);
        let raw = self.upload_buffer(&bytes, wgpu::BufferUsages::UNIFORM, "neocogi.gpu.uniform");
        let value = Rc::new(Uniform { raw, source });
        self.uploads.push(Upload::Uniform { _resource: value.clone() });
        self.uniforms.insert(key, value.clone());
        value
    }

    /// Captures every binding owner before constructing borrowed bind-group descriptors.
    fn draw(
        &mut self,
        draw: &Draw,
        format: wgpu::TextureFormat,
        depth: bool,
        images: &HashMap<neocogi_render_core::TextureId, super::GpuTexture>,
    ) -> Result<PreparedDraw, String> {
        let pipeline = self.pipeline(draw.pipeline.clone(), format, depth)?;
        let vertices = draw
            .vertex_buffers
            .iter()
            .map(|binding| (binding.slot, self.buffer(binding.buffer.clone()), binding.offset))
            .collect();
        let index = draw.index.as_ref().map(|binding| {
            (
                self.buffer(binding.buffer.clone()),
                if binding.format == GpuIndexFormat::U16 {
                    wgpu::IndexFormat::Uint16
                } else {
                    wgpu::IndexFormat::Uint32
                },
                binding.offset,
            )
        });
        let textures: Vec<_> = draw
            .textures
            .iter()
            .map(|binding| {
                Ok((
                    texture_group(binding.stage),
                    binding.slot,
                    self.texture(binding.texture.clone(), images)?,
                    self.sampler(binding.sampler.clone()),
                ))
            })
            .collect::<Result<_, String>>()?;
        let uniforms: Vec<_> = draw
            .uniforms
            .iter()
            .map(|binding| (uniform_group(binding.stage), binding.slot, self.uniform(binding.data.clone())))
            .collect();
        let groups = (0..4)
            .map(|group| {
                let mut entries = Vec::new();
                for (stage, slot, texture, sampler) in &textures {
                    if *stage == group {
                        entries.push(wgpu::BindGroupEntry {
                            binding: slot * 2,
                            resource: wgpu::BindingResource::TextureView(&texture.view),
                        });
                        entries.push(wgpu::BindGroupEntry {
                            binding: slot * 2 + 1,
                            resource: wgpu::BindingResource::Sampler(&sampler.raw),
                        });
                    }
                }
                for (stage, slot, uniform) in &uniforms {
                    if *stage == group {
                        entries.push(wgpu::BindGroupEntry {
                            binding: *slot,
                            resource: uniform.raw.as_entire_binding(),
                        });
                    }
                }
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("neocogi.gpu.bindings"),
                    layout: &pipeline.groups[group as usize],
                    entries: &entries,
                })
            })
            .collect();
        Ok(PreparedDraw {
            pipeline,
            vertices,
            index,
            groups,
            _textures: textures,
            _uniforms: uniforms,
            range: draw.range,
            viewport: draw.viewport,
            clip: draw.clip,
        })
    }

    /// Creates an exact reflected layout and complete fixed-state variant for the host target.
    fn pipeline(&mut self, source: Rc<PipelineVersion>, format: wgpu::TextureFormat, depth: bool) -> Result<Rc<Pipeline>, String> {
        let key = (source.id(), format, depth);
        if let Some(value) = self.pipelines.get(&key) {
            return Ok(value.clone());
        }
        if !depth && (source.info().depth_write || source.info().depth_compare != GpuCompare::Always) {
            return Err("graphics pipeline requires a host depth attachment".into());
        }
        let groups = std::array::from_fn(|group| {
            let mut entries = Vec::new();
            for binding in source.bindings() {
                let visibility = if binding.stage == GpuShaderStage::Vertex {
                    wgpu::ShaderStages::VERTEX
                } else {
                    wgpu::ShaderStages::FRAGMENT
                };
                if binding.kind == GpuBindingKind::Uniform && uniform_group(binding.stage) == group as u32 {
                    entries.push(wgpu::BindGroupLayoutEntry {
                        binding: binding.slot,
                        visibility,
                        count: None,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(u64::from(binding.byte_len)),
                        },
                    });
                } else if binding.kind == GpuBindingKind::SampledTexture && texture_group(binding.stage) == group as u32 {
                    entries.push(wgpu::BindGroupLayoutEntry {
                        binding: binding.slot * 2,
                        visibility,
                        count: None,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                    });
                    entries.push(wgpu::BindGroupLayoutEntry {
                        binding: binding.slot * 2 + 1,
                        visibility,
                        count: None,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    });
                }
            }
            self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("neocogi.gpu.layout"),
                entries: &entries,
            })
        });
        let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("neocogi.gpu.pipeline.layout"),
            bind_group_layouts: &groups.each_ref().map(Some),
            immediate_size: 0,
        });
        let vertex = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("neocogi.gpu.vertex"),
            source: wgpu::ShaderSource::Wgsl(source.vertex().artifact().wgsl.clone().into()),
        });
        let fragment = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("neocogi.gpu.fragment"),
            source: wgpu::ShaderSource::Wgsl(source.fragment().artifact().wgsl.clone().into()),
        });
        let mut attributes: [Vec<wgpu::VertexAttribute>; 8] = std::array::from_fn(|_| Vec::new());
        for attribute in source.attributes() {
            attributes[attribute.slot as usize].push(wgpu::VertexAttribute {
                format: vertex_format(attribute.format),
                offset: u64::from(attribute.offset),
                shader_location: attribute.location,
            });
        }
        let count = source.vertex_buffers().iter().map(|layout| layout.slot + 1).max().unwrap_or(0);
        let buffers: Vec<_> = (0..count)
            .map(|slot| {
                source
                    .vertex_buffers()
                    .iter()
                    .find(|layout| layout.slot == slot)
                    .map(|layout| wgpu::VertexBufferLayout {
                        array_stride: u64::from(layout.stride),
                        step_mode: if layout.step == GpuStepMode::Instance {
                            wgpu::VertexStepMode::Instance
                        } else {
                            wgpu::VertexStepMode::Vertex
                        },
                        attributes: &attributes[slot as usize],
                    })
            })
            .collect();
        let state = source.info();
        let blend = state.blend.enabled.then(|| wgpu::BlendState {
            color: component(state.blend.src_color, state.blend.dst_color, state.blend.color_op),
            alpha: component(state.blend.src_alpha, state.blend.dst_alpha, state.blend.alpha_op),
        });
        let raw = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("neocogi.gpu.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vertex,
                entry_point: Some(&source.vertex().artifact().entry),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: if state.front_face == GpuFrontFace::Ccw {
                    wgpu::FrontFace::Ccw
                } else {
                    wgpu::FrontFace::Cw
                },
                cull_mode: match state.cull {
                    GpuCullMode::None => None,
                    GpuCullMode::Front => Some(wgpu::Face::Front),
                    _ => Some(wgpu::Face::Back),
                },
                ..Default::default()
            },
            depth_stencil: depth.then(|| wgpu::DepthStencilState {
                format: super::DEPTH_FORMAT,
                depth_write_enabled: Some(state.depth_write),
                depth_compare: Some(compare(state.depth_compare)),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &fragment,
                entry_point: Some(&source.fragment().artifact().entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::from_bits_truncate(u32::from(state.color_write)),
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let value = Rc::new(Pipeline { source, raw, groups });
        self.pipelines.insert(key, value.clone());
        Ok(value)
    }

    /// Creates an initialized host-only regional clear independent of any client pipeline state.
    fn clear(
        &mut self,
        format: wgpu::TextureFormat,
        depth: bool,
        depth_only: bool,
        color: [f32; 4],
        value: f32,
        clip: Rect,
    ) -> Result<PreparedCommand, String> {
        let pipeline = self
            .clears
            .entry((format, depth, depth_only))
            .or_insert_with(|| {
                let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("neocogi.gpu.clear.layout"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(32),
                        },
                        count: None,
                    }],
                });
                let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("neocogi.gpu.clear.shader"),
                    source: wgpu::ShaderSource::Wgsl(CLEAR_SHADER.into()),
                });
                let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("neocogi.gpu.clear.pipeline.layout"),
                    bind_group_layouts: &[Some(&layout)],
                    immediate_size: 0,
                });
                let raw = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("neocogi.gpu.clear.pipeline"),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: depth.then_some(wgpu::DepthStencilState {
                        format: super::DEPTH_FORMAT,
                        depth_write_enabled: Some(depth_only),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: if depth_only { wgpu::ColorWrites::empty() } else { wgpu::ColorWrites::ALL },
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                Rc::new(ClearPipeline { raw, layout })
            })
            .clone();
        let mut bytes = [0u8; 32];
        for (target, number) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(color.into_iter().chain([value, 0., 0., 0.])) {
            target.copy_from_slice(&number.to_le_bytes());
        }
        let buffer = self.upload_buffer(&bytes, wgpu::BufferUsages::UNIFORM, "neocogi.gpu.clear.value");
        self.uploads.push(Upload::Clear { _buffer: buffer.clone() });
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("neocogi.gpu.clear.bindings"),
            layout: &pipeline.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Ok(PreparedCommand::Clear { pipeline, group, _buffer: buffer, clip })
    }

    /// Records complete native state into a host-owned load/store pass with no foreign callbacks.
    pub(crate) fn record<'pass>(
        &'pass self,
        pass: &mut wgpu::RenderPass<'pass>,
        block: &'pass PreparedBlock,
        logical: Size,
        physical: Size,
    ) -> Result<(), String> {
        for command in &block.0.commands {
            let clip = match command {
                PreparedCommand::Draw(draw) => draw.clip,
                PreparedCommand::Clear { clip, .. } => *clip,
            };
            let Some(clip) = physical_rect(clip, logical, physical).map_err(|error| error.to_string())? else {
                continue;
            };
            pass.set_scissor_rect(clip.x, clip.y, clip.width, clip.height);
            match command {
                PreparedCommand::Clear { pipeline, group, .. } => {
                    pass.set_pipeline(&pipeline.raw);
                    pass.set_bind_group(0, group, &[]);
                    pass.set_viewport(0., 0., physical.width as f32, physical.height as f32, 0., 1.);
                    pass.draw(0..3, 0..1);
                }
                PreparedCommand::Draw(draw) => {
                    let Some(view) = physical_rect(draw.viewport, logical, physical).map_err(|error| error.to_string())? else {
                        continue;
                    };
                    pass.set_pipeline(&draw.pipeline.raw);
                    pass.set_viewport(view.x as f32, view.y as f32, view.width as f32, view.height as f32, 0., 1.);
                    for (index, group) in draw.groups.iter().enumerate() {
                        pass.set_bind_group(index as u32, group, &[]);
                    }
                    for (slot, buffer, offset) in &draw.vertices {
                        pass.set_vertex_buffer(*slot, buffer.raw.slice(*offset..));
                    }
                    match draw.range {
                        DrawRange::Vertices { first, count, instances } => pass.draw(first..first + count, 0..instances),
                        DrawRange::Indices { first, count, instances } => {
                            let (buffer, format, offset) = draw.index.as_ref().ok_or("missing prepared index binding")?;
                            pass.set_index_buffer(buffer.raw.slice(*offset..), *format);
                            pass.draw_indexed(first..first + count, 0, 0..instances);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Called immediately after queue submission; callback ownership cannot form a logical-device cycle.
    pub(crate) fn submitted(&mut self, blocks: Vec<PreparedBlock>) {
        if blocks.is_empty() {
            return;
        }
        let done = Arc::new(AtomicBool::new(false));
        let callback = done.clone();
        self.queue.on_submitted_work_done(move || callback.store(true, Ordering::Release));
        self.submitted.push(Submitted {
            done,
            _blocks: blocks,
            _uploads: Vec::new(),
            _capacity: Vec::new(),
        });
    }
}

impl Drop for Executor {
    /// Final native wait precedes the release of byte/capacity receipts and native handle owners.
    fn drop(&mut self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

/// Internal shader uses uniform depth in clip space and produces one exact replacement color.
const CLEAR_SHADER: &str = r#"
struct Values { color:vec4<f32>, params:vec4<f32> }
@group(0) @binding(0) var<uniform> values:Values;
@vertex fn vs(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
    return vec4(p[i],values.params.x,1.);
}
@fragment fn fs() -> @location(0) vec4<f32> { return values.color; }
"#;

/// Stage-separated texture groups are fixed by the portable shader profile.
fn texture_group(stage: GpuShaderStage) -> u32 {
    if stage == GpuShaderStage::Vertex { 0 } else { 2 }
}
/// Stage-separated uniform groups are fixed by the portable shader profile.
fn uniform_group(stage: GpuShaderStage) -> u32 {
    texture_group(stage) + 1
}
/// Converts one validated address mode.
fn address(value: GpuAddressMode) -> wgpu::AddressMode {
    match value {
        GpuAddressMode::Clamp => wgpu::AddressMode::ClampToEdge,
        GpuAddressMode::Repeat => wgpu::AddressMode::Repeat,
        _ => wgpu::AddressMode::MirrorRepeat,
    }
}
/// Converts one validated nonmipmapped filter.
fn filter(value: GpuFilter) -> wgpu::FilterMode {
    if value == GpuFilter::Nearest {
        wgpu::FilterMode::Nearest
    } else {
        wgpu::FilterMode::Linear
    }
}
/// Converts every admitted explicit byte format, preserving unsigned integer interpretation.
fn vertex_format(value: GpuVertexFormat) -> wgpu::VertexFormat {
    match value {
        GpuVertexFormat::Float32 => wgpu::VertexFormat::Float32,
        GpuVertexFormat::Float32x2 => wgpu::VertexFormat::Float32x2,
        GpuVertexFormat::Float32x3 => wgpu::VertexFormat::Float32x3,
        GpuVertexFormat::Float32x4 => wgpu::VertexFormat::Float32x4,
        GpuVertexFormat::Uint32 => wgpu::VertexFormat::Uint32,
        GpuVertexFormat::Uint32x2 => wgpu::VertexFormat::Uint32x2,
        GpuVertexFormat::Uint32x3 => wgpu::VertexFormat::Uint32x3,
        GpuVertexFormat::Uint32x4 => wgpu::VertexFormat::Uint32x4,
        _ => wgpu::VertexFormat::Unorm8x4,
    }
}
/// Converts the validated complete depth predicate.
fn compare(value: GpuCompare) -> wgpu::CompareFunction {
    match value {
        GpuCompare::Never => wgpu::CompareFunction::Never,
        GpuCompare::Less => wgpu::CompareFunction::Less,
        GpuCompare::Equal => wgpu::CompareFunction::Equal,
        GpuCompare::LessEqual => wgpu::CompareFunction::LessEqual,
        GpuCompare::Greater => wgpu::CompareFunction::Greater,
        GpuCompare::NotEqual => wgpu::CompareFunction::NotEqual,
        GpuCompare::GreaterEqual => wgpu::CompareFunction::GreaterEqual,
        _ => wgpu::CompareFunction::Always,
    }
}
/// Converts one validated source/destination blend factor.
fn factor(value: GpuBlendFactor) -> wgpu::BlendFactor {
    match value {
        GpuBlendFactor::Zero => wgpu::BlendFactor::Zero,
        GpuBlendFactor::One => wgpu::BlendFactor::One,
        GpuBlendFactor::SrcColor => wgpu::BlendFactor::Src,
        GpuBlendFactor::OneMinusSrcColor => wgpu::BlendFactor::OneMinusSrc,
        GpuBlendFactor::DstColor => wgpu::BlendFactor::Dst,
        GpuBlendFactor::OneMinusDstColor => wgpu::BlendFactor::OneMinusDst,
        GpuBlendFactor::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
        GpuBlendFactor::OneMinusSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
        GpuBlendFactor::DstAlpha => wgpu::BlendFactor::DstAlpha,
        _ => wgpu::BlendFactor::OneMinusDstAlpha,
    }
}
/// Converts independent color/alpha equations into complete native fixed state.
fn component(src: GpuBlendFactor, dst: GpuBlendFactor, op: GpuBlendOp) -> wgpu::BlendComponent {
    // Min/max use unscaled source/destination values. WebGPU requires One explicitly,
    // whereas GL/Vulkan accept and ignore other factor fields for these operations.
    let unscaled = matches!(op, GpuBlendOp::Min | GpuBlendOp::Max);
    wgpu::BlendComponent {
        src_factor: if unscaled { wgpu::BlendFactor::One } else { factor(src) },
        dst_factor: if unscaled { wgpu::BlendFactor::One } else { factor(dst) },
        operation: match op {
            GpuBlendOp::Add => wgpu::BlendOperation::Add,
            GpuBlendOp::Subtract => wgpu::BlendOperation::Subtract,
            GpuBlendOp::ReverseSubtract => wgpu::BlendOperation::ReverseSubtract,
            GpuBlendOp::Min => wgpu::BlendOperation::Min,
            _ => wgpu::BlendOperation::Max,
        },
    }
}
