//! Executable-relative loading of the build-selected platform using only schema contracts.
#![deny(missing_docs)]
/// Canonical schema contracts; this loader has no renderer SDK dependency.
pub use neocogi_platform_contracts as sdk;
mod artifacts;
/// Recognized platform identities; runtime requests verify the library's compiled backend.
pub use artifacts::Backend;
use sdk::render::abi::RenderError;
use std::path::{Path, PathBuf};

/// Exact filename of the platform library on this executable's target system.
pub fn library_filename() -> &'static str {
    artifacts::library_filename(artifacts::System::from_os(std::env::consts::OS))
}

/// Resolve the one deployed file and optional expected identity before loading native code.
/// Invalid backend requests reject before filesystem or plugin work.
fn preflight(requested: Option<&str>, directory: &Path) -> Result<(PathBuf, Option<Backend>), RenderError> {
    let expected = requested
        .map(|name| {
            Backend::parse(name).ok_or_else(|| {
                RenderError::new(
                    neocogi_com::NC_E_INVALID_ARG,
                    format!("Unknown backend {name}; --backend accepts glow, vulkan or wgpu"),
                )
            })
        })
        .transpose()?;
    let path = directory.join(library_filename());
    if !path.is_file() {
        return Err(RenderError::new(
            neocogi_com::NC_E_LOAD_FAILED,
            format!(
                "Required plugin library is missing or inaccessible: {}. Build matching library with: {}",
                path.display(),
                preparation_command(expected.unwrap_or(Backend::Glow))
            ),
        ));
    }
    Ok((path, expected))
}

/// Load the trusted platform library and acquire its typed root.
/// Its descriptor identifies the build-selected backend; an explicit request must agree.
/// Missing or incompatible libraries never fall back to another file or linked code.
pub fn acquire(runtime: neocogi_com::Runtime, requested: Option<&str>) -> Result<neocogi_com::ComPtr<sdk::render::platform::PlatformAbi>, RenderError> {
    let executable = std::env::current_exe().map_err(|_| neocogi_com::NC_E_INVALID_ARG)?;
    let directory = executable.parent().ok_or(neocogi_com::NC_E_INVALID_ARG)?.join("lib");
    let (path, expected) = preflight(requested, &directory)?;
    // SAFETY: This executable deliberately ships the trusted Cargo-built final root.
    // The SDK validates its metadata and pins implementation code for process lifetime.
    let id = unsafe { runtime.load_plugin_library(path) };
    let backend = Backend::ALL
        .into_iter()
        .find(|backend| id == neocogi_com::guid_from_qualified_name(backend.plugin_name()))
        .ok_or_else(|| RenderError::new(neocogi_com::NC_E_INVALID_ARG, "Platform library has an unrecognized backend descriptor"))?;
    if let Some(expected) = expected
        && backend != expected
    {
        return Err(RenderError::new(
            neocogi_com::NC_E_INVALID_ARG,
            format!(
                "Platform library {} uses {}; it does not identify the requested {} backend. Rebuild it with --features {}.",
                library_filename(),
                backend.name(),
                expected.name(),
                expected.name()
            ),
        ));
    }
    runtime.acquire_plugin_root(id).map_err(Into::into)
}

/// Standard Cargo command for the normal library, preserving this application's profile.
/// Cross-target deployments add the same --target used for the application.
pub fn preparation_command(backend: Backend) -> String {
    format!(
        "cargo build --profile {} -p neocogi-platform-dylib --no-default-features --features {}",
        env!("NEOCOGI_PLUGIN_BUILD_PROFILE"),
        backend.name()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One filename is used for every backend, and the optional request only constrains its descriptor.
    #[test]
    fn preflight_checks_one_normal_library_and_rejects_invalid_requests() {
        let root = tempfile::tempdir().unwrap();
        for backend in Backend::ALL {
            let error = preflight(Some(backend.name()), root.path()).unwrap_err();
            assert_eq!(error.status, neocogi_com::NC_E_LOAD_FAILED);
            assert!(error.message.contains(library_filename()));
            assert!(error.message.contains(&preparation_command(backend)));
            assert!(!error.message.contains("--examples"));
        }
        assert_eq!(preflight(None, root.path()).unwrap_err().status, neocogi_com::NC_E_LOAD_FAILED);
        let path = root.path().join(library_filename());
        std::fs::write(&path, []).unwrap();
        assert_eq!(preflight(None, root.path()).unwrap(), (path.clone(), None));
        for backend in Backend::ALL {
            assert_eq!(preflight(Some(backend.name()), root.path()).unwrap(), (path.clone(), Some(backend)));
        }
        for unknown in ["", "GLow", "metal", "../vulkan"] {
            assert_eq!(preflight(Some(unknown), root.path()).unwrap_err().status, neocogi_com::NC_E_INVALID_ARG);
        }
    }
}
