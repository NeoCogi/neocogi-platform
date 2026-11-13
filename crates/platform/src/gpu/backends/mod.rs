//! Concrete graphics families behind mutually exclusive renderer features.
//!
//! Each module owns its API-specific handles, preparation, submission and retirement. Native
//! families retain distinct concrete resource types and driver identities. The builtin platform
//! selects one family per build; Glow is the default. Audio and window hosting share that build.

/// Shared OpenGL ES/WebGL executor, including resource-owned physical context lifetime.
#[cfg(any(feature = "glow", feature = "webgl"))]
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
