//! Platform clipboard transfer with copied completion facts and no retained UI borrows.
use super::*;

#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
pub(super) use x11::X11Clipboard;

/// The driver selects clipboard capabilities independently of the renderer backend.
#[cfg(not(target_arch = "wasm32"))]
enum Backend {
    /// SDL video lifetime for desktop drivers using SDL's native clipboard receipts.
    Sdl(sdl2::clipboard::ClipboardUtil),
    /// SDL2 Wayland reads depend on a focused public input route. Writes are unavailable.
    Wayland {
        /// Keeps SDL video live independently of the public window.
        service: sdl2::clipboard::ClipboardUtil,
        /// Observes the current public route without keeping the host or window alive.
        host: Weak<NativeHost>,
    },
    /// Independent X11 responder and requestor connections shared by the host's projections.
    #[cfg(target_os = "linux")]
    X11(Rc<X11Clipboard>),
}

#[com_object(interfaces(ITextClipboard))]
/// Delegable clipboard with no renderer, factory, or window interface on its COM identity.
pub(super) struct Clipboard {
    /// Keeps the selected native clipboard service alive after window/root release.
    #[cfg(not(target_arch = "wasm32"))]
    backend: Backend,
    /// SDL2's Win32 writes require a live HWND even after the last public window goes away.
    #[cfg(windows)]
    _owner: Rc<gpu::Window>,
}

impl Clipboard {
    /// Retains clipboard services independently of the application window and graphics objects.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn for_host(host: &Rc<NativeHost>, _window: &gpu::Window) -> Result<ComPtr<ITextClipboardAbi>, NcStatus> {
        let backend = match host.video.current_video_driver() {
            #[cfg(target_os = "linux")]
            "x11" => Backend::X11(host.x11_clipboard(_window)?),
            "wayland" => Backend::Wayland {
                service: host.video.clipboard(),
                host: Rc::downgrade(host),
            },
            _ => Backend::Sdl(host.video.clipboard()),
        };
        Self::from_value(Self {
            backend,
            #[cfg(windows)]
            _owner: host.clipboard_window()?,
        })
        .query()
    }

    /// Browser clipboard operations resolve the navigator service without a canvas owner.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn browser() -> Result<ComPtr<ITextClipboardAbi>, NcStatus> {
        Self::from_value(Self {}).query()
    }
}

/// Validates copied text before an OS call; SDL requires strings without interior NUL bytes.
fn text(input: NcCopyFromBufferOffset<'_, u8>) -> Result<&str, NcStatus> {
    if input.offset() != 0 || input.as_slice().len() > MAX_CLIPBOARD_BYTES {
        return Err(NC_E_INVALID_ARG);
    }
    let text = std::str::from_utf8(input.as_slice()).map_err(|_| NC_E_INVALID_ARG)?;
    if text.contains('\0') {
        return Err(NC_E_INVALID_ARG);
    }
    Ok(text)
}

/// Delivers one bounded result; write success and every failure carry no text.
fn complete(completion: ComRef<'_, IClipboardCompletionAbi>, result: Result<String, NcStatus>) -> Result<(), NcStatus> {
    let result = result.and_then(|text| {
        if text.len() > MAX_CLIPBOARD_BYTES {
            Err(neocogi_com_sdk::NC_E_OUT_OF_RANGE)
        } else {
            Ok(text)
        }
    });
    match result {
        Ok(text) => completion.complete(0, NcCopyFromBufferOffset::new(0, text.as_bytes())),
        Err(error) => completion.complete(error.code(), NcCopyFromBufferOffset::new(0, &[])),
    }
}

/// One SDL-owned clipboard result. Every non-null return must be released with SDL_free.
#[cfg(not(target_arch = "wasm32"))]
struct SdlText(std::ptr::NonNull<std::ffi::c_char>);

#[cfg(not(target_arch = "wasm32"))]
impl Drop for SdlText {
    fn drop(&mut self) {
        // SAFETY: This pointer came from SDL_GetClipboardText and is still owned here.
        unsafe { sdl2::sys::SDL_free(self.0.as_ptr().cast()) };
    }
}

/// Reads while SDL's video service is live, treating a fresh SDL error as a failed transfer.
#[cfg(not(target_arch = "wasm32"))]
fn read_sdl_text(_video: &sdl2::clipboard::ClipboardUtil) -> Result<String, NcStatus> {
    sdl2::clear_error();
    // SAFETY: The retained ClipboardUtil keeps SDL video initialized on this thread.
    let native = unsafe { sdl2::sys::SDL_GetClipboardText() };
    // SAFETY: SDL_GetError returns a NUL-terminated thread-local string. Read only its
    // first byte before another SDL call can change it. Error text need not decode as UTF-8.
    let failed = unsafe { *sdl2::sys::SDL_GetError() != 0 };
    let native = SdlText(std::ptr::NonNull::new(native).ok_or(PLATFORM_E_BACKEND)?);
    // SDL can retain an allocated prefix when a later transfer step fails.
    if failed {
        return Err(PLATFORM_E_BACKEND);
    }
    // SAFETY: SDL returns an owned, NUL-terminated C string on every non-null path.
    let text = unsafe { std::ffi::CStr::from_ptr(native.0.as_ptr()) };
    if text.count_bytes() > MAX_CLIPBOARD_BYTES {
        return Err(neocogi_com_sdk::NC_E_OUT_OF_RANGE);
    }
    text.to_str().map(str::to_owned).map_err(|_| PLATFORM_E_BACKEND)
}

