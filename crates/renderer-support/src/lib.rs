//! Internal construction support shared by native renderer implementations.
//!
//! Shared frame validation owns only copied contracts and typed resource records. Native application
//! renderers continue to use their concrete backend APIs inside the selected platform plugin.

pub mod com;

#[doc(hidden)]
pub mod resource_guard;

/// Physical pixel wheel conversion shared by native and browser platform hosts.
pub mod wheel;
