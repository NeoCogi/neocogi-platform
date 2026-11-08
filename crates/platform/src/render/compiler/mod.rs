//! Renderer-owned shader translation. Each compiler instance has one immutable output
//! format chosen by its renderer; requests cannot change it. No compiler plugin is registered.
#![deny(missing_docs)]
mod compile;
mod objects;
/// Fixed-format CPU compiler and its owned COM result graph.
pub use objects::CompilerObject;
