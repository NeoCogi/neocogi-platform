//! Color-only image output shares the common executor and owns its submission lifetime.
use super::*;

/// Only completed command storage may be reused; uncertainty is terminal until native teardown.
#[derive(Clone, Copy)]
enum Submission {
    /// No submission references this session's command buffer.
    Ready,
    /// Submitted work has not yet produced an observed completion fence.
    Submitted,
    /// Submit/wait failure keeps every pending owner and forbids reuse.
    Failed,
}

/// Independent host output execution, with no UI pipeline, atlas or vertex representation.
pub(super) struct Offscreen {
    /// Cache-free native device ownership outlives all commands and attachment handles.
    core: Rc<generic::Core>,
    /// RGBA8 attachment contract is compatible with generic color-only pipelines.
    pub(super) render_pass: vk::RenderPass,
    /// Resettable command storage is independent of all presentation frame slots.
    command_pool: vk::CommandPool,
    /// Only Ready permits resetting this command buffer.
    command_buffer: vk::CommandBuffer,
    /// Completion observation precedes reuse and pending resource retirement.
    fence: vk::Fence,
    /// Legal transitions: Ready -> Submitted -> Ready; any submit/wait error -> Failed.
    submission: Submission,
    /// Native descriptor/backing/accounting owners survive a failed completion observation.
    pending: Vec<PreparedBlock>,
    /// Exact attachment aggregate survives uncertain use, including an empty clear-only frame.
    attachment: Option<Rc<OutputState>>,
}
impl Offscreen {
    /// Construct all output submission handles transactionally, without creating UI shader state.
    pub(super) fn new(core: &Rc<generic::Core>) -> Result<Self> {
        let device = &core.device;
        let render_pass = ResourceGuard::new(
            pass::create(
                device,
                generic::Signature {
                    color: vk::Format::R8G8B8A8_UNORM,
                    depth: None,
                },
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                None,
            )?,
            // SAFETY: This unpublished pass has no submitted users and belongs to this device.
            |pass| unsafe { device.destroy_render_pass(pass, None) },
        );
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(core.family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = ResourceGuard::new(
            // SAFETY: The live Core owns this device and its selected graphics queue family.
            unsafe { device.create_command_pool(&pool_info, None) }.map_err(|e| format!("offscreen command pool: {e:?}"))?,
            // SAFETY: Construction has not submitted any command allocated from this pool.
            |pool| unsafe { device.destroy_command_pool(pool, None) },
        );
        let allocation = vk::CommandBufferAllocateInfo::default()
            .command_pool(*command_pool.get())
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: The pool is live on this device and the initialized request allocates one buffer.
        let command_buffer = unsafe { device.allocate_command_buffers(&allocation) }.map_err(|e| format!("offscreen command buffer: {e:?}"))?[0];
        let fence = ResourceGuard::new(
            // SAFETY: The Core owns a live device; the initialized default creates an unsignaled fence.
            unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }.map_err(|e| format!("offscreen fence: {e:?}"))?,
            // SAFETY: This construction guard never submitted work referencing the fence.
            |fence| unsafe { device.destroy_fence(fence, None) },
        );
        Ok(Self {
            core: core.clone(),
            render_pass: render_pass.into_inner(),
            command_pool: command_pool.into_inner(),
            command_buffer,
            fence: fence.into_inner(),
            submission: Submission::Ready,
            pending: Vec::new(),
            attachment: None,
        })
    }

    /// Uncertain native completion permanently disables this output session.
    pub(super) fn unusable(&self) -> bool {
        !matches!(self.submission, Submission::Ready)
    }

    /// Validate and record only realized blocks, then retain native owners before queue submission.
    pub(super) fn render(
        &mut self,
        executor: &generic::Executor,
        output: &Rc<OutputState>,
        clear: vk::ClearValue,
        width: u32,
        height: u32,
        blocks: &[PreparedBlock],
    ) -> Result<()> {
        if self.unusable() {
            return Err("offscreen Vulkan submission is unavailable after an unfinished frame".into());
        }
        let signature = generic::Signature {
            color: vk::Format::R8G8B8A8_UNORM,
            depth: None,
        };
        for block in blocks {
            executor.admit(block, signature)?;
        }
        let mut pending = Vec::new();
        pending.try_reserve_exact(blocks.len()).map_err(|error| error.to_string())?;
        pending.extend(blocks.iter().cloned());
        let framebuffer = output.framebuffer();
        let size = <Backend as crate::native::NativeResources>::image_size(&output.image);
        let extent = vk::Extent2D {
            width: size.width as u32,
            height: size.height as u32,
        };
        let device = &self.core.device;
        // SAFETY: Ready proves no GPU access to this command storage. All handles belong to
        // the retained Core; output/framebuffer and realized blocks outlive recording/submission.
        unsafe {
            device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(|e| format!("reset offscreen commands: {e:?}"))?;
            device
                .begin_command_buffer(
                    self.command_buffer,
                    &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("begin offscreen commands: {e:?}"))?;
            device.cmd_begin_render_pass(
                self.command_buffer,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(framebuffer)
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent,
                    })
                    .clear_values(&[clear]),
                vk::SubpassContents::INLINE,
            );
        }
        for block in blocks {
            executor.record(
                self.command_buffer,
                block,
                crate::abi::Size {
                    width: width as i32,
                    height: height as i32,
                },
                crate::abi::Size {
                    width: extent.width as i32,
                    height: extent.height as i32,
                },
                signature,
            )?;
        }
        // SAFETY: Recording began above and every admitted block left this render pass active.
        unsafe {
            device.cmd_end_render_pass(self.command_buffer);
            device
                .end_command_buffer(self.command_buffer)
                .map_err(|e| format!("end offscreen commands: {e:?}"))?;
            device.reset_fences(&[self.fence]).map_err(|e| format!("reset offscreen fence: {e:?}"))?;
        }
        self.pending = pending;
        self.attachment = Some(output.clone());
        let command_buffers = [self.command_buffer];
        let submit = vk::SubmitInfo::default().command_buffers(&command_buffers);
        // Ownership is installed before submit: even a failed native call cannot release resources
        // whose execution status is uncertain. Failed sessions can retire only after idle or loss.
        // SAFETY: Command recording ended successfully; the reset fence and all referenced
        // native allocations are retained by this session before submission begins.
        match unsafe { device.queue_submit(self.core.queue, &[submit], self.fence) } {
            Ok(()) => self.submission = Submission::Submitted,
            Err(e) => {
                self.submission = Submission::Failed;
                return Err(self.core.failure("submit offscreen commands", e));
            }
        }
        // SAFETY: The live owned fence is attached to the successful submission above.
        match unsafe { device.wait_for_fences(&[self.fence], true, u64::MAX) } {
            Ok(()) => {
                self.submission = Submission::Ready;
                self.pending.clear();
                self.attachment.take();
                Ok(())
            }
            Err(e) => {
                self.submission = Submission::Failed;
                Err(self.core.failure("wait offscreen commands", e))
            }
        }
    }
}
impl Drop for Offscreen {
    /// Uncertain work must complete (or lose the device) before releasing command references.
    fn drop(&mut self) {
        if self.unusable() {
            self.core.wait_idle();
        }
        self.pending.clear();
        self.attachment.take();
        // SAFETY: Successful completion or idle/device loss ends all pending use; Core remains
        // alive until after these handles are destroyed and owns their exact native device.
        unsafe {
            self.core.device.destroy_fence(self.fence, None);
            self.core.device.destroy_command_pool(self.command_pool, None);
            self.core.device.destroy_render_pass(self.render_pass, None);
        }
    }
}
