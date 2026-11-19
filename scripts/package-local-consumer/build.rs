//! Generates all shared namespaces without a regular Platform Contracts dependency.
use neocogi_platform_contracts::build::{Module, build, sources};

/// The emitted path of each namespace belongs exclusively to this consumer.
fn main() {
    // An audio-only generation reaches neither the platform nor renderer imports.
    let audio_only = sources(&[(Module::Audio, "local_projection::sound")])
        .and_then(build)
        .expect("generate audio in isolation");
    assert_eq!(audio_only.len(), 1);
    assert_eq!(audio_only[0].module_name, Module::Audio.name());
    let graph: serde_json::Value = serde_json::from_slice(&std::fs::read(&audio_only[0].dependency_graph).unwrap()).unwrap();
    let names: Vec<_> = graph["modules"].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(names, [Module::Audio.name()]);

    let modules = sources(&[
        (Module::Renderer, "local_projection::graphics"),
        (Module::Audio, "local_projection::sound"),
        (Module::Platform, "local_projection::host"),
    ])
    .and_then(build)
    .expect("generate build-only Platform projections");
    let audio = modules.iter().find(|module| module.module_name == Module::Audio.name()).unwrap();
    assert_eq!(audio.module_id, audio_only[0].module_id);
    assert_eq!(audio.abi_fingerprint, audio_only[0].abi_fingerprint);
}
