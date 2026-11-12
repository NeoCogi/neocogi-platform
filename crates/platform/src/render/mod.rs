//! The renderer implements the canonical contracts and owns shader translation.
#![deny(missing_docs)]

/// Shared contract types and helpers, identical to those used by dynamic applications.
pub use neocogi_platform_contracts::render::*;
/// Optional linked S-expression COM compiler; dynamic platforms receive the same factory from the host.
#[cfg(any(test, feature = "shader-compiler"))]
pub mod compiler;
