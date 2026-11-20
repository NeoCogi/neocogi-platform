//! Build-only native layout policy, with no renderer, COM or IDL dependency.
//!
//! Resource mappings exclusively own their destination subtree. Ownership receipts
//! contain an opaque hash of the consuming manifest directory, never checkout paths.
//! Filesystem locking serializes competing compiler processes, not application state.
#![deny(missing_docs)]

mod mirror;
mod resources;
pub use mirror::mirror_library;
pub use resources::{Resource, StagedResources, stage_resources};

use std::{
    fs, io,
    io::Read,
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
    inferred_profile(anchor).ok_or_else(|| {
        io::Error::other("native output inference requires a consumer build.rs and Cargo OUT_DIR; set an absolute NEOCOGI_NATIVE_OUTPUT_DIR instead")
    })
}

/// Find the Cargo final profile when Cargo's build and target roots share an ancestor.
fn inferred_profile(anchor: Option<&Path>) -> Option<PathBuf> {
    anchor
        .and_then(|anchor| anchor.ancestors().find(|directory| directory.join(".cargo-lock").is_file()))
        .map(Path::to_path_buf)
}

/// Final Cargo artifact input and deployment destination for a native example build.
pub struct CargoLayout {
    /// The selected final profile directory, or no proven provider source location.
    /// Its selected basename identifies the Cargo profile even through a directory alias.
    pub profile: Option<PathBuf>,
    /// The explicit export or the selected profile. Artwork needs only this location.
    pub destination: Option<PathBuf>,
}

/// Resolve final Cargo artifacts separately from deployment files.
/// A split build directory may yield no profile. Both overrides use absolute paths.
/// The input override names an existing final profile directory for this target.
pub fn cargo_layout(anchor: &Path, profile: Option<PathBuf>, destination: Option<PathBuf>) -> io::Result<CargoLayout> {
    let profile = match profile {
        Some(path) => {
            if !path.is_absolute() || !path.is_dir() {
                return Err(io::Error::other(format!(
                    "NEOCOGI_CARGO_PROFILE_DIR must name an existing absolute Cargo final profile directory: {}",
                    path.display()
                )));
            }
            Some(path)
        }
        None => inferred_profile(Some(anchor)),
    };
    let destination = match destination {
        Some(path) => Some(output_root(None, Some(path))?),
        None => profile.clone(),
    };
    Ok(CargoLayout { profile, destination })
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
    let _lock = resources::lock_output(&root)?;
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
    if files_equal(source, destination)? {
        return Ok(());
    }
    let temporary = copy_candidate(source, destination)?;
    temporary.persist(destination).map_err(|error| error.error)?;
    Ok(())
}

/// Copies complete contents into an unpublished sibling on the destination filesystem.
fn copy_candidate(source: &Path, destination: &Path) -> io::Result<tempfile::NamedTempFile> {
    let directory = destination.parent().ok_or_else(|| io::Error::other("staged file has no directory"))?;
    fs::create_dir_all(directory)?;
    let temporary = tempfile::NamedTempFile::new_in(directory)?;
    fs::copy(source, temporary.path())?;
    Ok(temporary)
}

/// Fixed scratch per comparison, independent of library or asset size.
const COMPARE_CHUNK: usize = 8 * 1024;

/// Compares regular-file shape, permissions, length and bytes under the staging
/// contract's no-concurrent-source-mutation assumption. Missing destinations differ;
/// actual I/O failures propagate instead of silently certifying equality.
fn files_equal(a: &Path, b: &Path) -> io::Result<bool> {
    let left = fs::symlink_metadata(a)?;
    let right = match fs::symlink_metadata(b) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !left.is_file() || !right.is_file() || left.len() != right.len() || left.permissions() != right.permissions() {
        return Ok(false);
    }
    equal_readers(fs::File::open(a)?, fs::File::open(b)?, left.len())
}

