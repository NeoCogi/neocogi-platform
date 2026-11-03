//! Direct Vulkan realization of the shared immutable graphics vocabulary.
//! Native allocations retain a narrow device core and opaque accounting, without source versions.
use ash::{Entry, vk};
use crate::{
    abi::*,
    command::{DrawRange, CommandCharge},
    resource::BackingCharge,
    native::NativeResources,
    scope::physical_rect,
};
use crate::resource_guard::ResourceGuard;
use std::{
    cell::Cell,
    ops::Deref,
    sync::mpsc::{self, Sender, Receiver, SendError},
    ffi::CString,
    rc::Rc,
};

type Block = crate::record::Block<Backend>;
type Command = crate::command::Command<Backend>;
type Draw = crate::command::Draw<Backend>;
type BufferVersion = crate::resource::BufferVersion<Backend>;
type TextureVersion = crate::resource::TextureVersion<Backend>;
type SamplerVersion = crate::resource::SamplerVersion<Backend>;
type PipelineVersion = crate::resource::PipelineVersion<Backend>;
type UniformData = crate::command::UniformData<Backend>;
/// Statically selected concrete Vulkan resource family.
pub struct Backend;
/// Exclusive native construction capability; render-pass compatibility comes only from the host.
pub struct Preparation<'a> {
    /// Exclusive existing driver; native construction never borrows a foreign capability.
    executor: &'a mut Executor,
    /// Host-selected compatible render pass, borrowed only for native pipeline construction.
    pass: vk::RenderPass,
}
impl NativeResources for Backend {
    type Core = Core;
    type Buffer = Buffer;
    type Image = Texture;
    type Sampler = Sampler;
    type Pipeline = Pipeline;
    type Uniform = Buffer;
    type PipelineKey = Signature;
    type Preparation<'a> = Preparation<'a>;
    fn identity(core: &Core) -> &neocogi_render_core::source::DriverIdentity {
        &core.identity
    }
    fn core<'a>(preparation: &'a Preparation<'_>) -> &'a Rc<Core> {
        &preparation.executor.core
    }
    fn check(preparation: &mut Preparation<'_>) -> Result<(), String> {
        if preparation.executor.unusable() {
            Err("Vulkan executor is unavailable".into())
        } else {
            preparation.executor.core.check()
        }
    }
    fn image_core(image: &Texture) -> &Rc<Core> {
        &image.native.core
    }
    fn image_size(image: &Texture) -> Size {
        image.size
    }
    fn buffer(source: &BufferVersion, preparation: &mut Preparation<'_>) -> Result<Rc<Buffer>, String> {
        preparation.executor.buffer(source)
    }
    fn sampler(source: &SamplerVersion, preparation: &mut Preparation<'_>) -> Result<Rc<Sampler>, String> {
        preparation.executor.sampler(source)
    }
    fn uniform(source: &UniformData, preparation: &mut Preparation<'_>) -> Result<Rc<Buffer>, String> {
        preparation.executor.uniform(source)
    }
    fn pipeline(source: &PipelineVersion, key: &Signature, preparation: &mut Preparation<'_>) -> Result<Rc<Pipeline>, String> {
        preparation.executor.pipeline(source, preparation.pass, *key)
    }
    fn image(source: &TextureVersion, preparation: &mut Preparation<'_>) -> Result<Rc<Texture>, String> {
        if let Some(image) = source.imported_image() {
            return Ok(image.clone());
        }
        preparation.executor.create_image(
            source.desc().size,
            if source.desc().format == GpuTextureFormat::Rgba8Srgb {
                vk::Format::R8G8B8A8_SRGB
            } else {
                vk::Format::R8G8B8A8_UNORM
            },
            source.pixels().and_then(|p| p.as_slice()),
            Some(source.receipt()),
        )
    }
}
/// Complete owned packets contain no logical source, domain, driver or producer endpoint.
pub(in crate::backends::vulkan) enum Retirement {
    /// Vertex/index/uniform storage and its opaque receipt.
    Buffer(BufferAllocation),
    /// Independent native sampling state.
    Sampler(SamplerAllocation),
    /// One target-compatible executable and dependency receipts.
    Pipeline(PipelineAllocation),
    /// Shared immutable native pixels and original allocation charge.
    Texture(TextureAllocation),
    /// Native framebuffer and its sender-free raw image owner.
    Framebuffer(super::FramebufferAllocation),
}
impl Retirement {
    /// Exhaustively destroy the returned/drained owned packet; payloads contain no producer.
    pub(in crate::backends::vulkan) fn discard(self) {
        match self {
            Self::Buffer(value) => drop(value),
            Self::Sampler(value) => drop(value),
            Self::Pipeline(value) => drop(value),
            Self::Texture(value) => drop(value),
            Self::Framebuffer(value) => drop(value),
        }
    }
}
macro_rules! notifying {
    ($name:ident,$allocation:ident) => {
        /// Native last-owner notification; the owned packet itself contains no producer back-edge.
        pub struct $name {
            /// Taken once; the notifying wrapper and producer never enter the packet.
            owner: Option<$allocation>,
            /// Producer exists only on this native backing; disconnected sends recover ownership.
            retired: Sender<Retirement>,
        }
        impl Deref for $name {
            type Target = $allocation;
            fn deref(&self) -> &$allocation {
                self.owner.as_ref().expect("live native backing")
            }
        }
        impl Drop for $name {
            fn drop(&mut self) {
                if let Some(owner) = self.owner.take()
                    && let Err(SendError(packet)) = self.retired.send(Retirement::$name(owner))
                {
                    packet.discard();
                }
            }
        }
    };
}

/// Raw device/instance ownership is independent of presentation, caches and client domains.
pub struct Core {
    /// Retained metadata token has no native back-edge.
    identity: neocogi_render_core::source::DriverIdentity,
    /// A native device-loss observation permanently invalidates every old slot.
    lost: Cell<bool>,
    /// Loader must remain live until instance and device destruction completes.
    _entry: Entry,
    /// Parent of the logical device; no swapchain or cache is owned here.
    instance: ash::Instance,
    /// Shared native dispatch and allocation owner, never a client-facing capability.
    pub(in crate::backends::vulkan) device: ash::Device,
    /// Query handle and immutable native limits belong to this exact logical device.
    physical: vk::PhysicalDevice,
    /// Logical byte/resource budgets cannot replace Vulkan's per-device validity limits.
    limits: vk::PhysicalDeviceLimits,
    /// Counts every allocation on this device, including host UI and ordinary images.
    memory_count: Cell<u32>,
    /// Count is conservative if the driver interns identical sampler descriptors.
    sampler_count: Cell<u32>,
    /// Physical memory types used for explicit host/device allocations.
    memory: vk::PhysicalDeviceMemoryProperties,
    /// Preparation uploads and final graphics use this same queue.
    pub(super) queue: vk::Queue,
    /// Graphics family determines the executor's transient upload command pool.
    pub(super) family: u32,
}
impl Core {
    /// Authoritative device-loss observation shared with every immutable resource slot.
    pub(super) fn check(&self) -> Result<(), String> {
        if self.lost.get() { Err("Vulkan device is lost".into()) } else { Ok(()) }
    }
    /// Preserve the original native failure while making actual device loss terminal.
    pub(super) fn failure(&self, context: &str, error: vk::Result) -> String {
        if error == vk::Result::ERROR_DEVICE_LOST {
            self.lost.set(true);
        }
        format!("{context}: {error:?}")
    }
    /// Observe production allocation accounting without exposing mutable counters to tests.
    #[cfg(test)]
    pub(super) fn live_allocations(&self) -> u32 {
        self.memory_count.get()
    }

