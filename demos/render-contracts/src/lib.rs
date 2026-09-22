//! Generated demo mesh contracts shared by hosts and renderer plugins.
//!
//! External math types are converted by ordinary Rust helper implementations;
//! the ABI itself contains only IDL-owned copied vector and matrix layouts.

#![no_std]
#![deny(missing_docs)]

include!(concat!(env!("OUT_DIR"), "/neocogi_demo_render.include.rs"));

mod helpers;
