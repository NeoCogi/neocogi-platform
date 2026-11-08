//! Native WGPU realization of the shared immutable command vocabulary.
//! No client callbacks or COM owners enter a render pass or completion collection.
use crate::gpu::{
    abi::*,
    command::{DrawRange, CommandCharge},
    resource::BackingCharge,
    native::NativeResources,
    scope::physical_rect,
};
use std::{
    collections::HashMap,
    num::NonZeroU64,
    rc::Rc,
    ops::Deref,
    sync::{
        mpsc::{self, Sender, Receiver, SendError},
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use super::{Core, Image, ImageAllocation};
type Block = crate::gpu::record::Block<Backend>;
type Command = crate::gpu::command::Command<Backend>;
type Draw = crate::gpu::command::Draw<Backend>;
type BufferVersion = crate::gpu::resource::BufferVersion<Backend>;
type TextureVersion = crate::gpu::resource::TextureVersion<Backend>;
type SamplerVersion = crate::gpu::resource::SamplerVersion<Backend>;
type PipelineVersion = crate::gpu::resource::PipelineVersion<Backend>;
/// Statically selected native WGPU resource family, never an erased payload or COM extension.
pub struct Backend;
/// Exclusive native construction capability, not publicly constructible from a resource owner.
pub struct Preparation<'a> {
    /// Exclusive existing driver; native construction never borrows a foreign capability.
    executor: &'a mut Executor,
}
impl NativeResources for Backend {
    fn shader_target(_core: &Self::Core) -> crate::shader::ShaderTarget {
        crate::shader::ShaderTarget::Wgsl
    }
    type Core = Core;
    type Buffer = Buffer;
    type Image = Image;
    type Sampler = Sampler;
    type Pipeline = Pipeline;
    type PipelineKey = (wgpu::TextureFormat, bool);
    type Preparation<'a> = Preparation<'a>;
    fn identity(core: &Core) -> &neocogi_platform::render::source::DriverIdentity {
        &core.identity
    }
    fn storage_available(core: &Core) -> Result<(), String> {
        core.check()
    }
    fn core<'a>(preparation: &'a Preparation<'_>) -> &'a Rc<Core> {
        &preparation.executor.core
    }
    fn check(preparation: &mut Preparation<'_>) -> Result<(), String> {
        preparation.executor.core.check()
    }
    fn image_core(image: &Image) -> &Rc<Core> {
        &image.core
    }
    fn image_size(image: &Image) -> Size {
        image.size
    }
    fn buffer(source: &BufferVersion, preparation: &mut Preparation<'_>) -> Result<Rc<Buffer>, String> {
        let core = preparation.executor.core.clone();
        core.scoped(|| preparation.executor.buffer(source))
    }
    /// Queue writes transfer upload bytes into the same persistent target buffer.
    fn update_buffer(source: &BufferVersion, native: &Rc<Buffer>, preparation: &mut Preparation<'_>) -> Result<(), String> {
        let executor = &mut preparation.executor;
        executor.uploads.try_reserve(1).map_err(|error| error.to_string())?;
        executor.uploads.push(Upload::Buffer { _resource: native.clone() });
        let bytes = source.bytes().as_slice();
        executor.core.scoped(|| {
            let full = bytes.len() / 4 * 4;
            if full != 0 {
                executor.core.queue.write_buffer(&native.raw, 0, &bytes[..full]);
            }
            if full != bytes.len() {
                let mut tail = [0u8; 4];
                tail[..bytes.len() - full].copy_from_slice(&bytes[full..]);
                executor.core.queue.write_buffer(&native.raw, full as u64, &tail);
            }
            Ok(())
        })
    }
    /// Retains the original texture and receipt through the executor's next submission.
    fn update_image(source: &TextureVersion, native: &Rc<Image>, preparation: &mut Preparation<'_>) -> Result<(), String> {
        let executor = &mut preparation.executor;
        executor.uploads.try_reserve(1).map_err(|error| error.to_string())?;
        executor.uploads.push(Upload::Image {
            _resource: native.clone(),
            _receipt: Some(source.receipt()),
        });
        let bytes = source
            .pixels()
            .map(|bytes| bytes.as_slice())
            .ok_or("immutable imported image cannot be updated")?;
        let size = source.desc().size;
        executor.core.scoped(|| {
            executor.core.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &native._texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.width as u32 * 4),
                    rows_per_image: Some(size.height as u32),
                },
                wgpu::Extent3d {
                    width: size.width as u32,
                    height: size.height as u32,
                    depth_or_array_layers: 1,
                },
            );
            Ok(())
        })
    }
    fn sampler(source: &SamplerVersion, preparation: &mut Preparation<'_>) -> Result<Rc<Sampler>, String> {
        let core = preparation.executor.core.clone();
        core.scoped(|| preparation.executor.sampler(source))
    }
    fn pipeline(source: &PipelineVersion, key: &Self::PipelineKey, preparation: &mut Preparation<'_>) -> Result<Rc<Pipeline>, String> {
        let core = preparation.executor.core.clone();
        core.scoped(|| preparation.executor.pipeline(source, key.0, key.1))
    }
    fn image(source: &TextureVersion, preparation: &mut Preparation<'_>) -> Result<Rc<Image>, String> {
        let executor = &mut preparation.executor;
        if let Some(image) = source.imported_image() {
            executor.uploads.try_reserve(1).map_err(|e| e.to_string())?;
            executor.uploads.push(Upload::Image {
                _resource: image.clone(),
                _receipt: Some(source.receipt()),
            });
            return Ok(image.clone());
        }
        executor.create_image(
            source.desc().size,
            if source.desc().format == GpuTextureFormat::Rgba8Srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            source.pixels().map(|p| p.as_slice()),
            Some(source.receipt()),
            false,
        )
    }
}
/// Sender-free owned packets retain only actual native handles, physical lifetime and reservations.
pub(in crate::gpu::backends::wgpu) enum Retirement {
    /// Vertex/index/uniform storage and its opaque receipt.
    Buffer(BufferAllocation),
    /// Independent native sampling state.
    Sampler(SamplerAllocation),
    /// One target-compatible executable and dependency receipts.
    Pipeline(PipelineAllocation),
    /// Exact immutable native pixels and physical lifetime.
    Image(ImageAllocation),
}
impl Retirement {
    /// Exhaustively destroy the returned/drained owned packet; payloads contain no producer.
    pub(in crate::gpu::backends::wgpu) fn discard(self) {
        match self {
            Self::Buffer(value) => drop(value),
            Self::Sampler(value) => drop(value),
            Self::Pipeline(value) => drop(value),
            Self::Image(value) => drop(value),
        }
    }
}
macro_rules! allocation {
    ($name:ident,$allocation:ident,$native:ty) => {
        /// Owned sender-free native allocation with no logical version or facade back-reference.
        pub struct $allocation {
            /// Physical device lifetime only, with no driver or channel endpoint.
            _core: Rc<Core>,
            /// Exact native handle is destroyed before its reservation refunds.
            raw: $native,
            /// Shared original reservation, with no source-version or domain reference.
            _charge: BackingCharge,
        }
        /// Last-owner notification transfers the complete native allocation to the existing driver.
        pub struct $name {
            /// Taken exactly once into an owned packet on final native backing release.
            allocation: Option<$allocation>,
            /// Producer exists only on this native backing; disconnected sends recover ownership.
            retired: Sender<Retirement>,
        }
        impl Deref for $name {
            type Target = $allocation;
            fn deref(&self) -> &$allocation {
                self.allocation.as_ref().expect("live native allocation")
            }
        }
        impl Drop for $name {
            fn drop(&mut self) {
                if let Some(allocation) = self.allocation.take()
                    && let Err(SendError(packet)) = self.retired.send(Retirement::$name(allocation))
                {
                    packet.discard();
                }
            }
        }
    };
}
allocation!(Buffer, BufferAllocation, wgpu::Buffer);
allocation!(Sampler, SamplerAllocation, wgpu::Sampler);
/// Sender-free native executable/layout owner; only opaque definition/shader receipts are retained.
pub struct PipelineAllocation {
    /// Physical device lifetime only, with no driver or channel endpoint.
    _core: Rc<Core>,
    /// Complete target-compatible native executable.
    raw: wgpu::RenderPipeline,
    /// Stage-separated layouts remain with the matching executable.
    groups: [wgpu::BindGroupLayout; 4],
    /// Shared definition/shader reservations have no parent-version back-reference.
    _charges: [BackingCharge; 3],
}
/// Target-specific native variant owned by its immutable logical version.
pub struct Pipeline {
    /// Taken exactly once before final executable retirement.
    allocation: Option<PipelineAllocation>,
    /// Producer exists only on this native backing; disconnected sends recover ownership.
    retired: Sender<Retirement>,
}
impl Deref for Pipeline {
    type Target = PipelineAllocation;
    fn deref(&self) -> &PipelineAllocation {
        self.allocation.as_ref().expect("live native pipeline")
    }
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take()
            && let Err(SendError(packet)) = self.retired.send(Retirement::Pipeline(allocation))
        {
            packet.discard();
        }
    }
}
/// One existing driver owns completion and retirement. Only tiny host clear variants are cached here;
/// application realizations live in their returned versions and are never indexed globally.
pub(in crate::gpu::backends::wgpu) struct Executor {
    /// Actual physical device identity/lifetime, independent from the driver facade.
    pub(super) core: Rc<Core>,
    /// Only actual native construction clones this producer.
    retired_tx: Sender<Retirement>,
    /// The existing driver is the sole receiver and drains after safe completion.
    retired_rx: Receiver<Retirement>,
    /// Small host clear variants have no application resource/source keys.
    clears: HashMap<(wgpu::TextureFormat, bool, bool), Rc<ClearPipeline>>,
    /// Actual native owners follow their real queue completion signals.
    submitted: Vec<Submitted>,
    /// Unpublished storage is retained before its first queued initialization use.
    uploads: Vec<Upload>,
    /// Inject one real mapped-buffer submission error after valid initialization has been issued.
    #[cfg(test)]
    pub(super) submit_failure: bool,
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
#[cfg(test)]
impl PreparedBlock {
    /// Observe actual native reuse, without adding a production registry or performance counter.
    pub(in crate::gpu::backends::wgpu) fn first_pipeline(&self) -> Rc<Pipeline> {
        let PreparedCommand::Draw(draw) = &self.0.commands[0] else { panic!("draw") };
        draw.pipeline.clone()
    }
}
/// Concrete native lowering and original metadata receipts share one completion lifetime.
struct PreparedData {
    /// Exact physical device and target signature are checked before replay.
    core: Rc<Core>,
    /// Host color format and depth presence required by this prepared data.
    signature: (wgpu::TextureFormat, bool),
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
/// Native descriptor namespace/slot, exact image/sampler owners and this import's independent charge.
type SampledBinding = (u32, u32, Rc<Image>, Rc<Sampler>, BackingCharge);
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
    _textures: Vec<SampledBinding>,
    /// Retains persistent uniform allocations and their charges through completion.
    _uniforms: Vec<(u32, u32, Rc<Buffer>, u64, u64)>,
    /// Checked direct triangle ranges.
    range: DrawRange,
    /// Viewport mapping is independent of its raster clip.
    viewport: Rect,
    /// Effective host-bounded raster write rectangle.
    clip: Rect,
}
/// Native success permits signal-based collection; uncertain submission requires final idle/loss.
enum Completion {
    /// Queue callback owns only this atomic flag; UI-thread native owners stay in Submitted.
    Pending(Arc<AtomicBool>),
    /// An error after submit does not establish which native work ran; callbacks cannot refund it.
    Uncertain,
}
/// UI-thread resources are released only after the queue's ordered completion callback fires.
struct Submitted {
    /// Actual submission outcome controls when its native owners may be collected.
    completion: Completion,
    /// Exact snapshots/accounting used by the submitted encoder.
    _blocks: Vec<PreparedBlock>,
    /// Native initialization is submitted separately from attachment rendering on the same queue.
    _uploads: Vec<Upload>,
    /// Partial preparation retains original capacity without retaining domain authority.
    _capacity: Vec<CommandCharge>,
    /// Includes offscreen clear-only attachments until actual submission completion.
    _attachments: Vec<Rc<Image>>,
}

/// Exact typed backing references for asynchronous initialization; none owns a client domain.
enum Upload {
    /// Vertex/index logical bytes remain charged while the native upload is in flight.
    Buffer { _resource: Rc<Buffer> },
    /// Image bytes remain charged independently from later frame acquisition.
    Image { _resource: Rc<Image>, _receipt: Option<BackingCharge> },
    /// Private clear values use the containing command's retained capacity receipt.
    Clear { _buffer: wgpu::Buffer },
}

impl Executor {
    /// Native regression construction retains the same exclusive executor borrow.
    #[cfg(test)]
    pub(in crate::gpu::backends::wgpu) fn test_preparation(&mut self) -> Preparation<'_> {
        Preparation { executor: self }
    }

