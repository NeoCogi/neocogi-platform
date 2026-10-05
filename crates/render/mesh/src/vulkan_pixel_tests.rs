//! Headless framebuffer/readback owner for real Vulkan mesh draws and ordered color replacement.
use super::*;
use neocogi_render_core::{RenderItemArgs, prelude::Dimensioni};

/// A native image allocation; the target destroys its framebuffer before either view.
struct TestImage {
    /// Device dispatch borrowed from the parent owner.
    device: ash::Device,
    /// Optimal tiled image used by the render pass.
    image: vk::Image,
    /// Allocation bound to the image.
    memory: vk::DeviceMemory,
    /// Single-level color or depth attachment view.
    view: vk::ImageView,
}
impl TestImage {
    /// Allocates one tiny attachment using the selected physical device's memory capabilities.
    fn new(gpu: &TestDevice, format: vk::Format, usage: vk::ImageUsageFlags, extent: vk::Extent2D, aspect: vk::ImageAspectFlags) -> Self {
        // SAFETY: All create-info values are initialized and parent device outlives the returned owner.
        unsafe {
            let image = gpu
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
            let requirements = gpu.device.get_image_memory_requirements(image);
            let memory_type = find_memory_type(&gpu.memory_properties, requirements.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL).unwrap();
            let memory = gpu
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(memory_type),
                    None,
                )
                .unwrap();
            gpu.device.bind_image_memory(image, memory, 0).unwrap();
            let view = gpu
                .device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(vk::ImageSubresourceRange::default().aspect_mask(aspect).level_count(1).layer_count(1)),
                    None,
                )
                .unwrap();
            Self {
                device: gpu.device.clone(),
                image,
                memory,
                view,
            }
        }
    }
}
impl Drop for TestImage {
    /// Queue completion precedes target destruction, and all framebuffer references are gone.
    fn drop(&mut self) {
        // SAFETY: The parent target waits for completion and destroys its framebuffer first.
        unsafe {
            self.device.destroy_image_view(self.view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

/// Concrete operations keep painter order explicit without callbacks or erased payloads.
enum PixelDraw<'a> {
    /// Production mesh recording, including its viewport/scissor and depth behavior.
    Mesh { args: RenderItemArgs, command: &'a MeshCommand },
    /// Color-only replacement represents an intervening UI background without touching depth.
    Color { rect: vk::Rect2D, rgba: [f32; 4] },
}

/// Reuses color, depth, readback and command storage across completed frames.
struct PixelTarget<'a> {
    /// Physical framebuffer size; item coordinates remain in the 32 by 32 logical frame.
    extent: vk::Extent2D,
    /// The borrowed parent outlives all attachment and buffer allocations.
    gpu: &'a TestDevice,
    /// Color becomes transfer-readable after every render pass.
    color: TestImage,
    /// Depth stays attached across all item draws in one pass.
    _depth: TestImage,
    /// Host-coherent destination for completed RGBA pixels.
    readback: VulkanBuffer,
    /// Native attachment contract passed to the actual production mesh pipeline.
    render_pass: vk::RenderPass,
    /// The two attachment views, independent of any presentation surface.
    framebuffer: vk::Framebuffer,
    /// Resettable pool for one serially submitted graphics buffer.
    pool: vk::CommandPool,
    /// Primary graphics buffer reused only after queue completion.
    command_buffer: vk::CommandBuffer,
}
impl<'a> PixelTarget<'a> {
    /// Builds a native color/depth pass with an explicit color-write to readback dependency.
    fn new(gpu: &'a TestDevice, extent: vk::Extent2D) -> Self {
        let color = TestImage::new(
            gpu,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            extent,
            vk::ImageAspectFlags::COLOR,
        );
        let depth = TestImage::new(
            gpu,
            vk::Format::D32_SFLOAT,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
            extent,
            vk::ImageAspectFlags::DEPTH,
        );
        let readback = VulkanBuffer::new(
            &gpu.device,
            &gpu.memory_properties,
            &vec![0; (extent.width * extent.height * 4) as usize],
            vk::BufferUsageFlags::TRANSFER_DST,
        )
        .unwrap();
        let attachments = [
            vk::AttachmentDescription::default()
                .format(vk::Format::R8G8B8A8_UNORM)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL),
            vk::AttachmentDescription::default()
                .format(vk::Format::D32_SFLOAT)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
        ];
        let color_ref = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let depth_ref = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let subpasses = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_ref)
            .depth_stencil_attachment(&depth_ref)];
        let dependencies = [vk::SubpassDependency::default()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::TRANSFER)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)];
        // SAFETY: Attachment arrays and infos live through creation; all native owners outlive submission.
        unsafe {
            let render_pass = gpu
                .device
                .create_render_pass(
                    &vk::RenderPassCreateInfo::default()
                        .attachments(&attachments)
                        .subpasses(&subpasses)
                        .dependencies(&dependencies),
                    None,
                )
                .unwrap();
            let views = [color.view, depth.view];
            let framebuffer = gpu
                .device
                .create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(render_pass)
                        .attachments(&views)
                        .width(extent.width)
                        .height(extent.height)
                        .layers(1),
                    None,
                )
                .unwrap();
            let pool = gpu
                .device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                        .queue_family_index(gpu.family),
                    None,
                )
                .unwrap();
            let command_buffer = gpu
                .device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .unwrap()[0];
            Self {
                extent,
                gpu,
                color,
                _depth: depth,
                readback,
                render_pass,
                framebuffer,
                pool,
                command_buffer,
            }
        }
    }

    /// Executes production preparation and item recording in one native ordered graphics pass.
    fn draw(&mut self, renderer: &mut MeshRenderer, draws: &[PixelDraw<'_>]) -> Vec<u8> {
        let device = &self.gpu.device;
        let extent = self.extent;
        let rect = vk::Rect2D { offset: vk::Offset2D::default(), extent };
        let commands: Vec<_> = draws
            .iter()
            .filter_map(|draw| match draw {
                PixelDraw::Mesh { command, .. } => Some(*command),
                PixelDraw::Color { .. } => None,
            })
            .collect();
        // SAFETY: The previous submission completed before reuse; commands target live attachments.
        unsafe {
            device.reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty()).unwrap();
            device
                .begin_command_buffer(self.command_buffer, &vk::CommandBufferBeginInfo::default())
                .unwrap();
        }
        renderer
            .prepare(
                VulkanApplicationPrepareContext {
                    device,
                    command_buffer: self.command_buffer,
                    render_pass: self.render_pass,
                    color_format: vk::Format::R8G8B8A8_UNORM,
                    depth_format: vk::Format::D32_SFLOAT,
                    extent,
                    frame_index: 0,
                    frames_in_flight: 1,
                    memory_properties: self.gpu.memory_properties,
                    swapchain_generation: 1,
                },
                &commands,
            )
            .unwrap();
        let clears = [
            vk::ClearValue {
                color: vk::ClearColorValue { float32: [0., 0., 0., 1.] },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue { depth: 1., stencil: 0 },
            },
        ];
        // SAFETY: Framebuffer matches the supplied pass and bounded render area.
        unsafe {
            device.cmd_begin_render_pass(
                self.command_buffer,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(self.framebuffer)
                    .render_area(rect)
                    .clear_values(&clears),
                vk::SubpassContents::INLINE,
            );
        }
        let mut index = 0;
        for draw in draws {
            match draw {
                PixelDraw::Mesh { args, command } => {
                    renderer
                        .render(
                            VulkanApplicationRenderContext {
                                device,
                                command_buffer: self.command_buffer,
                                extent,
                                logical_width: 32,
                                logical_height: 32,
                            },
                            *args,
                            index,
                            command,
                        )
                        .unwrap();
                    index += 1;
                }
                PixelDraw::Color { rect, rgba } => {
                    let color = vk::ClearAttachment::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .color_attachment(0)
                        .clear_value(vk::ClearValue {
                            color: vk::ClearColorValue { float32: *rgba },
                        });
                    // SAFETY: This color-only clear is inside the pass and bounded by the supplied rectangle.
                    unsafe {
                        device.cmd_clear_attachments(self.command_buffer, &[color], &[vk::ClearRect::default().rect(*rect).layer_count(1)]);
                    }
                }
            }
        }
        let copy = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
            .image_extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            });
        let host = vk::BufferMemoryBarrier::default()
            .buffer(self.readback.buffer)
            .size(vk::WHOLE_SIZE)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ);
        // SAFETY: The pass publishes color writes to transfer; queue completion publishes coherent bytes.
        unsafe {
            device.cmd_end_render_pass(self.command_buffer);
            device.cmd_copy_image_to_buffer(
                self.command_buffer,
                self.color.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.readback.buffer,
                &[copy],
            );
            device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &[host],
                &[],
            );
            device.end_command_buffer(self.command_buffer).unwrap();
            let buffers = [self.command_buffer];
            let queue = device.get_device_queue(self.gpu.family, 0);
            device
                .queue_submit(queue, &[vk::SubmitInfo::default().command_buffers(&buffers)], vk::Fence::null())
                .unwrap();
            device.queue_wait_idle(queue).unwrap();
            let mapped = device
                .map_memory(
                    self.readback.memory,
                    0,
                    u64::from(extent.width * extent.height * 4),
                    vk::MemoryMapFlags::empty(),
                )
                .unwrap();
            let pixels = slice::from_raw_parts(mapped.cast::<u8>(), (extent.width * extent.height * 4) as usize).to_vec();
            device.unmap_memory(self.readback.memory);
            pixels
        }
    }
}
impl Drop for PixelTarget<'_> {
    /// Native recording objects release references before Rust drops the owned attachment views.
    fn drop(&mut self) {
        // SAFETY: Every draw waits for completion, including the final frame before destruction.
        unsafe {
            self.gpu.device.destroy_command_pool(self.pool, None);
            self.gpu.device.destroy_framebuffer(self.framebuffer, None);
            self.gpu.device.destroy_render_pass(self.render_pass, None);
        }
    }
}

