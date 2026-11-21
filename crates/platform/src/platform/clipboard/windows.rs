//! Checked Win32 clipboard operations on the SDL thread.

use super::{MAX_CLIPBOARD_BYTES, PLATFORM_E_BACKEND, gpu};
use neocogi_com_sdk::{NC_E_OUT_OF_RANGE, NcStatus};
use sdl2::sys;
use std::{ffi::c_void, ptr::NonNull, rc::Rc, time::Duration};
use windows_sys::Win32::{
    Foundation::{ERROR_SUCCESS, GetLastError, GlobalFree, HANDLE, HGLOBAL, HWND, SetLastError},
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData},
        Diagnostics::Debug::ReadProcessMemory,
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        Ole::CF_UNICODETEXT,
        Threading::GetCurrentProcess,
    },
};

/// Win32 format used in this adapter's reads and writes.
const TEXT_FORMAT: u32 = CF_UNICODETEXT as u32;
/// Match SDL2's short contention retry policy.
const OPEN_ATTEMPTS: usize = 3;
/// Pause only between open attempts.
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(10);

/// Native SDL2 window info. The pregenerated sdl2-sys union only names Linux members.
#[repr(C)]
struct WindowInfo {
    /// SDL version supported by the caller.
    version: sys::SDL_version,
    /// Discriminant filled in by the current video driver.
    subsystem: sys::SDL_SYSWM_TYPE,
    /// Driver-specific storage from SDL_syswm.h.
    info: WindowInfoPayload,
}

/// Fixed-size SDL2 SysWM union on Windows. The HWND is first in its Win32 member.
#[repr(C)]
union WindowInfoPayload {
    /// HWND furnished only when SDL reports SDL_SYSWM_WINDOWS.
    hwnd: HWND,
    /// Complete native storage even when this code needs only the first field.
    storage: [u8; 64],
}

/// Owns the private SDL window for every transaction on this thread.
pub(super) struct Win32Clipboard {
    /// Keeps the HWND and SDL video service alive independently of public routes.
    _owner: Rc<gpu::Window>,
    /// Nonnull HWND extracted from SDL's Win32 window-info record.
    hwnd: NonNull<c_void>,
    /// Replaces only the write operations needed to observe injected failures at the COM boundary.
    #[cfg(test)]
    write_calls: WriteCalls,
}

impl Win32Clipboard {
    /// Borrows the live Win32 handle without the SDL Rust wrapper's panic path.
    pub(super) fn new(owner: Rc<gpu::Window>) -> Result<Self, NcStatus> {
        let mut info = WindowInfo {
            version: sys::SDL_version {
                major: sys::SDL_MAJOR_VERSION as u8,
                minor: sys::SDL_MINOR_VERSION as u8,
                patch: sys::SDL_PATCHLEVEL as u8,
            },
            subsystem: sys::SDL_SYSWM_TYPE::SDL_SYSWM_UNKNOWN,
            info: WindowInfoPayload { storage: [0; 64] },
        };
        // SAFETY: The window is live and the cast supplies the writable Windows layout
        // defined by SDL_syswm.h. Read the HWND only if SDL populated the Win32 record.
        let hwnd = unsafe {
            if sys::SDL_GetWindowWMInfo(owner.raw(), std::ptr::from_mut(&mut info).cast()) != sys::SDL_bool::SDL_TRUE
                || info.subsystem != sys::SDL_SYSWM_TYPE::SDL_SYSWM_WINDOWS
            {
                return Err(PLATFORM_E_BACKEND);
            }
            NonNull::new(info.info.hwnd).ok_or(PLATFORM_E_BACKEND)?
        };
        Ok(Self {
            _owner: owner,
            hwnd,
            #[cfg(test)]
            write_calls: WriteCalls::NATIVE,
        })
    }

    /// Copies a text result and closes the native session before returning it.
    pub(super) fn read(&self) -> Result<String, NcStatus> {
        let session = Session::open(self.hwnd)?;
        let result = session.read_text();
        session.finish(result)
    }

    /// Prepares Unicode memory before opening or emptying the clipboard.
    pub(super) fn write(&self, text: &str) -> Result<(), NcStatus> {
        #[cfg(test)]
        let calls = self.write_calls;
        #[cfg(not(test))]
        let calls = WriteCalls::NATIVE;
        let units = (calls.encode)(text)?;
        let allocation = Allocation::new(&units, calls)?;
        let session = Session::open(self.hwnd)?;
        let result = session.publish(allocation, calls.publish);
        session.finish(result)
    }
}

