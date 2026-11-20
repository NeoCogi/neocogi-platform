//! Runtime Cargo workspace selection for commands that use checkout files.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// The private package identifies compatible workspaces even when Cargo reuses the executable.
const TOOLS_PACKAGE: &str = "neocogi-platform-tools";

/// Cargo's workspace identity and member records from dependency-free metadata.
#[derive(Deserialize)]
struct Discovery {
    /// Root reported by Cargo, before filesystem alias resolution.
    workspace_root: PathBuf,
    /// Source-aware package IDs of workspace members.
    workspace_members: HashSet<String>,
    /// Package records carrying source locations.
    packages: Vec<Member>,
}

/// Concrete package source used to validate the tools member.
#[derive(Deserialize)]
struct Member {
    /// Cargo identity shared with its workspace membership list.
    id: String,
    /// Published or private Cargo package name.
    name: String,
    /// Manifest for this concrete source.
    manifest_path: PathBuf,
}

/// A validated checkout and the separate directory used to resolve Cargo configuration.
pub(crate) struct Workspace {
    /// Physical checkout root for authored repository file inputs.
    root: PathBuf,
    /// Original invocation directory or explicitly selected root for Cargo.
    cargo_directory: PathBuf,
}

impl Workspace {
    /// Resolve the workspace once, using Cargo's ordinary manifest and configuration discovery.
    pub(crate) fn discover(explicit: Option<&Path>) -> Result<Self> {
        let cargo_directory = match explicit {
            Some(directory) => directory
                .canonicalize()
                .with_context(|| format!("resolve --workspace directory {}", directory.display()))?,
            None => std::env::current_dir().context("resolve tool invocation directory")?,
        };
        ensure!(cargo_directory.is_dir(), "workspace context is not a directory: {}", cargo_directory.display());
        let metadata: Discovery = serde_json::from_slice(
            &crate::process::capture(crate::process::cargo(&cargo_directory, None).args(["metadata", "--locked", "--no-deps", "--format-version=1"]))
                .with_context(|| format!("discover the NeoCogi Platform workspace from {}", cargo_directory.display()))?,
        )
        .context("read Cargo workspace metadata")?;
        let root = metadata.workspace_root.canonicalize().context("resolve Cargo workspace root")?;
        if explicit.is_some() {
            ensure!(root == cargo_directory, "--workspace must name the Cargo workspace root: {}", root.display());
        }
        let expected_manifest = root.join("scripts/Cargo.toml");
        ensure!(
            metadata.packages.iter().any(|package| {
                package.name == TOOLS_PACKAGE
                    && metadata.workspace_members.contains(&package.id)
                    && package.manifest_path.canonicalize().is_ok_and(|path| path == expected_manifest)
            }),
            "Cargo workspace {} must contain the {TOOLS_PACKAGE} member at {}",
            root.display(),
            expected_manifest.display(),
        );
        Ok(Self { root, cargo_directory })
    }

    /// Resolve authored, repository-relative source paths against the physical workspace.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Start a Cargo operation with this invocation's effective configuration.
    pub(crate) fn cargo(&self, subcommand: &str) -> Command {
        let mut command = crate::process::cargo(&self.cargo_directory, None);
        command.arg(subcommand);
        command
    }

    /// Select the validated workspace manifest for repository-directed Cargo operations.
    pub(crate) fn cargo_workspace(&self, subcommand: &str) -> Command {
        let mut command = self.cargo(subcommand);
        command.arg("--manifest-path").arg(self.root.join("Cargo.toml"));
        command
    }

    /// Read the exact workspace pin shared by binding tool installation and execution.
    pub(crate) fn bindgen_version(&self) -> Result<String> {
        let manifest: toml::Value = toml::from_str(&fs::read_to_string(self.root.join("Cargo.toml"))?)?;
        let version = manifest["workspace"]["dependencies"]["wasm-bindgen"]
            .as_str()
            .context("workspace wasm-bindgen must have an exact string pin")?;
        Ok(version.strip_prefix('=').context("workspace wasm-bindgen must be pinned exactly")?.to_owned())
    }
}
