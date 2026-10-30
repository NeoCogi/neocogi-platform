//! Native GPU checks for fallible texture construction, without an SDL window or surface.

use super::*;

/// Runs the actual WGPU validation layer; requires any native adapter, including a software one.
#[test]
#[ignore = "requires a native WGPU adapter; run cargo test -p neocogi-render-wgpu -- --ignored"]
fn texture_upload_failures_are_returned_without_poisoning_later_uploads() {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let (_pipeline, _texture_pipeline, layout, sampler) = WgpuRenderer::create_pipeline(&device, wgpu::TextureFormat::Rgba8Unorm);
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("texture regression uniforms"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: false,
    });

    // An outer scope catches any validation error the helper accidentally lets escape. A real
    // existing texture remains owned while each invalid replacement candidate is rejected.
    let outer = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let live = WgpuRenderer::create_gpu_texture(&device, &queue, &layout, &sampler, &uniforms, 1, 1, Some(&[255; 4])).unwrap();
    let too_wide = device.limits().max_texture_dimension_2d + 1;
    let dimensions = WgpuRenderer::create_gpu_texture(&device, &queue, &layout, &sampler, &uniforms, too_wide, 1, None);
    assert!(dimensions.err().unwrap().contains("dimension limit"));
    let zero = WgpuRenderer::create_gpu_texture(&device, &queue, &layout, &sampler, &uniforms, 0, 1, None);
    assert!(zero.err().unwrap().contains("non-zero"));

    // Deliberately undersized bytes exercise asynchronous-shaped WGPU error scopes rather than
    // just the helper's dimension precheck. This must return Err instead of reporting success.
    let invalid = WgpuRenderer::create_gpu_texture(&device, &queue, &layout, &sampler, &uniforms, 1, 1, Some(&[0; 3]));
    assert!(invalid.err().unwrap().contains("texture creation or upload failed"));
    let replacement = WgpuRenderer::create_gpu_texture(&device, &queue, &layout, &sampler, &uniforms, 1, 1, Some(&[17; 4])).unwrap();
    assert!(pollster::block_on(outer.pop()).is_none(), "helper scopes must own their diagnostics");
    drop((replacement, live));
}

/// Reads a completed tiny RGBA target through the actual queue, preserving row orientation.
fn texture_pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, size: Dimensioni) -> Vec<u8> {
    let stride = (size.width as u32 * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("surface readback"),
        size: u64::from(stride) * size.height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size.height as u32),
            },
        },
        wgpu::Extent3d {
            width: size.width as u32,
            height: size.height as u32,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    let (send, receive) = std::sync::mpsc::channel();
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| send.send(result).unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receive.recv().unwrap().unwrap();
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    let result = mapped
        .chunks(stride as usize)
        .flat_map(|row| row[..size.width as usize * 4].iter().copied())
        .collect();
    drop(mapped);
    buffer.unmap();
    result
}

/// Reads the production UI-only texture while keeping its independent atlas and format contract.
fn pixels(renderer: &WgpuRenderer, id: TextureId) -> Vec<u8> {
    texture_pixels(&renderer.device, &renderer.queue, &renderer.textures[&id]._texture, id.size())
}

/// Real texture passes preserve source orientation and older outputs without touching window extent.
#[test]
#[ignore = "requires an SDL native display and WGPU adapter"]
fn independent_texture_frames_preserve_pixels_and_native_surface_configuration() {
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let window = video.window("WGPU surface regression", 64, 64).hidden().build().unwrap();
    let mut renderer = WgpuRenderer::new(&window, ImageSource { width: 1, height: 1, pixels: &[255; 4] }, 64, 64).unwrap();
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
    assert_eq!((renderer.config.width, renderer.config.height), (64, 64));
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

#[path = "generic_tests.rs"]
mod generic;
