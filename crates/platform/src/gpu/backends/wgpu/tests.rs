//! Native GPU checks for fallible texture construction, without an SDL window or surface.

use super::*;

/// Ordered capability lists must never manufacture an unadvertised presentation format.
#[test]
fn presentation_format_chooses_an_advertised_non_srgb_format() {
    use wgpu::TextureFormat::{Bgra8Unorm, Bgra8UnormSrgb, Rgba8Unorm, Rgba8UnormSrgb};
    for (formats, expected) in [
        (vec![Bgra8UnormSrgb, Rgba8Unorm], Rgba8Unorm),
        (vec![Bgra8UnormSrgb, Rgba8Unorm, Bgra8Unorm], Bgra8Unorm),
        (vec![Rgba8Unorm, Bgra8UnormSrgb], Rgba8Unorm),
        (vec![Rgba8UnormSrgb, Bgra8Unorm], Bgra8Unorm),
    ] {
        let selected = surface_format(&formats).unwrap();
        assert_eq!(selected, expected);
        assert!(formats.contains(&selected) && !selected.is_srgb());
    }
    assert!(surface_format(&[]).unwrap_err().contains("no color formats"));
    assert!(surface_format(&[Bgra8UnormSrgb, Rgba8UnormSrgb]).unwrap_err().contains("non-sRGB"));
}

