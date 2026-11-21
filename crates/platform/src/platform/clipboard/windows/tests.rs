//! Native clipboard qualification through generated COM and an independent Win32 process.

use super::*;
use crate::platform::{
    IClipboardCompletion, IClipboardCompletionAbi, IClipboardCompletionVtblFor, IPlatformWindowAbi, ITextClipboardAbi, NcCopyFromBufferOffset, PlatformObject,
    Size, WindowObject,
    clipboard::{Backend, Clipboard},
    com_object,
};
use neocogi_com_sdk::ComPtr;
use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    ptr::{null, null_mut},
    thread,
    time::Instant,
};
use windows_sys::Win32::{
    System::DataExchange::{GetClipboardOwner, RegisterClipboardFormatW},
    UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, IsWindow, WS_POPUP},
};

/// Interactive gate also launches this exact test as a separate executable.
const PEER_TEST: &str = "platform::clipboard::windows::tests::independent_win32_peer";
/// Directory used by the subprocess spawned through the test harness.
const PEER_DIR: &str = "NEOCOGI_TEST_WIN32_CLIPBOARD_PEER_DIR";
/// Role passed on the child's inherited environment.
const PEER_ROLE: &str = "NEOCOGI_TEST_WIN32_CLIPBOARD_PEER_ROLE";
/// Bound each cross-process handshake, including cleanup on test failure.
const PEER_TIMEOUT: Duration = Duration::from_secs(15);
/// Platform write input includes LF, CRLF, a lone CR, BMP text and a surrogate pair.
const CUT: &str = "copied é😀\nsecond\r\nthird\rlast\n";
/// The independent peer must observe exact Win32 line endings.
const PUBLISHED: &str = "copied é😀\r\nsecond\r\nthird\rlast\r\n";
/// Known nonempty native data retained throughout clipboard contention.
const HELD: &str = "external 😀\r\nUnicode é\r\n";

/// Isolates one child with a fixed lifetime and a private file handshake.
struct Peer {
    /// Killed and joined if an assertion interrupts its role.
    child: Child,
    /// Kept until the child has finished all clipboard coordination.
    directory: tempfile::TempDir,
}

impl Peer {
    /// Runs a role in an independent Win32 process of this same harness.
    fn spawn(role: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", PEER_TEST, "--nocapture"])
            .env(PEER_ROLE, role)
            .env(PEER_DIR, directory.path())
            .spawn()
            .unwrap();
        Self { child, directory }
    }

    /// Waits for the child to own the open clipboard before testing contention.
    fn ready(&mut self) {
        let deadline = Instant::now() + PEER_TIMEOUT;
        while !self.directory.path().join("ready").exists() {
            assert!(Instant::now() < deadline, "peer never acquired the clipboard");
            assert!(self.child.try_wait().unwrap().is_none(), "peer exited before opening the clipboard");
            pump_wait();
        }
    }

    /// Gives a live holder permission to close, then checks its exit status.
    fn release(mut self) {
        fs::write(self.directory.path().join("release"), []).unwrap();
        self.wait();
    }

    /// Waits for native work to finish while servicing synchronous owner messages.
    fn wait(&mut self) {
        let deadline = Instant::now() + PEER_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "Win32 clipboard peer failed with {status}");
                return;
            }
            assert!(Instant::now() < deadline, "Win32 clipboard peer did not exit");
            pump_wait();
        }
    }

    /// Acquires and reads the native clipboard while the calling test remains on its current stack.
    fn read() -> Option<String> {
        let mut peer = Self::spawn("read");
        peer.wait();
        let outcome = fs::read(peer.directory.path().join("read")).unwrap();
        match outcome.split_first().unwrap() {
            (0, []) => None,
            (1, text) => Some(str::from_utf8(text).unwrap().to_owned()),
            _ => panic!("invalid clipboard peer response"),
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// SDL may receive synchronous Win32 ownership messages while a peer is replacing a selection.
fn pump_wait() {
    // SAFETY: The interactive test keeps a live SDL window and executes on its owning thread.
    unsafe { sys::SDL_PumpEvents() };
    thread::sleep(Duration::from_millis(2));
}

/// Win32 peer window, with no SDL clipboard or Platform linkage on this call path.
struct PeerWindow(HWND);

impl PeerWindow {
    /// Creates a hidden top-level Win32 owner using the predefined system STATIC class.
    fn new() -> Self {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: The class name is terminated and every optional object is absent.
        let window = unsafe { CreateWindowExW(0, class.as_ptr(), null(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), null_mut(), null()) };
        assert!(!window.is_null());
        Self(window)
    }

    /// Peers use one direct open attempt, so success during a callback proves that it was released.
    fn open(&self) -> PeerSession<'_> {
        // SAFETY: This window stays live for the entire peer session.
        assert_ne!(unsafe { OpenClipboard(self.0) }, 0, "independent OpenClipboard failed");
        PeerSession { _window: self }
    }
}

impl Drop for PeerWindow {
    fn drop(&mut self) {
        // SAFETY: This thread owns the window and no peer session still borrows it.
        unsafe { DestroyWindow(self.0) };
    }
}

/// Independent native access scoped to the lifetime of its peer owner.
struct PeerSession<'a> {
    /// Retains the window until the matching CloseClipboard.
    _window: &'a PeerWindow,
}

