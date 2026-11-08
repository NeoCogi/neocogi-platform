//! Actual selected consumer graphs and invalid selection diagnostics, with no nested build.
#![cfg(not(target_arch = "wasm32"))]
use std::{collections::HashSet, fs, path::Path, process::Command};

/// Cargo itself must select one backend-named platform library from the shim package.
#[test]
fn cargo_library_examples_declare_exact_backend_libraries() {
    let output = Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["metadata", "--no-deps", "--offline", "--locked", "--format-version=1"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let shim = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "neocogi-platform-dylib")
        .unwrap();
    let examples: Vec<_> = shim["targets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|target| target["kind"] == serde_json::json!(["example"]))
        .collect();
    assert_eq!(examples.len(), 3);
    for backend in ["glow", "vulkan", "wgpu"] {
        let name = format!("nc_platform_{backend}");
        let target = examples.iter().find(|target| target["name"] == name).unwrap();
        assert_eq!(target["crate_types"], serde_json::json!(["cdylib"]));
        assert_eq!(target["required-features"], serde_json::json!([backend]));
    }
}

/// Resolve one isolated consumer, explicitly selecting normal and/or build dependency edges.
fn graph(mode: &str, backend: &str, edges: &str) -> HashSet<String> {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("src")).unwrap();
    fs::write(directory.path().join("src/lib.rs"), "").unwrap();
    let package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = serde_json::to_string(package.to_str().unwrap()).unwrap();
    let manifest = format!(
        "[workspace]\n[package]\nname='example-graph'\nversion='0.0.0'\nedition='2024'\n[dependencies]\nneocogi-platform-examples={{path={path},default-features=false,features=['{mode}','{backend}']}}\n"
    );
    fs::write(directory.path().join("Cargo.toml"), manifest).unwrap();
    fs::copy(package.join("../../Cargo.lock"), directory.path().join("Cargo.lock")).unwrap();
    let output = Command::new(env!("CARGO"))
        .current_dir(directory.path())
        .args(["tree", "--offline", "--edges", edges, "--prefix", "none", "--format", "{p}"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| line.split_whitespace().next().unwrap().to_owned())
        .collect()
}

/// Dynamic executables contain no ordinary provider/backend dependency or linked fallback.
#[test]
fn static_and_dynamic_graphs_keep_provider_implementation_ownership_distinct() {
    for backend in ["glow", "vulkan", "wgpu"] {
        let dynamic = graph("dynamic", backend, "normal");
        for forbidden in ["neocogi-platform-plugin", "neocogi-shader-compiler", "naga", "sdl2", "wgpu", "ash"] {
            assert!(!dynamic.contains(forbidden), "{backend}: {dynamic:?}");
        }
        assert!(dynamic.contains("neocogi-platform"));
        assert!(dynamic.contains("neocogi-example-plugins"));
        let linked = graph("static", backend, "normal");
        assert!(linked.contains("neocogi-platform"));
        assert!(linked.contains("naga"));
        assert!(linked.contains("sdl2"));
        assert!(!linked.contains("neocogi-example-plugins"));
    }
}

/// Dynamic compilation must keep renderer features out of host-side build helpers too.
#[test]
fn dynamic_build_helpers_do_not_compile_shader_or_native_backend_implementations() {
    for backend in ["glow", "vulkan", "wgpu"] {
        let selected = graph("dynamic", backend, "normal,build");
        for forbidden in ["naga", "sdl2", "glow", "wgpu", "ash"] {
            assert!(!selected.contains(forbidden), "{backend} enabled {forbidden} in its build graph: {selected:?}");
        }
        assert!(selected.contains("neocogi-idl"), "schema generation remains available");
    }
}

/// Selection errors must precede implementation expansion, including every invalid subset.
#[test]
fn invalid_assembly_and_backend_subsets_report_only_selection_diagnostics() {
    let temporary = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    let features = ["static", "dynamic", "glow", "vulkan", "wgpu"];
    for mask in 0u32..32 {
        if (mask & 3).count_ones() == 1 && (mask >> 2).count_ones() == 1 {
            continue;
        }
        let mut compiler = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
        compiler
            .args(["--edition=2024", "--crate-type=rlib", "--emit=metadata", "--crate-name=selection_probe"])
            .arg(&source)
            .arg("--out-dir")
            .arg(temporary.path());
        for (bit, feature) in features.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                compiler.arg("--cfg").arg(format!("feature=\"{feature}\""));
            }
        }
        let result = compiler.output().unwrap();
        let diagnostic = String::from_utf8(result.stderr).unwrap();
        assert!(!result.status.success(), "{mask:05b}");
        assert!(
            diagnostic.contains("select exactly") || diagnostic.contains("mutually exclusive"),
            "{diagnostic}"
        );
        assert!(!diagnostic.contains("error["), "{mask:05b}: {diagnostic}");
    }
}