/// All four clipped edges and an interior clip fit identically to a visible-size reference.
#[test]
#[ignore = "requires a Vulkan device"]
fn visible_client_rectangles_fit_the_scene_in_actual_pixels() {
    let gpu = TestDevice::new();
    let mut target = PixelTarget::new(&gpu, vk::Extent2D { width: 32, height: 32 });
    let mut renderer = MeshRenderer::new();
    let command = crate::pixel_tests::triangle_command();
    crate::pixel_tests::assert_visible_fit(|content_area, view| {
        target.draw(
            &mut renderer,
            &[PixelDraw::Mesh {
                args: RenderItemArgs {
                    dimensions: Dimensioni::new(32, 32),
                    content_area,
                    view,
                },
                command: &command,
            }],
        )
    });
}

/// Fractional physical scaling still uses one identical rectangle for viewport and scissor.
#[test]
#[ignore = "requires a Vulkan device"]
fn visible_client_fitting_survives_nonuniform_framebuffer_scaling() {
    let gpu = TestDevice::new();
    let mut target = PixelTarget::new(&gpu, vk::Extent2D { width: 48, height: 64 });
    let mut renderer = MeshRenderer::new();
    let command = crate::pixel_tests::triangle_command();
    crate::pixel_tests::assert_visible_fit(|content_area, view| {
        target.draw(
            &mut renderer,
            &[PixelDraw::Mesh {
                args: RenderItemArgs {
                    dimensions: Dimensioni::new(32, 32),
                    content_area,
                    view,
                },
                command: &command,
            }],
        )
    });
}

