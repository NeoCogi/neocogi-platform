//! Executable-relative plugin loading with SDK implementation features disabled.
#![deny(missing_docs)]

/// SDK contracts; dynamic consumers enable no implementation features.
pub use neocogi_platform as sdk;

mod artifacts;

/// Compiler then backend-specific platform filenames staged by the package build script.
#[cfg(not(target_arch = "wasm32"))]
pub const LIBRARIES: [&str; 2] = artifacts::libraries(
    artifacts::selected_backend(),
    if cfg!(target_os = "windows") {
        artifacts::System::Windows
    } else if cfg!(target_os = "macos") {
        artifacts::System::Macos
    } else {
        artifacts::System::Unix
    },
);

/// Loads trusted executable-relative artifacts and acquires their exact typed platform root.
#[cfg(not(target_arch = "wasm32"))]
pub fn acquire(
    runtime: neocogi_com::Runtime,
) -> Result<neocogi_com::ComPtr<neocogi_platform::render::platform::PlatformAbi>, neocogi_platform::render::abi::RenderError> {
    let executable = std::env::current_exe().map_err(|_| neocogi_com::NC_E_INVALID_ARG)?;
    let directory = executable.parent().ok_or(neocogi_com::NC_E_INVALID_ARG)?.join("lib");
    // Check the complete pair before mutating the process-lived plugin catalog.
    // Missing development artifacts are recoverable application errors; trusted
    // existing libraries retain the SDK's validation and fail-fatal loader policy.
    for filename in LIBRARIES {
        let path = directory.join(filename);
        if !path.is_file() {
            return Err(neocogi_platform::render::abi::RenderError::new(
                neocogi_com::NC_E_LOAD_FAILED,
                format!(
                    "Required plugin library is missing or inaccessible: {}. Build matching libraries with: {}",
                    path.display(),
                    preparation_command()
                ),
            ));
        }
    }
    // SAFETY: The application deliberately ships these trusted Cargo-built final
    // roots. The SDK validates dependencies and pins their code for process lifetime.
    unsafe {
        runtime.load_plugin_library(directory.join(LIBRARIES[0]));
    }
    // SAFETY: Same trusted artifact/lifetime obligation; the compiler is registered first.
    let platform = unsafe { runtime.load_plugin_library(directory.join(LIBRARIES[1])) };
    runtime.acquire_plugin_root(platform).map_err(Into::into)
}

/// Build command for this backend and Cargo profile; no private build path is embedded.
/// Cross-target deployments add the same `--target` used for the application.
pub fn preparation_command() -> String {
    let backend = if cfg!(feature = "vulkan") {
        "vulkan"
    } else if cfg!(feature = "wgpu") {
        "wgpu"
    } else {
        "glow"
    };
    format!(
        "cargo build --profile {} -p neocogi-platform-dylib --no-default-features --features {backend} --examples",
        env!("NEOCOGI_PLUGIN_BUILD_PROFILE")
    )
}