impl PeerSession<'_> {
    /// Publishes a known terminated payload using independent Win32 calls.
    fn seed(&self, format: u32, units: &[u16]) {
        // SAFETY: The open session owns a valid HWND and the fresh buffer has the exact input size.
        unsafe {
            assert_ne!(EmptyClipboard(), 0);
            let handle = GlobalAlloc(GMEM_MOVEABLE, size_of_val(units));
            assert!(!handle.is_null());
            let data = GlobalLock(handle).cast::<u16>();
            assert!(!data.is_null());
            std::ptr::copy_nonoverlapping(units.as_ptr(), data, units.len());
            SetLastError(ERROR_SUCCESS);
            assert_eq!(GlobalUnlock(handle), 0);
            assert_eq!(GetLastError(), ERROR_SUCCESS);
            assert!(!SetClipboardData(format, handle).is_null());
        }
    }

    /// Copies independently known text and serializes presence separately from the value.
    fn read_to(&self, path: &Path) {
        // SAFETY: This thread holds an open clipboard and borrows its handle only for the copy.
        let output = unsafe {
            if IsClipboardFormatAvailable(TEXT_FORMAT) == 0 {
                vec![0]
            } else {
                let handle = GetClipboardData(TEXT_FORMAT);
                assert!(!handle.is_null());
                let count = (GlobalSize(handle) / size_of::<u16>()).min(MAX_CLIPBOARD_BYTES + 1);
                assert_ne!(count, 0);
                let mut units = vec![0u16; count];
                let bytes = size_of_val(units.as_slice());
                let data = GlobalLock(handle);
                assert!(!data.is_null());
                // Copy the native bytes before inspecting words, including any allocation padding.
                let mut copied = 0;
                let read = ReadProcessMemory(GetCurrentProcess(), data, units.as_mut_ptr().cast(), bytes, &mut copied);
                GlobalUnlock(handle);
                assert_ne!(read, 0);
                assert_eq!(copied, bytes);
                let end = units.iter().position(|unit| *unit == 0).expect("peer received unterminated Unicode text");
                let mut output = vec![1];
                output.extend(String::from_utf16(&units[..end]).unwrap().as_bytes());
                output
            }
        };
        fs::write(path, output).unwrap();
    }
}

impl Drop for PeerSession<'_> {
    fn drop(&mut self) {
        // SAFETY: This peer thread holds the matching successful open.
        assert_ne!(unsafe { CloseClipboard() }, 0);
    }
}

/// Independent raw Win32 fixture selected explicitly by the parent process.
#[test]
#[ignore = "child process for the interactive Win32 clipboard gate"]
fn independent_win32_peer() {
    let Ok(role) = std::env::var(PEER_ROLE) else {
        return;
    };
    let directory = PathBuf::from(std::env::var_os(PEER_DIR).unwrap());
    let window = PeerWindow::new();
    match role.as_str() {
        "missing" => {
            let name: Vec<u16> = "NeoCogi native clipboard test\0".encode_utf16().collect();
            // SAFETY: The Win32 format name is NUL-terminated.
            let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
            assert_ne!(format, 0);
            window.open().seed(format, &[1, 0]);
        }
        "empty" => window.open().seed(TEXT_FORMAT, &[0]),
        "read" => window.open().read_to(&directory.join("read")),
        "hold" => {
            let units: Vec<u16> = HELD.encode_utf16().chain([0]).collect();
            window.open().seed(TEXT_FORMAT, &units);
            let _session = window.open();
            fs::write(directory.join("ready"), []).unwrap();
            let deadline = Instant::now() + PEER_TIMEOUT;
            while !directory.join("release").exists() {
                assert!(Instant::now() < deadline, "parent never released the open clipboard");
                thread::sleep(Duration::from_millis(2));
            }
        }
        _ => panic!("unknown Win32 peer role {role:?}"),
    }
}

