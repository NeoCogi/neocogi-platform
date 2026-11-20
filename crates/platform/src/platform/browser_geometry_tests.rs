//! Browser layout and drawable coordinates observed through the public COM window.
use super::*;

#[wasm_bindgen(inline_js = r#"
export function override_device_scale(scale) {
    const previous = Object.getOwnPropertyDescriptor(window, 'devicePixelRatio');
    Object.defineProperty(window, 'devicePixelRatio', {configurable:true, value:scale});
    return previous;
}
export function restore_device_scale(previous) {
    if (previous) Object.defineProperty(window, 'devicePixelRatio', previous);
    else delete window.devicePixelRatio;
}
export function geometry_pointer(target, name, clientX, clientY, buttons) {
    // Only trusted browser events can request actual pointer capture. Stub the native
    // primitive while exercising the production capture and signed position routing.
    target.setPointerCapture = () => {};
    target.dispatchEvent(new PointerEvent(name, {isPrimary:true, pointerId:5, clientX, clientY, buttons}));
}
export function geometry_wheel(target, deltaMode, deltaX, deltaY) {
    target.dispatchEvent(new WheelEvent('wheel', {deltaMode, deltaX, deltaY}));
}
"#)]
extern "C" {
    /// Sets the reported DPR and returns its prior own-property descriptor.
    fn override_device_scale(scale: f64) -> JsValue;
    /// Restores the ambient DPR for the following browser tests.
    fn restore_device_scale(previous: &JsValue);
    /// Enters the installed pointer listeners with explicit viewport coordinates.
    fn geometry_pointer(target: &HtmlCanvasElement, name: &str, x: f64, y: f64, buttons: u16);
    /// Enters the installed wheel listener with the requested DOM wheel unit.
    fn geometry_wheel(target: &HtmlCanvasElement, mode: u32, x: f64, y: f64);
}

/// Restores the ambient DPR when the geometry fixture exits.
struct DeviceScale(JsValue);

impl Drop for DeviceScale {
    fn drop(&mut self) {
        restore_device_scale(&self.0);
    }
}

/// Reads the actual applied textarea placement, allowing browser CSS pixel serialization.
fn assert_anchor(text: &HtmlTextAreaElement, expected: [f64; 3]) {
    for (property, expected) in ["left", "top", "height"].into_iter().zip(expected) {
        let value = text.style().get_property_value(property).unwrap();
        let actual = value.strip_suffix("px").unwrap().parse::<f64>().unwrap();
        assert!((actual - expected).abs() < 0.02, "{property}: expected {expected}, observed {actual}");
    }
}

