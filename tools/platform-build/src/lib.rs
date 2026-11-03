//! Build-time source owner for the renderer, platform and shader interfaces.
//!
//! Declare this package under `[build-dependencies]`. Its packaged `schemas`
//! directory is the only authored source tree for these namespaces. Consumers
//! generate into their own `OUT_DIR`; they never search the surrounding checkout
//! or depend on another contract crate's build output. No runtime dependency on
//! this crate is necessary.
#![deny(missing_docs)]

use std::path::PathBuf;

/// Generation request and results from the pinned IDL toolchain.
pub use neocogi_idl::cargo::{BuildError, BuildOutput, SharedSources};

/// A namespace whose ordinary Rust definitions have one established library location.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Module {
    /// Renderer values, resource groups and recording interfaces in render-core.
    Renderer,
    /// Window, input and clipboard interfaces in render-core.
    Platform,
    /// Compiler interfaces and artifact exports in shader-core.
    Shader,
}

impl Module {
    /// Authored dotted namespace, independent of the consuming package's location.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Renderer => "neocogi.renderer",
            Self::Platform => "neocogi.platform",
            Self::Shader => "neocogi.shader",
        }
    }

    /// Library path used when importing ordinary Rust values from this namespace.
    fn rust_path(self) -> &'static str {
        match self {
            Self::Renderer => "neocogi_render_core::abi",
            Self::Platform => "neocogi_render_core::platform",
            Self::Shader => "neocogi_shader_core::abi",
        }
    }
}

/// Returns this build dependency's own packaged source directory.
///
/// Cargo supplies this crate's manifest directory when compiling the helper, so
/// the path follows the actual downloaded/extracted dependency rather than the
/// caller's current directory. These files must remain available while Cargo
/// builds consumers, as ordinary dependency source files do.
pub fn schema_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas")
}

/// Configures local generation and imports for the three shared namespaces.
///
/// `local` selects independent emissions for this consumer. Other reachable
/// namespaces use their ordinary library definitions; this matters when Rust
/// helper APIs exchange values, without changing COM identities. Repeating a
/// selected module is harmless. An empty selection is useful when extending the
/// request with a consumer-owned namespace using [`SharedSources::module`].
/// Every reachable external namespace still needs an explicit Rust location.
pub fn sources(local: &[Module]) -> SharedSources {
    let mut request = SharedSources::new(schema_root());
    for module in [Module::Renderer, Module::Platform, Module::Shader] {
        request = if local.contains(&module) {
            request.module(module.name(), module.rust_path())
        } else {
            request.import(module.name(), module.rust_path())
        };
    }
    request
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

    /// Every shared namespace resolves inside this one package, including multipart sources.
    #[test]
    fn every_namespace_has_one_packaged_source_owner() {
        let root = schema_root();
        assert!(root.is_dir());
        for module in [Module::Renderer, Module::Platform, Module::Shader] {
            let path = root.join(module.name().replace('.', "/"));
            assert!(path.is_dir(), "{}", path.display());
            assert!(
                std::fs::read_dir(path)
                    .unwrap()
                    .any(|entry| entry.unwrap().path().extension().is_some_and(|ext| ext == "ncidl"))
            );
        }
    }
}
