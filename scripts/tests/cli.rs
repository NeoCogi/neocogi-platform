//! Actual CLI regressions run with an empty PATH to prove no interpreter or shell is needed.
use std::{fs, path::Path, process::Command};

/// Invoke the built Rust binary without any external executables on PATH.
fn tools(path: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_neocogi-platform-tools"));
    command.env("PATH", path);
    command
}

/// The standalone overlay command writes valid Cargo input while source files remain untouched.
#[test]
fn local_overlay_without_python_or_cargo_on_path() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("Cargo.toml"), "[package]\nname='local-library'\nversion='0.0.0'\n[lib]\n").unwrap();
    let output = tools(directory.path())
        .arg("local-sources")
        .arg("--platform")
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let document: toml::Value = toml::from_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert_eq!(
        document["patch"]["crates-io"]["local-library"]["path"].as_str().unwrap(),
        directory.path().canonicalize().unwrap().to_str().unwrap()
    );
    assert!(!directory.path().join("Cargo.lock").exists());
}

/// LLDB metadata is translated by Rust alone, and explicit output contains only native debugger commands.
#[test]
fn source_map_without_python_or_lldb_on_path() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("app.trim-paths.json"),
        r#"{"v":1,"remaps":[{"from":".","to":"/local sources"}]}"#,
    )
    .unwrap();
    let destination = directory.path().join("maps.lldb");
    let output = tools(directory.path())
        .arg("lldb-source-map")
        .arg(directory.path().join("app"))
        .arg("--output")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(destination).unwrap(),
        "settings append target.source-map \".\" \"/local sources\"\n"
    );
}

/// An incorrectly matched bindgen fails before any Cargo build or browser launch can occur.
#[test]
fn mismatched_bindgen_rejects_before_browser_build() {
    let directory = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_neocogi-platform-tools");
    let output = tools(directory.path())
        .args(["test-browser", "--browser", binary, "--wasm-bindgen", binary])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Expected wasm-bindgen"));
}
