//! Stages explicitly prebuilt Cargo library examples; this build script never runs Cargo.
use std::path::PathBuf;
#[path = "src/artifacts.rs"]
pub mod artifacts;

/// Track exact named inputs and deployed outputs, so a later shim build invalidates cached staging.
fn main() {
    use neocogi_platform_schema::build::layout::{output_root, stage_library};
    let anchor = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let profile_root = output_root(Some(&anchor), None).expect("Cargo profile output root");
    let directory = profile_root.file_name().and_then(|value| value.to_str()).expect("Cargo profile name");
    let profile = if directory == "debug" { "dev" } else { directory };
    println!("cargo::rustc-env=NEOCOGI_PLUGIN_BUILD_PROFILE={profile}");
    println!("cargo::rerun-if-env-changed=NEOCOGI_NATIVE_OUTPUT_DIR");
    let output = output_root(Some(&anchor), std::env::var_os("NEOCOGI_NATIVE_OUTPUT_DIR").map(Into::into)).expect("native deployment output");
    let system = artifacts::System::from_os(&std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo target OS"));
    for backend in artifacts::Backend::ALL {
        let [filename] = artifacts::libraries(backend, system);
        // These are explicitly declared public Cargo example filenames, under the
        // same Cargo-owned profile as the application. No hashes/newest-file guesses.
        let source = profile_root.join("examples").join(filename);
        let staged = output.join("lib").join(filename);
        println!("cargo::rerun-if-changed={}", source.display());
        println!("cargo::rerun-if-changed={}", staged.display());
        if source.is_file() {
            stage_library(&source, &output).expect("stage prebuilt Cargo plugin");
        }
        // Missing inputs leave help/build usable; startup returns an owned preparation
        // error before loading any library or preparing models/HDR lighting.
    }
}
