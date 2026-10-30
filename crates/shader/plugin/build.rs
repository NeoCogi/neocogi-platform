//! Generate an independent caller projection for ABI regressions.
#[path = "../../../scripts/idl_build.rs"]
mod shared_idl;
fn main() {
    shared_idl::build(shared_idl::sources("../../..", &["neocogi.shader"]));
}
