//! Build-only native layout policy, with no renderer, COM or IDL dependency.
//!
//! Resource mappings exclusively own their destination subtree. Ownership receipts
//! contain an opaque hash of the consuming manifest directory, never checkout paths.
//! Filesystem locking serializes competing compiler processes, not application state.
#![deny(missing_docs)]

mod resources;
pub use resources::{Resource, StagedResources, stage_resources};

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Resolves an explicit absolute root first; only inferred output needs a Cargo anchor.
///
/// Automatic inference requires a consumer OUT_DIR (normally supplied by build.rs).
/// The pinned Cargo profile directory is identified by its .cargo-lock marker.
/// No package names, target triples, hashes or nested Cargo graph are reconstructed.
pub fn output_root(anchor: Option<&Path>, explicit: Option<PathBuf>) -> io::Result<PathBuf> {
    if let Some(root) = explicit {
        return if root.is_absolute() {
            Ok(root)
        } else {
            Err(io::Error::other("NEOCOGI_NATIVE_OUTPUT_DIR must be absolute"))
        };
    }
    anchor
        .and_then(|anchor| anchor.ancestors().find(|directory| directory.join(".cargo-lock").is_file()))
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            io::Error::other("native output inference requires a consumer build.rs and Cargo OUT_DIR; set an absolute NEOCOGI_NATIVE_OUTPUT_DIR instead")
        })
}

/// Copies a selected library into app/lib and returns only its UTF-8 filename.
/// Existing loaded Unix mappings remain intact because replacement uses rename.
pub fn stage_library(source: &Path, root: &Path) -> io::Result<String> {
    let filename = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("Cargo library filename must be UTF-8"))?;
    fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    let destination = root.join("lib").join(filename);
    resources::check_destination(&root, &destination)?;
    copy_file(source, &destination)?;
    Ok(filename.to_owned())
}

/// Atomically copies a regular file; identical bytes and permissions retain their mtime.
fn copy_file(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if !metadata.is_file() {
        return Err(io::Error::other("staged input must be a regular file"));
    }
    if fs::read(destination).ok() == Some(fs::read(source)?) && fs::metadata(destination).is_ok_and(|existing| existing.permissions() == metadata.permissions())
    {
        return Ok(());
    }
    let directory = destination.parent().ok_or_else(|| io::Error::other("staged file has no directory"))?;
    fs::create_dir_all(directory)?;
    let temporary = tempfile::NamedTempFile::new_in(directory)?;
    fs::copy(source, temporary.path())?;
    temporary.persist(destination).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Explicit output needs no build script; inference keeps the consumer-root rule.
    #[test]
    fn output_precedence_and_inference() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("profile with spaces");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".cargo-lock"), []).unwrap();
        let anchor = root.join("build/consumer/out");
        assert_eq!(output_root(Some(&anchor), None).unwrap(), root);
        assert_eq!(output_root(None, Some(root.clone())).unwrap(), root);
        assert!(output_root(Some(&anchor), Some("relative".into())).is_err());
        assert!(output_root(None, None).unwrap_err().to_string().contains("build.rs"));
        assert!(output_root(Some(temporary.path()), None).is_err());
    }
}
