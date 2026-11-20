//! DOM ownership and translation for the canvas host; widgets never depend on browser objects.

use std::{cell::RefCell, rc::Rc};
use neocogi_platform::render::platform::*;
use neocogi_com_sdk::{ComRef, NcCopyFromBufferOffset, NcStatus};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Event, EventTarget, FocusEvent, HtmlCanvasElement, HtmlTextAreaElement, InputEvent, KeyboardEvent, PointerEvent, WheelEvent};
use neocogi_platform::platform::wheel::{WheelAccumulator, WheelUnit};
use super::{browser::CanvasContent, gpu};
use super::input_backlog::{Backlog, Delivery, KeyRecovery};

/// One FIFO entry, including refusal at the position of a complete oversized commit.
enum PendingInput {
    /// A copied transition delivered through the common input sink ABI.
    Event {
        /// Canonical event used by both the native platform and the UI queue.
        event: UiInput,
        /// Complete bounded UTF-8 commit, empty for every other event.
        text: Box<str>,
    },
    /// The oversized payload was discarded before it could occupy the queue.
    TextLimit,
}

/// An overflow can discard an IME transaction before the browser sends its final event.
#[derive(Clone, Copy, Default, PartialEq)]
enum Composition {
    /// Ordinary text commits can be admitted.
    #[default]
    Idle,
    /// The textarea's value is browser-owned until its composition ends.
    Active,
    /// Ignore the old composition's end and tagged input until a new composition begins.
    Discarding,
}

/// State shared only by DOM listeners and the host's frame drain.
#[derive(Default)]
struct InputQueue {
    /// FIFO transitions waiting for the core.
    events: Backlog<PendingInput>,
    /// One primary pointer emulates the core's mouse; other contacts are ignored.
    pointer: Option<i32>,
    /// DOM button mask used to recover chord changes delivered through pointermove.
    buttons: u16,
    /// Wheel fractions that have not yet accumulated to a physical pixel.
    wheel: WheelAccumulator,
    /// Composition owns the textarea until its final DOM value is available.
    composition: Composition,
    /// Records fresh logical presses only after backlog loss.
    keys: KeyRecovery,
}

impl InputQueue {
    /// Reports admission. Adjacent pointer positions can share one slot within an uninterrupted run.
    fn push(&mut self, event: UiInput) -> bool {
        if self.events.saturated() {
            return false;
        }
        if let UiInput::Key(stroke) = event
            && !self.keys.accepts(stroke)
        {
            return false;
        }
        if matches!(event, UiInput::Pointer(_))
            && let Some(PendingInput::Event { event: tail @ UiInput::Pointer(_), .. }) = self.events.tail_mut()
        {
            *tail = event;
            return true;
        }
        self.events.push(PendingInput::Event { event, text: Box::default() }, 0)
    }

    /// Revokes local gesture state and appends an ordered core cancellation.
    fn cancel(&mut self) {
        if !self.events.push(PendingInput::Event { event: UiInput::Cancel, text: Box::default() }, 0) {
            return;
        }
        self.pointer = None;
        self.buttons = 0;
        self.discard_composition();
        self.wheel = WheelAccumulator::default();
    }

    /// Reset local state before cancellation can reenter the host via the sink.
    fn reset_after_overflow(&mut self) -> Option<i32> {
        self.buttons = 0;
        self.wheel = WheelAccumulator::default();
        self.keys.reset();
        self.discard_composition();
        self.pointer.take()
    }

    /// A late browser completion can follow release of the logical editor focus.
    fn discard_composition(&mut self) {
        if self.composition == Composition::Active {
            self.composition = Composition::Discarding;
        }
    }
}

/// Transfer a complete DOM commit with bounded retained storage and no queue borrow across JS.
fn commit_text(queue: &RefCell<InputQueue>, text: &HtmlTextAreaElement) {
    if queue.borrow().events.saturated() {
        text.set_value("");
        return;
    }
    // A DOM string costs at least one UTF-8 byte per UTF-16 unit. Reject before copying when possible.
    let value = js_sys::Reflect::get(text.as_ref(), &JsValue::from_str("value"))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::JsString>().ok());
    text.set_value("");
    let Some(value) = value else { return };
    if value.length() as usize > MAX_TEXT_INPUT_BYTES {
        queue.borrow_mut().events.push(PendingInput::TextLimit, 0);
        return;
    }
    let value = String::from(value);
    if value.len() > MAX_TEXT_INPUT_BYTES {
        queue.borrow_mut().events.push(PendingInput::TextLimit, 0);
    } else if !value.is_empty() {
        let size = value.len();
        queue.borrow_mut().events.push(PendingInput::Event { event: UiInput::Text, text: value.into_boxed_str() }, size);
    }
}

