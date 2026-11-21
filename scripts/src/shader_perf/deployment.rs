//! Build the profiler input in private Cargo roots and publish one verified native deployment.
use super::{APPLICATIONS, Backend, Options};
use crate::{
    artwork::{DOWNLOADS, LICENSES, STAGED_DIR},
    assets::{digest, gltf_download_directory, require_gltf_downloads},
    cargo_artifacts::{ArtifactReceipt, CargoDirectories, owned_artifact, receipts},
    workspace::{Member, Workspace},
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// The normal library provider is a separate package from the measured applications.
const SHIM: &str = "neocogi-platform-dylib";
/// The application package declares more bins than the three qualification workloads.
const EXAMPLES: &str = "neocogi-platform-examples";
/// All producing native artifacts use this repository profile.
pub(super) const PROFILE: &str = "min-size";

/// One input's verified bytes and relative path in the published candidate tree.
#[derive(Debug, Serialize)]
pub(super) struct FileCheck {
    /// Original authored or Cargo-reported path at the time of staging.
    source: PathBuf,
    /// Destination relative to the candidate root.
    staged: PathBuf,
    /// SHA-256 of the source file immediately before candidate publication.
    source_sha256: String,
    /// Independently calculated SHA-256 of the staged file.
    staged_sha256: String,
    /// Length of the verified staged file.
    pub(super) bytes: u64,
}

/// Checked staging provenance for one requested Cargo target.
#[derive(Debug, Serialize)]
pub(super) struct BuiltArtifact {
    /// Exact package ID from Cargo's compiler-artifact message.
    package_id: String,
    /// Manifest of the producing workspace package.
    manifest_path: PathBuf,
    /// Cargo target name and kind, documented independently of file names.
    target: String,
    /// Cargo target roles for the selected receipt.
    kind: Vec<String>,
    /// Full feature set in the actual compiler-artifact message.
    features: BTreeSet<String>,
    /// Source and staged file evidence.
    pub(super) file: FileCheck,
}

/// Provenance of all authored and produced inputs admitted to the candidate.
#[derive(Serialize)]
pub(super) struct Deployment {
    /// Run-owned normal provider, verified against the library staged by the application build.
    pub(super) provider: BuiltArtifact,
    /// One checked executable per measured workload, keyed by target name.
    applications: BTreeMap<&'static str, BuiltArtifact>,
    /// Required model, HDR and license files plus any authored cache staged before warmup.
    assets: Vec<FileCheck>,
}

/// Completed publication with actual toolchain evidence from the producing Cargo context.
pub(super) struct Prepared {
    /// Stable report-local directory containing the candidate to time.
    pub(super) candidate: PathBuf,
    /// Verbose version information from Cargo's selected Rust compiler.
    pub(super) rustc: String,
    /// Evidence of the specific inputs moved into the candidate.
    pub(super) deployment: Deployment,
}

/// One admitted receipt and the exact native file selected from it.
#[derive(Debug)]
struct ProducedArtifact {
    /// Cargo's package, target and feature evidence.
    receipt: ArtifactReceipt,
    /// The verified native file below the final Cargo output directory.
    path: PathBuf,
}

impl ProducedArtifact {
    /// Match the staged bytes with this run's receipt and bind them to its provenance.
    fn verify(self, draft: &Path, destination: &Path) -> Result<BuiltArtifact> {
        let file = verify_staged(&self.path, draft, destination)?;
        ensure!(file.bytes > 0, "empty native artifact {}", self.path.display());
        Ok(BuiltArtifact {
            package_id: self.receipt.package_id,
            manifest_path: self.receipt.manifest_path,
            target: self.receipt.target.name,
            kind: self.receipt.target.kind,
            features: self.receipt.features,
            file,
        })
    }
}

/// Distinguish native final outputs from similarly named targets and intermediates.
#[derive(Clone, Copy)]
enum Kind {
    /// The ordinary platform library, exposed as a final cdylib.
    Provider,
    /// A named workload's normal executable target.
    Application,
}

impl Kind {
    /// Select the exact target kind declared by the native workspace.
    fn cargo_name(self) -> &'static str {
        match self {
            Self::Provider => "cdylib",
            Self::Application => "bin",
        }
    }

    /// Require a single normal final file, with path layout independent of Cargo profile placement.
    fn output(self, receipt: &ArtifactReceipt) -> Result<PathBuf> {
        let name = &receipt.target.name;
        let expected = match self {
            Self::Provider => format!("{}{name}{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX),
            Self::Application => format!("{name}{}", std::env::consts::EXE_SUFFIX),
        };
        let mut files = receipt
            .filenames
            .iter()
            .filter(|file| file.file_name().is_some_and(|name| name == expected.as_str()));
        let file = files.next().with_context(|| format!("Cargo did not report the final file {expected}"))?;
        ensure!(files.next().is_none(), "Cargo reported multiple final files named {expected}");
        if matches!(self, Self::Application) {
            ensure!(
                receipt.executable.as_ref() == Some(file),
                "Cargo did not report {expected} as a final executable"
            );
        }
        Ok(file.clone())
    }
}

/// Select exactly one valid receipt per requested target, ignoring unrelated Cargo outputs.
fn select<'a>(
    bytes: &[u8],
    member: &Member,
    requested_features: &str,
    kind: Kind,
    names: &[&'a str],
    target: &Path,
) -> Result<BTreeMap<&'a str, ProducedArtifact>> {
    let manifest = member.manifest_path.canonicalize()?;
    let features: BTreeSet<_> = requested_features.split(',').collect();
    let mut selected = BTreeMap::new();
    for receipt in receipts(bytes)? {
        let Some(&name) = names.iter().find(|&&name| name == receipt.target.name) else {
            continue;
        };
        if receipt.target.kind != [kind.cargo_name()] {
            continue;
        }
        receipt.require_source(&member.id, &manifest)?;
        let actual: BTreeSet<_> = receipt.features.iter().map(String::as_str).collect();
        ensure!(actual == features, "unexpected {name} artifact features: {actual:?}, expected {features:?}");
        let path = owned_artifact(&kind.output(&receipt)?, target)?;
        ensure!(
            selected.insert(name, ProducedArtifact { receipt, path }).is_none(),
            "multiple matching Cargo receipts for {name}"
        );
    }
    for name in names {
        ensure!(
            selected.contains_key(name),
            "Cargo did not report a matching {} receipt for {name}",
            kind.cargo_name()
        );
    }
    Ok(selected)
}

