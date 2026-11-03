//! Browser-executed DOM regressions exported from the wasm library test artifact.
//!
//! The final neocogi-browser-tests cdylib owns the public JavaScript export and
//! single host. scripts/test_browser.py builds it, generates matching bindings,
//! serves the checked-in page and requires actual browser PASS.

use super::*;
use crate::{PlatformObject, WindowObject, identity};
use neocogi_com::{ComPtr, NC_E_INVALID_ARG, com_object};
use neocogi_render_core::abi::*;
use wasm_bindgen::prelude::*;

use neocogi_render_gpu::testing::{derivatives, sampled};

#[com_object(interfaces(IRenderStream))]
/// Browser-owned sealed contributions exercise the same generated COM path as native applications.
struct SurfaceBlocks {
    /// Exclusive owners remain alive through each synchronous image/window submission.
    blocks: Vec<neocogi_com::ComBox<IRenderViewBlockAbi>>,
}
impl IRenderStream for SurfaceBlocks {
    /// Borrows a sealed contribution without transferring its owner to the renderer.
    fn view(&self, _: u64, index: u32) -> Result<neocogi_com::WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        self.blocks.get(index as usize).map(neocogi_com::ComBox::downgrade).ok_or(NC_E_INVALID_ARG)
    }
}

/// Records a regional color clear with depth disabled; no automatic depth command may be required.
fn surface_clear(host: &ComPtr<IRenderHostAbi>, group: &neocogi_com::ComBox<IRenderClientDomainAbi>) -> ComPtr<IRenderStreamAbi> {
    let bounds = Rect { x: 0, y: 0, width: 4, height: 4 };
    let mut frame = host.recording_broker().unwrap().begin_frame().unwrap();
    let resources = group.resources().unwrap();
    let mut session = frame
        .begin_view(RenderViewRoot { client: bounds, target: bounds }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.clear_color(0xff0000ff).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    SurfaceBlocks::from_value(SurfaceBlocks { blocks: vec![block] }).query().unwrap()
}

/// Actual WebGL COM submission distinguishes framebuffer views from exact image targets.
/// Pixel readback verifies that a view's regional clear preserves the rest of the parent frame.
fn browser_surface_targets() {
    use glow::HasContext;
    let size = Size { width: 8, height: 8 };
    let platform = PlatformObject::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let object = platform.window("browser target contract", size).unwrap();
    object.device.window.style().set_css_text("width:8px;height:8px");
    let gl = object.device.access().unwrap().state().gpu.core().dispatch().clone();
    let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(object).query().unwrap();
    let drawable = window.size().unwrap();
    let host = window.graphics_host().unwrap();
    let groups: ComPtr<IRenderClientFactoryAbi> = host.query().unwrap();
    let surfaces: ComPtr<IRenderSurfaceFactoryAbi> = host.query().unwrap();
    assert!(host.query::<IRendererAbi>().is_err());
    let permissions = RenderClientPermissions {
        clear_color: true,
        write_color: true,
        write_resources: true,
        import_images: true,
        ..RenderClientPermissions::none()
    };
    let framebuffer = surfaces
        .create_surface(RenderSurfaceDesc {
            kind: RenderSurfaceKind::Framebuffer,
            size: Size::default(),
        })
        .unwrap();
    assert_eq!(framebuffer.size().unwrap(), drawable);
    assert!(framebuffer.query::<IRendererAbi>().is_err());
    let window_group = groups.create_client(Default::default(), framebuffer.as_com_ref(), permissions).unwrap();
    let image = surfaces.create_surface(RenderSurfaceDesc { kind: RenderSurfaceKind::Image, size }).unwrap();
    let image_group = groups.create_client(Default::default(), image.as_com_ref(), permissions).unwrap();
    let image_renderer: ComPtr<IRendererAbi> = image.query().unwrap();
    let image_stream = surface_clear(&host, &image_group);
    let info = RenderInfo {
        size,
        clear: 0xff00ff00,
        invocation: 1,
        block_count: 1,
    };
    image_renderer.render(info, image_stream.as_com_ref()).map_err(|error| error.status).unwrap();
    let old = image.completed_image().unwrap();
    let other = surfaces.create_surface(RenderSurfaceDesc { kind: RenderSurfaceKind::Image, size }).unwrap();
    assert_eq!(
        other
            .query::<IRendererAbi>()
            .unwrap()
            .render(info, image_stream.as_com_ref())
            .map_err(|error| error.status),
        Err(RENDER_E_OWNER)
    );
    assert!(matches!(other.completed_image(), Err(neocogi_com::NC_E_NOT_FOUND)));
    let renderer: ComPtr<IRendererAbi> = window.query().unwrap();
    assert_eq!(
        renderer
            .render(RenderInfo { size: drawable, ..info }, image_stream.as_com_ref())
            .map_err(|error| error.status),
        Err(RENDER_E_OWNER)
    );
    renderer
        .render(RenderInfo { size: drawable, ..info }, surface_clear(&host, &window_group).as_com_ref())
        .map_err(|error| error.status)
        .unwrap();
    let mut inside = [0u8; 4];
    let mut outside = [0u8; 4];
    // SAFETY: The live window retains this WebGL context and framebuffer; both outputs are initialized
    // RGBA storage, and coordinates are within the size just accepted by the production renderer.
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
        gl.read_pixels(
            1,
            drawable.height - 2,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut inside)),
        );
        gl.read_pixels(
            drawable.width - 1,
            0,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut outside)),
        );
        assert_eq!(gl.get_error(), glow::NO_ERROR);
    }
    assert_eq!(inside, [255, 0, 0, 255]);
    assert_eq!(outside, [0, 255, 0, 255]);
    let empty: ComPtr<IRenderStreamAbi> = SurfaceBlocks::from_value(SurfaceBlocks { blocks: vec![] }).query().unwrap();
    image_renderer
        .render(
            RenderInfo {
                block_count: 0,
                clear: 0xff00ffff,
                ..info
            },
            empty.as_com_ref(),
        )
        .map_err(|error| error.status)
        .unwrap();
    assert_ne!(identity(&old).unwrap(), identity(&image.completed_image().unwrap()).unwrap());
    let resources = window_group.resources().unwrap();
    let imported = host.import_texture(resources.as_com_ref(), old.as_com_ref()).unwrap();
    host.prepare_texture(resources.as_com_ref(), imported.as_com_ref()).unwrap();
}

