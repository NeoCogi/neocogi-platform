//! Generic graphics admission and optional concrete renderer backends.
//!
//! Semantic geometry, cameras and material shaders belong to clients. Public
//! resources have exclusive COM owners; commands retain immutable private data.
#![deny(missing_docs)]

// Opt-in acceptance fixtures use this same typed library API from native and browser callers.

#[cfg(all(test, not(target_arch = "wasm32"), any(feature = "glow", feature = "vulkan", feature = "wgpu")))]
use crate::gpu::testing::{derivatives as derivative_fixture, sampled as sampled_fixture};

pub mod backends;

mod budget;
pub mod command;
pub mod native;
mod pipeline;
// Native construction cleanup is crate-private; callers receive concrete completed resources.
pub mod record;
pub mod resource;
#[cfg(any(test, feature = "glow", feature = "webgl", feature = "vulkan"))]
mod resource_guard;
pub mod scope;
pub mod shader;
pub mod target;
/// Opt-in concrete probe family for CPU ownership tests; never selected by production activation.
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use resource::GpuDevice;

/// Canonical generated graphics contracts, without duplicated ABI definitions.
pub use neocogi_platform::render::abi;
/// Canonical upload producers live beside the contracts, independently of resource realization.
pub use neocogi_platform::render::source;

/// Admitted storage stays immutable even after the exclusive COM owner is gone.
/// Read-only access remains available if an Rc becomes uniquely owned or is unwrapped.
///
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_platform::gpu::resource::TextureVersion;
/// fn corrupt<B:neocogi_platform::gpu::native::NativeResources>(mut snapshot: Rc<TextureVersion<B>>) {
///     Rc::get_mut(&mut snapshot).unwrap().desc.size.width = 64;
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_platform::gpu::resource::BufferVersion;
/// fn corrupt<B:neocogi_platform::gpu::native::NativeResources>(snapshot: Rc<BufferVersion<B>>) {
///     let mut owned = Rc::try_unwrap(snapshot).ok().unwrap();
///     owned.bytes.clear();
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_platform::gpu::resource::ShaderVersion;
/// fn corrupt<B:neocogi_platform::gpu::native::NativeResources>(mut snapshot: Rc<ShaderVersion<B>>) {
///     Rc::get_mut(&mut snapshot).unwrap().artifact.glsl.clear();
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_platform::gpu::resource::PipelineVersion;
/// fn corrupt<B:neocogi_platform::gpu::native::NativeResources>(mut snapshot: Rc<PipelineVersion<B>>) {
///     Rc::get_mut(&mut snapshot).unwrap().info.color_write = 0;
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_platform::gpu::record::Block;
/// fn corrupt<B:neocogi_platform::gpu::native::NativeResources>(mut snapshot: Rc<Block<B>>) {
///     Rc::get_mut(&mut snapshot).unwrap().commands.clear();
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_platform::gpu::record::Block;
/// fn corrupt<B:neocogi_platform::gpu::native::NativeResources>(mut snapshot: Rc<Block<B>>) {
///     Rc::get_mut(&mut snapshot).unwrap().root.target.width = 4096;
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_platform::gpu::command::{Command, UniformData};
/// use neocogi_platform::gpu::record::Block;
/// fn corrupt<B:neocogi_platform::gpu::native::NativeResources>(block: Rc<Block<B>>) {
///     let Command::Draw(draw) = &block.commands()[0] else { return };
///     let mut uniform = draw.uniforms[0].data.clone();
///     drop(block);
///     Rc::get_mut(&mut uniform).unwrap().bytes.resize(4096, 0);
/// }
/// ```
/// ```
/// use neocogi_platform::gpu::{record::Block, resource::TextureVersion};
/// fn inspect<B:neocogi_platform::gpu::native::NativeResources>(block: &Block<B>, image: &TextureVersion<B>) {
///     let _ = (block.root(), block.commands(), image.desc(), image.pixels());
/// }
/// ```
pub mod immutable_storage_contract {}