    /// Teardown cannot free children while completion is uncertain. Device loss permits native
    /// destruction; other wait failures retry rather than pretending that queue use has ended.
    pub(in crate::backends::vulkan) fn wait_idle(&self) {
        loop {
            match unsafe { self.device.device_wait_idle() } {
                Ok(()) => return,
                Err(vk::Result::ERROR_DEVICE_LOST) => {
                    self.lost.set(true);
                    return;
                }
                Err(_) => std::thread::yield_now(),
            }
        }
    }
    /// Takes native ownership after bootstrap has transferred every handle successfully.
    pub(in crate::backends::vulkan) fn new(entry: Entry, instance: ash::Instance, physical: vk::PhysicalDevice, device: ash::Device, family: u32) -> Rc<Self> {
        // SAFETY: Bootstrap owns a live physical/logical device and requested queue zero.
        let memory = unsafe { instance.get_physical_device_memory_properties(physical) };
        let limits = unsafe { instance.get_physical_device_properties(physical) }.limits;
        let queue = unsafe { device.get_device_queue(family, 0) };
        Rc::new(Self {
            identity: neocogi_render_core::source::new_driver_identity(),
            lost: Cell::new(false),
            _entry: entry,
            instance,
            device,
            physical,
            limits,
            memory_count: Cell::new(0),
            sampler_count: Cell::new(0),
            memory,
            queue,
            family,
        })
    }

    /// Finds a compatible heap with the requested properties; no caller-provided type index is used.
    fn memory_type(&self, bits: u32, properties: vk::MemoryPropertyFlags) -> Result<u32, String> {
        (0..self.memory.memory_type_count)
            .find(|index| bits & (1 << index) != 0 && self.memory.memory_types[*index as usize].property_flags.contains(properties))
            .ok_or_else(|| format!("no compatible Vulkan memory type for flags 0x{:x}", properties.as_raw()))
    }

    /// Reserve a native object slot before calling Vulkan; failure publishes neither owner nor count.
    pub(in crate::backends::vulkan) fn allocate(self: &Rc<Self>, info: &vk::MemoryAllocateInfo<'_>) -> Result<Memory, String> {
        let count = self.memory_count.get();
        if count >= self.limits.max_memory_allocation_count {
            return Err("Vulkan device memory allocation count exhausted".into());
        }
        // SAFETY: All callers supply a queried memory type and actual bound-object requirements.
        let raw = unsafe { self.device.allocate_memory(info, None) }.map_err(|error| self.failure("allocate Vulkan memory", error))?;
        self.memory_count.set(count + 1);
        Ok(Memory { core: self.clone(), raw })
    }

    /// Native sampler counts are shared across generic and host-owned graphics resources.
    pub(in crate::backends::vulkan) fn sampler(self: &Rc<Self>, info: &vk::SamplerCreateInfo<'_>) -> Result<NativeSampler, String> {
        let count = self.sampler_count.get();
        if count >= self.limits.max_sampler_allocation_count {
            return Err("Vulkan sampler allocation count exhausted".into());
        }
        // SAFETY: Callers use normalized descriptors without unenabled optional features.
        let raw = unsafe { self.device.create_sampler(info, None) }.map_err(|error| self.failure("create Vulkan sampler", error))?;
        self.sampler_count.set(count + 1);
        Ok(NativeSampler { core: self.clone(), raw })
    }

