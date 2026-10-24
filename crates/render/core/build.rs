//! Generates this library's bindings from subsystem-owned shared sources.
#[path = "../../../scripts/idl_build.rs"]
mod shared_idl;
fn main() {
    shared_idl::build(shared_idl::sources("../../..", &["neocogi.renderer", "neocogi.platform"]));
}