/// Fitting an interior client preserves previously painted color everywhere outside that clip.
#[test]
#[ignore = "requires a Vulkan device"]
fn visible_client_preserves_surrounding_painted_color() {
    let gpu = TestDevice::new();
    let mut target = PixelTarget::new(&gpu, vk::Extent2D { width: 32, height: 32 });
    let mut renderer = MeshRenderer::new();
    let command = crate::pixel_tests::triangle_command();
    let pixels = target.draw(
        &mut renderer,
        &[
            PixelDraw::Color {
                rect: vk::Rect2D {
                    offset: vk::Offset2D::default(),
                    extent: target.extent,
                },
                rgba: [0., 1., 0., 1.],
            },
            PixelDraw::Mesh {
                args: RenderItemArgs {
                    dimensions: Dimensioni::new(32, 32),
                    content_area: Recti::new(0, 0, 32, 32),
                    view: Recti::new(7, 5, 19, 13),
                },
                command: &command,
            },
        ],
    );
    assert!(pixels.chunks_exact(4).any(|pixel| pixel != [0, 255, 0, 255]));
    for y in 0..32 {
        for x in 0..32 {
            if !(7..26).contains(&x) || !(5..18).contains(&y) {
                assert_eq!(&pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4], &[0, 255, 0, 255]);
            }
        }
    }
}

