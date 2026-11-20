//! Generated COM receipts against independently connected clients on a private Wayland server.
use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};

/// A new empty directory shared with the private compositor input driver.
const COORDINATION: &str = "NEOCOGI_TEST_ISOLATED_WAYLAND_DIRECTORY";
/// Selects the standalone SDL client role, never the Platform clipboard ABI.
const PEER_ROLE: &str = "NEOCOGI_TEST_WAYLAND_PEER";
/// Input from a different client, including multibyte UTF-8 and a newline.
const SELECTION: &str = "External Wayland selection é雪\nsecond line";
/// Text that the editor must preserve on a rejected cut.
const CUT: &str = "Preserve this editor text 🔐";

/// Waits for compositor dispatch or another client, keeping every acceptance phase finite.
fn wait_for(description: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {description}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Reaps independent clients after success or a failed assertion.
struct Peer(Child);
impl Peer {
    /// Starts a process with its own SDL and Wayland connection.
    fn start(role: &str) -> Self {
        Self(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "--ignored",
                    "--nocapture",
                    "platform::tests::wayland_clipboard::independent_sdl_client",
                ])
                .env(PEER_ROLE, role)
                .spawn()
                .unwrap(),
        )
    }

    /// Confirms a writer still owns its source or a reader has succeeded.
    fn status(&mut self) -> Option<std::process::ExitStatus> {
        self.0.try_wait().unwrap()
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Records exact receipt copies and models removal of cut text only upon accepted writes.
#[com_object(interfaces(IClipboardCompletion))]
struct Receipt {
    /// Native queue ownership is observable without retaining the host.
    host: Weak<NativeHost>,
    /// A cut holds its source text until a successful write receipt releases it.
    cut: Rc<RefCell<Option<String>>>,
    /// Ordered receipt facts, which can outlive the native transfer.
    received: Rc<RefCell<Vec<(i32, String)>>>,
}
impl IClipboardCompletion for Receipt {
    /// Copied data and cut decisions occur with no route borrow from the clipboard service.
    fn complete(&self, status: i32, text: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        if let Some(host) = self.host.upgrade() {
            assert!(host.queues.try_borrow_mut().is_ok(), "host route borrow crossed the COM callback");
        }
        assert_eq!(text.offset(), 0);
        let text = std::str::from_utf8(text.as_slice()).unwrap().to_owned();
        if status == 0 {
            self.cut.borrow_mut().take();
        }
        self.received.borrow_mut().push((status, text));
        Ok(())
    }
}

/// Observes a real compositor key through the public input route.
#[com_object(interfaces(IInputSink))]
struct InputProbe {
    /// Shared observation of the real key delivered after write rejection.
    pressed: Rc<Cell<bool>>,
}
impl IInputSink for InputProbe {
    /// F9 is sent by the private compositor harness after the write has rejected.
    fn input(&self, input: UiInput, _: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        if matches!(
            input,
            UiInput::Key(KeyStroke {
                key: KeyCode::Function(9),
                action: KeyAction::Pressed { repeat: false },
                ..
            })
        ) {
            self.pressed.set(true);
        }
        Ok(())
    }
}

/// Pumps the public route until SDL has dispatched all currently available input.
fn poll(window: &ComPtr<IPlatformWindowAbi>, sink: &ComPtr<IInputSinkAbi>) {
    while window.poll(sink.as_com_ref()).unwrap() == PollStatus::Event {}
}

/// Publishes or observes actual compositor selection through an independent SDL2 connection.
#[test]
#[ignore = "helper process spawned by wayland_receipts_follow_the_focused_public_route"]
fn independent_sdl_client() {
    let Ok(role) = std::env::var(PEER_ROLE) else { return };
    let directory = PathBuf::from(std::env::var_os(COORDINATION).unwrap());
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    assert_eq!(video.current_video_driver(), "wayland");
    let mut canvas = video
        .window(&format!("Independent clipboard {role}"), 80, 80)
        .build()
        .unwrap()
        .into_canvas()
        .software()
        .build()
        .unwrap();
    canvas.clear();
    canvas.present();
    let mut events = sdl.event_pump().unwrap();
    let clipboard = video.clipboard();
    match role.as_str() {
        "owner" => {
            let mut announced = false;
            wait_for("F8 in the selection owner", || {
                for event in events.poll_iter() {
                    if matches!(
                        event,
                        sdl2::event::Event::KeyDown {
                            keycode: Some(sdl2::keyboard::Keycode::F8),
                            repeat: false,
                            ..
                        }
                    ) {
                        clipboard.set_clipboard_text(SELECTION).unwrap();
                        // The selection request is flushed before the next client gains focus.
                        events.pump_events();
                        return true;
                    }
                }
                if !announced && sdl.keyboard().focused_window_id() == Some(canvas.window().id()) {
                    fs::write(directory.join("owner-focused"), []).unwrap();
                    announced = true;
                }
                false
            });
            fs::write(directory.join("owner-wrote"), []).unwrap();
            wait_for("parent to release the selection owner", || {
                events.pump_events();
                directory.join("owner-done").is_file()
            });
        }
        "reader" => wait_for("unchanged selection in the independent reader", || {
            events.pump_events();
            sdl.keyboard().focused_window_id() == Some(canvas.window().id()) && clipboard.clipboard_text().is_ok_and(|value| value == SELECTION)
        }),
        _ => panic!("invalid Wayland peer role: {role}"),
    }
}

/// Private compositor keyboard input must send F8 at owner-focused and F9 at app-ready.
#[test]
#[ignore = "requires a fresh private Wayland server and the compositor input handshake"]
fn wayland_receipts_follow_the_focused_public_route() {
    let Some(directory) = std::env::var_os(COORDINATION) else { return };
    let directory = PathBuf::from(directory);
    assert!(fs::read_dir(&directory).unwrap().next().is_none(), "use a new private coordination directory");
    let _session = native_session();
    let mut owner = Peer::start("owner");
    wait_for("independent selection publication", || directory.join("owner-wrote").is_file());
    assert!(owner.status().is_none());
    exercise_platform(&directory);
    fs::write(directory.join("owner-done"), []).unwrap();
    wait_for("independent selection owner exit", || {
        owner.status().is_some_and(|status| {
            assert!(status.success(), "independent Wayland owner failed: {status}");
            true
        })
    });
}

/// Verifies the application can read and cannot claim write success or outlive the public route.
fn exercise_platform(directory: &Path) {
    let platform = PlatformObject::new();
    let native = platform.window("NeoCogi clipboard", SIZE).unwrap();
    let host = Rc::downgrade(&native.device.host);
    let device = Rc::downgrade(&native.device);
    let id = native.device.window.id();
    let keyboard = native.device.host.video.sdl().keyboard();
    assert_eq!(native.device.host.video.current_video_driver(), "wayland");
    let window = WindowObject::from_value(native).query::<IPlatformWindowAbi>().unwrap();
    let clipboard = window.clipboard().unwrap();
    let graphics = window.graphics_host().unwrap();
    {
        // The first presented buffer maps the native xdg_toplevel for keyboard focus.
        let renderer = window.query::<IRendererAbi>().unwrap();
        let empty = Empty::from_value(Empty).query::<IRenderStreamAbi>().unwrap();
        renderer
            .render(
                RenderInfo {
                    size: window.size().unwrap(),
                    block_count: 0,
                    ..info()
                },
                empty.as_com_ref(),
            )
            .unwrap();
    }
    let key = Rc::new(Cell::new(false));
    let sink = InputProbe::from_value(InputProbe { pressed: key.clone() }).query::<IInputSinkAbi>().unwrap();
    let cut = Rc::new(RefCell::new(None));
    let received = Rc::new(RefCell::new(Vec::new()));
    let receipt = Receipt::from_value(Receipt {
        host: host.clone(),
        cut: cut.clone(),
        received: received.clone(),
    })
    .query::<IClipboardCompletionAbi>()
    .unwrap();

    wait_for("focused routed read of external Unicode", || {
        poll(&window, &sink);
        if keyboard.focused_window_id() != Some(id) {
            return false;
        }
        clipboard.read(receipt.as_com_ref()).unwrap();
        let mut results = received.borrow_mut();
        assert_eq!(results.len(), 1);
        results.pop().unwrap() == (0, SELECTION.into())
    });
    *cut.borrow_mut() = Some(CUT.into());
    for input in [NcCopyFromBufferOffset::new(1, b"bad"), NcCopyFromBufferOffset::new(0, b"\0")] {
        assert_eq!(clipboard.write(input, receipt.as_com_ref()), Err(NC_E_INVALID_ARG));
    }
    assert!(received.borrow().is_empty());
    clipboard.write(NcCopyFromBufferOffset::new(0, CUT.as_bytes()), receipt.as_com_ref()).unwrap();
    assert_eq!(received.borrow().as_slice(), [(PLATFORM_E_BACKEND.code(), String::new())]);
    assert_eq!(cut.borrow().as_deref(), Some(CUT));

    fs::write(directory.join("app-ready"), []).unwrap();
    wait_for("real F9 through the public input route after rejected write", || {
        poll(&window, &sink);
        key.get()
    });
    assert_eq!(received.borrow().len(), 1, "a later keyboard event must not produce a second receipt");
    assert_eq!(keyboard.focused_window_id(), Some(id));

    drop(window);
    drop(platform);
    assert!(device.upgrade().is_some(), "graphics must still own the actual native window");
    assert_eq!(keyboard.focused_window_id(), Some(id), "SDL still sees the retained native window as focused");
    assert!(!host.upgrade().unwrap().queues.borrow().contains_key(&id), "the public input route ended");
    clipboard.read(receipt.as_com_ref()).unwrap();
    assert_eq!(
        received.borrow().as_slice(),
        [(PLATFORM_E_BACKEND.code(), String::new()), (PLATFORM_E_BACKEND.code(), String::new())]
    );
    assert_eq!(cut.borrow().as_deref(), Some(CUT));
    // Probe the compositor before releasing the application's native window and clipboard.
    // A late publication after the real key would still be visible to this independent client.
    let mut reader = Peer::start("reader");
    wait_for("independent reader success", || {
        reader.status().is_some_and(|status| {
            assert!(status.success(), "independent Wayland reader failed: {status}");
            true
        })
    });
    drop(graphics);
    assert!(host.upgrade().is_none(), "the clipboard holds only a weak host");
    clipboard.read(receipt.as_com_ref()).unwrap();
    assert_eq!(received.borrow().last(), Some(&(PLATFORM_E_BACKEND.code(), String::new())));
    assert_eq!(received.borrow().len(), 3);
}
