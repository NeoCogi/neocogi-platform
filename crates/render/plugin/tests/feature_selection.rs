//! Invalid backend combinations must stop at selection diagnostics before implementation expansion.
#![cfg(not(target_arch = "wasm32"))]

use std::{path::Path, process::Command};

/// Compiles the actual crate entrypoint for every illegal native feature subset.
///
/// No external dependencies are supplied: a rejected selection must not expand the implementation
/// or attempt to resolve backend types. Valid selections are covered by ordinary Cargo builds.
/// This also catches duplicate generated factory symbols without maintaining a second fake crate.
#[test]
fn invalid_features_report_selection_errors_without_implementation_errors() {
    let output = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    let features = ["glow", "vulkan", "wgpu", "webgl"];
    for mask in 0_u8..16 {
        if matches!(mask, 1 | 2 | 4) {
            continue; // Exactly one native backend is legal on this native test host.
        }
        let mut compiler = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
        compiler.args(["--edition=2024", "--crate-type=rlib", "--emit=metadata", "--crate-name=renderer_feature_probe"]);
        compiler.arg(&source).arg("--out-dir").arg(output.path());
        for (index, feature) in features.iter().enumerate() {
            if mask & (1 << index) != 0 {
                compiler.arg("--cfg").arg(format!("feature=\"{feature}\""));
            }
        }
        let result = compiler.output().unwrap();
        let errors = String::from_utf8(result.stderr).unwrap();
        assert!(!result.status.success(), "illegal feature mask {mask:04b} compiled");
        let expected = match mask {
            0 => "select exactly one renderer feature",
            8 => "the webgl renderer plugin requires wasm32",
            _ => "renderer features are mutually exclusive",
        };
        assert!(errors.contains(expected), "mask {mask:04b}: {errors}");
        assert!(!errors.contains("error["), "implementation expanded for mask {mask:04b}: {errors}");
        if mask == 3 {
            assert!(errors.contains("--no-default-features --features vulkan"), "{errors}");
            assert!(
                errors.contains("cargo run -p cargo-neocogi -- run --assembly full --variant vulkan --release"),
                "{errors}"
            );
            assert_eq!(errors.matches("error: renderer features").count(), 1, "{errors}");
        }
    }
}
