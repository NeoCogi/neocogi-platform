//! Builds the renderer-neutral contract module from its documented IDL parts.

/// Generates Rust bindings and the fingerprint-qualified native description.
fn main() {
    // The directory path, rather than any individual source filename, owns the
    // stable `neocogi.renderer` module identity used by every consuming plugin.
    neocogi_idl_build::Module::directory("idl", "neocogi/renderer")
        .build()
        .expect("failed to build neocogi.renderer");
}
