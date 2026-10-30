//! Real Vulkan image, submission, and window-lifetime regression tests.
use super::*;

/// Reads a completed RGBA image without changing the descriptor's shader-readable layout.
fn pixels(renderer: &VulkanRenderer, id: TextureId) -> Vec<u8> {
    let ctx = &renderer.context;
    let image = &renderer.textures[&id].image;
    let length = (image.extent.width * image.extent.height * 4) as usize;
    let buffer = ctx
        .create_buffer(
            length as u64,
            vk::BufferUsageFlags::TRANSFER_DST,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .unwrap()
        .guarded(&ctx.device);
    ctx.single_time_commands(|command| {
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let before = vk::ImageMemoryBarrier::default()
            .image(image.image)
            .subresource_range(range)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::SHADER_READ)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
        let copy = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
            .image_extent(vk::Extent3D {
                width: image.extent.width,
                height: image.extent.height,
                depth: 1,
            });
        let after = vk::ImageMemoryBarrier::default()
            .image(image.image)
            .subresource_range(range)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(vk::AccessFlags::SHADER_READ);
        // SAFETY: Both images and the tightly packed destination remain live through queue idle.
        unsafe {
            ctx.device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[before],
            );
            ctx.device
                .cmd_copy_image_to_buffer(command, image.image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, buffer.get().buffer, &[copy]);
            ctx.device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[after],
            );
        }
    })
    .unwrap();
    // SAFETY: Submission completed and the host-coherent mapping contains exactly `length` bytes.
    unsafe {
        let mapped = ctx
            .device
            .map_memory(buffer.get().memory(), 0, length as u64, vk::MemoryMapFlags::empty())
            .unwrap();
        let bytes = std::slice::from_raw_parts(mapped.cast::<u8>(), length).to_vec();
        ctx.device.unmap_memory(buffer.get().memory());
        bytes
    }
}

/// Real texture passes preserve source orientation and older outputs without touching window extent.
#[test]
#[ignore = "requires an SDL native display and Vulkan adapter"]
fn independent_texture_frames_preserve_pixels_and_native_surface_configuration() {
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let window = video.window("Vulkan surface regression", 64, 64).vulkan().hidden().build().unwrap();
    let mut renderer = VulkanRenderer::new(&window, ImageSource { width: 1, height: 1, pixels: &[255; 4] }, 64, 64).unwrap();
    let source = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
    let atlas = TextureId::for_upload(2, 2, &source).unwrap();
    renderer.create_texture(atlas, &source).unwrap();
    let first = TextureId::for_render_target(2, 2).unwrap();
    renderer.create_render_texture(first).unwrap();
    let info = FrameInfo::try_new(Dimensioni::new(2, 2), color(0, 0, 0, 255)).unwrap();
    let mut frame = renderer.texture_frame(info, first, atlas).unwrap();
    frame.push_quad(
        [(0., 0., 0., 0.), (2., 0., 1., 0.), (2., 2., 1., 1.), (0., 2., 0., 1.)]
            .map(|(x, y, u, v)| Vertex::new(Vec2f::new(x, y), Vec2f::new(u, v), color4b(255, 255, 255, 255))),
    );
    frame.finish().unwrap();
    assert_eq!(pixels(&renderer, first), source);
    assert_eq!((renderer.context.extent.width, renderer.context.extent.height), (64, 64));
    let next = TextureId::for_render_target(3, 1).unwrap();
    renderer.create_render_texture(next).unwrap();
    renderer
        .texture_frame(FrameInfo::try_new(Dimensioni::new(3, 1), color(17, 31, 63, 255)).unwrap(), next, atlas)
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(pixels(&renderer, next), [17, 31, 63, 255].repeat(3));
    assert_eq!(pixels(&renderer, first), source, "later passes must not mutate earlier snapshots");
    let bad = TextureId::for_render_target(131072, 1).unwrap();
    assert!(renderer.create_render_texture(bad).is_err());
    assert_eq!(pixels(&renderer, first), source);
    // Abandonment before submission has acquired no swapchain image and must remain recoverable.
    drop(
        renderer
            .texture_frame(FrameInfo::try_new(Dimensioni::new(3, 1), color(0, 0, 0, 255)).unwrap(), next, atlas)
            .unwrap(),
    );
    assert!(!renderer.disabled);
    assert_eq!(pixels(&renderer, first), source);
    // A normal window frame still uses its own extent, depth attachment, and atlas binding.
    renderer
        .frame(FrameInfo::try_new(Dimensioni::new(64, 64), color(0, 0, 0, 255)).unwrap())
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(pixels(&renderer, first), source);
    renderer.destroy_texture(atlas);
    renderer.destroy_texture(next);
    renderer.destroy_texture(first);
}
