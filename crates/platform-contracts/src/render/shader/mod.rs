//! Immutable portable shader contracts, plain snapshots and bounded ABI import.
#![deny(missing_docs)]
use neocogi_platform_contracts::render::abi::GpuShaderStage;
pub use abi::{ShaderScalar, ShaderInterpolation, ShaderSampling, ShaderTarget};
// The renderer namespace owns the generated compiler contracts; this module owns
// only Rust snapshots, reflection and bounded readers of those same ABI definitions.
use crate::render::abi;
/// Maximum source bytes admitted before parsing.
pub const MAX_SHADER_BYTES: usize = 1024 * 1024;
/// Selected code plus entry-point payload limit; input and reflection have separate bounds.
pub const MAX_SHADER_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
/// Maximum diagnostic UTF-8 bytes, truncated at a character boundary.
pub const MAX_DIAGNOSTIC_BYTES: usize = 4096;
/// A known target was not enabled in this compiler build; no alternative is emitted.
pub const SHADER_E_TARGET_UNAVAILABLE: neocogi_com::NcStatus = neocogi_com::NcStatus::from_code(-3100);
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
    /// Native block name only for GLSL; absent for other targets.
    pub gl_block: Option<String>,
}

/// Combined texture/sampler binding in emitted GLSL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampledTexture {
    /// Stage-local slot shared by the texture and its corresponding sampler.
    pub slot: u32,
    /// Native combined-sampler name only for GLSL; absent for other targets.
    pub gl_name: Option<String>,
}

/// Validated CPU artifacts; immutable snapshots may outlive the public shader owner.
#[derive(Debug)]
pub struct Shader {
    /// Selected graphics stage.
    pub stage: GpuShaderStage,
    /// Selected WGSL/SPIR-V entry point.
    pub entry: String,
    /// Exactly one complete backend artifact; no parallel or fallback representation.
    pub code: ShaderCode,
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
        self.entry.len() + self.code.byte_len()
    }
}

/// One immutable backend representation. Text and aligned words retain distinct types.
#[derive(Debug)]
pub enum ShaderCode {
    /// Generated validated WGSL for WGPU.
    Wgsl(String),
    /// Guarded Vulkan words in aligned owned storage.
    SpirV(Vec<u32>),
    /// GLSL ES300 with explicit bounds guards and standard coordinate conventions.
    GlslEs300(String),
}
impl ShaderCode {
    /// Format derived from the actual owned variant, never a separate mutable tag.
    pub fn target(&self) -> ShaderTarget {
        match self {
            Self::Wgsl(_) => ShaderTarget::Wgsl,
            Self::SpirV(_) => ShaderTarget::SpirV,
            Self::GlslEs300(_) => ShaderTarget::GlslEs300,
        }
    }
    /// Exact exported byte length; compiler/import bounds precede construction.
    pub fn byte_len(&self) -> usize {
        match self {
            Self::Wgsl(text) | Self::GlslEs300(text) => text.len(),
            Self::SpirV(words) => words.len() * 4,
        }
    }
    /// Selected WGSL; other variants cannot substitute for this backend.
    pub fn wgsl(&self) -> Option<&str> {
        match self {
            Self::Wgsl(text) => Some(text),
            _ => None,
        }
    }
    /// Selected GLSL ES300; internal intermediate words are never retained here.
    pub fn glsl(&self) -> Option<&str> {
        match self {
            Self::GlslEs300(text) => Some(text),
            _ => None,
        }
    }
    /// Selected aligned SPIR-V; text is never reinterpreted as binary.
    pub fn spirv(&self) -> Option<&[u32]> {
        match self {
            Self::SpirV(words) => Some(words),
            _ => None,
        }
    }
}

mod read;
pub use read::PreparedShader;

/// Opt-in portable-profile inputs shared by compiler and GPU acceptance tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

/// Reads one compiler report without consulting shared provider state.
/// A failing text getter cannot replace the original compilation status.
pub fn check_compilation(report: &abi::IShaderCompilationClient) -> Result<(), neocogi_platform_contracts::render::abi::RenderError> {
    use neocogi_com::{INcStringComPtrExt, NcStatus};
    use neocogi_platform_contracts::render::abi::RenderError;
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
    ) -> Result<neocogi_com::ComPtr<abi::IShaderCompilationAbi>, neocogi_platform_contracts::render::abi::RenderError> {
        use neocogi_platform_contracts::render::abi::RenderError;
        let report = self
            .compile_result(request, source, entry)
            .map_err(|status| RenderError::from(status).context("compile shader"))?;
        check_compilation(&report)?;
        Ok(report)
    }
}
