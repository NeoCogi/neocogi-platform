//! Build-only destination policy for staged native artifacts and resources.
//!
//! The pinned Cargo layout marks each target/profile directory with .cargo-lock.
//! Walking ancestors of a Cargo-supplied path avoids reconstructing unit hashes,
//! profile names or target triples. Separate build/output directories can supply
//! an explicit NEOCOGI_NATIVE_OUTPUT_DIR; its value never enters runtime code.

use std::{
    io,
    path::{Path, PathBuf},
};

/// Returns the application output directory from an explicit override or Cargo's lock marker.
pub fn output_root(anchor: &Path, explicit: Option<PathBuf>) -> io::Result<PathBuf> {
    if let Some(root) = explicit {
        return if root.is_absolute() {
            Ok(root)
        } else {
            Err(io::Error::other("NEOCOGI_NATIVE_OUTPUT_DIR must be absolute"))
        };
    }
    anchor
        .ancestors()
        .find(|directory| directory.join(".cargo-lock").is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("Cargo output directory unavailable; set NEOCOGI_NATIVE_OUTPUT_DIR for a separate build/output layout"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An actual Cargo anchor selects its closest marked profile, including spaces.
    #[test]
    fn cargo_anchor_and_explicit_output() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("target with spaces").join("custom-profile");
        fs_create_marker(&root);
        let anchor = root.join("build/package/hash/artifact/cdylib/library.so");
        assert_eq!(output_root(&anchor, None).unwrap(), root);
        let separate = temporary.path().join("application");
        assert_eq!(output_root(&anchor, Some(separate.clone())).unwrap(), separate);
        assert!(output_root(&anchor, Some(PathBuf::from("relative"))).is_err());
        assert!(output_root(&temporary.path().join("unmarked"), None).is_err());
    }

    /// Creates the marker without relying on a compiler-specific hash shape.
    fn fs_create_marker(directory: &Path) {
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(directory.join(".cargo-lock"), b"").unwrap();
    }
}
