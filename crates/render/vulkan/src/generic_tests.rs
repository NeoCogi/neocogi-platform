//! Headless direct-Vulkan acceptance uses real COM recording, native passes and readback.
use super::*;
use neocogi_com::{ComBox, ComPtr, NcCopyFromBufferOffset};
use neocogi_render_gpu::{
    GpuDevice,
    source::{Bytes, PipelineDesc},
};

/// Opens a graphics device without requiring a native window or presentation extension.
fn core() -> Rc<Core> {
    let entry = Entry::linked();
    let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
    let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None) }.unwrap();
    let (physical, family) = unsafe { instance.enumerate_physical_devices() }
        .unwrap()
        .into_iter()
        .find_map(|physical| {
            unsafe { instance.get_physical_device_queue_family_properties(physical) }
                .iter()
                .position(|queue| queue.queue_count > 0 && queue.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .map(|family| (physical, family as u32))
        })
        .expect("a graphics-capable Vulkan device is required");
    let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&[1.])];
    let device = unsafe { instance.create_device(physical, &vk::DeviceCreateInfo::default().queue_create_infos(&queues), None) }.unwrap();
    Core::new(entry, instance, physical, device, family)
}

/// Uncertain submit/wait observations keep the real transfer and accounting alive,
/// reject reuse, and leave an already produced immutable output untouched.
#[test]
#[ignore = "requires a native Vulkan graphics adapter"]
fn uncertain_upload_status_retains_owners_until_idle_and_rejects_reuse() {
    for failure in [UploadFailure::Submit, UploadFailure::Wait] {
        let core = core();
        let mut executor = Executor::new(core.clone()).unwrap();
        let mut target = Target::new(core.clone(), 8, 8);
        target.render_color(&mut executor, Vec::new(), Size { width: 8, height: 8 }, [0.25, 0.5, 0.75, 1.]);
        let previous = target.read(&target.pixels);
        let baseline = core.memory_count.get();
        let gpu = GpuDevice::new(
            nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
                .query()
                .unwrap(),
        );
        let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
        let resources = controller.resources().unwrap();
        let image = resources
            .create_texture(
                GpuTextureDesc {
                    size: Size { width: 1, height: 1 },
                    format: GpuTextureFormat::Rgba8Unorm,
                    mip_count: 1,
                },
                Bytes::source(vec![255; 4]).as_com_ref(),
            )
            .unwrap();
        let source = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
        executor.upload_failure = Some(failure);
        let error = match executor.upload_image(
            source.desc().size,
            vk::Format::R8G8B8A8_UNORM,
            source.pixels().unwrap(),
            Some(Rc::new(source.receipt())),
        ) {
            Ok(_) => panic!("injected observation failure must reject the upload"),
            Err(error) => error,
        };
        assert!(error.contains("complete graphics image upload"));
        assert!(executor.unusable());
        assert_eq!(executor.transfers.len(), 1);
        drop((image, source));
        assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 4);
        assert_eq!(core.memory_count.get(), baseline + 2);
        assert!(
            executor
                .upload_image(Size { width: 1, height: 1 }, vk::Format::R8G8B8A8_UNORM, &[0; 4], None)
                .is_err()
        );
        executor.collect().unwrap();
        assert_eq!(executor.transfers.len(), 1);
        assert_eq!(target.read(&target.pixels), previous);
        // The real queue is healthy: bounded native wait proves completion, then
        // normal executor teardown performs its production idle-before-release path.
        unsafe { core.device.wait_for_fences(&[executor.transfers[0].fence], true, 5_000_000_000) }.unwrap();
        drop(executor);
        assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
        assert_eq!(core.memory_count.get(), baseline);
    }
}

/// A target destroys framebuffer references before its independently owned attachment images.
struct Image {
    /// Cache-free native device ownership.
    core: Rc<Core>,
    /// One optimal native attachment.
    raw: vk::Image,
    /// Exactly one bound allocation.
    _memory: Memory,
    /// Single-mip attachment view.
    view: vk::ImageView,
}
impl Image {
    /// Creates initialized ownership; render-pass clear supplies all attachment contents.
    fn new(core: &Rc<Core>, format: vk::Format, usage: vk::ImageUsageFlags, aspect: vk::ImageAspectFlags, extent: vk::Extent2D) -> Self {
        unsafe {
            let raw = core
                .device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(format)
                        .extent(vk::Extent3D {
                            width: extent.width,
                            height: extent.height,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(usage)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )
                .unwrap();
            let needs = core.device.get_image_memory_requirements(raw);
            let kind = core.memory_type(needs.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL).unwrap();
            let memory = core
                .allocate(&vk::MemoryAllocateInfo::default().allocation_size(needs.size).memory_type_index(kind))
                .unwrap();
            core.device.bind_image_memory(raw, memory.raw, 0).unwrap();
            let range = vk::ImageSubresourceRange::default().aspect_mask(aspect).level_count(1).layer_count(1);
            let view = core
                .device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(raw)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(range),
                    None,
                )
                .unwrap();
            Self {
                core: core.clone(),
                raw,
                _memory: memory,
                view,
            }
        }
    }
}
impl Drop for Image {
    /// The target waits before destroying its framebuffer and then these attachment owners.
    fn drop(&mut self) {
        unsafe {
            self.core.device.destroy_image_view(self.view, None);
            self.core.device.destroy_image(self.raw, None);
        }
    }
}

