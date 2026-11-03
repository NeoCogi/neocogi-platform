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

/// Reads one compiler report without consulting shared provider state.
/// A failing text getter cannot replace the original compilation status.
pub fn check_compilation(report: &abi::IShaderCompilationClient) -> Result<(), neocogi_render_core::abi::RenderError> {
    use neocogi_com::{INcStringComPtrExt, NcStatus};
    use neocogi_render_core::abi::RenderError;
    let status = NcStatus::from_code(report.status().map_err(RenderError::from)?);
    if status.is_ok() {
        return Ok(());
    }
    let message = match report.diagnostic().and_then(|text| text.to_string_bounded(MAX_DIAGNOSTIC_BYTES)) {
        Ok(text) if !text.is_empty() => text,
        Ok(_) => format!("shader compilation failed: {status}"),
        Err(detail) => format!("shader compilation failed: {status}; error text unavailable: {detail}"),
    };
    Err(RenderError::new(status, message))
}

impl abi::ShaderCompilerClient {
    /// Returns complete immutable artifacts or an owned compilation failure.
    /// The COM transport exposes `compile_result`; this facade reads that same report
    /// before returning, so callers never perform a separate last-error lookup.
    pub fn compile(
        &self,
        request: abi::ShaderCompileRequest,
        source: neocogi_com::NcCopyFromBufferOffset<'_, u8>,
        entry: neocogi_com::NcCopyFromBufferOffset<'_, u8>,
    ) -> Result<neocogi_com::ComPtr<abi::IShaderCompilationAbi>, neocogi_render_core::abi::RenderError> {
        use neocogi_render_core::abi::RenderError;
        let report = self
            .compile_result(request, source, entry)
            .map_err(|status| RenderError::from(status).context("compile shader"))?;
        check_compilation(&report)?;
        Ok(report)
    }
}