/// Exercises content-box and border-box sizing, viewport transforms, all wheel units, and IME.
pub(super) fn canvas_content_coordinates_through_com() {
    let _device_scale = DeviceScale(override_device_scale(2.0));
    let object = PlatformObject::new()
        .window("canvas content geometry", Size { width: 100, height: 80 })
        .unwrap();
    let canvas = object.device.window.clone();
    let text = object.input.text.clone();
    let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(object).query().unwrap();
    let received = Rc::new(RefCell::new(Vec::new()));
    let sink: ComPtr<IInputSinkAbi> = ShortcutSink::from_value(ShortcutSink { received: received.clone() }).query().unwrap();
    let poll = || {
        while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
        std::mem::take(&mut *received.borrow_mut())
    };

    canvas
        .style()
        .set_css_text("position:fixed;left:100px;top:100px;box-sizing:content-box;width:100px;height:80px;padding:20px;border:10px solid black");
    assert_eq!(window.size().unwrap(), Size { width: 200, height: 160 });
    assert_eq!((canvas.width(), canvas.height()), (200, 160));
    drop(poll());
    geometry_pointer(&canvas, "pointermove", 130.0, 130.0, 0);
    assert_eq!(poll(), [(UiInput::Pointer(Point { x: 0, y: 0 }), String::new())]);
    geometry_pointer(&canvas, "pointermove", 130.5, 130.5, 0);
    geometry_wheel(&canvas, 0, 1.5, -2.0);
    geometry_wheel(&canvas, 1, 1.0, -1.0);
    geometry_wheel(&canvas, 2, 1.0, -1.0);
    assert_eq!(
        poll(),
        [
            (UiInput::Pointer(Point { x: 1, y: 1 }), String::new()),
            (UiInput::Scroll(Point { x: 3, y: -4 }), String::new()),
            (UiInput::Scroll(Point { x: 60, y: -60 }), String::new()),
            (UiInput::Scroll(Point { x: 200, y: -160 }), String::new()),
        ]
    );
    let press = Point { x: 6, y: 8 };
    let outside = Point { x: -10, y: -10 };
    geometry_pointer(&canvas, "pointerdown", 133.0, 134.0, 1);
    assert_eq!(
        poll(),
        [
            (UiInput::Pointer(press), String::new()),
            (UiInput::Down { position: press, buttons: 1 }, String::new())
        ]
    );
    geometry_pointer(&canvas, "pointermove", 125.0, 125.0, 1);
    assert_eq!(poll(), [(UiInput::Pointer(outside), String::new())]);
    geometry_pointer(&canvas, "pointerup", 125.0, 125.0, 0);
    assert_eq!(
        poll(),
        [
            (UiInput::Pointer(outside), String::new()),
            (UiInput::Up { position: outside, buttons: 1 }, String::new())
        ]
    );

    let area = |x, y| TextInputArea::Area(Rect { x, y, width: 1, height: 8 });
    window.text_input(area(0, 0)).unwrap();
    assert_anchor(&text, [130.0, 130.0, 4.0]);
    exercise(text.unchecked_ref(), "compose-start");
    window.text_input(area(20, 16)).unwrap();
    assert_anchor(&text, [140.0, 138.0, 4.0]);
    assert!(poll().is_empty());
    exercise(text.unchecked_ref(), "compose-end");
    assert_eq!(poll(), [(UiInput::Text, "日本".to_owned())]);

    // Change layout before the next window.size(). Input observes the new display box while
    // using the existing physical backing. Width zero still leaves visible border and padding.
    canvas.style().set_property("width", "0px").unwrap();
    geometry_pointer(&canvas, "pointermove", 130.0, 130.0, 0);
    window.text_input(area(0, 0)).unwrap();
    assert!(poll().is_empty());
    assert_anchor(&text, [140.0, 138.0, 4.0]);
    assert_eq!((canvas.width(), canvas.height()), (200, 160));

    let document = canvas.owner_document().unwrap();
    let scroller = document.create_element("div").unwrap().dyn_into::<web_sys::HtmlElement>().unwrap();
    scroller
        .style()
        .set_css_text("position:fixed;left:100px;top:100px;width:300px;height:220px;overflow:auto");
    let spacer = document.create_element("div").unwrap().dyn_into::<web_sys::HtmlElement>().unwrap();
    spacer.style().set_css_text("width:600px;height:600px");
    scroller.append_child(&spacer).unwrap();
    document.body().unwrap().append_child(&scroller).unwrap();
    scroller.append_child(&canvas).unwrap();
    canvas.style().set_css_text(concat!(
        "position:absolute;left:60.5px;top:100.5px;box-sizing:border-box;width:100.5px;height:80.5px;",
        "padding:6.5px 7.5px 8.5px 9.5px;border:solid black;border-width:2px 3px 4px 5px;",
        "transform-origin:0 0;transform:translate(10px,20px) scale(2,0.5)"
    ));
    canvas.focus().unwrap();
    scroller.set_scroll_left(20);
    scroller.set_scroll_top(40);
    drop(poll());

    // Insets yield 75.5 × 59.5 layout pixels, displayed at 151 × 29.75. The viewport
    // content starts at (179.5, 184.75) after offset, scroll, translation, and scaled insets.
    assert_eq!(window.size().unwrap(), Size { width: 302, height: 60 });
    assert_eq!((canvas.width(), canvas.height()), (302, 60));
    geometry_pointer(&canvas, "pointermove", 189.5, 195.25, 0);
    assert_eq!(poll(), [(UiInput::Pointer(Point { x: 20, y: 21 }), String::new())]);
    window.text_input(area(20, 20)).unwrap();
    assert_anchor(&text, [189.5, 184.75 + 20.0 * 29.75 / 60.0, 8.0 * 29.75 / 60.0]);
    scroller.remove();
}
