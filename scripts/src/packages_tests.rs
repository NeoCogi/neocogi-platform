//! Adversarial archive and release metadata fixtures; no Cargo or network is required.
use super::*;
use flate2::{Compression, write::GzEncoder};

/// The checkout itself and its descendants fail before creating a workspace. A sibling can share its name prefix.
#[test]
fn qualification_requires_an_external_temporary_parent() {
    let directories = tempfile::tempdir().unwrap();
    let checkout = directories.path().join("checkout");
    let inside = checkout.join("inside");
    let sibling = directories.path().join("checkout-sibling");
    fs::create_dir_all(&inside).unwrap();
    fs::create_dir(&sibling).unwrap();
    for parent in [&checkout, &inside] {
        let error = external_workspace(&checkout, parent).unwrap_err().to_string();
        assert!(error.contains(parent.to_str().unwrap()), "{error}");
        assert!(error.contains(TEMP_DIRECTORY_SETTING), "{error}");
    }
    assert_eq!(fs::read_dir(&inside).unwrap().count(), 0);
    let (owner, workspace) = external_workspace(&checkout, &sibling).unwrap();
    assert_eq!(workspace.parent(), Some(sibling.canonicalize().unwrap().as_path()));
    drop(owner);
    assert!(!workspace.exists());
}

/// Alias spellings cannot admit an internal location, and an outside alias uses its real parent.
#[cfg(unix)]
#[test]
fn qualification_checks_the_targets_of_temporary_directory_aliases() {
    use std::os::unix::fs::symlink;

    let directories = tempfile::tempdir().unwrap();
    let checkout = directories.path().join("checkout");
    let inside = checkout.join("inside");
    let outside = directories.path().join("outside");
    let checkout_alias = directories.path().join("checkout-alias");
    let inside_alias = directories.path().join("inside-alias");
    let outside_alias = directories.path().join("outside-alias");
    fs::create_dir_all(&inside).unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(&checkout, &checkout_alias).unwrap();
    symlink(&inside, &inside_alias).unwrap();
    symlink(&outside, &outside_alias).unwrap();
    let error = external_workspace(&checkout_alias, &inside_alias).unwrap_err().to_string();
    assert!(error.contains(inside_alias.to_str().unwrap()), "{error}");
    assert!(error.contains(inside.to_str().unwrap()), "{error}");
    assert_eq!(fs::read_dir(&inside).unwrap().count(), 0);
    let (_owner, workspace) = external_workspace(&checkout_alias, &outside_alias).unwrap();
    assert_eq!(workspace.parent(), Some(outside.canonicalize().unwrap().as_path()));
}

/// Cargo's receipt must name the normal native cdylib; examples and metadata cannot mask a missing library.
#[test]
fn normal_platform_artifact_is_required() {
    let path = format!("/output/{}nc_platform.{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_EXTENSION);
    let receipt = serde_json::json!({
        "reason": "compiler-artifact",
        "package_id": "platform-shim 0.3.0-alpha.2",
        "manifest_path": "/extract/shim/Cargo.toml",
        "target": { "name": "nc_platform", "kind": ["cdylib"] },
        "filenames": ["/output/libnc_platform.rmeta", path]
    });
    let output = format!("{{\"reason\":\"build-finished\",\"success\":true}}\n{receipt}\n");
    assert_eq!(platform_artifact(output.as_bytes()).unwrap().path, PathBuf::from(&path));
    assert!(platform_artifact(format!("{receipt}\n{receipt}\n").as_bytes()).is_err());
    let mut example = receipt.clone();
    example["target"]["kind"] = serde_json::json!(["example"]);
    assert!(platform_artifact(example.to_string().as_bytes()).is_err());
    assert!(platform_artifact(b"").is_err());
    assert!(platform_artifact(b"not JSON").is_err());
}

/// Extracted commands select one owned output and build directory on all toolchains.
#[test]
fn extracted_build_directories_are_explicit_overrides() {
    let workspace = Path::new("/extract");
    let target = Path::new("/qualification/target");
    let command = extracted_cargo("stable", workspace, target);
    assert_eq!(command.get_current_dir(), Some(workspace));
    let env: BTreeMap<_, _> = command.get_envs().collect();
    assert_eq!(env[std::ffi::OsStr::new("CARGO_TARGET_DIR")], Some(target.as_os_str()));
    assert_eq!(env[std::ffi::OsStr::new("CARGO_BUILD_BUILD_DIR")], Some(target.join("build").as_os_str()));
    for key in TEMP_DIRECTORY_KEYS {
        assert_eq!(env[std::ffi::OsStr::new(key)], workspace.parent().map(Path::as_os_str));
    }
}

/// The shim receipt binds the private output to the exact package source selected by this Cargo.
#[test]
fn qualification_accepts_only_its_own_reported_library() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("qualification");
    fs::create_dir(&target).unwrap();
    let filename = format!("{}nc_platform.{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_EXTENSION);
    let manifest = directory.path().join("Cargo.toml");
    let alternate = directory.path().join("alternate.toml");
    fs::write(&manifest, []).unwrap();
    fs::write(&alternate, []).unwrap();
    let manifest = manifest.canonicalize().unwrap();
    let receipt = |path: &Path, package_id: &str, manifest: &Path| {
        fs::write(path, []).unwrap();
        serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": package_id,
            "manifest_path": manifest,
            "target": { "name": "nc_platform", "kind": ["cdylib"] },
            "filenames": [path],
        })
        .to_string()
    };
    let own = target.join(&filename);
    let qualify = |json: String| qualified_library(json.as_bytes(), &target, "resolved-shim", &manifest);
    assert_eq!(qualify(receipt(&own, "resolved-shim", &manifest)).unwrap(), own.canonicalize().unwrap());
    assert!(qualify(receipt(&directory.path().join(filename), "resolved-shim", &manifest)).is_err());
    assert!(qualify(receipt(&own, "alternate-id", &manifest)).is_err());
    assert!(qualify(receipt(&own, "resolved-shim", &alternate)).is_err());
}