/// Serial native target supports color and depth readback without any backend-native callback seam.
struct Target {
    /// Device lifetime is independent of the renderer facade and client domain.
    core: Rc<Core>,
    /// Physical framebuffer dimensions.
    extent: vk::Extent2D,
    /// Exact pipeline attachment compatibility.
    signature: Signature,
    /// Color becomes transfer-readable at pass completion.
    color: Image,
    /// Depth also becomes transfer-readable to test sentinels outside client roots.
    depth: Image,
    /// Initialized host-visible RGBA destination.
    pixels: Allocation,
    /// Initialized host-visible D32 destination.
    depths: Allocation,
    /// Color/depth attachment contract for production pipelines.
    pass: vk::RenderPass,
    /// References the owned attachment views.
    framebuffer: vk::Framebuffer,
    /// Resettable serial command storage.
    pool: vk::CommandPool,
    /// Reused only after observed completion.
    command: vk::CommandBuffer,
    /// Owned fence outlives executor completion collection in every test.
    fence: vk::Fence,
}
impl Target {
    /// Constructs a tiny native color/depth/readback session with explicit transfer dependencies.
    fn new(core: Rc<Core>, width: u32, height: u32) -> Self {
        let extent = vk::Extent2D { width, height };
        let signature = Signature {
            color: vk::Format::R8G8B8A8_UNORM,
            depth: Some(vk::Format::D32_SFLOAT),
        };
        let color = Image::new(
            &core,
            signature.color,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            vk::ImageAspectFlags::COLOR,
            extent,
        );
        let depth = Image::new(
            &core,
            signature.depth.unwrap(),
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            vk::ImageAspectFlags::DEPTH,
            extent,
        );
        let pixels = Allocation::new(&core, &vec![0; (width * height * 4) as usize], vk::BufferUsageFlags::TRANSFER_DST).unwrap();
        let depths = Allocation::new(&core, &vec![0; (width * height * 4) as usize], vk::BufferUsageFlags::TRANSFER_DST).unwrap();
        unsafe {
            let pass = crate::pass::create(
                &core.device,
                signature,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                Some(vk::ImageLayout::TRANSFER_SRC_OPTIMAL),
            )
            .unwrap();
            let views = [color.view, depth.view];
            let framebuffer = core
                .device
                .create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(pass)
                        .attachments(&views)
                        .width(width)
                        .height(height)
                        .layers(1),
                    None,
                )
                .unwrap();
            let pool = core
                .device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(core.family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .unwrap();
            let command = core
                .device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .unwrap()[0];
            let fence = core
                .device
                .create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None)
                .unwrap();
            Self {
                core,
                extent,
                signature,
                color,
                depth,
                pixels,
                depths,
                pass,
                framebuffer,
                pool,
                command,
                fence,
            }
        }
    }

    /// Replays production snapshots, submits them and retains their real fence owners.
    fn render(&mut self, executor: &mut Executor, blocks: Vec<PreparedBlock>, logical: Size) {
        self.render_color(executor, blocks, logical, [0.2, 0.2, 0.2, 1.]);
    }

    /// Host clear selection remains independent from every client pipeline and root.
    fn render_color(&mut self, executor: &mut Executor, blocks: Vec<PreparedBlock>, logical: Size, clear: [f32; 4]) {
        for block in &blocks {
            executor.admit(block, self.signature).unwrap();
        }
        unsafe {
            self.core.device.wait_for_fences(&[self.fence], true, u64::MAX).unwrap();
            executor.collect().unwrap();
            self.core.device.reset_command_pool(self.pool, vk::CommandPoolResetFlags::empty()).unwrap();
            self.core
                .device
                .begin_command_buffer(self.command, &vk::CommandBufferBeginInfo::default())
                .unwrap();
            let clears = [
                vk::ClearValue {
                    color: vk::ClearColorValue { float32: clear },
                },
                vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue { depth: 0., stencil: 0 },
                },
            ];
            let area = vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: self.extent,
            };
            self.core.device.cmd_begin_render_pass(
                self.command,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.pass)
                    .framebuffer(self.framebuffer)
                    .render_area(area)
                    .clear_values(&clears),
                vk::SubpassContents::INLINE,
            );
            let physical = Size {
                width: self.extent.width as i32,
                height: self.extent.height as i32,
            };
            for block in &blocks {
                executor.record(self.command, block, logical, physical, self.signature).unwrap();
            }
            self.core.device.cmd_end_render_pass(self.command);
            for (image, aspect, buffer) in [
                (&self.color, vk::ImageAspectFlags::COLOR, &self.pixels),
                (&self.depth, vk::ImageAspectFlags::DEPTH, &self.depths),
            ] {
                let copy = vk::BufferImageCopy::default()
                    .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(aspect).layer_count(1))
                    .image_extent(vk::Extent3D {
                        width: self.extent.width,
                        height: self.extent.height,
                        depth: 1,
                    });
                self.core
                    .device
                    .cmd_copy_image_to_buffer(self.command, image.raw, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, buffer.raw, &[copy]);
            }
            self.core.device.cmd_pipeline_barrier(
                self.command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::HOST_READ)],
                &[],
                &[],
            );
            self.core.device.end_command_buffer(self.command).unwrap();
            self.core.device.reset_fences(&[self.fence]).unwrap();
            self.core
                .device
                .queue_submit(self.core.queue, &[vk::SubmitInfo::default().command_buffers(&[self.command])], self.fence)
                .unwrap();
        }
        executor.submitted(self.fence, blocks);
    }

    /// Host reads happen only after completion and explicit noncoherent visibility handling.
    fn read(&self, buffer: &Allocation) -> Vec<u8> {
        unsafe {
            self.core.device.wait_for_fences(&[self.fence], true, u64::MAX).unwrap();
            let data = self
                .core
                .device
                .map_memory(buffer.memory.raw, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                .unwrap();
            self.core
                .device
                .invalidate_mapped_memory_ranges(&[vk::MappedMemoryRange::default().memory(buffer.memory.raw).offset(0).size(vk::WHOLE_SIZE)])
                .unwrap();
            let bytes = std::slice::from_raw_parts(data.cast::<u8>(), (self.extent.width * self.extent.height * 4) as usize).to_vec();
            self.core.device.unmap_memory(buffer.memory.raw);
            bytes
        }
    }
}
impl Drop for Target {
    /// Native target references are destroyed before attachment/readback owners and the core.
    fn drop(&mut self) {
        self.core.wait_idle();
        unsafe {
            self.core.device.destroy_fence(self.fence, None);
            self.core.device.destroy_command_pool(self.pool, None);
            self.core.device.destroy_framebuffer(self.framebuffer, None);
            self.core.device.destroy_render_pass(self.pass, None);
        }
    }
}