/// Runs the actual WGPU validation layer; requires any native adapter, including a software one.
#[test]
#[ignore = "requires a native WGPU adapter; run cargo test -p neocogi-platform --features wgpu,test-support -- --ignored"]
fn texture_upload_failures_are_returned_without_poisoning_later_uploads() {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    // An outer scope catches any validation error the helper accidentally lets escape. A real
    // existing texture remains owned while each invalid replacement candidate is rejected.
    let outer = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut executor = crate::gpu::backends::wgpu::generic::Executor::new(Core::new(device.clone(), queue.clone()));
    let live = executor
        .image(Size { width: 1, height: 1 }, wgpu::TextureFormat::Rgba8Unorm, Some(&[255; 4]), None, false)
        .unwrap();
    let too_wide = device.limits().max_texture_dimension_2d + 1;
    let dimensions = executor.image(Size { width: too_wide as i32, height: 1 }, wgpu::TextureFormat::Rgba8Unorm, None, None, true);
    assert!(dimensions.err().unwrap().contains("2D limit"));
    let zero = executor.image(Size { width: 0, height: 1 }, wgpu::TextureFormat::Rgba8Unorm, None, None, true);
    assert!(zero.err().unwrap().contains("nonzero"));

    // Deliberately undersized bytes exercise asynchronous-shaped WGPU error scopes rather than
    // just the helper's dimension precheck. This must return Err instead of reporting success.
    let invalid = executor.image(Size { width: 1, height: 1 }, wgpu::TextureFormat::Rgba8Unorm, Some(&[0; 3]), None, false);
    assert!(invalid.is_err(), "malformed native upload must return its scoped error");
    let replacement = executor
        .image(Size { width: 1, height: 1 }, wgpu::TextureFormat::Rgba8Unorm, Some(&[17; 4]), None, false)
        .unwrap();
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

/// Reads the production color-only texture while keeping its independent image and format contract.
fn pixels(renderer: &WgpuRenderer, image: &Image) -> Vec<u8> {
    texture_pixels(
        &renderer.core.device,
        &renderer.core.queue,
        &image._texture,
        Dimensioni::new(image.size.width, image.size.height),
    )
}

/// Real texture passes preserve source orientation and older outputs without touching window extent.
#[test]
#[ignore = "requires an SDL native display and WGPU adapter"]
fn independent_texture_frames_preserve_pixels_and_native_surface_configuration() {
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let mut window = video.window("WGPU surface regression", 64, 64).hidden().build().unwrap();
    let oversized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| WgpuRenderer::new(&window, 32768, 1)));
    assert!(matches!(oversized, Ok(Err(_))), "oversized construction must return Err");
    let mut renderer = WgpuRenderer::new(&window, 64, 64).unwrap();
    let too_large = FrameInfo::try_new(Dimensioni::new(32768, 1), color(0, 0, 0, 255)).unwrap();
    assert!(renderer.frame(too_large).is_err());
    assert_eq!((renderer.presentation.config().width, renderer.presentation.config().height), (64, 64));
    // A destructive native configure error must require actual reconfiguration, even for old extents.
    renderer.configuration_failure = true;
    let changed = FrameInfo::try_new(Dimensioni::new(80, 80), color(0, 0, 0, 255)).unwrap();
    assert!(renderer.frame(changed).is_err());
    assert!(matches!(renderer.presentation, Presentation::NeedsConfiguration { .. }));
    let old = FrameInfo::try_new(Dimensioni::new(64, 64), color(0, 0, 0, 255)).unwrap();
    renderer.frame(old).unwrap().finish().unwrap();
    assert!(matches!(renderer.presentation, Presentation::Ready { .. }));
    // Actual native presentation errors remain inside finish's Result after successful submission.
    renderer.present_failure = true;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| renderer.frame(old).unwrap().finish()));
    assert!(matches!(result, Ok(Err(_))), "present must return its native error without panicking");
    assert!(matches!(renderer.presentation, Presentation::NeedsConfiguration { .. }));
    renderer.frame(old).unwrap().finish().unwrap();
    // Lost/outdated recovery uses the same forced path even when dimensions are unchanged.
    renderer.configuration_failure = true;
    assert!(renderer.reconfigure_surface(64, 64).is_err());
    assert!(matches!(renderer.presentation, Presentation::NeedsConfiguration { .. }));
    renderer.frame(old).unwrap().finish().unwrap();
    let size = Size { width: 1, height: 1 };
    for length in [0, 1, 3, 5] {
        assert!(
            matches!(renderer.create_texture(size,Some(&vec![0;length])),Err(TextureError::Image{source:neocogi_platform::render::ImageValidationError::PixelLength{expected:4,actual}}) if actual==length)
        );
    }
    let empty = renderer.create_texture(size, None).unwrap();
    assert_eq!(pixels(&renderer, &empty), [0; 4]);
    let source = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
    let gpu = crate::gpu::GpuDevice::new(
        renderer.core(),
        neocogi_platform::compiler::CompilerObject::from_value(neocogi_platform::compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderClientPermissions {
                clear_color: true,
                write_color: true,
                write_resources: true,
                import_images: true,
                ..neocogi_platform::render::abi::RenderClientPermissions::none()
            },
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let root = crate::gpu::abi::Rect { x: 0, y: 0, width: 2, height: 2 };
    let block = sampled::block(&gpu, &resources, root, crate::gpu::abi::Size { width: 2, height: 2 }, &source);
    let prepared = renderer.prepare_block(&gpu.block_snapshot(block.as_com_ref()).unwrap(), true).unwrap();
    let output = renderer.create_render_texture(Size { width: 2, height: 2 }).unwrap();
    let info = FrameInfo::try_new(Dimensioni::new(2, 2), color(0, 0, 0, 255)).unwrap();
    let mut frame = renderer.texture_frame(info, output).unwrap();
    frame.execute_block(&prepared);
    let first = frame.finish().unwrap().unwrap();
    assert_eq!(pixels(&renderer, &first), source);
    assert_eq!((renderer.presentation.config().width, renderer.presentation.config().height), (64, 64));
    let output = renderer.create_render_texture(Size { width: 3, height: 1 }).unwrap();
    let next = renderer
        .texture_frame(FrameInfo::try_new(Dimensioni::new(3, 1), color(17, 31, 63, 255)).unwrap(), output)
        .unwrap()
        .finish()
        .unwrap()
        .unwrap();
    assert_eq!(pixels(&renderer, &next), [17, 31, 63, 255].repeat(3));
    assert_eq!(pixels(&renderer, &first), source, "later passes must not mutate earlier snapshots");
    assert!(renderer.create_render_texture(Size { width: 131072, height: 1 }).is_err());
    assert_eq!(pixels(&renderer, &first), source);
    // A normal window frame still uses its own extent, depth attachment, and atlas binding.
    renderer
        .frame(FrameInfo::try_new(Dimensioni::new(64, 64), color(0, 0, 0, 255)).unwrap())
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(pixels(&renderer, &first), source);
    // The same original WGSL/COM blocks provide an independent native derivative reference.
    for case in derivatives::cases() {
        let owner = derivatives::block(&gpu, &resources, &case.source);
        let prepared = renderer.prepare_block(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), true).unwrap();
        let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap();
        let output = renderer.create_render_texture(Size { width: 8, height: 8 }).unwrap();
        let mut frame = renderer.texture_frame(info, output).unwrap();
        frame.execute_block(&prepared);
        let image = frame.finish().unwrap().unwrap();
        let output = pixels(&renderer, &image);
        assert!((i32::from(output[(4 * 8 + 4) * 4]) - i32::from(case.red)).abs() <= 1, "{}", case.name);
    }
    // Exact texel reads use real COM artifacts; GL/Vulkan must preserve explicit zero guards.
    for case in sampled::load_cases() {
        // WGPU's checked WGSL translation may clamp out-of-range coordinates.
        // Its portable contract does not promise the direct GL/Vulkan zero result.
        if case.outside {
            continue;
        }
        let owner = sampled::load_block(&gpu, &resources, &case);
        let prepared = renderer.prepare_block(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), true).unwrap();
        let output = renderer.create_render_texture(Size { width: 8, height: 8 }).unwrap();
        let mut frame = renderer
            .texture_frame(FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap(), output)
            .unwrap();
        frame.execute_block(&prepared);
        let image = frame.finish().unwrap().unwrap();
        let actual = pixels(&renderer, &image);
        let expected: [u8; 4] = if case.outside { [0, 0, 0, 0] } else { [191, 63, 127, 255] };
        assert!(
            actual[(4 * 8 + 4) * 4..(4 * 8 + 4) * 4 + 4]
                .iter()
                .zip(expected)
                .all(|(&a, b)| (i32::from(a) - i32::from(b)).abs() <= 1),
            "{}: {:?}",
            case.name,
            actual
        );
    }
    // Defined None contents and later updates are observed through real vertex/index fetches.
    let owner = sampled::zero_storage_block(&gpu, &resources);
    let prepared = renderer.prepare_block(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), true).unwrap();
    let output = renderer.create_render_texture(Size { width: 8, height: 8 }).unwrap();
    let mut frame = renderer
        .texture_frame(FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 255, 0, 255)).unwrap(), output)
        .unwrap();
    frame.execute_block(&prepared);
    let image = frame.finish().unwrap().unwrap();
    sampled::assert_zero_storage_pixels(&pixels(&renderer, &image));
    let empty = sampled::block_initial(
        &gpu,
        &resources,
        crate::gpu::abi::Rect { x: 0, y: 0, width: 8, height: 8 },
        Size { width: 2, height: 2 },
        None,
    );
    let prepared = renderer.prepare_block(&gpu.block_snapshot(empty.as_com_ref()).unwrap(), true).unwrap();
    let output = renderer.create_render_texture(Size { width: 8, height: 8 }).unwrap();
    let mut frame = renderer
        .texture_frame(FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 255, 0, 255)).unwrap(), output)
        .unwrap();
    frame.execute_block(&prepared);
    let image = frame.finish().unwrap().unwrap();
    assert_eq!(pixels(&renderer, &image), [0; 8 * 8 * 4]);
    // Real native window presentation must survive resize/recreation after private outputs.
    for (width, height) in [(96, 80), (64, 64)] {
        window.set_size(width, height).unwrap();
        renderer
            .frame(FrameInfo::try_new(Dimensioni::new(width as i32, height as i32), color(9, 11, 13, 255)).unwrap())
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(pixels(&renderer, &first), source, "presentation/resize mutated a retained output");
        assert_eq!((renderer.presentation.config().width, renderer.presentation.config().height), (width, height));
    }
}

#[path = "generic_tests.rs"]
mod generic;

use crate::gpu::sampled_fixture as sampled;

use crate::gpu::derivative_fixture as derivatives;
