//! Concrete graphics families behind additive Cargo features.
//!
//! Each module owns its API-specific handles, preparation, submission and retirement. Native
//! families may coexist in one library while retaining distinct concrete resource types and driver
//! identities. The platform assembly chooses one family per build. Empty default features keep
//! CPU admission/recording and virtual fixtures independent of SDL and graphics libraries.

/// Shared OpenGL ES/WebGL executor, including resource-owned physical context lifetime.
#[cfg(feature = "glow")]
pub mod glow;
/// Vulkan execution, native allocations and completion-fenced retirement.
#[cfg(all(feature = "vulkan", not(target_arch = "wasm32")))]
pub mod vulkan;
/// Browser canvas adapter sharing the GL executor and concrete resource family.
#[cfg(feature = "webgl")]
pub mod webgl;
/// Native WGPU execution, scoped validation and queue-completion retirement.
#[cfg(all(feature = "wgpu", not(target_arch = "wasm32")))]
pub mod wgpu;