/// Client-owned shader semantics deliberately differ from the native backend's resource vocabulary.
const SHADER: &str = r#"
struct Values { color:vec4<f32>, params:vec4<f32> }
@group(1) @binding(0) var<uniform> vertex_values:Values;
@group(3) @binding(0) var<uniform> fragment_values:Values;
@vertex fn vs(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
    let p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));return vec4(p[i],vertex_values.params.x,1.);
}
@fragment fn fs() -> @location(0) vec4<f32> { return fragment_values.color; }
"#;

/// Builds a portable reflected pipeline through real generated COM calls.
fn pipeline(resources: &ComPtr<IRenderResourcesAbi>, mask: u8) -> ComBox<IGpuPipelineAbi> {
    pipeline_state(
        resources,
        GpuPipelineInfo {
            color_write: mask,
            depth_write: true,
            depth_compare: GpuCompare::LessEqual,
            ..Default::default()
        },
    )
}

/// Vary raster state without introducing a native-only test pipeline.
fn pipeline_state(resources: &ComPtr<IRenderResourcesAbi>, state: GpuPipelineInfo) -> ComBox<IGpuPipelineAbi> {
    let source = Bytes::source(SHADER.as_bytes().to_vec());
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let desc = PipelineDesc {
        state,
        bindings: [GpuShaderStage::Vertex, GpuShaderStage::Fragment]
            .map(|stage| GpuShaderBinding {
                stage,
                slot: 0,
                kind: GpuBindingKind::Uniform,
                byte_len: 32,
            })
            .to_vec(),
        ..Default::default()
    }
    .source()
    .unwrap();
    resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap()
}
/// Explicit initialized uniform bytes never borrow Rust structure storage.
fn values(view: &mut ComBox<IRenderViewAbi>, color: [f32; 4], depth: f32) {
    let bytes: Vec<_> = color.into_iter().chain([depth, 0., 0., 0.]).flat_map(f32::to_le_bytes).collect();
    for stage in [GpuShaderStage::Vertex, GpuShaderStage::Fragment] {
        view.push_uniforms(stage, 0, NcCopyFromBufferOffset::new(0, &bytes)).unwrap();
    }
}

