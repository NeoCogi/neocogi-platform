//! Executable-relative plugin loading with SDK implementation features disabled.
#![deny(missing_docs)]

/// SDK contracts; dynamic consumers enable no implementation features.
pub use neocogi_platform as sdk;

/// Compiler then backend-specific platform filenames prepared by build_plugins.py.
#[cfg(not(target_arch = "wasm32"))]
pub const LIBRARIES: [&str; 2] = [compiler_filename(), platform_filename()];

/// Loads trusted executable-relative artifacts and acquires their exact typed platform root.
#[cfg(not(target_arch = "wasm32"))]
pub fn acquire(runtime: neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_platform::render::platform::PlatformAbi>, neocogi_com::NcStatus> {
    let executable = std::env::current_exe().map_err(|_| neocogi_com::NC_E_INVALID_ARG)?;
    let directory = executable.parent().ok_or(neocogi_com::NC_E_INVALID_ARG)?.join("lib");
    // SAFETY: The application deliberately ships these trusted Cargo-built final
    // roots. The SDK validates dependencies and pins their code for process lifetime.
    unsafe {
        runtime.load_plugin_library(directory.join(LIBRARIES[0]));
    }
    // SAFETY: Same trusted artifact/lifetime obligation; the compiler is registered first.
    let platform = unsafe { runtime.load_plugin_library(directory.join(LIBRARIES[1])) };
    runtime.acquire_plugin_root(platform)
}

/// Target-specific filename of the independent CPU compiler provider.
const fn compiler_filename() -> &'static str {
    if cfg!(target_os = "windows") {
        "nc_shader_compiler.dll"
    } else if cfg!(target_os = "macos") {
        "libnc_shader_compiler.dylib"
    } else {
        "libnc_shader_compiler.so"
    }
}

/// Target/backend-specific platform filename; selected explicitly by the application.
const fn platform_filename() -> &'static str {
    if cfg!(feature = "vulkan") {
        if cfg!(target_os = "windows") {
            "nc_platform_vulkan.dll"
        } else if cfg!(target_os = "macos") {
            "libnc_platform_vulkan.dylib"
        } else {
            "libnc_platform_vulkan.so"
        }
    } else if cfg!(feature = "wgpu") {
        if cfg!(target_os = "windows") {
            "nc_platform_wgpu.dll"
        } else if cfg!(target_os = "macos") {
            "libnc_platform_wgpu.dylib"
        } else {
            "libnc_platform_wgpu.so"
        }
    } else {
        if cfg!(target_os = "windows") {
            "nc_platform_glow.dll"
        } else if cfg!(target_os = "macos") {
            "libnc_platform_glow.dylib"
        } else {
            "libnc_platform_glow.so"
        }
    }
}
