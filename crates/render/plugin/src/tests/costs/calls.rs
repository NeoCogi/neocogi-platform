//! Typed interception of real native GL initialization calls, in unit tests only.
use std::{cell::Cell, ffi::c_void};
/// Exact system ABI supplied by SDL for glBufferData.
type BufferData = unsafe extern "system" fn(u32, isize, *const c_void, u32);
/// Exact system ABI supplied by SDL for glTexImage2D.
type ImageData = unsafe extern "system" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
/// Exact system ABI supplied by SDL for glLinkProgram.
type LinkProgram = unsafe extern "system" fn(u32);

/// Native calls rather than logical resource versions; never compiled into deployed providers.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Calls {
    /// Actual immutable buffer initialization calls.
    buffers: u64,
    /// Actual image initialization/allocation calls, including the UI atlas.
    images: u64,
    /// Actual complete program links.
    links: u64,
}
thread_local! {
    /// Typed dispatch pointers follow SDL's current-thread context affinity.
    static BUFFER: Cell<Option<BufferData>> = const { Cell::new(None) };
    static IMAGE: Cell<Option<ImageData>> = const { Cell::new(None) };
    static LINK: Cell<Option<LinkProgram>> = const { Cell::new(None) };
    /// Tests serialize complete native context lifetimes; no cross-thread native state is shared.
    static COUNTS: Cell<Calls> = Cell::new(Calls::default());
}
/// Read and reset counts at the setup/cold/warm boundaries.
pub(crate) fn take() -> Calls {
    COUNTS.with(|value| value.replace(Calls::default()))
}
/// Forward the native call without changing any argument or client memory obligation.
unsafe extern "system" fn buffer(target: u32, size: isize, data: *const c_void, usage: u32) {
    COUNTS.with(|cell| {
        let mut value = cell.get();
        value.buffers += 1;
        cell.set(value);
    });
    // SAFETY: The loader installs the exact typed function for the live owning context.
    unsafe { BUFFER.with(|function| function.get().unwrap()(target, size, data, usage)) };
}
/// Count the actual image upload/allocation while preserving the native ABI.
#[allow(clippy::too_many_arguments, reason = "exact native glTexImage2D ABI")]
unsafe extern "system" fn image(target: u32, level: i32, internal: i32, width: i32, height: i32, border: i32, format: u32, kind: u32, data: *const c_void) {
    COUNTS.with(|cell| {
        let mut value = cell.get();
        value.images += 1;
        cell.set(value);
    });
    // SAFETY: SDL provided glTexImage2D for this exact current context.
    unsafe { IMAGE.with(|function| function.get().unwrap()(target, level, internal, width, height, border, format, kind, data)) };
}
/// Count actual native linkage separately from logical pipeline construction.
unsafe extern "system" fn link(program: u32) {
    COUNTS.with(|cell| {
        let mut value = cell.get();
        value.links += 1;
        cell.set(value);
    });
    // SAFETY: SDL provided glLinkProgram for this exact current context.
    unsafe { LINK.with(|function| function.get().unwrap()(program)) };
}
/// Interpose the three production upload/link seams with their exact typed signatures.
pub(crate) fn load(name: &str, pointer: *const c_void) -> *const c_void {
    if pointer.is_null() {
        return pointer;
    }
    // SAFETY: Each matched name specifies exactly the function ABI represented by its alias.
    unsafe {
        match name {
            "glBufferData" => {
                BUFFER.with(|value| value.set(Some(std::mem::transmute::<*const c_void, BufferData>(pointer))));
                buffer as *const c_void
            }
            "glTexImage2D" => {
                IMAGE.with(|value| value.set(Some(std::mem::transmute::<*const c_void, ImageData>(pointer))));
                image as *const c_void
            }
            "glLinkProgram" => {
                LINK.with(|value| value.set(Some(std::mem::transmute::<*const c_void, LinkProgram>(pointer))));
                link as *const c_void
            }
            _ => pointer,
        }
    }
}
