//! Build-time source owner for the renderer, audio and platform interfaces.
//!
//! Declare this package under `[build-dependencies]`. Its packaged `schemas`
//! directory is the only authored source tree for these namespaces. Consumers
//! generate into their own `OUT_DIR`; they never search the surrounding checkout
//! or depend on another contract crate's build output. A build-only dependency
//! suffices for fully local projections; importing canonical Rust definitions
//! also requires this package as a normal dependency.
#![deny(missing_docs)]

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Generation request and results from the pinned IDL toolchain.
pub use neocogi_idl::cargo::{BuildError, BuildOutput, SharedSources};

/// A namespace whose ordinary Rust definitions have one established library location.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Module {
    /// Renderer values, compiler results, resource groups and recording in render::abi.
    Renderer,
    /// Independent PCM interfaces, types, limits and validation in audio.
    Audio,
    /// Window, input, clipboard and system-service discovery in render::platform.
    Platform,
}

impl Module {
    /// Authored dotted namespace, independent of the consuming package's location.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Renderer => "neocogi.renderer",
            Self::Audio => "neocogi.audio",
            Self::Platform => "neocogi.platform",
        }
    }

    /// Library path used when importing ordinary Rust values from this namespace.
    fn rust_path(self) -> &'static str {
        match self {
            Self::Renderer => "neocogi_platform_contracts::render::abi",
            Self::Audio => "neocogi_platform_contracts::audio",
            Self::Platform => "neocogi_platform_contracts::render::platform",
        }
    }
}

/// Canonical files embedded from the one authored tree. Embedding lets a cached
/// helper survive Cargo relocating or removing the dependency's source directory.
const SOURCES: &[(&str, &[u8])] = &[
    ("neocogi/audio/audio.ncidl", include_bytes!("../schemas/neocogi/audio/audio.ncidl")),
    (
        "neocogi/platform/dependencies.ncidl",
        include_bytes!("../schemas/neocogi/platform/dependencies.ncidl"),
    ),
    ("neocogi/platform/input.ncidl", include_bytes!("../schemas/neocogi/platform/input.ncidl")),
    ("neocogi/platform/path.ncidl", include_bytes!("../schemas/neocogi/platform/path.ncidl")),
    ("neocogi/platform/window.ncidl", include_bytes!("../schemas/neocogi/platform/window.ncidl")),
    ("neocogi/renderer/frames.ncidl", include_bytes!("../schemas/neocogi/renderer/frames.ncidl")),
    ("neocogi/renderer/geometry.ncidl", include_bytes!("../schemas/neocogi/renderer/geometry.ncidl")),
    ("neocogi/renderer/gpu.ncidl", include_bytes!("../schemas/neocogi/renderer/gpu.ncidl")),
    (
        "neocogi/renderer/resources.ncidl",
        include_bytes!("../schemas/neocogi/renderer/resources.ncidl"),
    ),
    ("neocogi/renderer/compiler.ncidl", include_bytes!("../schemas/neocogi/renderer/compiler.ncidl")),
];

/// Adds concrete operation/path context to failures before any binding generation starts.
fn io_error(operation: &'static str, path: &Path, source: std::io::Error) -> BuildError {
    BuildError::Io { operation, path: path.to_owned(), source }
}

/// Removes stale generated inputs only inside the helper-owned OUT_DIR subtree.
/// Expected files keep their timestamps; no caller-authored source tree is touched.
fn prune(root: &Path, directory: &Path) -> Result<(), BuildError> {
    let entries = fs::read_dir(directory).map_err(|error| io_error("read generated sources", directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| io_error("read generated source entry", directory, error))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|error| io_error("inspect generated source", &path, error))?;
        if kind.is_dir() {
            prune(root, &path)?;
        } else if !SOURCES.iter().any(|(name, _)| root.join(name) == path) || !kind.is_file() {
            fs::remove_file(&path).map_err(|error| io_error("remove obsolete generated source", &path, error))?;
        }
    }
    Ok(())
}

/// Materializes derived inputs in this consumer's own output, preserving unchanged
/// mtimes and pruning removed declarations so old files cannot reenter resolution.
fn materialize(output: &Path) -> Result<PathBuf, BuildError> {
    let root = output.join("neocogi-shared-sources");
    fs::create_dir_all(&root).map_err(|error| io_error("create generated sources", &root, error))?;
    prune(&root, &root)?;
    for (name, contents) in SOURCES {
        let path = root.join(name);
        let parent = path.parent().expect("fixed source name has a parent");
        fs::create_dir_all(parent).map_err(|error| io_error("create generated namespace", parent, error))?;
        if fs::read(&path).ok().as_deref() != Some(*contents) {
            fs::write(&path, contents).map_err(|error| io_error("write generated source", &path, error))?;
        }
    }
    Ok(root)
}

