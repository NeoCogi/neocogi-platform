//! X11 text selection independent of the SDL video thread and visible windows.
mod connection;

use super::{MAX_CLIPBOARD_BYTES, PLATFORM_E_BACKEND};
use connection::{Connection, Event, open, refresh_deadline, wait};
use neocogi_com_sdk::{NC_E_OUT_OF_RANGE, NcStatus};
use rustix::{
    event::{PollFd, PollFlags},
    fd::OwnedFd,
    net::{Shutdown, shutdown},
};
use std::{
    cell::RefCell,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use x11rb_protocol::protocol::xproto::{
    Atom, AtomEnum, EventMask, GetPropertyReply, PropMode, Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Timestamp, Window,
};
const CURRENT_TIME: Timestamp = 0;
const NONE: u32 = 0;

/// Absolute socket budget for a native read, write, or small responder round trip.
/// OS resolution and Xauthority file preparation can delay opening a connection.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(2);
/// Finite lifetime of a peer's incremental transfer, even if it stops requesting data.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(5);
/// A transfer retains at most one immutable 1 MiB payload while the selection changes.
const MAX_TRANSFERS: usize = 8;
/// Bounds target negotiation, including properties in a MULTIPLE request.
const MAX_TARGET_ATOMS: usize = 128;
/// Limit each property write, even on servers which offer much larger requests.
const MAX_CHUNK_BYTES: usize = 64 * 1024;

macro_rules! atoms {
    ($($atom:ident : $name:expr,)*) => {
        #[allow(non_snake_case)]
        struct Atoms { $($atom: Atom,)* }

        impl Atoms {
            fn new(connection: &Connection) -> Result<Self, NcStatus> {
                Ok(Self { $($atom: connection.atom($name)?,)* })
            }
        }
    };
}
atoms! {
    CLIPBOARD: b"CLIPBOARD",
    TARGETS: b"TARGETS",
    TIMESTAMP: b"TIMESTAMP",
    MULTIPLE: b"MULTIPLE",
    ATOM_PAIR: b"ATOM_PAIR",
    INCR: b"INCR",
    UTF8_STRING: b"UTF8_STRING",
    TEXT: b"TEXT",
    COMPOUND_TEXT: b"COMPOUND_TEXT",
    UTF8_MIME: b"text/plain;charset=utf-8",
    UTF8_MIME_UPPER: b"text/plain;charset=UTF-8",
    PLAIN_TEXT: b"text/plain",
    NEOCOGI_TEXT_SELECTION: b"NEOCOGI_TEXT_SELECTION",
    NEOCOGI_SELECTION_TIME: b"NEOCOGI_SELECTION_TIME",
}

/// Retained clipboard service for one SDL-selected X11 display.
pub(in crate::platform) struct X11Clipboard {
    /// Captured display name lives independently of the SDL connection.
    display: String,
    /// Caller-owned handle, initialized only for the first publication.
    responder: RefCell<Option<Responder>>,
}

impl X11Clipboard {
    /// Captures display identity without opening a connection or starting a thread.
    pub(in crate::platform) fn new(display: String) -> Self {
        Self { display, responder: RefCell::new(None) }
    }

    /// Gets current text on a transient requestor connection with one absolute deadline.
    pub(super) fn read(&self) -> Result<String, NcStatus> {
        SelectionWindow::new(&self.display, Instant::now() + OPERATION_TIMEOUT)?.read()
    }

    /// Publishes accepted UTF-8 only after the responder confirms real X ownership.
    pub(super) fn write(&self, text: &str) -> Result<(), NcStatus> {
        if text.len() > MAX_CLIPBOARD_BYTES {
            return Err(NC_E_OUT_OF_RANGE);
        }
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        let mut responder = self.responder.borrow_mut();
        if responder.is_none() {
            *responder = Some(Responder::start(&self.display, deadline)?);
        }
        match responder.as_mut().expect("started responder").publish(text, deadline) {
            Ok(()) => Ok(()),
            Err(PublicationFailure::Unstarted) => failed(),
            Err(PublicationFailure::Retire) => {
                // A timed-out ownership change is ambiguous until the native owner is retired.
                // Wake, disconnect, and join before reporting failure or accepting another write.
                drop(responder.take());
                failed()
            }
        }
    }
}

/// Maps native/protocol errors to the same receipt as other native clipboard failures.
fn backend<T, E>(result: Result<T, E>) -> Result<T, NcStatus> {
    result.map_err(|_| PLATFORM_E_BACKEND)
}

/// An unavailable or invalid native clipboard result.
fn failed<T>() -> Result<T, NcStatus> {
    Err(PLATFORM_E_BACKEND)
}

/// A private unmapped X window with no SDL object, input route, or graphics context.
struct SelectionWindow {
    /// Reader or writer's independently owned X connection.
    connection: Connection,
    /// Unmapped window serving as selection owner or requestor.
    id: Window,
    /// Protocol names interned on this window's server.
    atoms: Atoms,
}

impl SelectionWindow {
    /// Initializes a requestor or responder window on its independently owned connection.
    fn new(display: &str, deadline: Instant) -> Result<Self, NcStatus> {
        let (connection, screen) = open(display, deadline)?;
        let root = connection.root(screen)?;
        let id = connection.create_selection_window(root)?;
        let atoms = Atoms::new(&connection)?;
        Ok(Self { connection, id, atoms })
    }

    /// Queries real server ownership, including selection replacement by another desktop client.
    fn owner(&self) -> Result<Window, NcStatus> {
        self.connection.get_selection_owner(self.atoms.CLIPBOARD)
    }

    /// Generates a PropertyNotify carrying the X server's timestamp.
    fn request_timestamp(&self) -> Result<(), NcStatus> {
        self.connection
            .change_property8(PropMode::REPLACE, self.id, self.atoms.NEOCOGI_SELECTION_TIME, AtomEnum::INTEGER, &[0])
    }

    /// Selects the timestamp event for our own property.
    fn timestamp(&self, event: &Event) -> Option<Timestamp> {
        match event {
            Event::PropertyNotify(event)
                if event.window == self.id
                    && event.atom == self.atoms.NEOCOGI_SELECTION_TIME
                    && event.state == Property::NEW_VALUE
                    && event.response_type & 0x80 == 0 =>
            {
                Some(event.time)
            }
            _ => None,
        }
    }

    /// Waits for an event under the current connection deadline.
    fn event(&self) -> Result<Event, NcStatus> {
        backend(self.connection.wait_for_event())
    }

    /// Requests one conversion and consumes its matching reply under the caller's deadline.
    fn convert(&self, target: Atom, time: Timestamp, bytes: usize) -> Result<Option<GetPropertyReply>, NcStatus> {
        let property = self.atoms.NEOCOGI_TEXT_SELECTION;
        self.connection.delete_property(self.id, property)?;
        self.connection.convert_selection(self.id, self.atoms.CLIPBOARD, target, property, time)?;
        loop {
            if let Event::SelectionNotify(event) = self.event()?
                && event.requestor == self.id
                && event.selection == self.atoms.CLIPBOARD
                && event.target == target
                && event.time == time
            {
                return match event.property {
                    NONE => Ok(None),
                    reply if reply == property => self.property(self.id, property, true, bytes).map(Some),
                    _ => failed(),
                };
            }
        }
    }

    /// Caps the X server's reply before x11rb allocates its property payload.
    fn property(&self, window: Window, property: Atom, delete: bool, bytes: usize) -> Result<GetPropertyReply, NcStatus> {
        let words = bytes.div_ceil(4) as u32;
        let reply = self.connection.get_property(delete, window, property, AtomEnum::ANY.into(), 0, words)?;
        if reply.bytes_after != 0 || reply.value.len() > bytes {
            return Err(NC_E_OUT_OF_RANGE);
        }
        Ok(reply)
    }

    /// Negotiates text without mistaking an unresponsive owner for an empty selection.
    fn read(&self) -> Result<String, NcStatus> {
        if self.owner()? == NONE {
            return Ok(String::new());
        }
        self.request_timestamp()?;
        let time = loop {
            if let Some(time) = self.timestamp(&self.event()?) {
                break time;
            }
        };
        let targets = self.convert(self.atoms.TARGETS, time, MAX_TARGET_ATOMS * 4)?;
        let choices = self.atoms.text_targets();
        if let Some(targets) = targets {
            if targets.type_ != u32::from(AtomEnum::ATOM) {
                return failed();
            }
            let targets: Vec<_> = targets.value32().ok_or(PLATFORM_E_BACKEND)?.collect();
            let Some(target) = choices.into_iter().find(|target| targets.contains(target)) else {
                return if targets.contains(&self.atoms.COMPOUND_TEXT) {
                    failed()
                } else {
                    Ok(String::new())
                };
            };
            let reply = self.convert(target, time, MAX_CLIPBOARD_BYTES)?.ok_or(PLATFORM_E_BACKEND)?;
            return self.read_text(reply, target);
        }
        // Some older owners refuse TARGETS yet still implement individual text conversions.
        // An actual timeout or failure to read does not enter this fallback.
        for target in choices {
            if let Some(reply) = self.convert(target, time, MAX_CLIPBOARD_BYTES)? {
                return self.read_text(reply, target);
            }
        }
        Ok(String::new())
    }

    /// Reassembles a complete text response and decodes only the negotiated text property.
    fn read_text(&self, reply: GetPropertyReply, target: Atom) -> Result<String, NcStatus> {
        if reply.type_ != self.atoms.INCR {
            let encoding = self.atoms.encoding(target, &reply)?;
            return encoding.decode(reply.value);
        }
        let mut length = reply.value32().ok_or(PLATFORM_E_BACKEND)?;
        let lower_bound = length.next().ok_or(PLATFORM_E_BACKEND)? as usize;
        if length.next().is_some() {
            return failed();
        }
        if lower_bound > MAX_CLIPBOARD_BYTES {
            return Err(NC_E_OUT_OF_RANGE);
        }

        let mut bytes = Vec::new();
        let mut encoding = None;
        let property = self.atoms.NEOCOGI_TEXT_SELECTION;
        loop {
            let Event::PropertyNotify(event) = self.event()? else { continue };
            if event.window != self.id || event.atom != property || event.state != Property::NEW_VALUE {
                continue;
            }
            let chunk = self.property(self.id, property, true, MAX_CLIPBOARD_BYTES - bytes.len())?;
            if chunk.type_ == NONE {
                continue;
            }
            let chunk_encoding = self.atoms.encoding(target, &chunk)?;
            if encoding.replace(chunk_encoding).is_some_and(|previous| previous != chunk_encoding) {
                return failed();
            }
            if chunk.value.is_empty() {
                // The ICCCM permits a lower bound in the INCR marker.
                if bytes.len() < lower_bound {
                    return failed();
                }
                return chunk_encoding.decode(bytes);
            }
            bytes.extend_from_slice(&chunk.value);
        }
    }
}

/// Encoding of a concrete text property after target negotiation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TextEncoding {
    Utf8,
    Latin1,
}

