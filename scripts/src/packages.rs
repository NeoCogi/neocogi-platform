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
/// The normal native provider selected by the extracted consumers.
const SHIM: &str = "neocogi-platform-dylib";
/// Complete authored declarations, including platform PCM services.
const SCHEMA_COUNT: usize = 10;
/// The complete release set; adding a workspace tool must not add a published crate.
const RELEASE_ORDER: &[&str] = &[CONTRACTS, "neocogi-platform", SHIM];
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
    /// The final target configured for the developer, which selects the build volume.
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
    /// Manifest for this concrete package source, after Cargo applies configuration patches.
    manifest_path: PathBuf,
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

/// Inspect an owned archive snapshot for unique regular files and directories beneath one root.
fn inspect_archive(bytes: &[u8], package: &Package) -> Result<String> {
    let prefix = format!("{}-{}", package.name, package.version);
    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
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

/// Inspect and unpack the same compressed bytes into the run's fresh package directory.
fn inspect_and_unpack_archive(bytes: &[u8], package: &Package, destination: &Path) -> Result<String> {
    let prefix = inspect_archive(bytes, package)?;
    tar::Archive::new(GzDecoder::new(bytes)).unpack(destination)?;
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

/// A normal platform cdylib receipt tying a final library to Cargo's selected package source.
#[derive(Debug)]
pub(crate) struct PlatformArtifact {
    /// Cargo package identity from the producing toolchain's build.
    package_id: String,
    /// Manifest of the package that produced the library.
    manifest_path: PathBuf,
    /// Final library path reported by Cargo for this profile and target.
    pub(crate) path: PathBuf,
}

/// Read Cargo's artifact receipt for the one normal platform cdylib, without assuming a profile path.
/// Library-example targets and intermediate metadata cannot satisfy the packaged-library check.
pub(crate) fn platform_artifact(output: &[u8]) -> Result<PlatformArtifact> {
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
                let receipt = PlatformArtifact {
                    package_id: message["package_id"].as_str().context("missing platform artifact package ID")?.into(),
                    manifest_path: message["manifest_path"].as_str().context("missing platform artifact manifest path")?.into(),
                    path: filename,
                };
                ensure!(artifact.replace(receipt).is_none(), "multiple normal platform libraries in Cargo output");
            }
        }
    }
    artifact.context("Cargo did not produce the normal platform cdylib")
}

/// Own this run's artifacts on Cargo's configured build volume, separate from the extracted sources.
fn private_cargo_artifacts(configured_target: &Path) -> Result<tempfile::TempDir> {
    fs::create_dir_all(configured_target).with_context(|| format!("prepare configured Cargo target {}", configured_target.display()))?;
    tempfile::Builder::new()
        .prefix("neocogi-qualification-")
        .tempdir_in(configured_target)
        .with_context(|| format!("create private qualification artifacts in {}", configured_target.display()))
}

/// Pre-create one phase's final and intermediate Cargo directories within the owned run.
fn create_cargo_target(artifacts: &Path, name: &str) -> Result<PathBuf> {
    let target = artifacts.join(name);
    fs::create_dir(&target).with_context(|| format!("create Cargo target {}", target.display()))?;
    let build = target.join("build");
    fs::create_dir(&build).with_context(|| format!("create Cargo build directory {}", build.display()))?;
    Ok(target)
}

/// Select explicit run-owned Cargo output paths while inheriting other Cargo configuration.
fn qualification_cargo(toolchain: Option<&str>, workspace: &Path, target: &Path) -> std::process::Command {
    let mut command = crate::process::cargo(toolchain);
    command
        .current_dir(workspace)
        .env("CARGO_TARGET_DIR", target)
        .env("CARGO_BUILD_BUILD_DIR", target.join("build"));
    command
}

/// Build and test the extracted workspace with a canonical temporary parent and no audio device.
fn extracted_cargo(toolchain: &str, workspace: &Path, target: &Path) -> std::process::Command {
    let mut command = qualification_cargo(Some(toolchain), workspace, target);
    command.env("SDL_AUDIODRIVER", "dummy");
    // Resource staging also checks ancestors of temporary inputs created by the tests.
    let temporary_parent = workspace.parent().expect("the extracted workspace has a temporary parent");
    for key in TEMP_DIRECTORY_KEYS {
        command.env(key, temporary_parent);
    }
    command
}