/// Native color AND depth readback proves confinement, reset, ordered scopes and exact DPI mapping.
#[test]
#[ignore = "requires a native Vulkan graphics device, including a software device"]
fn regional_depth_and_copied_uniforms_preserve_surrounding_targets() {
    let core = core();
    let mut target = Target::new(core.clone(), 64, 64);
    let mut executor = Executor::new(core.clone()).unwrap();
    let gpu = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    let pipeline = pipeline(&resources, 15);
    let hidden = self::pipeline(&resources, 0);
    let mut frame = gpu.broker().begin_frame().unwrap();
    let first = Rect { x: 2, y: 2, width: 28, height: 28 };
    let mut session = frame
        .begin_view(RenderViewRoot { client: first, target: first }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    values(&mut view, [1., 0., 0., 1.], 0.25);
    view.draw(3, 1, 0).unwrap();
    view.push_clip(Rect { x: 14, y: 0, width: 14, height: 28 }).unwrap();
    view.clear_depth(1.).unwrap();
    values(&mut view, [0., 0., 1., 1.], 0.75);
    view.draw(3, 1, 0).unwrap();
    view.pop().unwrap();
    view.reset().unwrap();
    values(&mut view, [1., 1., 0., 1.], 0.9);
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let first = session.take_block().unwrap();
    let second = Rect { x: 10, y: 10, width: 12, height: 12 };
    let mut second_session = frame
        .begin_view(RenderViewRoot { client: second, target: second }, resources.as_com_ref())
        .unwrap();
    let mut second_view = second_session.view().unwrap();
    second_view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    values(&mut second_view, [0., 1., 0., 1.], 0.75);
    second_view.draw(3, 1, 0).unwrap();
    second_view.finish().unwrap();
    let second = second_session.take_block().unwrap();
    let third = Rect { x: 1, y: 1, width: 1, height: 1 };
    let mut third_session = frame
        .begin_view(RenderViewRoot { client: third, target: third }, resources.as_com_ref())
        .unwrap();
    let mut third_view = third_session.view().unwrap();
    third_view.bind_pipeline(hidden.as_com_ref()).unwrap();
    values(&mut third_view, [1., 0., 1., 1.], 0.5);
    third_view.draw(3, 1, 0).unwrap();
    third_view.clear_color(0xffffffff).unwrap();
    third_view.finish().unwrap();
    let third = third_session.take_block().unwrap();
    frame.finish().unwrap();
    let mut blocks = Vec::new();
    for owner in [&first, &second, &third] {
        blocks.push(
            executor
                .prepare(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), target.pass, target.signature, &HashMap::new())
                .unwrap(),
        );
    }
    drop((
        first,
        second,
        third,
        view,
        second_view,
        third_view,
        session,
        second_session,
        third_session,
        frame,
        pipeline,
        hidden,
    ));
    let charged = gpu.recording_usage(resources.as_com_ref()).unwrap().0;
    assert!(charged > 0);
    target.render(&mut executor, blocks, Size { width: 32, height: 32 });
    assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap(), (charged, 0));
    let pixels = target.read(&target.pixels);
    let depth = target.read(&target.depths);
    let pixel = |x: usize, y: usize| &pixels[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
    let z = |x: usize, y: usize| f32::from_le_bytes(depth[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4].try_into().unwrap());
    assert_eq!(pixel(8, 8), [255, 0, 0, 255]);
    assert_eq!(pixel(52, 8), [0, 0, 255, 255]);
    assert_eq!(pixel(24, 24), [0, 255, 0, 255]);
    assert_eq!(pixel(2, 2), [255; 4]);
    assert_eq!(pixel(0, 0), [51, 51, 51, 255]);
    assert_eq!(z(0, 0), 0.);
    assert_eq!(z(8, 8), 0.25);
    assert_eq!(z(52, 8), 0.75);
    assert_eq!(z(24, 24), 0.75);
    assert_eq!(z(2, 2), 0.5);
    executor.collect().unwrap();
    assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap(), (0, 0));
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
}

/// Untrusted dynamic indices cannot read outside uniform storage or write past private arrays.
#[test]
#[ignore = "requires a native Vulkan graphics device, including a software device"]
fn generated_bounds_guards_execute_on_the_native_driver() {
    let core = core();
    let mut target = Target::new(core.clone(), 8, 8);
    let mut executor = Executor::new(core).unwrap();
    let gpu = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    let source = Bytes::source(
        br#"
        @vertex fn vs(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
            let p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));return vec4(p[i],0.5,1.);
        }
        struct Values { items:array<vec4<f32>,2>, indices:vec4<u32> }
        @group(3) @binding(0) var<uniform> data:Values;
        @fragment fn fs() -> @location(0) vec4<f32> {
            var local=array<vec4<f32>,2>(vec4(0.25),vec4(0.75));
            local[data.indices.x]=vec4(1.0);
            return data.items[data.indices.y]+local[0];
        }
    "#
        .to_vec(),
    );
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let desc = PipelineDesc {
        bindings: vec![GpuShaderBinding {
            stage: GpuShaderStage::Fragment,
            slot: 0,
            kind: GpuBindingKind::Uniform,
            byte_len: 48,
        }],
        ..Default::default()
    }
    .source()
    .unwrap();
    let pipeline = resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let mut frame = gpu.broker().begin_frame().unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut session = frame.begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    let mut bytes = vec![0; 32];
    for index in [2u32, 999, 0, 0] {
        bytes.extend_from_slice(&index.to_le_bytes());
    }
    view.push_uniforms(GpuShaderStage::Fragment, 0, NcCopyFromBufferOffset::new(0, &bytes)).unwrap();
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    frame.finish().unwrap();
    let prepared = executor
        .prepare(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), target.pass, target.signature, &HashMap::new())
        .unwrap();
    target.render(&mut executor, vec![prepared], Size { width: 8, height: 8 });
    for pixel in target.read(&target.pixels).as_chunks::<4>().0 {
        assert_eq!(*pixel, [64; 4]);
    }
    executor.collect().unwrap();
}