impl TextEncoding {
    /// Returns only complete normalized UTF-8 within the public size limit.
    fn decode(self, bytes: Vec<u8>) -> Result<String, NcStatus> {
        match self {
            Self::Utf8 => backend(String::from_utf8(bytes)),
            Self::Latin1 => {
                let length = bytes.len() + bytes.iter().filter(|byte| **byte >= 128).count();
                if length > MAX_CLIPBOARD_BYTES {
                    return Err(NC_E_OUT_OF_RANGE);
                }
                let mut text = String::with_capacity(length);
                text.extend(bytes.into_iter().map(char::from));
                Ok(text)
            }
        }
    }
}

impl Atoms {
    /// Prefers explicit UTF-8 targets and then legacy text encodings.
    fn text_targets(&self) -> [Atom; 6] {
        [
            self.UTF8_STRING,
            self.UTF8_MIME,
            self.UTF8_MIME_UPPER,
            self.PLAIN_TEXT,
            self.TEXT,
            AtomEnum::STRING.into(),
        ]
    }

    /// Checks the actual property type and its wire width before decoding.
    fn encoding(&self, target: Atom, reply: &GetPropertyReply) -> Result<TextEncoding, NcStatus> {
        if reply.format != 8 || (target != self.TEXT && target != self.PLAIN_TEXT && reply.type_ != target) {
            return failed();
        }
        if reply.type_ == AtomEnum::STRING.into() {
            Ok(TextEncoding::Latin1)
        } else if self.text_targets()[..4].contains(&reply.type_) {
            Ok(TextEncoding::Utf8)
        } else {
            failed()
        }
    }
}

