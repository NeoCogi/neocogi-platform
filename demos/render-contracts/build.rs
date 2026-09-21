//! Builds the demo renderer-extension contract and its narrow renderer dependency.

/// Generates Rust bindings and the fingerprint-qualified native description.
fn main() {
    // The build helper resolves `neocogi.renderer` from Cargo metadata, keeping
    // the demo protocol independent of UI widgets and their transitive modules.
    neocogi_idl::cargo::Module::new("idl", "neocogi/demo/render.ncidl")
        .build()
        .expect("failed to build neocogi.demo.render");
}
