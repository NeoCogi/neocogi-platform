//! Exclusive resource ownership and complete subtree replacement at build time.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

/// One authored input mapped to an exclusively owned application-relative destination.
pub struct Resource {
    /// Regular file or directory; the supplied path's ancestors and contents must not traverse symlinks.
    pub source: PathBuf,
    /// Nonempty relative subtree; parent traversal, lib and receipt names are reserved.
    pub destination: PathBuf,
}

/// Actual build-script inputs/outputs to track after successful staging, including directories.
#[derive(Debug, Default)]
pub struct StagedResources {
    /// Authored files and directory membership used to construct the snapshot.
    pub inputs: Vec<PathBuf>,
    /// Complete deployed snapshot; changed or missing members invalidate cached build-script execution.
    pub outputs: Vec<PathBuf>,
}

/// Versioned build-only receipts; serialized paths are relative and reveal no checkout root.
#[derive(Serialize, Deserialize)]
struct Receipts {
    /// Reject incompatible metadata rather than guessing its deletion authority.
    version: u32,
    /// Each entry grants exactly one package ownership of one nonoverlapping subtree.
    resources: Vec<Ownership>,
}

/// A package's stable build slot. Rebuilding may change its source, never its deletion scope.
#[derive(Serialize, Deserialize)]
struct Ownership {
    /// SHA-256 of the canonical consuming manifest path, not a runtime identity.
    owner: [u8; 32],
    /// Normalized application-relative destination owned by this slot.
    destination: PathBuf,
}

/// Complete candidate built before any destination changes.
struct Prepared {
    /// Owns the candidate payload and, during publication, the previous complete output.
    temporary: tempfile::TempDir,
    /// Absolute destination already checked against ownership and symlink traversal.
    destination: PathBuf,
}

/// Validated source shape determines whether a candidate directory could be copied as source content.
#[derive(Clone, Copy)]
enum SourceKind {
    /// A file has no descendant directory in which a temporary candidate could appear.
    File,
    /// Every descendant of this directory belongs to the input tree during preparation.
    Directory,
}

/// One source checked in its supplied spelling before resolving its physical path.
struct Source {
    /// Canonical path used for containment checks and copying, only after every ancestor rejects links.
    path: PathBuf,
    /// Ordinary file or directory; links and special files never reach this representation.
    kind: SourceKind,
}

/// Complete mapping plan, validated with every other mapping before any candidate directory exists.
struct Plan {
    /// Owned source identity and shape, independent of output preparation order.
    source: Source,
    /// Absolute, ownership-checked slot below the canonical application output root.
    destination: PathBuf,
}

impl Plan {
    /// Candidates are siblings of this destination so final replacement stays on the same filesystem.
    fn temporary_parent(&self) -> &Path {
        self.destination.parent().expect("validated nonempty relative destination")
    }
}

