//! Exercises example import and animation without opening a native display or downloading data.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
#[path = "../gltf/scene.rs"]
mod scene;