    /// Validate the exact format/usage before an image creation call can violate device limits.
    pub(in crate::backends::vulkan) fn image_extent(&self, format: vk::Format, width: u32, height: u32, usage: vk::ImageUsageFlags) -> Result<(), String> {
        if width == 0 || height == 0 || width > self.limits.max_image_dimension2_d || height > self.limits.max_image_dimension2_d {
            return Err("Vulkan image dimensions exceed device limits".into());
        }
        if usage.intersects(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
            && (width > self.limits.max_framebuffer_width
                || height > self.limits.max_framebuffer_height
                || width > self.limits.max_viewport_dimensions[0]
                || height > self.limits.max_viewport_dimensions[1]
                || width as f32 > self.limits.viewport_bounds_range[1]
                || height as f32 > self.limits.viewport_bounds_range[1])
        {
            return Err("Vulkan render target exceeds framebuffer or viewport limits".into());
        }
        // SAFETY: This is a read-only capability query for the physical device owning this core.
        let properties = unsafe {
            self.instance.get_physical_device_image_format_properties(
                self.physical,
                format,
                vk::ImageType::TYPE_2D,
                vk::ImageTiling::OPTIMAL,
                usage,
                vk::ImageCreateFlags::empty(),
            )
        }
        .map_err(|error| self.failure("unsupported Vulkan image format/usage", error))?;
        if width > properties.max_extent.width || height > properties.max_extent.height || !properties.sample_counts.contains(vk::SampleCountFlags::TYPE_1) {
            return Err("Vulkan image dimensions or samples exceed format capabilities".into());
        }
        Ok(())
    }
}

/// A device allocation owns its exact count until actual native memory release.
pub(in crate::backends::vulkan) struct Memory {
    /// No presentation/cache/domain edge; an escaped native allocation keeps only its device alive.
    core: Rc<Core>,
    /// Bound memory handle, released after the associated buffer/image is destroyed.
    pub(in crate::backends::vulkan) raw: vk::DeviceMemory,
}
impl Drop for Memory {
    /// Completion owners establish quiescence before the bound resource is destroyed.
    fn drop(&mut self) {
        unsafe { self.core.device.free_memory(self.raw, None) };
        self.core.memory_count.set(self.core.memory_count.get() - 1);
    }
}

/// Shared native sampler owner enforces the physical count independently of client quotas.
pub(in crate::backends::vulkan) struct NativeSampler {
    /// Cache-free device remains live for destruction.
    core: Rc<Core>,
    /// Normalized sampler used by immutable descriptors.
    pub(in crate::backends::vulkan) raw: vk::Sampler,
}
impl Drop for NativeSampler {
    /// Cache and submission owners release only after the final native sampling use completes.
    fn drop(&mut self) {
        unsafe { self.core.device.destroy_sampler(self.raw, None) };
        self.core.sampler_count.set(self.core.sampler_count.get() - 1);
    }
}
impl Drop for Core {
    /// Remaining allocation owners keep this core alive; presentation has already released its objects.
    fn drop(&mut self) {
        // SAFETY: This is the unique final native owner, with no cache/domain reverse edge.
        self.wait_idle();
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// Fully bound native buffer; staging and logical vertex/index/uniform owners share this mechanism.
struct Allocation {
    /// Native allocation destruction remains valid even after the renderer facade disappears.
    core: Rc<Core>,
    /// Exactly one initialized native buffer.
    raw: vk::Buffer,
    /// Exactly one bound memory allocation.
    memory: Memory,
    /// Native padding does not enlarge admitted logical draw ranges.
    size: u64,
}
impl Allocation {
    /// Creates initialized host-visible storage with explicit visibility before any queue submission.
    fn new(core: &Rc<Core>, len: usize, bytes: Option<&[u8]>, usage: vk::BufferUsageFlags) -> Result<Self, String> {
        let size = len.max(1).div_ceil(16) as u64 * 16;
        if bytes.is_some_and(|bytes| bytes.len() != len) {
            return Err("native buffer extent mismatch".into());
        }
        let device = &core.device;
        // SAFETY: All pointers reference call-local initialized descriptors or owned input bytes.
        unsafe {
            let raw = ResourceGuard::new(
                device
                    .create_buffer(
                        &vk::BufferCreateInfo::default().size(size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE),
                        None,
                    )
                    .map_err(|error| core.failure("create graphics buffer", error))?,
                |value| device.destroy_buffer(value, None),
            );
            let requirements = device.get_buffer_memory_requirements(*raw.get());
            let host = vk::MemoryPropertyFlags::HOST_VISIBLE;
            let coherent = vk::MemoryPropertyFlags::HOST_COHERENT;
            let kind = core
                .memory_type(requirements.memory_type_bits, host | coherent)
                .or_else(|_| core.memory_type(requirements.memory_type_bits, host))?;
            let memory = core.allocate(&vk::MemoryAllocateInfo::default().allocation_size(requirements.size).memory_type_index(kind))?;
            device
                .bind_buffer_memory(*raw.get(), memory.raw, 0)
                .map_err(|error| core.failure("bind graphics buffer", error))?;
            let value = Self {
                core: core.clone(),
                raw: raw.into_inner(),
                memory,
                size,
            };
            let mapped = device
                .map_memory(value.memory.raw, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                .map_err(|error| core.failure("map graphics buffer", error))?;
            std::ptr::write_bytes(mapped.cast::<u8>(), 0, size as usize);
            if let Some(bytes) = bytes {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len());
            }
            let visibility = if core.memory.memory_types[kind as usize].property_flags.contains(coherent) {
                Ok(())
            } else {
                device.flush_mapped_memory_ranges(&[vk::MappedMemoryRange::default().memory(value.memory.raw).offset(0).size(vk::WHOLE_SIZE)])
            };
            device.unmap_memory(value.memory.raw);
            visibility.map_err(|error| core.failure("flush graphics buffer", error))?;
            Ok(value)
        }
    }
}
impl Drop for Allocation {
    /// Completion owners ensure no queue can still access the allocation when it is released.
    fn drop(&mut self) {
        unsafe {
            self.core.device.destroy_buffer(self.raw, None);
        }
    }
}

/// Sender-free storage packet keeps allocation destruction before its final opaque receipt.
pub struct BufferAllocation {
    /// Mapped initialized native storage is destroyed before refunding its reservation.
    allocation: Allocation,
    /// Shared original reservation, with no source-version or domain reference.
    _charge: BackingCharge,
}
notifying!(Buffer, BufferAllocation);
/// Cache-free native image can be shared by ordinary UI images and generic sampled bindings.
pub(in crate::backends::vulkan) struct Image {
    /// Bound storage drops after this owner's view and image destruction.
    _memory: Memory,
    /// Parent dispatch has no renderer/cache/domain reverse edge.
    core: Rc<Core>,
    /// Single-mip immutable image storage.
    pub(in crate::backends::vulkan) raw: vk::Image,
    /// Read-only sampling or host-owned attachment view.
    pub(in crate::backends::vulkan) view: vk::ImageView,
}
impl Image {
    /// Transfers fully initialized same-device handles from an armed creation transaction.
    /// Allocate one native image/view with device-limit checks and counted memory ownership.
    /// Both initialized uploads and host color/depth attachments use this same transaction.
    pub(in crate::backends::vulkan) fn allocate(
        core: &Rc<Core>,
        size: Size,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
        aspect: vk::ImageAspectFlags,
    ) -> Result<Rc<Self>, String> {
        let core = core.clone();
        let device = &core.device;
        let extent = vk::Extent3D {
            width: size.width as u32,
            height: size.height as u32,
            depth: 1,
        };
        core.image_extent(format, extent.width, extent.height, usage)?;
        let image = ResourceGuard::new(
            unsafe {
                device.create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(format)
                        .extent(extent)
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(usage)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE)
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
            }
            .map_err(|error| core.failure("create graphics image", error))?,
            |value| unsafe { device.destroy_image(value, None) },
        );
        let requirements = unsafe { device.get_image_memory_requirements(*image.get()) };
        let kind = core
            .memory_type(requirements.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
            .or_else(|_| core.memory_type(requirements.memory_type_bits, vk::MemoryPropertyFlags::empty()))?;
        let memory = core.allocate(&vk::MemoryAllocateInfo::default().allocation_size(requirements.size).memory_type_index(kind))?;
        unsafe { device.bind_image_memory(*image.get(), memory.raw, 0) }.map_err(|error| core.failure("bind graphics image", error))?;
        let range = vk::ImageSubresourceRange::default().aspect_mask(aspect).level_count(1).layer_count(1);
        let view = ResourceGuard::new(
            unsafe {
                device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(*image.get())
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(range),
                    None,
                )
            }
            .map_err(|error| core.failure("create graphics image view", error))?,
            |view| unsafe { device.destroy_image_view(view, None) },
        );
        Ok(Rc::new(Self::new(core.clone(), image.into_inner(), memory, view.into_inner())))
    }

    pub(in crate::backends::vulkan) fn new(core: Rc<Core>, raw: vk::Image, memory: Memory, view: vk::ImageView) -> Self {
        Self { _memory: memory, core, raw, view }
    }
}
impl Drop for Image {
    /// Last native use is protected by upload/draw completion owners before this final release.
    fn drop(&mut self) {
        unsafe {
            self.core.device.destroy_image_view(self.view, None);
            self.core.device.destroy_image(self.raw, None);
        }
    }
}

/// Sender-free image packet shares raw storage without retaining a source version or public image.
pub struct TextureAllocation {
    /// Shared raw image owner contains neither a sender nor a logical source.
    pub(in crate::backends::vulkan) native: Rc<Image>,
    /// Immutable admitted extent of this exact native allocation.
    size: Size,
    /// Generic allocation charge; per-import use reservations stay outside this image.
    _charge: Option<BackingCharge>,
}
notifying!(Texture, TextureAllocation);
/// Native sampling packet owns physical-count accounting and an opaque logical reservation.
pub struct SamplerAllocation {
    /// The native owner also accounts for the physical device sampler limit.
    native: NativeSampler,
    /// Shared original reservation, with no source-version or domain reference.
    _charge: BackingCharge,
}
notifying!(Sampler, SamplerAllocation);

/// Target compatibility is independent of swapchain geometry or descriptor-pool generations.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct Signature {
    /// Host-selected single color attachment format.
    pub(in crate::backends::vulkan) color: vk::Format,
    /// Optional host depth attachment; clients cannot add one.
    pub(in crate::backends::vulkan) depth: Option<vk::Format>,
}
/// Pipeline layouts/descriptors are owned independently of the presentation render pass.
pub struct PipelineAllocation {
    /// Opaque definition/shader reservations contain no authority or version back-reference.
    _charges: [BackingCharge; 3],
    /// Native core, without renderer/cache/domain authority.
    core: Rc<Core>,
    /// Exact target-compatible graphics executable.
    raw: vk::Pipeline,
    /// Four stage-separated descriptor namespaces.
    groups: [vk::DescriptorSetLayout; 4],
    /// Graphics pipeline layout has no push constants or native client escapes.
    layout: vk::PipelineLayout,
}
impl Drop for PipelineAllocation {
    /// All command buffers referencing this pipeline have completed before final release.
    fn drop(&mut self) {
        unsafe {
            self.core.device.destroy_pipeline(self.raw, None);
            self.core.device.destroy_pipeline_layout(self.layout, None);
            for group in self.groups {
                self.core.device.destroy_descriptor_set_layout(group, None);
            }
        }
    }
}
notifying!(Pipeline, PipelineAllocation);
/// One block's descriptor sets have the same completion lifetime as its prepared commands.
struct Pool {
    /// Keeps descriptor destruction valid after presentation teardown.
    core: Rc<Core>,
    /// Dedicated pool allows one deterministic final destruction without individual free races.
    raw: vk::DescriptorPool,
}
impl Drop for Pool {
    fn drop(&mut self) {
        unsafe {
            self.core.device.destroy_descriptor_pool(self.raw, None);
        }
    }
}

/// Native blocks retain no domain/COM authority and can survive CPU/public owner expiry.
#[derive(Clone)]
pub struct PreparedBlock(Rc<PreparedData>);
/// Exact native snapshots and original logical capacity receipts share a completion lifetime.
struct PreparedData {
    /// Native core also identifies the device for checked host-side replay.
    core: Rc<Core>,
    /// Reject incompatible attachment changes before emitting native target commands.
    signature: Signature,
    /// Ordered native primitive and regional clear operations.
    commands: Vec<PreparedCommand>,
    /// Descriptor storage is reclaimed only after the same submission completes.
    _pool: Option<Pool>,
    /// Device-free logical recording capacity remains charged through actual native use.
    _capacity: Vec<CommandCharge>,
}
/// Fully realized native operations require no client callbacks or allocations during replay.
enum PreparedCommand {
    /// Complete direct draw state.
    Draw(PreparedDraw),
    /// Exact unblended regional color replacement.
    Color(u32, Rect),
    /// Exact regional normalized depth replacement.
    Depth(f32, Rect),
}
/// Native descriptor namespace/slot, exact image/sampler owners and this import's independent charge.
type SampledBinding = (u32, u32, Rc<Texture>, Rc<Sampler>, BackingCharge);
/// Consumed native binding owners are explicit; descriptor handles alone do not retain accounting.
struct PreparedDraw {
    /// Native pipeline and immutable shader/layout dependencies.
    pipeline: Rc<Pipeline>,
    /// Stream slot, native buffer and checked byte base.
    vertices: Vec<(u32, Rc<Buffer>, u64)>,
    /// Optional index role/width/base snapshot.
    index: Option<(Rc<Buffer>, vk::IndexType, u64)>,
    /// Four native sets allocated from the containing block pool.
    groups: Vec<vk::DescriptorSet>,
    /// Actual image/sampler accounting stays alive behind sampled descriptors.
    _textures: Vec<SampledBinding>,
    /// Copied uniform payload accounting stays alive behind buffer descriptors.
    _uniforms: Vec<(u32, u32, Rc<Buffer>)>,
    /// Checked primitive/instance ranges.
    range: DrawRange,
    /// Host-bounded mapping and independent raster clip.
    viewport: Rect,
    /// Effective root-bounded raster writes.
    clip: Rect,
}
/// A host-owned fence may be reused only after its native submission has completed.
struct Submitted {
    /// Context-owned fence outlives the executor's completion collection.
    fence: vk::Fence,
    /// Exact device-free native snapshots referenced by that submission.
    _blocks: Vec<PreparedBlock>,
}

/// Upload failure after submission prevents further native frame work on this executor.
#[derive(Clone, Copy)]
enum Health {
    /// Resource preparation and frame recording may proceed.
    Active,
    /// Submission/completion failed; pending owners remain until proven idle or device loss.
    Failed,
}
/// One deterministic observation failure at the real upload submission boundary.
/// Native work is valid and submitted normally; only its reported status is replaced.
#[cfg(test)]
#[derive(Clone, Copy)]
enum UploadFailure {
    /// Submission may have reached the queue even though its caller receives failure.
    Submit,
    /// Submission succeeded but completion could not be established by the caller.
    Wait,
}
/// A failed upload wait retains every referenced object until final native synchronization.
struct Transfer {
    /// Native destruction owner has no renderer/cache/client back-reference.
    core: Rc<Core>,
    /// The executor destroys this pool only after retiring every transfer.
    pool: vk::CommandPool,
    /// Recorded upload command cannot be freed while submission completion is uncertain.
    command: vk::CommandBuffer,
    /// Fence remains owned even if the first wait returns an error.
    fence: vk::Fence,
    /// Host-visible upload source must outlive its queued copy.
    _staging: Allocation,
    /// Destination survives any failed preparation without a COM/device-facade back-reference.
    _image: Rc<Image>,
    /// Generic allocations retain accounting; ordinary native images have no client-domain charge.
    _receipt: Option<BackingCharge>,
    /// Original block capacity follows a failed preparatory upload through uncertain completion.
    _capacity: Vec<CommandCharge>,
}
impl Drop for Transfer {
    /// Successful wait or final device-idle/loss is required before this aggregate is released.
    fn drop(&mut self) {
        unsafe {
            self.core.device.destroy_fence(self.fence, None);
            self.core.device.free_command_buffers(self.pool, &[self.command]);
        }
    }
}

/// One direct Vulkan executor realizes exactly the same CPU command vocabulary as other backends.
pub(in crate::backends::vulkan) struct Executor {
    /// Cache-free raw instance/device lifetime.
    core: Rc<Core>,
    /// Dedicated upload command pool never selects or acquires presentation targets.
    uploads: vk::CommandPool,
    /// Receiver remains on the existing executor; only actual native backings clone the producer.
    retired_tx: Sender<Retirement>,
    /// Drained after completion, failures, maintenance and shutdown, never owned by Core.
    retired_rx: Receiver<Retirement>,
    /// Native completion retains private backing/receipts without retaining client domain authority.
    submitted: Vec<Submitted>,
    /// Usually empty because initialization waits before returning; errors retain uncertain use.
    transfers: Vec<Transfer>,
    /// A native submit/wait failure is not a panel-local recoverable recording error.
    health: Health,
    /// Test-only status substitution, consumed once after installing all completion owners.
    #[cfg(test)]
    upload_failure: Option<UploadFailure>,
}

impl Executor {
    /// Creates only a private transient command pool; resource realization is lazy and fallible.
    pub(in crate::backends::vulkan) fn new(core: Rc<Core>) -> Result<Self, String> {
        let uploads = unsafe {
            core.device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(core.family)
                    .flags(vk::CommandPoolCreateFlags::TRANSIENT),
                None,
            )
        }
        .map_err(|error| core.failure("create graphics upload pool", error))?;
        let (retired_tx, retired_rx) = mpsc::channel();
        Ok(Self {
            core,
            uploads,
            retired_tx,
            retired_rx,
            submitted: Vec::new(),
            transfers: Vec::new(),
            health: Health::Active,
            #[cfg(test)]
            upload_failure: None,
        })
    }

