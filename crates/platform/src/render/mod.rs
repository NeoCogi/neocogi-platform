//! The renderer implements the canonical contracts and owns shader translation.
#![deny(missing_docs)]

/// Shared contract types and helpers, identical to those used by dynamic applications.
pub use neocogi_platform_contracts::render::*;
/// Built-in S-expression COM compiler with an optional Python-like frontend and renderer-selected writers.
pub mod compiler;
