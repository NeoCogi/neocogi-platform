//! SDL translation owns no UI references. Delivery occurs after every native state borrow ends.
use super::*;
use neocogi_render_support::wheel::{WheelAccumulator, WheelUnit};
use sdl2::{
    event::{Event, WindowEvent},
    keyboard::{Keycode, Mod},
    mouse::{MouseButton, MouseWheelDirection},
};

/// One native transition, with owned text only for an actual SDL text commit.
enum Transition {
    /// A copied input event plus its complete UTF-8 argument.
    Input(UiInput, String),
    /// Resize, exposure or another relevant native window change.
    Changed,
    /// Native close or process-wide quit request.
    Close,
    /// No UI-visible effect.
    Ignore,
}

#[derive(Default)]
/// Fractional physical wheel movement retained across consecutive native events.
pub(super) struct InputState {
    /// Fractions accumulate independently on each axis until a whole pixel is available.
    wheel: WheelAccumulator,
}

impl InputState {
    /// Converts one event without invoking foreign code or accessing the SDL event pump.
    fn translate(&mut self, event: Event, logical: (u32, u32), drawable: Size) -> Transition {
        let input = match event {
            Event::Quit { .. } | Event::Window { win_event: WindowEvent::Close, .. } => return Transition::Close,
            Event::Window { win_event: WindowEvent::FocusLost, .. } => {
                self.wheel = WheelAccumulator::default();
                UiInput::Cancel
            }
            Event::Window { .. } => return Transition::Changed,
            Event::MouseMotion { x, y, .. } => UiInput::Pointer(pointer(x, y, logical, drawable)),
            Event::MouseButtonDown { mouse_btn, x, y, .. } => {
                let Some(buttons) = button(mouse_btn) else { return Transition::Ignore };
                UiInput::Down {
                    position: pointer(x, y, logical, drawable),
                    buttons,
                }
            }
            Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                let Some(buttons) = button(mouse_btn) else { return Transition::Ignore };
                UiInput::Up {
                    position: pointer(x, y, logical, drawable),
                    buttons,
                }
            }
            Event::MouseWheel { precise_x, precise_y, direction, .. } => {
                // SDL reports positive up/left; the UI uses positive down/right. Honor natural
                // scrolling and retain subpixel trackpad motion instead of rounding each event.
                let direction = if direction == MouseWheelDirection::Flipped { 1.0 } else { -1.0 };
                let delta = self.wheel.translate(
                    [f64::from(precise_x) * direction, f64::from(precise_y) * direction],
                    WheelUnit::Line,
                    [
                        f64::from(drawable.width) / f64::from(logical.0.max(1)),
                        f64::from(drawable.height) / f64::from(logical.1.max(1)),
                    ],
                    [f64::from(drawable.width), f64::from(drawable.height)],
                );
                if delta == [0, 0] {
                    return Transition::Ignore;
                }
                UiInput::Scroll(Point { x: delta[0], y: delta[1] })
            }
            Event::KeyDown { keycode: Some(code), keymod, repeat, .. } => {
                let Some((key, value)) = key(code) else { return Transition::Ignore };
                UiInput::Key(KeyStroke {
                    key,
                    value,
                    pressed: true,
                    repeat,
                    modifiers: modifiers(keymod),
                })
            }
            Event::KeyUp { keycode: Some(code), keymod, .. } => {
                let Some((key, value)) = key(code) else { return Transition::Ignore };
                UiInput::Key(KeyStroke {
                    key,
                    value,
                    pressed: false,
                    repeat: false,
                    modifiers: modifiers(keymod),
                })
            }
            Event::TextInput { text, .. } => return Transition::Input(UiInput::Text, text),
            // TextEditing is a composition preview, never committed editor content. SDL owns it
            // until TextInput; forwarding both would duplicate or prematurely publish IME text.
            _ => return Transition::Ignore,
        };
        Transition::Input(input, String::new())
    }
}

/// Applies independent drawable scaling with wide intermediate arithmetic and bounded output.
fn pointer(x: i32, y: i32, logical: (u32, u32), drawable: Size) -> Point {
    Point {
        x: scale(x, drawable.width as u32, logical.0),
        y: scale(y, drawable.height as u32, logical.1),
    }
}

/// Scales signed coordinates without overflow, including captured drags beyond the window edges.
fn scale(value: i32, numerator: u32, denominator: u32) -> i32 {
    (i64::from(value) * i64::from(numerator) / i64::from(denominator.max(1))).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Maps only the three supported mouse buttons; unsupported native buttons do not synthesize input.
fn button(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Middle => Some(4),
        _ => None,
    }
}

