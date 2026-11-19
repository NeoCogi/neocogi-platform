//! Owns the single library copied from a Cargo profile into its development layout.
use super::{
    copy_candidate, files_equal,
    resources::{check_destination, lock_output},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    io::Read,
    path::{Component, Path},
    time::SystemTime,
};

/// Only this storage format can authorize retirement.
const RECEIPT_VERSION: u8 = 1;

/// Identifies an installed regular file and its exact contents and mode.
#[derive(Deserialize, Serialize, PartialEq)]
struct Stamp {
    /// SHA-256 of the bytes, including edits that restore a timestamp.
    contents: [u8; 32],
    /// Last write time helps distinguish manual replacement with identical bytes.
    modified: SystemTime,
    /// Creation time on filesystems that support it.
    created: Option<SystemTime>,
    /// The portable permission flag, stored even on systems without Unix modes.
    readonly: bool,
    /// Unix device, inode and mode distinguish renamed or chmod-ed manual copies.
    #[cfg(unix)]
    unix: (u64, u64, u32),
}

impl Stamp {
    /// Reads bounded scratch chunks after identifying an ordinary file without following a link.
    fn read(path: &Path) -> io::Result<Option<Self>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut digest = Sha256::new();
        io::copy(&mut fs::File::open(path)?, &mut digest)?;
        Ok(Some(Self {
            contents: digest.finalize().into(),
            modified: metadata.modified()?,
            created: metadata.created().ok(),
            readonly: metadata.permissions().readonly(),
            #[cfg(unix)]
            unix: {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino(), metadata.mode())
            },
        }))
    }
}

/// Receipts live in the Cargo profile and authorize no operations in exported layouts.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    /// Allows later versions to change ownership policy without guessing compatibility.
    version: u8,
    /// Binds this record to the canonical profile and exact library filename.
    owner: [u8; 32],
    /// Contents and identity of the most recently prepared copy.
    staged: Stamp,
    /// Former owned copy survives as the authority if publication is interrupted.
    previous: Option<Stamp>,
}

impl Receipt {
    /// Unreadable metadata propagates. Missing or malformed receipts grant no deletion authority.
    fn read(path: &Path, owner: [u8; 32]) -> io::Result<Option<Self>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !metadata.is_file() || metadata.len() > 4096 {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        fs::File::open(path)?.take(4096).read_to_end(&mut bytes)?;
        let receipt = serde_json::from_slice::<Self>(&bytes).ok();
        Ok(receipt.filter(|receipt| receipt.version == RECEIPT_VERSION && receipt.owner == owner))
    }