/// Fills fixed-size chunks independently: valid short reads need not line up
/// between files. Metadata supplied the expected length; premature EOF is an I/O
/// failure, not evidence of unchanged contents. No file-sized vector is retained.
fn equal_readers(mut left: impl Read, mut right: impl Read, mut remaining: u64) -> io::Result<bool> {
    let mut a = [0u8; COMPARE_CHUNK];
    let mut b = [0u8; COMPARE_CHUNK];
    while remaining != 0 {
        let count = remaining.min(COMPARE_CHUNK as u64) as usize;
        left.read_exact(&mut a[..count])?;
        right.read_exact(&mut b[..count])?;
        if a[..count] != b[..count] {
            return Ok(false);
        }
        remaining -= count as u64;
    }
    Ok(true)
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

    /// A detached export always remains independent of whether a final profile is discoverable.
    #[test]
    fn cargo_layout_preserves_split_source_and_destination_roles() {
        let temporary = tempfile::tempdir().unwrap();
        let profile = temporary.path().join("target/triple/min-size");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join(".cargo-lock"), []).unwrap();
        let profile = profile.canonicalize().unwrap();
        let export = temporary.path().join("export");
        let split = temporary.path().join("build/debug/consumer/out");
        let combined = profile.join("build/consumer/out");
        let layout = cargo_layout(&combined, None, None).unwrap();
        assert_eq!(layout.profile.as_ref(), Some(&profile));
        assert_eq!(layout.destination.as_ref(), Some(&profile));
        let layout = cargo_layout(&split, None, Some(export.clone())).unwrap();
        assert!(layout.profile.is_none());
        assert_eq!(layout.destination.as_ref(), Some(&export));
        let layout = cargo_layout(&split, Some(profile.clone()), Some(export.clone())).unwrap();
        assert_eq!(layout.profile.as_ref(), Some(&profile));
        assert_eq!(layout.destination.as_ref(), Some(&export));
        let layout = cargo_layout(&split, Some(profile.clone()), None).unwrap();
        assert_eq!(layout.destination.as_ref(), Some(&profile));
        let layout = cargo_layout(&split, None, None).unwrap();
        assert!(layout.profile.is_none() && layout.destination.is_none());
        for invalid in [PathBuf::from("relative"), export, profile.join(".cargo-lock")] {
            let error = cargo_layout(&combined, Some(invalid), None).err().unwrap();
            assert!(error.to_string().contains("NEOCOGI_CARGO_PROFILE_DIR"));
        }
    }

    /// The final profile name belongs to the selected path even if its bytes live in a cache.
    #[cfg(unix)]
    #[test]
    fn cargo_layout_preserves_the_name_of_an_aliased_profile() {
        let temporary = tempfile::tempdir().unwrap();
        let storage = temporary.path().join("cache-storage");
        let profile = temporary.path().join("debug");
        fs::create_dir(&storage).unwrap();
        std::os::unix::fs::symlink(&storage, &profile).unwrap();
        let layout = cargo_layout(temporary.path(), Some(profile.clone()), Some(storage.clone())).unwrap();
        assert_eq!(layout.profile.as_ref(), Some(&profile));
        assert_eq!(layout.profile.unwrap().canonicalize().unwrap(), storage.canonicalize().unwrap());
        assert_eq!(layout.destination, Some(storage));
    }
}

#[cfg(test)]
mod comparison_tests {
    use super::*;

    /// A reader with deliberately short fills records the largest requested scratch window.
    struct ShortReader<'a> {
        /// Remaining immutable input, borrowed rather than copied per read.
        bytes: &'a [u8],
        /// Independent short-read cadence for this side of the comparison.
        fill: usize,
        /// Observed largest request checks that scratch stays bounded.
        largest: usize,
    }
    impl Read for ShortReader<'_> {
        /// Models legal short reads without changing source order or contents.
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.largest = self.largest.max(output.len());
            let count = output.len().min(self.fill).min(self.bytes.len());
            output[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            Ok(count)
        }
    }

    /// Chunk boundaries, asymmetric short reads, changed final bytes and truncated input remain observable.
    #[test]
    fn comparisons_bound_scratch_without_assuming_full_reads() {
        let bytes = vec![37; 3 * 8192 + 7];
        let mut left = ShortReader { bytes: &bytes, fill: 13, largest: 0 };
        let mut right = ShortReader { bytes: &bytes, fill: 29, largest: 0 };
        assert!(equal_readers(&mut left, &mut right, bytes.len() as u64).unwrap());
        assert!(left.largest <= 8192 && right.largest <= 8192);
        let mut changed = bytes.clone();
        *changed.last_mut().unwrap() = 38;
        assert!(!equal_readers(bytes.as_slice(), changed.as_slice(), bytes.len() as u64).unwrap());
        assert_eq!(equal_readers(&bytes[..4], &bytes[..3], 4).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    /// Unchanged selected libraries retain their old timestamp; content and permissions still replace.
    #[test]
    fn library_replacement_preserves_equal_files_and_detects_changes() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("library");
        fs::write(&source, vec![7; 3 * 8192 + 1]).unwrap();
        let output = directory.path().join("app");
        stage_library(&source, &output).unwrap();
        let staged = output.join("lib/library");
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1234);
        fs::File::options()
            .write(true)
            .open(&staged)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
        stage_library(&source, &output).unwrap();
        assert_eq!(fs::metadata(&staged).unwrap().modified().unwrap(), old);
        fs::write(&source, vec![8; 3 * 8192 + 1]).unwrap();
        stage_library(&source, &output).unwrap();
        assert_eq!(fs::read(&staged).unwrap(), fs::read(&source).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
            stage_library(&source, &output).unwrap();
            assert_eq!(fs::metadata(&staged).unwrap().permissions().mode() & 0o777, 0o640);
        }
    }
}
