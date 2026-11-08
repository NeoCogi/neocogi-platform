//! Real Vulkan image, submission, and window-lifetime regression tests.
use super::*;

/// SDL video lifetime is process-global even though each renderer is thread-local.
static NATIVE_SESSION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A one-shot query override and observation of the actual native dispatch boundary.
struct CompositionProbe {
    /// Consumed only after the real surface-capabilities query succeeds.
    requested: Option<vk::CompositeAlphaFlagsKHR>,
    /// Count actual vkCreateSwapchainKHR attempts, independently of selector results.
    creates: usize,
}

thread_local! {
    /// Initial construction has no published context on which to install fixture state.
    /// Scope it to the current test thread and restore it on all exits, including panic.
    static COMPOSITION: std::cell::RefCell<Option<CompositionProbe>> = const { std::cell::RefCell::new(None) };
}

/// Owns the bounded constructor/rebuild seam; never changes production renderer state.
struct CompositionScope;
impl CompositionScope {
    /// Starts exactly one native query observation under the SDL session lock.
    fn new(requested: vk::CompositeAlphaFlagsKHR) -> Self {
        COMPOSITION.with_borrow_mut(|slot| {
            assert!(slot.is_none(), "composition scopes cannot nest");
            *slot = Some(CompositionProbe { requested: Some(requested), creates: 0 });
        });
        Self
    }

    /// Prove both capability consumption and dispatch; an unrelated error cannot pass.
    fn assert_observed(&self, creates: usize) {
        COMPOSITION.with_borrow(|slot| {
            let probe = slot.as_ref().unwrap();
            assert!(probe.requested.is_none(), "real query did not consume the override");
            assert_eq!(probe.creates, creates);
        });
    }
}
impl Drop for CompositionScope {
    fn drop(&mut self) {
        COMPOSITION.with_borrow_mut(|slot| {
            slot.take();
        });
    }
}

/// Replace only the advertised composition mask, after querying the actual surface.
pub(super) fn composition_capabilities(mut caps: vk::SurfaceCapabilitiesKHR) -> vk::SurfaceCapabilitiesKHR {
    COMPOSITION.with_borrow_mut(|slot| {
        if let Some(requested) = slot.as_mut().and_then(|probe| probe.requested.take()) {
            // Negative controls bypass admission and dispatch OPAQUE. Require real
            // OPAQUE support first so those controls never submit invalid driver usage.
            assert!(
                caps.supported_composite_alpha.contains(vk::CompositeAlphaFlagsKHR::OPAQUE),
                "fixture requires a real OPAQUE-capable surface"
            );
            caps.supported_composite_alpha = requested;
        }
    });
    caps
}

/// Count exactly at the native call, rather than assuming selector acceptance dispatched.
pub(super) fn record_swapchain_create() {
    COMPOSITION.with_borrow_mut(|slot| {
        if let Some(probe) = slot {
            probe.creates += 1;
        }
    });
}

/// Both supported and unsupported initial calls must reach the real query/dispatch boundary.
#[test]
#[ignore = "requires an SDL native display and Vulkan adapter"]
fn initial_window_composition_is_admitted_before_native_creation() {
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let window = video.window("Vulkan initial composition", 64, 64).vulkan().hidden().build().unwrap();
    for supported in [
        vk::CompositeAlphaFlagsKHR::OPAQUE | vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
    ] {
        let scope = CompositionScope::new(supported);
        let candidate = VulkanRenderer::new(&window, 64, 64);
        if supported.contains(vk::CompositeAlphaFlagsKHR::OPAQUE) {
            let mut renderer = candidate.unwrap();
            scope.assert_observed(1);
            renderer
                .frame(FrameInfo::try_new(Dimensioni::new(64, 64), color(0, 0, 0, 255)).unwrap())
                .unwrap()
                .finish()
                .unwrap();
        } else {
            scope.assert_observed(0);
            assert_eq!(candidate.err().unwrap(), "Vulkan surface does not support opaque window composition");
        }
    }
}

/// Destructive rebuild rejection must latch unavailable and forbid another native frame.
#[test]
#[ignore = "requires an SDL native display and Vulkan adapter"]
fn rebuilt_window_composition_is_admitted_before_native_creation() {
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let window = video.window("Vulkan rebuilt composition", 64, 64).vulkan().hidden().build().unwrap();
    for supported in [
        vk::CompositeAlphaFlagsKHR::OPAQUE | vk::CompositeAlphaFlagsKHR::INHERIT,
        vk::CompositeAlphaFlagsKHR::INHERIT,
    ] {
        let mut renderer = VulkanRenderer::new(&window, 64, 64).unwrap();
        let core = renderer.context.core.clone();
        let scope = CompositionScope::new(supported);
        let result = renderer.context.recreate_swapchain(64, 64);
        if supported.contains(vk::CompositeAlphaFlagsKHR::OPAQUE) {
            result.unwrap();
            scope.assert_observed(1);
            assert_eq!(renderer.context.presentation, PresentationState::Ready);
            renderer
                .frame(FrameInfo::try_new(Dimensioni::new(64, 64), color(0, 0, 0, 255)).unwrap())
                .unwrap()
                .finish()
                .unwrap();
        } else {
            scope.assert_observed(0);
            assert_eq!(result, Err("Vulkan surface does not support opaque window composition".to_owned()));
            assert_eq!(renderer.context.presentation, PresentationState::Unavailable);
            assert_eq!(renderer.context.swapchain, vk::SwapchainKHR::null());
            assert!(renderer.ensure_usable().is_err());
            assert!(
                renderer
                    .frame(FrameInfo::try_new(Dimensioni::new(64, 64), color(0, 0, 0, 255)).unwrap())
                    .is_err()
            );
            scope.assert_observed(0);
        }
        drop(renderer);
        assert_eq!(core.live_allocations(), 0);
    }
}

