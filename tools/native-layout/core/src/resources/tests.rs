//! Filesystem behavior of exclusive slots, preparation failure and competing owners.
use super::*;

/// Isolated package and output roots; every write stays inside this temporary fixture.
struct Fixture {
    /// Keeps all source/output directories alive until the assertion finishes.
    _directory: tempfile::TempDir,
    /// Consuming package identity, distinct from the source tree.
    manifest: PathBuf,
    /// Mutable authored resources.
    source: PathBuf,
    /// Shared staged application root.
    output: PathBuf,
}
impl Fixture {
    /// Constructs a real package-like source tree with one file.
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let manifest = directory.path().join("package");
        let source = manifest.join("source");
        let output = directory.path().join("output");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("keep"), b"one").unwrap();
        Self {
            _directory: directory,
            manifest,
            source,
            output,
        }
    }
    /// Maps the complete source directory into one destination slot.
    fn mapping(&self, destination: &str) -> Resource {
        Resource {
            source: self.source.clone(),
            destination: destination.into(),
        }
    }
    /// Installs the fixture's ordinary complete snapshot.
    fn stage(&self) -> io::Result<StagedResources> {
        stage_resources(&self.manifest, &self.output, &[self.mapping("assets")])
    }
}

/// A successful rebuild is an exact snapshot, including empty directories and deletions.
#[test]
fn snapshots_prune_rename_preserve_timestamps_and_restore_deleted_outputs() {
    let f = Fixture::new();
    fs::write(f.source.join("obsolete"), []).unwrap();
    fs::create_dir(f.source.join("empty")).unwrap();
    let first = f.stage().unwrap();
    assert!(first.outputs.contains(&f.output.join("assets/empty")));
    let timestamp = fs::metadata(f.output.join("assets/keep")).unwrap().modified().unwrap();
    f.stage().unwrap();
    assert_eq!(fs::metadata(f.output.join("assets/keep")).unwrap().modified().unwrap(), timestamp);
    fs::remove_file(f.source.join("obsolete")).unwrap();
    fs::rename(f.source.join("keep"), f.source.join("renamed")).unwrap();
    f.stage().unwrap();
    assert!(!f.output.join("assets/obsolete").exists());
    assert!(!f.output.join("assets/keep").exists());
    assert_eq!(fs::read(f.output.join("assets/renamed")).unwrap(), b"one");
    fs::remove_dir_all(f.output.join("assets")).unwrap();
    f.stage().unwrap();
    assert!(f.output.join("assets/empty").is_dir());
    let receipts = fs::read_to_string(f.output.join(".neocogi-layout.json")).unwrap();
    assert!(!receipts.contains(f.manifest.to_str().unwrap()), "receipt must not disclose build paths");
}

/// One package owns its destination slot even when the source or shape changes.
#[test]
fn a_b_a_and_file_directory_replacement() {
    let f = Fixture::new();
    let file = f.manifest.join("file");
    fs::write(&file, b"A").unwrap();
    let mapping = || Resource {
        source: file.clone(),
        destination: "assets".into(),
    };
    stage_resources(&f.manifest, &f.output, &[mapping()]).unwrap();
    assert!(f.output.join("assets").is_file());
    f.stage().unwrap();
    assert!(f.output.join("assets/keep").is_file());
    stage_resources(&f.manifest, &f.output, &[mapping()]).unwrap();
    assert_eq!(fs::read(f.output.join("assets")).unwrap(), b"A");
    fs::create_dir(f.source.join("nested")).unwrap();
    fs::write(f.source.join("nested/member"), []).unwrap();
    f.stage().unwrap();
    fs::remove_dir_all(f.source.join("nested")).unwrap();
    f.stage().unwrap();
    assert!(!f.output.join("assets/nested").exists());
}

/// Prefix/duplicate mappings reject before any candidate can replace existing output.
#[test]
fn overlaps_and_invalid_paths_are_rejected_before_publication() {
    let f = Fixture::new();
    for (a, b) in [("assets", "assets/child"), ("assets/child", "assets"), ("assets", "./assets")] {
        assert!(stage_resources(&f.manifest, &f.output, &[f.mapping(a), f.mapping(b)]).is_err());
    }
    for path in [
        "",
        ".",
        "../escape",
        "/absolute",
        "a/../../escape",
        "lib",
        "lib/plugin.so",
        ".neocogi-layout.json",
    ] {
        assert!(stage_resources(&f.manifest, &f.output, &[f.mapping(path)]).is_err(), "{path}");
    }
    assert!(!f.output.join("assets").exists());
}

/// Unclaimed output is never silently adopted and deleted.
#[test]
fn existing_unclaimed_content_and_other_packages_are_preserved() {
    let f = Fixture::new();
    fs::create_dir_all(f.output.join("assets")).unwrap();
    fs::write(f.output.join("assets/foreign"), b"foreign").unwrap();
    assert!(f.stage().unwrap_err().to_string().contains("without ownership"));
    assert_eq!(fs::read(f.output.join("assets/foreign")).unwrap(), b"foreign");
    fs::remove_dir_all(f.output.join("assets")).unwrap();
    f.stage().unwrap();
    let other = f.manifest.with_file_name("other");
    fs::create_dir(&other).unwrap();
    for destination in ["assets", "assets/child", "."] {
        assert!(stage_resources(&other, &f.output, &[f.mapping(destination)]).is_err());
    }
    stage_resources(&other, &f.output, &[f.mapping("other-assets")]).unwrap();
    assert!(f.output.join("assets/keep").exists());
    assert!(f.output.join("other-assets/keep").exists());
}

/// Every candidate is prepared before publishing any mapping.
#[test]
fn missing_later_source_does_not_publish_an_earlier_candidate() {
    let f = Fixture::new();
    f.stage().unwrap();
    fs::write(f.source.join("keep"), b"new").unwrap();
    let missing = Resource {
        source: f.manifest.join("missing"),
        destination: "another".into(),
    };
    assert!(stage_resources(&f.manifest, &f.output, &[f.mapping("assets"), missing]).is_err());
    assert_eq!(fs::read(f.output.join("assets/keep")).unwrap(), b"one");
}

/// Competing compiler processes/threads cannot both claim a shared subtree.
#[test]
fn simultaneous_package_claims_have_one_owner() {
    let f = Fixture::new();
    let other = f.manifest.with_file_name("other");
    fs::create_dir(&other).unwrap();
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| stage_resources(&f.manifest, &f.output, &[f.mapping("assets")]).is_ok());
        let b = scope.spawn(|| stage_resources(&other, &f.output, &[f.mapping("assets")]).is_ok());
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_ne!(results.0, results.1);
    assert!(f.output.join("assets/keep").is_file());
}

/// Symlinks neither expand deletion authority nor become copied inputs.
#[cfg(unix)]
#[test]
fn symlinks_reject_without_following_external_storage() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = f.manifest.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("protected"), b"protected").unwrap();
    symlink(&outside, f.source.join("link")).unwrap();
    assert!(f.stage().is_err());
    fs::remove_file(f.source.join("link")).unwrap();
    fs::create_dir_all(&f.output).unwrap();
    symlink(&outside, f.output.join("assets")).unwrap();
    assert!(f.stage().is_err());
    assert_eq!(fs::read(outside.join("protected")).unwrap(), b"protected");
    fs::remove_file(f.output.join("assets")).unwrap();
    f.stage().unwrap();
    symlink(&outside, f.output.join("assets/extra")).unwrap();
    f.stage().unwrap();
    assert!(!f.output.join("assets/extra").exists());
    assert!(outside.join("protected").exists(), "pruning must not follow an old child symlink");
}
