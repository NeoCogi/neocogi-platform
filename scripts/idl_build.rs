//! Repository build helper for shared IDL and existing Rust helper libraries.

/// Generates the requested namespaces in this consumer; other namespaces use
/// existing library definitions where ordinary Rust helper APIs exchange them.
pub fn sources(repository: &str, local: &[&str]) -> neocogi_idl::cargo::SharedSources {
    let locations = [
        ("neocogi.ui.runtime.internal", "nc_ui::choice_menu::bindings"),
        ("neocogi.renderer", "neocogi_render_core::abi"),
        ("neocogi.platform", "neocogi_render_core::platform"),
        ("neocogi.render.mesh", "neocogi_render_mesh::abi"),
        ("neocogi.gfx", "neocogi_gfx_support::abi"),
        ("neocogi.skin", "neocogi_appearance::skin"),
        ("neocogi.ui.core", "neocogi_ui::abi::core"),
        ("neocogi.ui.nodes", "neocogi_ui::abi::nodes"),
        ("neocogi.ui.document", "neocogi_ui::abi::document"),
        ("neocogi.demo", "neocogi_ui_samples::abi"),
    ];
    assert!(
        local.iter().all(|name| locations.iter().any(|(known, _)| known == name)),
        "unknown shared IDL module"
    );
    let repository = std::path::Path::new(repository);
    let mut request = neocogi_idl::cargo::SharedSources::new(repository.join("crates/render/core/schemas"));
    for relative in [
        "crates/render/core/schemas",
        "crates/gfx/schemas",
        "crates/skin/schemas",
        "crates/ui/schemas",
        "examples/schemas",
    ] {
        request = request.include_root(repository.join(relative));
    }
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
pub fn build(request: neocogi_idl::cargo::SharedSources) {
    for module in request.build().expect("generate shared IDL bindings") {
        println!(
            "cargo::rustc-env=IDL_{}={}",
            module.module_name.replace('.', "_").to_uppercase(),
            module.include.display()
        );
    }
}