/// Preserves logical shortcut identity without deriving committed text from key presses.
fn key(code: Keycode) -> Option<(KeyCode, u32)> {
    let key = match code {
        Keycode::Backspace => KeyCode::Backspace,
        Keycode::Delete => KeyCode::Delete,
        Keycode::Return | Keycode::KpEnter => KeyCode::Enter,
        Keycode::Escape => KeyCode::Escape,
        Keycode::Space => KeyCode::Space,
        Keycode::Tab => KeyCode::Tab,
        Keycode::Insert => KeyCode::Insert,
        Keycode::Home => KeyCode::Home,
        Keycode::End => KeyCode::End,
        Keycode::PageUp => KeyCode::PageUp,
        Keycode::PageDown => KeyCode::PageDown,
        Keycode::Up => KeyCode::ArrowUp,
        Keycode::Down => KeyCode::ArrowDown,
        Keycode::Left => KeyCode::ArrowLeft,
        Keycode::Right => KeyCode::ArrowRight,
        Keycode::LAlt | Keycode::RAlt => KeyCode::Alt,
        Keycode::LCtrl | Keycode::RCtrl => KeyCode::Control,
        Keycode::LShift | Keycode::RShift => KeyCode::Shift,
        Keycode::LGui | Keycode::RGui => KeyCode::Super,
        _ => {
            // SDL's two function-key ranges are individually contiguous; keep the hole between
            // F12 and F13 out of the public logical key numbering.
            let raw = code.into_i32();
            for (first, last, offset) in [(Keycode::F1, Keycode::F12, 1), (Keycode::F13, Keycode::F24, 13)] {
                if (first.into_i32()..=last.into_i32()).contains(&raw) {
                    return Some((KeyCode::Function, (raw - first.into_i32() + offset) as u32));
                }
            }
            return char::from_u32(raw as u32)
                .filter(|c| !c.is_control())
                .map(|c| (KeyCode::Character, u32::from(c)));
        }
    };
    Some((key, 0))
}

/// Copies the full modifier snapshot; left/right distinctions stay inside SDL.
fn modifiers(value: Mod) -> u8 {
    u8::from(value.intersects(Mod::LALTMOD | Mod::RALTMOD))
        | u8::from(value.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD)) << 1
        | u8::from(value.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD)) << 2
        | u8::from(value.intersects(Mod::LGUIMOD | Mod::RGUIMOD)) << 3
}

impl IPlatformWindow for WindowObject {
    /// Returns only the separate delegable clipboard identity.
    fn clipboard(&self) -> Result<ComPtr<ITextClipboardAbi>, NcStatus> {
        clipboard::Clipboard::for_window(&self.device.window)
    }

    /// Reports drawable pixels, which are also the coordinates used by every delivered pointer.
    fn size(&self) -> Result<Size, NcStatus> {
        Ok(gpu::pixel_size(&self.device.window))
    }

    /// Removes only the attempted event; foreign delivery cannot retain any SDL or queue borrow.
    fn poll(&self, destination: ComRef<'_, IInputSinkAbi>) -> Result<PollStatus, NcStatus> {
        if self.polling.replace(true) {
            return Err(PLATFORM_E_BUSY);
        }
        let _polling = Polling { active: &self.polling };
        while let Some(event) = self.device.host.poll(self.device.window.id())? {
            let transition = self.input.borrow_mut().translate(event, self.device.window.size(), self.size()?);
            match transition {
                Transition::Ignore => continue,
                Transition::Changed => return Ok(PollStatus::Event),
                Transition::Close => return Ok(PollStatus::Close),
                Transition::Input(event, text) => {
                    destination.input(event, NcCopyFromBufferOffset::new(0, text.as_bytes()))?;
                    return Ok(PollStatus::Event);
                }
            }
        }
        Ok(PollStatus::Idle)
    }