/// Native vertex/index interpretation, texture orientation and per-draw versions are portable.
#[test]
#[ignore = "requires a native Vulkan graphics device, including a software device"]
fn indexed_texture_versions_and_unscaled_min_max_blending_reach_native_pixels() {
    for operation in [GpuBlendOp::Min, GpuBlendOp::Max] {
        for imported in [false, true] {
            indexed_texture_case(operation, imported);
        }
    }
}

/// Both operations ignore factors, while exact image/uniform snapshots retain painter order.
fn indexed_texture_case(operation: GpuBlendOp, imported: bool) {
    let core = core();
    let mut target = Target::new(core.clone(), 8, 8);
    let mut executor = Executor::new(core).unwrap();
    let gpu = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = if imported {
        gpu.create_ui_client(RenderClientLimits::default(), None)
    } else {
        gpu.create_client(RenderClientLimits::default())
    }
    .unwrap();
    let mut images = HashMap::new();
    let resources = controller.resources().unwrap();
    let source = Bytes::source(
        br#"
        struct Output { @builtin(position) pos:vec4<f32>, @location(0) uv:vec2<f32>, @location(1) color:vec4<f32> }
        @vertex fn vs(@location(0) p:vec2<f32>, @location(1) color:vec4<f32>) -> Output {
            var out:Output; out.pos=vec4(p,0.5,1.); out.uv=vec2(p.x*0.5+0.5,0.5-p.y*0.5); out.color=color; return out;
        }
        struct Values { tint:vec4<f32> }
        @group(2) @binding(0) var image:texture_2d<f32>;
        @group(2) @binding(1) var image_sampler:sampler;
        @group(3) @binding(0) var<uniform> values:Values;
        @fragment fn fs(input:Output) -> @location(0) vec4<f32> {
            return textureSample(image,image_sampler,input.uv)*input.color*values.tint;
        }
    "#
        .to_vec(),
    );
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let desc = PipelineDesc {
        state: GpuPipelineInfo {
            blend: GpuBlendState {
                enabled: true,
                color_op: operation,
                alpha_op: operation,
                src_color: GpuBlendFactor::Zero,
                dst_color: GpuBlendFactor::Zero,
                src_alpha: GpuBlendFactor::Zero,
                dst_alpha: GpuBlendFactor::Zero,
            },
            ..Default::default()
        },
        vertex_buffers: vec![GpuVertexBufferLayout {
            slot: 3,
            stride: 12,
            step: GpuStepMode::Vertex,
        }],
        attributes: vec![
            GpuVertexAttribute {
                location: 0,
                slot: 3,
                offset: 0,
                format: GpuVertexFormat::Float32x2,
            },
            GpuVertexAttribute {
                location: 1,
                slot: 3,
                offset: 8,
                format: GpuVertexFormat::Unorm8x4,
            },
        ],
        bindings: vec![
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 0,
                kind: GpuBindingKind::SampledTexture,
                byte_len: 0,
            },
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 0,
                kind: GpuBindingKind::Uniform,
                byte_len: 16,
            },
        ],
    }
    .source()
    .unwrap();
    let pipeline = resources.create_pipeline(desc.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let mut vertex_bytes = Vec::new();
    for [x, y] in [[-1.0f32, 1.], [1., 1.], [1., -1.], [-1., -1.]] {
        vertex_bytes.extend_from_slice(&x.to_le_bytes());
        vertex_bytes.extend_from_slice(&y.to_le_bytes());
        vertex_bytes.extend_from_slice(&[255; 4]);
    }
    let source = Bytes::source(vertex_bytes);
    let vertices = resources
        .create_buffer(
            GpuBufferDesc {
                role: GpuBufferRole::Vertex,
                byte_len: 48,
            },
            source.as_com_ref(),
        )
        .unwrap();
    let source = Bytes::source([0u16, 1, 2, 0, 2, 3].into_iter().flat_map(u16::to_le_bytes).collect());
    let indices = resources
        .create_buffer(GpuBufferDesc { role: GpuBufferRole::Index, byte_len: 12 }, source.as_com_ref())
        .unwrap();
    let source = Bytes::source(vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255]);
    let image = resources
        .create_texture(
            GpuTextureDesc {
                size: Size { width: 2, height: 2 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            source.as_com_ref(),
        )
        .unwrap();
    let image = if imported {
        import_native_image(&gpu, &resources, &mut executor, &mut images, image)
    } else {
        image
    };
    let sampler = resources.create_sampler(GpuSamplerDesc::default()).unwrap();
    let mut recording = gpu.broker().begin_frame().unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut session = recording
        .begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.bind_vertex_buffer(3, vertices.as_com_ref(), 0).unwrap();
    view.bind_index_buffer(indices.as_com_ref(), GpuIndexFormat::U16, 0).unwrap();
    view.bind_texture(GpuShaderStage::Fragment, 0, image.as_com_ref(), sampler.as_com_ref())
        .unwrap();
    let white: Vec<_> = [1.0f32; 4].into_iter().flat_map(f32::to_le_bytes).collect();
    view.push_uniforms(GpuShaderStage::Fragment, 0, NcCopyFromBufferOffset::new(0, &white)).unwrap();
    view.push_view(Rect { x: 0, y: 0, width: 4, height: 8 }).unwrap();
    view.draw_indexed(6, 1, 0).unwrap();
    view.pop().unwrap();
    let image_b = if imported {
        let pixel = resources
            .create_texture(
                GpuTextureDesc {
                    size: Size { width: 2, height: 2 },
                    format: GpuTextureFormat::Rgba8Unorm,
                    mip_count: 1,
                },
                Bytes::source(vec![255; 16]).as_com_ref(),
            )
            .unwrap();
        let image_b = import_native_image(&gpu, &resources, &mut executor, &mut images, pixel);
        view.bind_texture(GpuShaderStage::Fragment, 0, image_b.as_com_ref(), sampler.as_com_ref())
            .unwrap();
        Some(image_b)
    } else {
        resources
            .update_texture(
                image.as_com_ref(),
                Rect { x: 0, y: 0, width: 2, height: 2 },
                Bytes::source(vec![255; 16]).as_com_ref(),
            )
            .unwrap();
        None
    };
    let magenta: Vec<_> = [1.0f32, 0., 1., 1.].into_iter().flat_map(f32::to_le_bytes).collect();
    view.push_uniforms(GpuShaderStage::Fragment, 0, NcCopyFromBufferOffset::new(0, &magenta))
        .unwrap();
    view.push_view(Rect { x: 4, y: 0, width: 4, height: 8 }).unwrap();
    view.draw_indexed(6, 1, 0).unwrap();
    view.pop().unwrap();
    drop(image_b);
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    recording.finish().unwrap();
    let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
    let prepared = executor.prepare(&snapshot, target.pass, target.signature, &images).unwrap();
    drop(images);
    drop((snapshot, owner, view, session, recording, vertices, indices, pipeline, image, sampler, vs, fs));

    let clear = if operation == GpuBlendOp::Min { [1.; 4] } else { [0., 0., 0., 1.] };
    target.render_color(&mut executor, vec![prepared], Size { width: 8, height: 8 }, clear);
    let pixels = target.read(&target.pixels);
    let pixel = |x: usize, y: usize| &pixels[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4];
    assert_eq!(pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(3, 0), [0, 255, 0, 255]);
    assert_eq!(pixel(0, 7), [0, 0, 255, 255]);
    assert_eq!(pixel(3, 7), [255, 255, 0, 255]);
    assert_eq!(pixel(6, 3), [255, 0, 255, 255]);

    executor.collect().unwrap();
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
    assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap(), (0, 0));
}

