//! Owned native realizations and one driver-local retirement/completion path for GL and WebGL.
use glow::HasContext;
use crate::{
    abi::*,
    command::{DrawRange, CommandCharge},
    native::NativeResources,
    resource::BackingCharge,
    scope::physical_rect,
};
use std::{
    ops::Deref,
    rc::Rc,
    sync::mpsc::{self, Sender, Receiver, SendError},
};
use super::{Core, Image, ImageAllocation, OutputState};
type BufferVersion = crate::resource::BufferVersion<Backend>;
type TextureVersion = crate::resource::TextureVersion<Backend>;
type SamplerVersion = crate::resource::SamplerVersion<Backend>;
type PipelineVersion = crate::resource::PipelineVersion<Backend>;
type Draw = crate::command::Draw<Backend>;
type Block = crate::record::Block<Backend>;
type Command = crate::command::Command<Backend>;

/// Statically selected resource family; its native objects are never Rust trait objects over COM.
pub struct Backend;
/// Exclusive native construction capability; only this backend owns an executor reference.
pub struct Preparation<'a> {
    /// The existing driver owns queues and completion tracking, not the shared physical core.
    executor: &'a mut Executor,
}
impl NativeResources for Backend {
    type Core = Core;
    type Buffer = Buffer;
    type Image = Image;
    type Sampler = Sampler;
    type Pipeline = Pipeline;
    type PipelineKey = bool;
    type Preparation<'a> = Preparation<'a>;
    fn identity(core: &Core) -> &neocogi_render_core::source::DriverIdentity {
        core.identity()
    }
    fn storage_available(core: &Core) -> Result<(), String> {
        core.current()
    }
    fn core<'a>(preparation: &'a Preparation<'_>) -> &'a Rc<Core> {
        &preparation.executor.core
    }
    fn check(preparation: &mut Preparation<'_>) -> Result<(), String> {
        preparation.executor.core.current()?;
        preparation.executor.check()
    }
    fn image_core(image: &Image) -> &Rc<Core> {
        &image.core
    }
    fn image_size(image: &Image) -> Size {
        image.size
    }
    fn buffer(source: &BufferVersion, preparation: &mut Preparation<'_>) -> Result<Rc<Buffer>, String> {
        preparation.executor.create_buffer(source)
    }
    /// BufferSubData preserves the same native name and fixed storage allocation.
    fn update_buffer(source: &BufferVersion, native: &Rc<Buffer>, preparation: &mut Preparation<'_>) -> Result<(), String> {
        let executor = &mut preparation.executor;
        executor.core.current()?;
        executor.uploads.try_reserve(1).map_err(|error| error.to_string())?;
        executor.uploads.push(Upload::Buffer { _resource: native.clone() });
        let target = match source.desc().role() {
            GpuBufferRole::Vertex => glow::ARRAY_BUFFER,
            GpuBufferRole::Index => glow::ELEMENT_ARRAY_BUFFER,
            GpuBufferRole::Uniform => glow::UNIFORM_BUFFER,
            _ => return Err("invalid linear role".into()),
        };
        let bytes = source.bytes().as_slice();
        // SAFETY: The owning context is current and common admission ended all
        // older logical/native use before these fixed-range writes.
        unsafe {
            executor.gl.bind_vertex_array(Some(executor.array));
            executor.gl.bind_buffer(target, Some(native.raw));
            executor.gl.buffer_sub_data_u8_slice(target, 0, bytes);
            executor.gl.bind_vertex_array(None);
        }
        executor.check()
    }
    /// Pixel replacement writes existing image storage; earlier captures forbid entry.
    fn update_image(source: &TextureVersion, native: &Rc<Image>, preparation: &mut Preparation<'_>) -> Result<(), String> {
        let executor = &mut preparation.executor;
        executor.core.current()?;
        executor.uploads.try_reserve(1).map_err(|error| error.to_string())?;
        executor.uploads.push(Upload::Image {
            _image: native.clone(),
            _receipt: Some(source.receipt()),
        });
        let bytes = source
            .pixels()
            .map(|bytes| bytes.as_slice())
            .ok_or("immutable imported image cannot be updated")?;
        let size = source.desc().size;
        // SAFETY: The full initialized RGBA extent matches this existing image,
        // and concrete completion/capture ownership ended before slot reuse.
        unsafe {
            executor.gl.bind_texture(glow::TEXTURE_2D, Some(native.raw));
            executor.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            executor.gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                size.width,
                size.height,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(bytes)),
            );
        }
        executor.check()
    }
    fn sampler(source: &SamplerVersion, preparation: &mut Preparation<'_>) -> Result<Rc<Sampler>, String> {
        preparation.executor.create_sampler(source)
    }
    fn pipeline(source: &PipelineVersion, key: &bool, preparation: &mut Preparation<'_>) -> Result<Rc<Pipeline>, String> {
        preparation.executor.create_pipeline(source, *key)
    }
    fn image(source: &TextureVersion, preparation: &mut Preparation<'_>) -> Result<Rc<Image>, String> {
        let executor = &mut preparation.executor;
        if let Some(image) = source.imported_image() {
            executor.uploads.try_reserve(1).map_err(|error| error.to_string())?;
            executor.uploads.push(Upload::Image {
                _image: image.clone(),
                _receipt: Some(source.receipt()),
            });
            return Ok(image.clone());
        }
        let desc = source.desc();
        let format = if desc.format == GpuTextureFormat::Rgba8Srgb {
            glow::SRGB8_ALPHA8
        } else {
            glow::RGBA8
        };
        executor.create_image(
            desc.size,
            format,
            Some(source.pixels().ok_or("missing pixel contents")?.as_slice()),
            Some(source.receipt()),
            true,
        )
    }
}

