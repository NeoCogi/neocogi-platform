//! System libxcb connection and native deadlines for the private X11 clipboard.
use super::{backend, failed};
use neocogi_com_sdk::{NC_E_OUT_OF_RANGE, NcStatus};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fd::{AsFd, BorrowedFd, OwnedFd},
    io::Errno,
    net::{AddressFamily, Shutdown, SocketAddrUnix, SocketFlags, SocketType, addr::SocketAddrArg, connect, shutdown, socket_with, sockopt},
};
use std::{
    env, fs,
    ffi::{c_char, c_int, c_void},
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs},
    os::fd::IntoRawFd,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    ptr::{self, NonNull},
    slice,
    time::Instant,
};
use x11rb_protocol::{
    protocol::xproto::{
        Atom, DestroyNotifyEvent, EventMask, GetPropertyReply, PropMode, PropertyNotifyEvent, SelectionClearEvent, SelectionNotifyEvent, SelectionRequestEvent,
        Timestamp, Visualid, Window, WindowClass,
    },
    x11_utils::{Serialize, TryParse},
    parse_display::{ConnectAddress, parse_display},
    xauth::{Family, get_auth},
};

/// The opaque connection lives on one thread at a time and owns its system socket.
pub(super) struct Connection(NonNull<c_void>, Deadline, Xcb);
// SAFETY: Libxcb connections can move between threads. Only the unique Connection calls disconnect.
unsafe impl Send for Connection {}

#[repr(C)]
#[derive(Clone, Copy)]
struct Cookie {
    sequence: u32,
}

#[repr(C)]
struct ScreenIterator {
    data: *mut u32,
    remaining: c_int,
    index: c_int,
}

unsafe extern "C" {
    fn gethostname(name: *mut c_char, length: usize) -> c_int;
    fn free(address: *mut c_void);
}

/// Pins an SDL-loaded native library for all of its callback pointers.
struct NativeLibrary(NonNull<c_void>);

impl NativeLibrary {
    fn open(name: &std::ffi::CStr) -> Result<Self, NcStatus> {
        // SAFETY: SDL requires a valid live C string and returns an independently owned module.
        NonNull::new(unsafe { sdl2::sys::SDL_LoadObject(name.as_ptr()) })
            .map(Self)
            .ok_or(super::PLATFORM_E_BACKEND)
    }

    fn symbol(&self, name: *const c_char) -> Result<NonNull<c_void>, NcStatus> {
        // SAFETY: The module is live, and the only callers use statically terminated symbol names.
        NonNull::new(unsafe { sdl2::sys::SDL_LoadFunction(self.0.as_ptr(), name) }).ok_or(super::PLATFORM_E_BACKEND)
    }
}

impl Drop for NativeLibrary {
    fn drop(&mut self) {
        // SAFETY: This module handle came from SDL_LoadObject and is not used after Drop.
        unsafe { sdl2::sys::SDL_UnloadObject(self.0.as_ptr()) };
    }
}

/// Core ABI of the system libxcb loaded only for the SDL X11 desktop.
macro_rules! xcb_symbols {
    ($(fn $name:ident($($param:ident: $type:ty),* $(,)?) -> $result:ty;)*) => {
        struct Xcb {
            $($name: unsafe extern "C" fn($($type),*) -> $result,)*
            /// Keeps every copied function pointer valid for the connection and its Drop.
            _library: NativeLibrary,
        }

        impl Xcb {
            fn load() -> Result<Self, NcStatus> {
                // SAFETY: This loads the standard system library for the libxcb core ABI.
                // Every symbol is typed to match its xcb.h and xproto.h declaration.
                let library = NativeLibrary::open(c"libxcb.so.1")?;
                Ok(Self {
                    $($name: {
                        let pointer = library.symbol(concat!(stringify!($name), "\0").as_ptr().cast())?;
                        // SAFETY: This symbol belongs to the matching libxcb function declaration.
                        unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($type),*) -> $result>(pointer.as_ptr()) }
                    },)*
                    _library: library,
                })
            }
        }
    };
}

