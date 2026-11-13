//! Adversarial archive and release metadata fixtures; no Cargo or network is required.
use super::*;
use flate2::{Compression, write::GzEncoder};

/// Cargo's receipt must name the normal native cdylib; examples and metadata cannot mask a missing library.
#[test]
fn normal_platform_artifact_is_required() {
    let path = format!("/output/{}nc_platform.{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_EXTENSION);
    let receipt = serde_json::json!({
        "reason": "compiler-artifact",
        "target": { "name": "nc_platform", "kind": ["cdylib"] },
        "filenames": ["/output/libnc_platform.rmeta", path]
    });
    let output = format!("{{\"reason\":\"build-finished\",\"success\":true}}\n{receipt}\n");
    assert_eq!(platform_artifact(output.as_bytes()).unwrap(), PathBuf::from(&path));
    assert!(platform_artifact(format!("{receipt}\n{receipt}\n").as_bytes()).is_err());
    let mut example = receipt.clone();
    example["target"]["kind"] = serde_json::json!(["example"]);
    assert!(platform_artifact(example.to_string().as_bytes()).is_err());
    assert!(platform_artifact(b"").is_err());
    assert!(platform_artifact(b"not JSON").is_err());
}

/// Fixture registry identity, independently checked against the archived manifest.
fn package(name: &str) -> Package {
    Package {
        id: name.into(),
        name: name.into(),
        version: "0.1.0-alpha.2".into(),
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
