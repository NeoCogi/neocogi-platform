//! Independent source generation and an explicit compile-time staging destination.
use neocogi_platform::build::{Module, build, sources};

/// Every build input comes from a declared dependency or this consumer's own files.
fn main() {
    sources(&[Module::Shader]).and_then(build).expect("generate independent shader caller");
    // Each disposable consumer owns a fresh output root and its staging receipts.
    // Reusing a Cargo cache must not reuse another extraction's exclusive owner.
    let staged = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest")).join("staged");
    use neocogi_platform::build::layout::{Resource, stage_resources};
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let resources = [Resource { source: manifest.join("asset.txt"), destination: "asset.txt".into() }];
    let tracked = stage_resources(&manifest, &staged, &resources).unwrap();
    for path in tracked.inputs.into_iter().chain(tracked.outputs) {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    println!("cargo::rustc-env=NEOCOGI_NATIVE_OUTPUT_DIR={}", staged.display());
}