xcb_symbols! {

    fn xcb_connect_to_fd(fd: c_int, auth: *mut Auth) -> *mut c_void;
    fn xcb_disconnect(connection: *mut c_void) -> ();
    fn xcb_connection_has_error(connection: *mut c_void) -> c_int;
    fn xcb_get_setup(connection: *mut c_void) -> *const c_void;
    fn xcb_setup_roots_iterator(setup: *const c_void) -> ScreenIterator;
    fn xcb_screen_next(iterator: *mut ScreenIterator) -> ();
    fn xcb_generate_id(connection: *mut c_void) -> u32;
    fn xcb_poll_for_event(connection: *mut c_void) -> *mut c_void;
    fn xcb_wait_for_event(connection: *mut c_void) -> *mut c_void;
    fn xcb_request_check(connection: *mut c_void, cookie: Cookie) -> *mut c_void;
    fn xcb_intern_atom(connection: *mut c_void, exists: u8, len: u16, name: *const u8) -> Cookie;
    fn xcb_intern_atom_reply(connection: *mut c_void, cookie: Cookie, error: *mut *mut c_void) -> *mut c_void;
    fn xcb_get_selection_owner(connection: *mut c_void, selection: Atom) -> Cookie;
    fn xcb_get_selection_owner_reply(connection: *mut c_void, cookie: Cookie, error: *mut *mut c_void) -> *mut c_void;
    fn xcb_create_window_checked(connection: *mut c_void, depth: u8, id: Window, parent: Window, x: i16, y: i16,
        w: u16, h: u16, border: u16, class: u16, visual: Visualid, mask: u32, values: *const u32) -> Cookie;
    fn xcb_change_window_attributes_checked(connection: *mut c_void, window: Window, mask: u32, values: *const u32) -> Cookie;
    fn xcb_change_property_checked(connection: *mut c_void, mode: u8, window: Window, property: Atom, type_: Atom,
        format: u8, data_len: u32, data: *const c_void) -> Cookie;
    fn xcb_get_property(connection: *mut c_void, delete: u8, window: Window, property: Atom, type_: Atom,
        offset: u32, length: u32) -> Cookie;
    fn xcb_get_property_reply(connection: *mut c_void, cookie: Cookie, error: *mut *mut c_void) -> *mut c_void;
    fn xcb_delete_property_checked(connection: *mut c_void, window: Window, property: Atom) -> Cookie;
    fn xcb_convert_selection_checked(connection: *mut c_void, requestor: Window, selection: Atom, target: Atom,
        property: Atom, time: Timestamp) -> Cookie;
    fn xcb_set_selection_owner_checked(connection: *mut c_void, owner: Window, selection: Atom, time: Timestamp) -> Cookie;
    fn xcb_send_event_checked(connection: *mut c_void, propagate: u8, destination: Window, mask: u32, data: *const u8) -> Cookie;
}

/// Owns xcb's allocated reply, event, or error for the duration of decoding.
struct Packet(NonNull<c_void>);
impl Packet {
    fn new(pointer: *mut c_void) -> Option<Self> {
        NonNull::new(pointer).map(Self)
    }

    fn header(&self) -> &[u8] {
        // SAFETY: Every xcb reply, error, and ordinary event has a 32 byte common allocation.
        unsafe { slice::from_raw_parts(self.0.as_ptr().cast(), 32) }
    }

    fn reply(&self, request_words: u32) -> Result<&[u8], NcStatus> {
        let words = u32::from_ne_bytes(self.header()[4..8].try_into().unwrap());
        if words > request_words {
            return Err(NC_E_OUT_OF_RANGE);
        }
        let length = usize::try_from(words)
            .ok()
            .and_then(|words| words.checked_mul(4)?.checked_add(32))
            .filter(|length| *length <= isize::MAX as usize)
            .ok_or(NC_E_OUT_OF_RANGE)?;
        // SAFETY: The packet owns libxcb's complete wire allocation. The checked size also
        // fits Rust slice arithmetic and the property's requested word budget.
        Ok(unsafe { slice::from_raw_parts(self.0.as_ptr().cast(), length) })
    }
}
impl Drop for Packet {
    fn drop(&mut self) {
        // SAFETY: This owns the allocation returned by xcb, with the matching system allocator.
        unsafe { free(self.0.as_ptr()) };
    }
}

