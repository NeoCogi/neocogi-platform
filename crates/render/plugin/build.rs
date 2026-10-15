//! Links this native plugin to the one Cargo-owned shared runtime when deployed dynamically.

/// Feature unification may enable shared-runtime imports even in an object-only
/// provider. Declare its native linkage at the DLL boundary, never in ABI crates.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    #[cfg(feature = "dynamic-plugin")]
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32") {
        let directory = std::env::var_os("CARGO_CDYLIB_DIR_COM_LIBRARY").expect("dynamic plugins require Cargo's com-library artifact");
        println!("cargo::rustc-link-search=native={}", std::path::Path::new(&directory).display());
    }
}
