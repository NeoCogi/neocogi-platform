//! Runtime backend selection and executable-relative loading using only schema contracts.
#![deny(missing_docs)]
/// Canonical schema contracts; this loader has no renderer SDK dependency.
pub use neocogi_platform_schema as sdk;
mod artifacts;
/// Supported dynamic platform choices; this is application configuration, not a renderer ABI.
pub use artifacts::Backend;
use sdk::render::abi::RenderError;
use std::path::Path;

/// Exact filename of one backend on this executable's target system.
pub fn library_filename(backend: Backend) -> &'static str {
    artifacts::libraries(backend, artifacts::System::from_os(std::env::consts::OS))[0]
}

/// Resolve an explicit name or the sole deployed library before mutating the plugin catalog.
/// Multiple libraries require a runtime choice; missing/invalid choices never try another backend.
pub fn select_backend(requested: Option<&str>, directory: &Path) -> Result<Backend, RenderError> {
    if let Some(name) = requested {
        let backend = Backend::parse(name).ok_or_else(|| {
            RenderError::new(
                neocogi_com::NC_E_INVALID_ARG,
                format!("Unknown backend {name}; --backend accepts glow, vulkan or wgpu"),
            )
        })?;
        if !directory.join(library_filename(backend)).is_file() {
            return Err(missing(directory, backend));
        }
        return Ok(backend);
    }
    let installed: Vec<_> = Backend::ALL
        .into_iter()
        .filter(|backend| directory.join(library_filename(*backend)).is_file())
        .collect();
    match installed.as_slice() {
        [backend] => Ok(*backend),
        [] => Err(RenderError::new(
            neocogi_com::NC_E_LOAD_FAILED,
            format!(
                "No platform library found in {}. Build one backend with: {} (or use --features glow/wgpu for that library build)",
                directory.display(),
                preparation_command(Backend::Vulkan)
            ),
        )),
        _ => Err(RenderError::new(
            neocogi_com::NC_E_INVALID_ARG,
            format!(
                "Multiple platform libraries found ({}); select one with --backend glow, --backend vulkan or --backend wgpu",
                installed.iter().map(|backend| backend.name()).collect::<Vec<_>>().join(", ")
            ),
        )),
    }
}

/// Missing development artifacts are owned errors naming the exact selected file and build command.
fn missing(directory: &Path, backend: Backend) -> RenderError {
    RenderError::new(
        neocogi_com::NC_E_LOAD_FAILED,
        format!(
            "Required plugin library is missing or inaccessible: {}. Build matching library with: {}",
            directory.join(library_filename(backend)).display(),
            preparation_command(backend)
        ),
    )
}

/// Load exactly the resolved trusted library and acquire its typed platform root.
/// Selection occurs before loading; activation never falls back to another library or linked code.
pub fn acquire(runtime: neocogi_com::Runtime, requested: Option<&str>) -> Result<neocogi_com::ComPtr<sdk::render::platform::PlatformAbi>, RenderError> {
    let executable = std::env::current_exe().map_err(|_| neocogi_com::NC_E_INVALID_ARG)?;
    let directory = executable.parent().ok_or(neocogi_com::NC_E_INVALID_ARG)?.join("lib");
    let backend = select_backend(requested, &directory)?;
    // SAFETY: This executable deliberately ships the trusted Cargo-built final root.
    // The SDK validates its metadata and pins implementation code for process lifetime.
    let id = unsafe { runtime.load_plugin_library(directory.join(library_filename(backend))) };
    if id != neocogi_com::guid_from_qualified_name(backend.plugin_name()) {
        return Err(RenderError::new(
            neocogi_com::NC_E_INVALID_ARG,
            format!(
                "Platform library {} does not identify the requested {} backend",
                library_filename(backend),
                backend.name()
            ),
        ));
    }
    runtime.acquire_plugin_root(id).map_err(Into::into)
}

/// Standard Cargo command for one selected library, preserving this application's profile.
/// Cross-target deployments add the same --target used for the application.
pub fn preparation_command(backend: Backend) -> String {
    format!(
        "cargo build --profile {} -p neocogi-platform-dylib --no-default-features --features {} --examples",
        env!("NEOCOGI_PLUGIN_BUILD_PROFILE"),
        backend.name()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exhaust every deployed subset: automatic selection is unique, while explicit choices are exact.
    #[test]
    fn runtime_selection_covers_all_deployed_backend_subsets() {
        for mask in 0..8 {
            let root = tempfile::tempdir().unwrap();
            let mut present = Vec::new();
            for (index, backend) in Backend::ALL.into_iter().enumerate() {
                if mask & (1 << index) != 0 {
                    std::fs::write(root.path().join(library_filename(backend)), []).unwrap();
                    present.push(backend);
                }
            }
            let automatic = select_backend(None, root.path());
            match present.as_slice() {
                [backend] => assert_eq!(automatic.unwrap(), *backend),
                [] => {
                    let error = automatic.unwrap_err();
                    assert_eq!(error.status, neocogi_com::NC_E_LOAD_FAILED);
                    assert!(error.message.contains("cargo build --profile"));
                }
                _ => {
                    let error = automatic.unwrap_err();
                    assert_eq!(error.status, neocogi_com::NC_E_INVALID_ARG);
                    assert!(error.message.contains("--backend"));
                    for backend in &present {
                        assert!(error.message.contains(backend.name()));
                    }
                }
            }
            for backend in Backend::ALL {
                let selected = select_backend(Some(backend.name()), root.path());
                if present.contains(&backend) {
                    assert_eq!(selected.unwrap(), backend);
                } else {
                    let error = selected.unwrap_err();
                    assert_eq!(error.status, neocogi_com::NC_E_LOAD_FAILED);
                    assert!(error.message.contains(library_filename(backend)));
                    assert!(error.message.contains(&preparation_command(backend)));
                }
            }
            for unknown in ["", "GLow", "metal", "../vulkan"] {
                assert_eq!(select_backend(Some(unknown), root.path()).unwrap_err().status, neocogi_com::NC_E_INVALID_ARG);
            }
        }
    }
}
