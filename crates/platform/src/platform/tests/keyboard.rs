//! Arithmetic shortcuts cross actual SDL event routing and generated COM polling.
use super::*;
use sdl2::{
    event::Event,
    keyboard::{Keycode, Mod},
};

/// Drains bounded fixture input through public polling, rejecting unexpected window closure.
fn drain(window: &ComPtr<IPlatformWindowAbi>, sink: &ComPtr<IInputSinkAbi>) {
    for _ in 0..64 {
        match window.poll(sink.as_com_ref()).unwrap() {
            PollStatus::Event => (),
            PollStatus::Idle => return,
            other => panic!("shortcut fixture returned unexpected poll status {other:?}"),
        }
    }
    panic!("shortcut fixture did not become idle");
}

/// Every operator alias and Enter preserve three actions, all modifier combinations and empty text.
/// Both native modifier sides normalize to the same snapshot through real SDL queue storage.
#[test]
#[ignore = "requires SDL graphics; run with SDL_VIDEODRIVER=offscreen"]
fn keypad_shortcuts_preserve_actions_modifiers_and_text_through_com() {
    let _session = native_session();
    let native = PlatformObject::new().window("keypad shortcut contract", Size { width: 8, height: 8 }).unwrap();
    let id = native.device.window.id();
    let events = sdl2::init().unwrap().event().unwrap();
    let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(native).query().unwrap();
    let received = Rc::new(RefCell::new(Vec::new()));
    let sink: ComPtr<IInputSinkAbi> = InputProbe::from_value(InputProbe {
        window: None,
        empty: None,
        received: received.clone(),
        fail: Cell::new(false),
    })
    .query()
    .unwrap();
    drain(&window, &sink);
    received.borrow_mut().clear();

    for flags in [
        [Mod::LALTMOD, Mod::LCTRLMOD, Mod::LSHIFTMOD, Mod::LGUIMOD],
        [Mod::RALTMOD, Mod::RCTRLMOD, Mod::RSHIFTMOD, Mod::RGUIMOD],
    ] {
        for modifiers in 0u8..16 {
            let keymod = flags
                .into_iter()
                .zip([1, 2, 4, 8])
                .fold(Mod::NOMOD, |value, (flag, bit)| if modifiers & bit != 0 { value | flag } else { value });
            for (keycode, key) in [
                (Keycode::KpPlus, KeyCode::Character(u32::from('+'))),
                (Keycode::Plus, KeyCode::Character(u32::from('+'))),
                (Keycode::KpMinus, KeyCode::Character(u32::from('-'))),
                (Keycode::Minus, KeyCode::Character(u32::from('-'))),
                (Keycode::KpMultiply, KeyCode::Character(u32::from('*'))),
                (Keycode::Asterisk, KeyCode::Character(u32::from('*'))),
                (Keycode::KpDivide, KeyCode::Character(u32::from('/'))),
                (Keycode::Slash, KeyCode::Character(u32::from('/'))),
                (Keycode::KpEnter, KeyCode::Enter),
                (Keycode::Return, KeyCode::Enter),
            ] {
                let actions = [KeyAction::Pressed { repeat: false }, KeyAction::Pressed { repeat: true }, KeyAction::Released];
                for action in actions {
                    let event = match action {
                        KeyAction::Pressed { repeat } => Event::KeyDown {
                            timestamp: 0,
                            window_id: id,
                            keycode: Some(keycode),
                            scancode: None,
                            keymod,
                            repeat,
                        },
                        KeyAction::Released => Event::KeyUp {
                            timestamp: 0,
                            window_id: id,
                            keycode: Some(keycode),
                            scancode: None,
                            keymod,
                            repeat: false,
                        },
                    };
                    events.push_event(event).unwrap();
                }
                drain(&window, &sink);
                let expected = actions.map(|action| (UiInput::Key(KeyStroke { key, action, modifiers }), String::new()));
                assert_eq!(received.borrow().as_slice(), expected, "{keycode:?}, native modifiers {keymod:?}");
                received.borrow_mut().clear();
            }
        }
    }
    drop(sink);
    assert_eq!(Rc::strong_count(&received), 1, "polling does not retain the borrowed sink");
}
