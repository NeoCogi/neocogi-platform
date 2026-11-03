//! Immutable portable shader contracts, plain snapshots and bounded ABI import.
#![deny(missing_docs)]
extern crate self as neocogi_shader_core;
use neocogi_render_core::abi::GpuShaderStage;
pub use abi::{ShaderScalar, ShaderInterpolation, ShaderSampling};
/// Generated versioned CPU compiler contracts.
#[allow(
    clippy::len_without_is_empty,
    reason = "IDL collections expose len/get; checking zero needs no additional ABI call"
)]
pub mod abi {
    include!(env!("IDL_NEOCOGI_SHADER"));
}
/// Required CPU provider identity for renderer activation.
pub const SHADER_COMPILER_PLUGIN_ID: neocogi_com::NcGuid = neocogi_com::guid_from_qualified_name("neocogi.shader.compiler");
/// Maximum source bytes admitted before parsing.
pub const MAX_SHADER_BYTES: usize = 1024 * 1024;
/// Combined source, entry, GLSL and SPIR-V payload limit; reflection is bounded separately.
pub const MAX_SHADER_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
/// Maximum diagnostic UTF-8 bytes, truncated at a character boundary.
pub const MAX_DIAGNOSTIC_BYTES: usize = 4096;
/// Scalar/vector shape compared across independent shader modules and vertex layouts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IoShape {
    /// Floating-point or unsigned integer scalar family.
    pub kind: ShaderScalar,
    /// One for scalars, otherwise the vector width.
    pub components: u8,
}

/// One user location in an entry point's input/output interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IoLocation {
    /// Portable location, below sixteen.
    pub location: u32,
    /// Required scalar/vector interpretation.
    pub shape: IoShape,
    /// Cross-stage interpolation must agree even when scalar/vector shapes match.
    pub interpolation: ShaderInterpolation,
    /// Cross-stage sampling qualifier; matching names alone do not establish linkage.
    pub sampling: ShaderSampling,
}

/// Uniform metadata copied out of compiler IR before publication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uniform {
    /// Stage-local slot, below four.
    pub slot: u32,
    /// Exact WGSL uniform byte length; native end padding is separate.
    pub byte_len: u32,
    /// Reflected generated GLSL block name; never an application-facing convention.
    pub gl_block: String,
}

/// Combined texture/sampler binding in emitted GLSL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampledTexture {
    /// Stage-local slot shared by the texture and its corresponding sampler.
    pub slot: u32,
    /// Reflected GLSL combined-sampler uniform name.
    pub gl_name: String,
}

/// Validated CPU artifacts; immutable snapshots may outlive the public shader owner.
#[derive(Debug)]
pub struct Shader {
    /// Selected graphics stage.
    pub stage: GpuShaderStage,
    /// Selected WGSL/SPIR-V entry point.
    pub entry: String,
    /// Original canonical source used by WGPU.
    pub wgsl: String,
    /// GL ES 300 / WebGL 2 source, including normalized clip-space conversion.
    pub glsl: String,
    /// Vulkan artifact with the same binding and clip conventions.
    pub spirv: Vec<u32>,
    /// Active uniform blocks in this entry point only.
    pub uniforms: Vec<Uniform>,
    /// Active texture/sampler pairs in this entry point only.
    pub textures: Vec<SampledTexture>,
    /// Location-based inputs, including vertex attributes or fragment varyings.
    pub inputs: Vec<IoLocation>,
    /// Location-based outputs; fragment output is restricted to color location zero.
    pub outputs: Vec<IoLocation>,
}

impl Shader {
    /// Exact code-payload charge; bounded readers establish arithmetic safety first.
    pub fn byte_len(&self) -> usize {
        self.wgsl.len() + self.entry.len() + self.glsl.len() + self.spirv.len() * 4
    }
}

mod read;
pub use read::PreparedShader;

/// Opt-in portable-profile inputs shared by compiler and GPU acceptance tests.
#[cfg(feature = "test-support")]
pub mod testing;