    /// Atomically publishes one complete receipt beside the Cargo profile.
    fn write(&self, path: &Path) -> io::Result<()> {
        let mut temporary = tempfile::NamedTempFile::new_in(path.parent().expect("receipt in profile"))?;
        serde_json::to_writer(&mut temporary, self).map_err(io::Error::other)?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    /// Both possible files are owned if an interrupted rename left the old copy in place.
    fn owns(&self, stamp: &Stamp) -> bool {
        &self.staged == stamp || self.previous.as_ref() == Some(stamp)
    }
}

/// Mirrors a normal Cargo library into its own profile's `lib` directory.
/// A present source refreshes an ordinary staged file when the contents differ.
/// A definitely missing source retires only an unchanged file this helper published.
/// Writers using these layout helpers share the output lock. Source files must
/// be stable while copied.
pub fn mirror_library(profile: &Path, filename: &str) -> io::Result<()> {
    let mut components = Path::new(filename).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(io::Error::other("Cargo library filename must be one ordinary path component"));
    }
    let root = profile.canonicalize()?;
    let source = root.join(filename);
    let directory = root.join("lib");
    let destination = directory.join(filename);
    let marker = root.join(format!(".neocogi-library-{filename}.json"));
    let owner = Sha256::digest(source.as_os_str().as_encoded_bytes()).into();
    let _lock = lock_output(&root)?;
    let receipt = Receipt::read(&marker, owner)?;
    match fs::symlink_metadata(&source) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(io::Error::other("Cargo library input must be a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(receipt) = receipt {
                // Inspect the directory itself first. An application-installed symlink
                // under the profile never grants this helper authority over another root.
                let ordinary_directory = match fs::symlink_metadata(&directory) {
                    Ok(metadata) => metadata.is_dir(),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                    Err(error) => return Err(error),
                };
                if ordinary_directory && Stamp::read(&destination)?.is_some_and(|stamp| receipt.owns(&stamp)) {
                    fs::remove_file(destination)?;
                }
                fs::remove_file(marker)?;
            }
            return Ok(());
        }
        Err(error) => return Err(error),
    }
    check_destination(&root, &destination)?;
    check_destination(&root, &marker)?;
    // Leaving a byte-identical manual deployment alone cannot grant deletion authority.
    if files_equal(&source, &destination)? {
        return Ok(());
    }
    let previous = match receipt {
        Some(receipt) => Stamp::read(&destination)?.filter(|stamp| receipt.owns(stamp)),
        None => None,
    };
    let candidate = copy_candidate(&source, &destination)?;
    let staged = Stamp::read(candidate.path())?.ok_or_else(|| io::Error::other("library candidate is not a regular file"))?;
    let mut receipt = Receipt {
        version: RECEIPT_VERSION,
        owner,
        staged,
        previous,
    };
    // Record both possible outcomes before the rename. If either publication is
    // interrupted, the surviving exact owned file still has a valid receipt.
    receipt.write(&marker)?;
    candidate.persist(destination).map_err(|error| error.error)?;
    if receipt.previous.take().is_some() {
        receipt.write(&marker)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the exact unchanged output of the development mirror is retired.
    #[test]
    fn restores_while_source_exists_and_retires_after_removal() {
        let profile = tempfile::tempdir().unwrap();
        let root = profile.path();
        let source = root.join("native.so");
        let deployed = root.join("lib/native.so");
        fs::write(&source, b"first provider").unwrap();
        mirror_library(root, "native.so").unwrap();
        assert_eq!(fs::read(&deployed).unwrap(), b"first provider");
        fs::remove_file(&deployed).unwrap();
        mirror_library(root, "native.so").unwrap();
        assert_eq!(fs::read(&deployed).unwrap(), b"first provider");
        fs::write(&source, b"replacement provider").unwrap();
        mirror_library(root, "native.so").unwrap();
        assert_eq!(fs::read(&deployed).unwrap(), b"replacement provider");
        fs::remove_file(&source).unwrap();
        mirror_library(root, "native.so").unwrap();
        assert!(!deployed.exists());
        mirror_library(root, "native.so").unwrap();
    }

    /// Manual copies and later changes cannot inherit deletion authority, and
    /// an export remains detached from the profile's treatment of the source.
    #[test]
    fn preserves_manual_modified_and_exported_copies() {
        let profile = tempfile::tempdir().unwrap();
        let export = tempfile::tempdir().unwrap();
        let root = profile.path();
        let source = root.join("native.so");
        let deployed = root.join("lib/native.so");
        fs::create_dir(deployed.parent().unwrap()).unwrap();
        fs::write(&source, b"provider").unwrap();
        fs::copy(&source, &deployed).unwrap();
        mirror_library(root, "native.so").unwrap();
        fs::remove_file(&source).unwrap();
        mirror_library(root, "native.so").unwrap();
        assert_eq!(fs::read(&deployed).unwrap(), b"provider");

        fs::write(&source, b"rebuilt provider").unwrap();
        super::super::stage_library(&source, export.path()).unwrap();
        mirror_library(root, "native.so").unwrap();
        fs::write(&deployed, b"manual change").unwrap();
        fs::remove_file(&source).unwrap();
        mirror_library(root, "native.so").unwrap();
        assert_eq!(fs::read(&deployed).unwrap(), b"manual change");
        assert_eq!(fs::read(export.path().join("lib/native.so")).unwrap(), b"rebuilt provider");
    }

    /// Either side of the file rename can be recovered from the pending receipt.
    #[test]
    fn interrupted_receipt_can_retire_either_published_owned_file() {
        for published in [false, true] {
            let profile = tempfile::tempdir().unwrap();
            let root = profile.path().canonicalize().unwrap();
            let source = root.join("native.so");
            let deployed = root.join("lib/native.so");
            let marker = root.join(".neocogi-library-native.so.json");
            let owner = Sha256::digest(source.as_os_str().as_encoded_bytes()).into();
            fs::write(&source, b"previous provider").unwrap();
            mirror_library(&root, "native.so").unwrap();
            fs::write(&source, b"replacement provider").unwrap();
            let candidate = copy_candidate(&source, &deployed).unwrap();
            Receipt {
                version: RECEIPT_VERSION,
                owner,
                staged: Stamp::read(candidate.path()).unwrap().unwrap(),
                previous: Stamp::read(&deployed).unwrap(),
            }
            .write(&marker)
            .unwrap();
            if published {
                candidate.persist(&deployed).unwrap();
            }
            fs::remove_file(source).unwrap();
            mirror_library(&root, "native.so").unwrap();
            assert!(!deployed.exists());
            assert!(!marker.exists());
        }
    }

    /// A marker from another root or format confers no retirement authority.
    #[test]
    fn preserves_files_when_receipts_are_malformed_foreign_or_unsupported() {
        for damage in ["malformed", "foreign", "version", "unknown"] {
            let profile = tempfile::tempdir().unwrap();
            let root = profile.path();
            let source = root.join("native.so");
            let deployed = root.join("lib/native.so");
            let marker = root.join(".neocogi-library-native.so.json");
            fs::write(&source, b"provider").unwrap();
            mirror_library(root, "native.so").unwrap();
            let mut data: serde_json::Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
            match damage {
                "malformed" => data = serde_json::Value::Null,
                "foreign" => data["owner"][0] = serde_json::json!(data["owner"][0].as_u64().unwrap() ^ 1),
                "version" => data["version"] = serde_json::json!(RECEIPT_VERSION + 1),
                _ => data["unexpected"] = serde_json::json!(true),
            }
            fs::write(marker, serde_json::to_vec(&data).unwrap()).unwrap();
            fs::remove_file(source).unwrap();
            mirror_library(root, "native.so").unwrap();
            assert_eq!(fs::read(deployed).unwrap(), b"provider");
        }
    }

    /// Unknown path shapes and links neither certify source absence nor supply a removable copy.
    #[cfg(unix)]
    #[test]
    fn does_not_retire_on_nonaligned_paths_or_follow_a_replaced_directory_link() {
        use std::os::unix::fs::symlink;
        let profile = tempfile::tempdir().unwrap();
        let export = tempfile::tempdir().unwrap();
        let root = profile.path();
        let source = root.join("native.so");
        let directory = root.join("lib");
        fs::write(&source, b"provider").unwrap();
        mirror_library(root, "native.so").unwrap();
        fs::remove_file(&source).unwrap();
        symlink(root.join("missing"), &source).unwrap();
        assert!(mirror_library(root, "native.so").is_err());
        assert!(directory.join("native.so").is_file());
        fs::remove_file(&source).unwrap();
        fs::rename(&directory, export.path().join("actual-lib")).unwrap();
        symlink(export.path().join("actual-lib"), &directory).unwrap();
        mirror_library(root, "native.so").unwrap();
        assert!(directory.join("native.so").is_file());
    }
}