/// One write crosses the responder boundary with owned bytes and a private receipt.
struct Publish {
    /// Immutable UTF-8 selection sent when the write becomes current.
    text: Arc<[u8]>,
    /// An optional lossless legacy X STRING representation.
    latin1: Option<Arc<[u8]>>,
    /// Applies to queueing, preparation, and ownership confirmation together.
    deadline: Instant,
    /// Either the worker claims the write or the timed-out caller cancels it.
    claimed: Arc<AtomicBool>,
    /// Delivers the native result to the caller without holding a COM reference.
    receipt: SyncSender<Result<(), PublicationFailure>>,
}

/// Indicates whether an unsuccessful write might already have changed ownership.
enum PublicationFailure {
    /// No native change started. The previous selection can continue serving.
    Unstarted,
    /// Disconnect the responder before reporting failure to retire any unresolved change.
    Retire,
}

/// Caller-owned endpoint for a private selection responder.
struct Responder {
    /// At most one publication waits for the worker to claim it.
    commands: SyncSender<Publish>,
    /// Wakes the worker when a publication is queued while X events are idle.
    wake: UnixStream,
    /// Interrupts the worker's socket before joining or reporting ambiguous failure.
    socket: OwnedFd,
    /// Joined on the caller thread before releasing native provider code.
    worker: Option<JoinHandle<()>>,
}