pub(super) enum Event {
    SelectionRequest(SelectionRequestEvent),
    SelectionClear(SelectionClearEvent),
    SelectionNotify(SelectionNotifyEvent),
    PropertyNotify(PropertyNotifyEvent),
    DestroyNotify(DestroyNotifyEvent),
    Other,
}
impl Event {
    fn from_packet(packet: Packet) -> Result<Self, NcStatus> {
        let bytes = packet.header();
        Ok(match bytes[0] & 0x7f {
            17 => Self::DestroyNotify(backend(DestroyNotifyEvent::try_parse(bytes))?.0),
            28 => Self::PropertyNotify(backend(PropertyNotifyEvent::try_parse(bytes))?.0),
            29 => Self::SelectionClear(backend(SelectionClearEvent::try_parse(bytes))?.0),
            30 => Self::SelectionRequest(backend(SelectionRequestEvent::try_parse(bytes))?.0),
            31 => Self::SelectionNotify(backend(SelectionNotifyEvent::try_parse(bytes))?.0),
            _ => Self::Other,
        })
    }
}

#[repr(C)]
struct Auth {
    name_length: c_int,
    name: *mut c_char,
    data_length: c_int,
    data: *mut c_char,
}

/// A joined watchdog interrupts blocking libxcb setup or checked reply calls via the socket.
struct Deadline {
    state: Arc<Watch>,
    worker: Option<JoinHandle<()>>,
}

struct Watch {
    socket: OwnedFd,
    phase: Mutex<WatchPhase>,
    changed: Condvar,
}

enum WatchPhase {
    Idle,
    Armed(Instant),
    Stopped,
}

impl Deadline {
    fn new(socket: &OwnedFd, deadline: Instant) -> Result<Self, NcStatus> {
        let state = Arc::new(Watch {
            socket: backend(rustix::io::fcntl_dupfd_cloexec(socket, 0))?,
            phase: Mutex::new(WatchPhase::Armed(deadline)),
            changed: Condvar::new(),
        });
        let watch = state.clone();
        let worker = backend(thread::Builder::new().name("clipboard-xcb-deadline".to_owned()).spawn(move || watch.run()))?;
        Ok(Self { state, worker: Some(worker) })
    }

    fn arm(&self, deadline: Instant) {
        self.state.update(WatchPhase::Armed(deadline));
    }

    fn disarm(&self) {
        self.state.update(WatchPhase::Idle);
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        self.state.update(WatchPhase::Stopped);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Watch {
    fn update(&self, next: WatchPhase) {
        let mut phase = self.phase.lock().expect("only the watchdog uses this state");
        if !matches!(*phase, WatchPhase::Stopped) {
            *phase = next;
            self.changed.notify_one();
        }
    }

    fn run(&self) {
        let mut phase = self.phase.lock().expect("only the watchdog uses this state");
        loop {
            match *phase {
                WatchPhase::Stopped => return,
                WatchPhase::Armed(deadline) => {
                    let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                        let _ = shutdown(&self.socket, Shutdown::Both);
                        *phase = WatchPhase::Stopped;
                        return;
                    };
                    (phase, _) = self.changed.wait_timeout(phase, left).expect("only the watchdog uses this state");
                }
                WatchPhase::Idle => {
                    phase = self.changed.wait(phase).expect("only the watchdog uses this state");
                }
            }
        }
    }
}

/// Checks deadline before native setup or a socket operation.
fn check_deadline(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::ErrorKind::TimedOut.into())
    } else {
        Ok(())
    }
}

