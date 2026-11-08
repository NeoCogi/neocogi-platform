//! Example-owned assembly shared by triangle and both native glTF applications.
#![deny(missing_docs)]
extern crate self as neocogi_platform_examples;

/// SDK contracts and the implementations selected by this example assembly.
#[cfg(all(
    feature = "static",
    not(feature = "dynamic"),
    any(
        all(feature = "glow", not(any(feature = "vulkan", feature = "wgpu"))),
        all(feature = "vulkan", not(any(feature = "glow", feature = "wgpu"))),
        all(feature = "wgpu", not(any(feature = "glow", feature = "vulkan")))
    )
))]
pub use provider as sdk;
/// Dynamic applications require only contracts and choose their platform at runtime.
#[cfg(all(feature = "dynamic", not(any(feature = "static", feature = "glow", feature = "vulkan", feature = "wgpu"))))]
pub use neocogi_example_plugins::sdk;

#[cfg(all(not(target_arch = "wasm32"), not(any(feature = "static", feature = "dynamic"))))]
compile_error!("select exactly one example assembly: static or dynamic");
#[cfg(all(feature = "static", feature = "dynamic"))]
compile_error!("static and dynamic example assembly are mutually exclusive; use --no-default-features --features dynamic");
#[cfg(all(feature = "static", not(any(feature = "glow", feature = "vulkan", feature = "wgpu"))))]
compile_error!("select exactly one native backend for static assembly: glow, vulkan or wgpu");
#[cfg(any(all(feature = "glow", any(feature = "vulkan", feature = "wgpu")), all(feature = "vulkan", feature = "wgpu")))]
compile_error!("native backend features are mutually exclusive for static assembly");
#[cfg(all(feature = "dynamic", any(feature = "glow", feature = "vulkan", feature = "wgpu")))]
compile_error!("backend Cargo features select static implementations; select exactly dynamic and use --backend at runtime");

// Invalid selections emit only the diagnostics above, without implementation expansion.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        all(feature = "dynamic", not(any(feature = "static", feature = "glow", feature = "vulkan", feature = "wgpu"))),
        all(
            feature = "static",
            not(feature = "dynamic"),
            any(
                all(feature = "glow", not(any(feature = "vulkan", feature = "wgpu"))),
                all(feature = "vulkan", not(any(feature = "glow", feature = "wgpu"))),
                all(feature = "wgpu", not(any(feature = "glow", feature = "vulkan")))
            )
        )
    )
))]
mod assembly;
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        all(feature = "dynamic", not(any(feature = "static", feature = "glow", feature = "vulkan", feature = "wgpu"))),
        all(
            feature = "static",
            not(feature = "dynamic"),
            any(
                all(feature = "glow", not(any(feature = "vulkan", feature = "wgpu"))),
                all(feature = "vulkan", not(any(feature = "glow", feature = "wgpu"))),
                all(feature = "wgpu", not(any(feature = "glow", feature = "vulkan")))
            )
        )
    )
))]
pub use assembly::*;

/// Returns the executable-relative artwork directory used by the default native viewers.
#[cfg(not(target_arch = "wasm32"))]
pub fn assets_directory() -> std::io::Result<std::path::PathBuf> {
    std::env::current_exe()?
        .parent()
        .map(|parent| parent.join("assets/gltf"))
        .ok_or_else(|| std::io::Error::other("executable has no parent directory"))
}
