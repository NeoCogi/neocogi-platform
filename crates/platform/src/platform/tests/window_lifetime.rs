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

#[com_object(interfaces(IClipboardCompletion))]
/// Copies the bounded receipt while the native clipboard owns its text storage.
struct ClipboardReceipt {
    /// COM callbacks share a receiver, with no borrow retained beyond delivery.
    results: Rc<RefCell<Vec<(i32, String)>>>,
}

impl IClipboardCompletion for ClipboardReceipt {
    /// Both an empty write receipt and a successful read must arrive once.
    fn complete(&self, result: i32, text: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        self.results
            .borrow_mut()
            .push((result, std::str::from_utf8(text.as_slice()).unwrap().to_owned()));
        Ok(())
    }
}

/// The retained clipboard keeps SDL services alive through actual window destruction.
#[test]
#[ignore = "requires an isolated clipboard, for example SDL_VIDEODRIVER=offscreen with Glow"]
fn retained_clipboard_survives_application_window_destruction() {
    let _session = native_session();
    let platform = PlatformObject::new();
    let native = platform.window("clipboard-only lifetime", SIZE).unwrap();
    let id = native.device.window.id();
    let device = Rc::downgrade(&native.device);
    let host = Rc::downgrade(&native.device.host);
    let window = WindowObject::from_value(native).query::<IPlatformWindowAbi>().unwrap();
    let clipboard = window.clipboard().unwrap();
    assert!(clipboard.query::<IPlatformWindowAbi>().is_err());
    assert!(clipboard.query::<IRendererAbi>().is_err());
    #[cfg(windows)]
    let (private_id, _video, second) = {
        let host = host.upgrade().unwrap();
        let owner = host.clipboard_window().unwrap();
        assert_ne!(owner.id(), id);
        assert!(!host.queues.borrow().contains_key(&owner.id()));
        (owner.id(), host.video.clone(), window.clipboard().unwrap())
    };
    drop(window);
    drop(platform);
    assert!(device.upgrade().is_none());
    assert!(host.upgrade().is_none());
    // SAFETY: SDL stays initialized through the retained clipboard's VideoSubsystem.
    // The returned pointer is only checked for existence and never dereferenced.
    assert!(unsafe { sdl2::sys::SDL_GetWindowFromID(id) }.is_null());
    #[cfg(windows)]
    assert!(!unsafe { sdl2::sys::SDL_GetWindowFromID(private_id) }.is_null());

    let results = Rc::new(RefCell::new(Vec::new()));
    let receipt = ClipboardReceipt::from_value(ClipboardReceipt { results: results.clone() })
        .query::<IClipboardCompletionAbi>()
        .unwrap();
    let value = "retained clipboard é☃";
    clipboard.write(NcCopyFromBufferOffset::new(0, value.as_bytes()), receipt.as_com_ref()).unwrap();
    clipboard.read(receipt.as_com_ref()).unwrap();
    assert_eq!(results.borrow().as_slice(), [(0, String::new()), (0, value.to_owned())]);
    #[cfg(windows)]
    {
        drop(clipboard);
        // SAFETY: The fixture retains the video subsystem for both handle existence queries.
        assert!(!unsafe { sdl2::sys::SDL_GetWindowFromID(private_id) }.is_null());
        drop(second);
        assert!(unsafe { sdl2::sys::SDL_GetWindowFromID(private_id) }.is_null());
    }
}

/// A retained graphics host must not keep an abandoned SDL input queue registered.
#[test]
#[ignore = "requires native SDL; run with SDL_VIDEODRIVER=offscreen for GL"]
fn retained_graphics_release_public_window_input_routes() {
    let _session = native_session();
    let platform = PlatformObject::new();
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