#[com_object(interfaces(IClipboardCompletion))]
/// Copied COM receipts and an editor-style source that retires only on successful cut.
struct Receipt {
    /// Appended once per requested native transfer, after native ownership ends.
    outcomes: Rc<RefCell<Vec<(i32, String)>>>,
    /// If set, the requesting editor still owns this cut source.
    source: Option<Rc<Cell<Option<&'static str>>>>,
}

impl IClipboardCompletion for Receipt {
    fn complete(&self, result: i32, text: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        assert_eq!(text.offset(), 0);
        self.outcomes.borrow_mut().push((result, str::from_utf8(text.as_slice()).unwrap().to_owned()));
        if result == 0
            && let Some(source) = &self.source
        {
            source.set(None);
        }
        Ok(())
    }
}

#[com_object(interfaces(IClipboardCompletion))]
/// Observes a native publication and then reenters the same COM clipboard.
struct Reenter {
    /// A successful completion must let a new native process acquire the clipboard immediately.
    expected: &'static str,
    /// Reenters the same service without borrowing the public window.
    clipboard: ComPtr<ITextClipboardAbi>,
    /// Records the nested read's separately owned receipt.
    nested: ComPtr<IClipboardCompletionAbi>,
    /// Counts the outer callback exactly once.
    calls: Rc<Cell<u32>>,
}

impl IClipboardCompletion for Reenter {
    fn complete(&self, status: i32, text: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        assert_eq!((status, text.as_slice()), (0, &[][..]));
        self.calls.set(self.calls.get() + 1);
        assert_eq!(Peer::read().as_deref(), Some(self.expected));
        self.clipboard.read(self.nested.as_com_ref())
    }
}

/// Exercises native receipt status, independent transfer and private owner lifetime.
#[test]
#[ignore = "requires an isolated interactive Windows desktop using the SDL windows driver"]
fn windows_clipboard_native_receipts() {
    let _session = crate::platform::tests::native_session();
    let platform = PlatformObject::new();
    let native = platform.window("Win32 clipboard qualification", Size { width: 64, height: 64 }).unwrap();
    let host = Rc::downgrade(&native.device.host);
    let public_id = native.device.window.id();
    assert_eq!(native.device.host.video.current_video_driver(), "windows");
    let window = WindowObject::from_value(native).query::<IPlatformWindowAbi>().unwrap();
    let clipboard = window.clipboard().unwrap();
    let second = window.clipboard().unwrap();
    let (private_id, private_hwnd, _video) = {
        let host = host.upgrade().unwrap();
        let owner = host.clipboard_window().unwrap();
        assert_ne!(owner.id(), public_id);
        assert!(!host.queues.borrow().contains_key(&owner.id()));
        let private = Win32Clipboard::new(owner.clone()).unwrap();
        (owner.id(), private.hwnd, host.video.clone())
    };
    drop(window);
    drop(platform);
    assert!(host.upgrade().is_none());
    // SAFETY: These lookups query the live video subsystem and the native window table.
    unsafe {
        assert!(sys::SDL_GetWindowFromID(public_id).is_null());
        assert!(!sys::SDL_GetWindowFromID(private_id).is_null());
        assert_ne!(IsWindow(private_hwnd.as_ptr()), 0);
    }

    let outcomes = Rc::new(RefCell::new(Vec::new()));
    let receipt = Receipt::from_value(Receipt { outcomes: outcomes.clone(), source: None })
        .query::<IClipboardCompletionAbi>()
        .unwrap();
    for (role, external) in [("missing", None), ("empty", Some(""))] {
        Peer::spawn(role).wait();
        assert_eq!(Peer::read().as_deref(), external);
        clipboard.read(receipt.as_com_ref()).unwrap();
    }
    let mut holder = Peer::spawn("hold");
    holder.ready();
    clipboard.read(receipt.as_com_ref()).unwrap();

    let source = Rc::new(Cell::new(Some(CUT)));
    let cut = Receipt::from_value(Receipt {
        outcomes: outcomes.clone(),
        source: Some(source.clone()),
    })
    .query::<IClipboardCompletionAbi>()
    .unwrap();
    clipboard.write(NcCopyFromBufferOffset::new(0, CUT.as_bytes()), cut.as_com_ref()).unwrap();
    assert_eq!(source.get(), Some(CUT));
    let failed = PLATFORM_E_BACKEND.code();
    assert_eq!(outcomes.take(), [(0, "".into()), (0, "".into()), (failed, "".into()), (failed, "".into())]);
    holder.release();
    assert_eq!(Peer::read().as_deref(), Some(HELD));
    clipboard.read(receipt.as_com_ref()).unwrap();
    assert_eq!(outcomes.take(), [(0, HELD.into())]);

    let calls = Rc::new(Cell::new(0));
    let reenter = Reenter::from_value(Reenter {
        expected: PUBLISHED,
        clipboard: clipboard.clone(),
        nested: receipt,
        calls: calls.clone(),
    })
    .query::<IClipboardCompletionAbi>()
    .unwrap();
    clipboard.write(NcCopyFromBufferOffset::new(0, CUT.as_bytes()), reenter.as_com_ref()).unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(outcomes.take(), [(0, PUBLISHED.into())]);
    // SAFETY: The live private owner supplied the HWND to OpenClipboard.
    assert_eq!(unsafe { GetClipboardOwner() }, private_hwnd.as_ptr());
    drop(reenter);
    drop(clipboard);
    // SAFETY: The second projection keeps this exact owner alive until it is dropped.
    assert_ne!(unsafe { IsWindow(private_hwnd.as_ptr()) }, 0);
    drop(second);
    // SAFETY: The SDL video guard remains alive after the last private window lease dropped.
    assert_eq!(unsafe { IsWindow(private_hwnd.as_ptr()) }, 0);
}