/// Portable front-face state preserves winding across canonical-to-Vulkan coordinates.
#[test]
#[ignore = "requires a native Vulkan graphics device, including a software device"]
fn front_face_and_culling_match_canonical_winding() {
    let core = core();
    let mut target = Target::new(core.clone(), 8, 8);
    let mut executor = Executor::new(core).unwrap();
    let gpu = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    for (front_face, cull, visible) in [
        (GpuFrontFace::Ccw, GpuCullMode::Back, true),
        (GpuFrontFace::Cw, GpuCullMode::Back, false),
        (GpuFrontFace::Ccw, GpuCullMode::Front, false),
        (GpuFrontFace::Cw, GpuCullMode::Front, true),
    ] {
        let pipeline = pipeline_state(&resources, GpuPipelineInfo { front_face, cull, ..Default::default() });
        let mut frame = gpu.broker().begin_frame().unwrap();
        let root = Rect { x: 0, y: 0, width: 8, height: 8 };
        let mut session = frame.begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref()).unwrap();
        let mut view = session.view().unwrap();
        view.bind_pipeline(pipeline.as_com_ref()).unwrap();
        values(&mut view, [1., 0., 0., 1.], 0.5);
        view.draw(3, 1, 0).unwrap();
        view.finish().unwrap();
        let block = session.take_block().unwrap();
        frame.finish().unwrap();
        let prepared = executor
            .prepare(&gpu.block_snapshot(block.as_com_ref()).unwrap(), target.pass, target.signature, &HashMap::new())
            .unwrap();
        target.render(&mut executor, vec![prepared], Size { width: 8, height: 8 });
        let pixels = target.read(&target.pixels);
        let expected = if visible { [255, 0, 0, 255] } else { [51, 51, 51, 255] };
        assert_eq!(&pixels[4 * (4 * 8 + 4)..4 * (4 * 8 + 4) + 4], expected);
    }
    executor.collect().unwrap();
}

