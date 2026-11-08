//! The renderer implements the canonical contracts and owns shader translation.
#![deny(missing_docs)]

/// Shared contract types and helpers, identical to those used by dynamic applications.
pub use neocogi_platform_contracts::render::*;
/// Renderer-owned Naga compiler configured once for its backend's output format.
#[cfg(any(test, feature = "shader-compiler"))]
pub mod compiler;