impl Responder {
    /// Finishes native setup before publishing a worker handle to the retained capability.
    fn start(display: &str, deadline: Instant) -> Result<Self, NcStatus> {
        let window = SelectionWindow::new(display, deadline)?;
        let socket = backend(rustix::io::fcntl_dupfd_cloexec(&window.connection, 0))?;
        let (commands, pending) = mpsc::sync_channel(1);
        let (wake, wake_reader) = backend(UnixStream::pair())?;
        backend(wake.set_nonblocking(true))?;
        backend(wake_reader.set_nonblocking(true))?;
        let worker = backend(thread::Builder::new().name("neocogi-x11-clipboard".to_owned()).spawn(move || {
            SelectionOwner {
                window,
                selection: None,
                transfers: Vec::new(),
            }
            .run(pending, wake_reader);
        }))?;
        Ok(Self {
            commands,
            wake,
            socket,
            worker: Some(worker),
        })
    }

    /// Sends a single bounded write and waits for confirmation from the checked selection owner.
    fn publish(&mut self, text: &str, deadline: Instant) -> Result<(), PublicationFailure> {
        let (receipt, result) = mpsc::sync_channel(1);
        let claimed = Arc::new(AtomicBool::new(false));
        let utf8: Arc<[u8]> = Arc::from(text.as_bytes());
        let latin1 = if text.is_ascii() {
            Some(utf8.clone())
        } else {
            text.chars()
                .map(|scalar| u8::try_from(u32::from(scalar)).ok())
                .collect::<Option<Vec<_>>>()
                .map(Arc::from)
        };
        self.commands
            .try_send(Publish {
                text: utf8,
                latin1,
                deadline,
                claimed: claimed.clone(),
                receipt,
            })
            .map_err(|error| match error {
                TrySendError::Full(_) => PublicationFailure::Unstarted,
                TrySendError::Disconnected(_) => PublicationFailure::Retire,
            })?;
        if self.wake.write_all(&[0]).is_err_and(|error| error.kind() != io::ErrorKind::WouldBlock) {
            return Err(PublicationFailure::Retire);
        }
        match result.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) if !claimed.swap(true, Ordering::AcqRel) => Err(PublicationFailure::Unstarted),
            Err(_) => Err(PublicationFailure::Retire),
        }
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        // This duplicate interrupts a blocked socket operation before joining. The worker owns
        // the X connection itself. There are no callbacks or SDL references on its stack.
        let _ = shutdown(&self.socket, Shutdown::Both);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Current confirmed selection and its encoding choices.