/// Check the staged bytes against the exact source before exposing the deployment.
fn verify_staged(source: &Path, draft: &Path, relative: &Path) -> Result<FileCheck> {
    let staged = draft.join(relative);
    let staged_metadata = fs::symlink_metadata(&staged).with_context(|| format!("inspect staged file {}", staged.display()))?;
    ensure!(staged_metadata.is_file(), "staged file is not a regular file: {}", staged.display());
    let source_sha256 = digest(fs::File::open(source).with_context(|| format!("open source file {}", source.display()))?)?;
    let staged_sha256 = digest(fs::File::open(&staged)?)?;
    ensure!(
        source_sha256 == staged_sha256,
        "staged bytes differ from {} at {}",
        source.display(),
        staged.display()
    );
    Ok(FileCheck {
        source: source.to_path_buf(),
        staged: relative.to_path_buf(),
        source_sha256,
        staged_sha256,
        bytes: staged_metadata.len(),
    })
}

/// Preserve both streams through build failures and retain the raw output used for receipt admission.
fn cargo_log(command: &mut Command, logs: &Path, phase: &str) -> Result<PathBuf> {
    let stdout = logs.join(format!("{phase}.stdout"));
    let stderr = logs.join(format!("{phase}.stderr"));
    eprintln!("+ {command:?}");
    let status = command
        .stdout(Stdio::from(fs::File::create(&stdout)?))
        .stderr(Stdio::from(fs::File::create(&stderr)?))
        .status()
        .with_context(|| format!("start {command:?}"))?;
    ensure!(status.success(), "{phase} failed ({status}); see {} and {}", stdout.display(), stderr.display());
    Ok(stdout)
}

/// Record the compiler through Cargo so the caller's rustup, rustc and wrapper selections apply.
fn compiler_version(command: &mut Command, logs: &Path) -> Result<String> {
    // Cargo's information-only --print path forwards extra arguments to the selected compiler.
    // rustc handles -vV before the requested print format, so this never compiles a target.
    let output = cargo_log(command.args(["-Z", "unstable-options", "--print", "target-libdir", "--", "-vV"]), logs, "rustc")?;
    let version = fs::read_to_string(output)?;
    ensure!(
        version.starts_with("rustc ") && version.lines().any(|line| line.starts_with("host: ")),
        "Cargo did not report a Rust compiler version"
    );
    Ok(version.trim_end().to_owned())
}

