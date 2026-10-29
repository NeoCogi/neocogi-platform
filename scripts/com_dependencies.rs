//! Registry dependency declarations for independently compiled repository fixtures.
//!
//! Fixtures must use the workspace's exact published COM version: mixing registry
//! and checkout sources gives otherwise identical Rust types different package
//! identities. Keep the version pin in the root manifest alone.

/// Copies one inline workspace registry dependency and selects the fixture's features.
///
/// The root manifest deliberately keeps these entries on one line. Failing here
/// after a manifest format change is preferable to silently selecting a second
/// SDK version or enabling the facade's default macro/runtime features.
pub fn dependency(package: &str, features: &[&str]) -> String {
    let prefix = format!("{package} = ");
    let declaration = include_str!("../Cargo.toml")
        .lines()
        .find(|line| line.starts_with(&prefix))
        .expect("COM dependency is declared in the workspace");
    assert!(
        declaration.contains("version = \"=")
            && !["git", "rev", "path", "registry", "registry-index"]
                .iter()
                .any(|key| declaration.contains(&format!("{key} = "))),
        "COM fixtures require an exact crates.io version"
    );
    let table = declaration.trim_end().strip_suffix('}').expect("inline COM dependency table");
    format!("{table}, features = {features:?} }}\n")
}
