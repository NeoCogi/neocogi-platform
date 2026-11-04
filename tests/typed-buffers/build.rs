//! Compile a consumer-owned specialization using the real Platform source package.
use neocogi_platform_build::{sources, build};

/// Derive a local trait alias from the generated declaration, never a copied UUID.
fn main() {
    let modules = build(
        sources(&[])
            .unwrap()
            .include_root("schemas")
            .module("tests.typed_buffer", "fixture_typed_buffers::tests::typed_contracts"),
    )
    .unwrap();
    let module = modules.iter().find(|module| module.module_name == "tests.typed_buffer").unwrap();
    let syntax = syn::parse_file(&std::fs::read_to_string(&module.bindings).unwrap()).unwrap();
    let name = syntax
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Trait(item) if item.ident.to_string().starts_with("ITypedGpuBufferInstance") => Some(item.ident.to_string()),
            _ => None,
        })
        .expect("closed typed interface is generated in its consumer");
    let alias = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("typed_trait.rs");
    std::fs::write(
        &alias,
        format!("/// Consumer-local name for the generated closed typed contract.\nuse typed_contracts::{name} as TypedBufferContract;\n"),
    )
    .unwrap();
    println!("cargo::rustc-env=IDL_TYPED_BUFFER_TRAIT={}", alias.display());
}