/// One registered DOM callback, removed before its Rust closure is released.
struct Listener {
    /// DOM object that owns the registration.
    target: EventTarget,
    /// Exact event name needed to remove the registration.
    name: &'static str,
    /// Browser-required closure boundary; its state is owned by the concrete input queue.
    callback: Closure<dyn FnMut(Event)>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        // Removing a listener is infallible for a live EventTarget and matching registration.
        let _ = self
            .target
            .remove_event_listener_with_callback(self.name, self.callback.as_ref().unchecked_ref());
    }
}

/// Owns the canvas listeners and the native text input used for IME and paste.
pub(super) struct BrowserInputHost {
    /// Shared event queue, drained without keeping a RefCell borrow during application updates.
    queue: Rc<RefCell<InputQueue>>,
    /// The actual pointer capture owner.
    canvas: HtmlCanvasElement,
    /// Transparent input anchored inside the focused editor, outside pointer hit testing.
    text: HtmlTextAreaElement,
    /// Physical drawing-buffer lifetime, available while renderer state is detached.
    drawable: Rc<gpu::Core>,
    /// Registrations are removed when the host is dropped, including partial construction failure.
    listeners: Vec<Listener>,
}

impl Drop for BrowserInputHost {
    fn drop(&mut self) {
        // Unregister before removing the focused element so teardown cannot enqueue new input.
        self.listeners.clear();
        self.text.remove();
    }
}