/// Installs exact resource snapshots, serializing compiler processes sharing an output root.
///
/// One package may rebuild the same destination with a different source. Distinct slots
/// must not overlap, including across packages. Existing unclaimed destinations reject:
/// the caller must use a clean output before this tool can acquire deletion authority.
/// Every input is copied before publication; filesystem commit errors can leave earlier
/// complete mappings installed, but never turn a partial candidate into a valid snapshot.
/// Supplied input paths reject symlink ancestors before canonicalization. Every temporary
/// sibling directory must lie outside every directory input, including inputs of other mappings.
pub fn stage_resources(manifest: &Path, root: &Path, resources: &[Resource]) -> io::Result<StagedResources> {
    // Resolve all sources before output setup can introduce new directories or files inside an input.
    // Inspect the supplied path first: canonicalization alone would conceal a symlink ancestor.
    let sources = resources.iter().map(|resource| source(&resource.source)).collect::<io::Result<Vec<_>>>()?;
    fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    let owner: [u8; 32] = Sha256::digest(manifest.canonicalize()?.as_os_str().as_encoded_bytes()).into();
    // Build scripts can run in several Cargo processes. File ownership must be
    // checked and committed under this cross-process lock, with no foreign callbacks.
    check_destination(&root, &root.join(".neocogi-layout.lock"))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".neocogi-layout.lock"))?;
    lock.lock()?;
    let path = root.join(".neocogi-layout.json");
    check_destination(&root, &path)?;
    let mut receipts: Receipts = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Receipts { version: 1, resources: Vec::new() },
        Err(error) => return Err(error),
    };
    if receipts.version != 1 {
        return Err(io::Error::other("unsupported native resource ownership receipts; use a clean output"));
    }
    let destinations = resources
        .iter()
        .map(|resource| relative(&resource.destination))
        .collect::<io::Result<Vec<_>>>()?;
    for (index, destination) in destinations.iter().enumerate() {
        if destinations[..index].iter().any(|other| overlaps(other, destination)) {
            return Err(io::Error::other("resource mappings in one invocation must not overlap"));
        }
        let mut claimed = false;
        for receipt in &receipts.resources {
            if overlaps(&receipt.destination, destination) {
                if receipt.destination == *destination && receipt.owner == owner {
                    claimed = true;
                } else {
                    return Err(io::Error::other(format!(
                        "resource destination overlaps an owned subtree: {}",
                        destination.display()
                    )));
                }
            }
        }
        let absolute = root.join(destination);
        check_destination(&root, &absolute)?;
        if !claimed {
            if absolute.exists() {
                return Err(io::Error::other(format!(
                    "resource destination already exists without ownership: {}; use a clean output",
                    destination.display()
                )));
            }
            receipts.resources.push(Ownership { owner, destination: destination.clone() });
        }
    }

    let plans = sources
        .into_iter()
        .zip(destinations)
        .map(|(source, relative)| Plan { source, destination: root.join(relative) })
        .collect::<Vec<_>>();
    for plan in &plans {
        for input in &plans {
            match input.source.kind {
                SourceKind::Directory if plan.temporary_parent().starts_with(&input.source.path) => {
                    return Err(io::Error::other(format!(
                        "temporary directory beside destination {} would be inside resource input {}; choose an output outside that input",
                        plan.destination.display(),
                        input.source.path.display()
                    )));
                }
                SourceKind::File | SourceKind::Directory => {}
            }
        }
    }

    let mut tracked = StagedResources::default();
    let mut prepared = Vec::new();
    for plan in plans {
        // All containment checks have finished. No earlier candidate can enter any later input,
        // including empty directories or another mapping that copies an existing output subtree.
        fs::create_dir_all(plan.temporary_parent())?;
        let temporary = tempfile::Builder::new().prefix(".neocogi-resource-").tempdir_in(plan.temporary_parent())?;
        copy_tree(&plan.source.path, &temporary.path().join("payload"), &mut tracked.inputs)?;
        prepared.push(Prepared { temporary, destination: plan.destination });
    }
    // Reserve ownership before publication. A failed commit remains retryable by
    // this package; another package cannot adopt partially installed output.
    let mut metadata = tempfile::NamedTempFile::new_in(&root)?;
    serde_json::to_writer(&mut metadata, &receipts).map_err(io::Error::other)?;
    metadata.write_all(b"\n")?;
    metadata.persist(&path).map_err(|error| error.error)?;

    for prepared in prepared {
        let candidate = prepared.temporary.path().join("payload");
        if !same_tree(&candidate, &prepared.destination)? {
            let previous = prepared.temporary.path().join("previous");
            let existed = prepared.destination.exists();
            if existed {
                fs::rename(&prepared.destination, &previous)?;
            }
            if let Err(error) = fs::rename(&candidate, &prepared.destination) {
                if existed && let Err(restore) = fs::rename(&previous, &prepared.destination) {
                    // Preserve the old complete output for manual recovery instead
                    // of letting TempDir delete it after an unsuccessful rollback.
                    let recovery = prepared.temporary.keep();
                    return Err(io::Error::other(format!(
                        "resource publication failed ({error}); restore failed ({restore}); previous output retained at {}",
                        recovery.display()
                    )));
                }
                return Err(error);
            }
        }
        collect_outputs(&prepared.destination, &mut tracked.outputs)?;
    }
    Ok(tracked)
}

