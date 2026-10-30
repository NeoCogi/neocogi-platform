//! Explicit, non-asserting measurements of real UI recording and native execution.
use super::*;
use std::time::Instant;
pub(crate) mod calls;

/// Owned metadata for one measurement; never enters a renderer ABI.
pub(crate) struct Sample {
    /// Window/assets/UI/client construction, including CPU shader compilation.
    pub(crate) setup_ns: u128,
    /// Real retained painting into authenticated immutable blocks.
    pub(crate) record_ns: u128,
    /// Includes a separately owned procedural client contribution.
    pub(crate) mixed: bool,
    /// Actual admitted commands in painter order.
    pub(crate) commands: usize,
    /// Actual draws, excluding regional clears.
    pub(crate) draws: usize,
    /// Distinct logical vertex/index versions.
    pub(crate) buffers: usize,
    /// Distinct logical sampled image versions.
    pub(crate) textures: usize,
    /// Distinct logical pipelines; native links are counted separately.
    pub(crate) pipelines: usize,
}

