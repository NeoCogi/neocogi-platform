//! Platform assembly with SDL/browser hosting and build-selected graphics execution.
//!
//! Each backend build has its own plugin identity. The native window, input pump, GPU resources,
//! and renderer deliberately remain in that same artifact: splitting those owners would expose
//! native handles or add another dynamic call boundary to every frame.
#![deny(missing_docs)]

#[cfg(not(any(feature = "webgl", feature = "glow", feature = "vulkan", feature = "wgpu")))]
compile_error!("select exactly one renderer feature: glow, vulkan, wgpu, or webgl");
#[cfg(any(
    all(feature = "webgl", any(feature = "glow", feature = "vulkan", feature = "wgpu")),
    all(feature = "glow", any(feature = "vulkan", feature = "wgpu")),
    all(feature = "vulkan", feature = "wgpu"),
))]
compile_error!(
    "renderer features are mutually exclusive: Cargo adds --features to the default Glow selection.\n\
     Disable defaults when selecting another backend, e.g. --no-default-features --features vulkan.\n\
     For the Vulkan platform example: cargo run --locked -p neocogi-platform-plugin --example gltf_damaged_helmet --no-default-features --features vulkan"
);
#[cfg(all(feature = "webgl", not(target_arch = "wasm32")))]
compile_error!("the webgl platform plugin requires wasm32");
#[cfg(all(target_arch = "wasm32", not(feature = "webgl")))]
compile_error!("wasm32 rendering requires the webgl feature");

// Rust continues checking items after compile_error!, which used to expand several renderer
// aliases and factory descriptors and obscure the selection error with unrelated diagnostics.
// Only a valid target/backend pair expands the implementation. Include at the crate root keeps
// the public factory API, generated entrypoint and existing child-module paths unchanged; there
// is no fallback backend, runtime selection or extra build script.
#[cfg(any(
    all(not(target_arch = "wasm32"), feature = "glow", not(any(feature = "vulkan", feature = "wgpu", feature = "webgl"))),
    all(not(target_arch = "wasm32"), feature = "vulkan", not(any(feature = "glow", feature = "wgpu", feature = "webgl"))),
    all(not(target_arch = "wasm32"), feature = "wgpu", not(any(feature = "glow", feature = "vulkan", feature = "webgl"))),
    all(target_arch = "wasm32", feature = "webgl", not(any(feature = "glow", feature = "vulkan", feature = "wgpu"))),
))]
include!("implementation.rs");

// This unit-test executable owns one runtime; normal library builds never embed it.
#[cfg(test)]
extern crate neocogi_com_host as _;
