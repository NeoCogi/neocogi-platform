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
/// SDK contracts used by the dynamically loaded assembly.
#[cfg(all(
    feature = "dynamic",
    not(feature = "static"),
    any(
        all(feature = "glow", not(any(feature = "vulkan", feature = "wgpu"))),
        all(feature = "vulkan", not(any(feature = "glow", feature = "wgpu"))),
        all(feature = "wgpu", not(any(feature = "glow", feature = "vulkan")))
    )
))]
pub use neocogi_example_plugins::sdk;

#[cfg(all(not(target_arch = "wasm32"), not(any(feature = "static", feature = "dynamic"))))]
compile_error!("select exactly one example assembly: static or dynamic");
#[cfg(all(not(target_arch = "wasm32"), feature = "static", feature = "dynamic"))]
compile_error!("static and dynamic example assembly are mutually exclusive; use --no-default-features --features dynamic,glow");
#[cfg(all(not(target_arch = "wasm32"), not(any(feature = "glow", feature = "vulkan", feature = "wgpu"))))]
compile_error!("select exactly one native backend: glow, vulkan or wgpu");
#[cfg(any(all(feature = "glow", any(feature = "vulkan", feature = "wgpu")), all(feature = "vulkan", feature = "wgpu")))]
compile_error!("native backend features are mutually exclusive; use --no-default-features with the selected assembly and backend");

/// Returns the executable-relative artwork directory used by the default native viewers.
#[cfg(not(target_arch = "wasm32"))]
pub fn assets_directory() -> std::io::Result<std::path::PathBuf> {
    std::env::current_exe()?
        .parent()
        .map(|parent| parent.join("assets/gltf"))
        .ok_or_else(|| std::io::Error::other("executable has no parent directory"))
}

/// Acquires one application platform root using the selected assembly.
///
/// Call once and retain this typed capability for the application's windows.
/// The executable owns the process-lived SDK host in both modes; the existing
/// native-session behavior is unchanged. No compiler/platform implementation
/// constructor or native handle is exposed to scene/rendering code.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(all(feature = "static", not(feature = "dynamic")), all(feature = "dynamic", not(feature = "static"))),
    any(
        all(feature = "glow", not(any(feature = "vulkan", feature = "wgpu"))),
        all(feature = "vulkan", not(any(feature = "glow", feature = "wgpu"))),
        all(feature = "wgpu", not(any(feature = "glow", feature = "vulkan")))
    )
))]
pub fn acquire_platform()
-> Result<neocogi_com::ComPtr<neocogi_platform_examples::sdk::render::platform::PlatformAbi>, neocogi_platform_examples::sdk::render::abi::RenderError> {
    // SAFETY: This final executable owns the immutable process-lived host tables.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }?;
    #[cfg(feature = "static")]
    {
        // SAFETY: Both descriptor graphs and implementation code are linked for
        // process lifetime. Registration/acquisition follows the same dependency order.
        unsafe {
            runtime.register_plugin_descriptor(neocogi_platform_examples::sdk::compiler::plugin_descriptor())?;
            runtime.register_plugin_descriptor(neocogi_platform_examples::sdk::platform::plugin_descriptor())?;
        }
        runtime
            .acquire_plugin_root(neocogi_platform_examples::sdk::platform::plugin_descriptor().plugin_id)
            .map_err(Into::into)
    }
    #[cfg(feature = "dynamic")]
    neocogi_example_plugins::acquire(runtime)
}