/// Validates the authored path component by component before canonicalization can erase link traversal.
/// Parent and trailing-dot forms retain their ordering during this check. Children are checked again
/// by `copy_tree`; this is a build-input policy, not a sandbox against concurrent source mutation.
fn source(path: &Path) -> io::Result<Source> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(io::Error::other(format!(
                "resource input must not traverse a symlink: {} (input {})",
                current.display(),
                path.display()
            )));
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    let kind = if metadata.is_file() {
        SourceKind::File
    } else if metadata.is_dir() {
        SourceKind::Directory
    } else {
        return Err(io::Error::other(
            "resource inputs must be regular files/directories, never symlinks or special files",
        ));
    };
    Ok(Source { path: path.canonicalize()?, kind })
}

/// Rejects escaping/root destinations and names owned by library/receipt staging.
fn relative(path: &Path) -> io::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => normalized.push(name),
            Component::CurDir => {}
            _ => return Err(io::Error::other("resource destination must stay inside the application directory")),
        }
    }
    let Some(first) = normalized.components().next() else {
        return Err(io::Error::other("resource destination must own a nonempty subtree, not the output root"));
    };
    if first.as_os_str() == "lib" || first.as_os_str().to_string_lossy().starts_with(".neocogi-") {
        return Err(io::Error::other("lib and .neocogi-* destinations are reserved for native layout"));
    }
    Ok(normalized)
}

/// Prefix overlap means one owner could otherwise prune another owner's output.
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Rejects destination symlinks at every existing component below the canonical output.
pub(crate) fn check_destination(root: &Path, destination: &Path) -> io::Result<()> {
    let mut current = root.to_path_buf();
    for component in destination.strip_prefix(root).map_err(io::Error::other)?.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Err(io::Error::other("native layout destinations must not traverse symlinks")),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Builds a complete candidate with deterministic membership and no symlink traversal.
fn copy_tree(source: &Path, destination: &Path, inputs: &mut Vec<PathBuf>) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    inputs.push(source.to_owned());
    if metadata.is_dir() {
        fs::create_dir(destination)?;
        for path in children(source)? {
            copy_tree(&path, &destination.join(path.file_name().expect("directory entry name")), inputs)?;
        }
        Ok(())
    } else if metadata.is_file() {
        // The payload is new and privately owned; equality cannot avoid this copy.
        fs::copy(source, destination).map(|_| ())
    } else {
        Err(io::Error::other(
            "resource inputs must be regular files/directories, never symlinks or special files",
        ))
    }
}

/// Sorted children make snapshot comparison independent of filesystem enumeration order.
fn children(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    paths.sort();
    Ok(paths)
}

/// Equality permits identical builds to preserve deployed file timestamps.
fn same_tree(a: &Path, b: &Path) -> io::Result<bool> {
    let left = fs::symlink_metadata(a)?;
    let right = match fs::symlink_metadata(b) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if left.file_type() != right.file_type() {
        return Ok(false);
    }
    if left.is_file() {
        return super::files_equal(a, b);
    }
    if !left.is_dir() {
        return Ok(false);
    }
    let a = children(a)?;
    let b = children(b)?;
    if a.len() != b.len() {
        return Ok(false);
    }
    for (a, b) in a.iter().zip(&b) {
        if a.file_name() != b.file_name() || !same_tree(a, b)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Tracks directories as well as files, so deleted members and empty trees are visible.
fn collect_outputs(path: &Path, outputs: &mut Vec<PathBuf>) -> io::Result<()> {
    outputs.push(path.to_owned());
    if path.is_dir() {
        for child in children(path)? {
            collect_outputs(&child, outputs)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
