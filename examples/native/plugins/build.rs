//! Stages the prebuilt normal Cargo platform library; this build script never runs Cargo.
use std::path::PathBuf;
#[path = "src/artifacts.rs"]
pub mod artifacts;

/// Track exact named inputs and deployed outputs, so a later shim build invalidates cached staging.
fn main() {
    use neocogi_platform_contracts::build::layout::{cargo_layout, mirror_library, stage_library};
    let anchor = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    println!("cargo::rerun-if-env-changed=NEOCOGI_CARGO_PROFILE_DIR");
    println!("cargo::rerun-if-env-changed=NEOCOGI_NATIVE_OUTPUT_DIR");
    let layout = cargo_layout(
        &anchor,
        std::env::var_os("NEOCOGI_CARGO_PROFILE_DIR").map(Into::into),
        std::env::var_os("NEOCOGI_NATIVE_OUTPUT_DIR").map(Into::into),
    )
    .expect("resolve native layout");
    let Some(profile_root) = layout.profile else {
        println!("cargo::rustc-env=NEOCOGI_PLUGIN_BUILD_PROFILE=");
        println!("cargo::warning=Native shim staging needs NEOCOGI_CARGO_PROFILE_DIR set to the absolute Cargo final profile directory");
        return;
    };
    let directory = profile_root.file_name().and_then(|value| value.to_str()).expect("Cargo profile name");
    let profile = if directory == "debug" { "dev" } else { directory };
    println!("cargo::rustc-env=NEOCOGI_PLUGIN_BUILD_PROFILE={profile}");
    let output = layout.destination.expect("a known profile supplies the default deployment destination");
    let system = artifacts::System::from_os(&std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo target OS"));
    let filename = artifacts::library_filename(system);
    // The public library target lives directly in Cargo's profile output root.
    // Track both sides so rebuilding it or deleting its deployed copy restages it.
    let source = profile_root.join(filename);
    let staged = output.join("lib").join(filename);
    println!("cargo::rerun-if-changed={}", source.display());
    println!("cargo::rerun-if-changed={}", staged.display());
    if output == profile_root
        || output
            .canonicalize()
            .is_ok_and(|path| path == profile_root.canonicalize().expect("Cargo profile output"))
    {
        mirror_library(&profile_root, filename).expect("mirror Cargo plugin in its development profile");
    } else if source.is_file() {
        stage_library(&source, &output).expect("stage prebuilt Cargo plugin");
    }
    // Missing inputs leave help/build usable; startup returns an owned preparation
    // error before loading the library or preparing models/HDR lighting.
}