/// Verify the effective source of every internal package before any extracted build.
/// Metadata without `--no-deps` includes the dependencies selected by Cargo's resolver.
fn extracted_shim_id(metadata: &Metadata, expected: &BTreeMap<&str, PathBuf>) -> Result<String> {
    let mut checked = BTreeMap::new();
    for package in &metadata.packages {
        let name = package.name.as_str();
        let Some(manifest) = expected.get(name) else { continue };
        let resolved = package
            .manifest_path
            .canonicalize()
            .with_context(|| format!("resolve Cargo manifest for {name}"))?;
        ensure!(
            resolved == *manifest,
            "qualification resolved {name} from {}, expected {}",
            resolved.display(),
            manifest.display()
        );
        ensure!(
            checked.insert(name, &package.id).is_none(),
            "qualification resolved multiple packages named {name}"
        );
    }
    for name in expected.keys() {
        ensure!(checked.contains_key(name), "qualification did not resolve packaged {name}");
    }
    let shim = checked[SHIM];
    ensure!(!metadata.workspace_members.contains(shim), "qualification shim must be a nonmember dependency");
    Ok(shim.clone())
}

/// Verify both ends of the receipt against this resolution and this run's output directory.
fn qualified_library(output: &[u8], target: &Path, shim_id: &str, shim_manifest: &Path) -> Result<PathBuf> {
    let receipt = platform_artifact(output)?;
    ensure!(receipt.package_id == shim_id, "unexpected platform artifact package ID: {}", receipt.package_id);
    let manifest = receipt.manifest_path.canonicalize()?;
    ensure!(manifest == shim_manifest, "unexpected platform artifact manifest: {}", manifest.display());
    let artifact = receipt.path.canonicalize()?;
    ensure!(
        artifact.starts_with(target.canonicalize()?),
        "platform library is outside the qualification target"
    );
    Ok(artifact)
}

/// Name the environment setting Rust uses when it selects the OS temporary directory.
#[cfg(windows)]
const TEMP_DIRECTORY_SETTING: &str = "TMP or TEMP";
/// Name the environment setting Rust uses when it selects the OS temporary directory.
#[cfg(not(windows))]
const TEMP_DIRECTORY_SETTING: &str = "TMPDIR";
/// Temporary directory environment keys of the extracted child processes.
#[cfg(windows)]
const TEMP_DIRECTORY_KEYS: &[&str] = &["TMP", "TEMP"];
/// Temporary directory environment keys of the extracted child processes.
#[cfg(not(windows))]
const TEMP_DIRECTORY_KEYS: &[&str] = &["TMPDIR"];

/// Check a resolved location by path component so a sibling may share the checkout's name prefix.
fn verify_external_path(checkout: &Path, requested: &Path, resolved: &Path) -> Result<()> {
    ensure!(
        !resolved.starts_with(checkout),
        "package qualification temporary location {} resolves inside checkout {} (resolved location {}). Set {TEMP_DIRECTORY_SETTING} to a directory outside the checkout",
        requested.display(),
        checkout.display(),
        resolved.display(),
    );
    Ok(())
}

/// Own a fresh workspace under the canonical OS temporary parent and verify its created location.
fn external_workspace(checkout: &Path, parent: &Path) -> Result<(tempfile::TempDir, PathBuf)> {
    let checkout = checkout.canonicalize().with_context(|| format!("resolve checkout {}", checkout.display()))?;
    let resolved_parent = parent.canonicalize().with_context(|| {
        format!(
            "resolve package qualification temporary parent {}. Set {TEMP_DIRECTORY_SETTING} to an existing directory outside the checkout",
            parent.display(),
        )
    })?;
    verify_external_path(&checkout, parent, &resolved_parent)?;
    let outside = tempfile::Builder::new()
        .prefix("neocogi-package-check-")
        .tempdir_in(&resolved_parent)
        .with_context(|| {
            format!(
                "create package qualification workspace under {}. Set {TEMP_DIRECTORY_SETTING} to a writable directory outside the checkout",
                resolved_parent.display(),
            )
        })?;
    let workspace = outside
        .path()
        .canonicalize()
        .with_context(|| format!("resolve created package qualification workspace {}", outside.path().display()))?;
    verify_external_path(&checkout, parent, &workspace)?;
    Ok((outside, workspace))
}