/// Opens a nonblocking socket on the current I/O budget.
fn dial(family: AddressFamily, address: &impl SocketAddrArg, deadline: Instant) -> io::Result<OwnedFd> {
    check_deadline(deadline)?;
    let fd = socket_with(family, SocketType::STREAM, SocketFlags::NONBLOCK | SocketFlags::CLOEXEC, None)?;
    match connect(&fd, address) {
        Ok(()) => (),
        Err(Errno::INPROGRESS | Errno::AGAIN | Errno::INTR) => {
            wait(&mut [PollFd::new(&fd, PollFlags::OUT)], Some(deadline))?;
            sockopt::socket_error(&fd)??;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(fd)
}

/// Local connections and loopback SSH proxies use the local authority family.
fn local_authority() -> (Family, Vec<u8>) {
    let mut name = [0u8; 256];
    // SAFETY: gethostname writes at most the supplied initialized allocation length.
    let _ = unsafe { gethostname(name.as_mut_ptr().cast(), name.len()) };
    let length = name.iter().position(|byte| *byte == 0).unwrap_or(name.len());
    (Family::LOCAL, name[..length].to_vec())
}

fn unix_socket(path: &str, deadline: Instant) -> io::Result<(OwnedFd, (Family, Vec<u8>))> {
    let abstract_address = SocketAddrUnix::new_abstract_name(path.as_bytes())?;
    let fd = dial(AddressFamily::UNIX, &abstract_address, deadline).or_else(|_| {
        let address = SocketAddrUnix::new(path)?;
        dial(AddressFamily::UNIX, &address, deadline)
    })?;
    Ok((fd, local_authority()))
}

fn tcp_socket(host: &str, port: u16, deadline: Instant) -> io::Result<(OwnedFd, (Family, Vec<u8>))> {
    check_deadline(deadline)?;
    let addresses: Vec<_> = if host.eq_ignore_ascii_case("localhost") {
        vec![SocketAddr::from((Ipv6Addr::LOCALHOST, port)), SocketAddr::from((Ipv4Addr::LOCALHOST, port))]
    } else {
        (host, port).to_socket_addrs()?.collect()
    };
    let mut last = io::ErrorKind::AddrNotAvailable.into();
    for address in addresses {
        check_deadline(deadline)?;
        let result = match address {
            SocketAddr::V4(address) => dial(AddressFamily::INET, &address, deadline),
            SocketAddr::V6(address) => dial(AddressFamily::INET6, &address, deadline),
        };
        match result {
            Ok(fd) => {
                let authority = if address.ip().is_loopback() {
                    local_authority()
                } else {
                    match address {
                        SocketAddr::V4(address) => (Family::INTERNET, address.ip().octets().to_vec()),
                        SocketAddr::V6(address) => (Family::INTERNET6, address.ip().octets().to_vec()),
                    }
                };
                return Ok((fd, authority));
            }
            Err(error) => last = error,
        }
    }
    Err(last)
}

fn has_authority_file() -> bool {
    env::var_os("XAUTHORITY")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".Xauthority")))
        .is_some_and(|path| fs::metadata(path).is_ok_and(|metadata| metadata.is_file()))
}

