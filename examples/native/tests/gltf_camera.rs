//! Arcball regressions exercise normalized platform input with no window or GPU.
#![cfg(not(target_arch = "wasm32"))]
#[path = "../gltf/camera.rs"]
mod camera;