    /// Converts physical editor placement to logical window pixels and preserves active composition.
    fn text_input(&self, area: TextInputArea) -> Result<(), NcStatus> {
        if matches!(area, TextInputArea::Area(Rect { width, height, .. }) if width < 0 || height < 0) {
            return Err(NC_E_INVALID_ARG);
        }
        // SDL's IME service is process-wide. An unfocused window must not stop or reposition the
        // focused window's composition when applications synchronize several windows each frame.
        if !self.device.window.has_input_focus() {
            return Ok(());
        }
        let ime = self.device.host.video.text_input();
        match area {
            TextInputArea::None => {
                if ime.is_active() {
                    ime.stop();
                }
            }
            TextInputArea::Area(area) => {
                let logical = self.device.window.size();
                let drawable = self.size()?;
                let rect = sdl2::rect::Rect::new(
                    scale(area.x, logical.0, drawable.width as u32),
                    scale(area.y, logical.1, drawable.height as u32),
                    scale(area.width, logical.0, drawable.width as u32).max(1) as u32,
                    scale(area.height, logical.1, drawable.height as u32).max(1) as u32,
                );
                ime.set_rect(rect);
                if !ime.is_active() {
                    ime.start();
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Key identities, complete modifier snapshots and pointer scaling are platform-independent
    /// contracts; exercise their SDL translation without opening a display or constructing a GPU.
    #[test]
    fn logical_keys_buttons_and_scaled_capture_keep_their_complete_values() {
        assert_eq!(key(Keycode::F1), Some((KeyCode::Function, 1)));
        assert_eq!(key(Keycode::F12), Some((KeyCode::Function, 12)));
        assert_eq!(key(Keycode::F13), Some((KeyCode::Function, 13)));
        assert_eq!(key(Keycode::F24), Some((KeyCode::Function, 24)));
        assert_eq!(key(Keycode::A), Some((KeyCode::Character, u32::from('a'))));
        assert_eq!(key(Keycode::KpEnter), Some((KeyCode::Enter, 0)));
        assert_eq!(modifiers(Mod::RALTMOD | Mod::LCTRLMOD | Mod::RSHIFTMOD | Mod::LGUIMOD), 15);
        assert_eq!(button(MouseButton::Left), Some(1));
        assert_eq!(button(MouseButton::Right), Some(2));
        assert_eq!(button(MouseButton::Middle), Some(4));
        assert_eq!(button(MouseButton::X1), None);
        assert_eq!(pointer(-10, 25, (100, 100), Size { width: 200, height: 300 }), Point { x: -20, y: 75 });
        assert_eq!(scale(i32::MAX, u32::MAX, 1), i32::MAX);
        assert_eq!(scale(i32::MIN, u32::MAX, 1), i32::MIN);
    }

    /// Repeated key presses and their releases remain separate from composed Unicode text.
    #[test]
    fn translation_preserves_repeats_releases_text_and_cancellation_order() {
        let mut state = InputState::default();
        let events = [
            Event::KeyDown {
                timestamp: 0,
                window_id: 1,
                keycode: Some(Keycode::A),
                scancode: None,
                keymod: Mod::LCTRLMOD,
                repeat: true,
            },
            Event::KeyUp {
                timestamp: 0,
                window_id: 1,
                keycode: Some(Keycode::A),
                scancode: None,
                keymod: Mod::NOMOD,
                repeat: false,
            },
            Event::TextInput {
                timestamp: 0,
                window_id: 1,
                text: "é☃".to_owned(),
            },
            Event::Window {
                timestamp: 0,
                window_id: 1,
                win_event: WindowEvent::FocusLost,
            },
        ];
        let translated = events.map(|event| {
            let Transition::Input(input, text) = state.translate(event, (100, 100), Size { width: 200, height: 200 }) else {
                panic!("input expected")
            };
            (input, text)
        });
        let UiInput::Key(press) = translated[0].0 else { panic!("press expected") };
        let UiInput::Key(release) = translated[1].0 else { panic!("release expected") };
        assert!(press.pressed && press.repeat && press.modifiers == 2);
        assert!(!release.pressed && !release.repeat && release.modifiers == 0);
        assert_eq!(translated[2], (UiInput::Text, "é☃".to_owned()));
        assert_eq!(translated[3], (UiInput::Cancel, String::new()));
    }

    /// SDL's precise wheel values accumulate, natural direction reverses them, and focus loss
    /// discards the old gesture's remainder instead of scrolling a newly focused editor.
    #[test]
    fn fractional_wheel_direction_and_cancel_use_drawable_pixels() {
        let mut state = InputState::default();
        let wheel = Event::MouseWheel {
            timestamp: 0,
            window_id: 1,
            which: 0,
            x: 0,
            y: 0,
            precise_x: 0.01,
            precise_y: -0.01,
            direction: MouseWheelDirection::Normal,
            mouse_x: 0,
            mouse_y: 0,
        };
        let size = Size { width: 200, height: 200 };
        assert!(matches!(state.translate(wheel.clone(), (100, 100), size), Transition::Ignore));
        let Transition::Input(UiInput::Scroll(delta), _) = state.translate(wheel.clone(), (100, 100), size) else {
            panic!("accumulated wheel expected")
        };
        assert_eq!(delta, Point { x: -1, y: 1 });
        state.translate(
            Event::Window {
                timestamp: 0,
                window_id: 1,
                win_event: WindowEvent::FocusLost,
            },
            (100, 100),
            size,
        );
        let mut flipped = wheel;
        if let Event::MouseWheel { direction, precise_x, precise_y, .. } = &mut flipped {
            *direction = MouseWheelDirection::Flipped;
            *precise_x = 1.0;
            *precise_y = -1.0;
        }
        let Transition::Input(UiInput::Scroll(delta), _) = state.translate(flipped, (100, 100), size) else {
            panic!("flipped wheel expected")
        };
        assert_eq!(delta, Point { x: 60, y: -60 });
    }
}