/// Native write operations used at the conversion, copy and publication boundaries.
#[derive(Clone, Copy)]
struct WriteCalls {
    /// Translates validated UTF-8 into terminated Win32 text before allocation.
    encode: fn(&str) -> Result<Vec<u16>, NcStatus>,
    /// Allocates a fresh application-owned block or returns null on failure.
    allocate: unsafe extern "system" fn(u32, usize) -> HGLOBAL,
    /// Locks the block for the duration of the initialized payload copy.
    lock: unsafe extern "system" fn(HGLOBAL) -> *mut c_void,
    /// Frees an unlocked block on any path retaining application ownership.
    free: unsafe extern "system" fn(HGLOBAL) -> HGLOBAL,
    /// Transfers ownership only if the native call returns nonnull.
    publish: unsafe extern "system" fn(u32, HGLOBAL) -> HANDLE,
}

impl WriteCalls {
    /// Production operations use Win32 directly.
    const NATIVE: Self = Self {
        encode: encode_text,
        allocate: GlobalAlloc,
        lock: GlobalLock,
        free: GlobalFree,
        publish: SetClipboardData,
    };
}

/// Encodes the payload and ensures lone LF is represented as CRLF in Win32.
fn encode_text(text: &str) -> Result<Vec<u16>, NcStatus> {
    let mut units = Vec::new();
    let line_feeds = text.bytes().filter(|byte| *byte == b'\n').count();
    // The validated input is bounded and has no interior NUL. One UTF-8 byte
    // needs at most one UTF-16 unit, plus at most one inserted CR per LF.
    units.try_reserve_exact(text.len() + line_feeds + 1).map_err(|_| PLATFORM_E_BACKEND)?;
    let mut previous = 0;
    for unit in text.encode_utf16() {
        if unit == u16::from(b'\n') && previous != u16::from(b'\r') {
            units.push(u16::from(b'\r'));
        }
        units.push(unit);
        previous = unit;
    }
    units.push(0);
    Ok(units)
}

/// Exclusive native clipboard access on the calling thread.
struct Session {
    /// Holds the obligation to call CloseClipboard until a close succeeds.
    open: bool,
}

impl Session {
    /// Acquires the native clipboard for a live private HWND with bounded retries.
    fn open(hwnd: NonNull<c_void>) -> Result<Self, NcStatus> {
        for attempt in 0..OPEN_ATTEMPTS {
            // SAFETY: The caller retains the owning SDL window for this entire transaction.
            if unsafe { OpenClipboard(hwnd.as_ptr()) } != 0 {
                return Ok(Self { open: true });
            }
            if attempt + 1 < OPEN_ATTEMPTS {
                std::thread::sleep(OPEN_RETRY_DELAY);
            }
        }
        Err(PLATFORM_E_BACKEND)
    }

    /// Reads only while the current thread holds the clipboard, treating absent text as empty.
    fn read_text(&self) -> Result<String, NcStatus> {
        // SAFETY: This session holds the clipboard and will close it after all text is copied.
        if unsafe { IsClipboardFormatAvailable(TEXT_FORMAT) } == 0 {
            return Ok(String::new());
        }
        // SAFETY: This format is backed by a clipboard-owned HGLOBAL. Borrow it while open.
        let handle = NonNull::new(unsafe { GetClipboardData(TEXT_FORMAT) }).ok_or(PLATFORM_E_BACKEND)?;
        // SAFETY: The HGLOBAL is still live and borrowed from this open session.
        let available = unsafe { GlobalSize(handle.as_ptr()) } / size_of::<u16>();
        if available == 0 {
            return Err(PLATFORM_E_BACKEND);
        }
        let lock = Lock::new(&handle, GlobalLock)?;
        // A payload within the UTF-8 limit cannot need more UTF-16 units than UTF-8 bytes.
        let scan = available.min(MAX_CLIPBOARD_BYTES + 1);
        let units = lock.copy_units(scan);
        lock.finish()?;
        decode_text(&units?, available > scan)
    }

    /// Empties the clipboard only after preparation and transfers the allocation on success.
    fn publish(&self, allocation: Allocation, publish: unsafe extern "system" fn(u32, HGLOBAL) -> HANDLE) -> Result<(), NcStatus> {
        // SAFETY: This thread holds the open clipboard with a live nonnull owner.
        if unsafe { EmptyClipboard() } == 0 {
            return Err(PLATFORM_E_BACKEND);
        }
        // SAFETY: This session is open and empty and the movable allocation is unlocked.
        if unsafe { publish(TEXT_FORMAT, allocation.handle.as_ptr()) }.is_null() {
            return Err(PLATFORM_E_BACKEND);
        }
        std::mem::forget(allocation);
        Ok(())
    }

