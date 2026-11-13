//! Release-set policy, archive inspection and external consumer qualification, without publication.
use anyhow::{Context, Result, ensure};
use flate2::read::GzDecoder;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

/// Canonical source owner must be published before its implementation consumers.
const CONTRACTS: &str = "neocogi-platform-contracts";
/// Complete authored declarations, including platform PCM services.
const SCHEMA_COUNT: usize = 10;
/// The complete release set; adding a workspace tool must not add a published crate.
const RELEASE_ORDER: &[&str] = &[CONTRACTS, "neocogi-platform", "neocogi-platform-dylib"];
/// Files needed by each implementation package's self-contained README.
/// The SDK owns the canonical screenshots/report; the final shim packages matching copies.
const DOCUMENTATION_FILES: &[&str] = &[
    "docs/images/damaged-helmet.png",
    "docs/images/brainstem.png",
    "docs/benchmarks/shader-examples-2026-11-12.json",
    "docs/licenses/DamagedHelmet-LICENSE.md",
    "docs/licenses/BrainStem-LICENSE.md",
    "docs/licenses/LicenseRef-Poser-EULA.txt",
    "docs/licenses/Cannon_Exterior-LICENSE.md",
    "docs/licenses/Cannon_Exterior-CC0.txt",
];
/// Known workspace applications, fixtures and development tools are always unpublished.
const PRIVATE: &[&str] = &[
    "neocogi-typed-buffer-tests",
    "neocogi-platform-examples",
    "neocogi-example-plugins",
    "neocogi-browser-tests",
    "neocogi-platform-tools",
];

/// Relevant Cargo metadata, retaining member IDs to exclude third-party packages.
#[derive(Deserialize)]
struct Metadata {
    /// Package IDs belonging to this workspace, rather than its dependencies.
    workspace_members: HashSet<String>,
    /// Cargo's resolved packages and declared publication policy.
    packages: Vec<Package>,
    /// One shared target directory used sequentially throughout qualification.
    target_directory: PathBuf,
}

/// Registry identity and publication policy used to compare actual archive manifests.
#[derive(Debug, Deserialize)]
struct Package {
    /// Cargo's source-aware package identity.
    id: String,
    /// Registry package name.
    name: String,
    /// Exact release version; all release members must agree.
    version: String,
    /// None permits all registries; release members must explicitly select crates-io.
    publish: Option<Vec<String>>,
}

/// Validate policy before creating archives; unknown private members cannot silently become releases.
fn published_packages(metadata: &Metadata) -> Result<BTreeMap<&str, &Package>> {
    let mut selected = BTreeMap::new();
    for package in &metadata.packages {
        if !metadata.workspace_members.contains(&package.id) {
            continue;
        }
        if PRIVATE.contains(&package.name.as_str()) {
            ensure!(
                package.publish.as_ref().is_some_and(Vec::is_empty),
                "private package enabled for publication: {}",
                package.name
            );
        } else {
            ensure!(
                package.publish.as_deref() == Some(&["crates-io".to_owned()]),
                "release package must explicitly select crates.io: {}",
                package.name
            );
            selected.insert(package.name.as_str(), package);
        }
    }
    ensure!(selected.contains_key(CONTRACTS), "shared source owner is missing from release set");
    ensure!(
        selected.values().map(|package| &package.version).collect::<HashSet<_>>().len() == 1,
        "release members have inconsistent versions"
    );
    ensure!(
        selected.keys().copied().collect::<HashSet<_>>() == RELEASE_ORDER.iter().copied().collect(),
        "release set must contain exactly contracts, SDK and cdylib shim"
    );
    Ok(selected)
}

