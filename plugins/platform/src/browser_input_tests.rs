//! Browser-executed DOM regressions exported from the wasm library test artifact.
//!
//! Generate web bindings for `cargo test --lib --no-run --target wasm32-unknown-unknown
//! --features webgl`, then call `run_browser_input_tests()` after the generated module initializes.

use super::*;
use wasm_bindgen::prelude::*;

// Synthetic DOM events exercise translation deterministically. Pointer capture itself requires
// trusted browser input, so this fixture stubs only that platform primitive, not the Rust routing.
#[wasm_bindgen(inline_js = r#"
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
    /// Dispatches one deterministic DOM sequence against the actual installed Rust listeners.
    fn exercise(target: &EventTarget, name: &str);
}

/// Runs DOM composition, paste, keyboard, wheel, pointer, and teardown assertions in a browser.
#[wasm_bindgen]
pub fn run_browser_input_tests() {
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
        matches!(host.drain().as_slice(), [PendingInput { event: UiInput::Key(event), .. }] if event.key == KeyCode::CHARACTER && event.value == u32::from('F'))
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
    static_com_editing_and_webgl();
}

/// Registers both assembly descriptors in one linked runtime, then exercises real DOM input,
/// inherited editing, committed IME placement and WebGL submission through their COM interfaces.
fn static_com_editing_and_webgl() {
    use neocogi_com::{ComPtr, Runtime, NC_OK};
    let runtime = Runtime::global();
    // SAFETY: Both descriptors and all referenced code/data live in this statically linked wasm.
    unsafe {
        assert_eq!(runtime.register_plugin_descriptor(neocogi_ui_runtime::plugin_descriptor()), NC_OK);
        assert_eq!(runtime.register_plugin_descriptor(crate::plugin_descriptor()), NC_OK);
    }
    assert!(runtime.require_interface_type::<UiRuntimeAbi>(UiRuntimeAbi::CLASS_ID).is_ok());
    assert!(runtime.require_interface_type::<PlatformAbi>(PlatformAbi::CLASS_ID).is_ok());
    let platform: ComPtr<PlatformAbi> = runtime.construct(()).unwrap();
    let assets = neocogi_ui_demo_assets::load_bundle().unwrap();
    let native = platform
        .create_window(
            NcCopyFromBufferOffset::new(0, b"Browser COM regression"),
            Size { width: 640, height: 400 },
            assets.as_com_ref(),
        )
        .unwrap();
    let document = web_sys::window().unwrap().document().unwrap();
    let canvas = document.query_selector("canvas").unwrap().unwrap().dyn_into::<HtmlCanvasElement>().unwrap();
    canvas.style().set_css_text("width:640px;height:400px");
    let renderer: ComPtr<IRendererAbi> = native.query().unwrap();
    let arguments = UiRuntimeAbi::arguments(assets.as_com_ref()).unwrap();
    let ui = neocogi_ui::Ui::from_interface(runtime.construct::<UiRuntimeAbi>(arguments).unwrap()).unwrap();
    let editor = ui.widgets().textbox("").unwrap();
    let window = ui
        .window("Editable COM window", Rect { x: 10, y: 10, width: 400, height: 180 }, &editor)
        .unwrap();
    let input = ui.input_sink().unwrap();
    exercise(canvas.unchecked_ref(), "tab");
    while native.poll(input.as_com_ref()).unwrap() == PollStatus::EVENT {}
    let size = native.size().unwrap();
    ui.update(size).unwrap();
    native.text_input(ui.text_input_area().unwrap()).unwrap();
    let text = document.query_selector("textarea").unwrap().unwrap().dyn_into::<HtmlTextAreaElement>().unwrap();
    assert_eq!(document.active_element(), Some(text.clone().into()));
    exercise(text.unchecked_ref(), "compose-start");
    assert_eq!(native.poll(input.as_com_ref()).unwrap(), PollStatus::IDLE);
    exercise(text.unchecked_ref(), "compose-end");
    while native.poll(input.as_com_ref()).unwrap() == PollStatus::EVENT {}
    assert_eq!(TextInputArea::try_from(ui.text_input_area().unwrap()).unwrap(), TextInputArea::None);
    ui.update(size).unwrap();
    assert_eq!(editor.text().unwrap(), "日本");
    ui.render(size, renderer.as_com_ref()).unwrap();
    // Changing CSS size requires the same explicit update-before-render used on SDL drawables.
    canvas.style().set_property("width", "500px").unwrap();
    let resized = native.size().unwrap();
    assert_ne!(resized, size);
    assert_eq!(ui.render(resized, renderer.as_com_ref()), Err(UI_E_UPDATE_REQUIRED));
    ui.update(resized).unwrap();
    ui.render(resized, renderer.as_com_ref()).unwrap();
    window.close().unwrap();
    ui.close().unwrap();
    drop(renderer);
    drop(native);
    assert!(canvas.parent_node().is_none());
    assert!(text.parent_node().is_none());
}
