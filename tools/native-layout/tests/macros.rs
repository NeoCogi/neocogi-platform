//! Compile actual proc-macro consumers without nested Cargo or another target directory.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// One standalone rustc consumer; all generated output is removed with this fixture.
struct Consumer {
    /// Owns the manifest, source, metadata and staged files.
    directory: tempfile::TempDir,
    /// Actual Cargo-built proc-macro library, loaded by the same pinned rustc.
    macros: PathBuf,
    /// Dependency lookup directory for the proc-macro host.
    dependencies: PathBuf,
}
impl Consumer {
    /// Finds the compiler-selected host profile rather than reconstructing its hash.
    fn new() -> Self {
        let executable = std::env::current_exe().unwrap();
        let profile = neocogi_native_layout_core::output_root(Some(&executable), None).unwrap();
        let prefix = format!("{}neocogi_native_layout-", std::env::consts::DLL_PREFIX);
        let mut pending = vec![profile];
        let mut candidates = Vec::new();
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap().filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.file_name().unwrap().to_string_lossy().starts_with(&prefix) && path.to_string_lossy().ends_with(std::env::consts::DLL_SUFFIX) {
                    candidates.push(path);
                }
            }
        }
        let macros = candidates
            .into_iter()
            .max_by_key(|path| fs::metadata(path).unwrap().modified().unwrap())
            .expect("Cargo built the proc macro before this integration test");
        let dependencies = macros.parent().unwrap().to_owned();
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("assets")).unwrap();
        fs::write(directory.path().join("assets/keep"), b"keep").unwrap();
        Self { directory, macros, dependencies }
    }
    /// Runs the actual expansion with no build script/OUT_DIR unless explicitly supplied.
    fn compile(&self, source: &str, output: Option<&Path>, anchor: Option<&Path>, artifact: Option<&Path>) -> Output {
        let root = self.directory.path();
        fs::write(root.join("probe.rs"), source).unwrap();
        let mut command = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
        command
            .args(["--edition=2024", "--crate-name=layout_probe", "--emit=metadata"])
            .arg(root.join("probe.rs"))
            .arg("--out-dir")
            .arg(root)
            .arg("--extern")
            .arg(format!("neocogi_native_layout={}", self.macros.display()))
            .arg("-L")
            .arg(format!("dependency={}", self.dependencies.display()))
            .env("CARGO_MANIFEST_DIR", root)
            .env_remove("OUT_DIR")
            .env_remove("NEOCOGI_NATIVE_OUTPUT_DIR");
        if let Some(output) = output {
            command.env("NEOCOGI_NATIVE_OUTPUT_DIR", output);
        }
        if let Some(anchor) = anchor {
            command.env("OUT_DIR", anchor);
        }
        if let Some(artifact) = artifact {
            command.env("CARGO_CDYLIB_FILE_FIXTURE_fixture", artifact);
        }
        command.output().unwrap()
    }
}

/// Explicit resource output works without OUT_DIR and source removal reaches actual expansion.
#[test]
fn explicit_resource_root_and_source_deletion_work_through_rustc() {
    let consumer = Consumer::new();
    let output = consumer.directory.path().join("application with spaces");
    let code = "fn main(){neocogi_native_layout::stage_resources!(\"assets\"=>\"data\");}";
    fs::write(consumer.directory.path().join("assets/obsolete"), b"obsolete").unwrap();
    let first = consumer.compile(code, Some(&output), None, None);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    fs::remove_file(consumer.directory.path().join("assets/obsolete")).unwrap();
    let second = consumer.compile(code, Some(&output), None, None);
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    assert!(output.join("data/keep").is_file());
    assert!(!output.join("data/obsolete").exists());
}

/// Missing/relative output errors and valid inference are compiler-facing contracts.
#[test]
fn output_errors_are_focused_and_consumer_inference_is_preserved() {
    let consumer = Consumer::new();
    let code = "fn main(){neocogi_native_layout::stage_resources!(\"assets\"=>\"data\");}";
    for (output, expected) in [(None, "consumer build.rs"), (Some(Path::new("relative")), "must be absolute")] {
        let result = consumer.compile(code, output, None, None);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains(expected));
    }
    let output = consumer.directory.path().join("profile");
    fs::create_dir(&output).unwrap();
    fs::write(output.join(".cargo-lock"), []).unwrap();
    let result = consumer.compile(code, None, Some(&output.join("build/pkg/out")), None);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(output.join("data/keep").is_file());
}

/// Library expansion uses an actual compiled cdylib and needs no dummy build script.
#[test]
fn library_staging_honors_explicit_output_without_out_dir() {
    let consumer = Consumer::new();
    let root = consumer.directory.path();
    fs::write(root.join("library.rs"), "#[unsafe(no_mangle)] pub extern \"C\" fn fixture() {}").unwrap();
    let result = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .args(["--edition=2024", "--crate-name=fixture", "--crate-type=cdylib"])
        .arg(root.join("library.rs"))
        .arg("--out-dir")
        .arg(root)
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let filename = format!("{}fixture{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX);
    let artifact = root.join(&filename);
    let output = root.join("application");
    let code = "const NAME:&str=neocogi_native_layout::stage_library!(\"CARGO_CDYLIB_FILE_FIXTURE_fixture\");fn main(){}";
    let result = consumer.compile(code, Some(&output), None, Some(&artifact));
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::read(output.join("lib").join(filename)).unwrap(), fs::read(artifact).unwrap());
}

/// A staging-only consumer must not compile renderer/COM/IDL contracts for its host.
#[test]
fn staging_dependency_graph_has_no_runtime_contract_owner() {
    let result = Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "tree",
            "--offline",
            "-p",
            "neocogi-native-layout",
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let graph = String::from_utf8(result.stdout).unwrap();
    for package in ["neocogi-render-core", "neocogi-com", "neocogi-idl", "rs-math3d"] {
        assert!(!graph.lines().any(|line| line.starts_with(&format!("{package} "))), "{graph}");
    }
}