/// Owned packets contain no sender, driver, Domain, source version or public COM resource.
pub(in crate::backends::glow) enum Retirement {
    /// Vertex/index/uniform allocation and its opaque receipt.
    Buffer(BufferAllocation),
    /// Immutable sampling state.
    Sampler(SamplerAllocation),
    /// Linked program and source/dependency receipts only.
    Program(ProgramAllocation),
    /// Shared raw pixels, also retained independently by attachments when needed.
    Image(Rc<ImageAllocation>),
    /// Raw framebuffer plus sender-free image allocation lifetime.
    Framebuffer(super::FramebufferAllocation),
}
/// Driver-only retirement observes owned payloads without looking them up or reusing them.
fn discard(retired: Retirement) {
    match retired {
        Retirement::Buffer(value) => drop(value),
        Retirement::Sampler(value) => drop(value),
        Retirement::Program(value) => drop(value),
        Retirement::Image(value) => drop(value),
        Retirement::Framebuffer(value) => drop(value),
    }
}
/// Concrete shared storage uses one destruction rule for vertex/index buffers and uniform payloads.
macro_rules! allocation {
    ($backing:ident,$allocation:ident,$handle:ty,$delete:ident,$variant:ident) => {
        /// Self-contained actual native allocation with narrow lifetime and accounting only.
        pub struct $allocation {
            /// Actual owning context, independent of driver/queue lifetime.
            core: Rc<Core>,
            /// One native name, deleted exactly once after all actual uses have ended.
            raw: $handle,
            /// Reserved bytes/capacity are refunded after native destruction.
            _charge: BackingCharge,
        }
        impl Drop for $allocation {
            fn drop(&mut self) {
                if self.core.for_drop() {
                    // SAFETY: Physical context was selected and the last actual native owner is releasing.
                    unsafe { self.core.dispatch().$delete(self.raw) };
                }
            }
        }
        /// Notifying wrapper shared by immutable versions and actual native work.
        pub struct $backing {
            /// Only final Drop takes/moves allocation ownership into the message.
            allocation: Option<$allocation>,
            /// Producers stay outside allocation packets and hardware core.
            retired: Sender<Retirement>,
        }
        impl Deref for $backing {
            type Target = $allocation;
            fn deref(&self) -> &$allocation {
                self.allocation.as_ref().expect("live native backing")
            }
        }
        impl Drop for $backing {
            fn drop(&mut self) {
                if let Some(allocation) = self.allocation.take()
                    && let Err(SendError(retired)) = self.retired.send(Retirement::$variant(allocation))
                {
                    discard(retired);
                }
            }
        }
    };
}
allocation!(Buffer, BufferAllocation, glow::Buffer, delete_buffer, Buffer);
allocation!(Sampler, SamplerAllocation, glow::Sampler, delete_sampler, Sampler);
/// Linked program packet cannot retain its parent pipeline or any shader version.
pub struct ProgramAllocation {
    /// Program's physical context lifetime.
    core: Rc<Core>,
    /// Exactly one successfully linked native program.
    raw: glow::Program,
    /// Definition and both shader reservations, with no source-owner back-edge.
    _charges: [BackingCharge; 3],
}
impl Drop for ProgramAllocation {
    fn drop(&mut self) {
        if self.core.for_drop() {
            // SAFETY: This program's actual physical context is current and all native users ended.
            unsafe { self.core.dispatch().delete_program(self.raw) };
        }
    }
}
/// Native target variant retains immutable execution metadata, not its logical parent.
pub struct Pipeline {
    /// Moved to retirement only by final backing destruction.
    allocation: Option<ProgramAllocation>,
    /// Notification belongs to the backing, never the packet or context owner.
    retired: Sender<Retirement>,
    /// Exact fixed state needed by replay after the source version expires.
    info: GpuPipelineInfo,
    /// Checked attributes copied once at successful variant construction.
    attributes: Vec<GpuVertexAttribute>,
    /// Checked stream stepping copied once at successful variant construction.
    layouts: Vec<GpuVertexBufferLayout>,
    /// Private target-coordinate adaptation location, not shader-compiler target policy.
    target_height: Option<glow::UniformLocation>,
}
impl Deref for Pipeline {
    type Target = ProgramAllocation;
    fn deref(&self) -> &ProgramAllocation {
        self.allocation.as_ref().expect("live native program")
    }
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take()
            && let Err(SendError(retired)) = self.retired.send(Retirement::Program(allocation))
        {
            discard(retired);
        }
    }
}
/// Opaque prepared native commands can outlive public resources and their driver facade.
#[derive(Clone)]
pub struct PreparedBlock {
    /// Native-only snapshots and original metadata accounting.
    data: Rc<PreparedData>,
}
#[cfg(test)]
impl PreparedBlock {
    /// Native test observation checks actual warm program reuse without an instrumentation cache.
    pub(super) fn first_pipeline(&self) -> Rc<Pipeline> {
        let PreparedCommand::Draw(draw) = &self.data.commands[0] else {
            panic!("draw")
        };
        draw.pipeline.clone()
    }
}
/// Native representations retain no entire sealed block or Domain.
struct PreparedData {
    /// Concrete compatible physical context, never the renderer facade.
    core: Rc<Core>,
    /// Coordinate/winding adaptation is part of the prepared target contract.
    offscreen: bool,
    /// Exact painter-order native operations.
    commands: Vec<PreparedCommand>,
    /// Original source capacity, no admission authority.
    _capacity: Vec<CommandCharge>,
}
/// Real fence ownership covers draws, initialization and output attachments independently.
struct Submitted {
    /// Fence created on this same context; successful observation releases the complete batch.
    fence: glow::Fence,
    /// Actual native resources for every issued draw.
    _blocks: Vec<PreparedBlock>,
    /// Actual unpublished/native initialization owners, not CPU sources.
    _uploads: Vec<Upload>,
    /// Destination owner remains present for clear-only and uncertain frames too.
    _targets: Vec<Rc<OutputState>>,
    /// Original capacity remains charged through asynchronous initialization.
    _capacity: Vec<CommandCharge>,
}
/// Issued initialization tracks actual storage even before its source slot can be published.
enum Upload {
    /// Vertex/index/uniform creation share native storage ownership.
    Buffer { _resource: Rc<Buffer> },
    /// Imports retain their own use charge independently from a shared image's lifetime.
    Image { _image: Rc<Image>, _receipt: Option<BackingCharge> },
}
/// Complete immutable native draw, with no source versions or client authority.
struct PreparedDraw {
    /// Linked target variant and copied fixed/layout state.
    pipeline: Rc<Pipeline>,
    /// Exact slots, native allocation owners and validated byte bases.
    vertices: Vec<(u32, Rc<Buffer>, u64)>,
    /// Exact index allocation, interpretation and byte base.
    index: Option<(Rc<Buffer>, GpuIndexFormat, u64)>,
    /// Image/sampler pairs retain per-import use accounting independently of physical sharing.
    textures: Vec<(u32, Rc<Image>, Rc<Sampler>, BackingCharge)>,
    /// Exact captured persistent uniform backing and its selected byte range.
    uniforms: Vec<(u32, Rc<Buffer>, u64, u64)>,
    /// Already checked direct primitive range.
    range: DrawRange,
    /// Flattened host-bounded viewport mapping.
    viewport: Rect,
    /// Flattened host-bounded raster-write region.
    clip: Rect,
}
/// One closed native operation set shared by UI and clients.
enum PreparedCommand {
    /// Exact immutable binding state.
    Draw(PreparedDraw),
    /// Unblended color replacement inside an admitted clip.
    Color(u32, Rect),
    /// Regional normalized depth replacement.
    Depth(f32, Rect),
}
/// Existing driver owns work and retirement endpoints; no global resource realization maps remain.
pub(in crate::backends::glow) struct Executor {
    /// Dispatch selected through the actual physical owner.
    gl: Rc<glow::Context>,
    /// Hardware-only shared lifetime, with neither channel endpoint nor submitted work.
    core: Rc<Core>,
    /// Private vertex layout is driver state, not a client resource.
    array: glow::VertexArray,
    /// Supplies clones only when concrete native backings are realized.
    retired_tx: Sender<Retirement>,
    /// Sole receiver for this existing driver state.
    retired_rx: Receiver<Retirement>,
    /// Actual issued immediate draw owners survive frame abandonment.
    pending: Vec<PreparedBlock>,
    /// Initialization is tracked even without an acquired rendering target.
    uploads: Vec<Upload>,
    /// Clear-only outputs need attachment ownership independently of block count.
    pending_targets: Vec<Rc<OutputState>>,
    /// Real native fence/completion ownership.
    submitted: Vec<Submitted>,
    /// Source capacity follows preparatory native work through completion.
    upload_capacity: Vec<CommandCharge>,
}
impl Executor {
    /// Creates only driver-private state and transport; resources remain lazy object-owned realizations.
    pub(in crate::backends::glow) fn new(core: Rc<Core>) -> Result<Self, String> {
        core.current()?;
        let gl = core.dispatch().clone();
        // SAFETY: The owning physical context is current.
        let array = unsafe { gl.create_vertex_array()? };
        let (retired_tx, retired_rx) = mpsc::channel();
        Ok(Self {
            gl,
            core,
            array,
            retired_tx,
            retired_rx,
            pending: Vec::new(),
            uploads: Vec::new(),
            pending_targets: Vec::new(),
            submitted: Vec::new(),
            upload_capacity: Vec::new(),
        })
    }
    /// Only concrete native creation receives a producer clone.
    pub(in crate::backends::glow) fn sender(&self) -> Sender<Retirement> {
        self.retired_tx.clone()
    }
    /// Drops safe owned packets; a failed poll never prevents unrelated retirement maintenance.
    fn drain(&self) {
        while let Ok(retired) = self.retired_rx.try_recv() {
            discard(retired);
        }
    }
    /// Polls completion then drains, preserving useful first-error reporting.
    pub(in crate::backends::glow) fn maintain(&mut self) -> Result<(), String> {
        let result = self.poll();
        self.drain();
        result
    }
    /// Reclaims previous work before subsequent preparation, even when the next frame is empty.
    pub(in crate::backends::glow) fn collect(&mut self) -> Result<(), String> {
        self.maintain()
    }
    /// Observes actual completion; no reference count or frame return substitutes for a fence.
    fn poll(&mut self) -> Result<(), String> {
        self.core.current()?;
        self.check()?;
        self.complete()?;
        let mut failure = false;
        self.submitted.retain(|batch| {
            // SAFETY: The matching live context owns every fence; a zero timeout only observes.
            match unsafe { self.gl.client_wait_sync(batch.fence, 0, 0) } {
                glow::ALREADY_SIGNALED | glow::CONDITION_SATISFIED => {
                    unsafe { self.gl.delete_sync(batch.fence) };
                    false
                }
                glow::TIMEOUT_EXPIRED => true,
                _ => {
                    failure = true;
                    true
                }
            }
        });
        if failure {
            Err("failed to observe graphics completion".into())
        } else {
            Ok(())
        }
    }
    /// Seals all issued immediate work; failed allocation/fence creation leaves its owners installed.
    pub(in crate::backends::glow) fn complete(&mut self) -> Result<(), String> {
        self.core.current()?;
        if self.pending.is_empty() && self.uploads.is_empty() && self.pending_targets.is_empty() {
            return Ok(());
        }
        self.submitted.try_reserve(1).map_err(|error| error.to_string())?;
        // SAFETY: Actual owners are already installed before fence creation/submission can fail.
        let fence = unsafe { self.gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0)? };
        unsafe { self.gl.flush() };
        self.submitted.push(Submitted {
            fence,
            _blocks: std::mem::take(&mut self.pending),
            _uploads: std::mem::take(&mut self.uploads),
            _targets: std::mem::take(&mut self.pending_targets),
            _capacity: std::mem::take(&mut self.upload_capacity),
        });
        Ok(())
    }
    /// Installs target lifetime before the host clear, independently of client block count.
    pub(in crate::backends::glow) fn retain_target(&mut self, output: Rc<OutputState>) -> Result<(), String> {
        self.pending_targets.try_reserve(1).map_err(|error| error.to_string())?;
        self.pending_targets.push(output);
        Ok(())
    }
    /// Complete immutable block realization precedes target acquisition and has no foreign callbacks.
    pub(in crate::backends::glow) fn prepare(&mut self, block: &Block, offscreen: bool) -> Result<PreparedBlock, String> {
        block.admit_on(&self.core).map_err(|error| error.to_string())?;
        self.collect()?;
        let capacity = block.retain_capacity();
        self.upload_capacity.try_reserve(capacity.len()).map_err(|error| error.to_string())?;
        let before = self.uploads.len();
        let result = self.prepare_inner(block, offscreen);
        if self.uploads.len() > before {
            self.upload_capacity.extend(capacity);
        }
        result
    }
    /// Partial failure leaves actual initialized candidates and charges in the upload tracker.
    fn prepare_inner(&mut self, block: &Block, offscreen: bool) -> Result<PreparedBlock, String> {
        let mut commands = Vec::new();
        commands.try_reserve_exact(block.commands().len()).map_err(|error| error.to_string())?;
        for command in block.commands() {
            commands.push(match command {
                Command::ClearColor { rgba, clip, .. } => PreparedCommand::Color(*rgba, *clip),
                Command::ClearDepth { depth, clip, .. } => PreparedCommand::Depth(*depth, *clip),
                Command::Draw(draw) => PreparedCommand::Draw(self.prepare_draw(draw, offscreen)?),
            });
        }
        self.check()?;
        Ok(PreparedBlock {
            data: Rc::new(PreparedData {
                core: self.core.clone(),
                offscreen,
                commands,
                _capacity: block.retain_capacity(),
            }),
        })
    }
    /// Captures native owners and independent import receipts, never the source draw or sealed block.
    fn prepare_draw(&mut self, draw: &Draw, offscreen: bool) -> Result<PreparedDraw, String> {
        let mut preparation = Preparation { executor: self };
        let pipeline = draw.pipeline.native(offscreen, &mut preparation)?;
        let mut vertices = Vec::new();
        vertices.try_reserve_exact(draw.vertex_buffers.len()).map_err(|error| error.to_string())?;
        for binding in &draw.vertex_buffers {
            vertices.push((binding.slot, binding.buffer.native(&mut preparation)?, binding.offset));
        }
        let index = draw
            .index
            .as_ref()
            .map(|binding| binding.buffer.native(&mut preparation).map(|native| (native, binding.format, binding.offset)))
            .transpose()?;
        let mut textures = Vec::new();
        textures.try_reserve_exact(draw.textures.len()).map_err(|error| error.to_string())?;
        for binding in &draw.textures {
            textures.push((
                unit(binding.stage, binding.slot),
                binding.texture.native(&mut preparation)?,
                binding.sampler.native(&mut preparation)?,
                binding.texture.receipt(),
            ));
        }
        let mut uniforms = Vec::new();
        uniforms.try_reserve_exact(draw.uniforms.len()).map_err(|error| error.to_string())?;
        for binding in &draw.uniforms {
            uniforms.push((
                unit(binding.stage, binding.slot),
                binding.data.native(&mut preparation)?,
                binding.data.offset(),
                binding.data.byte_len(),
            ));
        }
        Ok(PreparedDraw {
            pipeline,
            vertices,
            index,
            textures,
            uniforms,
            range: draw.range,
            viewport: draw.viewport,
            clip: draw.clip,
        })
    }
    /// Common typed atlas preparation uses the exact version's own image slot without target work.
    pub(in crate::backends::glow) fn prepare_texture(&mut self, source: Rc<TextureVersion>) -> Result<(), String> {
        self.collect()?;
        source.native(&mut Preparation { executor: self }).map(|_| ())
    }
    /// Creates actual storage and installs initialization ownership before issuing any write.
    fn create_buffer(&mut self, source: &BufferVersion) -> Result<Rc<Buffer>, String> {
        self.uploads.try_reserve(1).map_err(|error| error.to_string())?;
        // SAFETY: The checked owning preparation core is current.
        unsafe {
            let value = Rc::new(Buffer {
                allocation: Some(BufferAllocation {
                    core: self.core.clone(),
                    raw: self.gl.create_buffer()?,
                    _charge: source.receipt(),
                }),
                retired: self.sender(),
            });
            self.uploads.push(Upload::Buffer { _resource: value.clone() });
            self.gl.bind_vertex_array(Some(self.array));
            let target = match source.desc().role() {
                GpuBufferRole::Vertex => glow::ARRAY_BUFFER,
                GpuBufferRole::Index => glow::ELEMENT_ARRAY_BUFFER,
                GpuBufferRole::Uniform => glow::UNIFORM_BUFFER,
                _ => return Err("texture role cannot allocate a native linear buffer".into()),
            };
            self.gl.bind_buffer(target, Some(value.raw));
            let hint = match source.usage() {
                GpuBufferUsage::Static => glow::STATIC_DRAW,
                GpuBufferUsage::Dynamic => glow::DYNAMIC_DRAW,
                GpuBufferUsage::Streamed => glow::STREAM_DRAW,
                _ => return Err("invalid buffer usage".into()),
            };
            self.gl.buffer_data_u8_slice(target, source.bytes().as_slice(), hint);
            self.gl.bind_vertex_array(None);
            self.check()?;
            Ok(value)
        }
    }
    /// One native sampling object per immutable version, independent of public owner lifetime.
    fn create_sampler(&mut self, source: &SamplerVersion) -> Result<Rc<Sampler>, String> {
        // SAFETY: Portable descriptor enums were admitted and this preparation core is current.
        unsafe {
            let value = Rc::new(Sampler {
                allocation: Some(SamplerAllocation {
                    core: self.core.clone(),
                    raw: self.gl.create_sampler()?,
                    _charge: source.receipt(),
                }),
                retired: self.sender(),
            });
            let desc = source.desc();
            self.gl
                .sampler_parameter_i32(value.raw, glow::TEXTURE_MIN_FILTER, filter(desc.min_filter) as i32);
            self.gl
                .sampler_parameter_i32(value.raw, glow::TEXTURE_MAG_FILTER, filter(desc.mag_filter) as i32);
            self.gl.sampler_parameter_i32(value.raw, glow::TEXTURE_WRAP_S, address(desc.address_u) as i32);
            self.gl.sampler_parameter_i32(value.raw, glow::TEXTURE_WRAP_T, address(desc.address_v) as i32);
            self.check()?;
            Ok(value)
        }
    }
    /// Unified allocation for sampled images, ordinary images and private unpublished output storage.
    /// False initialization is only used by single-use Output, whose complete host clear precedes publication.
    pub(in crate::backends::glow) fn create_image(
        &mut self,
        size: Size,
        format: u32,
        pixels: Option<&[u8]>,
        charge: Option<BackingCharge>,
        initialize: bool,
    ) -> Result<Rc<Image>, String> {
        neocogi_render_core::checked_rgba_len(size.width, size.height).map_err(|error| error.to_string())?;
        if let Some(pixels) = pixels {
            neocogi_render_core::ImageSource {
                width: size.width,
                height: size.height,
                pixels,
            }
            .validate()
            .map_err(|error| error.to_string())?;
        }
        self.core.current()?;
        self.check()?;
        // SAFETY: Read an immutable limit on the actual current context before allocation.
        let limit = unsafe { self.gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) };
        if size.width > limit || size.height > limit {
            return Err(format!("texture {}x{} exceeds native dimension limit {limit}", size.width, size.height));
        }
        if initialize {
            self.uploads.try_reserve(1).map_err(|error| error.to_string())?;
        }
        // SAFETY: Extent/data admitted, real context selected; ownership installed before initialization.
        unsafe {
            let image = Rc::new(Image {
                allocation: Rc::new(ImageAllocation {
                    core: self.core.clone(),
                    raw: self.gl.create_texture()?,
                    size,
                    _charge: charge,
                }),
                retired: self.sender(),
            });
            if initialize {
                self.uploads.push(Upload::Image { _image: image.clone(), _receipt: None });
            }
            super::GLRenderer::prepare_upload(&self.gl);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(image.raw));
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_BASE_LEVEL, 0);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, 0);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                format as i32,
                size.width,
                size.height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(pixels),
            );
            self.check()?;
            if initialize && pixels.is_none() {
                let zero = &super::ZERO_BYTES;
                for y in 0..size.height {
                    for x in (0..size.width).step_by(zero.len() / 4) {
                        let count = (size.width - x).min((zero.len() / 4) as i32);
                        self.gl.tex_sub_image_2d(
                            glow::TEXTURE_2D,
                            0,
                            x,
                            y,
                            count,
                            1,
                            glow::RGBA,
                            glow::UNSIGNED_BYTE,
                            glow::PixelUnpackData::Slice(Some(&zero[..count as usize * 4])),
                        );
                    }
                }
                self.check()?;
            }
            self.gl.bind_texture(glow::TEXTURE_2D, None);
            Ok(image)
        }
    }
    /// Native metadata copy is bounded/fallible before native program construction.
    fn copied<T: Copy>(source: &[T]) -> Result<Vec<T>, String> {
        let mut output = Vec::new();
        output.try_reserve_exact(source.len()).map_err(|error| error.to_string())?;
        output.extend_from_slice(source);
        Ok(output)
    }
    /// Target-dependent compiler adaptation stays in this renderer and is memoized on the source owner.
    fn create_pipeline(&mut self, source: &PipelineVersion, offscreen: bool) -> Result<Rc<Pipeline>, String> {
        let info = source.info();
        if offscreen && (info.depth_write || info.depth_compare != GpuCompare::Always) {
            return Err("offscreen target has no depth attachment".into());
        }
        let attributes = Self::copied(source.attributes())?;
        let layouts = Self::copied(source.vertex_buffers())?;
        let vertex = super::shader_adapter::vertex(&source.vertex().artifact().glsl, offscreen)?;
        let (fragment, height_name) = super::shader_adapter::fragment(&source.fragment().artifact().glsl, offscreen)?;
        let raw = super::create_program(&self.gl, &vertex, &fragment).map_err(|error| error.to_string())?;
        // SAFETY: The linked program belongs to this checked current context and exact reflected stages.
        let target_height = height_name.and_then(|name| unsafe { self.gl.get_uniform_location(raw, &name) });
        let value = Rc::new(Pipeline {
            allocation: Some(ProgramAllocation {
                core: self.core.clone(),
                raw,
                _charges: source.receipts(),
            }),
            retired: self.sender(),
            info,
            attributes,
            layouts,
            target_height,
        });
        unsafe {
            self.gl.use_program(Some(raw));
            for shader in [source.vertex().artifact(), source.fragment().artifact()] {
                for uniform in &shader.uniforms {
                    if let Some(index) = self.gl.get_uniform_block_index(raw, &uniform.gl_block) {
                        self.gl.uniform_block_binding(raw, index, unit(shader.stage, uniform.slot));
                    }
                }
                for texture in &shader.textures {
                    let location = self.gl.get_uniform_location(raw, &texture.gl_name);
                    self.gl.uniform_1_i32(location.as_ref(), unit(shader.stage, texture.slot) as i32);
                }
            }
            self.gl.use_program(None);
        }
        self.check()?;
        Ok(value)
    }
    /// Reports native errors before publication; terminal context loss invalidates all old handles.
    pub(in crate::backends::glow) fn check(&self) -> Result<(), String> {
        // SAFETY: Called only in a selected live native execution phase.
        let error = unsafe { self.gl.get_error() };
        if error == glow::NO_ERROR {
            Ok(())
        } else {
            let reason = format!("OpenGL operation failed with error 0x{error:04x}");
            if error == glow::CONTEXT_LOST {
                self.core.lose(reason.clone());
            }
            Err(reason)
        }
    }
    /// Replays only prepared native objects; the caller has already bound its authorized target.
    pub(in crate::backends::glow) fn execute(&mut self, block: &PreparedBlock, logical: Size, physical: Size, offscreen: bool) -> Result<(), String> {
        if !Rc::ptr_eq(&self.core, &block.data.core) || offscreen != block.data.offscreen {
            return Err("prepared GL block belongs to another driver or target".into());
        }
        // Retain before the first native command, including partial failure after an earlier draw.
        self.pending.try_reserve(1).map_err(|error| error.to_string())?;
        self.pending.push(block.clone());
        let gl = &self.gl;
        for command in &block.data.commands {
            let clip = match command {
                PreparedCommand::Draw(draw) => draw.clip,
                PreparedCommand::Color(_, clip) | PreparedCommand::Depth(_, clip) => *clip,
            };
            let Some(clip) = physical_rect(clip, logical, physical).map_err(|error| error.to_string())? else {
                continue;
            };
            let y = if offscreen { clip.y } else { physical.height as u32 - clip.y - clip.height };
            // SAFETY: Checked physical bounds fit the live attachment. No command selects a target.
            unsafe {
                gl.enable(glow::SCISSOR_TEST);
                gl.scissor(clip.x as i32, y as i32, clip.width as i32, clip.height as i32);
                gl.disable(glow::RASTERIZER_DISCARD);
                gl.disable(glow::STENCIL_TEST);
                gl.disable(glow::DITHER);
                gl.disable(glow::SAMPLE_COVERAGE);
                gl.disable(glow::SAMPLE_ALPHA_TO_COVERAGE);
                gl.disable(glow::POLYGON_OFFSET_FILL);
                match command {
                    PreparedCommand::Color(rgba, _) => {
                        let [r, g, b, a] = rgba.to_le_bytes().map(|byte| f32::from(byte) / 255.0);
                        gl.color_mask(true, true, true, true);
                        gl.clear_color(r, g, b, a);
                        gl.clear(glow::COLOR_BUFFER_BIT);
                    }
                    PreparedCommand::Depth(depth, _) => {
                        gl.depth_mask(true);
                        gl.clear_depth_f32(*depth);
                        gl.clear(glow::DEPTH_BUFFER_BIT);
                    }
                    PreparedCommand::Draw(draw) => self.draw(draw, logical, physical, offscreen)?,
                }
            }
        }
        // SAFETY: Unbind only this executor's private layout/program, while the context is current.
        unsafe {
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
        self.check()
    }

    /// Establishes complete fixed/binding state and issues one checked direct triangle command.
    fn draw(&self, draw: &PreparedDraw, logical: Size, physical: Size, offscreen: bool) -> Result<(), String> {
        let Some(view) = physical_rect(draw.viewport, logical, physical).map_err(|error| error.to_string())? else {
            return Ok(());
        };
        let gl = &self.gl;
        let info = draw.pipeline.info;
        let y = if offscreen { view.y } else { physical.height as u32 - view.y - view.height };
        // SAFETY: Prepared resources belong to this context; every byte range and layout is checked
        // by immutable draw admission. CPU snapshots cannot be changed during this replay.
        unsafe {
            gl.viewport(view.x as i32, y as i32, view.width as i32, view.height as i32);
            gl.depth_range_f32(0.0, 1.0);
            gl.depth_mask(info.depth_write);
            if info.depth_compare == GpuCompare::Always && !info.depth_write {
                gl.disable(glow::DEPTH_TEST);
            } else {
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(compare(info.depth_compare));
            }
            gl.color_mask(
                info.color_write & 1 != 0,
                info.color_write & 2 != 0,
                info.color_write & 4 != 0,
                info.color_write & 8 != 0,
            );
            if info.blend.enabled {
                gl.enable(glow::BLEND);
                gl.blend_func_separate(
                    factor(info.blend.src_color),
                    factor(info.blend.dst_color),
                    factor(info.blend.src_alpha),
                    factor(info.blend.dst_alpha),
                );
                gl.blend_equation_separate(blend_op(info.blend.color_op), blend_op(info.blend.alpha_op));
            } else {
                gl.disable(glow::BLEND);
            }
            if info.cull == GpuCullMode::None {
                gl.disable(glow::CULL_FACE);
            } else {
                gl.enable(glow::CULL_FACE);
                gl.cull_face(if info.cull == GpuCullMode::Front { glow::FRONT } else { glow::BACK });
            }
            let ccw = (info.front_face == GpuFrontFace::Ccw) != offscreen;
            gl.front_face(if ccw { glow::CCW } else { glow::CW });
            gl.use_program(Some(draw.pipeline.raw));
            gl.uniform_1_f32(draw.pipeline.target_height.as_ref(), physical.height as f32);
            gl.bind_vertex_array(Some(self.array));
            for location in 0..16 {
                gl.disable_vertex_attrib_array(location);
                gl.vertex_attrib_divisor(location, 0);
            }
            for attribute in &draw.pipeline.attributes {
                let (slot, buffer, offset) = draw
                    .vertices
                    .iter()
                    .find(|(slot, _, _)| *slot == attribute.slot)
                    .ok_or("prepared vertex slot missing")?;
                let layout = draw
                    .pipeline
                    .layouts
                    .iter()
                    .find(|layout| layout.slot == *slot)
                    .ok_or("prepared vertex layout missing")?;
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer.raw));
                gl.enable_vertex_attrib_array(attribute.location);
                let (components, kind, normalized) = vertex_format(attribute.format);
                let offset = (*offset + u64::from(attribute.offset)) as i32;
                if kind == glow::UNSIGNED_INT {
                    gl.vertex_attrib_pointer_i32(attribute.location, components, kind, layout.stride as i32, offset);
                } else {
                    gl.vertex_attrib_pointer_f32(attribute.location, components, kind, normalized, layout.stride as i32, offset);
                }
                gl.vertex_attrib_divisor(attribute.location, if layout.step == GpuStepMode::Instance { 1 } else { 0 });
            }
            for (unit, texture, sampler, _) in &draw.textures {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture.raw));
                gl.bind_sampler(*unit, Some(sampler.raw));
            }
            for (unit, uniform, offset, len) in &draw.uniforms {
                gl.bind_buffer_range(glow::UNIFORM_BUFFER, *unit, Some(uniform.raw), *offset as i32, *len as i32);
            }
            match draw.range {
                DrawRange::Vertices { first, count, instances } => gl.draw_arrays_instanced(glow::TRIANGLES, first as i32, count as i32, instances as i32),
                DrawRange::Indices { first, count, instances } => {
                    let (buffer, format, offset) = draw.index.as_ref().ok_or("prepared index binding missing")?;
                    let (kind, size) = if *format == GpuIndexFormat::U16 {
                        (glow::UNSIGNED_SHORT, 2)
                    } else {
                        (glow::UNSIGNED_INT, 4)
                    };
                    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(buffer.raw));
                    gl.draw_elements_instanced(glow::TRIANGLES, count as i32, kind, (offset + u64::from(first) * size) as i32, instances as i32);
                }
            }
        }
        Ok(())
    }
}