/// Configures local generation and imports for the three shared namespaces.
///
/// `local` selects independent emissions for this consumer. Other reachable
/// namespaces use their ordinary library definitions; this matters when Rust
/// helper APIs exchange values, without changing COM identities. Repeating a
/// selected module is harmless. An empty selection is useful when extending the
/// request with a consumer-owned namespace using [`SharedSources::module`].
/// Every reachable external namespace still needs an explicit Rust location.
/// Embedded inputs are materialized under the calling build script's OUT_DIR;
/// missing environment or filesystem failures return before generation. Cached
/// helpers never depend on their original downloaded source path remaining live.
pub fn sources(local: &[Module]) -> Result<SharedSources, BuildError> {
    let output = std::env::var_os("OUT_DIR").ok_or(BuildError::MissingEnvironment("OUT_DIR"))?;
    let mut request = SharedSources::new(materialize(Path::new(&output))?);
    for module in [Module::Renderer, Module::Audio, Module::Platform] {
        request = if local.contains(&module) {
            request.module(module.name(), module.rust_path())
        } else {
            request.import(module.name(), module.rust_path())
        };
    }
    Ok(request)
}

/// Generates requested bindings, then publishes their paths to this consumer.
///
/// No include-path environment variables are emitted unless generation succeeds.
/// Source admission, output ownership and filesystem failures retain the IDL
/// toolchain's typed error. Each caller owns its generated files and must handle
/// failure in its build script; no shared process state or Cargo `links` registry
/// is introduced.
pub fn build(request: SharedSources) -> Result<Vec<BuildOutput>, BuildError> {
    let modules = request.build()?;
    for module in &modules {
        println!(
            "cargo::rustc-env=IDL_{}={}",
            module.module_name.replace('.', "_").to_uppercase(),
            module.include.display()
        );
    }
    Ok(modules)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Authored modules use the current IDL spelling without deprecated identity policies.
    #[test]
    fn canonical_modules_have_no_idl_warnings() {
        for module in [Module::Renderer, Module::Audio, Module::Platform] {
            let directory = module.name().replace('.', "/");
            let directory = Path::new(&directory);
            let parts: Vec<_> = SOURCES
                .iter()
                .filter(|(name, _)| Path::new(name).parent() == Some(directory))
                .map(|(name, bytes)| neocogi_idl::ModuleSource {
                    path: Path::new(name),
                    text: std::str::from_utf8(bytes).unwrap(),
                })
                .collect();
            let parsed = neocogi_idl::parse_module_parts(directory, &parts).unwrap();
            assert!(parsed.warnings.is_empty(), "{}: {:?}", module.name(), parsed.warnings);
        }
    }

    /// Warm reuse restores changed bytes, removes obsolete inputs and keeps unchanged mtimes.
    #[test]
    fn materialized_sources_are_complete_owned_and_reusable() {
        let output = tempfile::tempdir().unwrap();
        let root = materialize(output.path()).unwrap();
        for (name, contents) in SOURCES {
            assert_eq!(fs::read(root.join(name)).unwrap(), *contents);
        }
        let keep = root.join(SOURCES[0].0);
        let unchanged = fs::metadata(&keep).unwrap().modified().unwrap();
        let changed = root.join(SOURCES[1].0);
        fs::write(&changed, b"corrupt").unwrap();
        let stale = root.join("neocogi/shader/obsolete.ncidl");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, b"obsolete declaration").unwrap();
        // A cached pre-migration helper left the same audio declarations in platform.
        // Keeping that file would reintroduce platform-owned audio during resolution.
        let old_audio = root.join("neocogi/platform/audio.ncidl");
        fs::write(&old_audio, b"obsolete platform audio declarations").unwrap();
        assert_eq!(materialize(output.path()).unwrap(), root);
        assert!(!stale.exists());
        assert!(!old_audio.exists());
        assert!(root.join("neocogi/audio/audio.ncidl").is_file());
        assert_eq!(fs::metadata(keep).unwrap().modified().unwrap(), unchanged);
        assert_eq!(fs::read(changed).unwrap(), SOURCES[1].1);
    }

    /// A second consumer gets its own inputs without depending on the first consumer's files.
    #[test]
    fn separate_consumers_reconstruct_inputs_after_the_first_is_removed() {
        let first = tempfile::tempdir().unwrap();
        let root = materialize(first.path()).unwrap();
        first.close().unwrap();
        assert!(!root.exists());
        let second = tempfile::tempdir().unwrap();
        let root = materialize(second.path()).unwrap();
        assert!(SOURCES.iter().all(|(name, _)| root.join(name).is_file()));
    }
}