impl BrowserInputHost {
    /// Installs the host's pointer, wheel, keyboard, composition, and focus boundaries.
    pub(super) fn new(canvas: &HtmlCanvasElement, drawable: Rc<gpu::Core>) -> Result<Self, String> {
        let window = web_sys::window().ok_or("browser window is unavailable")?;
        let document = window.document().ok_or("browser document is unavailable")?;
        let text = document
            .create_element("textarea")
            .map_err(js_error)?
            .dyn_into::<HtmlTextAreaElement>()
            .map_err(|_| "created element is not a textarea")?;
        text.set_tab_index(-1);
        text.set_attribute("aria-label", "Canvas text input").map_err(js_error)?;
        text.set_attribute("autocomplete", "off").map_err(js_error)?;
        text.set_attribute("autocapitalize", "off").map_err(js_error)?;
        text.set_attribute("spellcheck", "false").map_err(js_error)?;
        text.style()
            .set_css_text("position:fixed;opacity:0;pointer-events:none;resize:none;padding:0;border:0;width:1px;height:1px;font-size:16px");
        document.body().ok_or("browser body is unavailable")?.append_child(&text).map_err(js_error)?;
        let mut host = Self {
            queue: Rc::new(RefCell::new(InputQueue::default())),
            canvas: canvas.clone(),
            text,
            drawable,
            listeners: Vec::new(),
        };
        canvas.set_tab_index(0);
        canvas.style().set_property("touch-action", "none").map_err(js_error)?;

        // Pointer capture keeps drags alive outside the canvas. Comparing the complete button mask
        // also handles chord transitions, for which browsers do not emit another pointerdown/up.
        for name in ["pointerdown", "pointermove", "pointerup"] {
            let canvas = canvas.clone();
            let target: EventTarget = canvas.clone().into();
            let queue = host.queue.clone();
            let drawable = host.drawable.clone();
            host.listen(&target, name, move |event| {
                let Some(pointer) = event.dyn_ref::<PointerEvent>() else { return };
                if queue.borrow().events.saturated() {
                    return;
                }
                if !pointer.is_primary() {
                    return;
                }
                if queue.borrow().pointer.is_some_and(|id| id != pointer.pointer_id()) {
                    return;
                }
                let Ok(Some(coordinates)) = CanvasCoordinates::observe(&canvas, &drawable) else {
                    return;
                };
                // The pinned stable web-sys getter narrows the browser's viewport doubles to i32.
                let [Some(client_x), Some(client_y)] = ["clientX", "clientY"].map(|property| {
                    js_sys::Reflect::get(pointer.as_ref(), &JsValue::from_str(property))
                        .ok()?
                        .as_f64()
                        .filter(|coordinate| coordinate.is_finite())
                }) else {
                    return;
                };
                let x = ((client_x - coordinates.origin[0]) * coordinates.scale[0]).round() as i32;
                let y = ((client_y - coordinates.origin[1]) * coordinates.scale[1]).round() as i32;
                if name == "pointerdown" {
                    if canvas.set_pointer_capture(pointer.pointer_id()).is_err() {
                        queue.borrow_mut().cancel();
                        return;
                    }
                    queue.borrow_mut().pointer = Some(pointer.pointer_id());
                    let _ = canvas.focus();
                }
                let mut queue = queue.borrow_mut();
                queue.push(UiInput::Pointer(Point { x, y }));
                let buttons = pointer.buttons();
                // Ignore held buttons from a gesture that began elsewhere; it has no local capture.
                if queue.pointer.is_some() {
                    for (mask, button) in [(1, 1), (2, 2), (4, 4)] {
                        if (buttons ^ queue.buttons) & mask != 0 {
                            queue.push(if buttons & mask != 0 {
                                UiInput::Down {
                                    position: Point { x, y },
                                    buttons: button,
                                }
                            } else {
                                UiInput::Up {
                                    position: Point { x, y },
                                    buttons: button,
                                }
                            });
                        }
                    }
                    queue.buttons = buttons;
                    if buttons == 0 {
                        // Clear ownership before the browser's normal lostpointercapture event.
                        queue.pointer = None;
                    }
                }
                event.prevent_default();
            })?;
        }
        for name in ["pointercancel", "lostpointercapture"] {
            let queue = host.queue.clone();
            let text = host.text.clone();
            host.listen(canvas.unchecked_ref(), name, move |event| {
                let Some(pointer) = event.dyn_ref::<PointerEvent>() else { return };
                let captured = {
                    let mut queue = queue.borrow_mut();
                    let captured = queue.pointer == Some(pointer.pointer_id());
                    if captured {
                        queue.cancel();
                    }
                    captured
                };
                if captured {
                    text.set_value("");
                }
            })?;
        }
        {
            let queue = host.queue.clone();
            let canvas = canvas.clone();
            let target = canvas.clone().into();
            let drawable = host.drawable.clone();
            host.listen(&target, "wheel", move |event| {
                let Some(wheel) = event.dyn_ref::<WheelEvent>() else { return };
                let unit = match wheel.delta_mode() {
                    1 => WheelUnit::Line,
                    2 => WheelUnit::Page,
                    _ => WheelUnit::Pixel,
                };
                let Ok(Some(coordinates)) = CanvasCoordinates::observe(&canvas, &drawable) else {
                    return;
                };
                let mut queue = queue.borrow_mut();
                if queue.events.saturated() {
                    return;
                }
                let delta = queue
                    .wheel
                    .translate([wheel.delta_x(), wheel.delta_y()], unit, coordinates.scale, coordinates.extent);
                if delta != [0, 0] {
                    queue.push(UiInput::Scroll(Point { x: delta[0], y: delta[1] }));
                }
                event.prevent_default();
            })?;
        }

        // Commands work on both focus targets. Printable keys are committed only through the DOM
        // textarea, allowing dead keys, IME, paste, and Unicode strings to share one text path.
        for target in [EventTarget::from(canvas.clone()), EventTarget::from(host.text.clone())] {
            for name in ["keydown", "keyup"] {
                let queue = host.queue.clone();
                let text = host.text.clone();
                host.listen(&target, name, move |event| {
                    let Some(keyboard) = event.dyn_ref::<KeyboardEvent>() else { return };
                    if keyboard.is_composing() || keyboard.key_code() == 229 || queue.borrow().composition == Composition::Active {
                        return;
                    }
                    if let Some(key) = map_key(&keyboard.key()) {
                        let pressed = name == "keydown";
                        let transition = KeyStroke {
                            key,
                            action: if pressed { KeyAction::Pressed { repeat: keyboard.repeat() } } else { KeyAction::Released },
                            modifiers: map_modifiers(keyboard),
                        };
                        let admitted = queue.borrow_mut().push(UiInput::Key(transition));
                        let editable = event.target().is_some_and(|target| target == EventTarget::from(text.clone()));
                        let printable = matches!(key, KeyCode::Character(_) | KeyCode::Space);
                        // Clipboard and history shortcuts belong to the retained editor. Suppress
                        // dummy-textarea defaults so asynchronous paste cannot also arrive as input.
                        let editor_command = editable
                            && transition.modifiers & (2 | 8) != 0
                            && transition.modifiers & 1 == 0
                            && matches!(key, KeyCode::Character(scalar) if char::from_u32(scalar).is_some_and(|key| matches!(key.to_ascii_lowercase(), 'a' | 'c' | 'x' | 'v' | 'z' | 'y')));
                        // A rejected repeat must also lose its default printable insertion.
                        if !admitted || editor_command || !(editable && printable) {
                            event.prevent_default();
                        }
                    }
                })?;
            }
            let queue = host.queue.clone();
            let canvas = canvas.clone();
            let text = host.text.clone();
            host.listen(&target, "blur", move |event| {
                let next = event.dyn_ref::<FocusEvent>().and_then(FocusEvent::related_target);
                if next == Some(canvas.clone().into()) || next == Some(text.clone().into()) {
                    return;
                }
                queue.borrow_mut().cancel();
                text.set_value("");
            })?;
        }
        for name in ["compositionstart", "compositionend", "input"] {
            let queue = host.queue.clone();
            let text = host.text.clone();
            host.listen(host.text.clone().unchecked_ref(), name, move |event| {
                match name {
                    "compositionstart" => queue.borrow_mut().composition = Composition::Active,
                    "compositionend" => {
                        let discarded = {
                            let mut queue = queue.borrow_mut();
                            let discarded = queue.composition == Composition::Discarding;
                            if !discarded {
                                queue.composition = Composition::Idle;
                            }
                            discarded
                        };
                        if discarded {
                            text.set_value("");
                        } else {
                            commit_text(&queue, &text);
                        }
                    }
                    _ => {
                        let composition = queue.borrow().composition;
                        let composing_input = event.dyn_ref::<InputEvent>().is_some_and(|input| {
                            input.is_composing() || matches!(input.input_type().as_str(), "insertCompositionText" | "insertFromComposition")
                        });
                        if composition != Composition::Active {
                            if composing_input {
                                text.set_value("");
                            } else {
                                commit_text(&queue, &text);
                            }
                        }
                    }
                }
                // Clearing after commit makes a following final input event observe an empty value.
            })?;
        }
        {
            let queue = host.queue.clone();
            let text = host.text.clone();
            host.listen(window.unchecked_ref(), "blur", move |_| {
                queue.borrow_mut().cancel();
                text.set_value("");
            })?;
        }
        Ok(host)
    }