impl Drop for Executor {
    /// Settle actual work before dropping tracked owners and disconnecting the receiver.
    fn drop(&mut self) {
        if self.core.for_drop() {
            // SAFETY: Owning context selected; finish precedes every final issued-work owner release.
            unsafe {
                self.gl.finish();
                for batch in self.submitted.drain(..) {
                    self.gl.delete_sync(batch.fence);
                }
                self.gl.delete_vertex_array(self.array);
            }
        }
        self.pending.clear();
        self.uploads.clear();
        self.pending_targets.clear();
        self.submitted.clear();
        self.upload_capacity.clear();
        self.drain();
    }
}

/// Flattens the bounded stage-local resource namespace into native GL binding indices.
fn unit(stage: GpuShaderStage, slot: u32) -> u32 {
    if stage == GpuShaderStage::Vertex { slot } else { 4 + slot }
}
/// Maps one admitted non-mip filter.
fn filter(value: GpuFilter) -> u32 {
    if value == GpuFilter::Nearest { glow::NEAREST } else { glow::LINEAR }
}
/// Maps one admitted address policy.
fn address(value: GpuAddressMode) -> u32 {
    match value {
        GpuAddressMode::Clamp => glow::CLAMP_TO_EDGE,
        GpuAddressMode::Repeat => glow::REPEAT,
        _ => glow::MIRRORED_REPEAT,
    }
}

