//! Generic graphics data and admission, shared by concrete renderer backends.
//!
//! Semantic geometry, cameras and material shaders belong to clients. Public
//! resources have exclusive COM owners; commands retain immutable private data.
#![deny(missing_docs)]

pub mod shader;

/// Canonical generated graphics contracts, without duplicated ABI definitions.
pub use neocogi_render_core::abi;