/// Build every input in one isolated Cargo phase and publish only the fully checked candidate.
pub(super) fn prepare(workspace: &Workspace, options: &Options, output: &Path, logs: &Path) -> Result<Prepared> {
    let authored = gltf_download_directory(workspace.root());
    require_gltf_downloads(&authored)?;
    let shim = workspace.member(SHIM, "crates/platform-dylib/Cargo.toml")?;
    let examples = workspace.member(EXAMPLES, "examples/native/Cargo.toml")?;
    let artifacts = tempfile::Builder::new()
        .prefix(".cargo-")
        .tempdir_in(output)
        .context("create private Cargo build root")?;
    let directories = CargoDirectories::create(artifacts.path(), "native")?;
    let draft = tempfile::Builder::new()
        .prefix(".candidate-")
        .tempdir_in(output)
        .context("create private draft deployment")?;
    let cargo = |subcommand| directories.command(workspace.cargo_workspace(subcommand));
    let backend_features = format!("{}{}", options.backend.name(), if options.shader_python { ",shader-python" } else { "" });
    let application_features = if options.shader_python { "dynamic,shader-python" } else { "dynamic" };
    let rustc = compiler_version(
        cargo("rustc").args([
            "--locked",
            "--profile",
            PROFILE,
            "-p",
            SHIM,
            "--no-default-features",
            "--features",
            &backend_features,
            "--lib",
        ]),
        logs,
    )?;
    // The static corpus reads source files and does not consume the candidate.
    cargo_log(
        cargo("test")
            .args(["--locked", "-p", EXAMPLES, "--no-default-features", "--features"])
            .arg(format!("static,{backend_features},test-support"))
            .args(["--test", "shader_corpus"])
            .env_remove("NEOCOGI_CARGO_PROFILE_DIR")
            .env("NEOCOGI_NATIVE_OUTPUT_DIR", artifacts.path().join("corpus-stage"))
            .env("NEOCOGI_SHADER_ARTIFACT_DIR", output.join("corpus"))
            .env("NEOCOGI_SHADER_VALIDATORS", "1"),
        logs,
        "corpus",
    )?;
    let library_output = cargo_log(
        cargo("build")
            .args([
                "--locked",
                "--profile",
                PROFILE,
                "-p",
                SHIM,
                "--no-default-features",
                "--features",
                &backend_features,
                "--lib",
                "--message-format=json",
            ])
            .env_remove("NEOCOGI_NATIVE_OUTPUT_DIR"),
        logs,
        "provider",
    )?;
    let target = directories.target.canonicalize()?;
    let library = select(&fs::read(library_output)?, shim, &backend_features, Kind::Provider, &["nc_platform"], &target)?
        .remove("nc_platform")
        .expect("validated platform receipt");
    let mut build = cargo("build");
    build
        .args([
            "--locked",
            "--profile",
            PROFILE,
            "-p",
            EXAMPLES,
            "--no-default-features",
            "--features",
            application_features,
            "--message-format=json",
        ])
        .env(
            "NEOCOGI_CARGO_PROFILE_DIR",
            library.path.parent().context("library is missing a Cargo profile directory")?,
        )
        .env("NEOCOGI_NATIVE_OUTPUT_DIR", draft.path());
    for name in APPLICATIONS {
        build.args(["--bin", name]);
    }
    let apps_output = cargo_log(&mut build, logs, "applications")?;
    let apps = select(
        &fs::read(apps_output)?,
        examples,
        application_features,
        Kind::Application,
        &APPLICATIONS,
        &target,
    )?;
    let library_relative = Path::new("lib").join(library.path.file_name().context("library filename")?);
    let provider = library.verify(draft.path(), &library_relative)?;
    if matches!(options.backend, Backend::Vulkan) {
        ensure!(
            provider.file.bytes < 524_288,
            "complete Vulkan platform exceeds its byte cap: {}",
            provider.file.bytes
        );
    }
    let mut applications = BTreeMap::new();
    for (name, artifact) in apps {
        let relative = PathBuf::from(artifact.path.file_name().context("executable filename")?);
        fs::copy(&artifact.path, draft.path().join(&relative))?;
        applications.insert(name, artifact.verify(draft.path(), &relative)?);
    }
    let mut assets = Vec::new();
    for (name, directory) in DOWNLOADS
        .into_iter()
        .map(|name| (name, authored.as_path()))
        .chain(LICENSES.into_iter().map(|name| (name, authored.parent().expect("glTF source location"))))
    {
        assets.push(verify_staged(&directory.join(name), draft.path(), &Path::new(STAGED_DIR).join(name))?);
    }
    // Warmup may produce a replacement for a missing or invalid cache. Record any authored cache
    // now and the actual warmed cache independently before measurement.
    let cache = authored.join("Cannon_Exterior.ibl");
    if cache.is_file() {
        let relative = Path::new(STAGED_DIR).join("Cannon_Exterior.ibl");
        fs::copy(&cache, draft.path().join(&relative))?;
        assets.push(verify_staged(&cache, draft.path(), &relative)?);
    }
    artifacts.close().context("remove private Cargo artifacts before publishing the candidate")?;
    let candidate = output.join("candidate");
    fs::rename(draft.path(), &candidate).with_context(|| format!("publish checked candidate {}", candidate.display()))?;
    Ok(Prepared {
        candidate,
        rustc,
        deployment: Deployment { provider, applications, assets },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// The fixture includes actual files so receipt paths pass the same physical checks as production.
    fn binary_receipt(member: &Member, target: &Path, name: &str) -> Value {
        let path = target.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        fs::write(&path, name).unwrap();
        json!({
            "reason": "compiler-artifact",
            "package_id": member.id,
            "manifest_path": member.manifest_path,
            "target": {"name": name, "kind": ["bin"]},
            "features": ["dynamic"],
            "filenames": [path],
            "executable": path,
        })
    }

    /// Exactly the three workloads must come from the selected package, features, kind and final root.
    #[test]
    fn rejects_foreign_duplicate_wrong_feature_and_out_of_root_receipts() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        let manifest_path = root.join("Cargo.toml");
        fs::write(&manifest_path, []).unwrap();
        let member = Member {
            id: "examples@1".into(),
            name: EXAMPLES.into(),
            manifest_path,
        };
        let mut all = Vec::new();
        for name in APPLICATIONS {
            all.push(binary_receipt(&member, &target, name));
        }
        let select = |records: &[Value]| {
            super::select(
                records.iter().map(Value::to_string).collect::<Vec<_>>().join("\n").as_bytes(),
                &member,
                "dynamic",
                Kind::Application,
                &APPLICATIONS,
                &target,
            )
        };
        assert_eq!(select(&all).unwrap().len(), APPLICATIONS.len());
        let unrelated = binary_receipt(&member, &target, "deployment_probe");
        all.push(unrelated);
        assert_eq!(select(&all).unwrap().len(), APPLICATIONS.len());
        all.pop();
        let backup = all[0].clone();
        all[0]["package_id"] = json!("other-package@1");
        assert!(select(&all).unwrap_err().to_string().contains("artifact package ID"));
        all[0] = backup.clone();
        let alternate = root.join("alternate.toml");
        fs::write(&alternate, []).unwrap();
        all[0]["manifest_path"] = json!(alternate);
        assert!(select(&all).unwrap_err().to_string().contains("artifact manifest"));
        all[0] = backup.clone();
        all[0]["features"] = json!(["dynamic", "shader-python"]);
        assert!(select(&all).unwrap_err().to_string().contains("artifact features"));
        all[0] = backup.clone();
        all[0]["target"]["kind"] = json!(["example"]);
        assert!(select(&all).unwrap_err().to_string().contains("matching bin receipt"));
        all[0] = binary_receipt(&member, &root, APPLICATIONS[0]);
        assert!(select(&all).unwrap_err().to_string().contains("outside the run's final target directory"));
        all[0] = backup.clone();
        all.push(backup);
        assert!(select(&all).unwrap_err().to_string().contains("multiple matching Cargo receipts"));
    }

    /// Existing files with equal lengths cannot pass deployment verification if their bytes differ.
    #[test]
    fn checks_staged_bytes_before_candidate_publication() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let relative = Path::new("staged");
        let staged = directory.path().join(relative);
        fs::write(&source, b"library bytes").unwrap();
        assert!(verify_staged(&source, directory.path(), relative).is_err());
        fs::write(&staged, b"another bytes").unwrap();
        assert!(
            verify_staged(&source, directory.path(), relative)
                .unwrap_err()
                .to_string()
                .contains("staged bytes differ")
        );
        fs::copy(&source, &staged).unwrap();
        let check = verify_staged(&source, directory.path(), relative).unwrap();
        assert_eq!(check.bytes, 13);
        assert_eq!(check.source_sha256, check.staged_sha256);
    }
}