/// Read actual native pixels through a completed transfer, preserving the source layout.
fn pixels(renderer: &VulkanRenderer, image: &Image) -> Vec<u8> {
    crate::gpu::backends::vulkan::generic::tests::readback(
        &renderer.context.core,
        &image.native,
        <Backend as crate::gpu::native::NativeResources>::image_size(image),
    )
}

/// Real texture passes preserve source orientation and older outputs without touching window extent.
#[test]
#[ignore = "requires an SDL native display and Vulkan adapter"]
fn independent_texture_frames_preserve_pixels_and_native_surface_configuration() {
    let _session = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let mut window = video.window("Vulkan surface regression", 64, 64).vulkan().hidden().build().unwrap();
    let mut renderer = VulkanRenderer::new(&window, 64, 64).unwrap();
    for length in [0, 1, 3, 5] {
        assert!(
            matches!(renderer.create_texture(Size{width:1,height:1},Some(&vec![0;length])),Err(TextureError::Image{source:neocogi_platform::render::ImageValidationError::PixelLength{expected:4,actual}}) if actual==length)
        );
    }
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
    let size = crate::gpu::abi::Size { width: 2, height: 2 };
    let owner = sampled::block(&gpu, &resources, crate::gpu::abi::Rect { x: 0, y: 0, width: 2, height: 2 }, size, &source);
    let prepared = renderer.prepare_block(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), true).unwrap();
    let output = renderer.create_render_texture(Size { width: 2, height: 2 }).unwrap();
    let info = FrameInfo::try_new(Dimensioni::new(2, 2), color(0, 0, 0, 255)).unwrap();
    let mut frame = renderer.texture_frame(info, output).unwrap();
    frame.execute_block(&prepared);
    let first = frame.finish().unwrap().unwrap();
    assert_eq!(pixels(&renderer, &first), source);
    assert_eq!((renderer.context.extent.width, renderer.context.extent.height), (64, 64));
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
    // Abandonment before submission has acquired no swapchain image and must remain recoverable.
    let abandoned = renderer.create_render_texture(Size { width: 3, height: 1 }).unwrap();
    drop(
        renderer
            .texture_frame(FrameInfo::try_new(Dimensioni::new(3, 1), color(0, 0, 0, 255)).unwrap(), abandoned)
            .unwrap(),
    );
    assert!(renderer.ensure_usable().is_ok());
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
        assert_eq!((renderer.context.extent.width, renderer.context.extent.height), (width, height));
    }
}

use crate::gpu::derivative_fixture as derivatives;

/// Real partial generations remain destructible, and no native entry reuses them.
/// Injection returns ordinary errors at production stages rather than provoking driver OOM.
#[test]
#[ignore = "requires an SDL native display and Vulkan adapter"]
fn reconstruction_failure_rejects_every_native_entry_and_preserves_finalizer_phase() {
    use crate::gpu::{abi::*, source::Bytes};
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
            let root = Rect { x: 0, y: 0, width: 64, height: 64 };
            let owner = sampled::block(&gpu, &resources, root, Size { width: 1, height: 1 }, &[255; 4]);
            let block = gpu.block_snapshot(owner.as_com_ref()).unwrap();
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
            let snapshot = gpu.texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
            assert!(renderer.context.swapchain_images.len() >= 2);
            let output = renderer.create_render_texture(Size { width: 2, height: 2 }).unwrap();
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
                    frame.finish().err().unwrap().to_string()
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
            assert!(renderer.create_texture(Size { width: 1, height: 1 }, Some(&[255; 4])).is_err());
            assert!(renderer.create_render_texture(Size { width: 2, height: 2 }).is_err());
            assert!(
                renderer
                    .texture_frame(FrameInfo::try_new(Dimensioni::new(2, 2), color(0, 0, 0, 255)).unwrap(), output)
                    .is_err()
            );
            assert_eq!(renderer.context.core.live_allocations(), before);
            let core = renderer.context.core.clone();
            // Normal Drop waits idle and releases the real partial generation.
            drop((renderer, gpu, domain, resources, owner, block, image, snapshot));
            assert_eq!(core.live_allocations(), 0, "partial generation leaked at {point:?}, entry={entry}");
        }
    }
}
