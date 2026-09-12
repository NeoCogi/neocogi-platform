//! DOM ownership and translation for the canvas host; widgets never depend on browser objects.

use std::{cell::RefCell, rc::Rc, collections::VecDeque};
use neocogi_ui_contracts::*;
use neocogi_com::{ComRef, NcCopyFromBufferOffset, NcStatus};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{Event, EventTarget, FocusEvent, HtmlCanvasElement, HtmlTextAreaElement, KeyboardEvent, PointerEvent, WheelEvent};
use neocogi_render_support::wheel::{WheelAccumulator, WheelUnit};

/// One pending copied transition owns text only for an actual committed string.
struct PendingInput {
    /// Canonical event used by both the native platform and the UI queue.
    event: UiInput,
    /// Complete UTF-8 commit, empty for every other event.
    text: String,
}

/// State shared only by DOM listeners and the host's frame drain.
#[derive(Default)]
struct InputQueue {
    /// FIFO transitions waiting for the core.
    events: VecDeque<PendingInput>,
    /// One primary pointer emulates the core's mouse; other contacts are ignored.
    pointer: Option<i32>,
    /// DOM button mask used to recover chord changes delivered through pointermove.
    buttons: u16,
    /// Wheel fractions that have not yet accumulated to a physical pixel.
    wheel: WheelAccumulator,
    /// Composition owns the textarea until its final DOM value is available.
    composing: bool,
}

impl InputQueue {
    /// Coalesces adjacent pointer positions without crossing a button, key, text or wheel transition.
    fn push(&mut self, event: UiInput) {
        if matches!(event, UiInput::Pointer(_)) && self.events.back().is_some_and(|previous| matches!(previous.event, UiInput::Pointer(_))) {
            self.events.pop_back();
        }
        self.events.push_back(PendingInput { event, text: String::new() });
    }

    /// Revokes local gesture state and appends an ordered core cancellation.
    fn cancel(&mut self) {
        self.pointer = None;
        self.buttons = 0;
        self.composing = false;
        self.wheel = WheelAccumulator::default();
        self.push(UiInput::Cancel);
    }

