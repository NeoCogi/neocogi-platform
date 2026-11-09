//! Canonical PCM audio bindings, validation, limits and owned diagnostics.
//!
//! The independent neocogi.audio namespace imports no renderer or window contract.
//! Platform discovery refers to these exact types instead of defining audio in its namespace.
#![deny(missing_docs)]

include!(env!("IDL_NEOCOGI_AUDIO"));

mod helpers;
pub use helpers::*;
