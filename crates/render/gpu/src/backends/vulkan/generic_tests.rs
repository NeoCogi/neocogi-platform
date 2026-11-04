//! Headless direct-Vulkan acceptance uses real COM recording, native passes and readback.
use super::*;
use crate::backends::vulkan::sampled;
use neocogi_com::{ComBox, ComPtr, NcCopyFromBufferOffset};
use crate::{
    source::{Bytes, PipelineDesc},
};

type GpuDevice = crate::GpuDevice<Backend>;

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
            executor.core.clone(),
            nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
                .query()
                .unwrap(),
        );
        let controller = gpu
            .create_client(
                RenderClientLimits::default(),
                gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
                neocogi_render_core::abi::RenderClientPermissions::default(),
            )
            .unwrap();
        let resources = controller.resources().unwrap();
        let image = resources
            .create_static_buffer(
                GpuBufferDesc::Texture {
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
            Some(source.pixels().unwrap().as_slice()),
            Some(source.receipt()),
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
                .upload_image(Size { width: 1, height: 1 }, vk::Format::R8G8B8A8_UNORM, Some(&[0; 4]), None)
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
        let pixels = Allocation::new(&core, (width * height * 4) as usize, None, vk::BufferUsageFlags::TRANSFER_DST).unwrap();
        let depths = Allocation::new(&core, (width * height * 4) as usize, None, vk::BufferUsageFlags::TRANSFER_DST).unwrap();
        unsafe {
            let pass = crate::backends::vulkan::pass::create(
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
            self.core
                .device
                .invalidate_mapped_memory_ranges(&[vk::MappedMemoryRange::default().memory(buffer.memory.memory.raw).offset(0).size(vk::WHOLE_SIZE)])
                .unwrap();
            std::slice::from_raw_parts(buffer.memory.pointer.as_ptr(), (self.extent.width * self.extent.height * 4) as usize).to_vec()
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
fn values(resources: &ComPtr<IRenderResourcesAbi>, view: &mut ComBox<IRenderViewAbi>, color: [f32; 4], depth: f32) -> Vec<ComBox<IGpuBufferAbi>> {
    let bytes: Vec<_> = color.into_iter().chain([depth, 0., 0., 0.]).flat_map(f32::to_le_bytes).collect();
    [GpuShaderStage::Vertex, GpuShaderStage::Fragment]
        .into_iter()
        .map(|stage| crate::testing::bind_uniform(resources.as_com_ref(), view, stage, 0, &bytes).unwrap())
        .collect()
}

/// Native color AND depth readback proves confinement, reset, ordered scopes and exact DPI mapping.
#[test]
#[ignore = "requires a native Vulkan graphics device, including a software device"]
fn regional_depth_and_copied_uniforms_preserve_surrounding_targets() {
    let core = core();
    let mut target = Target::new(core.clone(), 64, 64);
    let mut executor = Executor::new(core.clone()).unwrap();
    let gpu = GpuDevice::new(
        executor.core.clone(),
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu
        .create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_render_core::abi::RenderClientPermissions::default(),
        )
        .unwrap();
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
    let _uniforms = values(&resources, &mut view, [1., 0., 0., 1.], 0.25);
    view.draw(3, 1, 0).unwrap();
    drop(_uniforms);
    view.push_clip(Rect { x: 14, y: 0, width: 14, height: 28 }).unwrap();
    view.clear_depth(1.).unwrap();
    let _uniforms = values(&resources, &mut view, [0., 0., 1., 1.], 0.75);
    view.draw(3, 1, 0).unwrap();
    drop(_uniforms);
    view.pop().unwrap();
    view.reset().unwrap();
    let _uniforms = values(&resources, &mut view, [1., 1., 0., 1.], 0.9);
    view.draw(3, 1, 0).unwrap();
    drop(_uniforms);
    view.finish().unwrap();
    let first = session.take_block().unwrap();
    let second = Rect { x: 10, y: 10, width: 12, height: 12 };
    let mut second_session = frame
        .begin_view(RenderViewRoot { client: second, target: second }, resources.as_com_ref())
        .unwrap();
    let mut second_view = second_session.view().unwrap();
    second_view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    let _uniforms = values(&resources, &mut second_view, [0., 1., 0., 1.], 0.75);
    second_view.draw(3, 1, 0).unwrap();
    drop(_uniforms);
    second_view.finish().unwrap();
    let second = second_session.take_block().unwrap();
    let third = Rect { x: 1, y: 1, width: 1, height: 1 };
    let mut third_session = frame
        .begin_view(RenderViewRoot { client: third, target: third }, resources.as_com_ref())
        .unwrap();
    let mut third_view = third_session.view().unwrap();
    third_view.bind_pipeline(hidden.as_com_ref()).unwrap();
    let _uniforms = values(&resources, &mut third_view, [1., 0., 1., 1.], 0.5);
    third_view.draw(3, 1, 0).unwrap();
    drop(_uniforms);
    third_view.clear_color(0xffffffff).unwrap();
    third_view.finish().unwrap();
    let third = third_session.take_block().unwrap();
    frame.finish().unwrap();
    let mut blocks = Vec::new();
    for owner in [&first, &second, &third] {
        blocks.push(
            executor
                .prepare(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), target.pass, target.signature)
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
        executor.core.clone(),
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu
        .create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_render_core::abi::RenderClientPermissions::default(),
        )
        .unwrap();
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
    let _uniform = crate::testing::bind_uniform(resources.as_com_ref(), &mut view, GpuShaderStage::Fragment, 0, &bytes).unwrap();
    view.draw(3, 1, 0).unwrap();
    drop(_uniform);
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    frame.finish().unwrap();
    let prepared = executor
        .prepare(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), target.pass, target.signature)
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
        executor.core.clone(),
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = if imported {
        gpu.create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_render_core::abi::RenderClientPermissions {
                clear_color: true,
                write_color: true,
                write_resources: true,
                import_images: true,
                ..neocogi_render_core::abi::RenderClientPermissions::none()
            },
        )
    } else {
        gpu.create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_render_core::abi::RenderClientPermissions::default(),
        )
    }
    .unwrap();
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
        .create_static_buffer(GpuBufferDesc::Vertex { byte_len: 48 }, source.as_com_ref())
        .unwrap();
    let source = Bytes::source([0u16, 1, 2, 0, 2, 3].into_iter().flat_map(u16::to_le_bytes).collect());
    let indices = resources
        .create_static_buffer(GpuBufferDesc::Index { byte_len: 12 }, source.as_com_ref())
        .unwrap();
    let image = resources
        .create_streamed_buffer(GpuBufferDesc::Texture {
            size: Size { width: 2, height: 2 },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        })
        .unwrap();
    crate::testing::write_pixels(
        image.as_com_ref(),
        Rect { x: 0, y: 0, width: 2, height: 2 },
        &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255],
    )
    .unwrap();
    let image = if imported {
        import_native_image(&gpu, &resources, &mut executor, image)
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
    let _uniform = crate::testing::bind_uniform(resources.as_com_ref(), &mut view, GpuShaderStage::Fragment, 0, &white).unwrap();
    view.push_view(Rect { x: 0, y: 0, width: 4, height: 8 }).unwrap();
    view.draw_indexed(6, 1, 0).unwrap();
    drop(_uniform);
    view.pop().unwrap();
    let image_b = if imported {
        let pixel = resources
            .create_static_buffer(
                GpuBufferDesc::Texture {
                    size: Size { width: 2, height: 2 },
                    format: GpuTextureFormat::Rgba8Unorm,
                    mip_count: 1,
                },
                Bytes::source(vec![255; 16]).as_com_ref(),
            )
            .unwrap();
        let image_b = import_native_image(&gpu, &resources, &mut executor, pixel);
        view.bind_texture(GpuShaderStage::Fragment, 0, image_b.as_com_ref(), sampler.as_com_ref())
            .unwrap();
        Some(image_b)
    } else {
        crate::testing::write_pixels(image.as_com_ref(), Rect { x: 0, y: 0, width: 2, height: 2 }, &[255; 16]).unwrap();
        None
    };
    let magenta: Vec<_> = [1.0f32, 0., 1., 1.].into_iter().flat_map(f32::to_le_bytes).collect();
    let _uniform = crate::testing::bind_uniform(resources.as_com_ref(), &mut view, GpuShaderStage::Fragment, 0, &magenta).unwrap();
    view.push_view(Rect { x: 4, y: 0, width: 4, height: 8 }).unwrap();
    view.draw_indexed(6, 1, 0).unwrap();
    drop(_uniform);
    view.pop().unwrap();
    drop(image_b);
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    recording.finish().unwrap();
    let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
    let prepared = executor.prepare(&snapshot, target.pass, target.signature).unwrap();
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
        executor.core.clone(),
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu
        .create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_render_core::abi::RenderClientPermissions::default(),
        )
        .unwrap();
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
        let _uniforms = values(&resources, &mut view, [1., 0., 0., 1.], 0.5);
        view.draw(3, 1, 0).unwrap();
        drop(_uniforms);
        view.finish().unwrap();
        let block = session.take_block().unwrap();
        frame.finish().unwrap();
        let prepared = executor
            .prepare(&gpu.block_snapshot(block.as_com_ref()).unwrap(), target.pass, target.signature)
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
        executor.core.clone(),
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu
        .create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_render_core::abi::RenderClientPermissions::default(),
        )
        .unwrap();
    let resources = controller.resources().unwrap();
    let pipeline = pipeline(&resources, 15);
    let mut frame = gpu.broker().begin_frame().unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut session = frame.begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    let _uniforms = values(&resources, &mut view, [1., 0., 0., 1.], 0.5);
    view.draw(3, 1, 0).unwrap();
    drop(_uniforms);
    view.finish().unwrap();
    let owner = session.take_block().unwrap();
    frame.finish().unwrap();
    let prepared = executor
        .prepare(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), target.pass, target.signature)
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
    let first = Allocation::new(&core, 16, None, vk::BufferUsageFlags::VERTEX_BUFFER).unwrap();
    let second = Allocation::new(&core, 16, None, vk::BufferUsageFlags::UNIFORM_BUFFER).unwrap();
    assert_eq!(core.memory_count.get(), 2);
    let error = Allocation::new(&core, 16, None, vk::BufferUsageFlags::TRANSFER_SRC).err().unwrap();
    assert!(error.contains("allocation count exhausted"));
    assert_eq!(core.memory_count.get(), 2);
    drop(first);
    assert_eq!(core.memory_count.get(), 1);
    let retry = Allocation::new(&core, 16, None, vk::BufferUsageFlags::TRANSFER_SRC).unwrap();
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
        executor.core.clone(),
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let controller = gpu
        .create_client(
            RenderClientLimits::default(),
            gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_render_core::abi::RenderClientPermissions::default(),
        )
        .unwrap();
    let resources = controller.resources().unwrap();
    let bytes = Bytes::source(vec![255; width as usize * 4]);
    let image = resources
        .create_static_buffer(
            GpuBufferDesc::Texture {
                size: Size { width: width as i32, height: 1 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            bytes.as_com_ref(),
        )
        .unwrap();
    let snapshot = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
    let error = snapshot
        .native(&mut Preparation {
            executor: &mut executor,
            pass: vk::RenderPass::null(),
        })
        .err()
        .unwrap();
    assert_eq!(error, "Vulkan image dimensions exceed device limits");
    assert_eq!(core.memory_count.get(), 0);
    assert!(!executor.unusable());
    let valid = resources
        .create_static_buffer(
            GpuBufferDesc::Texture {
                size: Size { width: 1, height: 1 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            Bytes::source(vec![255; 4]).as_com_ref(),
        )
        .unwrap();
    let version = gpu.texture_snapshot(resources.as_com_ref(), valid.as_com_ref()).unwrap();
    let native = version
        .native(&mut Preparation {
            executor: &mut executor,
            pass: vk::RenderPass::null(),
        })
        .unwrap();
    assert_eq!(core.memory_count.get(), 1);
    drop(native);
    drop(executor);
    assert_eq!(core.memory_count.get(), 1, "returned version owns native storage beyond the driver");
    drop((version, valid));
    assert_eq!(core.memory_count.get(), 0);
}

/// Initialize ordinary native pixels and capture the actual shared image without a public lease.
fn import_native_image(
    gpu: &GpuDevice,
    resources: &ComPtr<IRenderResourcesAbi>,
    executor: &mut Executor,
    pixel: ComBox<IGpuBufferAbi>,
) -> ComBox<IGpuBufferAbi> {
    let source = gpu.texture_snapshot(resources.as_com_ref(), pixel.as_com_ref()).unwrap();
    let native = executor
        .create_image(source.desc().size, vk::Format::R8G8B8A8_UNORM, Some(source.pixels().unwrap().as_slice()), None)
        .unwrap();
    gpu.import_texture(resources.as_com_ref(), native).unwrap()
}

/// Production private-output pass and fence submission render initialized generic UI pixels.
#[test]
#[ignore = "requires a native Vulkan device"]
fn private_output_uses_production_pass_upload_and_completion_paths() {
    let core = core();
    let mut executor = Executor::new(core.clone()).unwrap();
    let mut output_session = crate::backends::vulkan::offscreen::Offscreen::new(&core).unwrap();
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
    let output = Rc::new(crate::backends::vulkan::OutputState {
        image: executor.own_image(native.clone(), size, None),
        framebuffer: Some(crate::backends::vulkan::FramebufferAllocation {
            core: core.clone(),
            raw: framebuffer,
            _native: native.clone(),
        }),
        retired: executor.sender(),
    });
    let gpu = GpuDevice::new(
        executor.core.clone(),
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_render_core::abi::RenderClientPermissions {
                clear_color: true,
                write_color: true,
                write_resources: true,
                import_images: true,
                ..neocogi_render_core::abi::RenderClientPermissions::none()
            },
        )
        .unwrap();
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
    drop(output);
    executor.collect().unwrap();
}

/// Native fixture readback preserves a completed image's shader-readable layout for later sampling.
pub(in crate::backends::vulkan) fn readback(core: &Rc<Core>, native: &super::Image, size: Size) -> Vec<u8> {
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
    target.read(&target.pixels)
}

/// Native siblings reuse their own target signatures without pinning the source or a driver cache.
#[test]
#[ignore = "requires a native Vulkan device"]
fn unsubmitted_variants_reuse_native_objects_and_reject_other_cores() {
    let core = core();
    let mut executor = Executor::new(core.clone()).unwrap();
    let target = Target::new(core.clone(), 8, 8);
    let output = crate::backends::vulkan::offscreen::Offscreen::new(&core).unwrap();
    let offscreen = Signature {
        color: vk::Format::R8G8B8A8_UNORM,
        depth: None,
    };
    let gpu = GpuDevice::new(
        core.clone(),
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_render_core::abi::RenderClientPermissions {
                clear_color: true,
                write_color: true,
                write_resources: true,
                import_images: true,
                ..neocogi_render_core::abi::RenderClientPermissions::none()
            },
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let mut other = Executor::new(self::core()).unwrap();
    let baseline = core.live_allocations();
    for _ in 0..8 {
        let owner = sampled::block(
            &gpu,
            &resources,
            Rect { x: 0, y: 0, width: 8, height: 8 },
            Size { width: 1, height: 1 },
            &[255; 4],
        );
        let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
        let Command::Draw(draw) = &snapshot.commands()[0] else { panic!("draw") };
        let source = Rc::downgrade(&draw.pipeline);
        assert!(other.prepare(&snapshot, vk::RenderPass::null(), offscreen).is_err());
        let first = executor.prepare(&snapshot, target.pass, target.signature).unwrap();
        let second = executor.prepare(&snapshot, output.render_pass, offscreen).unwrap();
        let repeated = executor.prepare(&snapshot, target.pass, target.signature).unwrap();
        let native = |block: &PreparedBlock| {
            let PreparedCommand::Draw(draw) = &block.0.commands[0] else { panic!("draw") };
            draw.pipeline.clone()
        };
        assert!(Rc::ptr_eq(&native(&first), &native(&repeated)));
        assert!(!Rc::ptr_eq(&native(&first), &native(&second)));
        assert!(other.admit(&first, target.signature).is_err());
        drop((snapshot, owner));
        assert!(source.upgrade().is_none());
        drop((first, second, repeated));
        executor.collect().unwrap();
        assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
        assert_eq!(gpu.recording_usage(resources.as_com_ref()).unwrap(), (0, 0));
        // Retiring final native owners releases every resource allocation.
        assert_eq!(core.live_allocations(), baseline);
    }
}

/// Constructor/completion loss observations invalidate existing Ready slots on that same native core.
#[test]
#[ignore = "requires a native Vulkan device"]
fn observed_device_loss_rejects_previously_realized_native_storage() {
    let core = core();
    let mut executor = Executor::new(core.clone()).unwrap();
    let gpu = GpuDevice::new(
        core.clone(),
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_render_core::abi::RenderClientPermissions::default(),
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let buffer = resources.create_dynamic_buffer(GpuBufferDesc::Vertex { byte_len: 16 }).unwrap();
    let source = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
    let native = source
        .native(&mut Preparation {
            executor: &mut executor,
            pass: vk::RenderPass::null(),
        })
        .unwrap();
    assert_eq!(core.live_allocations(), 1);
    // Exercise the same error classifier used by actual Vulkan calls; deliberately losing a
    // physical device is neither deterministic nor necessary to test the authoritative latch.
    assert!(
        core.failure("observed native operation", vk::Result::ERROR_DEVICE_LOST)
            .contains("ERROR_DEVICE_LOST")
    );
    assert!(
        source
            .native(&mut Preparation {
                executor: &mut executor,
                pass: vk::RenderPass::null()
            })
            .is_err()
    );
    drop((source, buffer, native));
    assert!(executor.collect().is_err());
    assert_eq!(core.live_allocations(), 0, "terminal admission must still drain safe native retirement");
}

/// Pool reset waits for every prepared clone and completion owner, then real replay rewrites
/// every consumed descriptor and produces identical sampled output after native pool reuse.
#[test]
#[ignore = "requires a native Vulkan device"]
fn descriptor_capacity_reuses_only_after_final_prepared_and_completion_ownership() {
    let core = core();
    let mut executor = Executor::new(core.clone()).unwrap();
    let mut target = Target::new(core.clone(), 8, 8);
    let gpu = GpuDevice::new(
        core.clone(),
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(neocogi_render_core::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            Default::default(),
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let owner = sampled::block(
        &gpu,
        &resources,
        Rect { x: 0, y: 0, width: 8, height: 8 },
        Size { width: 1, height: 1 },
        &[255; 4],
    );
    let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
    let first = executor.prepare(&snapshot, target.pass, target.signature).unwrap();
    let handle = first.0._pool.as_ref().unwrap().raw;
    let retained = first.clone();
    target.render_color(&mut executor, vec![first], Size { width: 8, height: 8 }, [0., 0., 0., 1.]);
    let expected = target.read(&target.pixels);
    assert!(expected.as_chunks::<4>().0.iter().all(|pixel| pixel == &[255; 4]));
    executor.collect().unwrap();
    assert!(executor.pools.idle.is_empty(), "a prepared clone still owns the descriptor handles");
    let second = executor.prepare(&snapshot, target.pass, target.signature).unwrap();
    assert_ne!(second.0._pool.as_ref().unwrap().raw, handle);
    drop(retained);
    executor.collect().unwrap();
    assert_eq!(executor.pools.idle.len(), 1);
    let reused = executor.prepare(&snapshot, target.pass, target.signature).unwrap();
    assert_eq!(reused.0._pool.as_ref().unwrap().raw, handle);
    target.render_color(&mut executor, vec![reused], Size { width: 8, height: 8 }, [0., 0., 0., 1.]);
    assert_eq!(target.read(&target.pixels), expected);
    drop((second, snapshot, owner));
    executor.collect().unwrap();
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
    assert!(executor.pools.sets <= IDLE_DESCRIPTOR_SETS);
    assert_eq!(executor.pools.idle.len(), 2);
}

/// Descriptor caches bound both opaque native pool count and total declared set capacity.
#[test]
#[ignore = "requires a native Vulkan device"]
fn idle_descriptor_capacity_obeys_pool_and_set_bounds() {
    let core = core();
    let make = |sets, uniforms, images| {
        let mut sizes = Vec::new();
        for (ty, count) in [
            (vk::DescriptorType::UNIFORM_BUFFER, uniforms),
            (vk::DescriptorType::SAMPLED_IMAGE, images),
            (vk::DescriptorType::SAMPLER, images),
        ] {
            if count > 0 {
                sizes.push(vk::DescriptorPoolSize { ty, descriptor_count: count });
            }
        }
        // SAFETY: Every capacity is backed by the corresponding native pool descriptor.
        let raw = unsafe {
            core.device
                .create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(sets).pool_sizes(&sizes), None)
        }
        .unwrap();
        PoolAllocation {
            core: core.clone(),
            raw,
            capacity: PoolCapacity { sets, uniforms, images },
        }
    };
    let mut pools = DescriptorPools::default();
    for _ in 0..IDLE_DESCRIPTOR_POOLS + 1 {
        pools.put(make(4, 0, 0));
    }
    assert_eq!(pools.idle.len(), IDLE_DESCRIPTOR_POOLS);
    assert_eq!(pools.sets, 4 * IDLE_DESCRIPTOR_POOLS as u32);
    pools.clear();
    for (sets, uniforms, images) in [
        (IDLE_DESCRIPTOR_SETS + 4, 0, 0),
        (4, IDLE_DESCRIPTOR_BINDINGS + 1, 0),
        (4, 0, IDLE_DESCRIPTOR_BINDINGS + 1),
    ] {
        pools.put(make(sets, uniforms, images));
        assert!(pools.idle.is_empty());
        assert_eq!((pools.sets, pools.uniforms, pools.images), (0, 0, 0));
    }
    for _ in 0..2 {
        pools.put(make(4, IDLE_DESCRIPTOR_BINDINGS / 2, IDLE_DESCRIPTOR_BINDINGS / 2));
    }
    pools.put(make(4, 1, 1));
    assert_eq!(pools.idle.len(), 2, "aggregate category capacities also obey their bounds");
    assert_eq!((pools.uniforms, pools.images), (IDLE_DESCRIPTOR_BINDINGS, IDLE_DESCRIPTOR_BINDINGS));
    let allocation = pools.take(PoolCapacity { sets: 4, uniforms: 1, images: 1 }).unwrap();
    assert_eq!((pools.uniforms, pools.images), (IDLE_DESCRIPTOR_BINDINGS / 2, IDLE_DESCRIPTOR_BINDINGS / 2));
    pools.put(allocation);
    pools.clear();
    assert_eq!((pools.sets, pools.uniforms, pools.images), (0, 0, 0));
}

/// Native mapping lengths stay representable on both 32-bit and 64-bit hosts.
#[test]
fn native_buffer_padding_rejects_unrepresentable_host_lengths() {
    for (len, expected) in [(0, 16), (1, 16), (16, 16), (17, 32)] {
        assert_eq!(Allocation::padded_size(len).unwrap(), expected);
    }
    assert!(Allocation::padded_size(usize::MAX).is_err());
}

/// A writable public object keeps its VkBuffer, VkImage and upload allocation across writes.
/// Native clones block CPU admission even after the recorded source snapshot is released.
#[test]
#[ignore = "requires a native Vulkan device"]
fn uploaded_buffers_and_textures_reuse_their_own_native_allocations() {
    let core = core();
    let mut executor = Executor::new(core.clone()).unwrap();
    let gpu = GpuDevice::new(
        core.clone(),
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(RenderSurfaceKind::Framebuffer).unwrap(),
            Default::default(),
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let buffer = resources.create_dynamic_buffer(GpuBufferDesc::Uniform { byte_len: 32 }).unwrap();
    let image = resources
        .create_dynamic_buffer(GpuBufferDesc::Texture {
            size: Size { width: 2, height: 2 },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        })
        .unwrap();
    let mut handles = None;
    for fill in 1..=8u8 {
        crate::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 16]).unwrap();
        crate::testing::write_pixels(image.as_com_ref(), Rect { x: 0, y: 0, width: 2, height: 2 }, &[fill; 16]).unwrap();
        let source = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
        let pixels = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
        let native = source
            .native(&mut Preparation {
                executor: &mut executor,
                pass: vk::RenderPass::null(),
            })
            .unwrap();
        let texture = pixels
            .native(&mut Preparation {
                executor: &mut executor,
                pass: vk::RenderPass::null(),
            })
            .unwrap();
        let upload = texture.upload.as_ref().unwrap();
        let current = (native.allocation.raw, texture.native.raw, upload.raw);
        assert_eq!(*handles.get_or_insert(current), current);
        assert_eq!(core.live_allocations(), 3);
        // SAFETY: Synchronous upload completion has ended all device accesses, and
        // these initialized persistent mappings retain their native owners here.
        let bytes = unsafe { std::slice::from_raw_parts(native.allocation.memory.pointer.as_ptr(), 32) };
        assert_eq!(&bytes[..16], &[fill; 16]);
        assert_eq!(&bytes[16..], &[0; 16]);
        // SAFETY: The successful transfer waits before returning this upload owner.
        assert_eq!(unsafe { std::slice::from_raw_parts(upload.memory.pointer.as_ptr(), 16) }, &[fill; 16]);
        drop((source, pixels));
        assert_eq!(crate::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 4]), Err(RENDER_E_BUSY));
        assert_eq!(crate::testing::write_buffer(image.as_com_ref(), 0, &[fill; 4]), Err(RENDER_E_BUSY));
        drop((native, texture));
        crate::testing::write_buffer(buffer.as_com_ref(), 0, &[fill; 4]).unwrap();
        crate::testing::write_buffer(image.as_com_ref(), 0, &[fill; 4]).unwrap();
    }
    drop((buffer, image));
    executor.collect().unwrap();
    assert_eq!(core.live_allocations(), 0);
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
}

/// A failed rewrite pins the published slot, not just its raw inner allocation.
/// Another same-core executor must not overwrite staging until final idle retirement.
#[test]
#[ignore = "requires a native Vulkan graphics adapter"]
fn uncertain_texture_rewrites_exclude_uploads_and_other_compatible_executors() {
    for failure in [UploadFailure::Submit, UploadFailure::Wait] {
        let core = core();
        let mut executor = Executor::new(core.clone()).unwrap();
        let gpu = GpuDevice::new(
            core.clone(),
            nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
                .query()
                .unwrap(),
        );
        let domain = gpu
            .create_client(
                Default::default(),
                gpu.surface_target(RenderSurfaceKind::Framebuffer).unwrap(),
                Default::default(),
            )
            .unwrap();
        let resources = domain.resources().unwrap();
        let image = resources
            .create_dynamic_buffer(GpuBufferDesc::Texture {
                size: Size { width: 2, height: 2 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            })
            .unwrap();
        let rect = Rect { x: 0, y: 0, width: 2, height: 2 };
        crate::testing::write_pixels(image.as_com_ref(), rect, &[1; 16]).unwrap();
        let first = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
        let native = first
            .native(&mut Preparation {
                executor: &mut executor,
                pass: vk::RenderPass::null(),
            })
            .unwrap();
        let handle = native.native.raw;
        drop((first, native));
        crate::testing::write_pixels(image.as_com_ref(), rect, &[2; 16]).unwrap();
        let changed = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
        executor.upload_failure = Some(failure);
        assert!(
            changed
                .native(&mut Preparation {
                    executor: &mut executor,
                    pass: vk::RenderPass::null()
                })
                .is_err()
        );
        assert_eq!(executor.transfers.len(), 1);
        let mut compatible = Executor::new(core.clone()).unwrap();
        assert!(
            changed
                .native(&mut Preparation {
                    executor: &mut compatible,
                    pass: vk::RenderPass::null()
                })
                .is_err()
        );
        drop(changed);
        assert_eq!(crate::testing::write_buffer(image.as_com_ref(), 0, &[2; 16]), Err(RENDER_E_BUSY));
        // Failed-executor destruction waits for actual device idle before releasing
        // the exact published owner and its uncertain command/staging aggregate.
        drop(executor);
        crate::testing::write_buffer(image.as_com_ref(), 0, &[2; 16]).unwrap();
        let changed = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
        let native = changed
            .native(&mut Preparation {
                executor: &mut compatible,
                pass: vk::RenderPass::null(),
            })
            .unwrap();
        assert_eq!(native.native.raw, handle);
        drop((native, changed, image));
        compatible.collect().unwrap();
        assert_eq!(core.live_allocations(), 0);
        assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
    }
}