    /// Polls context-owned fences, releases completed native owners and drains owned retirement packets.
    pub(in crate::backends::vulkan) fn collect(&mut self) -> Result<(), String> {
        let mut failure = None;
        self.submitted.retain(|entry| match unsafe { self.core.device.get_fence_status(entry.fence) } {
            Ok(true) => false,
            Ok(false) => true,
            Err(error) => {
                failure.get_or_insert(error);
                true
            }
        });
        self.drain();
        match failure {
            Some(error) => Err(self.core.failure("observe graphics completion", error)),
            None => self.core.check(),
        }
    }

    /// Realizes immutable native snapshots against a compatible host-selected render pass.
    pub(in crate::backends::vulkan) fn prepare(&mut self, block: &Block, pass: vk::RenderPass, signature: Signature) -> Result<PreparedBlock, String> {
        block.admit_on(&self.core).map_err(|error| error.to_string())?;
        let first = self.transfers.len();
        let result = self.prepare_inner(block, pass, signature);
        if result.is_err() {
            for transfer in &mut self.transfers[first..] {
                transfer._capacity = block.retain_capacity();
            }
        }
        result
    }
    /// Converts authenticated CPU captures, publishing no partial prepared block on failure.
    fn prepare_inner(&mut self, block: &Block, pass: vk::RenderPass, signature: Signature) -> Result<PreparedBlock, String> {
        block.admit_on(&self.core).map_err(|error| error.to_string())?;
        if self.unusable() {
            return Err("Vulkan graphics executor is unavailable after a native submission failure".into());
        }
        self.collect()?;
        self.submitted.try_reserve(1).map_err(|error| error.to_string())?;
        let pool = self.pool(block)?;
        let mut commands = Vec::new();
        commands.try_reserve_exact(block.commands().len()).map_err(|error| error.to_string())?;
        for command in block.commands() {
            commands.push(match command {
                Command::Draw(draw) => PreparedCommand::Draw(self.draw(draw, pass, signature, pool.as_ref().ok_or("missing graphics descriptor pool")?)?),
                Command::ClearColor { rgba, clip, .. } => PreparedCommand::Color(*rgba, *clip),
                Command::ClearDepth { depth, clip, .. } => {
                    if signature.depth.is_none() {
                        return Err("depth commands require a host depth attachment".into());
                    }
                    PreparedCommand::Depth(*depth, *clip)
                }
            });
        }
        Ok(PreparedBlock(Rc::new(PreparedData {
            core: self.core.clone(),
            signature,
            commands,
            _pool: pool,
            _capacity: block.retain_capacity(),
        })))
    }

