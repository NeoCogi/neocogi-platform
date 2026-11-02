//! Repository build helper for shared IDL and existing Rust helper libraries.

/// Generates the requested namespaces in this consumer; other namespaces use
/// existing library definitions where ordinary Rust helper APIs exchange them.
pub fn sources(repository: &str, local: &[&str]) -> neocogi_idl::cargo::SharedSources {
    let locations = [
        ("neocogi.shader", "neocogi_shader_core::abi"),
        ("neocogi.renderer", "neocogi_render_core::abi"),
        ("neocogi.platform", "neocogi_render_core::platform"),
    ];
    assert!(
        local.iter().all(|name| locations.iter().any(|(known, _)| known == name)),
        "unknown shared IDL module"
    );
    let repository = std::path::Path::new(repository);
    let mut request = neocogi_idl::cargo::SharedSources::new(repository.join("crates/render/core/schemas"));
    request = request.include_root(repository.join("crates/shader/core/schemas"));
    for (name, rust) in locations {
        request = if local.contains(&name) {
            request.module(name, rust)
        } else {
            request.import(name, rust)
        };
    }
    request
}

/// Publishes generated include paths for the calling crate only.
pub fn build(request: neocogi_idl::cargo::SharedSources) -> Vec<neocogi_idl::cargo::BuildOutput> {
    let modules = request.build().expect("generate shared IDL bindings");
    for module in &modules {
        println!(
            "cargo::rustc-env=IDL_{}={}",
            module.module_name.replace('.', "_").to_uppercase(),
            module.include.display()
        );
    }
    modules
}
