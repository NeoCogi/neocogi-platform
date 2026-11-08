//! All schema features must remain usable without the renderer in build or runtime graphs.
use std::{collections::HashSet, process::Command};

/// The schema/build package must never acquire an implementation edge, including target-specific ones.
#[test]
fn all_schema_features_exclude_renderer_shader_compiler_and_native_backends() {
    let output = Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "tree",
            "--locked",
            "--offline",
            "-p",
            "neocogi-platform-schema",
            "--all-features",
            "--target",
            "all",
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let names: HashSet<_> = std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .map(|line| line.split_whitespace().next().unwrap())
        .collect();
    for forbidden in ["neocogi-platform", "naga", "sdl2", "ash", "wgpu", "glow"] {
        assert!(!names.contains(forbidden), "schema package enabled {forbidden}: {names:?}");
    }
    assert!(names.contains("neocogi-com"));
    assert!(names.contains("neocogi-idl"));
}
