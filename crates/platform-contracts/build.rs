//! Generates renderer/audio/platform contracts and the resolved optional-source alias.
#[path = "build_support/schema.rs"]
mod schema;
use schema::{Module, build, sources};

/// Generate this package's namespaces from its declared build dependency.
fn main() {
    let modules = sources(&[Module::Renderer, Module::Audio, Module::Platform])
        .and_then(build)
        .expect("generate renderer/audio/platform bindings");
    let renderer = modules.iter().find(|module| module.module_name == "neocogi.renderer").expect("renderer module");
    let syntax = syn::parse_file(&std::fs::read_to_string(&renderer.bindings).expect("renderer bindings")).expect("generated Rust");
    let source = syntax
        .items
        .iter()
        .find_map(|item| {
            let syn::Item::Trait(interface) = item else { return None };
            if interface.ident != "ITextureFactory" {
                return None;
            }
            interface.items.iter().find_map(|item| {
                let syn::TraitItem::Fn(method) = item else { return None };
                if method.sig.ident != "create_texture_result" {
                    return None;
                }
                let syn::FnArg::Typed(argument) = method.sig.inputs.last()? else {
                    return None;
                };
                let syn::Type::Path(path) = &*argument.ty else { return None };
                let syn::PathArguments::AngleBracketed(arguments) = &path.path.segments.last()?.arguments else {
                    return None;
                };
                let syn::GenericArgument::Type(value) = arguments.args.last()? else {
                    return None;
                };
                Some(quote::quote!(#value).to_string())
            })
        })
        .expect("resolved checked optional source argument");
    // Derive the specialization from the actual generated method, never from a UUID spelling.
    let alias = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("initial_gpu_data.rs");
    std::fs::write(
        &alias,
        format!("/// Checked optional initial GPU data from the current generated contract.\npub type InitialGpuData = {source};\n"),
    )
    .expect("optional source alias");
    println!("cargo::rustc-env=IDL_INITIAL_GPU_DATA={}", alias.display());
}