/// Prepared resources may escape the presentation owner without retaining a renderer/cache cycle.
#[test]
#[ignore = "requires a native Vulkan graphics device, including a software device"]
fn escaped_prepared_block_keeps_only_its_cache_free_native_core_alive() {
    let core = core();
    let weak = Rc::downgrade(&core);
    let target = Target::new(core.clone(), 8, 8);
    let mut executor = Executor::new(core.clone()).unwrap();
    let gpu = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    let pipeline = pipeline(&resources, 15);
    let mut frame = gpu.broker().begin_frame().unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut session = frame.begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    values(&mut view, [1., 0., 0., 1.], 0.5);
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    frame.finish().unwrap();
    let prepared = executor
        .prepare(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), target.pass, target.signature, &HashMap::new())
        .unwrap();
    drop((owner, session, view, frame, pipeline, controller, resources, gpu));
    drop((executor, target, core));
    assert!(weak.upgrade().is_some(), "prepared native children must retain their device");
    drop(prepared);
    assert!(weak.upgrade().is_none(), "cache-free native owners must not form an idle cycle");
}

/// Tiny test caps prove early, device-wide count admission without stressing a real driver's limits.
#[test]
#[ignore = "requires a native Vulkan graphics device, including a software device"]
fn native_allocation_and_sampler_counts_refund_only_on_physical_release() {
    let mut core = core();
    let limits = &mut Rc::get_mut(&mut core).unwrap().limits;
    limits.max_memory_allocation_count = 2;
    limits.max_sampler_allocation_count = 1;
    let first = Allocation::new(&core, &[0; 16], vk::BufferUsageFlags::VERTEX_BUFFER).unwrap();
    let second = Allocation::new(&core, &[0; 16], vk::BufferUsageFlags::UNIFORM_BUFFER).unwrap();
    assert_eq!(core.memory_count.get(), 2);
    let error = Allocation::new(&core, &[0; 16], vk::BufferUsageFlags::TRANSFER_SRC).err().unwrap();
    assert!(error.contains("allocation count exhausted"));
    assert_eq!(core.memory_count.get(), 2);
    drop(first);
    assert_eq!(core.memory_count.get(), 1);
    let retry = Allocation::new(&core, &[0; 16], vk::BufferUsageFlags::TRANSFER_SRC).unwrap();
    drop((retry, second));
    assert_eq!(core.memory_count.get(), 0);

    let sampler = core.sampler(&vk::SamplerCreateInfo::default()).unwrap();
    assert_eq!(core.sampler_count.get(), 1);
    assert!(
        core.sampler(&vk::SamplerCreateInfo::default())
            .err()
            .unwrap()
            .contains("allocation count exhausted")
    );
    assert_eq!(core.sampler_count.get(), 1);
    drop(sampler);
    let retry = core.sampler(&vk::SamplerCreateInfo::default()).unwrap();
    drop(retry);
    assert_eq!(core.sampler_count.get(), 0);
}