/// Opens an X transport, with DNS and authority filesystem calls following normal OS scheduling.
pub(super) fn open(display: &str, deadline: Instant) -> Result<(Connection, usize), NcStatus> {
    backend(check_deadline(deadline))?;
    let parsed = backend(parse_display(Some(display)))?;
    let screen = usize::from(parsed.screen);
    for address in parsed.connect_instruction() {
        let result = match address {
            ConnectAddress::Socket(path) => unix_socket(&path, deadline),
            ConnectAddress::Hostname(host, port) => tcp_socket(host, port, deadline),
            _ => continue,
        };
        let Ok((socket, (family, address))) = result else { continue };
        backend(check_deadline(deadline))?;
        let use_auth = has_authority_file();
        backend(check_deadline(deadline))?;
        let (mut name, mut data) = if use_auth {
            get_auth(family, &address, parsed.display).ok().flatten().unwrap_or_default()
        } else {
            (Vec::new(), Vec::new())
        };
        backend(check_deadline(deadline))?;
        let api = Xcb::load()?;
        backend(check_deadline(deadline))?;
        let watch = Deadline::new(&socket, deadline)?;
        let mut auth = Auth {
            name_length: name.len() as c_int,
            name: name.as_mut_ptr().cast(),
            data_length: data.len() as c_int,
            data: data.as_mut_ptr().cast(),
        };
        let auth = if name.is_empty() { ptr::null_mut() } else { &mut auth };
        // SAFETY: XCB consumes the connected fd. Auth fields stay live through this call;
        // the watchdog owns a duplicate socket and interrupts any blocked native setup.
        let pointer = unsafe { (api.xcb_connect_to_fd)(socket.into_raw_fd(), auth) };
        let connection = Connection(NonNull::new(pointer).ok_or(super::PLATFORM_E_BACKEND)?, watch, api);
        connection.health()?;
        backend(check_deadline(deadline))?;
        return Ok((connection, screen));
    }
    failed()
}

pub(super) fn refresh_deadline(connection: &Connection) {
    connection.set_deadline(Instant::now() + super::OPERATION_TIMEOUT);
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: The live Connection uniquely owns the xcb handle until this call.
        unsafe { (self.2.xcb_disconnect)(self.0.as_ptr()) };
    }
}
impl AsFd for Connection {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.1.state.socket.as_fd()
    }
}

const CW_EVENT_MASK: u32 = 1 << 11;

impl Connection {
    pub(super) fn set_deadline(&self, deadline: Instant) {
        self.1.arm(deadline);
    }

    pub(super) fn release_deadline(&self) {
        self.1.disarm();
    }

    fn health(&self) -> Result<(), NcStatus> {
        // SAFETY: A Connection owns its live libxcb handle.
        if unsafe { (self.2.xcb_connection_has_error)(self.0.as_ptr()) } == 0 {
            Ok(())
        } else {
            failed()
        }
    }

    fn check(&self, cookie: Cookie) -> Result<(), NcStatus> {
        // SAFETY: The checked request and returned allocation belong to this connection.
        if Packet::new(unsafe { (self.2.xcb_request_check)(self.0.as_ptr(), cookie) }).is_some() {
            return failed();
        }
        self.health()
    }

    pub(super) fn root(&self, screen: usize) -> Result<Window, NcStatus> {
        // SAFETY: The checked live Connection retains the setup and screen data.
        unsafe {
            let mut screens = (self.2.xcb_setup_roots_iterator)((self.2.xcb_get_setup)(self.0.as_ptr()));
            if screen >= screens.remaining as usize {
                return failed();
            }
            for _ in 0..screen {
                (self.2.xcb_screen_next)(&mut screens);
            }
            Ok(*screens.data)
        }
    }

    pub(super) fn maximum_request_words(&self) -> u16 {
        // SAFETY: A live connection owns the complete protocol setup. This is the fixed X11 core header.
        let setup = unsafe { slice::from_raw_parts((self.2.xcb_get_setup)(self.0.as_ptr()).cast::<u8>(), 28) };
        u16::from_ne_bytes([setup[26], setup[27]])
    }

    /// Creates the unmapped InputOnly owner or requestor and subscribes to property changes.
    pub(super) fn create_selection_window(&self, root: Window) -> Result<Window, NcStatus> {
        // SAFETY: This checked connection owns its X resource ID range.
        let id = unsafe { (self.2.xcb_generate_id)(self.0.as_ptr()) };
        self.health()?;
        let mask = EventMask::PROPERTY_CHANGE.into();
        // SAFETY: The only mask bit selects the initialized event-mask word.
        self.check(unsafe {
            (self.2.xcb_create_window_checked)(
                self.0.as_ptr(),
                0,
                id,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY.into(),
                0,
                CW_EVENT_MASK,
                &mask,
            )
        })?;
        Ok(id)
    }

