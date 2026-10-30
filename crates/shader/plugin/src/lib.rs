//! CPU-only portable shader compiler provider. Reports and every export are immutable;
//! no result retains the root, a renderer, a native context or a mutable session.
#![deny(missing_docs)]
mod compiler;
mod objects;
/// Stateless COM root, also constructible by statically linked CPU consumers.
pub use objects::CompilerObject;
#[cfg(test)]
mod tests;
use neocogi_com::{ComPtr, NcStatus};
use neocogi_shader_core::abi::ShaderCompilerAbi;

/// Activates the stateless compiler without requiring a graphics device.
#[neocogi_com::plugin(name = "neocogi.shader.compiler")]
fn acquire_root(_runtime: neocogi_com::Runtime) -> Result<ComPtr<ShaderCompilerAbi>, NcStatus> {
    objects::CompilerObject::from_value(objects::CompilerObject).query()
}