    /// Descriptor storage is sized from exact consumed bindings, never caller-claimed counts.
    fn pool(&self, block: &Block) -> Result<Option<Pool>, String> {
        let mut draws = 0u32;
        let mut uniforms = 0u32;
        let mut images = 0u32;
        for command in block.commands() {
            if let Command::Draw(draw) = command {
                draws = draws.checked_add(1).ok_or("descriptor draw count overflow")?;
                uniforms = uniforms.checked_add(draw.uniforms.len() as u32).ok_or("uniform descriptor count overflow")?;
                images = images.checked_add(draw.textures.len() as u32).ok_or("image descriptor count overflow")?;
            }
        }
        if draws == 0 {
            return Ok(None);
        }
        let mut sizes = Vec::new();
        for (kind, count) in [
            (vk::DescriptorType::UNIFORM_BUFFER, uniforms),
            (vk::DescriptorType::SAMPLED_IMAGE, images),
            (vk::DescriptorType::SAMPLER, images),
        ] {
            if count > 0 {
                sizes.push(vk::DescriptorPoolSize { ty: kind, descriptor_count: count });
            }
        }
        let raw = unsafe {
            self.core.device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(draws.checked_mul(4).ok_or("descriptor set count overflow")?)
                    .pool_sizes(&sizes),
                None,
            )
        }
        .map_err(|error| self.core.failure("create graphics descriptor pool", error))?;
        Ok(Some(Pool { core: self.core.clone(), raw }))
    }

    /// Captures concrete native owners before constructing descriptor writes into the private pool.
    fn draw(&mut self, draw: &Draw, pass: vk::RenderPass, signature: Signature, pool: &Pool) -> Result<PreparedDraw, String> {
        let mut preparation = Preparation { executor: self, pass };
        let pipeline = draw.pipeline.native(signature, &mut preparation)?;
        let mut vertices = Vec::new();
        for binding in &draw.vertex_buffers {
            vertices.push((binding.slot, binding.buffer.native(&mut preparation)?, binding.offset));
        }
        let index = draw
            .index
            .as_ref()
            .map(|binding| {
                binding.buffer.native(&mut preparation).map(|buffer| {
                    (
                        buffer,
                        if binding.format == GpuIndexFormat::U16 {
                            vk::IndexType::UINT16
                        } else {
                            vk::IndexType::UINT32
                        },
                        binding.offset,
                    )
                })
            })
            .transpose()?;
        let mut textures = Vec::new();
        for binding in &draw.textures {
            textures.push((
                texture_group(binding.stage),
                binding.slot,
                binding.texture.native(&mut preparation)?,
                binding.sampler.native(&mut preparation)?,
                binding.texture.receipt(),
            ));
        }
        let mut uniforms = Vec::new();
        for binding in &draw.uniforms {
            uniforms.push((uniform_group(binding.stage), binding.slot, binding.data.native(&mut preparation)?));
        }
        let groups = unsafe {
            self.core
                .device
                .allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool.raw).set_layouts(&pipeline.groups))
        }
        .map_err(|error| self.core.failure("allocate graphics descriptor sets", error))?;
        // Descriptor arrays remain live for the synchronous update call. The prepared draw keeps
        // every referenced resource owner; Vulkan descriptor sets do not retain logical receipts.
        for (group, slot, texture, sampler, _) in &textures {
            let image = [vk::DescriptorImageInfo::default()
                .image_view(texture.native.view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let sampled = [vk::DescriptorImageInfo::default().sampler(sampler.native.raw)];
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(groups[*group as usize])
                    .dst_binding(slot * 2)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&image),
                vk::WriteDescriptorSet::default()
                    .dst_set(groups[*group as usize])
                    .dst_binding(slot * 2 + 1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&sampled),
            ];
            unsafe {
                self.core.device.update_descriptor_sets(&writes, &[]);
            }
        }
        for (group, slot, uniform) in &uniforms {
            let buffer = [vk::DescriptorBufferInfo::default()
                .buffer(uniform.allocation.raw)
                .offset(0)
                .range(uniform.allocation.size)];
            unsafe {
                self.core.device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(groups[*group as usize])
                        .dst_binding(*slot)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .buffer_info(&buffer)],
                    &[],
                );
            }
        }
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

    /// Host-visible initialization has no queued transfer, while its opaque receipt follows storage.
    fn buffer(&mut self, source: &BufferVersion) -> Result<Rc<Buffer>, String> {
        let usage = if source.desc().role == GpuBufferRole::Index {
            vk::BufferUsageFlags::INDEX_BUFFER
        } else {
            vk::BufferUsageFlags::VERTEX_BUFFER
        };
        let allocation = Allocation::new(&self.core, source.bytes().len(), source.bytes().as_slice(), usage)?;
        Ok(Rc::new(Buffer {
            owner: Some(BufferAllocation { allocation, _charge: source.receipt() }),
            retired: self.retired_tx.clone(),
        }))
    }
    /// Copied uniforms use the same native storage ownership as vertex/index bytes.
    fn uniform(&mut self, source: &UniformData) -> Result<Rc<Buffer>, String> {
        let allocation = Allocation::new(&self.core, source.bytes().len(), Some(source.bytes()), vk::BufferUsageFlags::UNIFORM_BUFFER)?;
        Ok(Rc::new(Buffer {
            owner: Some(BufferAllocation { allocation, _charge: source.receipt() }),
            retired: self.retired_tx.clone(),
        }))
    }
    /// One normalized native sampler owns its exact physical count until packet destruction.
    fn sampler(&mut self, source: &SamplerVersion) -> Result<Rc<Sampler>, String> {
        let desc = source.desc();
        let native = self.core.sampler(
            &vk::SamplerCreateInfo::default()
                .mag_filter(filter(desc.mag_filter))
                .min_filter(filter(desc.min_filter))
                .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                .address_mode_u(address(desc.address_u))
                .address_mode_v(address(desc.address_v))
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .min_lod(0.)
                .max_lod(0.),
        )?;
        Ok(Rc::new(Sampler {
            owner: Some(SamplerAllocation { native, _charge: source.receipt() }),
            retired: self.retired_tx.clone(),
        }))
    }
    /// Private UI uploads share the exact same immutable version-owned slot and completion path.
    pub(in crate::backends::vulkan) fn prepare_texture(&mut self, source: Rc<TextureVersion>) -> Result<(), String> {
        self.collect()?;
        source
            .native(&mut Preparation {
                executor: self,
                pass: vk::RenderPass::null(),
            })
            .map(|_| ())
    }
    /// Wrap actual same-core native pixels without retaining any public COM image or source version.
    pub(in crate::backends::vulkan) fn own_image(&self, native: Rc<Image>, size: Size, charge: Option<BackingCharge>) -> Rc<Texture> {
        Rc::new(Texture {
            owner: Some(TextureAllocation { native, size, _charge: charge }),
            retired: self.retired_tx.clone(),
        })
    }
    /// Ordinary and generic images share explicit initialized uploads with no renderer-global map.
    pub(in crate::backends::vulkan) fn create_image(&mut self, size: Size, format: vk::Format, pixels: Option<&[u8]>, charge: Option<BackingCharge>) -> Result<Rc<Texture>, String> {
        let native = self.upload_image(size, format, pixels, charge.clone())?;
        Ok(self.own_image(native, size, charge))
    }
    /// Only actual native backings clone a producer; source versions and Core own none.
    pub(in crate::backends::vulkan) fn sender(&self) -> Sender<Retirement> {
        self.retired_tx.clone()
    }
    /// Drains sender-free owned packets, including packets arriving while previous packets are dropped.
    fn drain(&mut self) {
        while let Ok(packet) = self.retired_rx.try_recv() {
            packet.discard();
        }
    }

    /// Shared ordinary/generic initialization retains commands, source and destination before
    /// submitting. A failed wait is terminal and keeps every referenced owner until idle/loss.
    pub(in crate::backends::vulkan) fn upload_image(&mut self, size: Size, format: vk::Format, pixels: Option<&[u8]>, receipt: Option<BackingCharge>) -> Result<Rc<Image>, String> {
        if self.unusable() {
            return Err("Vulkan graphics executor is unavailable".into());
        }
        self.collect()?;
        let core = self.core.clone();
        let device = &core.device;
        let native = Image::allocate(
            &core,
            size,
            format,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
            vk::ImageAspectFlags::COLOR,
        )?;
        let extent = vk::Extent3D {
            width: size.width as u32,
            height: size.height as u32,
            depth: 1,
        };
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let staging = Allocation::new(
            &core,
            rgba_bytes(size).map_err(|error| error.to_string())?,
            pixels,
            vk::BufferUsageFlags::TRANSFER_SRC,
        )?;
        let command = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.uploads)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
        }
        .map_err(|error| core.failure("allocate image upload command", error))?[0];
        let pool = self.uploads;
        let command = ResourceGuard::new(command, |value| unsafe { device.free_command_buffers(pool, &[value]) });
        let fence = ResourceGuard::new(
            unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }.map_err(|error| core.failure("create image upload fence", error))?,
            |value| unsafe { device.destroy_fence(value, None) },
        );
        unsafe {
            device
                .begin_command_buffer(
                    *command.get(),
                    &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|error| core.failure("begin image upload", error))?;
            let transition = |old, new, src, dst| {
                vk::ImageMemoryBarrier::default()
                    .old_layout(old)
                    .new_layout(new)
                    .src_access_mask(src)
                    .dst_access_mask(dst)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(native.raw)
                    .subresource_range(range)
            };
            device.cmd_pipeline_barrier(
                *command.get(),
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[transition(
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::TRANSFER_WRITE,
                )],
            );
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
                .image_extent(extent);
            device.cmd_copy_buffer_to_image(*command.get(), staging.raw, native.raw, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
            device.cmd_pipeline_barrier(
                *command.get(),
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::VERTEX_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[transition(
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::TRANSFER_WRITE,
                    vk::AccessFlags::SHADER_READ,
                )],
            );
            device
                .end_command_buffer(*command.get())
                .map_err(|error| core.failure("end image upload", error))?;
        }
        self.transfers.try_reserve(1).map_err(|error| error.to_string())?;
        let command = command.into_inner();
        let fence = fence.into_inner();
        self.transfers.push(Transfer {
            core: core.clone(),
            pool,
            command,
            fence,
            _staging: staging,
            _image: native.clone(),
            _receipt: receipt,
            _capacity: Vec::new(),
        });
        // The aggregate is installed before submission: neither an error nor a failed wait can
        // release queued source/destination objects while native completion remains uncertain.
        // SAFETY: Transfer owns the recorded command, fence, staging and destination
        // before the queue can observe them, including an uncertain failure result.
        let submission = unsafe { device.queue_submit(core.queue, &[vk::SubmitInfo::default().command_buffers(&[command])], fence) };
        #[cfg(test)]
        let failure = self.upload_failure.take();
        #[cfg(test)]
        let submission = submission.and(match failure {
            Some(UploadFailure::Submit) => Err(vk::Result::ERROR_UNKNOWN),
            _ => Ok(()),
        });
        let completion = submission.and_then(|()| {
            #[cfg(test)]
            if matches!(failure, Some(UploadFailure::Wait)) {
                return Err(vk::Result::TIMEOUT);
            }
            // SAFETY: The installed transfer retains its fence until synchronization.
            unsafe { device.wait_for_fences(&[fence], true, u64::MAX) }
        });
        if let Err(error) = completion {
            self.health = Health::Failed;
            return Err(self.core.failure("complete graphics image upload", error));
        }
        self.transfers.pop();
        Ok(native)
    }

    /// Host acquisition must also reject a failed executor, including frames without client blocks.
    pub(in crate::backends::vulkan) fn unusable(&self) -> bool {
        matches!(self.health, Health::Failed) || self.core.lost.get()
    }
}

