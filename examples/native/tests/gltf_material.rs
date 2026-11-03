//! Material packing and AO contracts without a native display or downloaded assets.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
#[path = "../gltf/material.rs"]
mod material;
#[path = "../gltf/scene.rs"]
mod scene;
