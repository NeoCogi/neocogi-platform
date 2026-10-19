//! Platform clipboard transfer with copied completion facts and no retained UI borrows.
use super::*;

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
            Err(neocogi_com::NC_E_OUT_OF_RANGE)
        } else {
            Ok(text)
        }
    });
    match result {
        Ok(text) => completion.complete(0, NcCopyFromBufferOffset::new(0, text.as_bytes())),
        Err(error) => completion.complete(error.code(), NcCopyFromBufferOffset::new(0, &[])),
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ITextClipboard for WindowObject {
    /// SDL completes synchronously, but uses the same copied receipt as asynchronous platforms.
    fn read(&self, completion: ComRef<'_, IClipboardCompletionAbi>) -> Result<(), NcStatus> {
        let result = self.device.window.subsystem().clipboard().clipboard_text().map_err(|_| PLATFORM_E_BACKEND);
        complete(completion, result)
    }
    /// Never acknowledge a cut until the OS accepted the entire validated selection.
    fn write(&self, input: NcCopyFromBufferOffset<'_, u8>, completion: ComRef<'_, IClipboardCompletionAbi>) -> Result<(), NcStatus> {
        let text = text(input)?;
        let result = self
            .device
            .window
            .subsystem()
            .clipboard()
            .set_clipboard_text(text)
            .map(|_| String::new())
            .map_err(|_| PLATFORM_E_BACKEND);
        complete(completion, result)
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
                    Ok(_) => Err(neocogi_com::NC_E_OUT_OF_RANGE),
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
impl ITextClipboard for WindowObject {
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
