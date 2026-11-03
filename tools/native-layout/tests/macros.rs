//! Compile actual proc-macro consumers without nested Cargo or another target directory.
// Load this integration test's actual macro dependency so rustc records its library in dep-info.
use neocogi_native_layout as _;
#[path = "support/artifact.rs"]
mod artifact;
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
    /// Uses this test's actual compiler-recorded dependency; no alternative library may substitute.
    fn new() -> Self {
        let executable = std::env::current_exe().unwrap();
        Self::from_artifact(artifact::selected_macro(&executable).expect("compiler-selected macro dependency"))
    }
    /// Creates isolated input/output storage for an explicitly selected artifact, also used by negative controls.
    fn from_artifact(macros: PathBuf) -> Self {
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

/// A newer, genuinely different macro must not replace the library named by compiler dependency data.
/// The negative control loads that other library deliberately, proving its failure is distinguishable.
#[test]
fn newer_unselected_macro_cannot_replace_the_compiler_selected_dependency() {
    let selected = artifact::selected_macro(&std::env::current_exe().unwrap()).unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let source = root.join("wrong.rs");
    fs::write(
        &source,
        r#"extern crate proc_macro;
        #[proc_macro] pub fn stage_resources(_: proc_macro::TokenStream) -> proc_macro::TokenStream {
            "compile_error!(\"unselected macro was loaded\");".parse().unwrap()
        }"#,
    )
    .unwrap();
    let wrong = root.join(format!(
        "{}neocogi_native_layout-newer{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    let result = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .args(["--edition=2024", "--crate-name=neocogi_native_layout", "--crate-type=proc-macro"])
        .arg(&source)
        .arg("-o")
        .arg(&wrong)
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let newer = fs::metadata(&selected).unwrap().modified().unwrap() + std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(&wrong)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(newer))
        .unwrap();
    assert!(fs::metadata(&wrong).unwrap().modified().unwrap() > fs::metadata(&selected).unwrap().modified().unwrap());
    // Copy the compiler's actual record beside the distractor, simulating a profile containing several builds.
    let executable = root.join("consumer");
    fs::copy(std::env::current_exe().unwrap().with_extension("d"), executable.with_extension("d")).unwrap();
    let chosen = artifact::selected_macro(&executable).unwrap();
    assert_eq!(chosen, selected);
    let code = "fn main(){neocogi_native_layout::stage_resources!(\"assets\"=>\"data\");}";
    let rejected = Consumer::from_artifact(wrong);
    let result = rejected.compile(code, Some(&rejected.directory.path().join("out")), None, None);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("unselected macro was loaded"));
    let accepted = Consumer::from_artifact(chosen);
    let output = accepted.directory.path().join("out");
    let result = accepted.compile(code, Some(&output), None, None);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::read(output.join("data/keep")).unwrap(), b"keep");
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

/// Public macro expansion must reject cross-mapping temporary contamination in both orders and for empty inputs.
/// Earlier installed files and ownership receipts remain intact after the compiler diagnostic.
#[test]
fn macro_rejects_temporary_directories_inside_any_input_before_copying() {
    for empty in [false, true] {
        for reverse in [false, true] {
            let consumer = Consumer::new();
            let root = consumer.directory.path();
            let output = root.join("out");
            let initial = consumer.compile(
                "fn main(){neocogi_native_layout::stage_resources!(\"assets\"=>\"installed\");}",
                Some(&output),
                None,
                None,
            );
            assert!(initial.status.success(), "{}", String::from_utf8_lossy(&initial.stderr));
            let receipts = fs::read(output.join(".neocogi-layout.json")).unwrap();
            fs::write(root.join("input.txt"), b"A").unwrap();
            fs::create_dir(output.join("shared")).unwrap();
            if !empty {
                fs::write(output.join("shared/keep"), b"keep").unwrap();
            }
            let pairs = if reverse {
                "\"out/shared\"=>\"b\",\"input.txt\"=>\"shared/a.txt\""
            } else {
                "\"input.txt\"=>\"shared/a.txt\",\"out/shared\"=>\"b\""
            };
            let result = consumer.compile(
                &format!("fn main(){{neocogi_native_layout::stage_resources!({pairs});}}"),
                Some(&output),
                None,
                None,
            );
            assert!(!result.status.success());
            let diagnostic = String::from_utf8_lossy(&result.stderr);
            assert!(
                diagnostic.contains("temporary directory") && diagnostic.contains("inside resource input"),
                "{diagnostic}"
            );
            assert_eq!(fs::read(output.join("installed/keep")).unwrap(), b"keep");
            assert_eq!(fs::read(output.join(".neocogi-layout.json")).unwrap(), receipts);
            assert!(!output.join("b").exists());
            assert!(!output.join("shared/a.txt").exists());
            assert_eq!(fs::read_dir(output.join("shared")).unwrap().count(), usize::from(!empty));
        }
    }
    // A single mapping must reject a recursive source/output relation before attempting a copy.
    let consumer = Consumer::new();
    let nested = consumer.directory.path().join("assets/out");
    let result = consumer.compile(
        "fn main(){neocogi_native_layout::stage_resources!(\"assets\"=>\"data\");}",
        Some(&nested),
        None,
        None,
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("inside resource input"));
    assert!(!nested.join("data").exists());
}