/// Construct, inspect and exercise actual Cargo archives from an isolated temporary workspace.
pub(crate) fn qualify(allow_dirty: bool, offline: bool, stable: Option<&str>) -> Result<()> {
    use crate::process::{capture, cargo, run};
    let (_outside, workspace) = external_workspace(crate::root(), &std::env::temp_dir())?;
    let flags = if offline { vec!["--offline"] } else { vec![] };
    let metadata: Metadata = serde_json::from_slice(&capture(cargo(None).args(["metadata", "--locked", "--format-version=1"]).args(&flags))?)?;
    let selected = published_packages(&metadata)?;
    let artifacts = private_cargo_artifacts(&metadata.target_directory)?;
    let package_target = create_cargo_target(artifacts.path(), "package-target")?;
    let target = create_cargo_target(artifacts.path(), "consumer-target")?;
    let toolchain: toml::Value = toml::from_str(&fs::read_to_string(crate::root().join("rust-toolchain.toml"))?)?;
    let toolchain = toolchain["toolchain"]["channel"].as_str().context("missing workspace toolchain channel")?;
    // Package together so Cargo resolves the unpublished release chain in this run's package index.
    let mut package = qualification_cargo(None, crate::root(), &package_target);
    package.args(["package", "--locked", "--no-verify"]).args(&flags);
    if allow_dirty {
        package.arg("--allow-dirty");
    }
    for name in RELEASE_ORDER {
        package.args(["-p", name]);
    }
    run(&mut package)?;
    let packages = workspace.join("packages");
    fs::create_dir(&packages)?;
    let mut members = vec!["consumer".to_owned(), "local-consumer".to_owned()];
    let mut excluded = Vec::new();
    let mut manifest = toml::toml! { workspace = { resolver = "3" } };
    let mut patches = toml::map::Map::new();
    let mut expected = BTreeMap::new();
    for (name, package) in &selected {
        let archive = package_target.join("package").join(format!("{name}-{}.crate", package.version));
        let bytes = fs::read(&archive).with_context(|| format!("read Cargo archive {}", archive.display()))?;
        let prefix = inspect_and_unpack_archive(&bytes, package, &packages).with_context(|| format!("qualify Cargo archive {}", archive.display()))?;
        expected.insert(*name, packages.join(&prefix).join("Cargo.toml").canonicalize()?);
        let path = format!("packages/{prefix}");
        if *name == SHIM {
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
    fs::write(workspace.join("Cargo.toml"), toml::to_string(&manifest)?)?;
    copy_tree(&crate::root().join("scripts/package-consumer"), &workspace.join("consumer"))?;
    copy_tree(&crate::root().join("scripts/package-local-consumer"), &workspace.join("local-consumer"))?;
    fs::copy(crate::root().join("Cargo.lock"), workspace.join("Cargo.lock"))?;
    let resolve = |toolchain: &str, lock: &[&str]| -> Result<String> {
        let metadata: Metadata = serde_json::from_slice(&capture(
            extracted_cargo(toolchain, &workspace, &target)
                .args(["metadata", "--format-version=1"])
                .args(&flags)
                .args(lock),
        )?)?;
        extracted_shim_id(&metadata, &expected).with_context(|| format!("verify extracted sources under {toolchain}"))
    };
    // The copied development lock must first be resolved in the new workspace.
    // Lock every later build to this graph and validate it on both toolchains.
    let shim = resolve(toolchain, &[])?;
    let stable_shim = stable.map(|stable| resolve(stable, &["--locked"])).transpose()?;
    let execute = |toolchain: &str, arguments: &[&str]| -> Result<()> {
        // Cargo options must precede the executable's argument separator.
        let (cargo_arguments, executable_arguments) = arguments.split_at(arguments.iter().position(|argument| *argument == "--").unwrap_or(arguments.len()));
        run(extracted_cargo(toolchain, &workspace, &target)
            .args(cargo_arguments)
            .args(&flags)
            .arg("--locked")
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
            "test-support,glow",
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
            "test-support,shader-python,glow",
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
    let library = |toolchain: &str, shim_id: &str| -> Result<PathBuf> {
        qualified_library(
            &capture(
                extracted_cargo(toolchain, &workspace, &target)
                    .args(["build", "--locked", "--lib", "-p", SHIM, "--message-format=json"])
                    .args(&flags),
            )?,
            &target,
            shim_id,
            &expected[SHIM],
        )
    };
    let platform = library(toolchain, &shim)?;
    let consumer = [
        "run",
        "-p",
        "neocogi-package-consumer",
        "--",
        platform.to_str().context("platform path is not UTF-8")?,
    ];
    execute(toolchain, &consumer)?;
    let local_consumer = |toolchain, platform: &Path| {
        execute(
            toolchain,
            &[
                "run",
                "-p",
                "neocogi-package-local-consumer",
                "--",
                platform.to_str().context("platform path is not UTF-8")?,
            ],
        )
    };
    local_consumer(toolchain, &platform)?;
    fs::remove_file(workspace.join("consumer/staged/asset.txt"))?;
    execute(toolchain, &consumer)?;
    fs::write(workspace.join("consumer/asset.txt"), b"changed packaged staging input\n")?;
    execute(toolchain, &consumer)?;
    if let Some((stable, shim_id)) = stable.zip(stable_shim) {
        // Qualify the actual stable-built plugin as well as the stable-built caller.
        let platform = library(stable, &shim_id)?;
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
        local_consumer(stable, &platform)?;
    }
    ensure!(!workspace.join("target").exists(), "package qualification created a second target directory");
    println!(
        "PASS: {} archives and both external consumers; internal sources came only from extracted archives",
        selected.len()
    );
    Ok(())
}

#[cfg(test)]
#[path = "packages_tests.rs"]
mod tests;