/// The whole resolved release set must come from the archives even when names and versions match.
#[test]
fn extracted_resolution_rejects_foreign_and_missing_package_sources() {
    let directory = tempfile::tempdir().unwrap();
    let mut metadata = metadata();
    metadata.workspace_members.remove(SHIM);
    let mut expected = BTreeMap::new();
    for (name, package) in RELEASE_ORDER.iter().zip(&mut metadata.packages) {
        let manifest = directory.path().join(format!("{name}.toml"));
        fs::write(&manifest, []).unwrap();
        package.manifest_path = manifest.clone();
        expected.insert(*name, manifest.canonicalize().unwrap());
    }
    assert_eq!(extracted_shim_id(&metadata, &expected).unwrap(), SHIM);
    let alternate = directory.path().join("alternate.toml");
    fs::write(&alternate, []).unwrap();
    for index in 0..RELEASE_ORDER.len() {
        let original = std::mem::replace(&mut metadata.packages[index].manifest_path, alternate.clone());
        let error = extracted_shim_id(&metadata, &expected).unwrap_err().to_string();
        assert!(error.contains(RELEASE_ORDER[index]) && error.contains("alternate.toml"), "{error}");
        metadata.packages[index].manifest_path = original;
    }
    let mut duplicate = package(SHIM);
    duplicate.id = "duplicate".into();
    duplicate.manifest_path = expected[SHIM].clone();
    metadata.packages.push(duplicate);
    assert!(extracted_shim_id(&metadata, &expected).unwrap_err().to_string().contains("multiple"));
    metadata.packages.pop();
    let shim = metadata.packages.remove(RELEASE_ORDER.len() - 1);
    assert!(extracted_shim_id(&metadata, &expected).unwrap_err().to_string().contains("did not resolve"));
    metadata.workspace_members.insert(shim.id.clone());
    metadata.packages.push(shim);
    assert!(extracted_shim_id(&metadata, &expected).unwrap_err().to_string().contains("nonmember"));
}

/// Fixture registry identity, independently checked against the archived manifest.
fn package(name: &str) -> Package {
    Package {
        id: name.into(),
        name: name.into(),
        version: "0.1.0-alpha.2".into(),
        manifest_path: PathBuf::new(),
        publish: Some(vec!["crates-io".into()]),
    }
}