/// Validate normalized dependency tables, including target-specific normal/build/dev dependencies.
fn registry_dependencies(manifest: &toml::Value) -> Result<()> {
    let mut tables = vec![manifest];
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        tables.extend(targets.values());
    }
    for table in tables {
        for group in ["dependencies", "build-dependencies", "dev-dependencies"] {
            if let Some(dependencies) = table.get(group).and_then(toml::Value::as_table) {
                for (name, dependency) in dependencies {
                    if let Some(fields) = dependency.as_table() {
                        ensure!(
                            !["path", "git", "workspace"].iter().any(|key| fields.contains_key(*key)),
                            "nonregistry dependency: {name}"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Inspect every member before extraction: only unique regular files/directories beneath one root.
fn inspect_archive(archive: &Path, package: &Package) -> Result<String> {
    let prefix = format!("{}-{}", package.name, package.version);
    let mut archive = tar::Archive::new(GzDecoder::new(fs::File::open(archive)?));
    let mut names = HashSet::new();
    let mut files = HashSet::new();
    let mut manifest = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path: PathBuf = entry.path()?.components().collect();
        let name = path.to_str().context("archive member path is not UTF-8")?;
        ensure!(
            path.components().next() == Some(Component::Normal(prefix.as_ref()))
                && path.components().all(|component| matches!(component, Component::Normal(_))),
            "invalid archive path: {name}"
        );
        ensure!(
            entry.header().entry_type().is_file() || entry.header().entry_type().is_dir(),
            "archive contains nonregular member: {name}"
        );
        ensure!(
            !name.contains("OPEN_ITEMS") && !name.contains("ADVERSARIAL"),
            "local review notes leaked into archive"
        );
        ensure!(names.insert(name.to_owned()), "duplicate archive member: {name}");
        if entry.header().entry_type().is_file() {
            files.insert(name.to_owned());
        }
        if name == format!("{prefix}/Cargo.toml") {
            ensure!(entry.header().entry_type().is_file(), "archive manifest must be a regular file");
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            manifest = Some(toml::from_str::<toml::Value>(&text)?);
        }
    }
    for required in ["Cargo.toml", "README.md", "LICENSE"] {
        ensure!(files.contains(&format!("{prefix}/{required}")), "missing regular {required} in {prefix}");
    }
    if matches!(package.name.as_str(), "neocogi-platform" | "neocogi-platform-dylib") {
        for required in DOCUMENTATION_FILES {
            ensure!(
                files.contains(&format!("{prefix}/{required}")),
                "missing packaged documentation {required} in {prefix}"
            );
        }
    }
    let manifest = manifest.context("missing Cargo.toml")?;
    let identity = manifest.get("package").context("archive manifest is missing its package identity")?;
    ensure!(
        identity.get("name").and_then(toml::Value::as_str) == Some(&package.name)
            && identity.get("version").and_then(toml::Value::as_str) == Some(&package.version),
        "archive package identity differs from selected metadata"
    );
    registry_dependencies(&manifest)?;
    let schemas = files.iter().filter(|name| name.ends_with(".ncidl")).count();
    ensure!(
        schemas == if package.name == CONTRACTS { SCHEMA_COUNT } else { 0 },
        "incorrect shared source ownership in {prefix}"
    );
    Ok(prefix)
}

/// Copy the authored consumer fixture without retaining a checkout dependency or target cache.
fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            ensure!(kind.is_file(), "consumer fixture contains a nonregular file");
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Read Cargo's artifact receipt for the one normal platform cdylib, without assuming a profile path.
/// Library-example targets and intermediate metadata cannot satisfy the packaged-library check.
pub(crate) fn platform_artifact(output: &[u8]) -> Result<PathBuf> {
    let mut artifact = None;
    for line in output.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
        let message: serde_json::Value = serde_json::from_slice(line).context("invalid Cargo JSON message")?;
        if message["reason"] != "compiler-artifact" || message["target"]["name"] != "nc_platform" || message["target"]["kind"] != serde_json::json!(["cdylib"])
        {
            continue;
        }
        for filename in message["filenames"].as_array().context("missing Cargo artifact filenames")? {
            let filename = PathBuf::from(filename.as_str().context("artifact filename is not a string")?);
            if filename.extension().and_then(|value| value.to_str()) == Some(std::env::consts::DLL_EXTENSION) {
                ensure!(artifact.replace(filename).is_none(), "multiple normal platform libraries in Cargo output");
            }
        }
    }
    artifact.context("Cargo did not produce the normal platform cdylib")
}

/// Construct, inspect and exercise actual Cargo archives from an isolated temporary workspace.
pub(crate) fn qualify(allow_dirty: bool, offline: bool, stable: Option<&str>) -> Result<()> {
    use crate::process::{capture, cargo, run};
    let flags = if offline { vec!["--offline"] } else { vec![] };
    let metadata: Metadata = serde_json::from_slice(&capture(cargo(None).args(["metadata", "--locked", "--format-version=1"]).args(&flags))?)?;
    let selected = published_packages(&metadata)?;
    let toolchain: toml::Value = toml::from_str(&fs::read_to_string(crate::root().join("rust-toolchain.toml"))?)?;
    let toolchain = toolchain["toolchain"]["channel"].as_str().context("missing workspace toolchain channel")?;
    let mut package = cargo(None);
    package.args(["package", "--locked", "--no-verify"]).args(&flags);
    if allow_dirty {
        package.arg("--allow-dirty");
    }
    for name in RELEASE_ORDER {
        package.args(["-p", name]);
    }
    run(&mut package)?;
    let outside = tempfile::Builder::new().prefix("neocogi-package-check-").tempdir()?;
    let packages = outside.path().join("packages");
    fs::create_dir(&packages)?;
    let mut members = vec!["consumer".to_owned()];
    let mut excluded = Vec::new();
    let mut manifest = toml::toml! { workspace = { resolver = "3" } };
    let mut patches = toml::map::Map::new();
    for (name, package) in &selected {
        let archive = metadata.target_directory.join("package").join(format!("{name}-{}.crate", package.version));
        let prefix = inspect_archive(&archive, package)?;
        // The destination is newly created, and inspection rejects symlinks, links,
        // duplicate members and parent traversal before any archive bytes are unpacked.
        tar::Archive::new(GzDecoder::new(fs::File::open(&archive)?)).unpack(&packages)?;
        let path = format!("packages/{prefix}");
        if *name == "neocogi-platform-dylib" {
            // The shim must also work as a consumer's registry dependency, rather
            // than relying on being a member of this repository's workspace.
            excluded.push(toml::Value::String(path.clone()));
        } else {
            members.push(path.clone());
        }
        let mut dependency = toml::map::Map::new();
        dependency.insert("path".into(), toml::Value::String(path));
        patches.insert((*name).to_owned(), toml::Value::Table(dependency));
    }
    manifest
        .get_mut("workspace")
        .and_then(toml::Value::as_table_mut)
        .expect("authored workspace table")
        .insert("members".into(), toml::Value::Array(members.into_iter().map(toml::Value::String).collect()));
    manifest["workspace"]
        .as_table_mut()
        .expect("authored workspace table")
        .insert("exclude".into(), toml::Value::Array(excluded));
    manifest.insert(
        "patch".into(),
        toml::Value::Table(toml::map::Map::from_iter([("crates-io".into(), toml::Value::Table(patches))])),
    );
    fs::write(outside.path().join("Cargo.toml"), toml::to_string(&manifest)?)?;
    copy_tree(&crate::root().join("scripts/package-consumer"), &outside.path().join("consumer"))?;
    fs::copy(crate::root().join("Cargo.lock"), outside.path().join("Cargo.lock"))?;
    // Cargo normalizes archive mtimes. Stale builds of the same version must never
    // qualify changed archive bytes; retain only third-party dependency artifacts.
    let mut clean = cargo(Some(toolchain));
    clean.arg("clean").args(&flags);
    for name in RELEASE_ORDER {
        clean.args(["-p", name]);
    }
    run(&mut clean)?;
    let execute = |toolchain: &str, arguments: &[&str]| -> Result<()> {
        // Cargo options must precede the executable's argument separator; otherwise
        // --target-dir reaches the consumer and silently creates a second build tree.
        let (cargo_arguments, executable_arguments) = arguments.split_at(arguments.iter().position(|argument| *argument == "--").unwrap_or(arguments.len()));
        run(cargo(Some(toolchain))
            .current_dir(outside.path())
            .args(cargo_arguments)
            // Archive qualification must not depend on a desktop audio daemon or physical microphone.
            .env("SDL_AUDIODRIVER", "dummy")
            .args(&flags)
            .arg("--target-dir")
            .arg(&metadata.target_directory)
            .args(executable_arguments))
    };
    execute(toolchain, &["build", "--workspace", "--all-targets"])?;
    execute(toolchain, &["test", "-p", CONTRACTS, "--features", "build-support,test-support"])?;
    execute(
        toolchain,
        &[
            "test",
            "--lib",
            "--test",
            "compiler",
            "--test",
            "shader_targets",
            "--test",
            "sexp_compiler",
            "--test",
            "shader_optimization",
            "-p",
            "neocogi-platform",
            "--features",
            "test-support,platform,glow,shader-spirv",
        ],
    )?;
    // Default consumers must also qualify without the optional parser. Opt in
    // separately so frontend tests do not hide accidental default activation.
    execute(
        toolchain,
        &[
            "test",
            "-p",
            "neocogi-platform",
            "--features",
            "test-support,shader-python,shader-glsl,shader-spirv",
            "--test",
            "python_compiler",
            "--test",
            "shader_targets",
            "--test",
            "compiler_plugin",
        ],
    )?;
    // Build the shim as an ordinary library from extracted registry-style inputs.
    // Read the final path from Cargo rather than accepting a library-example artifact.
    let library = |toolchain: &str| -> Result<PathBuf> {
        platform_artifact(&capture(
            cargo(Some(toolchain))
                .current_dir(outside.path())
                .args(["build", "--lib", "-p", "neocogi-platform-dylib", "--message-format=json"])
                .args(&flags)
                .arg("--target-dir")
                .arg(&metadata.target_directory),
        )?)
    };
    let platform = library(toolchain)?;
    let consumer = [
        "run",
        "-p",
        "neocogi-package-consumer",
        "--",
        platform.to_str().context("platform path is not UTF-8")?,
    ];
    execute(toolchain, &consumer)?;
    fs::remove_file(outside.path().join("consumer/staged/asset.txt"))?;
    execute(toolchain, &consumer)?;
    fs::write(outside.path().join("consumer/asset.txt"), b"changed packaged staging input\n")?;
    execute(toolchain, &consumer)?;
    if let Some(stable) = stable {
        // Qualify the actual stable-built plugin as well as the stable-built caller.
        let platform = library(stable)?;
        execute(
            stable,
            &[
                "run",
                "-p",
                "neocogi-package-consumer",
                "--",
                platform.to_str().context("stable platform path is not UTF-8")?,
            ],
        )?;
    }
    ensure!(
        !outside.path().join("target").exists(),
        "package qualification created a second target directory"
    );
    println!(
        "PASS: {} archives and external consumer; internal sources came only from extracted archives",
        selected.len()
    );
    Ok(())
}

#[cfg(test)]
#[path = "packages_tests.rs"]
mod tests;
