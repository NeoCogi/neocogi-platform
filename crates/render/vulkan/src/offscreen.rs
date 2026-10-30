//! UI-only Vulkan submission owns its own pass, command buffer, fence, and upload storage.
//! It shares device images/descriptors, but never borrows swapchain images or synchronization slots.
use super::*;

/// Submission progress controls whether command and upload storage may be reused.
#[derive(Clone, Copy)]
enum Submission {
    /// No in-flight use of this offscreen session's command buffer or upload storage.
    Ready,
    /// Submission succeeded; the completion fence has not yet been observed.
    Submitted,
    /// A submission/wait failure prevents safe reuse until the device is destroyed.
    Failed,
}

/// One packed draw references a contiguous range in a single host-visible vertex upload.
struct Draw {
    /// Byte offset of this triangle list in the shared frame upload.
    offset: vk::DeviceSize,
    /// Number of complete vertices to draw.
    count: u32,
    /// Existing device-owned sampled-image descriptor.
    descriptor: vk::DescriptorSet,
    /// Physical output clipping, with no native window scaling.
    clip: Recti,
}

/// Device-local execution resources independent of swapchain recreation and presentation.
pub(super) struct Offscreen {
    /// Cloned dispatch table; the enclosing renderer destroys this owner before its logical device.
    device: ash::Device,
    /// One RGBA8 color attachment with shader-readable final layout and explicit dependencies.
    pub(super) render_pass: vk::RenderPass,
    /// Layout has the same concrete sampled-image binding as native UI textures.
    descriptor_layout: vk::DescriptorSetLayout,
    /// Private push-constant/pipeline layout survives native UI-pool recreation.
    pipeline_layout: vk::PipelineLayout,
    /// Shared UI shaders compiled for the depth-free offscreen pass.
    pipeline: vk::Pipeline,
    /// Private resettable pool never touches native frame-in-flight command storage.
    command_pool: vk::CommandPool,
    /// Reused only after this owner's completion fence has signaled.
    command_buffer: vk::CommandBuffer,
    /// Completion authority for this submission alone; no swapchain semaphores are involved.
    fence: vk::Fence,
    /// CPU-visible coherent upload storage, retained and grown after prior completion.
    vertices: Option<MappedBuffer>,
    /// Explicit lifecycle for fallible submission and waiting.
    submission: Submission,
}
impl Offscreen {
    /// Allocates every native handle transactionally before publishing the execution owner.
    pub(super) fn new(ctx: &VulkanContext) -> Result<Self> {
        let device = &ctx.device;
        let attachment = vk::AttachmentDescription::default()
            .format(vk::Format::R8G8B8A8_UNORM)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let attachments = [attachment];
        let colors = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpasses = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&colors)];
        let dependencies = [
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::TOP_OF_PIPE)
                .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ),
        ];
        let pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        // SAFETY: All construction records and the live logical device remain valid for each call.
        let render_pass = ResourceGuard::new(
            unsafe { device.create_render_pass(&pass_info, None) }.map_err(|e| format!("offscreen render pass: {e:?}"))?,
            |pass| unsafe { device.destroy_render_pass(pass, None) },
        );
        let descriptor_layout = ResourceGuard::new(UiResources::create_descriptor_set_layout(device)?, |layout| unsafe {
            device.destroy_descriptor_set_layout(layout, None)
        });
        let pipeline_layout = ResourceGuard::new(UiResources::create_pipeline_layout(device, *descriptor_layout.get())?, |layout| unsafe {
            device.destroy_pipeline_layout(layout, None)
        });
        let pipeline = ResourceGuard::new(
            UiResources::create_pipeline(device, *render_pass.get(), *pipeline_layout.get())?,
            |pipeline| unsafe { device.destroy_pipeline(pipeline, None) },
        );
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(ctx.queue_indices.graphics_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = ResourceGuard::new(
            unsafe { device.create_command_pool(&pool_info, None) }.map_err(|e| format!("offscreen command pool: {e:?}"))?,
            |pool| unsafe { device.destroy_command_pool(pool, None) },
        );
        let allocation = vk::CommandBufferAllocateInfo::default()
            .command_pool(*command_pool.get())
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let command_buffer = unsafe { device.allocate_command_buffers(&allocation) }.map_err(|e| format!("offscreen command buffer: {e:?}"))?[0];
        let fence = ResourceGuard::new(
            unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }.map_err(|e| format!("offscreen fence: {e:?}"))?,
            |fence| unsafe { device.destroy_fence(fence, None) },
        );
        Ok(Self {
            device: device.clone(),
            render_pass: render_pass.into_inner(),
            descriptor_layout: descriptor_layout.into_inner(),
            pipeline_layout: pipeline_layout.into_inner(),
            pipeline: pipeline.into_inner(),
            command_pool: command_pool.into_inner(),
            command_buffer,
            fence: fence.into_inner(),
            vertices: None,
            submission: Submission::Ready,
        })
    }

    /// Failed or abandoned submitted work cannot reuse the command buffer or mapped storage.
    pub(super) fn unusable(&self) -> bool {
        !matches!(self.submission, Submission::Ready)
    }

    /// Packs one painter-ordered UI stream, records, submits, and observes its completion fence.
    pub(super) fn render(
        &mut self,
        ctx: &VulkanContext,
        output: &VulkanTexture,
        atlas: vk::DescriptorSet,
        clear: vk::ClearValue,
        vertices: &[Vertex],
        commands: &[FrameCommand],
    ) -> Result<()> {
        match self.submission {
            Submission::Ready => {}
            Submission::Submitted | Submission::Failed => return Err("offscreen Vulkan submission is unavailable after an unfinished frame".into()),
        }
        let extent = output.image.extent;
        let framebuffer = output.framebuffer.ok_or("image is not a Vulkan render target")?;
        let full = Recti::new(0, 0, extent.width as i32, extent.height as i32);
        let mut packed = Vec::new();
        let mut draws = Vec::new();
        let mut cursor = 0;
        for command in commands {
            let (slice, descriptor, clip) = match command {
                FrameCommand::DrawTo(end) => {
                    let end = (*end).min(vertices.len());
                    if end < cursor {
                        return Err("offscreen draw boundaries must be monotonic".into());
                    }
                    let slice = &vertices[cursor..end];
                    cursor = end;
                    (slice, atlas, full)
                }
                FrameCommand::Texture { clip, command } => (command.vertices.as_slice(), command.descriptor_set, *clip),
                FrameCommand::Colored { clip, command } => (command.vertices.as_slice(), atlas, *clip),
                FrameCommand::Block { .. } => return Err("offscreen Vulkan surfaces accept UI commands only".into()),
            };
            if descriptor == output.descriptor.set {
                return Err("a Vulkan render target cannot sample itself".into());
            }
            if slice.is_empty() {
                continue;
            }
            draws.push(Draw {
                offset: (packed.len() * mem::size_of::<Vertex>()) as u64,
                count: slice.len() as u32,
                descriptor,
                clip,
            });
            packed.extend_from_slice(slice);
        }
        let bytes = unsafe { std::slice::from_raw_parts(packed.as_ptr().cast::<u8>(), mem::size_of_val(packed.as_slice())) };
        if !bytes.is_empty() {
            let required = bytes.len() as u64;
            if self.vertices.as_ref().is_none_or(|buffer| buffer.size() < required) {
                let size = required.next_power_of_two().max(65536);
                let buffer = ctx.create_buffer(
                    size,
                    vk::BufferUsageFlags::VERTEX_BUFFER,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )?;
                let replacement = MappedBuffer::new(buffer, &self.device)?;
                if let Some(mut previous) = self.vertices.replace(replacement) {
                    previous.destroy(&self.device);
                }
            }
            self.vertices.as_ref().expect("upload allocated above").write(0, bytes)?;
        }
        // This command pool and upload buffer have no in-flight users while Ready.
        unsafe { self.device.reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty()) }
            .map_err(|e| format!("reset offscreen commands: {e:?}"))?;
        unsafe {
            self.device.begin_command_buffer(
                self.command_buffer,
                &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
        }
        .map_err(|e| format!("begin offscreen commands: {e:?}"))?;
        let clears = [clear];
        let area = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent,
        };
        let begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(framebuffer)
            .render_area(area)
            .clear_values(&clears);
        let projection = UiResources::ortho_matrix(extent.width as f32, extent.height as f32);
        // SAFETY: Every image, descriptor, pipeline, and vertex range is owned for this complete
        // recording/submission transaction. Output pixels transition to shader-read at pass end.
        unsafe {
            self.device.cmd_begin_render_pass(self.command_buffer, &begin, vk::SubpassContents::INLINE);
            self.device
                .cmd_bind_pipeline(self.command_buffer, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            self.device.cmd_set_viewport(self.command_buffer, 0, &[ui_full_viewport(extent)]);
            self.device.cmd_push_constants(
                self.command_buffer,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                UiResources::matrix_bytes(&projection),
            );
            for draw in draws {
                let clip = clamp_rect_to_surface(draw.clip, extent.width, extent.height);
                if clip.width <= 0 || clip.height <= 0 {
                    continue;
                }
                self.device
                    .cmd_set_scissor(self.command_buffer, 0, &[rect_to_vk(clip, extent.width, extent.height)]);
                self.device.cmd_bind_descriptor_sets(
                    self.command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    0,
                    &[draw.descriptor],
                    &[],
                );
                self.device.cmd_bind_vertex_buffers(
                    self.command_buffer,
                    0,
                    &[self.vertices.as_ref().expect("nonempty draw has upload").vk_buffer()],
                    &[draw.offset],
                );
                self.device.cmd_draw(self.command_buffer, draw.count, 1, 0, 0);
            }
            self.device.cmd_end_render_pass(self.command_buffer);
            self.device
                .end_command_buffer(self.command_buffer)
                .map_err(|e| format!("end offscreen commands: {e:?}"))?;
            self.device.reset_fences(&[self.fence]).map_err(|e| format!("reset offscreen fence: {e:?}"))?;
        }
        let buffers = [self.command_buffer];
        let submit = vk::SubmitInfo::default().command_buffers(&buffers);
        match unsafe { self.device.queue_submit(ctx.graphics_queue, &[submit], self.fence) } {
            Ok(()) => self.submission = Submission::Submitted,
            Err(error) => {
                self.submission = Submission::Failed;
                return Err(format!("submit offscreen commands: {error:?}"));
            }
        }
        match unsafe { self.device.wait_for_fences(&[self.fence], true, u64::MAX) } {
            Ok(()) => {
                self.submission = Submission::Ready;
                Ok(())
            }
            Err(error) => {
                self.submission = Submission::Failed;
                Err(format!("wait for offscreen completion: {error:?}"))
            }
        }
    }
}
impl Drop for Offscreen {
    /// The enclosing renderer waits for device idle before this owner and its images retire.
    fn drop(&mut self) {
        if let Some(mut vertices) = self.vertices.take() {
            vertices.destroy(&self.device);
        }
        // SAFETY: No queue can still reference these private execution resources after the
        // enclosing renderer's idle boundary; device loss also permits object destruction.
        unsafe {
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.pipeline_layout, None);
            self.device.destroy_descriptor_set_layout(self.descriptor_layout, None);
            self.device.destroy_render_pass(self.render_pass, None);
        }
    }
}
