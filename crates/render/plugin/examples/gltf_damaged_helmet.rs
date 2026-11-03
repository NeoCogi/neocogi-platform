//! Textured Khronos DamagedHelmet with arcball rotation, camera-plane pan and wheel zoom.
#[cfg(not(target_arch = "wasm32"))]
#[path = "gltf/viewer.rs"]
mod viewer;

/// Owns the complete native application and returns an observable failure on import/GPU errors.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    if let Err(error) = viewer::run("DamagedHelmet") {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

/// Browser execution needs a separate asynchronous asset/event host; these samples are native.
#[cfg(target_arch = "wasm32")]
fn main() {}