/// Exercises actual browser WebGL target adaptation and the delegated safe upload boundary.
fn canonical_browser_derivatives() {
    use glow::HasContext;
    use neocogi_render_core::abi::{GpuTextureDesc, GpuTextureFormat};
    use neocogi_render_core::{FrameInfo, RendererBackend, RendererFrame};
    use neocogi_render_core::prelude::{Dimensioni, color};
    let document = web_sys::window().unwrap().document().unwrap();
    let canvas = document.create_element("canvas").unwrap().dyn_into::<HtmlCanvasElement>().unwrap();
    canvas.set_width(8);
    canvas.set_height(8);
    let native = canvas
        .get_context("webgl2")
        .unwrap()
        .unwrap()
        .dyn_into::<web_sys::WebGl2RenderingContext>()
        .unwrap();
    let core = neocogi_render_gpu::backends::webgl::Core::from_canvas(canvas, native.clone()).unwrap();
    let gl = core.dispatch().clone();
    let mut renderer = neocogi_render_gpu::backends::webgl::WebGlCanvasRenderer::new(core, 8, 8).unwrap();
    let size = Size { width: 1, height: 1 };
    for len in [0, 1, 3, 5] {
        assert!(matches!(
            renderer.create_texture(size, Some(&vec![0; len])),
            Err(neocogi_render_core::TextureError::Image { .. })
        ));
    }
    renderer.create_texture(size, Some(&[0; 4])).unwrap();
    let gpu = neocogi_render_gpu::GpuDevice::new(
        renderer.core(),
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
    for case in derivatives::cases() {
        let owner = derivatives::block(&gpu, &resources, &case.source);
        let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
        for offscreen in [false, true] {
            let prepared = renderer.prepare_block(&snapshot, offscreen).unwrap();
            let info = FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap();
            let mut frame = if offscreen {
                let output = renderer.create_render_texture(Size { width: 8, height: 8 }).unwrap();
                renderer.texture_frame(info, output).unwrap()
            } else {
                renderer.frame(info).unwrap()
            };
            frame.execute_block(&prepared);
            let mut pixel = [0; 4];
            // SAFETY: current WebGL context/target remain live; pixel is initialized writable RGBA.
            unsafe {
                // Glow WebGL cannot map a null getParameter result to a tracked resource.
                let target = if offscreen {
                    gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING)
                } else {
                    None
                };
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, target);
                gl.read_pixels(4, 4, 1, 1, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixel)));
                assert_eq!(gl.get_error(), glow::NO_ERROR);
            }
            frame.finish().unwrap();
            assert!(
                (i32::from(pixel[0]) - i32::from(case.red)).abs() <= 1,
                "{} offscreen={offscreen}: {pixel:?}",
                case.name
            );
        }
    }
    // Real ES300 image reads exercise the same guarded compiler artifacts as native GL.
    for case in sampled::load_cases() {
        let owner = sampled::load_block(&gpu, &resources, &case);
        let snapshot = gpu.block_snapshot(owner.as_com_ref()).unwrap();
        let prepared = renderer.prepare_block(&snapshot, true).unwrap();
        let output = renderer.create_render_texture(Size { width: 8, height: 8 }).unwrap();
        let mut frame = renderer
            .texture_frame(FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 0, 0, 255)).unwrap(), output)
            .unwrap();
        frame.execute_block(&prepared);
        let mut actual = [0; 4];
        // SAFETY: The current WebGL framebuffer and initialized RGBA output remain live.
        unsafe {
            let target = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, target);
            gl.read_pixels(4, 4, 1, 1, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut actual)));
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        }
        frame.finish().unwrap();
        let expected: [u8; 4] = if case.outside { [0; 4] } else { [191, 63, 127, 255] };
        assert!(
            actual.into_iter().zip(expected).all(|(a, b)| (i32::from(a) - i32::from(b)).abs() <= 1),
            "{}: {actual:?}",
            case.name
        );
    }
    // Upload and defined-zero storage cross the same actual WebGL fetch boundary.
    let white = sampled::block(
        &gpu,
        &resources,
        Rect { x: 0, y: 0, width: 8, height: 8 },
        Size { width: 1, height: 1 },
        &[255; 4],
    );
    let zero = sampled::zero_storage_block(&gpu, &resources);
    for (owner, expected) in [(white, Some([255u8; 4])), (zero, None)] {
        let prepared = renderer.prepare_block(&gpu.block_snapshot(owner.as_com_ref()).unwrap(), true).unwrap();
        let output = renderer.create_render_texture(Size { width: 8, height: 8 }).unwrap();
        let mut frame = renderer
            .texture_frame(FrameInfo::try_new(Dimensioni::new(8, 8), color(0, 255, 0, 255)).unwrap(), output)
            .unwrap();
        frame.execute_block(&prepared);
        let mut pixels = [0; 8 * 8 * 4];
        // SAFETY: The selected 8x8 RGBA target and initialized destination remain live.
        unsafe {
            let target = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, target);
            gl.read_pixels(0, 0, 8, 8, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixels)));
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        }
        frame.finish().unwrap();
        match expected {
            Some(pixel) => assert!(pixels.as_chunks::<4>().0.iter().all(|value| *value == pixel)),
            None => sampled::assert_zero_storage_pixels(&pixels),
        }
    }
    let image = resources
        .create_texture(
            GpuTextureDesc {
                size: Size { width: 1, height: 1 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            neocogi_render_core::source::initial(None).borrow().unwrap(),
        )
        .unwrap();
    let version = gpu.imported_texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
    renderer.prepare_texture(version.clone()).unwrap();
    lose_test_context(&native);
    assert!(renderer.make_current().is_err(), "actual WebGL loss invalidates the physical owner");
    assert!(renderer.prepare_texture(version).is_err(), "a Ready image cannot bypass terminal context loss");
}

// Synthetic DOM events exercise translation deterministically. Pointer capture itself requires
// trusted browser input, so this fixture stubs only that platform primitive, not the Rust routing.
#[wasm_bindgen(inline_js = r#"
export function lose_test_context(context) {
    const extension = context.getExtension("WEBGL_lose_context");
    if (!extension) throw new Error("WebGL loss extension unavailable");
    extension.loseContext();
}
export function record_panic(text) {
    document.getElementById('result').dataset.panic = text;
}
export function exercise(target, name) {
    if (name === 'compose-start') {
        target.dispatchEvent(new CompositionEvent('compositionstart'));
        target.value = '日本';
        target.dispatchEvent(new InputEvent('input', {isComposing:true}));
        target.dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', isComposing:true}));
    } else if (name === 'compose-end') {
        target.dispatchEvent(new CompositionEvent('compositionend', {data:'日本'}));
        target.dispatchEvent(new InputEvent('input', {data:'日本'}));
    } else if (name === 'paste') {
        target.value = 'é\n pasted';
        target.dispatchEvent(new InputEvent('input', {inputType:'insertFromPaste'}));
    } else if (name === 'tab') {
        target.dispatchEvent(new KeyboardEvent('keydown', {key:'Tab'}));
        target.dispatchEvent(new KeyboardEvent('keyup', {key:'Tab'}));
    } else if (name === 'key') {
        target.dispatchEvent(new KeyboardEvent('keydown', {key:'F'}));
    } else if (name === 'wheel') {
        target.dispatchEvent(new WheelEvent('wheel', {deltaY:0.5, deltaMode:0}));
        target.dispatchEvent(new WheelEvent('wheel', {deltaY:0.5, deltaMode:0}));
    } else if (name === 'down') {
        target.setPointerCapture = () => {};
        target.dispatchEvent(new PointerEvent('pointerdown', {pointerId:7, isPrimary:true, buttons:1, clientX:10, clientY:10}));
    } else if (name === 'chord') {
        target.dispatchEvent(new PointerEvent('pointermove', {pointerId:7, isPrimary:true, buttons:3}));
    } else if (name === 'up') {
        target.dispatchEvent(new PointerEvent('pointerup', {pointerId:7, isPrimary:true, buttons:0}));
        target.dispatchEvent(new PointerEvent('lostpointercapture', {pointerId:7, isPrimary:true}));
    } else if (name === 'cancel') {
        target.dispatchEvent(new PointerEvent('pointercancel', {pointerId:7, isPrimary:true}));
    }
}
"#)]
extern "C" {
    /// Keeps the Rust panic diagnostic in the dumped DOM even when wasm traps without unwinding.
    fn record_panic(text: &str);
    /// Triggers actual WebGL context loss after an image has reached its prepared state.
    fn lose_test_context(context: &web_sys::WebGl2RenderingContext);
    /// Dispatches one deterministic DOM sequence against the actual installed Rust listeners.
    fn exercise(target: &EventTarget, name: &str);
}

/// Runs WebGL target/ownership checks and DOM composition, paste, keyboard, wheel, pointer and teardown assertions.
pub fn run_platform_input_tests() {
    std::panic::set_hook(Box::new(|info| record_panic(&info.to_string())));
    canonical_browser_derivatives();
    browser_surface_targets();
    let document = web_sys::window().unwrap().document().unwrap();
    let canvas = document.create_element("canvas").unwrap().dyn_into::<HtmlCanvasElement>().unwrap();
    canvas.set_width(200);
    canvas.set_height(200);
    canvas.style().set_css_text("width:100px;height:100px");
    document.body().unwrap().append_child(&canvas).unwrap();
    let host = BrowserInputHost::new(&canvas).unwrap();
    canvas.focus().unwrap();
    host.synchronize(&canvas, Some(Rect { x: 10, y: 10, width: 80, height: 20 }));
    assert_eq!(document.active_element(), Some(host.text.clone().into()));
    exercise(host.text.unchecked_ref(), "compose-start");
    assert!(host.drain().is_empty(), "preedit and its Enter key must stay browser-owned");
    exercise(host.text.unchecked_ref(), "compose-end");
    assert!(matches!(host.drain().as_slice(), [PendingInput { event: UiInput::Text, text }] if text == "日本"));
    exercise(host.text.unchecked_ref(), "paste");
    assert!(matches!(host.drain().as_slice(), [PendingInput { event: UiInput::Text, text }] if text == "é\n pasted"));
    exercise(host.text.unchecked_ref(), "key");
    assert!(
        matches!(host.drain().as_slice(), [PendingInput { event: UiInput::Key(event), .. }] if event.key == KeyCode::Character && event.value == u32::from('F'))
    );
    exercise(canvas.unchecked_ref(), "wheel");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput {
                event: UiInput::Scroll(Point { x: 0, y: 1 }),
                ..
            },
            PendingInput {
                event: UiInput::Scroll(Point { x: 0, y: 1 }),
                ..
            }
        ]
    ));

    exercise(canvas.unchecked_ref(), "down");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput { event: UiInput::Pointer(_), .. },
            PendingInput { event: UiInput::Down { .. }, .. }
        ]
    ));
    exercise(canvas.unchecked_ref(), "chord");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput { event: UiInput::Pointer(_), .. },
            PendingInput {
                event: UiInput::Down { buttons: 2, .. },
                ..
            }
        ]
    ));
    exercise(canvas.unchecked_ref(), "up");
    assert!(matches!(
        host.drain().as_slice(),
        [
            PendingInput { event: UiInput::Pointer(_), .. },
            PendingInput {
                event: UiInput::Up { buttons: 1, .. },
                ..
            },
            PendingInput {
                event: UiInput::Up { buttons: 2, .. },
                ..
            }
        ]
    ));
    exercise(canvas.unchecked_ref(), "down");
    host.drain();
    exercise(canvas.unchecked_ref(), "cancel");
    assert!(matches!(host.drain().as_slice(), [PendingInput { event: UiInput::Cancel, .. }]));

    // Holding the queue here verifies teardown removes the actual registrations, not just handles.
    let queue = host.queue.clone();
    let text = host.text.clone();
    drop(host);
    assert!(text.parent_node().is_none());
    exercise(canvas.unchecked_ref(), "key");
    assert!(queue.borrow().events.is_empty());
    canvas.remove();
}