/// Counts write boundaries and native ownership outcomes without replacing lock or session lifetimes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NativeCalls {
    /// GlobalAlloc attempts.
    alloc: u8,
    /// GlobalLock attempts.
    lock: u8,
    /// SetClipboardData attempts.
    publish: u8,
    /// Handles accepted by Win32.
    transferred: u8,
    /// Handles explicitly freed by the application.
    freed: u8,
}

thread_local! {
    /// Test seam is confined to the active SDL thread and reset for every attempt.
    static CALLS: Cell<NativeCalls> = const { Cell::new(NativeCalls { alloc: 0, lock: 0, publish: 0, transferred: 0, freed: 0 }) };
}

/// Records an observation on the thread holding the active native operation.
fn count(observe: impl FnOnce(&mut NativeCalls)) {
    CALLS.with(|calls| {
        let mut value = calls.get();
        observe(&mut value);
        calls.set(value);
    });
}

/// Injects a fallible UTF-16 preparation failure before allocating or opening.
fn bad_conversion(_: &str) -> Result<Vec<u16>, NcStatus> {
    Err(PLATFORM_E_BACKEND)
}

/// Records an actual movable allocation.
unsafe extern "system" fn alloc(flags: u32, bytes: usize) -> HGLOBAL {
    count(|calls| calls.alloc += 1);
    // SAFETY: Forwards exactly the valid native allocation arguments.
    unsafe { GlobalAlloc(flags, bytes) }
}

/// Reports a failed allocation at the native call boundary.
unsafe extern "system" fn bad_alloc(_: u32, _: usize) -> HGLOBAL {
    count(|calls| calls.alloc += 1);
    null_mut()
}

/// Records an actual native lock.
unsafe extern "system" fn lock(handle: HGLOBAL) -> *mut c_void {
    count(|calls| calls.lock += 1);
    // SAFETY: The production allocation keeps this live handle until unlock.
    unsafe { GlobalLock(handle) }
}

/// Reports a failed lock, leaving an owned unlocked allocation to free.
unsafe extern "system" fn bad_lock(_: HGLOBAL) -> *mut c_void {
    count(|calls| calls.lock += 1);
    null_mut()
}

/// Counts an actual successful free on failure paths that held an allocation.
unsafe extern "system" fn free(handle: HGLOBAL) -> HGLOBAL {
    // SAFETY: This handle remains application-owned and was released by any successful lock.
    let result = unsafe { GlobalFree(handle) };
    if result.is_null() {
        count(|calls| calls.freed += 1);
    }
    result
}

/// Records an actual native publication and its transfer result.
unsafe extern "system" fn publish(format: u32, handle: HGLOBAL) -> HANDLE {
    count(|calls| calls.publish += 1);
    // SAFETY: The production session owns the empty clipboard and unlocked movable handle.
    let result = unsafe { SetClipboardData(format, handle) };
    if !result.is_null() {
        count(|calls| calls.transferred += 1);
    }
    result
}

/// Reports a rejected publication after a successful EmptyClipboard.
unsafe extern "system" fn bad_publish(_: u32, _: HGLOBAL) -> HANDLE {
    count(|calls| calls.publish += 1);
    null_mut()
}