    /// Transfers the committed DOM value once; composition updates remain browser-owned.
    fn commit_text(&mut self, text: &HtmlTextAreaElement) {
        if !self.composing {
            let value = text.value();
            text.set_value("");
            if !value.is_empty() {
                self.events.push_back(PendingInput { event: UiInput::Text, text: value });
            }
        }
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
    /// Transparent input anchored inside the focused editor, outside pointer hit testing.
    text: HtmlTextAreaElement,
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
    pub(super) fn new(canvas: &HtmlCanvasElement) -> Result<Self, String> {
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
            text,
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
            host.listen(&target, name, move |event| {
                let Some(pointer) = event.dyn_ref::<PointerEvent>() else { return };
                if !pointer.is_primary() {
                    return;
                }
                if queue.borrow().pointer.is_some_and(|id| id != pointer.pointer_id()) {
                    return;
                }
                let bounds = canvas.get_bounding_client_rect();
                let scale = canvas_scale(&canvas);
                let x = ((pointer.client_x() as f64 - bounds.left()) * scale[0]).round() as i32;
                let y = ((pointer.client_y() as f64 - bounds.top()) * scale[1]).round() as i32;
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
                let mut queue = queue.borrow_mut();
                if queue.pointer == Some(pointer.pointer_id()) {
                    queue.cancel();
                    text.set_value("");
                }
            })?;
        }
        {
            let queue = host.queue.clone();
            let canvas = canvas.clone();
            let target = canvas.clone().into();
            host.listen(&target, "wheel", move |event| {
                let Some(wheel) = event.dyn_ref::<WheelEvent>() else { return };
                let unit = match wheel.delta_mode() {
                    1 => WheelUnit::Line,
                    2 => WheelUnit::Page,
                    _ => WheelUnit::Pixel,
                };
                let mut queue = queue.borrow_mut();
                let delta = queue.wheel.translate(
                    [wheel.delta_x(), wheel.delta_y()],
                    unit,
                    canvas_scale(&canvas),
                    [canvas.width() as f64, canvas.height() as f64],
                );
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
                    if keyboard.is_composing() || keyboard.key_code() == 229 || queue.borrow().composing {
                        return;
                    }
                    if let Some((key, value)) = map_key(&keyboard.key()) {
                        let pressed = name == "keydown";
                        let transition = KeyStroke {
                            key,
                            value,
                            pressed,
                            repeat: pressed && keyboard.repeat(),
                            modifiers: map_modifiers(keyboard),
                        };
                        queue.borrow_mut().push(UiInput::Key(transition));
                        let editable = event.target().is_some_and(|target| target == EventTarget::from(text.clone()));
                        let printable = matches!(key, KeyCode::Character | KeyCode::Space);
                        // Keep the native paste shortcut and printable defaults; navigation/editing
                        // commands belong to the retained editor and must not edit the input buffer.
                        if !(editable && printable) {
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
            host.listen(host.text.clone().unchecked_ref(), name, move |_| {
                let mut queue = queue.borrow_mut();
                match name {
                    "compositionstart" => queue.composing = true,
                    "compositionend" => {
                        queue.composing = false;
                        queue.commit_text(&text);
                    }
                    _ => queue.commit_text(&text),
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
        let pending = self.queue.borrow_mut().events.pop_front();
        let Some(PendingInput { event, text }) = pending else { return Ok(false) };
        destination.input(event.into(), NcCopyFromBufferOffset::new(0, text.as_bytes()))?;
        Ok(true)
    }

    /// Test observations consume the same ordered queue used by COM polling.
    #[cfg(test)]
    fn drain(&self) -> Vec<PendingInput> {
        self.queue.borrow_mut().events.drain(..).collect()
    }

    /// Places native input using the core's committed editor area without stealing external focus.
    pub(super) fn synchronize(&self, canvas: &HtmlCanvasElement, area: Option<Rect>) {
        let active = canvas.owner_document().and_then(|document| document.active_element());
        let on_canvas = active == Some(canvas.clone().into());
        let on_text = active == Some(self.text.clone().into());
        if !on_canvas && !on_text {
            return;
        }
        if let Some(area) = area {
            let bounds = canvas.get_bounding_client_rect();
            let scale = canvas_scale(canvas);
            let style = self.text.style();
            let _ = style.set_property("left", &format!("{}px", bounds.left() + area.x as f64 / scale[0]));
            let _ = style.set_property("top", &format!("{}px", bounds.top() + area.y as f64 / scale[1]));
            let _ = style.set_property("height", &format!("{}px", area.height as f64 / scale[1]));
            if !on_text {
                let _ = self.text.focus();
            }
        } else if on_text {
            self.queue.borrow_mut().composing = false;
            self.text.set_value("");
            let _ = canvas.focus();
        }
    }
}

/// CSS-to-framebuffer scale follows the actual canvas bounds, including nonuniform CSS sizing.
fn canvas_scale(canvas: &HtmlCanvasElement) -> [f64; 2] {
    let bounds = canvas.get_bounding_client_rect();
    [
        canvas.width() as f64 / bounds.width().max(1.0),
        canvas.height() as f64 / bounds.height().max(1.0),
    ]
}

/// Preserves a browser exception's useful message at the host construction boundary.
fn js_error(error: wasm_bindgen::JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}

#[cfg(test)]
#[path = "browser_input_tests.rs"]
mod tests;

/// Maps logical commands without deriving committed text from key events.
fn map_key(key: &str) -> Option<(KeyCode, u32)> {
    match key {
        "Backspace" => Some((KeyCode::Backspace, 0)),
        "Delete" => Some((KeyCode::Delete, 0)),
        "Enter" => Some((KeyCode::Enter, 0)),
        "Escape" | "Esc" => Some((KeyCode::Escape, 0)),
        " " | "Spacebar" => Some((KeyCode::Space, 0)),
        "Tab" => Some((KeyCode::Tab, 0)),
        "Insert" => Some((KeyCode::Insert, 0)),
        "Home" => Some((KeyCode::Home, 0)),
        "End" => Some((KeyCode::End, 0)),
        "PageUp" => Some((KeyCode::PageUp, 0)),
        "PageDown" => Some((KeyCode::PageDown, 0)),
        "ArrowUp" => Some((KeyCode::ArrowUp, 0)),
        "ArrowDown" => Some((KeyCode::ArrowDown, 0)),
        "ArrowLeft" => Some((KeyCode::ArrowLeft, 0)),
        "ArrowRight" => Some((KeyCode::ArrowRight, 0)),
        "Alt" => Some((KeyCode::Alt, 0)),
        "Control" => Some((KeyCode::Control, 0)),
        "Shift" => Some((KeyCode::Shift, 0)),
        "Meta" | "OS" => Some((KeyCode::Super, 0)),
        _ if key.starts_with('F') && key.len() > 1 => key[1..]
            .parse::<u8>()
            .ok()
            .filter(|number| (1..=24).contains(number))
            .map(|number| (KeyCode::Function, u32::from(number))),
        _ => {
            let mut characters = key.chars();
            let character = characters.next()?;
            characters.next().is_none().then_some((KeyCode::Character, u32::from(character)))
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
