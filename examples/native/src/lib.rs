//! Example-owned assembly shared by triangle and both native glTF applications.
#![deny(missing_docs)]

#[cfg(all(not(target_arch = "wasm32"), not(any(feature = "static", feature = "dynamic"))))]
compile_error!("select exactly one example assembly: static or dynamic");
#[cfg(all(not(target_arch = "wasm32"), feature = "static", feature = "dynamic"))]
compile_error!("static and dynamic example assembly are mutually exclusive; use --no-default-features --features dynamic,glow");
#[cfg(all(not(target_arch = "wasm32"), not(any(feature = "glow", feature = "vulkan", feature = "wgpu"))))]
compile_error!("select exactly one native backend: glow, vulkan or wgpu");
#[cfg(any(all(feature = "glow", any(feature = "vulkan", feature = "wgpu")), all(feature = "vulkan", feature = "wgpu")))]
compile_error!("native backend features are mutually exclusive; use --no-default-features with the selected assembly and backend");

/// Artifact-independent source assets are optional until the explicit fetch command runs.
/// Only immutable source files are owned slots; the sibling runtime IBL cache is not staged.
#[cfg(all(not(target_arch = "wasm32"), example_assets_available))]
const _ASSETS: () = neocogi_native_layout::stage_resources!(
    "gltf/assets/DamagedHelmet.glb" => "assets/gltf/DamagedHelmet.glb",
    "gltf/assets/BrainStem.glb" => "assets/gltf/BrainStem.glb",
    "gltf/assets/Cannon_Exterior.hdr" => "assets/gltf/Cannon_Exterior.hdr",
    "gltf/DamagedHelmet-LICENSE.md" => "assets/gltf/DamagedHelmet-LICENSE.md",
    "gltf/BrainStem-LICENSE.md" => "assets/gltf/BrainStem-LICENSE.md",
    "gltf/LicenseRef-Poser-EULA.txt" => "assets/gltf/LicenseRef-Poser-EULA.txt",
    "gltf/Cannon_Exterior-LICENSE.md" => "assets/gltf/Cannon_Exterior-LICENSE.md",
    "gltf/Cannon_Exterior-CC0.txt" => "assets/gltf/Cannon_Exterior-CC0.txt",
);

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
pub fn acquire_platform() -> Result<neocogi_com::ComPtr<neocogi_render_core::platform::PlatformAbi>, neocogi_com::NcStatus> {
    // SAFETY: This final executable owns the immutable process-lived host tables.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }?;
    #[cfg(feature = "static")]
    {
        // SAFETY: Both descriptor graphs and implementation code are linked for
        // process lifetime. Registration/acquisition follows the same dependency order.
        unsafe {
            runtime.register_plugin_descriptor(nc_shader_compiler::plugin_descriptor())?;
            runtime.register_plugin_descriptor(nc_platform::plugin_descriptor())?;
        }
        runtime.acquire_plugin_root(nc_platform::plugin_descriptor().plugin_id)
    }
    #[cfg(feature = "dynamic")]
    neocogi_example_plugins::acquire(runtime)
}