/// Reads through SDL only while its focused window still belongs to a live public input route.
#[cfg(not(target_arch = "wasm32"))]
fn read_wayland_text(service: &sdl2::clipboard::ClipboardUtil, host: &Weak<NativeHost>) -> Result<String, NcStatus> {
    let host = host.upgrade().ok_or(PLATFORM_E_BACKEND)?;
    let focused = host.video.sdl().keyboard().focused_window_id().ok_or(PLATFORM_E_BACKEND)?;
    let live_route = host.queues.try_borrow().map_err(|_| PLATFORM_E_BACKEND)?.contains_key(&focused);
    if !live_route {
        return Err(PLATFORM_E_BACKEND);
    }
    // SDL can service the current selection with a live focused route. No route borrow spans
    // the native read, and this function releases the host before invoking the completion.
    read_sdl_text(service)
}

#[cfg(not(target_arch = "wasm32"))]
impl ITextClipboard for Clipboard {
    /// SDL completes synchronously, but uses the same copied receipt as asynchronous platforms.
    fn read(&self, completion: ComRef<'_, IClipboardCompletionAbi>) -> Result<(), NcStatus> {
        let result = match &self.backend {
            Backend::Sdl(service) => read_sdl_text(service),
            Backend::Wayland { service, host } => read_wayland_text(service, host),
            #[cfg(target_os = "linux")]
            Backend::X11(service) => service.read(),
        };
        complete(completion, result)
    }
    /// Never acknowledge a cut until the OS accepted the entire validated selection.
    fn write(&self, input: NcCopyFromBufferOffset<'_, u8>, completion: ComRef<'_, IClipboardCompletionAbi>) -> Result<(), NcStatus> {
        let text = text(input)?;
        let result = match &self.backend {
            Backend::Sdl(service) => service.set_clipboard_text(text).map_err(|_| PLATFORM_E_BACKEND),
            // SDL2 cannot acknowledge compositor ownership. Avoid even a deferred SDL offer.
            Backend::Wayland { .. } => Err(PLATFORM_E_BACKEND),
            #[cfg(target_os = "linux")]
            Backend::X11(service) => service.write(text),
        };
        complete(completion, result.map(|()| String::new()))
    }
}

#[cfg(target_arch = "wasm32")]
/// Resolves an available browser capability without invoking methods on an undefined property.
fn browser_clipboard() -> Result<web_sys::Clipboard, NcStatus> {
    use wasm_bindgen::JsCast;
    let navigator = web_sys::window().ok_or(PLATFORM_E_BACKEND)?.navigator();
    js_sys::Reflect::get(navigator.as_ref(), &"clipboard".into())
        .map_err(|_| PLATFORM_E_BACKEND)?
        .dyn_into::<web_sys::Clipboard>()
        .map_err(|_| PLATFORM_E_BACKEND)
}

#[cfg(target_arch = "wasm32")]
/// Owns only a completion capability while the browser obtains clipboard permission/data.
fn await_result(promise: js_sys::Promise, completion: ComPtr<IClipboardCompletionAbi>, read: bool) {
    wasm_bindgen_futures::spawn_local(async move {
        let result = match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(value) if read => {
                use wasm_bindgen::JsCast;
                match value.dyn_into::<js_sys::JsString>() {
                    // UTF-16 length bounds conversion work before allocating a Rust UTF-8 copy.
                    Ok(text) if text.length() as usize <= MAX_CLIPBOARD_BYTES => Ok(String::from(text)),
                    Ok(_) => Err(neocogi_com_sdk::NC_E_OUT_OF_RANGE),
                    Err(_) => Err(PLATFORM_E_BACKEND),
                }
            }
            Ok(_) => Ok(String::new()),
            Err(_) => Err(PLATFORM_E_BACKEND),
        };
        // A retired editor may reject delivery; it owns no native resource requiring another retry.
        let _ = complete(completion.as_com_ref(), result);
    });
}

#[cfg(target_arch = "wasm32")]
impl ITextClipboard for Clipboard {
    /// Browser denial is an asynchronous failed receipt, never an empty successful paste.
    fn read(&self, completion: ComRef<'_, IClipboardCompletionAbi>) -> Result<(), NcStatus> {
        let completion = completion.try_add_ref()?;
        await_result(browser_clipboard()?.read_text(), completion, true);
        Ok(())
    }
    /// The promise resolves before the requesting editor may remove cut text.
    fn write(&self, input: NcCopyFromBufferOffset<'_, u8>, completion: ComRef<'_, IClipboardCompletionAbi>) -> Result<(), NcStatus> {
        let text = text(input)?;
        let completion = completion.try_add_ref()?;
        await_result(browser_clipboard()?.write_text(text), completion, false);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invalid native strings fail before SDL's interior-NUL panic path or any OS mutation.
    #[test]
    fn clipboard_write_validation_precedes_native_access() {
        for bytes in [&b"a\0b"[..], &b"\xff"[..]] {
            assert!(text(NcCopyFromBufferOffset::new(0, bytes)).is_err());
        }
        assert!(text(NcCopyFromBufferOffset::new(1, b"text")).is_err());
        let oversized = vec![b'a'; MAX_CLIPBOARD_BYTES + 1];
        assert!(text(NcCopyFromBufferOffset::new(0, &oversized)).is_err());
        assert_eq!(text(NcCopyFromBufferOffset::new(0, b"valid\ntext")).unwrap(), "valid\ntext");
    }
}
