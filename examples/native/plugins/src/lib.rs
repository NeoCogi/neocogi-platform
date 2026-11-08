//! Executable-relative plugin loading using only the dedicated schema contracts.
#![deny(missing_docs)]

/// Canonical schema contracts; this loader has no renderer SDK dependency.
pub use neocogi_platform_schema as sdk;

mod artifacts;

/// Backend-specific platform library staged by the package build script.
#[cfg(not(target_arch = "wasm32"))]
pub const LIBRARIES: [&str; 1] = artifacts::libraries(
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
) -> Result<neocogi_com::ComPtr<neocogi_platform_schema::render::platform::PlatformAbi>, neocogi_platform_schema::render::abi::RenderError> {
    let executable = std::env::current_exe().map_err(|_| neocogi_com::NC_E_INVALID_ARG)?;
    let directory = executable.parent().ok_or(neocogi_com::NC_E_INVALID_ARG)?.join("lib");
    // Check the platform library before mutating the process-lived plugin catalog.
    // Missing development artifacts are recoverable application errors; trusted
    // existing libraries retain the SDK's validation and fail-fatal loader policy.
    for filename in LIBRARIES {
        let path = directory.join(filename);
        if !path.is_file() {
            return Err(neocogi_platform_schema::render::abi::RenderError::new(
                neocogi_com::NC_E_LOAD_FAILED,
                format!(
                    "Required plugin library is missing or inaccessible: {}. Build matching libraries with: {}",
                    path.display(),
                    preparation_command()
                ),
            ));
        }
    }
    // SAFETY: The application deliberately ships this trusted Cargo-built final root.
    // The SDK validates its descriptor and pins implementation code for process lifetime.
    let platform = unsafe { runtime.load_plugin_library(directory.join(LIBRARIES[0])) };
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