struct Selection {
    /// Server timestamp used to confirm this specific ownership generation.
    time: Timestamp,
    /// Complete UTF-8 selection owned until another publication or selection loss.
    text: Arc<[u8]>,
    /// Lossless bytes when the current selection can also serve the legacy STRING target.
    latin1: Option<Arc<[u8]>>,
}

/// Incremental transfer retaining the exact selection given to a single requester.
struct Transfer {
    /// External window receiving each property.
    requester: Window,
    /// Destination property whose deletion permits the next chunk.
    property: Atom,
    /// Concrete X11 property type for every chunk and its terminator.
    format: Atom,
    /// Retained selection generation and encoding for this peer.
    data: Arc<[u8]>,
    /// First byte waiting for a property deletion acknowledgement.
    offset: usize,
    /// Absolute deadline releasing abandoned transfer snapshots.
    deadline: Instant,
}

/// The sole owner of publication and in-flight transfers on the worker thread.
struct SelectionOwner {
    /// Private owner window and all worker-side native I/O.
    window: SelectionWindow,
    /// Latest server-confirmed selection, independently of older transfers.
    selection: Option<Selection>,
    /// At most MAX_TRANSFERS accepted incremental peers.
    transfers: Vec<Transfer>,
}

impl Drop for SelectionOwner {
    fn drop(&mut self) {
        // Release the server's selection even if the caller still holds its wake-up descriptor.
        let _ = shutdown(&self.window.connection, Shutdown::Both);
    }
}

impl SelectionOwner {
    /// Serves X11 even while the application is idle or blocked in a separate read.
    fn run(mut self, pending: Receiver<Publish>, mut wake: UnixStream) {
        loop {
            // The command may have arrived before its wake byte. Always drain the wake so
            // an already handled command cannot keep an idle responder's poll readable.
            let _ = wake.read(&mut [0]);
            match pending.try_recv() {
                Ok(command) => {
                    if Instant::now() >= command.deadline || command.claimed.swap(true, Ordering::AcqRel) {
                        let _ = command.receipt.send(Err(PublicationFailure::Unstarted));
                        continue;
                    }
                    if self.publish(&command).is_err() {
                        // Release the owner before reporting a write whose outcome is uncertain.
                        drop(self);
                        let _ = command.receipt.send(Err(PublicationFailure::Retire));
                        return;
                    }
                    let _ = command.receipt.send(Ok(()));
                }
                Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => (),
            }
            refresh_deadline(&self.window.connection);
            self.expire();
            match self.window.connection.poll_for_event() {
                Ok(Some(event)) => {
                    if self.handle(event).is_err() {
                        return;
                    }
                }
                Ok(None) => {
                    self.window.connection.release_deadline();
                    let mut sockets = [PollFd::new(&self.window.connection, PollFlags::IN), PollFd::new(&wake, PollFlags::IN)];
                    let deadline = self.transfers.iter().map(|transfer| transfer.deadline).min();
                    match wait(&mut sockets, deadline) {
                        Ok(()) => (),
                        Err(error) if error.kind() == io::ErrorKind::TimedOut => (),
                        Err(_) => return,
                    }
                }
                Err(_) => return,
            }
        }
    }

    /// Acquires actual X ownership using server time, then publishes the immutable payload.
    fn publish(&mut self, command: &Publish) -> Result<(), NcStatus> {
        self.window.connection.set_deadline(command.deadline);
        self.window.request_timestamp()?;
        let time = loop {
            let event = self.window.event()?;
            if let Some(time) = self.window.timestamp(&event) {
                break time;
            }
            self.handle(event)?;
        };
        self.window.connection.set_selection_owner(self.window.id, self.window.atoms.CLIPBOARD, time)?;
        if self.window.owner()? != self.window.id || Instant::now() >= command.deadline {
            return failed();
        }
        self.selection = Some(Selection {
            time,
            text: command.text.clone(),
            latin1: command.latin1.clone(),
        });
        Ok(())
    }

