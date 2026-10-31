//! Real Vulkan image, submission, and window-lifetime regression tests.
use super::*;

/// SDL video lifetime is process-global even though each renderer is thread-local.
static NATIVE_SESSION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Read actual native pixels through a completed transfer, preserving the source layout.
fn pixels(renderer: &VulkanRenderer, id: TextureId) -> Vec<u8> {
    crate::generic::tests::readback(
        &renderer.context.core,
        renderer.textures[&id].image.native.as_ref().unwrap(),
        neocogi_render_gpu::abi::Size { width: id.width(), height: id.height() },
    )
}

#[path = "../../gpu/tests/support/sampled.rs"]
mod sampled;

/// Real texture passes preserve source orientation and older outputs without touching window extent.
#[test]
#[ignore = "requires an SDL native display and Vulkan adapter"]
fn independent_texture_frames_preserve_pixels_and_native_surface_configuration() {
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let mut window = video.window("Vulkan surface regression", 64, 64).vulkan().hidden().build().unwrap();
    let mut renderer = VulkanRenderer::new(&window, 64, 64).unwrap();
    let upload = TextureId::for_upload(1, 1, &[255; 4]).unwrap();
    // Admission must use this call's bytes, not the earlier slice that minted the key.
    for length in [0, 1, 3, 5] {
        assert!(matches!(renderer.create_texture(upload, Some(&vec![0; length])),
            Err(TextureError::Image { source: neocogi_render_core::ImageValidationError::PixelLength { expected: 4, actual } }) if actual == length));
        assert!(!renderer.textures.contains_key(&upload));
    }
    renderer.create_texture(upload, Some(&[255; 4])).unwrap();
    assert!(renderer.create_texture(upload, Some(&[0; 4])).is_err());
    assert!(renderer.create_render_texture(upload).is_err());
    renderer.destroy_texture(upload);
    let source = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255];
    let gpu = neocogi_render_gpu::GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu.create_ui_client(Default::default(), None).unwrap();
    let resources = domain.resources().unwrap();
    let size = neocogi_render_gpu::abi::Size { width: 2, height: 2 };
    let owner = sampled::block(
        &gpu,
        &resources,
        neocogi_render_gpu::abi::Rect { x: 0, y: 0, width: 2, height: 2 },
        size,
        &source,
    );
    let prepared = renderer.prepare_block(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), true).unwrap();
    let first = TextureId::for_render_target(2, 2).unwrap();
    renderer.create_render_texture(first).unwrap();
    assert!(renderer.create_render_texture(first).is_err());
    assert!(renderer.create_texture(first, Some(&[0; 16])).is_err());
    let info = FrameInfo::try_new(Dimensioni::new(2, 2), color(0, 0, 0, 255)).unwrap();
    let mut frame = renderer.texture_frame(info, first).unwrap();
    frame.execute_block(&prepared);
    frame.finish().unwrap();
    assert_eq!(pixels(&renderer, first), source);
    assert_eq!((renderer.context.extent.width, renderer.context.extent.height), (64, 64));
    let next = TextureId::for_render_target(3, 1).unwrap();
    renderer.create_render_texture(next).unwrap();
    renderer
        .texture_frame(FrameInfo::try_new(Dimensioni::new(3, 1), color(17, 31, 63, 255)).unwrap(), next)
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
            .texture_frame(FrameInfo::try_new(Dimensioni::new(3, 1), color(0, 0, 0, 255)).unwrap(), next)
            .unwrap(),
    );
    assert!(renderer.ensure_usable().is_ok());
    assert_eq!(pixels(&renderer, first), source);
    // A normal window frame still uses its own extent, depth attachment, and atlas binding.
    renderer
        .frame(FrameInfo::try_new(Dimensioni::new(64, 64), color(0, 0, 0, 255)).unwrap())
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(pixels(&renderer, first), source);
    // The same original WGSL/COM blocks provide an independent native derivative reference.
    let derivative_output = TextureId::for_render_target(8, 8).unwrap();
    renderer.create_render_texture(derivative_output).unwrap();
    for case in derivatives::cases() {
        let owner = derivatives::block(&gpu, &resources, &case.source);
        let prepared = renderer.prepare_block(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), true).unwrap();
        let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap();
        let mut frame = renderer.texture_frame(info, derivative_output).unwrap();
        frame.execute_block(&prepared);
        frame.finish().unwrap();
        let output = pixels(&renderer, derivative_output);
        assert!((i32::from(output[(4 * 8 + 4) * 4]) - i32::from(case.red)).abs() <= 1, "{}", case.name);
    }
    renderer.destroy_texture(derivative_output);
    // Real native window presentation must survive resize/recreation after private outputs.
    for (width, height) in [(96, 80), (64, 64)] {
        window.set_size(width, height).unwrap();
        renderer
            .frame(FrameInfo::try_new(Dimensioni::new(width as i32, height as i32), color(9, 11, 13, 255)).unwrap())
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(pixels(&renderer, first), source, "presentation/resize mutated a retained output");
        assert_eq!((renderer.context.extent.width, renderer.context.extent.height), (width, height));
    }
    renderer.destroy_texture(next);
    renderer.destroy_texture(first);
}

#[path = "../../gpu/tests/support/derivatives.rs"]
mod derivatives;

