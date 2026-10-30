//! Generic graphics data and admission, shared by concrete renderer backends.
//!
//! Semantic geometry, cameras and material shaders belong to clients. Public
//! resources have exclusive COM owners; commands retain immutable private data.
#![deny(missing_docs)]

mod budget;
pub mod command;
mod pipeline;
pub mod record;
pub mod resource;
pub mod scope;
pub mod shader;

pub use resource::GpuDevice;

/// Canonical generated graphics contracts, without duplicated ABI definitions.
pub use neocogi_render_core::abi;
/// Canonical upload producers live beside the contracts, independently of resource realization.
pub use neocogi_render_core::source;

/// Admitted storage stays immutable even after the exclusive COM owner is gone.
/// Read-only access remains available if an Rc becomes uniquely owned or is unwrapped.
///
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_render_gpu::resource::TextureVersion;
/// fn corrupt(mut snapshot: Rc<TextureVersion>) {
///     Rc::get_mut(&mut snapshot).unwrap().desc.size.width = 64;
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_render_gpu::resource::BufferVersion;
/// fn corrupt(snapshot: Rc<BufferVersion>) {
///     let mut owned = Rc::try_unwrap(snapshot).ok().unwrap();
///     owned.bytes.clear();
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_render_gpu::resource::ShaderVersion;
/// fn corrupt(mut snapshot: Rc<ShaderVersion>) {
///     Rc::get_mut(&mut snapshot).unwrap().artifact.glsl.clear();
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_render_gpu::resource::PipelineVersion;
/// fn corrupt(mut snapshot: Rc<PipelineVersion>) {
///     Rc::get_mut(&mut snapshot).unwrap().id = 0;
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_render_gpu::record::Block;
/// fn corrupt(mut snapshot: Rc<Block>) {
///     Rc::get_mut(&mut snapshot).unwrap().commands.clear();
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_render_gpu::record::Block;
/// fn corrupt(mut snapshot: Rc<Block>) {
///     Rc::get_mut(&mut snapshot).unwrap().root.target.width = 4096;
/// }
/// ```
/// ```compile_fail
/// use std::rc::Rc;
/// use neocogi_render_gpu::command::{Command, UniformData};
/// use neocogi_render_gpu::record::Block;
/// fn corrupt(block: Rc<Block>) {
///     let Command::Draw(draw) = &block.commands()[0] else { return };
///     let mut uniform = draw.uniforms[0].data.clone();
///     drop(block);
///     Rc::get_mut(&mut uniform).unwrap().bytes.resize(4096, 0);
/// }
/// ```
/// ```
/// use neocogi_render_gpu::{record::Block, resource::TextureVersion};
/// fn inspect(block: &Block, image: &TextureVersion) {
///     let _ = (block.root(), block.commands(), image.id(), image.desc(), image.pixels());
/// }
/// ```
pub mod immutable_storage_contract {}
