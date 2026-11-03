//! Generate an independent caller projection for ABI regressions.
use neocogi_platform_build::{Module, build, sources};

/// Generate the independent caller without borrowing another crate's generated output.
fn main() {
    build(sources(&[Module::Shader])).expect("generate independent shader caller");
}