/// Identical silhouettes with different depths and normals produce distinguishable scene colors.
fn depth_command(z: f32, normal_z: f32) -> MeshCommand {
    let MeshCommand::DrawMesh(mut submission) = crate::pixel_tests::triangle_command();
    let vertices = submission
        .mesh
        .vertices()
        .iter()
        .map(|vertex| {
            let mut vertex = *vertex;
            vertex.position.z = z;
            vertex.normal.z = normal_z;
            vertex
        })
        .collect();
    submission.mesh = MeshBuffers::from_vecs(vertices, vec![0, 1, 2]);
    MeshCommand::DrawMesh(submission)
}

/// Painter order dominates across scenes; self-occlusion still dominates inside one scene.
#[test]
#[ignore = "requires a Vulkan device"]
fn independent_mesh_items_keep_depth_local_with_ordered_color_and_self_occlusion() {
    let gpu = TestDevice::new();
    let mut target = PixelTarget::new(&gpu, vk::Extent2D { width: 32, height: 32 });
    let mut renderer = MeshRenderer::new();
    let near = depth_command(-0.6, 1.);
    let far = depth_command(0.6, -1.);
    let full = Recti::new(0, 0, 32, 32);
    let args = RenderItemArgs {
        dimensions: Dimensioni::new(32, 32),
        content_area: full,
        view: full,
    };
    let near_only = target.draw(&mut renderer, &[PixelDraw::Mesh { args, command: &near }]);
    let far_only = target.draw(&mut renderer, &[PixelDraw::Mesh { args, command: &far }]);
    assert_ne!(near_only, far_only, "the two scene materials must be distinguishable");
    for _ in 0..3 {
        assert_eq!(
            target.draw(
                &mut renderer,
                &[PixelDraw::Mesh { args, command: &near }, PixelDraw::Mesh { args, command: &far },]
            ),
            far_only,
            "a later farther independent scene wins painter order"
        );
        assert_eq!(
            target.draw(
                &mut renderer,
                &[PixelDraw::Mesh { args, command: &far }, PixelDraw::Mesh { args, command: &near },]
            ),
            near_only,
            "reversing independent items reverses their color result"
        );
    }
    let rect = vk::Rect2D {
        offset: vk::Offset2D::default(),
        extent: target.extent,
    };
    let color = || PixelDraw::Color { rect, rgba: [0., 1., 0., 1.] };
    let painted_reference = target.draw(&mut renderer, &[color(), PixelDraw::Mesh { args, command: &far }]);
    assert_eq!(
        target.draw(
            &mut renderer,
            &[PixelDraw::Mesh { args, command: &near }, color(), PixelDraw::Mesh { args, command: &far },]
        ),
        painted_reference,
        "intervening color replacement cannot leave stale scene depth"
    );
    let interior = RenderItemArgs { view: Recti::new(7, 5, 19, 13), ..args };
    let partial = target.draw(&mut renderer, &[PixelDraw::Mesh { args: interior, command: &far }]);
    let mut expected_partial = near_only.clone();
    for (destination, source) in expected_partial.chunks_exact_mut(4).zip(partial.chunks_exact(4)) {
        if source != [0, 0, 0, 255] {
            destination.copy_from_slice(source);
        }
    }
    assert_eq!(
        target.draw(
            &mut renderer,
            &[PixelDraw::Mesh { args, command: &near }, PixelDraw::Mesh { args: interior, command: &far },]
        ),
        expected_partial,
        "local depth clear preserves prior color outside the later silhouette"
    );
    let clipped_reference = target.draw(&mut renderer, &[color(), PixelDraw::Mesh { args: interior, command: &far }]);
    assert_eq!(
        target.draw(
            &mut renderer,
            &[
                PixelDraw::Mesh { args, command: &near },
                color(),
                PixelDraw::Mesh { args: interior, command: &far },
            ]
        ),
        clipped_reference,
        "interior item clear must preserve surrounding painted color"
    );

    // Both silhouettes in one submission share depth, so the nearer first triangle must survive.
    let MeshCommand::DrawMesh(near_mesh) = &near;
    let MeshCommand::DrawMesh(far_mesh) = &far;
    let mut vertices = near_mesh.mesh.vertices().to_vec();
    vertices.extend_from_slice(far_mesh.mesh.vertices());
    let combined = MeshCommand::DrawMesh(MeshSubmission {
        mesh: MeshBuffers::from_vecs(vertices, vec![0, 1, 2, 3, 4, 5]),
        pvm: Mat4f::identity(),
        view_model: Mat4f::identity(),
    });
    assert_eq!(
        target.draw(&mut renderer, &[PixelDraw::Mesh { args, command: &combined }]),
        near_only,
        "depth testing must still hide the farther triangle within one scene"
    );
}
