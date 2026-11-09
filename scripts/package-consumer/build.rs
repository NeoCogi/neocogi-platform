//! Independent source generation and an explicit compile-time staging destination.
use neocogi_platform_contracts::build::{Module, build, sources};

/// Every build input comes from a declared dependency or this consumer's own files.
fn main() {
    // Resolve audio in isolation before the full caller. Its source graph must not
    // reach platform/renderer just because those imports are registered by the helper.
    let audio_only = sources(&[Module::Audio])
        .and_then(build)
        .expect("generate audio without window/renderer schemas");
    assert_eq!(audio_only.len(), 1);
    assert_eq!(audio_only[0].module_name, Module::Audio.name());
    let graph: serde_json::Value = serde_json::from_slice(&std::fs::read(&audio_only[0].dependency_graph).unwrap()).unwrap();
    let names: Vec<_> = graph["modules"].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(names, [Module::Audio.name()]);

    // The final generation includes this exact audio ABI with the two other projections.
    // It owns the final graph/emissions; no earlier output is needed after this call.
    let modules = sources(&[Module::Renderer, Module::Audio, Module::Platform])
        .and_then(build)
        .expect("generate independent shader/audio/platform callers");
    let audio = modules.iter().find(|module| module.module_name == Module::Audio.name()).unwrap();
    assert_eq!(audio.module_id, audio_only[0].module_id);
    assert_eq!(audio.abi_fingerprint, audio_only[0].abi_fingerprint);
    // Each disposable consumer owns a fresh output root and its staging receipts.
    // Reusing a Cargo cache must not reuse another extraction's exclusive owner.
    let staged = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest")).join("staged");
    use neocogi_platform_contracts::build::layout::{Resource, stage_resources};
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let resources = [Resource {
        source: manifest.join("asset.txt"),
        destination: "asset.txt".into(),
    }];
    let tracked = stage_resources(&manifest, &staged, &resources).unwrap();
    for path in tracked.inputs.into_iter().chain(tracked.outputs) {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    println!("cargo::rustc-env=NEOCOGI_NATIVE_OUTPUT_DIR={}", staged.display());
}