    /// Dispatches only messages pertaining to this owner or one of its active peers.
    fn handle(&mut self, event: Event) -> Result<(), NcStatus> {
        match event {
            Event::SelectionRequest(request) => self.request(request),
            Event::SelectionClear(event) if event.owner == self.window.id && event.selection == self.window.atoms.CLIPBOARD => {
                if self.selection.as_ref().is_some_and(|selection| !precedes(event.time, selection.time)) && self.window.owner()? != self.window.id {
                    // An already acknowledged incremental transfer retains its old bytes.
                    self.selection = None;
                }
            }
            Event::PropertyNotify(event) if event.state == Property::DELETE && event.response_type & 0x80 == 0 => {
                if let Some(index) = self
                    .transfers
                    .iter()
                    .position(|transfer| transfer.requester == event.window && transfer.property == event.atom)
                {
                    self.advance(index);
                }
            }
            Event::DestroyNotify(event) => self.transfers.retain(|transfer| transfer.requester != event.window),
            _ => (),
        }
        Ok(())
    }

    /// Notifies a requester only after its property was written successfully.
    fn request(&mut self, request: SelectionRequestEvent) {
        let atoms = &self.window.atoms;
        let relevant = request.owner == self.window.id && request.selection == atoms.CLIPBOARD;
        let supported = self.selection.as_ref().is_some_and(|selection| !precedes(request.time, selection.time));
        let property = if request.property == NONE { request.target } else { request.property };
        let accepted = relevant
            && supported
            && if request.target == atoms.MULTIPLE {
                request.property != NONE && self.multiple(request.requestor, property)
            } else {
                self.convert(request.requestor, request.target, property)
            };
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            time: request.time,
            property: if accepted { property } else { NONE },
        };
        // A peer can disappear between property transfer and acknowledgement.
        let _ = self.window.connection.send_event(false, request.requestor, EventMask::NO_EVENT, notify);
    }

    /// A bounded list of distinct destination properties, with failed conversions marked None.
    fn multiple(&mut self, requester: Window, property: Atom) -> bool {
        let Ok(reply) = self.window.property(requester, property, false, MAX_TARGET_ATOMS * 4) else {
            return false;
        };
        if reply.type_ != self.window.atoms.ATOM_PAIR {
            return false;
        }
        let Some(pairs) = reply.value32() else { return false };
        let mut pairs: Vec<_> = pairs.collect();
        if !pairs.len().is_multiple_of(2) {
            return false;
        }
        let mut destinations = Vec::new();
        for pair in pairs.chunks_exact_mut(2) {
            let [target, destination] = pair else { unreachable!("an exact atom pair") };
            if *target == self.window.atoms.MULTIPLE
                || *destination == NONE
                || *destination == property
                || destinations.contains(destination)
                || !self.convert(requester, *target, *destination)
            {
                *destination = NONE;
            } else {
                destinations.push(*destination);
            }
        }
        self.window
            .connection
            .change_property32(PropMode::REPLACE, requester, property, self.window.atoms.ATOM_PAIR, &pairs)
            .is_ok()
    }

    /// Writes metadata directly and chooses a transferable encoding for text.
    fn convert(&mut self, requester: Window, target: Atom, property: Atom) -> bool {
        if requester == self.window.id
            || self
                .transfers
                .iter()
                .any(|transfer| transfer.requester == requester && transfer.property == property)
        {
            return false;
        }
        let Some(selection) = &self.selection else { return false };
        let atoms = &self.window.atoms;
        if target == atoms.TARGETS {
            let mut targets = vec![atoms.TARGETS, atoms.TIMESTAMP, atoms.MULTIPLE];
            targets.extend(
                atoms
                    .text_targets()
                    .into_iter()
                    .filter(|target| *target != AtomEnum::STRING.into() || selection.latin1.is_some()),
            );
            return self
                .window
                .connection
                .change_property32(PropMode::REPLACE, requester, property, AtomEnum::ATOM, &targets)
                .is_ok();
        }
        if target == atoms.TIMESTAMP {
            return self
                .window
                .connection
                .change_property32(PropMode::REPLACE, requester, property, AtomEnum::INTEGER, &[selection.time])
                .is_ok();
        }
        let (format, data) = if target == AtomEnum::STRING.into() {
            let Some(latin1) = &selection.latin1 else { return false };
            (target, latin1.clone())
        } else if atoms.text_targets()[..5].contains(&target) {
            let format = if target == atoms.TEXT { atoms.UTF8_STRING } else { target };
            (format, selection.text.clone())
        } else {
            return false;
        };
        if data.len() <= self.chunk_bytes() {
            return self.write_property(requester, property, format, &data).is_ok();
        }
        if self.transfers.len() >= MAX_TRANSFERS {
            return false;
        }
        self.begin_transfer(requester, property, format, data).is_ok()
    }

    /// Selects requestor property events before advertising a bounded incremental transfer.
    fn begin_transfer(&mut self, requester: Window, property: Atom, format: Atom, data: Arc<[u8]>) -> Result<(), NcStatus> {
        self.watch(requester, EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY)?;
        let marker = self
            .window
            .connection
            .change_property32(PropMode::REPLACE, requester, property, self.window.atoms.INCR, &[data.len() as u32]);
        if marker.is_err() {
            self.unwatch(requester);
            return marker;
        }
        self.transfers.push(Transfer {
            requester,
            property,
            format,
            data,
            offset: 0,
            deadline: Instant::now() + TRANSFER_TIMEOUT,
        });
        Ok(())
    }

    /// Emits the next owned chunk only after the peer deletes the preceding property.
    fn advance(&mut self, index: usize) {
        let transfer = &self.transfers[index];
        let end = (transfer.offset + self.chunk_bytes()).min(transfer.data.len());
        let result = self.write_property(transfer.requester, transfer.property, transfer.format, &transfer.data[transfer.offset..end]);
        if result.is_err() || end == transfer.offset {
            let transfer = self.transfers.swap_remove(index);
            self.unwatch(transfer.requester);
        } else {
            self.transfers[index].offset = end;
        }
    }

    /// Removes expired peers even when none of them sends another event.
    fn expire(&mut self) {
        let now = Instant::now();
        let mut index = 0;
        while index < self.transfers.len() {
            if self.transfers[index].deadline <= now {
                let transfer = self.transfers.swap_remove(index);
                self.unwatch(transfer.requester);
            } else {
                index += 1;
            }
        }
    }

    /// Fits each chunk inside one core ChangeProperty request.
    fn chunk_bytes(&self) -> usize {
        let server_limit = usize::from(self.window.connection.maximum_request_words()) * 4;
        server_limit.saturating_sub(32).clamp(1, MAX_CHUNK_BYTES)
    }

    /// Writes a checked 8-bit text property on the requesting window.
    fn write_property(&self, requester: Window, property: Atom, format: Atom, data: &[u8]) -> Result<(), NcStatus> {
        self.window.connection.change_property8(PropMode::REPLACE, requester, property, format, data)
    }

    /// Changes only this connection's interest in requestor events.
    fn watch(&self, requester: Window, events: EventMask) -> Result<(), NcStatus> {
        self.window.connection.change_window_attributes(requester, events)
    }

    /// Stops observing a requestor once its last transfer has ended.
    fn unwatch(&self, requester: Window) {
        if !self.transfers.iter().any(|transfer| transfer.requester == requester) {
            let _ = self.watch(requester, EventMask::NO_EVENT);
        }
    }
}

/// Compares timestamps across X11's 32-bit wraparound.
fn precedes(value: Timestamp, reference: Timestamp) -> bool {
    value != CURRENT_TIME && (value.wrapping_sub(reference) as i32) < 0
}

#[cfg(test)]
mod tests;
