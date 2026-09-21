//! Generates typed path, input, window, and IME bindings for `neocogi.platform`.

/// Resolves the checked-in module directory and delegates deterministic generation.
fn main() {
    // The normalized directory owns one module identity regardless of how its
    // declarations are divided between input and native-window source files.
    neocogi_idl::cargo::Module::directory("idl", "neocogi/platform")
        .build()
        .expect("failed to build neocogi.platform");
}