/// Build an archive with selectable missing inputs, dependency tables and raw malformed members.
fn archive(directory: &Path, owner: &str, omitted: &[&str], dependency: &str, extra: Option<(&str, tar::EntryType)>) -> PathBuf {
    let package = package(owner);
    let prefix = format!("{owner}-{}", package.version);
    let mut files = BTreeMap::from([
        (
            "Cargo.toml".to_owned(),
            format!("[package]\nname={owner:?}\nversion={:?}\n{dependency}", package.version),
        ),
        ("README.md".into(), "Documentation".into()),
        ("LICENSE".into(), "License notice".into()),
    ]);
    if owner == CONTRACTS {
        for index in 0..SCHEMA_COUNT {
            files.insert(format!("schemas/{index}.ncidl"), "declaration".into());
        }
    }
    if matches!(owner, "neocogi-platform" | "neocogi-platform-dylib") {
        for name in DOCUMENTATION_FILES {
            files.insert((*name).to_owned(), "Packaged documentation fixture".into());
        }
    }
    let path = directory.join("fixture.crate");
    let mut archive = tar::Builder::new(GzEncoder::new(fs::File::create(&path).unwrap(), Compression::fast()));
    for (name, content) in files {
        if omitted.contains(&name.as_str()) {
            continue;
        }
        append(&mut archive, &format!("{prefix}/{name}"), tar::EntryType::Regular, content.as_bytes());
    }
    if let Some((name, kind)) = extra {
        append(&mut archive, &format!("{prefix}/{name}"), kind, b"");
    }
    archive.into_inner().unwrap().finish().unwrap();
    path
}

/// Raw headers let traversal tests exercise inspection instead of the builder's path rejection.
fn append(writer: &mut tar::Builder<GzEncoder<fs::File>>, name: &str, kind: tar::EntryType, bytes: &[u8]) {
    assert!(name.len() < 100);
    let mut header = tar::Header::new_gnu();
    header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
    header.set_entry_type(kind);
    header.set_mode(0o644);
    header.set_size(bytes.len() as u64);
    header.set_cksum();
    writer.append(&header, bytes).unwrap();
}

/// Ordinary registry-only Cargo manifests remain sufficient; inspection does not require a checkout.
#[test]
fn normalized_registry_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let archive = archive(directory.path(), "library", &[], "[dependencies.helper]\nversion='=0.1.0-alpha.1'\n", None);
    assert_eq!(inspect_archive(&archive, &package("library")).unwrap(), "library-0.1.0-alpha.2");
}

/// All release notices and the normalized manifest are required independently of Cargo's success.
#[test]
fn missing_required_inputs() {
    let directory = tempfile::tempdir().unwrap();
    for missing in ["Cargo.toml", "README.md", "LICENSE"] {
        let archive = archive(directory.path(), "library", &[missing], "", None);
        assert!(inspect_archive(&archive, &package("library")).unwrap_err().to_string().contains("missing"));
    }
}

/// SDK and shim archives cannot pass release qualification with missing images, evidence or credits.
#[test]
fn implementation_packages_include_complete_readme_assets() {
    let directory = tempfile::tempdir().unwrap();
    for owner in ["neocogi-platform", "neocogi-platform-dylib"] {
        let complete = archive(directory.path(), owner, &[], "", None);
        inspect_archive(&complete, &package(owner)).unwrap();
        for missing in DOCUMENTATION_FILES {
            let incomplete = archive(directory.path(), owner, &[missing], "", None);
            let error = inspect_archive(&incomplete, &package(owner)).unwrap_err().to_string();
            assert!(error.contains("missing packaged documentation") && error.contains(missing), "{error}");
        }
    }
}

/// Paths, Git checkouts and workspace inheritance reject in all normal/build/dev target tables.
#[test]
fn nonregistry_dependencies_in_all_tables() {
    let directory = tempfile::tempdir().unwrap();
    for group in [
        "dependencies",
        "build-dependencies",
        "dev-dependencies",
        "target.'cfg(unix)'.dependencies",
        "target.'cfg(unix)'.build-dependencies",
        "target.'cfg(unix)'.dev-dependencies",
    ] {
        for source in ["path='../checkout'", "git='https://example.test/sdk'", "workspace=true"] {
            let archive = archive(directory.path(), "library", &[], &format!("[{group}.helper]\n{source}\n"), None);
            assert!(inspect_archive(&archive, &package("library")).unwrap_err().to_string().contains("nonregistry"));
        }
    }
}

/// Parent traversal, duplicate paths, symlinks, hard links and local review notes reject before unpacking.
#[test]
fn unsafe_members_and_review_notes() {
    let directory = tempfile::tempdir().unwrap();
    for (name, kind) in [
        ("../../escape", tar::EntryType::Regular),
        ("OPEN_ITEMS.md", tar::EntryType::Regular),
        ("ADVERSARIAL_REVIEW.md", tar::EntryType::Regular),
        ("link", tar::EntryType::Symlink),
        ("link", tar::EntryType::Link),
        ("LICENSE", tar::EntryType::Regular),
        ("./LICENSE", tar::EntryType::Regular),
    ] {
        let archive = archive(directory.path(), "library", &[], "", Some((name, kind)));
        assert!(inspect_archive(&archive, &package("library")).is_err(), "{name}");
        assert!(!directory.path().join("escape").exists());
    }
}

