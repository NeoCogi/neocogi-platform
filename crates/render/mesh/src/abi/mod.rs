//! Generated reusable mesh contracts shared by hosts and renderer plugins.
//!
//! External math types are converted by ordinary Rust helper implementations;
//! the ABI itself contains only IDL-owned copied vector and matrix layouts.

#![deny(missing_docs)]

include!(env!("IDL_NEOCOGI_RENDER_MESH"));

mod helpers;