/// Equality with an already owned output remains valid because its candidate is prepared outside the input.
#[test]
fn macro_can_copy_an_owned_destination_onto_itself() {
    let consumer = Consumer::new();
    let output = consumer.directory.path().join("out");
    for input in ["assets", "out/installed"] {
        let result = consumer.compile(
            &format!("fn main(){{neocogi_native_layout::stage_resources!(\"{input}\"=>\"installed\");}}"),
            Some(&output),
            None,
            None,
        );
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        assert_eq!(fs::read(output.join("installed/keep")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(output.join("installed")).unwrap().count(), 1);
    }
}

/// Exercise the actual exported macro with directory/file/trailing-dot paths that traverse an ancestor link.
#[cfg(unix)]
#[test]
fn macro_rejects_input_ancestor_symlinks_and_preserves_previous_output() {
    use std::os::unix::fs::symlink;
    let consumer = Consumer::new();
    let root = consumer.directory.path();
    let output = root.join("out");
    let first = consumer.compile(
        "fn main(){neocogi_native_layout::stage_resources!(\"assets\"=>\"installed\");}",
        Some(&output),
        None,
        None,
    );
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let receipts = fs::read(output.join(".neocogi-layout.json")).unwrap();
    symlink(root, root.join("linked")).unwrap();
    for input in ["linked/assets", "linked/assets/keep", "linked/."] {
        let result = consumer.compile(
            &format!("fn main(){{neocogi_native_layout::stage_resources!(\"{input}\"=>\"new-assets\");}}"),
            Some(&output),
            None,
            None,
        );
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("symlink"));
        assert_eq!(fs::read(output.join("installed/keep")).unwrap(), b"keep");
        assert_eq!(fs::read(output.join(".neocogi-layout.json")).unwrap(), receipts);
        assert!(!output.join("new-assets").exists());
    }
}

/// Parse the real compiler's rules and expand its selected library under punctuation-bearing paths.
/// A portable subset runs on Windows; Unix additionally admits literal backslashes/colons in names.
#[test]
fn compiler_emitted_paths_select_and_expand_the_exact_macro() {
    let selected = artifact::selected_macro(&std::env::current_exe().unwrap()).unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let mut names = vec!["ordinary", "space dir", "hash#dir", "money$dir", "double$$dir"];
    if cfg!(unix) {
        names.extend([r"slash\back", "colon:dir", r"double\\back", r"back\ space", r"back\#hash", r"back\:colon"]);
    }
    for name in names {
        let root = fixture.path().join(name);
        fs::create_dir(&root).unwrap();
        let library = root.join(selected.file_name().unwrap());
        fs::copy(&selected, &library).unwrap();
        let source = root.join("producer.rs");
        fs::write(&source, "use neocogi_native_layout as _; fn main(){}").unwrap();
        let executable = root.join("producer");
        let result = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args([
                "--edition=2024",
                "--crate-name=record_producer",
                "-Zbinary-dep-depinfo",
                "--emit=metadata,dep-info",
            ])
            .arg(&source)
            .arg("--extern")
            .arg(format!("neocogi_native_layout={}", library.display()))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(result.status.success(), "{name}: {}", String::from_utf8_lossy(&result.stderr));
        let exact = artifact::selected_macro(&executable).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(exact, library);
        let consumer = Consumer::from_artifact(exact);
        let output = consumer.directory.path().join("out");
        let result = consumer.compile(
            "fn main(){neocogi_native_layout::stage_resources!(\"assets\"=>\"data\");}",
            Some(&output),
            None,
            None,
        );
        assert!(result.status.success(), "{name}: {}", String::from_utf8_lossy(&result.stderr));
        assert_eq!(fs::read(output.join("data/keep")).unwrap(), b"keep");
    }
}