/// Real partial generations remain destructible, and no native entry reuses them.
/// Injection returns ordinary errors at production stages rather than provoking driver OOM.
#[test]
#[ignore = "requires an SDL native display and Vulkan adapter"]
fn reconstruction_failure_rejects_every_native_entry_and_preserves_finalizer_phase() {
    use neocogi_render_gpu::{abi::*, source::Bytes};
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let window = video.window("Vulkan reconstruction lifecycle", 64, 64).vulkan().hidden().build().unwrap();
    assert!(VulkanRenderer::new(&window, 0, 64).is_err());
    assert!(VulkanRenderer::new(&window, 64, 0).is_err());
    let info = FrameInfo::try_new(Dimensioni::new(64, 64), color(0, 0, 0, 255)).unwrap();
    let mut points = vec![
        RebuildPoint::WaitIdle,
        RebuildPoint::Teardown,
        RebuildPoint::Swapchain,
        RebuildPoint::RenderPass,
        RebuildPoint::Commands,
    ];
    for index in 0..2 {
        points.extend([
            RebuildPoint::PresentSemaphore(index),
            RebuildPoint::View(index),
            RebuildPoint::Depth(index),
            RebuildPoint::Framebuffer(index),
        ]);
    }
    let gpu = neocogi_render_gpu::GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu.create_ui_client(Default::default(), None).unwrap();
    let resources = domain.resources().unwrap();
    let root = Rect { x: 0, y: 0, width: 64, height: 64 };
    let owner = sampled::block(&gpu, &resources, root, Size { width: 1, height: 1 }, &[255; 4]);
    let block = gpu.block_snapshot(owner.as_com_ref()).unwrap();
    let image = resources
        .create_texture(
            GpuTextureDesc {
                size: Size { width: 1, height: 1 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            neocogi_render_core::source::supplied(Bytes::source(vec![255; 4]).as_com_ref())
                .unwrap()
                .borrow()
                .unwrap(),
        )
        .unwrap();
    let snapshot = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
    // Rebuild after an acquire error has no token: successful recovery returns the
    // established transient error, then allows a real acquisition/submission retry.
    {
        let mut renderer = VulkanRenderer::new(&window, 64, 64).unwrap();
        assert!(
            renderer
                .context
                .recover_acquisition(64, 64)
                .err()
                .unwrap()
                .contains("changed during frame acquisition")
        );
        assert_eq!(renderer.context.presentation, PresentationState::Ready);
        renderer.frame(info).unwrap().finish().unwrap();
        // Runtime minimization does not destroy an already complete generation.
        renderer.context.recreate_swapchain(0, 0).unwrap();
        renderer.frame(info).unwrap().finish().unwrap();
        // A real successful post-present reconstruction restores Acquired until finish.
        let mut frame = renderer.frame(info).unwrap();
        let FrameTarget::Window(acquired) = frame.target.as_mut().unwrap() else {
            panic!("window token")
        };
        acquired.suboptimal = true;
        frame.finish().unwrap();
        assert_eq!(renderer.context.presentation, PresentationState::Ready);
    }
    for point in points {
        // The display's ordinary swapchain has at least two images; the prefix
        // cases exercise the successful first owner as well as a later failure.
        for entry in 0..3 {
            let mut renderer = VulkanRenderer::new(&window, 64, 64).unwrap();
            assert!(renderer.context.swapchain_images.len() >= 2);
            let output = TextureId::for_render_target(2, 2).unwrap();
            renderer.create_render_texture(output).unwrap();
            renderer.context.rebuild_failure = Some(point);
            let error = match entry {
                0 => renderer
                    .frame(FrameInfo::try_new(Dimensioni::new(80, 80), color(0, 0, 0, 255)).unwrap())
                    .err()
                    .unwrap()
                    .to_string(),
                1 => renderer.context.recover_acquisition(64, 64).err().unwrap(),
                _ => {
                    let mut frame = renderer.frame(info).unwrap();
                    let FrameTarget::Window(acquired) = frame.target.as_mut().unwrap() else {
                        panic!("window token")
                    };
                    acquired.suboptimal = true;
                    frame.finish().unwrap_err().to_string()
                }
            };
            assert!(error.contains("injected swapchain reconstruction"), "entry={entry} point={point:?}: {error}");
            assert_eq!(renderer.context.presentation, PresentationState::Unavailable);
            assert!(renderer.prepare_block(&block, false).is_err());
            assert!(renderer.prepare_block(&block, true).is_err());
            assert!(renderer.prepare_texture(snapshot.clone()).is_err());
            let before = renderer.context.core.live_allocations();
            // Same-size empty and client retries must reject before acquisition,
            // pipeline work, output creation or indexing incomplete command arrays.
            assert!(renderer.frame(info).is_err());
            assert!(
                renderer
                    .create_texture(TextureId::for_upload(1, 1, &[255; 4]).unwrap(), Some(&[255; 4]))
                    .is_err()
            );
            assert!(renderer.create_render_texture(TextureId::for_render_target(2, 2).unwrap()).is_err());
            assert!(
                renderer
                    .texture_frame(FrameInfo::try_new(Dimensioni::new(2, 2), color(0, 0, 0, 255)).unwrap(), output)
                    .is_err()
            );
            assert_eq!(renderer.context.core.live_allocations(), before);
            renderer.destroy_texture(output);
            let core = renderer.context.core.clone();
            // Normal Drop waits idle and releases the real partial generation.
            drop(renderer);
            assert_eq!(core.live_allocations(), 0, "partial generation leaked at {point:?}, entry={entry}");
        }
    }
}
