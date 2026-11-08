//! Common attachment assembly for native presentation, private outputs and native readback tests.
use super::*;

/// Create one color pass and an optional depth attachment with explicit host-selected final layouts.
/// Initial UNDEFINED discards old contents; subpass references always select attachment-optimal
/// layouts. Readback finals preserve depth; ordinary presentation may discard it after the pass.
pub(super) fn create(
    device: &ash::Device,
    signature: generic::Signature,
    color_final: vk::ImageLayout,
    depth_final: Option<vk::ImageLayout>,
) -> Result<vk::RenderPass> {
    if signature.depth.is_some() != depth_final.is_some() {
        return Err("depth format/final layout mismatch".into());
    }
    let mut attachments = vec![
        vk::AttachmentDescription::default()
            .format(signature.color)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(color_final),
    ];
    let color = vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let depth = vk::AttachmentReference::default()
        .attachment(1)
        .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
    if let (Some(format), Some(final_layout)) = (signature.depth, depth_final) {
        attachments.push(
            vk::AttachmentDescription::default()
                .format(format)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(if final_layout == vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL {
                    vk::AttachmentStoreOp::DONT_CARE
                } else {
                    vk::AttachmentStoreOp::STORE
                })
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(final_layout),
        );
    }
    let colors = [color];
    let mut subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&colors);
    if signature.depth.is_some() {
        subpass = subpass.depth_stencil_attachment(&depth);
    }
    let stages = vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS;
    let writes = vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE;
    let mut dependencies = vec![
        vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(stages)
            .dst_stage_mask(stages)
            .src_access_mask(writes)
            .dst_access_mask(writes | vk::AccessFlags::COLOR_ATTACHMENT_READ),
    ];
    if color_final == vk::ImageLayout::TRANSFER_SRC_OPTIMAL || depth_final == Some(vk::ImageLayout::TRANSFER_SRC_OPTIMAL) {
        dependencies.push(
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(stages)
                .src_access_mask(writes)
                .dst_stage_mask(vk::PipelineStageFlags::TRANSFER)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ),
        );
    } else if color_final == vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL {
        dependencies.push(
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(stages)
                .src_access_mask(writes)
                .dst_stage_mask(vk::PipelineStageFlags::VERTEX_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags::SHADER_READ),
        );
    }
    let subpasses = [subpass];
    // SAFETY: Every attachment/reference and dependency remains initialized/live for this native
    // call. The caller immediately arms deterministic render-pass ownership or returns the error.
    unsafe {
        device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(&attachments)
                .subpasses(&subpasses)
                .dependencies(&dependencies),
            None,
        )
    }
    .map_err(|e| format!("create graphics render pass: {e:?}"))
}
