//! Run-owned Cargo directories and concrete build receipts.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

/// One Cargo phase's final outputs and disposable intermediate state.
pub(crate) struct CargoDirectories {
    /// Directory holding final libraries and executables reported by Cargo.
    pub(crate) target: PathBuf,
    /// Directory holding dependencies, build scripts and intermediate files.
    pub(crate) build: PathBuf,
}

impl CargoDirectories {
    /// Create a phase's directories below a parent already owned by the active run.
    pub(crate) fn create(parent: &Path, name: &str) -> Result<Self> {
        let target = parent.join(name);
        fs::create_dir(&target).with_context(|| format!("create Cargo target {}", target.display()))?;
        let build = target.join("build");
        fs::create_dir(&build).with_context(|| format!("create Cargo build directory {}", build.display()))?;
        Ok(Self { target, build })
    }

    /// Keep all other settings attached to the original Cargo command and override both outputs.
    pub(crate) fn command(&self, mut command: Command) -> Command {
        command.env("CARGO_TARGET_DIR", &self.target).env("CARGO_BUILD_BUILD_DIR", &self.build);
        command
    }

    /// Resolve aliases and missing tails so a report and both Cargo roots are disjoint before creation.
    pub(crate) fn report_path(&self, output: &Path) -> Result<PathBuf> {
        let output = physical_path(output)?;
        for (kind, path) in [("target", &self.target), ("build", &self.build)] {
            let directory = physical_path(path)?;
            ensure!(
                !output.starts_with(&directory) && !directory.starts_with(&output),
                "report output {} overlaps Cargo's {kind} directory {}",
                output.display(),
                directory.display(),
            );
        }
        Ok(output)
    }
}

/// Resolve existing components in traversal order and retain nonexistent components as a physical tail.
/// A parent segment applies after resolving a preceding symlink, as it does when opening the path.
fn physical_path(path: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "expected absolute path: {}", path.display());
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) => resolved.push(component),
            Component::RootDir => {
                resolved.push(component);
                resolved = resolved
                    .canonicalize()
                    .with_context(|| format!("resolve filesystem root {}", resolved.display()))?;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(_) => {
                resolved.push(component);
                match fs::symlink_metadata(&resolved) {
                    Ok(_) => {
                        resolved = resolved
                            .canonicalize()
                            .with_context(|| format!("resolve directory component {}", resolved.display()))?;
                        ensure!(resolved.is_dir(), "path component is not a directory: {}", resolved.display());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error).with_context(|| format!("inspect directory component {}", resolved.display())),
                }
            }
        }
    }
    Ok(resolved)
}

/// The payload of one Cargo `compiler-artifact` JSON message.
#[derive(Debug, Deserialize)]
pub(crate) struct ArtifactReceipt {
    /// Source-aware package ID in the producing Cargo resolution.
    pub(crate) package_id: String,
    /// Manifest of the producing source package.
    pub(crate) manifest_path: PathBuf,
    /// Cargo target identity and kind.
    pub(crate) target: Target,
    /// Active features of the producing package.
    #[serde(default)]
    pub(crate) features: BTreeSet<String>,
    /// Concrete files reported by Cargo, including the final cdylib for a normal library.
    pub(crate) filenames: Vec<PathBuf>,
    /// Final executable for a binary target.
    pub(crate) executable: Option<PathBuf>,
}

impl ArtifactReceipt {
    /// Require the same resolved source before moving any artifact into a deployment.
    pub(crate) fn require_source(&self, id: &str, manifest: &Path) -> Result<()> {
        ensure!(
            self.package_id == id,
            "unexpected {} artifact package ID: {}",
            self.target.name,
            self.package_id
        );
        let actual = self.manifest_path.canonicalize()?;
        ensure!(actual == manifest, "unexpected {} artifact manifest: {}", self.target.name, actual.display());
        Ok(())
    }
}

/// Cargo target identity distinguishes normal libraries, bins and example outputs.
#[derive(Debug, Deserialize)]
pub(crate) struct Target {
    /// Rust target name, independent of the filename suffix.
    pub(crate) name: String,
    /// All emitting roles declared for the target.
    pub(crate) kind: Vec<String>,
}

/// Cargo emits compiler artifacts alongside diagnostics and build-script messages.
#[derive(Deserialize)]
#[serde(tag = "reason")]
enum Message {
    /// One built target or a verified fresh result in the current Cargo run.
    #[serde(rename = "compiler-artifact")]
    Artifact {
        /// Receipt fields share the top-level Cargo message.
        #[serde(flatten)]
        receipt: ArtifactReceipt,
    },
    /// A diagnostic or other event which carries no artifact.
    #[serde(other)]
    Other,
}

/// Decode all Cargo messages and retain actual compiler artifact records.
pub(crate) fn receipts(output: &[u8]) -> Result<Vec<ArtifactReceipt>> {
    let mut receipts = Vec::new();
    for line in output.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
        match serde_json::from_slice(line).context("invalid Cargo JSON message")? {
            Message::Artifact { receipt } => receipts.push(receipt),
            Message::Other => {}
        }
    }
    Ok(receipts)
}

/// Verify that an emitted file really lives below the run-owned final output root.
pub(crate) fn owned_artifact(path: &Path, target: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "Cargo reported a relative artifact path: {}", path.display());
    let artifact = path.canonicalize().with_context(|| format!("resolve Cargo artifact {}", path.display()))?;
    let regular = fs::symlink_metadata(path)?.is_file();
    ensure!(
        artifact.starts_with(target) && regular,
        "Cargo artifact {} is outside the run's final target directory {} or is not a file",
        artifact.display(),
        target.display(),
    );
    Ok(artifact)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both effective roots are rejected before either missing root or output is created.
    #[test]
    fn report_stays_outside_merged_and_split_missing_roots() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().canonicalize().unwrap();
        let target = parent.join("missing-final");
        let build = parent.join("missing-intermediate");
        for directories in [
            CargoDirectories {
                target: target.clone(),
                build: target.clone(),
            },
            CargoDirectories {
                target: target.clone(),
                build: build.clone(),
            },
        ] {
            for root in [&directories.target, &directories.build] {
                assert!(directories.report_path(root).is_err());
                assert!(directories.report_path(&root.join("missing/report")).is_err());
                assert!(!root.exists());
            }
            let sibling = parent.join("missing-final-report");
            assert_eq!(directories.report_path(&sibling).unwrap(), sibling);
        }
    }

    /// Existing ancestors can alias either build root even when later components do not yet exist.
    #[cfg(unix)]
    #[test]
    fn report_resolves_aliases_and_parent_segments() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().canonicalize().unwrap();
        let real = parent.join("physical");
        fs::create_dir_all(real.join("child")).unwrap();
        let alias = parent.join("alias");
        symlink(real.join("child"), &alias).unwrap();
        let target = real.join("final");
        let build = real.join("build");
        let directories = CargoDirectories {
            target: target.clone(),
            build: alias.join("../build"),
        };
        for name in ["final", "build"] {
            assert!(directories.report_path(&alias.join("../").join(name).join("report")).is_err());
        }
        assert!(directories.report_path(&alias.join("../")).is_err());
        assert!(!target.exists() && !build.exists());
        let output = alias.join("../../outside/missing");
        assert_eq!(directories.report_path(&output).unwrap(), parent.join("outside/missing"));
    }
}