    /// Watches this requestor's property or destruction notifications for the current transfers.
    pub(super) fn change_window_attributes(&self, window: Window, events: EventMask) -> Result<(), NcStatus> {
        let mask = events.into();
        // SAFETY: The only mask bit selects the initialized event-mask word.
        self.check(unsafe { (self.2.xcb_change_window_attributes_checked)(self.0.as_ptr(), window, CW_EVENT_MASK, &mask) })
    }

    pub(super) fn atom(&self, name: &[u8]) -> Result<Atom, NcStatus> {
        // SAFETY: XCB copies exactly name.len() bytes before returning.
        let reply = unsafe {
            let cookie = (self.2.xcb_intern_atom)(self.0.as_ptr(), 0, name.len() as u16, name.as_ptr());
            (self.2.xcb_intern_atom_reply)(self.0.as_ptr(), cookie, ptr::null_mut())
        };
        let reply = Packet::new(reply).ok_or(super::PLATFORM_E_BACKEND)?;
        Ok(u32::from_ne_bytes(reply.header()[8..12].try_into().unwrap()))
    }

    pub(super) fn get_selection_owner(&self, selection: Atom) -> Result<Window, NcStatus> {
        // SAFETY: xcb owns the checked live connection. Returned reply ownership transfers here.
        let reply = unsafe {
            let cookie = (self.2.xcb_get_selection_owner)(self.0.as_ptr(), selection);
            (self.2.xcb_get_selection_owner_reply)(self.0.as_ptr(), cookie, ptr::null_mut())
        };
        let reply = Packet::new(reply).ok_or(super::PLATFORM_E_BACKEND)?;
        Ok(u32::from_ne_bytes(reply.header()[8..12].try_into().unwrap()))
    }

    pub(super) fn change_property8(&self, mode: PropMode, window: Window, property: Atom, type_: impl Into<Atom>, bytes: &[u8]) -> Result<(), NcStatus> {
        // SAFETY: format eight consumes exactly bytes.len() initialized bytes.
        self.check(unsafe {
            (self.2.xcb_change_property_checked)(
                self.0.as_ptr(),
                mode.into(),
                window,
                property,
                type_.into(),
                8,
                bytes.len() as u32,
                bytes.as_ptr().cast(),
            )
        })
    }

    pub(super) fn change_property32(&self, mode: PropMode, window: Window, property: Atom, type_: impl Into<Atom>, words: &[u32]) -> Result<(), NcStatus> {
        // SAFETY: format 32 consumes exactly words.len() initialized words in native byte order.
        self.check(unsafe {
            (self.2.xcb_change_property_checked)(
                self.0.as_ptr(),
                mode.into(),
                window,
                property,
                type_.into(),
                32,
                words.len() as u32,
                words.as_ptr().cast(),
            )
        })
    }

    pub(super) fn get_property(
        &self,
        delete: bool,
        window: Window,
        property: Atom,
        format: Atom,
        offset: u32,
        words: u32,
    ) -> Result<GetPropertyReply, NcStatus> {
        // SAFETY: The checked request restricts the reply to the caller's word budget.
        let reply = unsafe {
            let cookie = (self.2.xcb_get_property)(self.0.as_ptr(), u8::from(delete), window, property, format, offset, words);
            (self.2.xcb_get_property_reply)(self.0.as_ptr(), cookie, ptr::null_mut())
        };
        let reply = Packet::new(reply).ok_or(super::PLATFORM_E_BACKEND)?;
        Ok(backend(GetPropertyReply::try_parse(reply.reply(words)?))?.0)
    }

    pub(super) fn delete_property(&self, window: Window, property: Atom) -> Result<(), NcStatus> {
        // SAFETY: All arguments belong to the checked XCB connection.
        self.check(unsafe { (self.2.xcb_delete_property_checked)(self.0.as_ptr(), window, property) })
    }

