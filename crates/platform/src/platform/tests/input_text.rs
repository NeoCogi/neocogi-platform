//! Native SDL text packets cross the real queue and generated COM independently.
use super::*;
use sdl2::{
    event::Event,
    keyboard::{Keycode, Mod, Scancode},
};

/// The first valid packet can be admitted even if the second encounters the count limit.
#[test]
#[ignore = "requires SDL graphics; run with SDL_VIDEODRIVER=offscreen"]
fn native_text_packets_keep_separate_admission_boundaries_through_com() {
    use super::super::input_backlog::MAX_ENTRIES;
    let _session = native_session();
    let platform = PlatformObject::new();
    let native = platform.window("native text packets", SIZE).unwrap();
    let id = native.device.window.id();
    let window = WindowObject::from_value(native).query::<IPlatformWindowAbi>().unwrap();
    let received = Rc::new(RefCell::new(Vec::new()));
    let sink = InputProbe::from_value(InputProbe {
        window: None,
        empty: None,
        received: received.clone(),
        fail: Cell::new(false),
    })
    .query::<IInputSinkAbi>()
    .unwrap();
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    received.borrow_mut().clear();

    let events = sdl2::init().unwrap().event().unwrap();
    for _ in 0..MAX_ENTRIES - 1 {
        events
            .push_event(Event::KeyDown {
                timestamp: 0,
                window_id: id,
                keycode: Some(Keycode::A),
                scancode: Some(Scancode::A),
                keymod: Mod::NOMOD,
                repeat: false,
            })
            .unwrap();
    }
    let first = format!("{}é", "x".repeat(29));
    let second = "続き";
    assert_eq!(first.len(), 31);
    push_sdl_text(id, &first);
    push_sdl_text(id, second);
    for _ in 0..=MAX_ENTRIES {
        assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Event));
    }
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Idle));
    let prefix = std::mem::take(&mut *received.borrow_mut());
    assert_eq!(prefix.len(), MAX_ENTRIES + 1);
    assert_eq!(prefix[MAX_ENTRIES - 1], (UiInput::Text, first.clone()));
    assert_eq!(prefix[MAX_ENTRIES], (UiInput::Cancel, String::new()));

    push_sdl_text(id, &first);
    push_sdl_text(id, second);
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Event));
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Event));
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Idle));
    assert_eq!(received.borrow().as_slice(), [(UiInput::Text, first), (UiInput::Text, second.into())]);
}