/// Maps an admitted complete depth comparison.
fn compare(value: GpuCompare) -> u32 {
    match value {
        GpuCompare::Never => glow::NEVER,
        GpuCompare::Less => glow::LESS,
        GpuCompare::Equal => glow::EQUAL,
        GpuCompare::LessEqual => glow::LEQUAL,
        GpuCompare::Greater => glow::GREATER,
        GpuCompare::NotEqual => glow::NOTEQUAL,
        GpuCompare::GreaterEqual => glow::GEQUAL,
        _ => glow::ALWAYS,
    }
}
/// Maps admitted separate color/alpha blend factors.
fn factor(value: GpuBlendFactor) -> u32 {
    match value {
        GpuBlendFactor::Zero => glow::ZERO,
        GpuBlendFactor::One => glow::ONE,
        GpuBlendFactor::SrcColor => glow::SRC_COLOR,
        GpuBlendFactor::OneMinusSrcColor => glow::ONE_MINUS_SRC_COLOR,
        GpuBlendFactor::DstColor => glow::DST_COLOR,
        GpuBlendFactor::OneMinusDstColor => glow::ONE_MINUS_DST_COLOR,
        GpuBlendFactor::SrcAlpha => glow::SRC_ALPHA,
        GpuBlendFactor::OneMinusSrcAlpha => glow::ONE_MINUS_SRC_ALPHA,
        GpuBlendFactor::DstAlpha => glow::DST_ALPHA,
        _ => glow::ONE_MINUS_DST_ALPHA,
    }
}
/// Maps an admitted unblended/min/max/add/subtract equation.
fn blend_op(value: GpuBlendOp) -> u32 {
    match value {
        GpuBlendOp::Add => glow::FUNC_ADD,
        GpuBlendOp::Subtract => glow::FUNC_SUBTRACT,
        GpuBlendOp::ReverseSubtract => glow::FUNC_REVERSE_SUBTRACT,
        GpuBlendOp::Min => glow::MIN,
        _ => glow::MAX,
    }
}
/// Interprets portable encoded attributes; integer inputs never use float conversion.
fn vertex_format(value: GpuVertexFormat) -> (i32, u32, bool) {
    match value {
        GpuVertexFormat::Float32 => (1, glow::FLOAT, false),
        GpuVertexFormat::Float32x2 => (2, glow::FLOAT, false),
        GpuVertexFormat::Float32x3 => (3, glow::FLOAT, false),
        GpuVertexFormat::Float32x4 => (4, glow::FLOAT, false),
        GpuVertexFormat::Uint32 => (1, glow::UNSIGNED_INT, false),
        GpuVertexFormat::Uint32x2 => (2, glow::UNSIGNED_INT, false),
        GpuVertexFormat::Uint32x3 => (3, glow::UNSIGNED_INT, false),
        GpuVertexFormat::Uint32x4 => (4, glow::UNSIGNED_INT, false),
        _ => (4, glow::UNSIGNED_BYTE, true),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "mapping_tests.rs"]
mod mapping_tests;