    /// Closes the session before reporting the outcome. Drop retries a failed close.
    fn finish<T>(mut self, outcome: Result<T, NcStatus>) -> Result<T, NcStatus> {
        // SAFETY: The same thread owns this successful open.
        if unsafe { CloseClipboard() } != 0 {
            self.open = false;
            outcome
        } else {
            Err(PLATFORM_E_BACKEND)
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.open {
            // SAFETY: This thread still holds the matching successful open.
            unsafe { CloseClipboard() };
        }
    }
}

/// A global memory lock whose handle lives through the matching unlock.
struct Lock<'a> {
    /// The handle remains owned by the caller or its open clipboard session.
    handle: &'a NonNull<c_void>,
    /// The native address is valid only while the lock is held.
    data: NonNull<c_void>,
    /// One successful lock to release on every exit path.
    locked: bool,
}

impl<'a> Lock<'a> {
    /// Acquires a pointer to live HGLOBAL memory for a borrow of the handle.
    fn new(handle: &'a NonNull<c_void>, lock: unsafe extern "system" fn(HGLOBAL) -> *mut c_void) -> Result<Self, NcStatus> {
        // SAFETY: The handle is owned by an Allocation or borrowed from the active clipboard.
        let data = NonNull::new(unsafe { lock(handle.as_ptr()) }).ok_or(PLATFORM_E_BACKEND)?;
        Ok(Self { handle, data, locked: true })
    }

    /// Reads a bounded native byte range into owned initialized UTF-16 storage.
    fn copy_units(&self, count: usize) -> Result<Vec<u16>, NcStatus> {
        let mut units = Vec::new();
        units.try_reserve_exact(count).map_err(|_| PLATFORM_E_BACKEND)?;
        units.resize(count, 0u16);
        let bytes = size_of_val(units.as_slice());
        let mut copied = 0;
        // SAFETY: The source is locked and bounded by GlobalSize. This native copy
        // checks memory access without borrowing foreign allocation padding as a Rust slice.
        let valid = unsafe { ReadProcessMemory(GetCurrentProcess(), self.data.as_ptr(), units.as_mut_ptr().cast(), bytes, &mut copied) };
        if valid == 0 || copied != bytes {
            return Err(PLATFORM_E_BACKEND);
        }
        Ok(units)
    }

    /// Releases this lock before the caller closes or publishes the global allocation.
    fn finish(mut self) -> Result<(), NcStatus> {
        // SAFETY: This guard holds exactly one successful lock of this live handle.
        let unlocked = unsafe {
            SetLastError(ERROR_SUCCESS);
            GlobalUnlock(self.handle.as_ptr()) != 0 || GetLastError() == ERROR_SUCCESS
        };
        if !unlocked {
            return Err(PLATFORM_E_BACKEND);
        }
        self.locked = false;
        Ok(())
    }
}

impl Drop for Lock<'_> {
    fn drop(&mut self) {
        if self.locked {
            // SAFETY: This guard still owns its matching lock.
            unsafe { GlobalUnlock(self.handle.as_ptr()) };
        }
    }
}

/// An unlocked movable block owned by the application until successful publication.
struct Allocation {
    /// Nonnull movable global memory handle.
    handle: NonNull<c_void>,
    /// Releases this exact allocation if publication did not transfer it.
    free: unsafe extern "system" fn(HGLOBAL) -> HGLOBAL,
}

impl Allocation {
    /// Prepares a complete, terminated UTF-16 value before changing the clipboard.
    fn new(units: &[u16], calls: WriteCalls) -> Result<Self, NcStatus> {
        // SAFETY: The bounded payload has a nonzero representable object size.
        let handle = NonNull::new(unsafe { (calls.allocate)(GMEM_MOVEABLE, size_of_val(units)) }).ok_or(PLATFORM_E_BACKEND)?;
        let allocation = Self { handle, free: calls.free };
        let lock = Lock::new(&allocation.handle, calls.lock)?;
        // SAFETY: This fresh allocation is aligned and owns at least units.len() UTF-16 slots.
        unsafe { std::ptr::copy_nonoverlapping(units.as_ptr(), lock.data.as_ptr().cast(), units.len()) };
        lock.finish()?;
        Ok(allocation)
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: The application still owns this allocation. Its lock was released first.
        unsafe { (self.free)(self.handle.as_ptr()) };
    }
}

/// Decodes terminated text while preserving native line endings and bounding the UTF-8 result.
fn decode_text(units: &[u16], truncated_scan: bool) -> Result<String, NcStatus> {
    let Some(end) = units.iter().position(|unit| *unit == 0) else {
        return Err(if truncated_scan { NC_E_OUT_OF_RANGE } else { PLATFORM_E_BACKEND });
    };
    let text = String::from_utf16(&units[..end]).map_err(|_| PLATFORM_E_BACKEND)?;
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err(NC_E_OUT_OF_RANGE);
    }
    Ok(text)
}

#[cfg(test)]
mod tests;
