//! Actual CLI regressions for workspace selection and workspace-free commands.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

/// The same executable can be launched from many independent workspaces.
const BINARY: &str = env!("CARGO_BIN_EXE_neocogi-platform-tools");

/// Invoke the built Rust binary without any external executables on PATH.
fn tools(path: &Path) -> Command {
    let mut command = Command::new(BINARY);
    command.current_dir(path).env("PATH", path);
    command
}

/// The standalone overlay command writes valid Cargo input while source files remain untouched.
#[test]
fn local_overlay_without_python_or_cargo_on_path() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("Cargo.toml"), "[package]\nname='local-library'\nversion='0.0.0'\n[lib]\n").unwrap();
    let output = tools(directory.path()).arg("local-sources").arg("--platform").arg(".").output().unwrap();
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
    let output = tools(directory.path())
        .args(["lldb-source-map", "app", "--output", "maps.lldb"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(directory.path().join("maps.lldb")).unwrap(),
        "settings append target.source-map \".\" \"/local sources\"\n"
    );
}

/// Help and explicit asset destinations can be interpreted without Cargo or a workspace.
#[test]
fn help_and_explicit_asset_destination_without_workspace() {
    let directory = tempfile::tempdir().unwrap();
    assert!(tools(directory.path()).arg("--help").output().unwrap().status.success());
    fs::write(directory.path().join("file"), b"not a directory").unwrap();
    let output = tools(directory.path())
        .args(["fetch-gltf-examples", "--output", "file/assets"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("workspace"), "{stderr}");
}

/// A minimal dependency-free workspace exercises discovery through real Cargo metadata.
fn workspace(root: &Path, member: &str, pin: &str) {
    fs::create_dir_all(root.join(member).join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nresolver='3'\nmembers=['{member}']\n[workspace.dependencies]\nwasm-bindgen='={pin}'\n"),
    )
    .unwrap();
    fs::write(
        root.join(member).join("Cargo.toml"),
        "[package]\nname='neocogi-platform-tools'\nversion='0.0.0'\nedition='2024'\n",
    )
    .unwrap();
    fs::write(root.join(member).join("src/lib.rs"), "").unwrap();
}

/// The tools binary prints a real version which never matches the fixture's bindgen pin.
fn mismatched_bindgen(directory: &Path, options: &[&str]) -> Output {
    Command::new(BINARY)
        .current_dir(directory)
        .args(["test-browser", "--browser", BINARY, "--wasm-bindgen", BINARY])
        .args(options)
        .output()
        .unwrap()
}

/// The same executable reads each discovered pin and follows an explicit workspace from outside.
#[test]
fn browser_uses_invocation_workspace_and_preserves_version_diagnostic() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    workspace(&first, "scripts", "0.2.101");
    workspace(&second, "scripts", "0.2.102");
    let caller = directory.path().join("caller");
    fs::create_dir(&caller).unwrap();
    for (cwd, flags, pin) in [
        (first.as_path(), &[][..], "0.2.101"),
        (second.join("scripts/src").as_path(), &[][..], "0.2.102"),
        (caller.as_path(), &["--workspace", "../second"][..], "0.2.102"),
    ] {
        let output = mismatched_bindgen(cwd, flags);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(&format!("Expected wasm-bindgen {pin}")), "{stderr}");
    }
}

/// A similarly named package at another manifest cannot grant checkout access.
#[test]
fn discovery_rejects_wrong_member_path_and_explicit_nonroot() {
    let directory = tempfile::tempdir().unwrap();
    workspace(directory.path(), "elsewhere", "0.2.101");
    let output = mismatched_bindgen(directory.path(), &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("must contain the neocogi-platform-tools member at"), "{stderr}");
    let output = mismatched_bindgen(directory.path(), &["--workspace", "elsewhere"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--workspace must name the Cargo workspace root"), "{stderr}");
}