/// Only the contracts package may own the complete authored interface set.
#[test]
fn one_complete_schema_owner() {
    let directory = tempfile::tempdir().unwrap();
    let duplicate = archive(directory.path(), "library", &[], "", Some(("copy.ncidl", tar::EntryType::Regular)));
    assert!(
        inspect_archive(&duplicate, &package("library"))
            .unwrap_err()
            .to_string()
            .contains("source ownership")
    );
    let valid = archive(directory.path(), CONTRACTS, &[], "", None);
    inspect_archive(&valid, &package(CONTRACTS)).unwrap();
    let incomplete = archive(directory.path(), CONTRACTS, &["schemas/0.ncidl"], "", None);
    assert!(
        inspect_archive(&incomplete, &package(CONTRACTS))
            .unwrap_err()
            .to_string()
            .contains("source ownership")
    );
}

/// A well-formed archive for another package or version cannot certify the selected release.
#[test]
fn identity_mismatch() {
    let directory = tempfile::tempdir().unwrap();
    let archive = archive(directory.path(), "library", &[], "", None);
    assert!(inspect_archive(&archive, &package("other")).is_err());
    let mut wrong_version = package("library");
    wrong_version.version = "0.1.0-alpha.1".into();
    assert!(inspect_archive(&archive, &wrong_version).is_err());
    for manifest in [
        "",
        "[package]\nname='other'\nversion='0.1.0-alpha.2'\n",
        "[package]\nname='library'\nversion='wrong'\n",
    ] {
        let path = directory.path().join("identity.crate");
        let mut writer = tar::Builder::new(GzEncoder::new(fs::File::create(&path).unwrap(), Compression::fast()));
        for (name, content) in [("Cargo.toml", manifest), ("README.md", "docs"), ("LICENSE", "license")] {
            append(
                &mut writer,
                &format!("library-0.1.0-alpha.2/{name}"),
                tar::EntryType::Regular,
                content.as_bytes(),
            );
        }
        writer.into_inner().unwrap().finish().unwrap();
        assert!(inspect_archive(&path, &package("library")).is_err());
    }
}

/// Model the actual workspace, including the new private Rust tools member.
fn metadata() -> Metadata {
    let mut packages: Vec<_> = RELEASE_ORDER.iter().map(|name| package(name)).collect();
    for name in PRIVATE {
        let mut package = package(name);
        package.publish = Some(vec![]);
        packages.push(package);
    }
    Metadata {
        workspace_members: packages.iter().map(|package| package.id.clone()).collect(),
        packages,
        target_directory: PathBuf::new(),
    }
}

/// Tools stay private, mixed versions reject, and unrelated registry dependencies are ignored.
#[test]
fn private_publication_and_versions() {
    let mut metadata = metadata();
    assert_eq!(published_packages(&metadata).unwrap().len(), 3);
    for index in 3..metadata.packages.len() {
        metadata.packages[index].publish = Some(vec!["crates-io".into()]);
        assert!(published_packages(&metadata).unwrap_err().to_string().contains("private package"));
        metadata.packages[index].publish = Some(vec![]);
    }
    metadata.packages[0].version = "different".into();
    assert!(published_packages(&metadata).unwrap_err().to_string().contains("inconsistent versions"));
    metadata.packages[0].version = "0.1.0-alpha.2".into();
    metadata.packages.push(package("unrelated"));
    assert_eq!(published_packages(&metadata).unwrap().len(), 3);
}

/// No extra release member, missing shim or unrestricted registry policy is accepted.
#[test]
fn exact_release_set_and_registry() {
    let mut metadata = metadata();
    metadata.workspace_members.remove("neocogi-platform-dylib");
    assert!(published_packages(&metadata).unwrap_err().to_string().contains("exactly contracts"));
    metadata.workspace_members.insert("neocogi-platform-dylib".into());
    metadata.workspace_members.insert("extra".into());
    metadata.packages.push(package("extra"));
    assert!(published_packages(&metadata).unwrap_err().to_string().contains("exactly contracts"));
    metadata.packages.pop();
    metadata.packages[0].publish = None;
    assert!(published_packages(&metadata).unwrap_err().to_string().contains("explicitly select"));
}