    /// Adds a browser-required closure and immediately records its teardown ownership.
    fn listen<F: FnMut(Event) + 'static>(&mut self, target: &EventTarget, name: &'static str, callback: F) -> Result<(), String> {
        let callback = Closure::wrap(Box::new(callback) as Box<dyn FnMut(Event)>);
        target
            .add_event_listener_with_callback(name, callback.as_ref().unchecked_ref())
            .map_err(js_error)?;
        self.listeners.push(Listener { target: target.clone(), name, callback });
        Ok(())
    }

    /// Detaches one event before invoking the borrowed destination; failures consume only this event.
    pub(super) fn poll(&self, destination: ComRef<'_, IInputSinkAbi>) -> Result<bool, NcStatus> {
        match self.dequeue() {
            Some(PendingInput::Event { event, text }) => {
                destination.input(event, NcCopyFromBufferOffset::new(0, text.as_bytes()))?;
                Ok(true)
            }
            Some(PendingInput::TextLimit) => Err(PLATFORM_E_INPUT_LIMIT),
            None => Ok(false),
        }
    }

    /// Disarm browser capture and IME state outside queue borrows before delivering cancellation.
    fn dequeue(&self) -> Option<PendingInput> {
        let pointer = {
            let mut queue = self.queue.borrow_mut();
            match queue.events.pop()? {
                Delivery::Entry(entry) => return Some(entry),
                Delivery::Overflow => queue.reset_after_overflow(),
            }
        };
        if let Some(pointer) = pointer {
            let _ = self.canvas.release_pointer_capture(pointer);
        }
        self.text.set_value("");
        Some(PendingInput::Event { event: UiInput::Cancel, text: Box::default() })
    }

    /// Test observations consume the same ordered queue used by COM polling.
    #[cfg(any(test, feature = "test-support"))]
    fn drain(&self) -> Vec<PendingInput> {
        std::iter::from_fn(|| self.dequeue()).collect()
    }

    /// Places native input using the core's committed editor area without stealing external focus.
    pub(super) fn synchronize(&self, canvas: &HtmlCanvasElement, area: Option<Rect>) -> Result<(), NcStatus> {
        let active = canvas.owner_document().and_then(|document| document.active_element());
        let on_canvas = active == Some(canvas.clone().into());
        let on_text = active == Some(self.text.clone().into());
        if !on_canvas && !on_text {
            return Ok(());
        }
        if let Some(area) = area {
            let Some(coordinates) = CanvasCoordinates::observe(canvas, &self.drawable)? else {
                return Ok(());
            };
            let style = self.text.style();
            let _ = style.set_property("left", &format!("{}px", coordinates.origin[0] + f64::from(area.x) / coordinates.scale[0]));
            let _ = style.set_property("top", &format!("{}px", coordinates.origin[1] + f64::from(area.y) / coordinates.scale[1]));
            let _ = style.set_property("height", &format!("{}px", f64::from(area.height) / coordinates.scale[1]));
            if !on_text {
                let _ = self.text.focus();
            }
        } else if on_text {
            self.queue.borrow_mut().discard_composition();
            self.text.set_value("");
            let _ = canvas.focus();
        }
        Ok(())
    }
}

