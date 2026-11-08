//! Input-route ownership ends independently of retained native graphics lifetime.
use super::*;

#[com_object(interfaces(IInputSink))]
/// Records only copied text commits, ignoring construction/focus transitions.
struct TextSink {
    /// The synchronous input receiver and assertion share no window ownership.
    text: Rc<RefCell<Vec<String>>>,
}
impl IInputSink for TextSink {
    fn input(&self, event: UiInput, text: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        if event == UiInput::Text {
            self.text.borrow_mut().push(String::from_utf8(text.as_slice().to_vec()).unwrap());
        }
        Ok(())
    }
}

/// A retained graphics host must not keep an abandoned SDL input queue registered.
#[test]
#[ignore = "requires native SDL; run with SDL_VIDEODRIVER=offscreen for GL"]
fn retained_graphics_release_public_window_input_routes() {
    let _session = native_session();
    let platform = PlatformObject::new(neocogi_platform::compiler::CompilerObject::from_value(neocogi_platform::compiler::CompilerObject).query().unwrap());
    let native = platform.window("graphics-only lifetime", SIZE).unwrap();
    let id = native.device.window.id();
    let weak = Rc::downgrade(&native.device);
    let host = native.device.host.clone();
    let window = WindowObject::from_value(native);
    let graphics = window.query::<IPlatformWindowAbi>().unwrap().graphics_host().unwrap();
    let native_b = platform.window("live input lifetime", SIZE).unwrap();
    let id_b = native_b.device.window.id();
    let window_b = WindowObject::from_value(native_b);
    let platform_b = window_b.query::<IPlatformWindowAbi>().unwrap();
    drop(window);
    assert!(weak.upgrade().is_some(), "graphics must keep its native device alive");
    assert!(!host.queues.borrow().contains_key(&id), "input routing belongs to the released public window");

    let text = Rc::new(RefCell::new(Vec::new()));
    let sink: ComPtr<IInputSinkAbi> = TextSink::from_value(TextSink { text: text.clone() }).query().unwrap();
    while platform_b.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    text.borrow_mut().clear();
    // sdl2's safe push_event adapter does not encode text commits. Submit valid native storage
    // through SDL itself so this regression crosses the actual process-wide event routing path.
    let push_text = |window_id: u32, value: &str| {
        assert!(value.len() < 32);
        let mut bytes = [0; 32];
        for (target, source) in bytes.iter_mut().zip(value.bytes()) {
            *target = source as std::ffi::c_char;
        }
        let mut event = sdl2::sys::SDL_Event {
            text: sdl2::sys::SDL_TextInputEvent {
                type_: sdl2::sys::SDL_EventType::SDL_TEXTINPUT as u32,
                timestamp: 0,
                windowID: window_id,
                text: bytes,
            },
        };
        // SAFETY: SDL is initialized, the union contains the matching valid fixed text record,
        // and SDL copies its complete initialized storage before returning.
        assert_eq!(unsafe { sdl2::sys::SDL_PushEvent(&mut event) }, 1);
    };
    push_text(id, "abandoned A");
    push_text(id_b, "live B");
    while platform_b.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
    assert_eq!(&*text.borrow(), &["live B"]);
    assert!(!host.queues.borrow().contains_key(&id));
    drop(graphics);
    assert!(weak.upgrade().is_none(), "graphics release must still retire the native device");
}
