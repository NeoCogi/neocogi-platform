//! Cargo-selected native plugin composition; contains no normal provider Rust dependency.
#![deny(missing_docs)]

/// Compiler then platform filenames selected and staged by normal Cargo artifact edges.
#[cfg(not(target_arch = "wasm32"))]
pub const LIBRARIES: [&str; 2] = [
    neocogi_native_layout::stage_library!("CARGO_CDYLIB_FILE_SHADER_LIBRARY_nc_shader_compiler"),
    neocogi_native_layout::stage_library!("CARGO_CDYLIB_FILE_PLATFORM_LIBRARY_nc_platform"),
];

/// Loads trusted executable-relative artifacts and acquires their exact typed platform root.
#[cfg(not(target_arch = "wasm32"))]
pub fn acquire(runtime: neocogi_com::Runtime) -> Result<neocogi_com::ComPtr<neocogi_render_core::platform::PlatformAbi>, neocogi_com::NcStatus> {
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