impl Executor {
    /// Creates one exact layout/fixed-state executable for the compatible attachment signature.
    fn pipeline(&mut self, source: &PipelineVersion, pass: vk::RenderPass, signature: Signature) -> Result<Rc<Pipeline>, String> {
        if signature.depth.is_none() && (source.info().depth_write || source.info().depth_compare != GpuCompare::Always) {
            return Err("graphics pipeline requires a host depth attachment".into());
        }
        let core = self.core.clone();
        let device = &core.device;
        let mut groups = Vec::new();
        for group in 0..4 {
            let mut bindings = Vec::new();
            for binding in source.bindings() {
                let stage = if binding.stage == GpuShaderStage::Vertex {
                    vk::ShaderStageFlags::VERTEX
                } else {
                    vk::ShaderStageFlags::FRAGMENT
                };
                if binding.kind == GpuBindingKind::Uniform && uniform_group(binding.stage) == group {
                    bindings.push(
                        vk::DescriptorSetLayoutBinding::default()
                            .binding(binding.slot)
                            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                            .descriptor_count(1)
                            .stage_flags(stage),
                    );
                } else if binding.kind == GpuBindingKind::SampledTexture && texture_group(binding.stage) == group {
                    bindings.push(
                        vk::DescriptorSetLayoutBinding::default()
                            .binding(binding.slot * 2)
                            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                            .descriptor_count(1)
                            .stage_flags(stage),
                    );
                    bindings.push(
                        vk::DescriptorSetLayoutBinding::default()
                            .binding(binding.slot * 2 + 1)
                            .descriptor_type(vk::DescriptorType::SAMPLER)
                            .descriptor_count(1)
                            .stage_flags(stage),
                    );
                }
            }
            let raw = unsafe { device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None) }
                .map_err(|error| core.failure("create graphics binding layout", error))?;
            groups.push(ResourceGuard::new(raw, |value| unsafe { device.destroy_descriptor_set_layout(value, None) }));
        }
        let raw_groups: Vec<_> = groups.iter().map(|value| *value.get()).collect();
        let layout = ResourceGuard::new(
            unsafe { device.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&raw_groups), None) }
                .map_err(|error| core.failure("create graphics pipeline layout", error))?,
            |value| unsafe { device.destroy_pipeline_layout(value, None) },
        );
        let vertex = ResourceGuard::new(
            unsafe { device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&source.vertex().artifact().spirv), None) }
                .map_err(|error| core.failure("create vertex shader", error))?,
            |value| unsafe { device.destroy_shader_module(value, None) },
        );
        let fragment = ResourceGuard::new(
            unsafe { device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&source.fragment().artifact().spirv), None) }
                .map_err(|error| core.failure("create fragment shader", error))?,
            |value| unsafe { device.destroy_shader_module(value, None) },
        );
        let vertex_entry = CString::new(source.vertex().artifact().entry.as_bytes()).map_err(|_| "invalid vertex entry name")?;
        let fragment_entry = CString::new(source.fragment().artifact().entry.as_bytes()).map_err(|_| "invalid fragment entry name")?;
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(*vertex.get())
                .name(&vertex_entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(*fragment.get())
                .name(&fragment_entry),
        ];
        let streams: Vec<_> = source
            .vertex_buffers()
            .iter()
            .map(|value| vk::VertexInputBindingDescription {
                binding: value.slot,
                stride: value.stride,
                input_rate: if value.step == GpuStepMode::Instance {
                    vk::VertexInputRate::INSTANCE
                } else {
                    vk::VertexInputRate::VERTEX
                },
            })
            .collect();
        let attributes: Vec<_> = source
            .attributes()
            .iter()
            .map(|value| vk::VertexInputAttributeDescription {
                location: value.location,
                binding: value.slot,
                format: vertex_format(value.format),
                offset: value.offset,
            })
            .collect();
        let input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&streams)
            .vertex_attribute_descriptions(&attributes);
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);
        let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let state = source.info();
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.)
            .cull_mode(match state.cull {
                GpuCullMode::None => vk::CullModeFlags::NONE,
                GpuCullMode::Front => vk::CullModeFlags::FRONT,
                _ => vk::CullModeFlags::BACK,
            })
            .front_face(if state.front_face == GpuFrontFace::Ccw {
                vk::FrontFace::COUNTER_CLOCKWISE
            } else {
                vk::FrontFace::CLOCKWISE
            });
        let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(signature.depth.is_some())
            .depth_write_enable(state.depth_write)
            .depth_compare_op(compare(state.depth_compare));
        let blend = state.blend;
        let color = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(blend.enabled)
            .src_color_blend_factor(factor(blend.src_color))
            .dst_color_blend_factor(factor(blend.dst_color))
            .color_blend_op(blend_op(blend.color_op))
            .src_alpha_blend_factor(factor(blend.src_alpha))
            .dst_alpha_blend_factor(factor(blend.dst_alpha))
            .alpha_blend_op(blend_op(blend.alpha_op))
            .color_write_mask(vk::ColorComponentFlags::from_raw(u32::from(state.color_write)))];
        let color = vk::PipelineColorBlendStateCreateInfo::default().attachments(&color);
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&[vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR]);
        let descriptor = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&input)
            .input_assembly_state(&assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&color)
            .dynamic_state(&dynamic)
            .layout(*layout.get())
            .render_pass(pass)
            .subpass(0);
        let raw = match unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[descriptor], None) } {
            Ok(values) => values[0],
            Err((values, error)) => {
                for value in values {
                    unsafe {
                        device.destroy_pipeline(value, None);
                    }
                }
                return Err(core.failure("create graphics pipeline", error));
            }
        };
        let groups: Vec<_> = groups.into_iter().map(ResourceGuard::into_inner).collect();
        // Exactly four layouts were created above, without foreign callbacks or count inputs.
        let groups: [vk::DescriptorSetLayout; 4] = groups.try_into().map_err(|_| "internal graphics layout count")?;
        let owner = PipelineAllocation {
            _charges: source.receipts(),
            core: core.clone(),
            raw,
            groups,
            layout: layout.into_inner(),
        };
        let value = Rc::new(Pipeline {
            owner: Some(owner),
            retired: self.retired_tx.clone(),
        });
        Ok(value)
    }

    /// The actual acquired target must remain compatible, even after an asynchronous WSI rebuild.
    /// This local check runs before beginning the target render pass, including clear-only blocks.
    pub(in crate::backends::vulkan) fn admit(&self, block: &PreparedBlock, signature: Signature) -> Result<(), String> {
        if self.unusable() {
            return Err("Vulkan graphics executor is unavailable".into());
        }
        if !Rc::ptr_eq(&self.core, &block.0.core) || block.0.signature != signature {
            return Err("prepared graphics block belongs to another device or attachment signature".into());
        }
        Ok(())
    }

    /// Replays only native snapshots inside a host-owned pass; no client can select attachments.
    pub(in crate::backends::vulkan) fn record(&self, command: vk::CommandBuffer, block: &PreparedBlock, logical: Size, physical: Size, signature: Signature) -> Result<(), String> {
        self.admit(block, signature)?;
        let device = &self.core.device;
        for operation in &block.0.commands {
            let clip = match operation {
                PreparedCommand::Draw(draw) => draw.clip,
                PreparedCommand::Color(_, clip) | PreparedCommand::Depth(_, clip) => *clip,
            };
            let Some(clip) = physical_rect(clip, logical, physical).map_err(|error| error.to_string())? else {
                continue;
            };
            let region = vk::Rect2D {
                offset: vk::Offset2D { x: clip.x as i32, y: clip.y as i32 },
                extent: vk::Extent2D { width: clip.width, height: clip.height },
            };
            // SAFETY: The host owns the active pass and every referenced native allocation.
            // All byte ranges, clip bounds, formats and draw counts came from checked snapshots.
            unsafe {
                match operation {
                    PreparedCommand::Color(rgba, _) => {
                        let value = rgba.to_le_bytes().map(|byte| f32::from(byte) / 255.);
                        device.cmd_clear_attachments(
                            command,
                            &[vk::ClearAttachment {
                                aspect_mask: vk::ImageAspectFlags::COLOR,
                                color_attachment: 0,
                                clear_value: vk::ClearValue {
                                    color: vk::ClearColorValue { float32: value },
                                },
                            }],
                            &[vk::ClearRect {
                                rect: region,
                                base_array_layer: 0,
                                layer_count: 1,
                            }],
                        );
                    }
                    PreparedCommand::Depth(value, _) => device.cmd_clear_attachments(
                        command,
                        &[vk::ClearAttachment {
                            aspect_mask: vk::ImageAspectFlags::DEPTH,
                            color_attachment: 0,
                            clear_value: vk::ClearValue {
                                depth_stencil: vk::ClearDepthStencilValue { depth: *value, stencil: 0 },
                            },
                        }],
                        &[vk::ClearRect {
                            rect: region,
                            base_array_layer: 0,
                            layer_count: 1,
                        }],
                    ),
                    PreparedCommand::Draw(draw) => {
                        let Some(view) = physical_rect(draw.viewport, logical, physical).map_err(|error| error.to_string())? else {
                            continue;
                        };
                        device.cmd_bind_pipeline(command, vk::PipelineBindPoint::GRAPHICS, draw.pipeline.raw);
                        device.cmd_set_viewport(
                            command,
                            0,
                            &[vk::Viewport {
                                x: view.x as f32,
                                y: view.y as f32,
                                width: view.width as f32,
                                height: view.height as f32,
                                min_depth: 0.,
                                max_depth: 1.,
                            }],
                        );
                        device.cmd_set_scissor(command, 0, &[region]);
                        device.cmd_bind_descriptor_sets(command, vk::PipelineBindPoint::GRAPHICS, draw.pipeline.layout, 0, &draw.groups, &[]);
                        for (slot, buffer, offset) in &draw.vertices {
                            device.cmd_bind_vertex_buffers(command, *slot, &[buffer.allocation.raw], &[*offset]);
                        }
                        match draw.range {
                            DrawRange::Vertices { first, count, instances } => device.cmd_draw(command, count, instances, first, 0),
                            DrawRange::Indices { first, count, instances } => {
                                let (buffer, format, offset) = draw.index.as_ref().ok_or("missing prepared index binding")?;
                                device.cmd_bind_index_buffer(command, buffer.allocation.raw, *offset, *format);
                                device.cmd_draw_indexed(command, count, instances, first, 0, 0);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Keeps snapshots through the context-owned completion fence, including later presentation failure.
    pub(in crate::backends::vulkan) fn submitted(&mut self, fence: vk::Fence, blocks: Vec<PreparedBlock>) {
        if !blocks.is_empty() {
            self.submitted.push(Submitted { fence, _blocks: blocks });
        }
    }
}
impl Drop for Executor {
    /// All pending native uses end before descriptors, retirement packets and the upload pool are released.
    fn drop(&mut self) {
        self.core.wait_idle();
        self.submitted.clear();
        self.transfers.clear();
        self.drain();
        unsafe {
            self.core.device.destroy_command_pool(self.uploads, None);
        }
    }
}

/// Stage-separated texture groups match the canonical shader profile.
fn texture_group(stage: GpuShaderStage) -> u32 {
    if stage == GpuShaderStage::Vertex { 0 } else { 2 }
}
/// Uniform groups are separate from sampled bindings within each stage.
fn uniform_group(stage: GpuShaderStage) -> u32 {
    texture_group(stage) + 1
}
/// Explicit source bytes retain their integer/normalized interpretation.
fn vertex_format(value: GpuVertexFormat) -> vk::Format {
    match value {
        GpuVertexFormat::Float32 => vk::Format::R32_SFLOAT,
        GpuVertexFormat::Float32x2 => vk::Format::R32G32_SFLOAT,
        GpuVertexFormat::Float32x3 => vk::Format::R32G32B32_SFLOAT,
        GpuVertexFormat::Float32x4 => vk::Format::R32G32B32A32_SFLOAT,
        GpuVertexFormat::Uint32 => vk::Format::R32_UINT,
        GpuVertexFormat::Uint32x2 => vk::Format::R32G32_UINT,
        GpuVertexFormat::Uint32x3 => vk::Format::R32G32B32_UINT,
        GpuVertexFormat::Uint32x4 => vk::Format::R32G32B32A32_UINT,
        _ => vk::Format::R8G8B8A8_UNORM,
    }
}
/// Converts one validated nonmipmapped filter.
fn filter(value: GpuFilter) -> vk::Filter {
    if value == GpuFilter::Nearest {
        vk::Filter::NEAREST
    } else {
        vk::Filter::LINEAR
    }
}
/// Converts one validated texture coordinate policy.
fn address(value: GpuAddressMode) -> vk::SamplerAddressMode {
    match value {
        GpuAddressMode::Clamp => vk::SamplerAddressMode::CLAMP_TO_EDGE,
        GpuAddressMode::Repeat => vk::SamplerAddressMode::REPEAT,
        _ => vk::SamplerAddressMode::MIRRORED_REPEAT,
    }
}
/// Converts one complete depth predicate.
fn compare(value: GpuCompare) -> vk::CompareOp {
    match value {
        GpuCompare::Never => vk::CompareOp::NEVER,
        GpuCompare::Less => vk::CompareOp::LESS,
        GpuCompare::Equal => vk::CompareOp::EQUAL,
        GpuCompare::LessEqual => vk::CompareOp::LESS_OR_EQUAL,
        GpuCompare::Greater => vk::CompareOp::GREATER,
        GpuCompare::NotEqual => vk::CompareOp::NOT_EQUAL,
        GpuCompare::GreaterEqual => vk::CompareOp::GREATER_OR_EQUAL,
        _ => vk::CompareOp::ALWAYS,
    }
}
/// Converts independent source/destination scaling factors.
fn factor(value: GpuBlendFactor) -> vk::BlendFactor {
    match value {
        GpuBlendFactor::Zero => vk::BlendFactor::ZERO,
        GpuBlendFactor::One => vk::BlendFactor::ONE,
        GpuBlendFactor::SrcColor => vk::BlendFactor::SRC_COLOR,
        GpuBlendFactor::OneMinusSrcColor => vk::BlendFactor::ONE_MINUS_SRC_COLOR,
        GpuBlendFactor::DstColor => vk::BlendFactor::DST_COLOR,
        GpuBlendFactor::OneMinusDstColor => vk::BlendFactor::ONE_MINUS_DST_COLOR,
        GpuBlendFactor::SrcAlpha => vk::BlendFactor::SRC_ALPHA,
        GpuBlendFactor::OneMinusSrcAlpha => vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        GpuBlendFactor::DstAlpha => vk::BlendFactor::DST_ALPHA,
        _ => vk::BlendFactor::ONE_MINUS_DST_ALPHA,
    }
}
/// Min/max are unscaled operations; Vulkan ignores their factor fields.
fn blend_op(value: GpuBlendOp) -> vk::BlendOp {
    match value {
        GpuBlendOp::Add => vk::BlendOp::ADD,
        GpuBlendOp::Subtract => vk::BlendOp::SUBTRACT,
        GpuBlendOp::ReverseSubtract => vk::BlendOp::REVERSE_SUBTRACT,
        GpuBlendOp::Min => vk::BlendOp::MIN,
        _ => vk::BlendOp::MAX,
    }
}

#[cfg(test)]
#[path = "generic_tests.rs"]
pub(in crate::backends::vulkan) mod tests;
