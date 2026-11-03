//! Generate the portable shader namespace.
use neocogi_platform_build::{Module, build, sources};

/// Generate independent shader bindings from the packaged source owner.
fn main() {
    sources(&[Module::Shader]).and_then(build).expect("generate shader bindings");
}