    /// Existing driver receives owned packets; the shared physical core owns neither endpoint.
    pub(in crate::gpu::backends::wgpu) fn new(core: Rc<Core>) -> Self {
        let (retired_tx, retired_rx) = mpsc::channel();
        Self {
            core,
            retired_tx,
            retired_rx,
            clears: HashMap::new(),
            submitted: Vec::new(),
            uploads: Vec::new(),
            #[cfg(test)]
            submit_failure: false,
        }
    }
    /// Poll actual completion, drop completed owners, then drain even when polling reports failure.
    pub(in crate::gpu::backends::wgpu) fn collect(&mut self) -> Result<(), String> {
        let result = self.core.device.poll(wgpu::PollType::Poll).map_err(|e| e.to_string());
        self.submitted.retain(|entry| match &entry.completion {
            Completion::Pending(done) => !done.load(Ordering::Acquire),
            Completion::Uncertain => true,
        });
        self.drain();
        result?;
        self.core.check()
    }
    /// Destruction of safe queued packets never traverses a logical version or producer back-edge.
    fn drain(&mut self) {
        while let Ok(packet) = self.retired_rx.try_recv() {
            packet.discard();
        }
    }
    /// Authenticate the core before any native operation, including a clear-only block.
    pub(in crate::gpu::backends::wgpu) fn prepare(&mut self, block: &Block, format: wgpu::TextureFormat, depth: bool) -> Result<PreparedBlock, String> {
        block.admit_on(&self.core).map_err(|e| e.to_string())?;
        self.preflight(block.retain_capacity(), |executor| executor.prepare_inner(block, format, depth))
    }
    /// Native slots resolve all constructor scopes before publishing Ready; no global rollback exists.
    pub(in crate::gpu::backends::wgpu) fn prepare_texture(&mut self, source: Rc<TextureVersion>) -> Result<(), String> {
        self.preflight(Vec::new(), |executor| source.native(&mut Preparation { executor }).map(|_| ()))
    }
    /// Even failed preparation seals queued initialization and retains its exact owners/receipts.
    fn preflight<T>(&mut self, capacity: Vec<CommandCharge>, operation: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        self.collect()?;
        self.reserve_submission()?;
        let core = self.core.clone();
        let result = core.scoped(|| operation(self));
        let submission = self.finish_uploads(capacity);
        match result {
            Err(first) => Err(first),
            Ok(value) => {
                submission?;
                Ok(value)
            }
        }
    }
    /// Reserve before uncertain submission so registering already-used native owners is infallible.
    pub(in crate::gpu::backends::wgpu) fn reserve_submission(&mut self) -> Result<(), String> {
        self.submitted.try_reserve(1).map_err(|e| e.to_string())
    }
    /// Makes queued uploads progress even after preparation/acquisition failure or abandonment.
    /// This uses the same device queue and emits no attachment commands or presentation.
    fn finish_uploads(&mut self, capacity: Vec<CommandCharge>) -> Result<(), String> {
        if self.uploads.is_empty() {
            return Ok(());
        }
        self.submit(None, Vec::new(), Vec::new(), capacity)
    }
    /// One fallible queue boundary owns every actual resource before submit, including queued writes.
    /// A scoped submit failure is terminal for this compatible core, without evicting version slots.
    pub(in crate::gpu::backends::wgpu) fn submit(
        &mut self,
        command: Option<wgpu::CommandBuffer>,
        blocks: Vec<PreparedBlock>,
        attachments: Vec<Rc<Image>>,
        capacity: Vec<CommandCharge>,
    ) -> Result<(), String> {
        self.core.check()?;
        self.reserve_submission()?;
        let done = Arc::new(AtomicBool::new(false));
        let callback = done.clone();
        let index = self.submitted.len();
        self.submitted.push(Submitted {
            completion: Completion::Pending(done),
            _blocks: blocks,
            _uploads: std::mem::take(&mut self.uploads),
            _capacity: capacity,
            _attachments: attachments,
        });
        let core = self.core.clone();
        #[cfg(test)]
        let failure = std::mem::take(&mut self.submit_failure);
        let result = core.scoped(|| {
            core.queue.submit(command);
            #[cfg(test)]
            if failure {
                // This reaches WGPU's actual queue-submit validation, after valid native work.
                // Capturing this error proves the submission itself remains inside its scopes.
                let source = core.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("injected mapped submit source"),
                    size: 4,
                    usage: wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: true,
                });
                let target = core.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("injected submit destination"),
                    size: 4,
                    usage: wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let mut encoder = core.device.create_command_encoder(&Default::default());
                encoder.copy_buffer_to_buffer(&source, 0, &target, 0, 4);
                core.queue.submit(Some(encoder.finish()));
            }
            core.queue.on_submitted_work_done(move || callback.store(true, Ordering::Release));
            Ok(())
        });
        if result.is_err() {
            self.submitted[index].completion = Completion::Uncertain;
            core.fail_submission();
        }
        result
    }

    /// Explicit creation and scoped queue writes propagate native failures without mapped-range
    /// convenience helpers that panic when creation returns an invalid/OOM buffer.
    fn upload_buffer(&self, bytes: &[u8], usage: wgpu::BufferUsages, label: &'static str) -> wgpu::Buffer {
        let padded = bytes.len().div_ceil(4) * 4;
        let raw = self.core.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: padded as u64,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if padded == bytes.len() {
            self.core.queue.write_buffer(&raw, 0, bytes);
        } else {
            let mut data = bytes.to_vec();
            data.resize(padded, 0);
            self.core.queue.write_buffer(&raw, 0, &data);
        }
        raw
    }

    /// Converts only authenticated immutable CPU snapshots, without entering a native pass.
    fn prepare_inner(&mut self, block: &Block, format: wgpu::TextureFormat, depth: bool) -> Result<PreparedBlock, String> {
        let mut commands = Vec::new();
        commands.try_reserve_exact(block.commands().len()).map_err(|error| error.to_string())?;
        for command in block.commands() {
            commands.push(match command {
                Command::Draw(draw) => PreparedCommand::Draw(self.draw(draw, format, depth)?),
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
            core: self.core.clone(),
            signature: (format, depth),
            commands,
            _capacity: block.retain_capacity(),
        })))
    }

    /// Retains the actual unpublished native candidate before the first queued write.
    fn buffer(&mut self, source: &BufferVersion) -> Result<Rc<Buffer>, String> {
        let usage = match source.desc().role() {
            GpuBufferRole::Vertex => wgpu::BufferUsages::VERTEX,
            GpuBufferRole::Index => wgpu::BufferUsages::INDEX,
            GpuBufferRole::Uniform => wgpu::BufferUsages::UNIFORM,
            _ => return Err("texture role cannot allocate a native linear buffer".into()),
        };
        self.storage(source.bytes().len(), Some(source.bytes().as_slice()), usage, source.receipt())
    }
    /// New WGPU storage has defined zero contents; write only the supplied prefix and at most 3 pad bytes.
    fn storage(&mut self, len: usize, bytes: Option<&[u8]>, usage: wgpu::BufferUsages, charge: BackingCharge) -> Result<Rc<Buffer>, String> {
        self.uploads.try_reserve(1).map_err(|e| e.to_string())?;
        let raw = self.core.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("neocogi.storage"),
            size: len.div_ceil(4) as u64 * 4,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let value = Rc::new(Buffer {
            allocation: Some(BufferAllocation {
                _core: self.core.clone(),
                raw,
                _charge: charge,
            }),
            retired: self.retired_tx.clone(),
        });
        self.uploads.push(Upload::Buffer { _resource: value.clone() });
        if let Some(bytes) = bytes {
            let full = bytes.len() / 4 * 4;
            if full != 0 {
                self.core.queue.write_buffer(&value.raw, 0, &bytes[..full]);
            }
            if full < bytes.len() {
                let mut tail = [0; 4];
                tail[..bytes.len() - full].copy_from_slice(&bytes[full..]);
                self.core.queue.write_buffer(&value.raw, full as u64, &tail);
            }
        }
        Ok(value)
    }
    /// Scope validation before a sampler can become the version's native Ready value.
    fn sampler(&mut self, source: &SamplerVersion) -> Result<Rc<Sampler>, String> {
        let raw = self.core.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("neocogi.sampler"),
            address_mode_u: address(source.desc().address_u),
            address_mode_v: address(source.desc().address_v),
            mag_filter: filter(source.desc().mag_filter),
            min_filter: filter(source.desc().min_filter),
            ..Default::default()
        });
        Ok(Rc::new(Sampler {
            allocation: Some(SamplerAllocation {
                _core: self.core.clone(),
                raw,
                _charge: source.receipt(),
            }),
            retired: self.retired_tx.clone(),
        }))
    }
    /// Standalone ordinary/output construction owns initialization progress without a later frame.
    pub(in crate::gpu::backends::wgpu) fn image(
        &mut self,
        size: Size,
        format: wgpu::TextureFormat,
        pixels: Option<&[u8]>,
        charge: Option<BackingCharge>,
        target: bool,
    ) -> Result<Rc<Image>, String> {
        self.preflight(Vec::new(), |executor| executor.create_image(size, format, pixels, charge, target))
    }
    /// Generic textures, ordinary images and private outputs use exactly one owned native allocation.
    fn create_image(
        &mut self,
        size: Size,
        format: wgpu::TextureFormat,
        pixels: Option<&[u8]>,
        charge: Option<BackingCharge>,
        target: bool,
    ) -> Result<Rc<Image>, String> {
        self.core.extent(size.width as u32, size.height as u32)?;
        self.uploads.try_reserve(1).map_err(|e| e.to_string())?;
        self.reserve_submission()?;
        let core = self.core.clone();
        let result = core.scoped(|| {
            let extent = wgpu::Extent3d {
                width: size.width as u32,
                height: size.height as u32,
                depth_or_array_layers: 1,
            };
            let raw = core.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("neocogi.image"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC
                    | if target {
                        wgpu::TextureUsages::RENDER_ATTACHMENT
                    } else {
                        wgpu::TextureUsages::empty()
                    },
                view_formats: &[],
            });
            let view = raw.create_view(&Default::default());
            let image = Rc::new(Image {
                allocation: Some(ImageAllocation {
                    core: core.clone(),
                    _texture: raw,
                    view,
                    size,
                    _charge: charge,
                }),
                retired: self.retired_tx.clone(),
            });
            self.uploads.push(Upload::Image { _resource: image.clone(), _receipt: None });
            if let Some(bytes) = pixels {
                core.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &image._texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(extent.width * 4),
                        rows_per_image: Some(extent.height),
                    },
                    extent,
                );
            }
            Ok(image)
        });
        // The enclosing preflight seals uploads with the original source capacity receipt.
        result
    }

    /// Captures every binding owner before constructing borrowed bind-group descriptors.
    fn draw(&mut self, draw: &Draw, format: wgpu::TextureFormat, depth: bool) -> Result<PreparedDraw, String> {
        let mut preparation = Preparation { executor: self };
        let pipeline = draw.pipeline.native((format, depth), &mut preparation)?;
        let textures: Vec<_> = draw
            .textures
            .iter()
            .map(|binding| {
                Ok((
                    texture_group(binding.stage),
                    binding.slot,
                    binding.texture.native(&mut preparation)?,
                    binding.sampler.native(&mut preparation)?,
                    binding.texture.receipt(),
                ))
            })
            .collect::<Result<_, String>>()?;
        let uniforms: Vec<_> = draw
            .uniforms
            .iter()
            .map(|binding| {
                Ok((
                    uniform_group(binding.stage),
                    binding.slot,
                    binding.data.native(&mut preparation)?,
                    binding.data.offset(),
                    binding.data.byte_len(),
                ))
            })
            .collect::<Result<_, String>>()?;
        let vertices = draw
            .vertex_buffers
            .iter()
            .map(|binding| Ok((binding.slot, binding.buffer.native(&mut preparation)?, binding.offset)))
            .collect::<Result<_, String>>()?;
        let index = draw
            .index
            .as_ref()
            .map(|binding| {
                Ok::<_, String>((
                    binding.buffer.native(&mut preparation)?,
                    if binding.format == GpuIndexFormat::U16 {
                        wgpu::IndexFormat::Uint16
                    } else {
                        wgpu::IndexFormat::Uint32
                    },
                    binding.offset,
                ))
            })
            .transpose()?;
        let groups = (0..4)
            .map(|group| {
                let mut entries = Vec::new();
                for (stage, slot, texture, sampler, _) in &textures {
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
                for (stage, slot, uniform, offset, len) in &uniforms {
                    if *stage == group {
                        entries.push(wgpu::BindGroupEntry {
                            binding: *slot,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &uniform.raw,
                                offset: *offset,
                                size: NonZeroU64::new(*len),
                            }),
                        });
                    }
                }
                self.core.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
    fn pipeline(&mut self, source: &PipelineVersion, format: wgpu::TextureFormat, depth: bool) -> Result<Rc<Pipeline>, String> {
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
            self.core.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("neocogi.gpu.layout"),
                entries: &entries,
            })
        });
        let layout = self.core.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("neocogi.gpu.pipeline.layout"),
            bind_group_layouts: &groups.each_ref().map(Some),
            immediate_size: 0,
        });
        let vertex = self.core.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("neocogi.gpu.vertex"),
            source: wgpu::ShaderSource::Wgsl(source.vertex().artifact().code.wgsl().ok_or("WGPU requires validated WGSL")?.to_owned().into()),
        });
        let fragment = self.core.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("neocogi.gpu.fragment"),
            source: wgpu::ShaderSource::Wgsl(
                source
                    .fragment()
                    .artifact()
                    .code
                    .wgsl()
                    .ok_or("WGPU requires validated WGSL")?
                    .to_owned()
                    .into(),
            ),
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
        let raw = self.core.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        let value = Rc::new(Pipeline {
            allocation: Some(PipelineAllocation {
                _core: self.core.clone(),
                raw,
                groups,
                _charges: source.receipts(),
            }),
            retired: self.retired_tx.clone(),
        });
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
        let key = (format, depth, depth_only);
        let pipeline = if let Some(pipeline) = self.clears.get(&key) {
            pipeline.clone()
        } else {
            let pipeline = self.core.scoped(|| {
                let layout = self.core.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
                let module = self.core.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("neocogi.gpu.clear.shader"),
                    source: wgpu::ShaderSource::Wgsl(CLEAR_SHADER.into()),
                });
                let pipeline_layout = self.core.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("neocogi.gpu.clear.pipeline.layout"),
                    bind_group_layouts: &[Some(&layout)],
                    immediate_size: 0,
                });
                let raw = self.core.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                Ok(Rc::new(ClearPipeline { raw, layout }))
            })?;
            self.clears.insert(key, pipeline.clone());
            pipeline
        };
        let mut bytes = [0u8; 32];
        for (target, number) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(color.into_iter().chain([value, 0., 0., 0.])) {
            target.copy_from_slice(&number.to_le_bytes());
        }
        let buffer = self.upload_buffer(&bytes, wgpu::BufferUsages::UNIFORM, "neocogi.gpu.clear.value");
        self.uploads.push(Upload::Clear { _buffer: buffer.clone() });
        let group = self.core.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
    pub(in crate::gpu::backends::wgpu) fn record<'pass>(
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

    /// Reject foreign-device or incompatible target blocks before attachment recording/submission.
    pub(in crate::gpu::backends::wgpu) fn admit(&self, block: &PreparedBlock, signature: (wgpu::TextureFormat, bool)) -> Result<(), String> {
        self.core.check()?;
        if Rc::ptr_eq(&self.core, &block.0.core) && block.0.signature == signature {
            Ok(())
        } else {
            Err("prepared WGPU block belongs to another device or target".into())
        }
    }

    /// Both clear-only attachments and draw resources follow the actual ordered completion signal.
    #[cfg(test)]
    pub(in crate::gpu::backends::wgpu) fn submitted(&mut self, blocks: Vec<PreparedBlock>, attachments: Vec<Rc<Image>>) {
        let done = Arc::new(AtomicBool::new(false));
        let callback = done.clone();
        self.core.queue.on_submitted_work_done(move || callback.store(true, Ordering::Release));
        self.submitted.push(Submitted {
            completion: Completion::Pending(done),
            _blocks: blocks,
            _uploads: Vec::new(),
            _capacity: Vec::new(),
            _attachments: attachments,
        });
    }
}

impl Drop for Executor {
    /// Final native wait precedes the release of byte/capacity receipts and native handle owners.
    fn drop(&mut self) {
        let _ = self.finish_uploads(Vec::new());
        let _ = self.core.device.poll(wgpu::PollType::wait_indefinitely());
        self.uploads.clear();
        self.submitted.clear();
        self.clears.clear();
        self.drain();
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
