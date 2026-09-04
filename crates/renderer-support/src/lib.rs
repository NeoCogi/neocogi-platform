//! Internal construction support shared by native renderer implementations.
//!
//! This crate intentionally contains no scene, mesh, material, command, or renderer abstraction.
//! Backends expose their native application extension directly and depend here only on the small
//! transactional resource guard used during fallible driver-object construction.

#[doc(hidden)]
pub mod resource_guard;
