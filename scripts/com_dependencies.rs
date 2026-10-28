//! Git dependency declarations for independently compiled repository fixtures.
//!
//! Fixtures must use the workspace's exact COM source and revision: mixing a
//! checkout SDK with a Git SDK gives otherwise identical Rust types different
//! package identities. Keep the pin in the root manifest alone.

/// Copies one inline workspace Git dependency and selects the fixture's features.
///
/// The root manifest deliberately keeps these entries on one line. Failing here
/// after a manifest format change is preferable to silently selecting a second
/// SDK revision or enabling the facade's default macro/runtime features.
pub fn dependency(package: &str, features: &[&str]) -> String {
    let prefix = format!("{package} = ");
    let declaration = include_str!("../Cargo.toml")
        .lines()
        .find(|line| line.starts_with(&prefix))
        .expect("COM dependency is declared in the workspace");
    assert!(
        declaration.contains("git = ") && declaration.contains("rev = "),
        "COM fixtures require a pinned Git dependency"
    );
    let table = declaration.trim_end().strip_suffix('}').expect("inline COM dependency table");
    format!("{table}, features = {features:?} }}\n")
}
