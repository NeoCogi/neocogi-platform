//! Arithmetic shortcuts cross actual SDL event routing and generated COM polling.
use super::*;
use sdl2::{
    event::Event,
    keyboard::{Keycode, Mod, Scancode},
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

/// Multiple sibling polls preserve the exact accepted prefix and an ordered close during overflow.
#[test]
#[ignore = "requires SDL graphics; run with SDL_VIDEODRIVER=offscreen"]
fn dormant_route_bounds_input_and_preserves_close_through_com() {
    use super::super::input_backlog::MAX_ENTRIES;
    let _session = native_session();
    let platform = PlatformObject::new();
    let a = platform.window("dormant input", Size { width: 8, height: 8 }).unwrap();
    let b = platform.window("input pump", Size { width: 8, height: 8 }).unwrap();
    let a_id = a.device.window.id();
    let b_id = b.device.window.id();
    let a = WindowObject::from_value(a).query::<IPlatformWindowAbi>().unwrap();
    let b = WindowObject::from_value(b).query::<IPlatformWindowAbi>().unwrap();
    let received = Rc::new(RefCell::new(Vec::new()));
    let sink = InputProbe::from_value(InputProbe {
        window: None,
        empty: None,
        received: received.clone(),
        fail: Cell::new(false),
    })
    .query::<IInputSinkAbi>()
    .unwrap();
    drain(&a, &sink);
    drain(&b, &sink);
    received.borrow_mut().clear();
    let events = sdl2::init().unwrap().event().unwrap();
    let down = |window_id, keycode, repeat| Event::KeyDown {
        timestamp: 0,
        window_id,
        keycode: Some(keycode),
        scancode: Scancode::from_keycode(keycode),
        keymod: Mod::NOMOD,
        repeat,
    };
    push_sdl_text(a_id, "before é");
    for batch in 0..3 {
        for slot in 0..1600 {
            let key = if (batch * 1600 + slot) % 2 == 0 { Keycode::A } else { Keycode::B };
            events.push_event(down(a_id, key, false)).unwrap();
        }
        events.push_event(down(b_id, Keycode::Return, false)).unwrap();
        drain(&b, &sink);
        assert_eq!(received.borrow().len(), 1, "the sibling must continue receiving input");
        received.borrow_mut().clear();
    }
    events.push_event(Event::Quit { timestamp: 0 }).unwrap();
    assert_eq!(b.poll(sink.as_com_ref()), Ok(PollStatus::Close));
    for _ in 0..MAX_ENTRIES {
        assert_eq!(a.poll(sink.as_com_ref()), Ok(PollStatus::Event));
    }
    let cancel = InputProbe::from_value(InputProbe {
        window: Some(a.clone()),
        empty: Some(sink.clone()),
        received: received.clone(),
        fail: Cell::new(true),
    })
    .query::<IInputSinkAbi>()
    .unwrap();
    assert_eq!(a.poll(cancel.as_com_ref()), Err(NC_E_INVALID_ARG), "the failed cancellation is consumed");
    assert_eq!(a.poll(sink.as_com_ref()), Ok(PollStatus::Close));
    assert_eq!(a.poll(sink.as_com_ref()), Ok(PollStatus::Idle));
    {
        let received = received.borrow();
        assert_eq!(received.len(), MAX_ENTRIES + 1);
        assert_eq!(received[0], (UiInput::Text, "before é".to_owned()));
        for (index, (event, _)) in received[1..MAX_ENTRIES].iter().enumerate() {
            let key = KeyCode::Character(u32::from(if index % 2 == 0 { 'a' } else { 'b' }));
            assert_eq!(
                *event,
                UiInput::Key(KeyStroke {
                    key,
                    action: KeyAction::Pressed { repeat: false },
                    modifiers: 0
                })
            );
        }
        assert_eq!(received[MAX_ENTRIES].0, UiInput::Cancel);
    }
    received.borrow_mut().clear();
    for (keycode, repeat) in [(Keycode::A, true), (Keycode::B, false), (Keycode::A, true), (Keycode::B, true)] {
        events.push_event(down(a_id, keycode, repeat)).unwrap();
    }
    push_sdl_text(a_id, "after 🦀");
    drain(&a, &sink);
    let after = std::mem::take(&mut *received.borrow_mut());
    assert_eq!(after.len(), 3);
    assert_eq!(after[2], (UiInput::Text, "after 🦀".to_owned()));
    for ((event, _), repeat) in after.iter().zip([false, true]) {
        assert_eq!(
            *event,
            UiInput::Key(KeyStroke {
                key: KeyCode::Character(u32::from('b')),
                action: KeyAction::Pressed { repeat },
                modifiers: 0
            })
        );
    }
    // The two physical Enter keys share their logical value. A stale keypad repeat
    // cannot borrow the fresh main Enter, which itself must retain ordinary repeats.
    for (code, repeat) in [(Keycode::Return, false), (Keycode::KpEnter, true), (Keycode::Return, true)] {
        events.push_event(down(a_id, code, repeat)).unwrap();
    }
    events
        .push_event(Event::KeyUp {
            timestamp: 0,
            window_id: a_id,
            keycode: None,
            scancode: Some(Scancode::Return),
            keymod: Mod::NOMOD,
            repeat: false,
        })
        .unwrap();
    events.push_event(down(a_id, Keycode::Return, true)).unwrap();
    drain(&a, &sink);
    assert_eq!(
        received.borrow().as_slice(),
        [false, true].map(|repeat| (
            UiInput::Key(KeyStroke {
                key: KeyCode::Enter,
                action: KeyAction::Pressed { repeat },
                modifiers: 0
            }),
            String::new()
        ))
    );
}