/// One observation pairs the visible CSS content with the actual drawable.
struct CanvasCoordinates {
    /// Viewport CSS origin used for both directions of conversion.
    origin: [f64; 2],
    /// Width and height in drawing-buffer pixels, also used for page scrolling.
    extent: [f64; 2],
    /// Physical pixels per CSS pixel on the two axes.
    scale: [f64; 2],
}

impl CanvasCoordinates {
    /// Reads both coordinate spaces without resizing or acquiring the drawable.
    /// An empty or unavailable content box has no position conversion.
    fn observe(canvas: &HtmlCanvasElement, drawable: &gpu::Core) -> Result<Option<Self>, NcStatus> {
        let size = drawable.drawable_size().map_err(|_| PLATFORM_E_BACKEND)?;
        if size.width <= 0 || size.height <= 0 {
            return Err(PLATFORM_E_BACKEND);
        }
        let extent = [f64::from(size.width), f64::from(size.height)];
        let Some(content) = CanvasContent::measure(canvas).filter(|content| !content.extent.contains(&0.0)) else {
            return Ok(None);
        };
        let scale = [extent[0] / content.extent[0], extent[1] / content.extent[1]];
        Ok(Some(Self { origin: content.origin, extent, scale }))
    }
}

/// Preserves a browser exception's useful message at the host construction boundary.
fn js_error(error: wasm_bindgen::JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}

#[cfg(any(test, feature = "test-support"))]
#[path = "browser_input_tests.rs"]
pub(crate) mod tests;

/// Maps logical commands without deriving committed text from key events.
fn map_key(key: &str) -> Option<KeyCode> {
    match key {
        "Backspace" => Some(KeyCode::Backspace),
        "Delete" => Some(KeyCode::Delete),
        "Enter" => Some(KeyCode::Enter),
        "Escape" | "Esc" => Some(KeyCode::Escape),
        " " | "Spacebar" => Some(KeyCode::Space),
        "Tab" => Some(KeyCode::Tab),
        "Insert" => Some(KeyCode::Insert),
        "Home" => Some(KeyCode::Home),
        "End" => Some(KeyCode::End),
        "PageUp" => Some(KeyCode::PageUp),
        "PageDown" => Some(KeyCode::PageDown),
        "ArrowUp" => Some(KeyCode::ArrowUp),
        "ArrowDown" => Some(KeyCode::ArrowDown),
        "ArrowLeft" => Some(KeyCode::ArrowLeft),
        "ArrowRight" => Some(KeyCode::ArrowRight),
        "Alt" => Some(KeyCode::Alt),
        "Control" => Some(KeyCode::Control),
        "Shift" => Some(KeyCode::Shift),
        "Meta" | "OS" => Some(KeyCode::Super),
        _ if key.starts_with('F') && key.len() > 1 => key[1..].parse::<u8>().ok().filter(|number| (1..=24).contains(number)).map(KeyCode::Function),
        _ => {
            let mut characters = key.chars();
            let character = characters.next()?;
            characters.next().is_none().then_some(KeyCode::Character(u32::from(character)))
        }
    }
}

/// Copies the browser's authoritative modifier snapshot for this transition.
fn map_modifiers(event: &KeyboardEvent) -> u8 {
    let mut modifiers = 0;
    if event.alt_key() {
        modifiers |= 1;
    }
    if event.ctrl_key() {
        modifiers |= 2;
    }
    if event.shift_key() {
        modifiers |= 4;
    }
    if event.meta_key() {
        modifiers |= 8;
    }
    modifiers
}