/// Verifies stage-specific ownership changes and cut receipts through generated COM.
#[test]
#[ignore = "requires an isolated interactive Windows desktop using the SDL windows driver"]
fn windows_clipboard_native_write_failures() {
    let _session = crate::platform::tests::native_session();
    let platform = PlatformObject::new();
    let window = platform.window("Win32 clipboard write ownership", Size { width: 64, height: 64 }).unwrap();
    assert_eq!(window.device.host.video.current_video_driver(), "windows");
    let owner = window.device.host.clipboard_window().unwrap();
    let window = WindowObject::from_value(window).query::<IPlatformWindowAbi>().unwrap();
    let normal = window.clipboard().unwrap();
    let tracked = WriteCalls {
        allocate: alloc,
        lock,
        free,
        publish,
        ..WriteCalls::NATIVE
    };
    let empty = NativeCalls::default();
    let cases = [
        (WriteCalls { encode: bad_conversion, ..tracked }, empty, Some("prior"), false),
        (
            WriteCalls { allocate: bad_alloc, ..tracked },
            NativeCalls { alloc: 1, ..empty },
            Some("prior"),
            false,
        ),
        (
            WriteCalls { lock: bad_lock, ..tracked },
            NativeCalls { alloc: 1, lock: 1, freed: 1, ..empty },
            Some("prior"),
            false,
        ),
        (
            WriteCalls { publish: bad_publish, ..tracked },
            NativeCalls {
                alloc: 1,
                lock: 1,
                publish: 1,
                freed: 1,
                ..empty
            },
            None,
            false,
        ),
        (
            tracked,
            NativeCalls {
                alloc: 1,
                lock: 1,
                publish: 1,
                transferred: 1,
                ..empty
            },
            Some(PUBLISHED),
            true,
        ),
    ];
    let outcomes = Rc::new(RefCell::new(Vec::new()));
    let source = Rc::new(Cell::new(None));
    let receipt = Receipt::from_value(Receipt {
        outcomes: outcomes.clone(),
        source: Some(source.clone()),
    })
    .query::<IClipboardCompletionAbi>()
    .unwrap();
    for (calls, expected_calls, expected_text, success) in cases {
        normal.write(NcCopyFromBufferOffset::new(0, b"prior"), receipt.as_com_ref()).unwrap();
        assert_eq!(outcomes.take(), [(0, String::new())]);
        let backend = Win32Clipboard {
            write_calls: calls,
            ..Win32Clipboard::new(owner.clone()).unwrap()
        };
        let clipboard = Clipboard::from_value(Clipboard { backend: Backend::Win32(backend) })
            .query::<ITextClipboardAbi>()
            .unwrap();
        CALLS.set(empty);
        source.set(Some(CUT));
        clipboard.write(NcCopyFromBufferOffset::new(0, CUT.as_bytes()), receipt.as_com_ref()).unwrap();
        assert_eq!(outcomes.take(), [(if success { 0 } else { PLATFORM_E_BACKEND.code() }, String::new())]);
        assert_eq!(source.get(), if success { None } else { Some(CUT) });
        assert_eq!(CALLS.get(), expected_calls);
        assert_eq!(Peer::read().as_deref(), expected_text);
    }
}

/// Covers bounded native text before converting or returning any result.
#[test]
fn windows_text_conversion_boundaries() {
    assert_eq!(encode_text(CUT).unwrap(), PUBLISHED.encode_utf16().chain([0]).collect::<Vec<_>>());
    assert_eq!(encode_text("").unwrap(), [0]);
    assert_eq!(decode_text(&[0, 0xd800], false), Ok(String::new()));
    assert_eq!(decode_text(&[0xd800, 0], false), Err(PLATFORM_E_BACKEND));
    assert_eq!(decode_text(&[1, 2], false), Err(PLATFORM_E_BACKEND));
    let mut ascii = vec![u16::from(b'a'); MAX_CLIPBOARD_BYTES + 1];
    assert_eq!(decode_text(&ascii, true), Err(NC_E_OUT_OF_RANGE));
    ascii[MAX_CLIPBOARD_BYTES] = 0;
    assert_eq!(decode_text(&ascii, true).unwrap().len(), MAX_CLIPBOARD_BYTES);
    let mut unicode = vec!['é' as u16; MAX_CLIPBOARD_BYTES / 2 + 1];
    unicode.push(0);
    assert_eq!(decode_text(&unicode, false), Err(NC_E_OUT_OF_RANGE));
}