/// A byte-bounded logical resource must not reach an invalid native image creation call.
#[test]
#[ignore = "requires a native Vulkan graphics device, including a software device"]
fn logical_images_exceeding_native_dimensions_reject_before_native_allocation() {
    let core = core();
    let mut executor = Executor::new(core.clone()).unwrap();
    let width = core.limits.max_image_dimension2_d.checked_add(1).unwrap();
    // Keep the probe bounded even on a future device whose dimensions exceed ordinary CPU quotas.
    if width > 262144 {
        assert!(core.image_extent(vk::Format::R8G8B8A8_UNORM, width, 1, vk::ImageUsageFlags::SAMPLED).is_err());
        return;
    }
    let gpu = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu.create_client(RenderClientLimits::default()).unwrap();
    let resources = controller.resources().unwrap();
    let bytes = Bytes::source(vec![255; width as usize * 4]);
    let image = resources
        .create_texture(
            GpuTextureDesc {
                size: Size { width: width as i32, height: 1 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            bytes.as_com_ref(),
        )
        .unwrap();
    let snapshot = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
    let error = executor.texture(snapshot, &HashMap::new()).err().unwrap();
    assert_eq!(error, "Vulkan image dimensions exceed device limits");
    assert_eq!(core.memory_count.get(), 0);
    assert!(!executor.unusable());
    let valid = resources
        .create_texture(
            GpuTextureDesc {
                size: Size { width: 1, height: 1 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            Bytes::source(vec![255; 4]).as_com_ref(),
        )
        .unwrap();
    let native = executor
        .texture(gpu.texture_snapshot(resources.as_com_ref(), valid.as_com_ref()).unwrap(), &HashMap::new())
        .unwrap();
    assert_eq!(core.memory_count.get(), 1);
    drop(native);
    drop(executor);
    assert_eq!(core.memory_count.get(), 0);
}

/// Trusted test image lease stands in for the host's separately authenticated immutable capability.
#[neocogi_com::com_object(interfaces(ITexture))]
struct ImportedImage;
impl ITexture for ImportedImage {
    /// The native metadata is fixed; no pixel or device getter is exposed.
    fn size(&self) -> Result<Size, neocogi_com::NcStatus> {
        Ok(Size { width: 2, height: 2 })
    }
}

/// Initialize real native pixels, then hand only a cache-free shared image to the host map.
fn import_native_image(
    gpu: &GpuDevice,
    resources: &ComPtr<IRenderResourcesAbi>,
    executor: &mut Executor,
    images: &mut HashMap<neocogi_render_core::TextureId, crate::VulkanTexture>,
    pixel: ComBox<IGpuTextureAbi>,
) -> ComBox<IGpuTextureAbi> {
    let native = executor
        .texture(gpu.texture_snapshot(resources.as_com_ref(), pixel.as_com_ref()).unwrap(), &HashMap::new())
        .unwrap();
    let id = neocogi_render_core::TextureId::for_upload(2, 2, &[0; 16]).unwrap();
    images.insert(
        id,
        crate::VulkanTexture {
            image: crate::ImageResource {
                native: Some(native.native.clone()),
                extent: vk::Extent2D { width: 2, height: 2 },
                layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            },
            framebuffer: None,
        },
    );
    let lease: ComPtr<ITextureAbi> = ImportedImage::from_value(ImportedImage).query().unwrap();
    gpu.import_ui_texture(resources.as_com_ref(), id, lease).unwrap()
}

#[path = "../../gpu/tests/support/sampled.rs"]
mod sampled;

/// Production private-output pass and fence submission render initialized generic UI pixels.
#[test]
#[ignore = "requires a native Vulkan device"]
fn private_output_uses_production_pass_upload_and_completion_paths() {
    let core = core();
    let mut executor = Executor::new(core.clone()).unwrap();
    let mut output_session = crate::offscreen::Offscreen::new(&core).unwrap();
    let size = Size { width: 2, height: 2 };
    let native = super::Image::allocate(
        &core,
        size,
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC,
        vk::ImageAspectFlags::COLOR,
    )
    .unwrap();
    let views = [native.view];
    let framebuffer = unsafe {
        core.device.create_framebuffer(
            &vk::FramebufferCreateInfo::default()
                .render_pass(output_session.render_pass)
                .attachments(&views)
                .width(2)
                .height(2)
                .layers(1),
            None,
        )
    }
    .unwrap();
    let output = crate::VulkanTexture {
        image: crate::ImageResource {
            native: Some(native.clone()),
            extent: vk::Extent2D { width: 2, height: 2 },
            layout: vk::ImageLayout::UNDEFINED,
        },
        framebuffer: Some(framebuffer),
    };
    let gpu = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu.create_ui_client(Default::default(), None).unwrap();
    let resources = domain.resources().unwrap();
    let colors = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
    let owner = sampled::block(&gpu, &resources, Rect { x: 0, y: 0, width: 2, height: 2 }, size, &colors);
    let prepared = executor
        .prepare(
            &gpu.block_snapshot(owner.as_com_ref()).unwrap(),
            output_session.render_pass,
            Signature {
                color: vk::Format::R8G8B8A8_UNORM,
                depth: None,
            },
            &HashMap::new(),
        )
        .unwrap();
    drop(owner);
    output_session
        .render(
            &executor,
            &output,
            vk::ClearValue {
                color: vk::ClearColorValue { float32: [0., 0., 0., 1.] },
            },
            2,
            2,
            &[prepared],
        )
        .unwrap();
    assert!(!output_session.unusable());
    assert_eq!(readback(&core, &native, size), colors);
    core.wait_idle();
    unsafe {
        core.device.destroy_framebuffer(framebuffer, None);
    }
}

/// Native fixture readback preserves a completed image's shader-readable layout for later sampling.
pub(crate) fn readback(core: &Rc<Core>, native: &super::Image, size: Size) -> Vec<u8> {
    // Target supplies only independent command/readback ownership; this image was rendered by
    // the production Offscreen session above, rather than a second test-only output executor.
    let target = Target::new(core.clone(), size.width as u32, size.height as u32);
    unsafe {
        core.device
            .begin_command_buffer(target.command, &vk::CommandBufferBeginInfo::default())
            .unwrap();
        let barrier = vk::ImageMemoryBarrier::default()
            .image(native.raw)
            .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::SHADER_READ)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        core.device.cmd_pipeline_barrier(
            target.command,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
        let copy = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
            .image_extent(vk::Extent3D {
                width: size.width as u32,
                height: size.height as u32,
                depth: 1,
            });
        core.device
            .cmd_copy_image_to_buffer(target.command, native.raw, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, target.pixels.raw, &[copy]);
        let restore = vk::ImageMemoryBarrier::default()
            .image(native.raw)
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        core.device.cmd_pipeline_barrier(
            target.command,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::VERTEX_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[restore],
        );
        core.device.end_command_buffer(target.command).unwrap();
        core.device.reset_fences(&[target.fence]).unwrap();
        core.device
            .queue_submit(core.queue, &[vk::SubmitInfo::default().command_buffers(&[target.command])], target.fence)
            .unwrap();
    }
    let pixels = target.read(&target.pixels);
    pixels
}
