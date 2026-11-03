//! Internal construction support shared by native renderer implementations.
//!
//! Shared frame validation owns only copied contracts and typed resource records. Native application
//! renderers continue to use their concrete backend APIs inside the selected renderer plugin.

pub mod com;

/// Physical pixel wheel conversion shared by native and browser platform hosts.
pub mod wheel;
