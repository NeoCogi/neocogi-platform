//! Real COM pipeline, camera and optional native-pixel acceptance for the glTF applications.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
#[path = "../gltf/viewer.rs"]
mod viewer;