    pub(super) fn set_selection_owner(&self, owner: Window, selection: Atom, time: Timestamp) -> Result<(), NcStatus> {
        // SAFETY: All arguments belong to the checked XCB connection.
        self.check(unsafe { (self.2.xcb_set_selection_owner_checked)(self.0.as_ptr(), owner, selection, time) })
    }

    pub(super) fn convert_selection(&self, requestor: Window, selection: Atom, target: Atom, property: Atom, time: Timestamp) -> Result<(), NcStatus> {
        // SAFETY: All arguments belong to the checked XCB connection.
        self.check(unsafe { (self.2.xcb_convert_selection_checked)(self.0.as_ptr(), requestor, selection, target, property, time) })
    }

    pub(super) fn send_event(&self, propagate: bool, destination: Window, mask: EventMask, event: SelectionNotifyEvent) -> Result<(), NcStatus> {
        let message = event.serialize();
        // SAFETY: The serialized SelectionNotify has the required 32 initialized bytes.
        self.check(unsafe { (self.2.xcb_send_event_checked)(self.0.as_ptr(), u8::from(propagate), destination, mask.into(), message.as_ptr()) })
    }

    pub(super) fn wait_for_event(&self) -> Result<Event, NcStatus> {
        // SAFETY: A returned packet is transferred from this checked connection.
        let event = Packet::new(unsafe { (self.2.xcb_wait_for_event)(self.0.as_ptr()) }).ok_or(super::PLATFORM_E_BACKEND)?;
        Event::from_packet(event)
    }

    pub(super) fn poll_for_event(&self) -> Result<Option<Event>, NcStatus> {
        // SAFETY: A returned packet is transferred from this checked connection.
        if let Some(event) = Packet::new(unsafe { (self.2.xcb_poll_for_event)(self.0.as_ptr()) }) {
            return Event::from_packet(event).map(Some);
        }
        self.health()?;
        Ok(None)
    }
}

/// Waits for a socket or command wake. An absent deadline parks an idle responder.
pub(super) fn wait(fds: &mut [PollFd<'_>], deadline: Option<Instant>) -> io::Result<()> {
    loop {
        let timeout = match deadline {
            Some(deadline) => {
                let left = deadline.checked_duration_since(Instant::now()).ok_or(io::ErrorKind::TimedOut)?;
                Some(Timespec {
                    tv_sec: left.as_secs() as i64,
                    tv_nsec: left.subsec_nanos().into(),
                })
            }
            None => None,
        };
        match poll(fds, timeout.as_ref()) {
            Ok(0) => return Err(io::ErrorKind::TimedOut.into()),
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => (),
            Err(error) => return Err(error.into()),
        }
    }
}

/// Exercises blocked libxcb setup with a peer which receives the setup and leaves it unanswered.
#[cfg(test)]
pub(super) fn verify_handshake_deadline() {
    use std::{io::Read, os::unix::net::UnixStream, time::Duration};
    let (socket, mut peer) = UnixStream::pair().unwrap();
    let socket = OwnedFd::from(socket);
    let api = Xcb::load().unwrap();
    let start = Instant::now();
    let interval = Duration::from_millis(100);
    let watch = Deadline::new(&socket, start + interval).unwrap();
    assert!(rustix::io::fcntl_getfd(&watch.state.socket).unwrap().contains(rustix::io::FdFlags::CLOEXEC));
    // SAFETY: The successfully connected local socket transfers ownership to libxcb.
    let pointer = unsafe { (api.xcb_connect_to_fd)(socket.into_raw_fd(), ptr::null_mut()) };
    let connection = Connection(NonNull::new(pointer).unwrap(), watch, api);
    let elapsed = start.elapsed();
    assert!(connection.health().is_err());
    assert!(elapsed >= interval && elapsed < Duration::from_secs(2), "{elapsed:?}");
    peer.set_nonblocking(true).unwrap();
    let mut setup = [0; 12];
    peer.read_exact(&mut setup).unwrap();
    assert!(matches!(setup[0], b'l' | b'B'), "libxcb sent real setup before waiting");
}
