//! Browser-executed DOM regressions exported from the wasm library test artifact.
//!
//! Generate web bindings for `cargo test --lib --no-run --target wasm32-unknown-unknown
//! --features webgl`, then call `run_platform_input_tests()` after the generated module initializes.

use super::*;
use wasm_bindgen::prelude::*;

#[path = "../../gpu/tests/support/derivatives.rs"]
mod derivatives;

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
    let core = neocogi_render_webgl::Core::from_canvas(canvas, native.clone()).unwrap();
    let gl = core.dispatch().clone();
    let mut renderer = neocogi_render_webgl::WebGlCanvasRenderer::new(core, 8, 8).unwrap();
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
    let domain = gpu.create_ui_client(Default::default()).unwrap();
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
    let version = gpu.ui_texture_snapshot(resources.as_com_ref(), image.as_com_ref()).unwrap();
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
    /// Dispatches one deterministic DOM sequence against the actual installed Rust listeners.
    fn lose_test_context(context: &web_sys::WebGl2RenderingContext);
    fn exercise(target: &EventTarget, name: &str);
}

/// Runs DOM composition, paste, keyboard, wheel, pointer, and teardown assertions in a browser.
#[wasm_bindgen]
pub fn run_platform_input_tests() {
    std::panic::set_hook(Box::new(|info| record_panic(&info.to_string())));
    canonical_browser_derivatives();
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
